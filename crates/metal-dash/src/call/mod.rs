//! The call layer (`O.12.b′`, batch `S.U4a`).
//!
//! Every arm that used to live in `main.rs`'s `serve` `match` now lives here
//! as a typed Rust function: no raw socket type, no parsed-HTTP-request type,
//! no wire-level reply helper. A handler takes `&mut Ctx` (which bundles the
//! app state, the painter, the digest input and the absorbed studio — a
//! handler reads only the fields its own body needs) plus [`Params`], the
//! request's query/body already lifted off the wire, and returns a typed
//! result: `Result<Value, CallError>` for a JSON route, `Result<Bytes,
//! CallError>` for an image or HTML one.
//!
//! **Why this exists**: `plan-opus.md` § `O.12.b′`. Once the native shell
//! lives inside this same process, the cheap thing for a panel to do is call
//! a handler's body directly — and that is the second path rule 6
//! forbids, one panel at a time and without anyone noticing. This module is
//! the one door both HTTP (`serve`, in `main.rs`) and the shell (`S.U5`
//! onward) walk through. `serve` is now a thin wrapper: it reads the socket,
//! builds `Params`, walks [`ROUTES`], and turns whatever a handler returns
//! into bytes on the wire — the only place a status code is decided.
//!
//! **Scope note (`S.U4a` vs `the cleanup`)**: `/author` (the exact path), `/author/`
//! and the `/author/*` prefix are still not rows of [`ROUTES`] — they are a
//! *second* table, [`author::AUTHOR_ROUTES`], closed over a different `Ctx`
//! (the absorbed studio's own `State`, gated on `gpu`) rather than this
//! module's. `main.rs::serve` keeps intercepting those three paths before
//! consulting [`ROUTES`], exactly as it did before `S.U4a`, but now hands
//! them to `Author::dispatch` (which walks `author::AUTHOR_ROUTES`) instead
//! of the old raw-socket `author::serve`. See `author::mod`'s own doc
//! comment for why a second table rather than folding the rows in here.

use std::collections::HashMap;

use serde_json::Value;

use crate::listeners::Listeners;
use crate::paint::Paint;
use crate::{Author, State};

#[cfg(feature = "agent")]
mod ai;
// The browser build has no AI harness (`metal-agent` is desktop-only): the same
// route rows answer with a refusal rather than a 404, which `tests` treats as
// a row that does not dispatch.
#[cfg(not(feature = "agent"))]
#[path = "ai_off.rs"]
mod ai;
#[cfg(feature = "gpu")]
pub mod author;
// `S.U7b`: the door the native shell calls through, now that `Shared` can be
// `Send` (see that struct's doc comment in `main.rs`). A submodule, not
// folded into this file, so gate 1 (`shell/gates.rs`) can allowlist exactly
// its re-exports below — `Door`/`Call`/`Params`/`Reply`/`CallError`/`Bytes`
// — and still ban `use crate::call::door::…` itself as a submodule reach,
// the same way it already bans `call::transmit::…`. `S.U15`: gated on
// `any(gpu, glow)`, not `gpu` alone — nothing in this file is gpu-specific
// (`Shared`/`Author` are `Send` either way, `Box<dyn Paint>` is the CPU
// stand-in on a build with neither), the door exists purely because
// `shell/` (also `any(gpu, glow)` now) needs something to call through.
mod blackout;
mod colour;
#[cfg(feature = "gpu")]
mod export;
#[cfg(not(feature = "gpu"))]
#[path = "export_off.rs"]
mod export;
#[cfg(any(feature = "gpu", feature = "glow", target_arch = "wasm32"))]
pub(crate) mod door; // `chdir_lock` (the cwd-fixture dance serializer) is shared with `shell::kittest`
mod files;
pub mod frame;
pub mod frame_cache;
pub(crate) mod graph;
mod loop_audio;
mod loops;
mod outputs;
mod overlay;
pub(crate) mod peers;
mod page;
pub(crate) mod render;
mod script;
mod sensor;
mod sequences;
mod shaders;
pub(crate) mod settings;
mod switch;
mod sync;
mod transmit;
// The in-app updater's two routes (`GET`/`POST /update`). The state and the
// JSON replies live in `crate::update`; this file is only the route pair, the
// same split `call::settings` makes.
mod update;
mod venue;
pub(crate) mod videoio;

#[cfg(any(feature = "gpu", feature = "glow", target_arch = "wasm32"))]
pub use door::{Call, Door};
#[cfg(all(any(feature = "gpu", feature = "glow"), not(target_arch = "wasm32")))]
pub use door::{agent_door, install_agent_door};

/// What every handler is handed beyond [`Params`]. A handler reads only the
/// fields its own body touches — `paint`/`sim`/`author` are here because the
/// table needs one function-pointer shape to hold rows this different, not
/// because every arm wants all four.
pub struct Ctx<'a> {
    pub state: &'a mut State,
    pub paint: &'a mut dyn Paint,
    pub sim: &'a mut Listeners,
    pub author: &'a Author,
}

/// A request's query string and body, already lifted off the socket by the
/// wrapper in `main.rs`. `tail` is the remainder of the path past a prefix
/// route's `path` (e.g. the device id in `/out/<device>`, the trigger id in
/// `/trigger/<id>`) — `dispatch` fills it in once it knows which row
/// matched, so a caller only has to provide `query`/`body`.
#[derive(Default, Clone)]
pub struct Params {
    pub query: HashMap<String, String>,
    pub body: Vec<u8>,
    pub tail: Option<String>,
}

impl Params {
    pub fn num(&self, key: &str, fallback: f32) -> f32 {
        self.query
            .get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(fallback)
    }

    pub fn param(&self, key: &str) -> Option<&str> {
        self.query.get(key).map(String::as_str)
    }

    /// The body as JSON, or `None` if it is empty or not JSON — same rule
    /// the wire-level request's own `.json()` used, so a handler moved here behaves exactly
    /// as it did as a match arm.
    pub fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

/// What a handler refuses with. Mapped to a status in exactly one place —
/// [`Reply::error`], which `dispatch` calls — never scattered across the
/// handlers themselves. `Debug` so a test's `Result` can `unwrap`/`expect`
/// against it — the same trade every route's tests make.
#[derive(Debug)]
pub enum CallError {
    /// 400: the caller's own fault — a missing field, an id that does not
    /// parse, an op this route does not know.
    Bad(String),
    /// 404: named the wrong thing (a device, a graph, a trigger) that does
    /// not exist.
    NotFound,
    /// 409: the request is well-formed but conflicts with what is already
    /// true (reserved for a handler that needs it; none does yet).
    #[allow(dead_code)]
    Conflict(String),
    /// 500: this app's own bug, not the caller's — a save that failed, an
    /// invariant that should have held.
    Internal(String),
    /// 401: `POST /agent/tool` without the install's bearer token.
    Unauthorized,
}

/// Bytes with a content type — what an image or HTML route returns instead
/// of JSON.
pub struct Bytes {
    pub kind: &'static str,
    pub body: Vec<u8>,
}

/// What `dispatch` hands back to the wrapper: a status and a body, already
/// serialised. No wire-level reply type appears here — the wrapper is the one
/// that knows how to write a status and a body to a socket.
pub struct Reply {
    pub status: u16,
    pub kind: &'static str,
    pub body: Vec<u8>,
}

impl Reply {
    fn json_ok(v: Value) -> Reply {
        Reply {
            status: 200,
            kind: "application/json",
            body: v.to_string().into_bytes(),
        }
    }

    fn bytes_ok(b: Bytes) -> Reply {
        Reply {
            status: 200,
            kind: b.kind,
            body: b.body,
        }
    }

    fn error(e: CallError) -> Reply {
        let (status, message) = match e {
            CallError::Bad(m) => (400, m),
            CallError::NotFound => (404, "not found".to_string()),
            CallError::Conflict(m) => (409, m),
            CallError::Internal(m) => (500, m),
            CallError::Unauthorized => (401, "unauthorized".to_string()),
        };
        Reply {
            status,
            kind: "text/plain; charset=utf-8",
            body: message.into_bytes(),
        }
    }

    /// The HTTP status line the wire-level reply helper wants (e.g.
    /// `"404 Not Found"`) — the one place that spells one out, so the
    /// wrapper in `main.rs` never has to know the mapping itself. Not a
    /// dependency on that helper's crate: this produces a plain `&str`,
    /// nothing that touches a socket. The reason phrase for 400 is
    /// deliberately not the RFC's own two words — only the status number is
    /// wire-relevant, and every JS harness this app answers to reads
    /// `response.status`, never the phrase beside it.
    pub fn status_line(&self) -> &'static str {
        match self.status {
            200 => "200 OK",
            400 => "400 Malformed",
            401 => "401 Unauthorized",
            404 => "404 Not Found",
            409 => "409 Conflict",
            _ => "500 Internal Server Error",
        }
    }
}

/// The few signature shapes every one of the 34 arms boiled down to. A
/// handler returning `Value` is a JSON route; one returning [`Bytes`] is an
/// image or HTML route (its `kind` decides the header, so both live under
/// one variant).
#[derive(Clone, Copy)]
pub enum Handler {
    Json(fn(&mut Ctx, &Params) -> Result<Value, CallError>),
    Bytes(fn(&mut Ctx, &Params) -> Result<Bytes, CallError>),
}

pub struct Route {
    pub method: &'static str,
    pub path: &'static str,
    /// `true` when `path` is a prefix (`/out/`, `/trigger/`) rather than the
    /// whole path.
    pub prefix: bool,
    pub handler: Handler,
}

impl Route {
    fn matches(&self, method: &str, path: &str) -> bool {
        (self.method == "*" || self.method == method)
            && if self.prefix {
                path.starts_with(self.path)
            } else {
                path == self.path
            }
    }
}

/// Runs `req` (already reduced to method/path/[`Params`]) against [`ROUTES`],
/// dispatches the first match, and turns its result into a [`Reply`]. `None`
/// when nothing in the table matches — the wrapper answers 404 itself, the
/// same as any other unrouted path, so a truly-unrouted request and a
/// route-that-404s-on-purpose (e.g. `/output.jpg?device=nope`) both end up
/// 404 on the wire, which is the behaviour this refactor preserves rather
/// than changes.
/// `GET /perf`: the performance monitor's last window (`crate::perf`).
fn perf_snapshot(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(crate::perf::snapshot())
}

/// Without the desktop feature there is no agent harness, so no bearer token
/// is ever accepted and `POST /agent/tool` is never intercepted (`ai_off.rs`
/// answers the AI routes with the same refusal).
#[cfg(not(feature = "agent"))]
pub fn bearer_authorized(_authorization: Option<&str>) -> bool {
    false
}

#[cfg(not(feature = "agent"))]
pub fn agent_intercept(_method: &str, _path: &str, _params: &Params, _authorized: bool) -> Option<Reply> {
    None
}

/// `POST /agent/tool`, answered before the state lock is taken: a tool such as
/// `sleep` or `bake_clip` runs in the agent process and must not hold the app's
/// `State` while it works. `serve` and `Door::call` both consult this first.
/// `None` for every other request. See `crate::chat::agent_tool_at`.
pub const AGENT_TOOL_ROUTE: &str = "/agent/tool";

/// The bearer token `POST /agent/tool` requires, set once at startup from
/// `metal_agent::auth::ensure_token`. Unset (a test, or a startup that has not
/// finished) means nothing presents a valid token, so the route refuses.
static AGENT_TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_agent_token(token: String) {
    let _ = AGENT_TOKEN.set(token);
}

/// Whether an HTTP `Authorization` header carries the install's token:
/// exactly `Bearer <token>`, compared in constant time.
#[cfg(feature = "agent")]
pub fn bearer_authorized(authorization: Option<&str>) -> bool {
    let Some(expected) = AGENT_TOKEN.get() else {
        return false;
    };
    authorization
        .and_then(|h| h.strip_prefix("Bearer "))
        .is_some_and(|presented| metal_agent::auth::tokens_match(expected, presented.trim()))
}

/// `authorized` is whether the caller may use `POST /agent/tool`: an HTTP
/// request says so from its bearer header ([`bearer_authorized`]), and the
/// in-process door (`Door::call`, the shell) passes `true`.
#[cfg(feature = "agent")]
pub fn agent_intercept(method: &str, path: &str, params: &Params, authorized: bool) -> Option<Reply> {
    if method != "POST" || path != AGENT_TOOL_ROUTE {
        return None;
    }
    if !authorized {
        return Some(Reply::error(CallError::Unauthorized));
    }
    let result = match params.json() {
        Some(body) => crate::chat::agent_tool_at(&crate::output::data_root(), &body),
        None => Err(CallError::Bad("expected json".into())),
    };
    Some(match result {
        Ok(v) => Reply::json_ok(v),
        Err(e) => Reply::error(e),
    })
}

pub fn dispatch(method: &str, path: &str, params: &Params, ctx: &mut Ctx) -> Option<Reply> {
    let (reply, pending) = dispatch_deferred(method, path, params, ctx)?;
    Some(finish(reply, pending))
}

/// What a picture route parked instead of encoding: the codec and the pixels.
pub type Pending = Option<(render::Codec, Vec<u8>, u32, u32)>;

/// [`dispatch`] without the image encode: a picture route's reply comes back
/// with an empty body and its pixels in the [`Pending`], to be turned into the
/// body by [`finish`] after the caller drops the shared lock.
pub fn dispatch_deferred(method: &str, path: &str, params: &Params, ctx: &mut Ctx) -> Option<(Reply, Pending)> {
    render::set_deferring(true);
    let reply = dispatch_route(method, path, params, ctx);
    let pending = render::take_pending();
    render::set_deferring(false);
    reply.map(|r| (r, pending))
}

/// Runs the encode [`dispatch_deferred`] postponed.
pub fn finish(mut reply: Reply, pending: Pending) -> Reply {
    if let Some((codec, pixels, w, h)) = pending {
        reply.body = codec.encode(&pixels, w, h).body;
    }
    reply
}

fn dispatch_route(method: &str, path: &str, params: &Params, ctx: &mut Ctx) -> Option<Reply> {
    let route = ROUTES.iter().find(|r| r.matches(method, path))?;
    let params = Params {
        tail: route.prefix.then(|| path[route.path.len()..].to_string()),
        ..params.clone()
    };
    let params = &params;
    Some(match route.handler {
        Handler::Json(f) => match f(ctx, params) {
            Ok(v) => Reply::json_ok(v),
            Err(e) => Reply::error(e),
        },
        Handler::Bytes(f) => match f(ctx, params) {
            Ok(b) => Reply::bytes_ok(b),
            Err(e) => Reply::error(e),
        },
    })
}

/// The one source of what this app dispatches. `serve` (`main.rs`) walks
/// this to answer a socket; the native shell (from `S.U6` on) walks it — or
/// calls straight into the handlers named here — to answer a panel. A test
/// in `call::tests` walks it too, builds a synthetic request per row, and
/// asserts none of them 404s — see that module for why 404 specifically is
/// the one status that means "this row does not actually dispatch".
pub static ROUTES: &[Route] = &[
    Route {
        method: "GET",
        path: "/files",
        prefix: false,
        handler: Handler::Json(files::list),
    },
    Route {
        method: "POST",
        path: "/files",
        prefix: false,
        handler: Handler::Json(files::upload),
    },
    Route {
        method: "GET",
        path: "/files/download",
        prefix: false,
        handler: Handler::Bytes(files::download),
    },
    Route {
        method: "GET",
        path: "/state",
        prefix: false,
        handler: Handler::Json(transmit::state),
    },
    // The machines this one can compose or render on (`metal-sync`'s peer
    // list, relayed) — the properties panel's machine pickers.
    Route {
        method: "GET",
        path: "/peers",
        prefix: false,
        handler: Handler::Json(peers::peers),
    },
    Route {
        method: "GET",
        path: "/monitor",
        prefix: false,
        handler: Handler::Json(transmit::monitor),
    },
    Route {
        method: "POST",
        path: "/monitor",
        prefix: false,
        handler: Handler::Json(transmit::monitor_open),
    },
    Route {
        method: "GET",
        path: "/preview.jpg",
        prefix: false,
        handler: Handler::Bytes(render::preview_jpg),
    },
    Route {
        method: "GET",
        path: "/source.jpg",
        prefix: false,
        handler: Handler::Bytes(render::source_jpg),
    },
    Route {
        method: "GET",
        path: "/output.jpg",
        prefix: false,
        handler: Handler::Bytes(render::output_jpg),
    },
    Route {
        method: "GET",
        path: "/node.jpg",
        prefix: false,
        handler: Handler::Bytes(render::node_jpg),
    },
    // The gallery's fx/material cards: a kind at one float variant. It sits
    // under `/author` but needs the engine (`Paint`), so `Door::call` routes it
    // here before the author table (see `KIND_PATH`).
    Route {
        method: "GET",
        path: "/author/render/kind.jpg",
        prefix: false,
        handler: Handler::Bytes(render::kind_jpg),
    },
    // The models dropdown's hover poster: the `model-demo` JPEG and its caption.
    Route {
        method: "GET",
        path: "/models/demo.jpg",
        prefix: false,
        handler: Handler::Bytes(render::demo_jpg),
    },
    Route {
        method: "GET",
        path: "/models/demo",
        prefix: false,
        handler: Handler::Json(render::demo_info),
    },
    // H5: a `sensor.yolo` node's live reading (the "lost track" indicator's
    // one fact) and its picture — the capture frame with the head drawn on it.
    // The `set_head_tracking` tool posts the same route (rule 6).
    Route {
        method: "GET",
        path: "/sensor",
        prefix: false,
        handler: Handler::Json(sensor::reading),
    },
    Route {
        method: "POST",
        path: "/sensor",
        prefix: false,
        handler: Handler::Json(sensor::set),
    },
    Route {
        method: "GET",
        path: "/sensor.jpg",
        prefix: false,
        handler: Handler::Bytes(sensor::preview_jpg),
    },
    // `S.U16`'s interim transport: the monitor mode loops back to this
    // route (same machine, no `metal-sync` yet — see `src/monitor.rs`'s module
    // doc). Same composed frame `/output.jpg` serves, same one path, so a
    // monitor can never show what the projector's browser would not.
    Route {
        method: "GET",
        path: "/frame",
        prefix: false,
        handler: Handler::Bytes(render::frame_jpg),
    },
    // `fw/frame/1`'s layer asks and `capture_output`: one picture alone at a
    // frame, from `metal-sync` on this machine or a peer's loopback, or the
    // shell. Forms: `/render/<layer>/<frame>` (a layer),
    // `/render/<device>/<node>/<frame>` (a graph node, the E2 route) and
    // `/render/<node>/<frame>` (a node whose device is found here).
    Route {
        method: "GET",
        path: "/render/",
        prefix: true,
        handler: Handler::Bytes(render::layer_jpg),
    },
    Route {
        method: "POST",
        path: "/select",
        prefix: false,
        handler: Handler::Json(transmit::select),
    },
    Route {
        method: "POST",
        path: "/router",
        prefix: false,
        handler: Handler::Json(transmit::router),
    },
    Route {
        method: "POST",
        path: "/reel",
        prefix: false,
        handler: Handler::Json(transmit::reel),
    },
    Route {
        method: "GET",
        path: "/reel/files",
        prefix: false,
        handler: Handler::Json(transmit::reel_files),
    },
    // P0: one level of the file system, the Library browser's left column.
    Route {
        method: "GET",
        path: "/fs",
        prefix: false,
        handler: Handler::Json(transmit::fs_list),
    },
    // P-D: where `clock.timecode` puts the sequence playhead, and Art-Net
    // discovery for `out.artnet` — the properties pages and the AI tools
    // `timecode_status`/`artnet_nodes` read the same two (rule 6).
    Route {
        method: "GET",
        path: "/timecode",
        prefix: false,
        handler: Handler::Json(crate::timecode::timecode_get),
    },
    // P-E: the transition presets (`data/transitions/*.json`) — the
    // properties picker and the AI's `list_transitions` read this one route.
    Route {
        method: "GET",
        path: "/transitions",
        prefix: false,
        handler: Handler::Json(switch::transitions_get),
    },
    Route {
        method: "GET",
        path: "/artnet/nodes",
        prefix: false,
        handler: Handler::Json(crate::timecode::artnet_nodes),
    },
    // P-D: the one Art-Net listener's state and what an `in.artnet` window
    // reads now (the in.artnet page's readout, the AI's `artnet_in_status`).
    Route {
        method: "GET",
        path: "/artnet/in",
        prefix: false,
        handler: Handler::Json(crate::timecode::artnet_in),
    },
    // F13: sACN Universe Discovery (E1.31 §6.2) — the Settings → Discover
    // section's "listen to sources", the same route any other client reads.
    Route {
        method: "GET",
        path: "/sacn/nodes",
        prefix: false,
        handler: Handler::Json(crate::timecode::sacn_nodes),
    },
    Route {
        method: "POST",
        path: "/transport",
        prefix: false,
        handler: Handler::Json(transmit::transport),
    },
    Route {
        method: "POST",
        path: "/drive",
        prefix: false,
        handler: Handler::Json(transmit::drive),
    },
    // Loop detection (Phase 4B): the operator's hand on `State.looper` — one
    // body-driven route (`mode`/`lock`/`beats`/`exit`), so a control posts
    // exactly the op it means and an AI tool would post the same body.
    Route {
        method: "POST",
        path: "/loop/audio",
        prefix: false,
        handler: Handler::Json(loop_audio::loop_audio),
    },
    Route {
        method: "GET",
        path: "/out/",
        prefix: true,
        handler: Handler::Bytes(page::out_device),
    },
    Route {
        method: "POST",
        path: "/outputs",
        prefix: false,
        handler: Handler::Json(outputs::outputs),
    },
    Route {
        method: "POST",
        path: "/layers",
        prefix: false,
        handler: Handler::Json(outputs::layers),
    },
    Route {
        method: "POST",
        path: "/overlay",
        prefix: false,
        handler: Handler::Json(overlay::overlay),
    },
    // `S.O.1`: the room's off switch. See `blackout.rs`'s own module doc
    // for why this is a small body of its own rather than a literal call
    // into `switch::switch_post`.
    Route {
        method: "GET",
        path: "/overlay/blackout",
        prefix: false,
        handler: Handler::Json(blackout::blackout_get),
    },
    Route {
        method: "POST",
        path: "/overlay/blackout",
        prefix: false,
        handler: Handler::Json(blackout::blackout_post),
    },
    Route {
        method: "POST",
        path: "/trigger/",
        prefix: true,
        handler: Handler::Json(transmit::trigger),
    },
    // `S.AK`/`O.28`: the grid's on/off buttons.
    Route {
        method: "GET",
        path: "/switch",
        prefix: false,
        handler: Handler::Json(switch::switch_get),
    },
    Route {
        method: "POST",
        path: "/switch",
        prefix: false,
        handler: Handler::Json(switch::switch_post),
    },
    Route {
        method: "GET",
        path: "/graphs",
        prefix: false,
        handler: Handler::Json(graph::graphs),
    },
    Route {
        method: "GET",
        path: "/graph",
        prefix: false,
        handler: Handler::Json(graph::graph_get),
    },
    Route {
        method: "GET",
        path: "/graph/hit",
        prefix: false,
        handler: Handler::Json(graph::graph_hit),
    },
    Route {
        method: "POST",
        path: "/graph",
        prefix: false,
        handler: Handler::Json(graph::graph_post),
    },
    Route {
        method: "POST",
        path: "/graph/node",
        prefix: false,
        handler: Handler::Json(graph::graph_node),
    },
    Route {
        method: "POST",
        path: "/graph/edge",
        prefix: false,
        handler: Handler::Json(graph::graph_edge),
    },
    Route {
        method: "GET",
        path: "/graph/effects",
        prefix: false,
        handler: Handler::Json(graph::effects_get),
    },
    Route {
        method: "POST",
        path: "/graph/effects",
        prefix: false,
        handler: Handler::Json(graph::effects_post),
    },
    // `§9.4 V3`: the script code editor. `GET /script/<node>?graph=<id>` reads
    // one `script.*` node's `{lang, source}`; `POST /script {graph,node,source}`
    // is the one writer — it runs the source through the engine's own runtime
    // and only then writes `config.code`. The editor and the AI's
    // `read_script`/`save_script` both walk these two rows (rule 6).
    Route {
        method: "GET",
        path: "/script/",
        prefix: true,
        handler: Handler::Json(script::get),
    },
    Route {
        method: "POST",
        path: "/script",
        prefix: false,
        handler: Handler::Json(script::save),
    },
    // Music loops (the loops design notes T9): templates, palettes, a node's choice.
    Route { method: "GET", path: "/loops", prefix: false, handler: Handler::Json(loops::list) },
    Route { method: "GET", path: "/loops/get", prefix: false, handler: Handler::Json(loops::get) },
    Route { method: "POST", path: "/loops/save", prefix: false, handler: Handler::Json(loops::save) },
    Route { method: "GET", path: "/loops/palettes", prefix: false, handler: Handler::Json(loops::palettes) },
    Route { method: "GET", path: "/loops/node", prefix: false, handler: Handler::Json(loops::node) },
    Route { method: "POST", path: "/loops/choose", prefix: false, handler: Handler::Json(loops::choose) },
    Route { method: "POST", path: "/loops/tap", prefix: false, handler: Handler::Json(loops::tap) },
    Route { method: "POST", path: "/loops/song", prefix: false, handler: Handler::Json(loops::song) },
    // Param edges (`metal_graph::bind`): one node's inputs/outputs/parameters/
    // sources, and every node's values in the last frame drawn.
    Route {
        method: "GET",
        path: "/graph/io",
        prefix: false,
        handler: Handler::Json(graph::graph_io),
    },
    Route {
        method: "GET",
        path: "/graph/live",
        prefix: false,
        handler: Handler::Json(graph::graph_live),
    },
    Route {
        method: "GET",
        path: "/catalog",
        prefix: false,
        handler: Handler::Json(graph::catalog),
    },
    Route {
        method: "GET",
        path: "/scene",
        prefix: false,
        handler: Handler::Json(transmit::scene),
    },
    // The pre-English route, kept so an old client still reads the scene.
    Route {
        method: "GET",
        path: "/escena",
        prefix: false,
        handler: Handler::Json(transmit::scene),
    },
    // The one write: `{ op: "fit", wall, output, corners }` stores a wall's
    // hand-dragged homography ("calibrate wall") through `Escena::save`.
    Route {
        method: "POST",
        path: "/scene",
        prefix: false,
        handler: Handler::Json(transmit::escena_post),
    },
    // Alias of the POST above for an old client still posting `/escena`.
    Route {
        method: "POST",
        path: "/escena",
        prefix: false,
        handler: Handler::Json(transmit::escena_post),
    },
    // S.U14: the harness sidebar's tool controls and `GET /ai-config`.
    Route {
        method: "GET",
        path: "/ai-config",
        prefix: false,
        handler: Handler::Json(ai::get),
    },
    Route {
        method: "POST",
        path: "/ai-config",
        prefix: false,
        handler: Handler::Json(ai::post),
    },
    // S.X: the chat. `POST /ai/ask` appends to a conversation and hands the
    // turn to a worker thread (`crate::chat`, the harness's own loop);
    // `/ai/log` keeps the cross-chat prompt journal. The chat menu and the
    // conversation reads are the rows below.
    Route {
        method: "POST",
        path: "/ai/ask",
        prefix: false,
        handler: Handler::Json(ai::ask),
    },
    Route {
        method: "GET",
        path: "/ai/log",
        prefix: false,
        handler: Handler::Json(ai::log),
    },
    Route {
        method: "GET",
        path: "/ai/chats",
        prefix: false,
        handler: Handler::Json(ai::chats),
    },
    Route {
        method: "POST",
        path: "/ai/chats",
        prefix: false,
        handler: Handler::Json(ai::chats_new),
    },
    Route {
        method: "GET",
        path: "/ai/chats/",
        prefix: true,
        handler: Handler::Json(ai::chat_get),
    },
    Route {
        method: "POST",
        path: "/ai/chat/delete",
        prefix: false,
        handler: Handler::Json(ai::chat_delete),
    },
    // S.X: stop a running turn and answer an `ask`-policy tool call.
    Route {
        method: "POST",
        path: "/ai/chat/cancel",
        prefix: false,
        handler: Handler::Json(ai::chat_cancel),
    },
    Route {
        method: "POST",
        path: "/ai/chat/approve",
        prefix: false,
        handler: Handler::Json(ai::chat_approve),
    },
    // S.AM: the train gesture of a trainable `ai.*` node — the same body the
    // `train_ai` tool posts and the lane's `entrenar` button will post. It
    // dispatches by kind; `ai.colourist` writes the same grade file the
    // studio's `/author/grade/train` does.
    Route {
        method: "POST",
        path: "/ai/train",
        prefix: false,
        handler: Handler::Json(ai::train),
    },
    // S.AF: the settings overlay's own file (`data/settings.json`) — the
    // shell mode override the header writes, read at paints time by the
    // arrangement (`O.16`'s first slice).
    Route {
        method: "GET",
        path: "/perf",
        prefix: false,
        handler: Handler::Json(perf_snapshot),
    },
    // P-G: "Export frame" — the preview bar's camera and the
    // `export_frame` tool (rule 6) save the node's frame as a PNG.
    // P-C: video I/O kinds (virtual camera, NDI, Spout, Syphon) with the
    // reason each is off, and every screen's live sink.
    Route {
        method: "GET",
        path: "/videoio",
        prefix: false,
        handler: Handler::Json(videoio::get),
    },
    Route {
        method: "POST",
        path: "/frame/export",
        prefix: false,
        handler: Handler::Json(frame::export),
    },
    Route {
        method: "GET",
        path: "/settings",
        prefix: false,
        handler: Handler::Json(settings::get),
    },
    Route {
        method: "POST",
        path: "/settings",
        prefix: false,
        handler: Handler::Json(settings::post),
    },
    // The in-app updater. `GET` is read-only (it reports, never downloads);
    // `POST {"op":"apply","tag":"…"}` is the one explicit apply — the "never
    // auto-update mid-show" rule is the shape of the API, not a caller's
    // politeness. `[update].channel = "off"` (or `--updates off`) answers
    // `state: "off"` from both. See `call::update`'s module docs for the JSON.
    Route {
        method: "GET",
        path: "/update",
        prefix: false,
        handler: Handler::Json(update::get),
    },
    Route {
        method: "POST",
        path: "/update",
        prefix: false,
        handler: Handler::Json(update::post),
    },
    // The shell's pairing and sync status (`metal-sync` on :7901, forwarded).
    Route {
        method: "GET",
        path: "/sync/status",
        prefix: false,
        handler: Handler::Json(sync::status_get),
    },
    Route {
        method: "GET",
        path: "/sync/peers",
        prefix: false,
        handler: Handler::Json(sync::peers_get),
    },
    Route {
        method: "GET",
        path: "/sync/stats",
        prefix: false,
        handler: Handler::Json(sync::stats_get),
    },
    Route {
        method: "GET",
        path: "/sync/log",
        prefix: false,
        handler: Handler::Json(sync::log_get),
    },
    Route {
        method: "GET",
        path: "/sync/discover",
        prefix: false,
        handler: Handler::Json(sync::discover_get),
    },
    Route {
        method: "GET",
        path: "/sync/pair",
        prefix: false,
        handler: Handler::Json(sync::pair_get),
    },
    Route {
        method: "POST",
        path: "/sync/pair/request",
        prefix: false,
        handler: Handler::Json(sync::pair_request_post),
    },
    Route {
        method: "POST",
        path: "/sync/pair/confirm",
        prefix: false,
        handler: Handler::Json(sync::pair_confirm_post),
    },
    Route {
        method: "POST",
        path: "/sync/pair/reject",
        prefix: false,
        handler: Handler::Json(sync::pair_reject_post),
    },
    Route {
        method: "POST",
        path: "/collections",
        prefix: false,
        handler: Handler::Json(outputs::collections),
    },
    Route {
        method: "GET",
        path: "/sequences",
        prefix: false,
        handler: Handler::Json(sequences::sequences_get),
    },
    Route {
        method: "POST",
        path: "/sequences",
        prefix: false,
        handler: Handler::Json(sequences::sequences_post),
    },
    Route {
        method: "GET",
        path: "/io",
        prefix: false,
        handler: Handler::Json(transmit::io_get),
    },
    Route {
        method: "GET",
        path: "/reel/scenes",
        prefix: false,
        handler: Handler::Json(transmit::reel_scenes),
    },
    Route {
        method: "POST",
        path: "/io",
        prefix: false,
        handler: Handler::Json(transmit::io),
    },
    // Plan "Audio input is a node": the source combo, the readout/indicator
    // and the `probar` button of an `audio.live` node.
    Route {
        method: "GET",
        path: "/listen/sources",
        prefix: false,
        handler: Handler::Json(transmit::listen_sources),
    },
    Route {
        method: "GET",
        path: "/listen/status",
        prefix: false,
        handler: Handler::Json(transmit::listen_status),
    },
    Route {
        method: "POST",
        path: "/listen/probe",
        prefix: false,
        handler: Handler::Json(transmit::listen_probe),
    },
    Route {
        method: "POST",
        path: "/mask/grid",
        prefix: false,
        handler: Handler::Json(outputs::mask_grid),
    },
    Route {
        method: "POST",
        path: "/mask/project",
        prefix: false,
        handler: Handler::Bytes(render::mask_project),
    },
    // 3D venues: list/check an AI-authored venue, and project its masks
    // onto its output (the `project_venue` tool's first call).
    Route {
        method: "GET",
        path: "/venue",
        prefix: false,
        handler: Handler::Json(venue::list),
    },
    // Venue 3D: the whole room as the editor/viewport sees it (no writes) —
    // the AI's "look at the venue" read.
    Route {
        method: "GET",
        path: "/venue/scene",
        prefix: false,
        handler: Handler::Json(venue::scene),
    },
    // M2: every fixture's live state (commanded vs estimated actual).
    Route {
        method: "GET",
        path: "/venue/state",
        prefix: false,
        handler: Handler::Json(venue::state),
    },
    // F10/F11/F14: the last packet one `venue.fixture` node sent, read off the
    // universe bus with a swatch decoded from the profile's own channels.
    Route {
        method: "GET",
        path: "/venue/fixture-packet",
        prefix: false,
        handler: Handler::Json(venue::fixture_packet),
    },
    // M4.2: the fixture profiles under data/fixtures/.
    Route {
        method: "GET",
        path: "/fixtures",
        prefix: false,
        handler: Handler::Json(venue::fixtures_list),
    },
    // D2: import a `.gdtf` into `data/fixtures/<model>.toml`
    // (`{op:"import-gdtf", path|bytes, name?, mode?}`) — the same route the
    // AI's `import_gdtf` tool posts (rule 6).
    Route {
        method: "POST",
        path: "/fixtures",
        prefix: false,
        handler: Handler::Json(venue::fixtures_post),
    },
    Route {
        method: "POST",
        path: "/venue",
        prefix: false,
        handler: Handler::Json(venue::venue),
    },
    Route {
        method: "POST",
        path: "/reload",
        prefix: false,
        handler: Handler::Json(transmit::reload),
    },
    Route {
        method: "POST",
        path: "/colour/apply",
        prefix: false,
        handler: Handler::Json(colour::apply),
    },
    Route {
        method: "GET",
        path: "/colour/targets",
        prefix: false,
        handler: Handler::Json(colour::targets),
    },
    // export.video (plan §9.1 E1): start an offline export, read its progress,
    // cancel it. The AI's `export_video` tool posts `/export/video` (rule 6).
    Route {
        method: "POST",
        path: "/export/video",
        prefix: false,
        handler: Handler::Json(export::video),
    },
    Route {
        method: "GET",
        path: "/export",
        prefix: false,
        handler: Handler::Json(export::status),
    },
    Route {
        method: "POST",
        path: "/export",
        prefix: false,
        handler: Handler::Json(export::control),
    },
    // The export queue (plan §9.1): FIFO in `data/export-presets.json`, one job
    // at a time; `POST` adds, cancels, reorders or lists. The AI's
    // `export_queue` tool posts here (rule 6).
    Route {
        method: "GET",
        path: "/export/queue",
        prefix: false,
        handler: Handler::Json(export::queue_list),
    },
    Route {
        method: "POST",
        path: "/export/queue",
        prefix: false,
        handler: Handler::Json(export::queue_control),
    },
    // §9.4 V2: the WGSL editor's one writer and its reads. The operator's
    // editor and the AI's `save_shader` tool both post here (rule 6), so a
    // shader one saves and a shader the other saves pass the same naga gate
    // (`metal_forge::check`) before anything reaches `data/shaders/`.
    Route {
        method: "GET",
        path: "/shaders",
        prefix: false,
        handler: Handler::Json(shaders::list),
    },
    Route {
        method: "GET",
        path: "/shaders/",
        prefix: true,
        handler: Handler::Json(shaders::get),
    },
    Route {
        method: "POST",
        path: "/shaders",
        prefix: false,
        handler: Handler::Json(shaders::save),
    },
];

#[cfg(test)]
mod tests {
    use crate::chdir_lock;

    use super::*;

    /// `POST /agent/tool` is refused without the install's bearer token, and
    /// the refusal happens before any tool runs (a 401, no executor built).
    #[test]
    fn agent_tool_needs_the_install_token() {
        set_agent_token("test-install-token".into());
        assert!(bearer_authorized(Some("Bearer test-install-token")));
        assert!(!bearer_authorized(Some("Bearer wrong")));
        assert!(!bearer_authorized(Some("test-install-token")), "no Bearer scheme");
        assert!(!bearer_authorized(None));

        let body = br#"{"name":"sleep","args":{"ms":1}}"#.to_vec();
        let params = Params { body, ..Params::default() };
        let refused = agent_intercept("POST", AGENT_TOOL_ROUTE, &params, false).expect("routed");
        assert_eq!(refused.status, 401);
        assert_eq!(refused.status_line(), "401 Unauthorized");
        assert!(agent_intercept("GET", AGENT_TOOL_ROUTE, &params, false).is_none());
        assert!(agent_intercept("POST", "/sleep", &params, false).is_none());
    }

    /// No two rows answer the same (method, path) — the table is the one
    /// source of what dispatches, so a duplicate here would be a second,
    /// silent path to one route (the same shape rule 6 forbids for a
    /// tool/control pair, one level down).
    #[test]
    fn no_duplicate_method_path_rows() {
        let mut seen = std::collections::HashSet::new();
        for r in ROUTES {
            assert!(
                seen.insert((r.method, r.path, r.prefix)),
                "duplicate row: {} {} (prefix={})",
                r.method,
                r.path,
                r.prefix
            );
        }
    }

    /// Every row in [`ROUTES`] dispatches. Walking the table rather than a
    /// hand-written list of paths is the point — rule 7: this count is
    /// never written down anywhere, it is read off the table itself.
    ///
    /// A route whose own handler answers a real 404 (`/output.jpg?device=`
    /// naming nothing, say) is indistinguishable on the wire from one that
    /// never dispatched at all — that is an inherent limit of asserting off
    /// HTTP status alone, not a gap in the table. The fixture below gives
    /// every row something to resolve against (a device and a trigger both
    /// called `"probe"`) precisely so this test tells the two apart: a row
    /// that still 404s with a fixture built for it is a row that did not
    /// dispatch.
    #[test]
    fn every_route_dispatches() {
        // `Author::new()` (`gpu` builds only) reads `data/schema.json` off a
        // bare relative path (`author::SCHEMA`), unlike everything else in
        // this crate's tests, which go through `output::data_root()` (a
        // `CARGO_MANIFEST_DIR`-relative `OnceLock`, cwd-independent) or
        // `env!("CARGO_MANIFEST_DIR")` directly. `cargo test`'s working
        // directory is this package's own, not the repo root, so without
        // this the process reads a missing file and calls
        // `std::process::exit(1)` from inside `author::models::State::new`
        // — not a panic the test harness can catch. Scoped to just the
        // construction: nothing else in this crate is cwd-sensitive, so the
        // window is as small as it can be. Not touching `author::*` itself —
        // that module is `the cleanup`'s, not this batch's.
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        let _chdir = chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = crate::Author::new();
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let mut paint = crate::paint::painter();
        let mut state = crate::State::new(paint.as_ref());
        state
            .outputs
            .list
            .push(crate::output::Output::new("probe", "Probe", 64, 64));
        state
            .sources
            .entry("probe".to_string())
            .or_insert_with(crate::layers::Device::default);
        state
            .sources
            .get_mut("probe")
            .unwrap()
            .triggers
            .push(crate::trigger::Trigger::new_http(
                "probe".to_string(),
                "transport".to_string(),
            ));
        let mut sim = crate::listeners::Listeners::stand_in();

        for route in ROUTES {
            let path = if route.prefix {
                format!("{}probe", route.path)
            } else {
                route.path.to_string()
            };
            let method = if route.method == "*" {
                "GET"
            } else {
                route.method
            };
            let params = Params {
                query: HashMap::new(),
                body: Vec::new(),
                tail: None,
            };
            let mut ctx = Ctx {
                state: &mut state,
                paint: paint.as_mut(),
                sim: &mut sim,
                author: &author,
            };
            let reply = dispatch(method, &path, &params, &mut ctx);
            let status = reply.map(|r| r.status);
            assert_ne!(status, Some(404), "{method} {path} 404d — did not dispatch");
            assert!(status.is_some(), "{method} {path} matched no row in ROUTES");
        }
    }

    /// `GET /render/<layer>/<frame>` draws the layer alone: a JPEG for a
    /// layer the state holds, 404 for one it does not, 400 for a frame that
    /// is not a number or a path with no frame at all.
    #[test]
    fn the_layer_route_draws_a_known_layer_and_refuses_the_rest() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        let _chdir = chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = crate::Author::new();
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let mut paint = crate::paint::painter();
        let mut state = crate::State::new(paint.as_ref());
        state.layers.push(crate::layers::Layer::new("l1", "Layer", 0));
        let mut sim = crate::listeners::Listeners::stand_in();
        let query: HashMap<String, String> =
            [("w", "64"), ("h", "36")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let mut get = |path: &str| {
            let params = Params { query: query.clone(), body: Vec::new(), tail: None };
            let mut ctx = Ctx {
                state: &mut state,
                paint: paint.as_mut(),
                sim: &mut sim,
                author: &author,
            };
            let reply = dispatch("GET", path, &params, &mut ctx).expect("dispatches");
            (reply.status, reply.body)
        };
        let (status, body) = get("/render/l1/3");
        assert_eq!(status, 200);
        assert_eq!(&body[..2], &[0xff, 0xd8], "a JPEG");
        assert_eq!(get("/render/nobody/3").0, 404);
        assert_eq!(get("/render/l1/x").0, 400);
        assert_eq!(get("/render/l1").0, 400);
    }

    /// E2: `GET /render/<device>/<node>/<frame>` draws one graph node alone —
    /// the route `capture_output` reads. A JPEG of the asked size comes back
    /// for a node the device's graph holds; an unknown node 404s and a
    /// non-numeric frame 400s.
    #[test]
    fn the_node_capture_route_draws_a_known_node_to_a_jpeg() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        let _chdir = chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = crate::Author::new();
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let mut paint = crate::paint::painter();
        let mut state = crate::State::new(paint.as_ref());
        let device = format!("capture-node-{}", std::process::id());
        state.outputs.list.push(crate::output::Output::new(&device, "Node", 96, 54));
        state.sources.entry(device.clone()).or_insert_with(crate::layers::Device::default);
        let mut sim = crate::listeners::Listeners::stand_in();

        // A generator node in the device's own graph: what `render_node` draws.
        let root = crate::output::data_root();
        let model = paint.ids()[0].to_string();
        let mut graph =
            metal_graph::Graph::new(&crate::device_graph_id(&device), &device, metal_graph::Domain::Live);
        let gen = graph.add_node("gen", &format!("gen.{model}"), [0.0, 0.0]);
        crate::save_graph_at(&root, &graph).expect("save the device graph");

        let get = |state: &mut crate::State,
                   paint: &mut Box<dyn crate::paint::Paint>,
                   sim: &mut crate::listeners::Listeners,
                   path: &str| {
            let query: HashMap<String, String> = [("fmt", "jpeg"), ("w", "64"), ("h", "36")]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            let params = Params { query, body: Vec::new(), tail: None };
            let mut ctx = Ctx { state, paint: paint.as_mut(), sim, author: &author };
            let reply = dispatch("GET", path, &params, &mut ctx).expect("dispatches");
            (reply.status, reply.body)
        };

        let (status, body) = get(&mut state, &mut paint, &mut sim, &format!("/render/{device}/{gen}/0"));
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        assert_eq!(&body[..2], &[0xff, 0xd8], "a JPEG");
        let (w, h, rgb) = crate::shell::frames::decode_jpeg_rgb(&body).expect("decodes");
        assert_eq!((w, h), (64, 36), "the asked size");
        assert!(rgb.iter().any(|&p| p != 0), "not an empty image");

        assert_eq!(get(&mut state, &mut paint, &mut sim, &format!("/render/{device}/nope/0")).0, 404);
        assert_eq!(get(&mut state, &mut paint, &mut sim, &format!("/render/{device}/{gen}/x")).0, 400);

        let _ = metal_vfs::remove_file(crate::graph_path_at(&root, &crate::device_graph_id(&device)));
    }

    /// A peer with its own floats at `base`, answering `GET /render/<id>/3`
    /// with `query`: the status and body, as the route answers them.
    fn peer_answers(base: f32, id: &str, query: &str) -> (u16, Vec<u8>) {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        let _chdir = chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = crate::Author::new();
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let mut paint = crate::paint::painter();
        let mut state = crate::State::new(paint.as_ref());
        for row in state.base.iter_mut() {
            row.iter_mut().for_each(|f| *f = base);
        }
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut query: HashMap<String, String> = query
            .split('&')
            .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();
        query.entry("w".into()).or_insert("64".into());
        query.entry("h".into()).or_insert("36".into());
        let params = Params { query, body: Vec::new(), tail: None };
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let reply = dispatch("GET", &format!("/render/{id}/3"), &params, &mut ctx).expect("dispatches");
        (reply.status, reply.body)
    }

    /// The state a requester sends draws the same picture on any peer: two
    /// peers whose own floats differ answer the same ask with the same JPEG,
    /// and a layer the peer does not hold is drawn all the same.
    #[test]
    fn a_layer_ask_with_state_draws_the_same_pixels_on_any_peer() {
        // A row is model 0's whole float row (its schema width), as a requester sends it.
        let width = {
            let paint = crate::paint::painter();
            crate::State::new(paint.as_ref()).base[0].len()
        };
        let row: Vec<String> = (0..width).map(|i| format!("{}", 0.01 * i as f32 - 0.3)).collect();
        let query = format!("model=0&time=0.75&row={}", row.join(","));
        let (sa, a) = peer_answers(0.0, "anything", &query);
        let (sb, b) = peer_answers(0.9, "elsewhere", &query);
        assert_eq!((sa, sb), (200, 200));
        assert_eq!(&a[..2], &[0xff, 0xd8], "a JPEG");
        assert_eq!(a, b, "same state in, same pixels out, on any peer");
    }

    /// A row without its model, or a model this machine does not have, is refused.
    #[test]
    fn a_layer_ask_with_state_is_refused_when_it_is_incomplete() {
        assert_eq!(peer_answers(0.0, "x", "row=0.1,0.2").0, 400, "no model");
        assert_eq!(peer_answers(0.0, "x", "model=999&row=0.1").0, 400, "no such model");
        assert_eq!(peer_answers(0.0, "x", "model=0&row=0.1,oops").0, 400, "not a number");
    }
}
