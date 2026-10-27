//! The one door the native shell walks through (`S.U7b`, unblocking `S.U9`).
//!
//! `plan-opus.md` § `O.12.b′` puts one rule ahead of every panel: *"no panel
//! calls an HTTP handler or its body directly. It calls the `S.U4` layer"*.
//! HTTP already does — `main.rs::serve` walks [`super::ROUTES`].
//! The shell could not, until now, because [`crate::Shared`] (the app state,
//! the painter, the digest input) was `!Send` and therefore could not be
//! shared between the winit thread the shell runs on and the listener
//! thread that owned it — see `Shared`'s own doc comment for the two bounds
//! that were missing (`metal_render::model::Model: Send`, a `cpal::Stream`
//! opened off its owning thread) and are fixed by this same batch.
//!
//! [`Door`] is the shape that unblocking takes: it holds what `serve` always
//! held — a lock on the shared state and a reference to the absorbed
//! studio — and its one real method, [`Call::call`], is exactly
//! [`super::dispatch`] with the socket-facing parts (`main.rs::serve`'s own
//! job) stripped away. [`Call`] is a trait, not just an inherent method on
//! `Door`, so a kittest test can hand `Shell` a `FakeDoor` that records what
//! was called instead of building real renderers per test — see
//! `shell::kittest`'s own tests from `S.U8` onward for that fixture, and
//! `door::tests::the_door_is_send_and_locks_per_call` below for the one this
//! batch owes: that `Door` itself really is `Send` and really does route
//! through [`super::dispatch`], not a second copy of a handler's body.
//!
//! **Nothing here is a route.** No handler body moved into this file — every
//! one already lives under `call/{ai,graph,outputs,overlay,page,render,
//! transmit}.rs` (`S.U4a`) and `call::author` (`the cleanup`), and `Door::call`
//! reaches every one of them through the same [`super::dispatch`] `serve`
//! calls. Adding a route here instead of to [`super::ROUTES`] would be the
//! exact second path `O.12.b′` exists to forbid, one level further in.

use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::{Author, Shared};

use super::{Ctx, Params, Reply};

/// What a caller that is not `main.rs::serve` needs: a way to run one
/// request through [`super::dispatch`] without touching a socket. `Door`
/// implements it against the real, locked app state; a test double
/// implements it against a `Vec` of calls it recorded — see `shell::kittest`
/// (`S.U8`+) for that fixture. [`json`](Call::json) is a default method so
/// both get it for free.
pub trait Call {
    /// Runs one request through [`super::dispatch`] and returns what it
    /// returned — `None` for a `(method, path)` [`super::ROUTES`] has no row
    /// for, exactly as `main.rs::serve` treats an unrouted path as a 404
    /// itself rather than something [`super::dispatch`] answered.
    fn call(&self, method: &str, path: &str, params: Params) -> Option<Reply>;

    /// The JSON convenience every panel actually wants: give it a method, a
    /// path and an optional JSON body, get back the parsed reply or
    /// `(status, message)` on anything but 200. A route that answers
    /// [`super::Bytes`] (a `.jpg`) has nothing to parse
    /// here and reports 500 rather than panicking — no route a panel calls
    /// through this is one of those today, and if one ever needs to be, it
    /// is this method's job to grow a second one, not a panel's job to read
    /// `Reply::body` itself.
    // First called for real by `S.U8`'s header (`shell::header::paint`,
    // reading `/state` for `running` and `collections`, and calling
    // `POST /transport`).
    fn json(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, (u16, String)> {
        let params = Params {
            query: Default::default(),
            body: body.map(|v| v.to_string().into_bytes()).unwrap_or_default(),
            tail: None,
        };
        match self.call(method, path, params) {
            Some(reply) if reply.status == 200 => crate::json_memo::parse(&reply.body)
                .map_err(|e| (500, format!("{path} answered a body that is not JSON: {e}"))),
            Some(reply) => Err((
                reply.status,
                String::from_utf8_lossy(&reply.body).into_owned(),
            )),
            None => Err((404, format!("{method} {path} matched no route"))),
        }
    }

    /// A `POST` whose answer the caller ignores. A door with a background
    /// writer queues it there and returns at once; any other door runs it
    /// inline, exactly as `let _ = self.json("POST", path, Some(body))` does.
    fn post_async(&self, path: &str, body: Value) {
        let _ = self.json("POST", path, Some(body));
    }

    /// The open graph as a background writer read it back once its queued
    /// writes had settled, with the id it was read for (`None` when this door
    /// has no writer, or nothing new has landed). The canvas applies it in
    /// place of its own optimistic copy.
    fn graph_refresh(&self) -> Option<(String, Value)> {
        None
    }

    /// G3: the frame behind a picture route as a texture on the painter's own
    /// device (`/output.jpg`, `/preview.jpg`), when this door can give one -
    /// the window presents it without a JPEG or a readback. `None` otherwise
    /// (a test double, a CPU painter, a route with no texture form).
    #[cfg(feature = "gpu")]
    fn texture(&self, _path: &str, _query: &[(&str, String)]) -> Option<metal_perform::gpu_frame::GpuFrame> {
        None
    }

    /// The bytes convenience: give it a method, a path and a canned query,
    /// get back the 200 reply whole (the body is the bytes) — the seam the
    /// shell's first image consumer (`shell::frames::Frame`, S.U9b) needs,
    /// the same shape [`Call::json`] has for parsed JSON. A panel still
    /// never names a handler body; it names a route.
    fn bytes(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Reply, (u16, String)> {
        let mut q: std::collections::HashMap<String, String> = Default::default();
        for (k, v) in query {
            q.insert((*k).to_string(), v.clone());
        }
        let params = Params {
            query: q,
            body: Vec::new(),
            tail: None,
        };
        match self.call(method, path, params) {
            Some(reply) if reply.status == 200 => Ok(reply),
            Some(reply) => Err((
                reply.status,
                String::from_utf8_lossy(&reply.body).into_owned(),
            )),
            None => Err((404, format!("{method} {path} matched no route"))),
        }
    }
}

/// The real door: a lock on [`Shared`] and the absorbed studio, shared with
/// the listener thread (`main.rs::serve` locks the same `Arc<Mutex<Shared>>`
/// per request — see that function). Built once, in `main_windowed`, and
/// handed to [`crate::shell::window::run`], which is the only other place a
/// `Ctx` gets built outside `serve` itself.
///
/// `Clone` because there is now a second holder: `window::run` moves one
/// clone into the preview [`Fetcher`](crate::shell::frames::Fetcher)'s
/// worker thread and keeps one for the shell. Both hold the same two `Arc`s
/// and both walk the same [`Call::call`], so this is a second handle to one
/// door, not a second door — no handler body is duplicated by it.
/// The door the agent's executor uses (`chat::agent_tool_at`, chat turns):
/// installed once by `serve_forever`, which builds the same `Shared`/`Author`
/// pair the HTTP listener serves from. Unset in a test that builds none, and
/// then the door-bound tools answer "needs the app's door" as before.
#[cfg(not(target_arch = "wasm32"))]
static AGENT_DOOR: std::sync::OnceLock<Door> = std::sync::OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
pub fn install_agent_door(door: Door) {
    let _ = AGENT_DOOR.set(door);
}

/// A handle to the installed door, for one executor to hold.
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_door() -> Option<Box<dyn Call + Send>> {
    AGENT_DOOR.get().map(|d| Box::new(d.clone()) as Box<dyn Call + Send>)
}

/// Wraps a door as the closure `metal_agent::harness::Harness` uses to reach a
/// route (`Harness::with_routes`): a tool that must not touch a file itself
/// posts through the same door every panel calls through (rule 6). Only the
/// write routes use it, so it always posts.
#[cfg(all(feature = "agent", not(target_arch = "wasm32")))]
pub fn route_caller(door: Box<dyn Call + Send>) -> metal_agent::harness::RouteCall {
    let door = std::sync::Arc::new(std::sync::Mutex::new(door));
    std::sync::Arc::new(move |path: &str, body: &Value| {
        let door = door.lock().map_err(|_| "the door lock was poisoned".to_string())?;
        door.json("POST", path, Some(body.clone()))
            .map_err(|(status, message)| format!("{status}: {message}"))
    })
}

#[derive(Clone)]
pub struct Door {
    shared: Arc<Mutex<Shared>>,
    author: Arc<Author>,
}

impl Door {
    /// G3: a handle to the painter's device, when it composes on the GPU, so
    /// the window can share it.
    #[cfg(feature = "gpu")]
    pub fn shared_gpu(&self) -> Option<metal_render::gpu::Gpu> {
        crate::perf::locked(&self.shared, "door.shared").ok()?.paint.shared_gpu()
    }

    pub fn new(shared: Arc<Mutex<Shared>>, author: Arc<Author>) -> Door {
        Door { shared, author }
    }
}

/// The kind preview route: it needs the engine, so it is not an author route.
const KIND_PATH: &str = "/author/render/kind.jpg";

impl Call for Door {
    #[cfg(feature = "gpu")]
    fn texture(&self, path: &str, query: &[(&str, String)]) -> Option<metal_perform::gpu_frame::GpuFrame> {
        // G5: the author's solo picture draws on the painter's device, so it
        // is a texture here too (the painter takes the mask cut).
        if path == "/author/render/solo.jpg" {
            let mut guard = crate::perf::locked(&self.shared, "door.shared").ok()?;
            return self.author.solo_texture(guard.paint.as_mut(), query);
        }
        if path == "/author/render/grid.jpg" {
            // The grid is ~30 ms of tile renders on the studio's own state. It
            // needs only the painter's device handle, so the shell's lock on
            // `Shared` is held for that clone and no longer for the render.
            let cx = {
                let mut guard = crate::perf::locked(&self.shared, "door.shared").ok()?;
                guard.paint.gpu_cx()?
            };
            return self.author.grid_texture(&cx, query);
        }
        let params = Params {
            query: query.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect(),
            body: Vec::new(),
            tail: None,
        };
        let mut guard = crate::perf::locked(&self.shared, "door.shared").ok()?;
        let Shared { state, paint, sim } = &mut *guard;
        let mut ctx = Ctx { state, paint: paint.as_mut(), sim, author: &self.author };
        super::render::texture_of(&mut ctx, path, &params)
    }

    fn call(&self, method: &str, path: &str, params: Params) -> Option<Reply> {
        // `S.U11`: `/author/*` is a second table (`super::author::AUTHOR_ROUTES`,
        // `super::author::dispatch`) closed over its own `Ctx`, exactly the
        // split `main.rs::serve` already makes — see that function's own
        // comment on why (a second `State` type, gated on `gpu`, that the
        // top-level `Ctx` should not have to know about). `serve` intercepts
        // the `/author` prefix *before* consulting `super::dispatch`; this
        // does the same, so a panel that calls `door.json("GET",
        // "/author/songs", …)` reaches `Author::dispatch` — the same
        // function HTTP reaches — never a second, in-process copy of a
        // handler's body. `Author::dispatch` locks its own state and never
        // touches `Shared`, so this branch never takes the lock below.
        // `POST /agent/tool` runs without the state lock (see `agent_intercept`).
        // In-process, so it is authorised by construction: the bearer token
        // guards HTTP callers only.
        if let Some(reply) = super::agent_intercept(method, path, &params, true) {
            return Some(reply);
        }

        if (path == "/author" || path.starts_with("/author/")) && path != KIND_PATH {
            let rest = path.strip_prefix("/author").unwrap_or("");
            let inner_path = if rest.is_empty() {
                "/".to_string()
            } else {
                rest.to_string()
            };
            let inner_req = metal_web::Request {
                method: method.to_string(),
                path: inner_path,
                query: params.query,
                headers: Default::default(),
                body: params.body,
            };
            return Some(crate::perf::timed(method, path, || self.author.dispatch(&inner_req)));
        }

        // The same lock `serve` takes per request (`main.rs`) — a panel and
        // an inbound HTTP request never run a handler at the same instant,
        // which is the whole reason `Shared` moved behind a `Mutex` in this
        // batch rather than staying the listener thread's own local.
        let asked = web_time::Instant::now();
        let mut guard = crate::perf::locked(&self.shared, "door.shared").expect("shared lock poisoned");
        let wait = asked.elapsed();
        let Shared { state, paint, sim } = &mut *guard;
        let mut ctx = Ctx {
            state,
            paint: paint.as_mut(),
            sim,
            author: &self.author,
        };
        let reply = super::dispatch(method, path, &params, &mut ctx);
        drop(guard);
        crate::perf::call(method, path, wait, asked.elapsed() - wait);
        reply
    }
}

#[cfg(test)]
mod tests {
    use crate::chdir_lock;

    use super::*;

    fn assert_send<T: Send>() {}

    /// `S.U7b`'s own proof, in one test: `Door` is `Send` (so it can move
    /// into the winit thread `main_windowed` runs on) *and* a call made from
    /// another thread actually reaches a real handler and actually mutates
    /// the locked state — `POST /transport` is `call::transmit::transport`,
    /// which flips `State::paused_at` between `None` and `Some`. Building
    /// the fixture the same way `call::tests::every_route_dispatches` does
    /// (real painter, real `Author`, the cwd trick `Author::new()` needs) is
    /// what makes this a real `dispatch`, not a stand-in for one.
    #[test]
    fn the_door_is_send_and_locks_per_call() {
        assert_send::<Door>();

        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        let _chdir = chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = Arc::new(crate::Author::new());
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let paint = crate::paint::painter();
        let state = crate::State::new(paint.as_ref());
        let sim = crate::listeners::Listeners::stand_in();
        let shared = Arc::new(Mutex::new(Shared { state, paint, sim }));

        let door = Door::new(Arc::clone(&shared), Arc::clone(&author));
        let shared_check = Arc::clone(&shared);

        // Built on this thread, moved into another — `Send` is what makes
        // this line compile at all. What runs inside proves the call is
        // real rather than a stand-in: reading `Shared::state` straight off
        // the same `Arc<Mutex<…>>` (not the door's own JSON reply, whose
        // `/transport` body is just `{"ok": true}`) is what pins the claim
        // on `State::paused_at`, the field `call::transmit::transport`
        // actually flips — first to `Some` (paused), then back to `None`
        // (resumed).
        std::thread::spawn(move || {
            let first = door
                .call("POST", "/transport", Params::default())
                .expect("dispatched");
            assert_eq!(first.status, 200);
            assert!(
                shared_check.lock().expect("lock").state.paused_at.is_some(),
                "first /transport should have paused"
            );

            let second = door
                .call("POST", "/transport", Params::default())
                .expect("dispatched");
            assert_eq!(second.status, 200);
            assert!(
                shared_check.lock().expect("lock").state.paused_at.is_none(),
                "second /transport should have resumed"
            );
        })
        .join()
        .expect("door thread panicked");
    }
}
