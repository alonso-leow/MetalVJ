//! The engine state the routes and the shell share: [`Author`] (the studio's
//! door), [`Shared`] (the state, painter and listeners behind one lock), the
//! graph and AI-config helpers, and the reply builders. Moved here from
//! `main.rs` so the library builds on wasm32 without the HTTP server.
//!
//! File access in this module still goes straight to disk; it moves onto
//! [`crate::store::Store`] as each helper's callers are routed through it.

use std::sync::Mutex;
#[cfg(feature = "gpu")]
use std::time::Instant;

use serde_json::{json, Value};

#[cfg(feature = "gpu")]
use crate::author;
#[cfg(test)]
use crate::layers::Collection;
use crate::layers::{Blend, ClipMode, Composition, Layer, Limits, Sector};
use crate::listeners::Listeners;
#[cfg(test)]
use crate::output::Output;
use crate::paint::Paint;
use crate::store::Store;
use crate::{call, layers, output, stamped, write_device_mode_at, State, PREVIEW};
use metal_web::Request;

/// Owns the absorbed studio's state and answers `/author/*`.
///
/// **Locking rule.** Two locks: `state` (every route's inputs and outputs) and
/// `renderer` (the GPU renderer, which a grid or solo draw needs `&mut`). Take
/// `state` only to copy what a draw needs, release it, and only then take
/// `renderer`. Lock order is `state` before `renderer`, and no path takes
/// `state` while holding `renderer`. The texture and export paths follow this,
/// so a UI read never waits for a ~20 ms grid draw. Reads that never touch the
/// state skip its lock entirely, and a reply is serialised after `state` is
/// released (see [`Author::dispatch`]).
///
/// A thin wrapper rather than a bare `Mutex<author::State>` so this struct —
/// and the field in [`serve`]'s signature — exist on every build. The `gpu`
/// feature gates the one field that actually holds anything; without it,
/// `/author/*` is a 404, same as any other unrouted path. See `author::mod`
/// for why: the studio's models mode previews with the real wgpu renderers
/// and never had a no-GPU build to fall back to.
pub struct Author {
    #[cfg(feature = "gpu")]
    state: Mutex<author::State>,
    /// The last grid and solo textures, so a request the picture already
    /// answers is served without a re-draw (see [`Author::grid_texture`]).
    #[cfg(feature = "gpu")]
    grid_memo: Mutex<Option<call::author::render::PreviewMemo<metal_perform::gpu_frame::GpuFrame>>>,
    #[cfg(feature = "gpu")]
    solo_memo: Mutex<Option<call::author::render::PreviewMemo<metal_perform::gpu_frame::GpuFrame>>>,
}

impl Author {
    pub fn new() -> Self {
        Author {
            #[cfg(feature = "gpu")]
            state: Mutex::new(author::State::new(
                None,
                std::path::PathBuf::from("data/tracks/untitled.track.json"),
            )),
            #[cfg(feature = "gpu")]
            grid_memo: Mutex::new(None),
            #[cfg(feature = "gpu")]
            solo_memo: Mutex::new(None),
        }
    }

    /// The studio with its renderers on `gpu`, the painter's own device when it
    /// has one (G5), so a solo picture can be a texture.
    #[cfg(feature = "gpu")]
    pub fn with_gpu(gpu: Option<metal_render::gpu::Gpu>) -> Self {
        Author {
            state: Mutex::new(author::State::new_on(
                None,
                std::path::PathBuf::from("data/tracks/untitled.track.json"),
                gpu,
            )),
            grid_memo: Mutex::new(None),
            solo_memo: Mutex::new(None),
        }
    }

    /// G5: `GET /author/render/grid.jpg` as a texture on the painter's device.
    /// The state lock covers the copy of the rows; the draw runs under the
    /// renderer's lock alone.
    #[cfg(feature = "gpu")]
    pub fn grid_texture(
        &self,
        cx: &metal_perform::gpu_compose::GpuCx,
        query: &[(&str, String)],
    ) -> Option<metal_perform::gpu_frame::GpuFrame> {
        let params = call::Params {
            query: query
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect(),
            body: Vec::new(),
            tail: None,
        };
        let (plan, renderer) = {
            let state = crate::perf::locked(&self.state, "author.state").ok()?;
            (
                call::author::render::grid_plan(&state, &params)?,
                std::sync::Arc::clone(&state.renderer),
            )
        };
        let now = Instant::now();
        let (key, time) = (plan.key(), plan.time());
        if let Some(hit) = crate::perf::locked(&self.grid_memo, "author.grid_memo").ok()?.as_ref().and_then(|m| {
            m.hit(key, time, now, call::author::render::GRID_REFRESH)
        }) {
            return Some(hit);
        }
        let frame = {
            let mut renderer = crate::perf::locked(&renderer, "author.renderer").ok()?;
            call::author::render::grid_texture(&mut renderer, cx, &plan)
        };
        *crate::perf::locked(&self.grid_memo, "author.grid_memo").ok()? =
            Some(call::author::render::PreviewMemo::new(key, time, now, frame.clone()));
        Some(frame)
    }

    /// G5: `GET /author/render/solo.jpg`'s picture as a texture on the painter's
    /// device, when the renderers live on it (see [`Author::with_gpu`]).
    #[cfg(feature = "gpu")]
    pub fn solo_texture(
        &self,
        paint: &mut dyn metal_perform::paint::Paint,
        query: &[(&str, String)],
    ) -> Option<metal_perform::gpu_frame::GpuFrame> {
        let params = call::Params {
            query: query
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect(),
            body: Vec::new(),
            tail: None,
        };
        let (plan, renderer) = {
            let state = crate::perf::locked(&self.state, "author.state").ok()?;
            (
                call::author::render::solo_plan(&state, &params)?,
                std::sync::Arc::clone(&state.renderer),
            )
        };
        let now = Instant::now();
        let (key, time) = (plan.key(), plan.time());
        if let Some(hit) = crate::perf::locked(&self.solo_memo, "author.solo_memo").ok()?.as_ref().and_then(|m| {
            m.hit(key, time, now, call::author::render::solo_refresh())
        }) {
            return Some(hit);
        }
        let target = {
            let cx = paint.gpu_cx()?;
            let mut renderer = crate::perf::locked(&renderer, "author.renderer").ok()?;
            call::author::render::solo_target(&mut renderer, &cx, &plan)
        };
        let frame = call::author::render::solo_cut(paint, target, &plan)?;
        *crate::perf::locked(&self.solo_memo, "author.solo_memo").ok()? =
            Some(call::author::render::PreviewMemo::new(key, time, now, frame.clone()));
        Some(frame)
    }

    /// P-G: the authored row of `model` as RGBA at `w`×`h` (cut by
    /// `device`'s `mask`) — `POST /frame/export`'s sequence picture.
    pub fn solo_pixels(
        &self,
        model: &str,
        device: Option<&str>,
        mask: Option<&str>,
        w: u32,
        h: u32,
    ) -> Result<(Vec<u8>, u32, u32), call::CallError> {
        #[cfg(feature = "gpu")]
        {
            let (plan, renderer) = {
                let state =
                    crate::perf::locked(&self.state, "author.state").expect("author state lock");
                (
                    call::author::render::pixels_plan(&state, model, device, mask, w, h)?,
                    std::sync::Arc::clone(&state.renderer),
                )
            };
            let (pixels, w, h) = {
                let mut renderer =
                    crate::perf::locked(&renderer, "author.renderer").expect("renderer lock");
                call::author::render::solo_pixels_render(&mut renderer, &plan)
            };
            Ok(call::author::render::solo_pixels_cut(pixels, w, h, &plan))
        }
        #[cfg(not(feature = "gpu"))]
        {
            let _ = (model, device, mask, w, h);
            Err(call::CallError::NotFound)
        }
    }

    /// the cleanup: walks `call::author::AUTHOR_ROUTES` on the locked state and
    /// returns a typed [`call::Reply`] instead of writing to a socket —
    /// `serve` (below) is what turns that into bytes on the wire. `req.path`
    /// arrives with the outer `/author` prefix already stripped by the
    /// caller, exactly as the old `author::serve` always received it.
    ///
    /// P2: the state lock covers building the answer. Reads that never touch
    /// the state (`Handler::Free`) take no lock at all, and the reply is
    /// serialised after the lock is released.
    pub fn dispatch(&self, req: &Request) -> call::Reply {
        #[cfg(feature = "gpu")]
        {
            let params = call::Params {
                query: req.query.clone(),
                body: req.body.clone(),
                tail: None,
            };
            if let Some(reply) = call::author::dispatch_free(&req.method, &req.path, &params) {
                return reply;
            }
            let built = {
                let mut guard =
                    crate::perf::locked(&self.state, "author.state").expect("author state lock");
                let mut actx = call::author::Ctx { state: &mut guard };
                call::author::build(&req.method, &req.path, &params, &mut actx)
            };
            return match built {
                Some(built) => built.into_reply(),
                None => call::Reply {
                    status: 404,
                    kind: "text/plain; charset=utf-8",
                    body: b"not found".to_vec(),
                },
            };
        }
        #[cfg(not(feature = "gpu"))]
        {
            let _ = req;
            call::Reply {
                status: 404,
                kind: "text/plain; charset=utf-8",
                body: b"not found".to_vec(),
            }
        }
    }

    /// One movement out of the studio's reuse library, as plain float rows —
    /// what `POST /overlay` needs to arm a trigger (§16.7).
    ///
    /// Here rather than in `overlay.rs` for the same reason [`serve`] is:
    /// `author` is gated on `gpu`,
    /// because the studio previews with the real wgpu renderers, and this
    /// struct is the one place in this file that knows how to have that both
    /// ways. `overlay.rs` stays a pure player that compiles on every build,
    /// and this is the one door it reads its data through — the alternative
    /// was a second copy of `library::load`, which is the kind of second copy
    /// `node_catalog` below has a whole paragraph about.
    ///
    /// **A rig movement is refused rather than coerced.** Its ticks are joint
    /// rotations — `metal_rig::frame::Frame`, a map of quaternions — not a row of
    /// a model's floats, and there is no sense in which one can be played over
    /// a layer. Refusing by name is what lets the page grey those entries with
    /// their reason instead of hiding them (§16.4).
    ///
    /// Returns the model id the rows belong to, and the rows.
    pub fn movement(&self, name: &str) -> Result<(String, Vec<Vec<f32>>), String> {
        #[cfg(feature = "gpu")]
        {
            let Some(m) = author::library::get(author::LIBRARY, name) else {
                return Err(format!("no movement called {name:?}"));
            };
            let Some(model) = m.model.clone() else {
                return Err(format!(
                    "{name:?} is a rig movement — its ticks are joint rotations, \
                     not a row of a model's floats, so there is nothing to play over a layer"
                ));
            };
            let mut rows: Vec<Vec<f32>> = Vec::with_capacity(m.ticks.len());
            for (i, tick) in m.ticks.iter().enumerate() {
                let Some(cells) = tick.as_array() else {
                    return Err(format!("{name:?}: tick {i} is not a row of floats"));
                };
                let mut row = Vec::with_capacity(cells.len());
                for cell in cells {
                    // Not `unwrap_or(0.0)`. `library::check` refuses a
                    // non-number on the way in, so reaching this means the
                    // file was edited by hand — and defaulting it would put a
                    // zero in a float slot with nothing anywhere saying a
                    // value had been invented.
                    let Some(v) = cell.as_f64() else {
                        return Err(format!(
                            "{name:?}: tick {i} has something that is not a number"
                        ));
                    };
                    row.push(v as f32);
                }
                rows.push(row);
            }
            return Ok((model, rows));
        }
        #[cfg(not(feature = "gpu"))]
        {
            let _ = name;
            Err("this build has no movement library".into())
        }
    }

    /// `R22.8` point 4: the mask a movement is cut by while it plays (`None`
    /// for a movement with no mask, an unknown one, or a build without the
    /// library). The same `author::library::get` [`Self::movement`] reads.
    pub fn movement_mask(&self, name: &str) -> Option<String> {
        #[cfg(feature = "gpu")]
        {
            author::library::get(author::LIBRARY, name).and_then(|m| m.mask)
        }
        #[cfg(not(feature = "gpu"))]
        {
            let _ = name;
            None
        }
    }
}

/// Serializes the cwd dance of every real-fixture test: each one chdirs to
/// the repo root, reads `data/` through `Author::new()`, and chdirs back. In
/// parallel those dances corrupt each other's fixtures — a lock is cheaper
/// than a lost test. A poisoned lock carries no wrong-cwd invariant (the holder
/// only borrows the mutex; cwd is restored unconditionally), so a panic
/// mid-dance must not poison every later user — keep the queue going.
pub fn chdir_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(std::sync::Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Everything a request touches, owned together: the app state, the
/// painter and the input. `serve` locks, borrows all three, and unlocks —
/// once per request.
///
/// **`S.U7b` — shared, not owned by one thread.** `S.U6` asked for
/// `Arc<Mutex<Shared>>` between the window and the listener so a panel could
/// build a `call::Ctx` without a socket, and could not land it: `Shared`
/// was `!Send`, for two reasons outside this crate. `Box<dyn Paint>` held
/// `metal_render::headless::Renderer`, whose `Vec<Box<dyn Model>>` was `!Send`
/// because `metal_render::model::Model` had no `Send` supertrait (its
/// implementors are wgpu pipelines and buffers, all `Send + Sync` — a
/// missing bound, not a real constraint); and `Input::Live` held
/// `metal_listen::capture::Listener`, whose `cpal::Stream` is
/// `NotSendSyncAcrossAllPlatforms` by cpal's design. Both are fixed now:
/// `Model: Send` (`metal-render`, one line — see its doc comment), and
/// `Listener` opens the `cpal::Stream` on the worker thread it already
/// spawns rather than on the caller's (`metal-listen::capture::CpalOwner` —
/// see its doc comment for why the handle it hands back holds only a stop
/// flag and a `JoinHandle`). `paint::Paint` also gained a `Send` supertrait
/// (`paint.rs`) — the piece `S.U6`'s note did not need to name, because
/// `Box<dyn Paint>` needs the trait object itself to declare `Send`, not
/// just its concrete implementors. `Shared` is `Send` as a result, with no
/// `unsafe impl` anywhere: this struct's shape did not change, only how it
/// is held — `main_windowed` wraps it in `Arc<Mutex<Shared>>` alongside an
/// `Arc<Author>`, `serve` (below) locks it per request exactly as `main`
/// once locked a bare `Mutex<State>`, and `call::door::Door` (`S.U7b`) locks
/// the same pair from the shell's own thread. `State` was already `Send`.
pub struct Shared {
    pub state: State,
    pub paint: Box<dyn Paint>,
    /// Plan "Audio input is a node": the listener pool — one listener per
    /// distinct `audio.live` source, the seed's being the musical clock.
    /// Derefs to the clock's [`live::Input`] for the paths that only need a
    /// beat count.
    pub sim: Listeners,
}

#[cfg(test)]
/// C4: `data/graphs/*.json` as a flat list, for the merged page's
/// collections panel. Read-only and minimal on purpose — one graph's full
/// state (nodes, edges, layout, lint) is `GET /graph?id=`, below; this route
/// only exists so a list of collections can be painted without loading every
/// graph in full. A file that fails to parse is skipped rather than failing
/// the whole listing: one corrupt graph must not blank the list for every
/// other one.
pub fn list_graphs() -> Vec<Value> {
    let dir = metal_perform::engine::graphs_dir(&output::data_root());
    graph_rows(&crate::store::FsStore::new(dir), "")
}

/// [`list_graphs`] over any [`Store`]: every `*.json` directly under `dir`
/// that parses as a graph, as `{id, name, domain}`, sorted by id. The browser
/// calls it with `"graphs"` on its bundled store; the native host with its own
/// graphs directory as the root.
pub fn graph_rows(store: &dyn Store, dir: &str) -> Vec<Value> {
    let mut graphs: Vec<Value> = store
        .list(dir)
        .into_iter()
        .filter(|name| name.ends_with(".json"))
        .filter_map(|name| {
            let path = if dir.is_empty() {
                name
            } else {
                format!("{dir}/{name}")
            };
            store.read_text(&path)
        })
        .filter_map(|text| metal_graph::Graph::from_json(&text).ok())
        .map(|g| {
            // `published` marks a generated `device-<screen>` graph: the
            // canvas lists collections, not these (`metal_graph::collection`).
            json!({
                "id": g.id,
                "name": g.name,
                "domain": g.domain,
                "published": g.source.is_some(),
                "devices": metal_graph::collection::bound_devices(&g),
            })
        })
        .collect();
    graphs.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    graphs
}

#[cfg(test)]
/// §16.4: the node catalog, served exactly as [`metal_graph::NodeKind::catalog`]
/// returns it — 34 generators (32 of §2 plus `scene3d`, H.1, and `dla`) and
/// 16 effects derived from `data/schema.json`, plus the twelve fixed kinds.
///
/// **Why this is not `/graph/node`, and must not grow into it.** This reads a
/// pure function of the manifest. Nothing here creates, mutates or persists a
/// node; there is no graph on the other end of it. The node/edge mutation
/// surface is `POST /graph/node`/`POST /graph/edge`, below — kept separate so
/// this route can stay exactly what it says: "what kinds exist", answerable
/// with no graph loaded at all.
///
/// **Why the page was not just handed a literal list instead.** The same
/// reason `catalog()` itself reads the manifest rather than hard-coding one:
/// this repo already shipped the bug where a hand-written model list and
/// `data/schema.json` silently disagreed about three models (see
/// `manifest.rs`, and `metal-graph`'s own `catalog_matches_schema` test). A
/// second copy in the page would rot the same way, and the symptom — a menu
/// entry that creates a source pointed at a model index that no longer means
/// what the menu thought — is the silent, in-range, wrong failure §3.3 and
/// §11.7 both name.
///
/// **Why the manifest is read per request and not cached.** This is a
/// top-bar menu a human opens, not a pump: the page asks once on load. A
/// `OnceLock` here would buy nothing measurable and would need a story for
/// what happens when `data/schema.json` changes under a running app, which
/// reading it fresh does not.
///
/// An unreadable or unparseable `schema.json` is reported as such rather than
/// answered with an empty list. Empty would render as a catalog with nothing
/// in it — which is a lie of the exact kind §16.4 is about, and the caller
/// cannot tell it apart from "this build has no models".
pub fn node_catalog() -> Result<Vec<metal_graph::NodeKind>, String> {
    node_catalog_in(&crate::store::FsStore::new(output::data_root()))
}

/// [`node_catalog`] over any [`Store`], reading `schema.json` at its root.
pub fn node_catalog_in(store: &dyn Store) -> Result<Vec<metal_graph::NodeKind>, String> {
    Ok(metal_graph::NodeKind::catalog(&node_manifest_in(store)?))
}

/// The manifest `schema.json` holds, read per request (see [`node_catalog`]).
/// The catalog and the relevance scorer both read it, so one read answers both.
pub fn node_manifest() -> Result<std::sync::Arc<metal_schema::Manifest>, String> {
    // `/catalog` asks on every refresh, and parsing the ~1 MB `schema.json`
    // each time was most of its cost, under the shell's lock. The parse is
    // kept while the file keeps its mtime and length.
    // The mtime as its `Debug` text: the browser's clock type is not `std`'s.
    type Sig = (std::path::PathBuf, (String, u64));
    static MEMO: std::sync::Mutex<Option<(Sig, std::sync::Arc<metal_schema::Manifest>)>> =
        std::sync::Mutex::new(None);
    let path = output::data_root().join("schema.json");
    let stamp = metal_vfs::metadata(&path)
        .ok()
        .and_then(|m| Some((format!("{:?}", m.modified().ok()?), m.len())));
    let sig = stamp.map(|s| (path.clone(), s));
    if let (Some(sig), Ok(g)) = (sig.as_ref(), MEMO.lock()) {
        if let Some((kept, manifest)) = g.as_ref() {
            if kept == sig {
                return Ok(std::sync::Arc::clone(manifest));
            }
        }
    }
    let manifest = std::sync::Arc::new(node_manifest_in(&crate::store::FsStore::new(
        output::data_root(),
    ))?);
    if let (Some(sig), Ok(mut g)) = (sig, MEMO.lock()) {
        *g = Some((sig, std::sync::Arc::clone(&manifest)));
    }
    Ok(manifest)
}

fn node_manifest_in(store: &dyn Store) -> Result<metal_schema::Manifest, String> {
    let text = store
        .read_text("schema.json")
        .ok_or_else(|| "cannot read schema.json".to_string())?;
    metal_schema::Manifest::parse(&text).map_err(|e| format!("schema.json did not parse: {e}"))
}

#[cfg(test)]
/// H.8: the provider-config overlay's backend half, `GET`/`POST /ai-config`.
///
/// This is the *only* route that touches `data/ai.json`, on purpose — PARTE
/// IV's self-consistency rule says the agent loop's own future read of this
/// config (H.3/H.4) has to go through the same door the UI overlay writes
/// through, not a second one. `metal_agent::config::ProviderConfig` already
/// carries the non-negotiable rule (H.3: never log the raw key); this layer's
/// only added job is making sure the raw key never round-trips back to the
/// browser either — `GET` always answers with the masked tail, never `key`.
///
/// S.AH: `preset`/`url`/`model` are governed by `[agent.provider]` in
/// `~/.config/metalvj/config.toml` once that table exists — `load`/`save`
/// read and mirror it themselves (see `metal-agent/src/config.rs`). The `_at`
/// helpers take the `config.toml` path explicitly only so the tests below
/// can point them at a scratch file instead of the developer's real one.
pub fn ai_config_path() -> std::path::PathBuf {
    output::data_root().join("ai.json")
}

#[cfg(test)]
/// `{ "configured": bool, "url", "model", "preset", "key_masked" }`. All four
/// fields are omitted (not just blanked) when nothing has been saved yet, so
/// the page can tell "no provider configured" apart from "configured with an
/// empty model name" without a magic-string convention.
#[cfg(feature = "desktop")]
pub fn ai_config_get_reply_at(
    path: impl AsRef<std::path::Path>,
    config_toml: &std::path::Path,
) -> Result<Value, String> {
    match metal_agent::config::ProviderConfig::load_at(path, config_toml)
        .map_err(|e| format!("cannot read ai.json/config.toml: {e}"))?
    {
        None => Ok(json!({ "configured": false })),
        Some(cfg) => Ok(json!({
            "configured": true,
            "url": cfg.url,
            "model": cfg.model,
            "preset": cfg.preset,
            "key_masked": cfg.masked_key(),
        })),
    }
}

#[cfg(test)]
/// Applies a `POST /ai-config` body to disk and returns the same masked shape
/// `ai_config_get_reply` does, so the page can repaint from the response
/// without a follow-up `GET`.
///
/// `key` is optional in the body on purpose: the page shows a masked
/// placeholder rather than the real value (see the route's own doc comment),
/// so "the operator didn't touch the key field" and "the operator wants to
/// clear the key" have to be distinguishable. An absent or empty `key` keeps
/// whatever was already on disk; the only way to change it is to type a new,
/// non-empty one.
#[cfg(feature = "desktop")]
pub fn ai_config_post_at(
    path: impl AsRef<std::path::Path>,
    config_toml: &std::path::Path,
    body: &Value,
) -> Result<Value, String> {
    let preset: metal_agent::config::Preset = serde_json::from_value(body["preset"].clone())
        .map_err(|_| "invalid or missing preset".to_string())?;
    let url = body["url"].as_str().unwrap_or_default().to_string();
    let model = body["model"].as_str().unwrap_or_default().to_string();
    let path = path.as_ref();
    let existing = metal_agent::config::ProviderConfig::load_at(path, config_toml)
        .map_err(|e| format!("cannot read ai.json/config.toml: {e}"))?;
    let key = match body["key"].as_str() {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => match &existing {
            // A provider that exists only in `config.toml` (S.AH) loads with
            // an empty key — there is nothing on disk to keep, so it is the
            // same 400 as no file at all.
            Some(cfg) if !cfg.key.is_empty() => cfg.key.clone(),
            _ => return Err("expected a key".to_string()),
        },
    };
    // H.6: `stt`/`tts` are carried over from the existing file instead of being
    // left as `None`. This route only edits the *text* provider — the AI config
    // overlay (H.8) shows no voice — so rebuilding the struct from scratch here
    // would silently erase a voice configuration the user wrote by hand in
    // `ai.json`. Saving replaces the whole file, not a merge, and that is why
    // the merge has to happen here.
    let (stt, tts) = match &existing {
        Some(cfg) => (cfg.stt.clone(), cfg.tts.clone()),
        None => (None, None),
    };
    let cfg = metal_agent::config::ProviderConfig {
        url,
        model,
        key,
        preset,
        stt,
        tts,
    };
    cfg.save_at(path, config_toml)
        .map_err(|e| format!("cannot write ai.json/config.toml: {e}"))?;
    Ok(json!({
        "configured": true,
        "url": cfg.url,
        "model": cfg.model,
        "preset": cfg.preset,
        "key_masked": cfg.masked_key(),
    }))
}

#[cfg(test)]
/// `S.U14`: which of `metal_agent::harness::TOOL_CATALOG`'s tools the sidebar's
/// per-tool toggle has turned off, keyed by name. Its own file for the same
/// reason `ai_config_path` has one — rule 6's "no second state of the
/// world" rule means the sidebar's toggle and the agent's own future read of
/// it (`S.X`) have to agree on one place on disk, not a shell-local bool a
/// resize would forget.
pub fn ai_tools_path() -> std::path::PathBuf {
    output::data_root().join("ai_tools.json")
}

#[cfg(test)]
/// Absent file, or an absent name in it, both mean "the default policy" — a
/// fresh install starts with the whole implemented catalog available, the
/// destructive tools defaulting to `ask` (the flags in
/// [`metal_agent::tool_schemas`]). Legacy `{name: bool}` maps load unchanged
/// (`policy::resolve` folds `true → auto`, `false → off`).
pub fn ai_tools_load(path: impl AsRef<std::path::Path>) -> Value {
    metal_vfs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or_else(|| json!({}))
}

#[cfg(test)]
pub fn ai_tools_save(path: impl AsRef<std::path::Path>, map: &Value) -> Result<(), String> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        metal_vfs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::atomic::write(path, serde_json::to_string_pretty(map).unwrap_or_default())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
/// H.4's own route correspondence (`crates/metal-agent/src/harness.rs`'s module
/// doc: *"a thin wrapper over a route that already exists"*), moved to the
/// one place that can name both `metal_agent::harness::TOOL_CATALOG` and
/// `crate::call::ROUTES`/`crate::call::author::AUTHOR_ROUTES` without a
/// `shell/` file ever importing `metal_agent` directly (gate 1). A path here
/// that starts with `/author` is exactly what a panel would call through
/// the door (`crate::call::door::Door::call`'s own `/author` prefix
/// bifurcation) — never the bare `AUTHOR_ROUTES` entry's path on its own.
///
/// Three groups of `None`, each flagged rather than guessed at:
/// - `capture_output`: `TOOL_CATALOG` itself already marks this
///   unimplemented (no route exists to wrap — see `harness.rs`'s own doc).
/// - `sleep`, `locate`, `capture_camera_frame`, `bake_clip`: implemented,
///   but entirely inside the agent process — H.4's doc calls `sleep` out as
///   "wait", not an app mutation, and `locate`/`capture_camera_frame`/
///   `bake_clip` write files (`turn.rs`'s `Locating`, `metal_ingest::camera`,
///   `data/clips/`) that no `metal-dash` route reads or writes today. Rule 6
///   is satisfied vacuously here — there is no human control for any of the
///   four in the shell today either, so there is nothing for the sidebar's
///   control to be out of parity *with*.
/// - `get_params`/`set_params`, and `rasterize_overlay`: the closest
///   *registered* door surface, not a byte-identical one.  `get_params`/
///   `set_params` read/patch one node's free-form fields —`POST /graph/node`
///   (`op: "flag"`/`"layer"`) is the nearest existing op, not a generic
///   patch. `rasterize_overlay`'s own `metal-dash` executor
///   (`author::models::agent_tools::AgentHarness::rasterize_overlay`) is
///   itself a documented placeholder ("in a real implementation, this would
///   POST to /overlay/rasterize" — that route does not exist), so this maps
///   it to `POST /overlay`, the real route in the same family, rather than
///   inventing the placeholder's imagined one.
pub fn tool_route(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "list_tools" => Some(("GET", "/ai-config")),
        "get_env" | "list_nodes" | "get_params" => Some(("GET", "/graph")),
        "search_nodes" => Some(("GET", "/catalog")),
        "create_node" | "delete_node" | "set_params" => Some(("POST", "/graph/node")),
        // M4 of the materials batch: the `mat.*` node's part and fixed
        // overrides, the same `POST /graph/node {op:"config"}` merge its
        // properties page posts (rule 6).
        "set_material" => Some(("POST", "/graph/node")),
        // A1/A3.3: the embedded reaction is a setting in the source, over the
        // same `POST /graph/node` op its properties toggle posts.
        "embed_ai" => Some(("POST", "/graph/node")),
        // Surface sources: the two generic ops of the same route.
        "attach_source" | "insert_node" => Some(("POST", "/graph/node")),
        "connect_nodes" => Some(("POST", "/graph/edge")),
        // S.AM: the train gesture of an `ai.*` node — the same route its
        // `entrenar` button posts (`POST /ai/train { node }`).
        "train_ai" => Some(("POST", "/ai/train")),
        "read_shader" => Some(("GET", "/graph/effects")),
        // §9.4 V3: the script node's own source over the `/script` route the
        // properties editor walks (rule 6).
        "read_script" => Some(("GET", "/script/")),
        "save_script" => Some(("POST", "/script")),
        "raster" => Some(("POST", "/graph/effects")),
        "list_switches" => Some(("GET", "/switch")),
        "list_transitions" => Some(("GET", "/transitions")),
        "set_switch" => Some(("POST", "/switch")),
        // V2b: the tick-level switch is a shell setting; both tools use the
        // one `/settings` route the settings overlay uses (rule 6).
        "get_tick_level" => Some(("GET", "/settings")),
        "set_tick_level" => Some(("POST", "/settings")),
        // H5: the head-tracking gesture — the same `POST /sensor {op:"set"}`
        // the `sensor.yolo` panel's controls post.
        "set_head_tracking" => Some(("POST", "/sensor")),
        // P-G: the preview bar's "Exportar cuadro".
        "export_frame" => Some(("POST", "/frame/export")),
        // E1: the offline video export job.
        "export_video" => Some(("POST", "/export/video")),
        "export_queue" => Some(("POST", "/export/queue")),
        // P-D: Art-Net discovery and the timecode reading.
        "artnet_nodes" => Some(("GET", "/artnet/nodes")),
        "artnet_in_status" => Some(("GET", "/artnet/in")),
        "timecode_status" => Some(("GET", "/timecode")),
        // P-C: the properties page's "Send to" reads and writes these.
        "list_video_io" => Some(("GET", "/videoio")),
        "set_output_sink" => Some(("POST", "/outputs")),
        // D3: the universe bus's per-source priority/mode.
        "dmx_bus" => Some(("POST", "/outputs")),
        "create_mask_grid" => Some(("POST", "/mask/grid")),
        "test_mask_grid" => Some(("POST", "/mask/project")),
        "import_segment" | "move_segment" | "remove_segment" => {
            Some(("POST", "/author/models/timeline/segment"))
        }
        "set_keyframe" => Some(("POST", "/author/models/example")),
        "attach_digest" => Some(("POST", "/author/digest")),
        // `S.O.1`: real now — its own route, not a placeholder's imagined one.
        "blackout" => Some(("POST", "/overlay/blackout")),
        // `S.O.2`: real now — `POST /overlay {op:"solid"}`.
        "solid_color" => Some(("POST", "/overlay")),
        "rasterize_overlay" => Some(("POST", "/overlay")),
        // S.D (O.3): a segment's `SegmentMix` — the tool resubmits the whole
        // segment through the timeline's own segment route, never `/layers`
        // (`author::models::agent_tools::AgentHarness::set_layer_mix`).
        "set_layer_mix" => Some(("POST", "/author/models/timeline/segment")),
        // O.1's authoring half: the one route that writes a mask.
        "edit_mask" => Some(("POST", "/outputs")),
        // P3: the venue's authoring ops — the surface's geometry and one
        // projector's pins — share `POST /venue` with `project`.
        "edit_venue_surface" | "calibrate_projector" | "blend_edges" => Some(("POST", "/venue")),
        // 3D venues: the route that writes the venue's masks; the tool then
        // posts `/overlay` and `/monitor` like the panel buttons do.
        "project_venue" => Some(("POST", "/venue")),
        "look_venue" => Some(("GET", "/venue")),
        // M2: the pixel map's live aim — the same `POST /venue { op: "aim" }`
        // the venue editor's click-to-aim posts.
        "aim_fixture" => Some(("POST", "/venue")),
        // D1: the venue's fixtures — the same `POST /venue { op: "fixture" }`
        // the Settings page posts.
        "set_fixture" => Some(("POST", "/venue")),
        // F9: the per-fixture node's arguments are not the route body, so it
        // runs through the chat's executor, posting `POST /graph/node` and
        // `POST /graph/edge` itself (rule 6).
        "add_fixture_node" => Some(("POST", crate::call::AGENT_TOOL_ROUTE)),
        // M5: the camera tools all write a draft through `POST /venue
        // { op: "draft" }`, the route the editor's diff view accepts/rejects.
        "scan_venue" | "calibrate_projector" | "discover_fixture" | "solve_head" => {
            Some(("POST", "/venue"))
        }
        "listen_status" => Some(("GET", "/listen/status")),
        "edit_layer" => Some(("POST", "/layers")),
        // Param edges (`metal_graph::bind`): the properties panel's I/O read and
        // its two writes, `op: "bind"` and `op: "param"`.
        "get_io" => Some(("GET", "/graph/io")),
        "bind_param" | "set_param" => Some(("POST", "/graph/node")),
        // K: the palette node's write — the same `/author/palettes {op:"set"}`
        // its folder posts (the exported route is `/author/palettes`).
        "set_palette" => Some(("POST", "/author/palettes")),
        // Video scenes: the gallery's read, the book's writes.
        "list_scenes" => Some(("GET", "/author/scenes")),
        // The sequence editor's dataset picker: the list, and one part with `part`.
        "list_dataset" => Some(("GET", "/author/dataset")),
        "edit_scene" => Some(("POST", "/author/scenes")),
        _ => None,
    }
}

#[cfg(test)]
/// `GET /ai-config`'s `tools` array, and the tools-only half of
/// `POST /ai-config` reads this same shape back after saving — one tool per
/// `TOOL_CATALOG` entry, `enabled` folding in both "does this build actually
/// implement it" and "did the operator turn it off", and `route` naming the
/// exact `(method, path)` [`tool_route`] maps it to (`null` for the
/// documented gap list on that function's own doc comment).
#[cfg(feature = "desktop")]
pub fn ai_tools_reply(tools_path: impl AsRef<std::path::Path>) -> Value {
    let saved = ai_tools_load(tools_path);
    let tools: Vec<Value> = metal_agent::harness::TOOL_CATALOG
        .iter()
        .map(|(name, desc, implemented)| {
            // `S.X` D10: the per-tool setting is now a policy (`auto`/`ask`/
            // `off`), with the default from the schema flags. `enabled` stays
            // as the summed "this tool will be offered to the model" flag the
            // old readers (and tests) expect.
            let policy = if *implemented {
                metal_agent::chat::policy::resolve(name, &saved)
            } else {
                metal_agent::chat::policy::ToolPolicy::Off
            };
            let enabled = *implemented && policy != metal_agent::chat::policy::ToolPolicy::Off;
            let route = tool_route(name);
            json!({
                "name": name,
                "description": desc,
                "implemented": implemented,
                "enabled": enabled,
                "policy": policy.wire(),
                "route": route.map(|(m, p)| json!({ "method": m, "path": p })).unwrap_or(Value::Null),
            })
        })
        .collect();
    json!({ "tools": tools })
}

#[cfg(test)]
/// Applies the sidebar's tool setting: `{ "tools": { name: "auto"|"ask"|"off" } }`
/// (or the legacy `{name: bool}`). Silently refuses a policy for a tool
/// `TOOL_CATALOG` marks `implemented: false`.
#[cfg(feature = "desktop")]
pub fn ai_tools_post_at(path: impl AsRef<std::path::Path>, body: &Value) -> Result<Value, String> {
    let path = path.as_ref();
    let mut saved = ai_tools_load(path);
    if !saved.is_object() {
        saved = json!({});
    }
    if let Some(patch) = body["tools"].as_object() {
        for (name, v) in patch {
            let implemented = metal_agent::harness::TOOL_CATALOG
                .iter()
                .any(|(n, _, imp)| *n == name && *imp);
            if !implemented {
                continue;
            }
            let parsed = v
                .as_str()
                .and_then(metal_agent::chat::policy::ToolPolicy::parse)
                .or_else(|| {
                    v.as_bool().map(|b| {
                        if b {
                            metal_agent::chat::policy::ToolPolicy::Auto
                        } else {
                            metal_agent::chat::policy::ToolPolicy::Off
                        }
                    })
                });
            if let Some(policy) = parsed {
                saved["policy"][name] = json!(policy.wire());
            }
        }
    }
    ai_tools_save(path, &saved)?;
    Ok(ai_tools_reply(path))
}

#[cfg(test)]
/// `S.U14`: the sidebar's prompt transcript — `data/ai_log.json`, a plain
/// JSON array of `{ "role", "content", "at" }`, append-only from
/// `POST /ai/ask`, read whole by `GET /ai/log`. **This route does not call a
/// model.** `plan-opus.md` names the harness's own turn loop
/// (`metal_agent::agent_loop::run_turn`, the per-preset adapter it still
/// needs) as `S.X`'s job, not this one's — `S.U14`'s scope is the sidebar
/// and rule 6's parity, not standing up the first real network call to a
/// provider. Recording what was asked, honestly, rather than faking an
/// answer, is the same "flagged, not faked" trade `harness.rs`'s own module
/// doc makes for the tools it does not implement.
pub fn ai_log_path() -> std::path::PathBuf {
    output::data_root().join("ai_log.json")
}

#[cfg(test)]
pub fn ai_log_load(path: impl AsRef<std::path::Path>) -> Vec<Value> {
    metal_vfs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[cfg(test)]
pub fn ai_ask_post_at(path: impl AsRef<std::path::Path>, body: &Value) -> Result<Value, String> {
    let prompt = body["prompt"].as_str().unwrap_or_default();
    if prompt.trim().is_empty() {
        return Err("expected a non-empty `prompt`".to_string());
    }
    let path = path.as_ref();
    let mut log = ai_log_load(path);
    let at = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    log.push(json!({ "role": "user", "content": prompt, "at": at }));
    if let Some(parent) = path.parent() {
        metal_vfs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::atomic::write(path, serde_json::to_string_pretty(&log).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    Ok(json!({ "queued": true, "log": log }))
}

#[cfg(test)]
/// output is ever turned into JSON, the same discipline `Config` documents
/// for `serde_json` staying out of that crate's public surface.
///
/// Every primitive carries its **tag** (N: the Wire canvas). A painter reads
/// the tag to decide how to draw — a node box, a header band, a port dot, a
/// param dot, a value field, a swatch, a model tile, the `▸ n more` row, a row
/// label, a title, a value readout — and never re-derives which rectangle is
/// which from its size (`Layout::primitives`' own doc). A cable carries its
/// endpoints and its type (`image`/`control`/`param`), so a painter styles a
/// param edge differently from an image cable without re-reading the graph.
pub fn primitive_json(p: &metal_graph::layout::Primitive) -> Value {
    use metal_graph::layout::{Primitive, RectTag, TaggedRect, TextTag};
    match p {
        Primitive::Rect(TaggedRect { rect, tag }) => match tag {
            RectTag::Node { node } => json!({
                "type": "rect", "tag": "node", "node": node,
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
            }),
            RectTag::Header => json!({
                "type": "rect", "tag": "header",
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
            }),
            RectTag::Port { node, port, out } => json!({
                "type": "rect", "tag": "port",
                "node": node, "port": port, "out": out,
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
                "cx": rect.x + rect.w / 2.0, "cy": rect.y + rect.h / 2.0,
            }),
            RectTag::ParamDot { node, name } => json!({
                "type": "rect", "tag": "param_dot",
                "node": node, "name": name,
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
                "cx": rect.x + rect.w / 2.0, "cy": rect.y + rect.h / 2.0,
            }),
            RectTag::ValueField { node, name } => json!({
                "type": "rect", "tag": "value_field",
                "node": node, "name": name,
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
            }),
            RectTag::Swatch { node, name, rgb } => json!({
                "type": "rect", "tag": "swatch",
                "node": node, "name": name,
                "color": [rgb[0], rgb[1], rgb[2]],
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
            }),
            RectTag::Tile { node } => json!({
                "type": "rect", "tag": "tile",
                "node": node,
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
            }),
            RectTag::More { node } => json!({
                "type": "rect", "tag": "more",
                "node": node,
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
            }),
            RectTag::Chip { node, source, ai } => json!({
                "type": "rect", "tag": "chip",
                "node": node, "source": source, "ai": ai,
                "x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h,
            }),
        },
        Primitive::Path(p) => json!({
            "type": "path",
            "edge": p.edge,
            "from": { "node": p.from.0, "port": p.from.1 },
            "to": { "node": p.to.0, "port": p.to.1 },
            "kind": match p.kind {
                metal_graph::layout::EdgeKind::Image => "image",
                metal_graph::layout::EdgeKind::Control => "control",
                metal_graph::layout::EdgeKind::Param => "param",
            },
            "p0": [p.path.p0.x, p.path.p0.y],
            "p1": [p.path.p1.x, p.path.p1.y],
            "p2": [p.path.p2.x, p.path.p2.y],
            "p3": [p.path.p3.x, p.path.p3.y],
        }),
        Primitive::Text(t) => json!({
            "type": "text",
            "x": t.x, "y": t.y, "content": t.content,
            "tag": match t.tag {
                TextTag::Title => "title",
                TextTag::Row => "row",
                TextTag::Value => "value",
                TextTag::More => "more",
                TextTag::Plain => "plain",
            },
        }),
    }
}

#[cfg(test)]
pub fn issue_json(i: &metal_graph::Issue) -> Value {
    json!({
        "severity": match i.severity { metal_graph::Severity::Error => "error", metal_graph::Severity::Warning => "warning" },
        "node": i.node,
        "message": i.message,
    })
}

#[cfg(test)]
/// Thin wrapper over [`metal_graph::Graph::add_edge_checked`] (H.4: moved into
/// `metal-graph` itself so the AI harness's `connect_nodes`/`create_node` tools
/// share the exact same validated-connect logic this route uses, instead of
/// a second copy of "add, lint, maybe roll back").
pub fn add_edge_checked(
    g: &mut metal_graph::Graph,
    from: metal_graph::Endpoint,
    to: metal_graph::Endpoint,
    catalog: &[metal_graph::NodeKind],
) -> Result<String, Vec<String>> {
    g.add_edge_checked(from, to, catalog)
}

#[cfg(test)]
/// The reply every `/graph*` mutation converges on: the graph as saved, its
/// primitives (for the client to repaint from, never computing layout
/// itself), and every lint issue — errors and warnings together, per
/// `lint`'s own doc comment on why it is never a `bool`.
pub fn graph_reply(g: &metal_graph::Graph, catalog: &[metal_graph::NodeKind]) -> Value {
    let layout = g.layout(catalog);
    let issues = g.lint(catalog);
    json!({
        "graph": g,
        "primitives": layout.primitives().iter().map(primitive_json).collect::<Vec<_>>(),
        "issues": issues.iter().map(issue_json).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
/// B4: one generator's sixteen effects, exactly as the config panel shows
/// them — [`metal_graph::effects::view`]'s present effects first, in chain
/// order (§3.2: the order *is* the graph order), then every other `fx.*`
/// kind the catalog knows about, marked `off`. Built fresh from the graph on
/// every call — never cached, never stored beside it — which is the whole
/// answer to §3.2's warning that a second copy of this list is how "which
/// effects are on" and "what the graph says" end up disagreeing.
pub fn effects_reply(
    graph: &metal_graph::Graph,
    gen_node: &str,
    catalog: &[metal_graph::NodeKind],
) -> Value {
    let present = metal_graph::effects::view(graph, gen_node);
    let mut seen: std::collections::HashSet<String> =
        present.iter().map(|v| v.effect.clone()).collect();
    let mut out: Vec<Value> = present.iter().map(effect_view_json).collect();
    for k in catalog {
        if k.family != metal_graph::NodeFamily::Effect {
            continue;
        }
        // S.W: `fx.text.*` is a *glyph* vocabulary, not a pixel chain step, so
        // it is not one of the effects a generator's own panel offers.
        if k.kind.starts_with("fx.text.") {
            continue;
        }
        // `fx.forge` is not an amount to switch on: it is a shader, named on
        // the node when it is created (`create_node` with `after`). A panel
        // row that switched it "on" would make a node with nothing to run.
        // One already in the chain is in `present` above and shows there.
        if k.kind == metal_graph::FORGE_EFFECT_KIND {
            continue;
        }
        // K: `fx.speed`/`fx.move` are not `Post` amounts either — they act on
        // the clock and on the picture, not on a generator's row, so a panel
        // toggle for them would be a control that does nothing. Same rule as
        // `fx.forge`: offered only when already in the chain (`present`).
        if k.kind == format!("fx.{}", metal_graph::SPEED_EFFECT_ID)
            || k.kind == format!("fx.{}", metal_graph::MOVE_EFFECT_ID)
        {
            continue;
        }
        let name = k.kind.strip_prefix("fx.").unwrap_or(&k.kind).to_string();
        if seen.insert(name.clone()) {
            out.push(json!({ "effect": name, "node": Value::Null, "state": "off" }));
        }
    }
    json!({ "node": gen_node, "effects": out })
}

#[cfg(test)]
pub fn effect_view_json(v: &metal_graph::effects::EffectView) -> Value {
    let mut j = json!({ "effect": v.effect, "node": v.node });
    match v.state {
        metal_schema::EffectState::Off => {
            j["state"] = json!("off");
        }
        metal_schema::EffectState::Reactive => {
            j["state"] = json!("reactive");
            // `control_source` is `None` only when disconnected, which
            // `state_of`'s own rule already rules out for `Reactive` — the
            // `None` arm here is defensive, not reachable, per E4.
            j["source"] = json!(match v.source {
                Some(metal_graph::effects::ControlSource::Row) => "row",
                Some(metal_graph::effects::ControlSource::RawIngest) | None => "raw",
            });
        }
        metal_schema::EffectState::Fixed(val) => {
            j["state"] = json!("fixed");
            j["value"] = json!(val);
        }
    }
    j
}

/// Which contour a `/outputs` request just wrote, by id.
///
/// The one written by id is that id; the one written with no id was appended,
/// so it is the last on the device. `None` for every other op, and for a delete
/// — there is nothing to bind to.
pub fn touched_mask(state: &State, device: &str, body: &Value) -> Option<String> {
    if body["op"].as_str() != Some("mask") || body["points"].as_array().is_none() {
        return None;
    }
    let out = state.outputs.find(device)?;
    match body["mask"].as_str() {
        Some(id) => out.mask(id).map(|m| m.id.clone()),
        None => match body["index"].as_u64() {
            Some(at) => out.masks.get(at as usize).map(|m| m.id.clone()),
            None => out.masks.last().map(|m| m.id.clone()),
        },
    }
}

/// Draw a mask's outline in a specified color on a pixel buffer (for /mask/project).
/// Uses Bresenham line algorithm (same as draw_calibration in output.rs).
pub fn draw_mask_outline(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    mask: &output::Mask,
    color: [u8; 3],
) {
    if mask.points.len() < 2 {
        return; // Not enough points to draw
    }

    let (w, h) = (width as usize, height as usize);
    let mut plot = |x: isize, y: isize| {
        if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
            return;
        }
        let p = (y as usize * w + x as usize) * 4;
        if p + 2 < pixels.len() {
            pixels[p] = color[0];
            pixels[p + 1] = color[1];
            pixels[p + 2] = color[2];
        }
    };

    // Draw lines between consecutive points
    let n = mask.points.len();
    for i in 0..n {
        let a = mask.points[i];
        let b = mask.points[(i + 1) % n];
        let x0 = (a[0] * width as f32) as isize;
        let y0 = (a[1] * height as f32) as isize;
        let x1 = (b[0] * width as f32) as isize;
        let y1 = (b[1] * height as f32) as isize;

        // Bresenham line algorithm
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        loop {
            plot(x, y);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }
}

/// The fields a layer accepts from the page, applied where they are present.
///
/// One function for creating and editing, so a control that works in the
/// creation panel works inline in the table without being wired twice — which
/// is exactly the drift the ask is guarding against by asking for both.
pub fn apply_fields(layer: &mut Layer, body: &Value, models: usize) {
    if let Some(v) = body["name"].as_str() {
        layer.name = v.to_string();
    }
    // What the source draws. `model` alone still works and still means "by
    // hand" — the page sends both together when an animation is picked, so an
    // operator choosing one out of the dropdown leaves whatever feed it was on.
    if let Some(v) = body["model"].as_u64().map(|v| v as usize) {
        if v < models {
            layer.model = v;
        }
    }
    if let Some(v) = body["feed"].as_str().and_then(layers::Feed::parse) {
        layer.feed = v;
    }
    if let Some(v) = body["picked"].as_array() {
        // Filtered here rather than trusted: this list is handed to
        // `Layer::showing`, which indexes the renderer with it.
        let mut p: Vec<usize> = v
            .iter()
            .filter_map(Value::as_u64)
            .map(|n| n as usize)
            .filter(|n| *n < models)
            .collect();
        p.sort_unstable();
        p.dedup();
        layer.picked = p;
    }
    // Which folder of footage this source draws. Same `null` means unbind,
    // absent means leave alone as `mask` below — and unbinding is what puts a
    // source back on the metraje sheet's library.
    //
    // Not checked against what is indexed: a stack is saved on one laptop and
    // opened on another, and refusing the name there would silently drop the
    // binding rather than showing a slate that says the folder is missing.
    if !body["library"].is_null() {
        if let Some(v) = body["library"].as_str() {
            layer.library = (!v.is_empty()).then(|| v.to_string());
        }
    } else if body.get("library").is_some() {
        layer.library = None;
    }
    // Binding a source to a contour, and unbinding it. `null` is the unbind —
    // an absent key means "leave it alone", which is what every other field
    // here means and what lets the page send one changed value at a time.
    if !body["mask"].is_null() {
        if let Some(v) = body["mask"].as_str() {
            layer.mask = (!v.is_empty()).then(|| v.to_string());
        }
    } else if body.get("mask").is_some() {
        layer.mask = None;
    }
    if let Some(v) = body["opacity"].as_f64() {
        layer.opacity = (v as f32).clamp(0.0, 1.0);
    }
    if let Some(v) = body["z"].as_i64() {
        layer.z = v as i32;
    }
    if let Some(v) = body["blend"].as_str().and_then(Blend::parse) {
        layer.blend = v;
    }
    if let Some(v) = body["active"].as_bool() {
        layer.active = v;
    }
    if let Some(v) = body["solo"].as_bool() {
        layer.solo = v;
    }
    if let Some(v) = body["fondo_alpha"].as_bool() {
        layer.fondo_alpha = v;
    }
    // Geometric rotation of this source against its device — compositing
    // `giro`, not the router's rotation and not the FloatSpec of the same
    // name inside Escape/Relampago's own schema. See `Layer::giro` and
    // the design notes PARTE IV §17.2.
    if let Some(v) = body["turn"].as_f64() {
        layer.turn = v as f32;
    }
    if let Some(v) = body["giro_mode"].as_str().and_then(ClipMode::parse) {
        layer.giro_mode = v;
    }
    if let Some(s) = body["sector"].as_object() {
        let get = |k: &str, d: f32| s.get(k).and_then(Value::as_f64).unwrap_or(d as f64) as f32;
        layer.sector = Sector {
            x: get("x", layer.sector.x),
            y: get("y", layer.sector.y),
            w: get("w", layer.sector.w),
            h: get("h", layer.sector.h),
        }
        .clamped();
    }
    if let Some(v) = body.get("render_on").and_then(layers::RenderOn::parse) {
        layer.render_on = v;
    }
    // `Layer::span`: `{device: {x,y,w,h}}` sets each named device's window,
    // `{device: null}` clears one, `span: null` clears them all — the same
    // additive rule every other key here follows.
    match body.get("span") {
        Some(Value::Null) => layer.span.clear(),
        Some(Value::Object(map)) => {
            for (device, win) in map {
                match win.as_object() {
                    Some(s) => {
                        let get = |k: &str, d: f32| {
                            s.get(k).and_then(Value::as_f64).map_or(d, |v| v as f32)
                        };
                        let win = Sector {
                            x: get("x", 0.0).clamp(0.0, 1.0),
                            y: get("y", 0.0).clamp(0.0, 1.0),
                            w: get("w", 1.0).clamp(0.0, 1.0),
                            h: get("h", 1.0).clamp(0.0, 1.0),
                        };
                        layer.span.insert(device.clone(), win);
                    }
                    None => {
                        layer.span.remove(device);
                    }
                }
            }
        }
        _ => {}
    }
    // A `forge` source's shader id (`data/shaders/<id>.wgsl`); `null` clears.
    match body.get("shader") {
        Some(Value::Null) => layer.shader = None,
        Some(Value::String(s)) if !s.trim().is_empty() => layer.shader = Some(s.trim().to_string()),
        _ => {}
    }
}

/// Every layer currently routed to `device`, given fresh `outputs` of just
/// that one device — what "apply a collection" and "select a collection from
/// the mode dropdown" both hand a device: a copy, not a reference, per
/// [`Collection`]'s own note on why applying copies.

/// Applies one `POST /drive` body to `state`.
///
/// Pulled out of the route handler for the same reason [`apply_layers_op`]
/// below was: the mutation can be tested directly, with no socket in sight —
/// this file has no fixture for driving the HTTP layer itself. A pure
/// extraction, not a change of contract: the route still takes exactly the
/// same body and still updates `state.post`, so `GET /state` reports the
/// "master" value exactly as it did before this function existed.
///
/// **What changed underneath — the design notes PARTE IV §17.4 Conjunto E.**
/// `state.post` used to be read straight off by `apply_master_post` at
/// render time, so it was already "the picture" as well as "the report".
/// `apply_master_post` is gone: every layer now carries its own
/// `effects`/`fijo_mask` (see [`apply_node_post`]). So for the master dial to
/// keep doing what it has always visibly done, every effect field the body
/// brings is *also* written into every layer in the pool, fixing that
/// layer's own bit. An operator who never touches a layer's own effects
/// cannot tell the difference; a future per-layer control (needs the node
/// editor — E3, not this step) would simply stop being overwritten here, the
/// same way `apply_fields`'s "only touch what came" already leaves anything
/// the request did not mention alone.
///
/// The reaction dials (`depth`, `smooth`, `colour`, `palette`) are no longer
/// here: they are each source's own (`drive` in its node config, written by
/// `POST /graph/node {op:"config"}` — plan "Audio input is a node", Design 4).
pub fn apply_post_op(state: &mut State, sim: &mut Listeners, body: &Value) {
    // Both are no-ops on a real input — you cannot ask a room to play
    // at a different tempo. `controllable` in /state is what the page
    // reads to grey them out rather than offering a dead slider.
    if let Some(v) = body["bpm"].as_f64() {
        sim.set_bpm(v as f32);
    }
    if let Some(v) = body["level"].as_f64() {
        sim.set_drive(v as f32);
    }
    // The master effects dial. Each field below still updates `state.post`
    // (unchanged from before this step) and now *also* loops over the whole
    // pool fixing that same value on every layer — see this function's own
    // doc comment. Bit numbers match `apply_node_post`'s own `EFFECTS_FLOATS`
    // order exactly (0..13), then 14 for the `leak_color` colour slot: two
    // places that must agree on what bit means what, so if
    // `apply_node_post`'s order ever changes, this has to change with it.
    if let Some(v) = body["shake"].as_f64() {
        let v = (v as f32).clamp(0.0, 1.0);
        state.post.shake = v;
        for layer in state.layers.iter_mut() {
            layer.effects.shake = v;
            layer.fijo_mask |= 1 << 0;
        }
    }
    if let Some(v) = body["light_leak"].as_f64() {
        let v = (v as f32).clamp(0.0, 1.0);
        state.post.light_leak = v;
        for layer in state.layers.iter_mut() {
            layer.effects.light_leak = v;
            layer.fijo_mask |= 1 << 1;
        }
    }
    if let Some(v) = body["ghost"].as_f64() {
        let v = (v as f32).clamp(0.0, 1.0);
        state.post.ghost = v;
        for layer in state.layers.iter_mut() {
            layer.effects.ghost = v;
            layer.fijo_mask |= 1 << 2;
        }
    }
    if let Some(v) = body["kaleido"].as_f64() {
        // 0 or 1 mirror is off; the shader treats anything under 2 the
        // same way, and clamping here keeps the page's slider honest
        // about where "off" ends.
        let v = (v as f32).clamp(0.0, 16.0);
        state.post.kaleido = v;
        for layer in state.layers.iter_mut() {
            layer.effects.kaleido = v;
            layer.fijo_mask |= 1 << 3;
        }
    }
    if let Some(v) = body["crt"].as_f64() {
        let v = (v as f32).clamp(0.0, 1.0);
        state.post.crt = v;
        for layer in state.layers.iter_mut() {
            layer.effects.crt = v;
            layer.fijo_mask |= 1 << 4;
        }
    }
    if let Some(v) = body["glitch"].as_f64() {
        let v = (v as f32).clamp(0.0, 1.0);
        state.post.glitch = v;
        for layer in state.layers.iter_mut() {
            layer.effects.glitch = v;
            layer.fijo_mask |= 1 << 5;
        }
    }
    if let Some(v) = body["posterize"].as_f64() {
        let v = (v as f32).clamp(0.0, 1.0);
        state.post.posterize = v;
        for layer in state.layers.iter_mut() {
            layer.effects.posterize = v;
            layer.fijo_mask |= 1 << 6;
        }
    }
    if let Some(v) = body["zoom"].as_f64() {
        let v = (v as f32).clamp(-1.0, 1.0);
        state.post.zoom = v;
        for layer in state.layers.iter_mut() {
            layer.effects.zoom = v;
            layer.fijo_mask |= 1 << 7;
        }
    }
    if let Some(v) = body["fire"].as_f64() {
        let v = (v as f32).clamp(0.0, 1.0);
        state.post.fire = v;
        for layer in state.layers.iter_mut() {
            layer.effects.fire = v;
            layer.fijo_mask |= 1 << 8;
        }
    }
    if let Some(v) = body["lorenz"].as_f64() {
        let v = (v as f32).clamp(0.0, 1.0);
        state.post.lorenz = v;
        for layer in state.layers.iter_mut() {
            layer.effects.lorenz = v;
            layer.fijo_mask |= 1 << 9;
        }
    }
    if let Some(v) = body["vignette"].as_f64() {
        let v = (v as f32).clamp(0.0, 1.0);
        state.post.vignette = v;
        for layer in state.layers.iter_mut() {
            layer.effects.vignette = v;
            layer.fijo_mask |= 1 << 10;
        }
    }
    if let Some(v) = body["blur"].as_f64() {
        let v = (v as f32).clamp(0.0, 1.0);
        state.post.blur = v;
        for layer in state.layers.iter_mut() {
            layer.effects.blur = v;
            layer.fijo_mask |= 1 << 11;
        }
    }
    if let Some(v) = body["sharpen"].as_f64() {
        let v = (v as f32).clamp(0.0, 1.0);
        state.post.sharpen = v;
        for layer in state.layers.iter_mut() {
            layer.effects.sharpen = v;
            layer.fijo_mask |= 1 << 12;
        }
    }
    if let Some(v) = body["grain"].as_f64() {
        let v = (v as f32).clamp(0.0, 1.0);
        state.post.grain = v;
        for layer in state.layers.iter_mut() {
            layer.effects.grain = v;
            layer.fijo_mask |= 1 << 13;
        }
    }
    // The colour slot — bit 14, the one `apply_node_post` treats as a
    // twelve-float block rather than a scalar. Never wired to this route
    // before (`leak_color` had no handler here at all, on any code path), so
    // this is a new capability rather than a migration of an old one; the
    // shape mirrors every field above it.
    if let Some(v) = body["leak_color"].as_array() {
        let parsed: Vec<f32> = v
            .iter()
            .filter_map(Value::as_f64)
            .map(|f| f as f32)
            .collect();
        if parsed.len() == state.post.leak_color.len() {
            state.post.leak_color = parsed.clone();
            for layer in state.layers.iter_mut() {
                layer.effects.leak_color = parsed.clone();
                layer.fijo_mask |= 1 << 14;
            }
        }
    }
}

/// Applies one `POST /layers` mutation to `state`.
///
/// Pulled out of the route handler so the mutation itself — in particular the
/// fan-out this whole change exists for — can be tested by calling it
/// directly, with no socket in sight. Returns the error message the route
/// used to write straight to the stream, unchanged, so this is a pure
/// extraction and not a change of contract.
///
/// **`device` is looked up once, up front, and not held onto** — the design notes PARTE IV
/// §17.1/§17.4 moved layers out of `Device` and into `state.layers`, a
/// sibling field, so an op that needs both (say, "add": check the limit
/// against the pool, then flip `stack.mode` off automix) cannot hold one
/// `&mut Device` borrow across a `state.layers` mutation — it has to ask for
/// the device again after it is done with the pool. Cheap (a `HashMap`
/// lookup or the `PREVIEW` special case) and exactly what the original
/// "mode" arm already did twice within itself, so this is not a new shape,
/// just a more consistent use of the one that was already here.
///
/// The `Ok` payload is the `Layer` the op minted, for the two ops that mint
/// one — "add" and "duplicate" (the design notes PARTE IV §17.4) — and `None` for
/// every other op, unchanged from before this existed. Added rather than
/// left `()`: the route handler hands it straight back to the page, which
/// otherwise has no way to learn the id `POST /layers` just assigned without
/// diffing `devices` before and after.
pub fn apply_layers_op(
    state: &mut State,
    models: usize,
    limits: Limits,
    body: &Value,
) -> Result<Option<Layer>, String> {
    let device = body["device"].as_str().unwrap_or(PREVIEW).to_string();
    let op = body["op"].as_str().unwrap_or_default().to_string();

    // Resolved before anything else, because applying a collection reads one
    // part of the state (`state.collections`) and writes another.
    let applying = if op == "apply" {
        match body["collection"]
            .as_str()
            .and_then(|c| state.collections.find(c))
        {
            Some(c) => Some((c.id.clone(), c.layers.clone(), c.composition)),
            None => return Err("no such collection".into()),
        }
    } else {
        None
    };

    if state.stack(&device).is_none() {
        return Err("no such device".into());
    }

    // Set by "add" and "duplicate" — the two ops that mint a new `Layer` —
    // and handed back to the caller so it need not look it up again. Every
    // other arm leaves this `None`.
    let mut created: Option<Layer> = None;

    match op.as_str() {
        "add" => {
            let Some(model) = body["model"].as_u64().map(|v| v as usize) else {
                return Err("a layer needs a model".into());
            };
            if model >= models {
                return Err("that animation is not built yet".into());
            }
            if state.routed_to(&device).len() as u32 >= limits.max_layers {
                return Err(format!(
                    "this device is at its limit of {} layers",
                    limits.max_layers
                ));
            }
            let z = state.next_z_on(&device);
            // See `unique_layer_id`'s own doc for why `z`/the pool's length
            // alone are not enough to make one.
            let id = unique_layer_id(&state.layers, z);
            let name = body["name"].as_str().unwrap_or("layer").to_string();
            let mut layer = Layer::new(&id, &name, model);
            layer.z = z;
            layer.outputs = vec![device.clone()];
            apply_fields(&mut layer, body, models);
            state.layers.push(layer.clone());
            created = Some(layer);
            // Saving a layer onto a device that is on automix has to take it
            // off automix, or the layer the operator just created is
            // invisible and saving reads as having done nothing. This is one
            // of two places the mode changes by itself — "route" below is
            // the other, for the same reason — and it is the same rule
            // `/select` follows.
            if let Some(stack) = state.stack_mut(&device) {
                if stack.mode == layers::DeviceMode::Automix {
                    stack.mode = layers::DeviceMode::Own;
                }
            }
        }
        "set" => {
            let Some(lid) = body["layer"].as_str().map(str::to_string) else {
                return Err("which layer".into());
            };
            let Some(layer) = state.find_layer_mut(&lid) else {
                return Err("no such layer".into());
            };
            apply_fields(layer, body, models);
        }
        "remove" => {
            // A full delete from the pool, wherever it is routed — "route"
            // with `on:false` below is the surgical op that only takes a
            // layer off one device without touching the others it may be
            // fanned out to. This is the layer's own delete.
            let lid = body["layer"].as_str().unwrap_or_default().to_string();
            state.layers.retain(|l| l.id != lid);
        }
        // Move one step toward the front or the back, by swapping z with the
        // neighbour rather than renumbering the stack — so a hand-set z
        // somewhere else in the list is not destroyed by a click on an
        // arrow. Neighbours are found among this device's own routed layers
        // (z ordering is a per-device idea), but the swap writes into the
        // pool by id, since that is where the layers actually live.
        "raise" | "lower" => {
            let lid = body["layer"].as_str().unwrap_or_default().to_string();
            let routed: Vec<(String, i32)> = state
                .routed_to(&device)
                .iter()
                .map(|l| (l.id.clone(), l.z))
                .collect();
            let Some(z) = routed.iter().find(|(id, _)| *id == lid).map(|(_, z)| *z) else {
                return Err("no such layer".into());
            };
            let up = op == "raise";
            let neighbour = routed
                .iter()
                .filter(|(id, zz)| *id != lid && if up { *zz >= z } else { *zz <= z })
                .min_by_key(|(_, zz)| if up { zz - z } else { z - zz })
                .cloned();
            match neighbour {
                Some((nid, other_z)) if other_z != z => {
                    if let Some(l) = state.find_layer_mut(&lid) {
                        l.z = other_z;
                    }
                    if let Some(l) = state.find_layer_mut(&nid) {
                        l.z = z;
                    }
                }
                // Nothing above (or below), or a tie: nudge past it.
                _ => {
                    if let Some(l) = state.find_layer_mut(&lid) {
                        l.z = if up { z + 1 } else { z - 1 };
                    }
                }
            }
        }
        "composition" => {
            let Some(c) = body["composition"].as_str().and_then(Composition::parse) else {
                return Err("overlap or sectors".into());
            };
            let Some(stack) = state.stack_mut(&device) else {
                return Err("no such device".into());
            };
            stack.composition = c;
        }
        // The device dropdown: automix, off, own, or a
        // collection. One route, because the ask says it is one menu.
        "mode" => {
            let kind = body["mode"].as_str().unwrap_or_default();
            let Some(m) = layers::DeviceMode::parse(kind, body["collection"].as_str()) else {
                return Err("automix, disabled, own or collection".into());
            };
            if let layers::DeviceMode::Collection(ref cid) = m {
                if state.collections.find(cid).is_none() {
                    return Err("no such collection".into());
                }
                // Nothing copied here any more — the design notes PARTE IV §17.5.
                // Selecting a collection from the dropdown is reading it,
                // not applying it: the layers and the composition are
                // resolved live, every frame, in
                // `compile_device_plan`/`render_device`. What stays
                // is this guard — reject a dropdown pointed at a collection
                // that does not exist right here, at selection time, rather
                // than rendering a black screen on the next tick with an
                // audience already watching.
            }
            let Some(stack) = state.stack_mut(&device) else {
                return Err("no such device".into());
            };
            // LAY.7b: the graph is the truth, so the dropdown writes it — a
            // `compose.*` node in the device's own canvas, converted in place
            // if the graph still carries the legacy `stack.mode` node. The
            // field is written too, and only as the pre-migration fallback
            // `graph_device_mode` reads when the graph says nothing: one
            // writer keeps the two in step, and the reader always prefers the
            // graph. A failure to write it is the operator's to see — a
            // dropdown that reports success while the next frame renders the
            // old mode is exactly the silent, in-range, wrong failure class
            // §3.3 names.
            if let Err(e) = write_device_mode_at(&output::data_root(), &device, &m) {
                return Err(format!(
                    "mode saved to the device but not to its graph: {e}"
                ));
            }
            stack.mode = m;
        }
        "apply" => {
            let Some((_cid, list, composition)) = applying else {
                return Err("no such collection".into());
            };
            state
                .layers
                .retain(|l| !l.outputs.iter().any(|o| o == &device));
            state.layers.extend(stamped(list, &device));
            let Some(stack) = state.stack_mut(&device) else {
                return Err("no such device".into());
            };
            stack.composition = composition;
            // `Own`, not `Collection(cid)` — the design notes PARTE IV §17.5. A device
            // in `Collection` mode now reads that collection live (see
            // `compile_device_plan`/`render_device`) instead of
            // whatever is sitting in its own pool entries, so leaving this
            // device pointed at `cid` would make the layers just copied
            // invisible on the very next frame: render would go back to
            // reading `cid`'s own saved stack, not this fork. "apply" is the
            // op that forks a collection into an editable stack; "mode:
            // collection" is the one that means "show this collection,
            // live" — two different intents behind what used to be one
            // line.
            stack.mode = layers::DeviceMode::Own;
        }
        // Empties this device. Like "remove", a full delete from the pool —
        // a layer fanned out to another device as well as this one goes with
        // it. "route" with `on:false` is the op that only detaches one
        // device without touching the layer's other destinations; "clear" is
        // the blunt one, matching what it did before fan-out could exist.
        "clear" => {
            state
                .layers
                .retain(|l| !l.outputs.iter().any(|o| o == &device));
        }
        // the design notes PARTE IV §17.1/§17.4: the enrouting op that makes fan-out
        // real. Everything above this line makes one destination possible
        // again after a device's Vec stopped owning layers directly; this is
        // the op that lets a layer be told to *also* show on a second
        // device, or to stop showing on one it already fed, without
        // touching the other. "duplicate" below is the other §17.4 op — the
        // "duplicate branch" editor gesture — and it reaches for this same
        // `outputs` field rather than reimplementing routing: a duplicate's
        // destination is just where its own `outputs` starts.
        "route" => {
            let Some(lid) = body["layer"].as_str().map(str::to_string) else {
                return Err("which layer".into());
            };
            let on = body["on"].as_bool().unwrap_or(true);
            let Some(layer) = state.find_layer_mut(&lid) else {
                return Err("no such layer".into());
            };
            if on {
                if !layer.outputs.iter().any(|o| o == &device) {
                    layer.outputs.push(device.clone());
                }
            } else {
                layer.outputs.retain(|o| o != &device);
            }
            // Same rule "add" follows, and for the same reason: a layer just
            // routed onto a device on automix would be invisible, because
            // automix ignores the pool and synthesizes its own view
            // (`layers::automix`) regardless of what is routed to it.
            if on {
                if let Some(stack) = state.stack_mut(&device) {
                    if stack.mode == layers::DeviceMode::Automix {
                        stack.mode = layers::DeviceMode::Own;
                    }
                }
            }
        }
        // the design notes PARTE IV §17.4's "duplicate branch": clone a layer's own
        // identity so the operator gets an independent copy to diverge
        // (`giro`, effects, its own `outputs`) from — but copy
        // `model`/`feed`/`picked`/`base`/`library` byte for byte, on
        // purpose. Those five fields are exactly `GenKey` (see its own doc
        // comment), so the copy lands on the very same `gen.*` node as the
        // original in `compile_device_plan` —
        // `duplicating_a_layer_shares_its_generator_with_the_original` below
        // is the proof. One GPU render still serves both, however many
        // branches accumulate; cloning the whole `Layer` (rather than
        // building a bare one) is what makes the *editing* half cheap too —
        // a duplicated bean starts as an exact twin, not a blank source the
        // operator re-builds by hand.
        "duplicate" => {
            let Some(lid) = body["layer"].as_str().map(str::to_string) else {
                return Err("which layer".into());
            };
            let Some(src) = state.layers.iter().find(|l| l.id == lid) else {
                return Err("no such layer".into());
            };
            let mut copy = src.clone();
            copy.name = format!("{} copia", src.name);
            // The new destination, if the caller named one — aiming the
            // duplicate straight at a second device, the §17.4 example's
            // "the same bean, a different rotation, on another screen".
            // Absent, the copy inherits the original's own destinations
            // unchanged: a branch that exists but has not been pointed
            // anywhere new yet, routable later with "route" like any other
            // layer.
            copy.outputs = match body["device"].as_str() {
                Some(d) => vec![d.to_string()],
                None => src.outputs.clone(),
            };
            // Same cap "add" enforces, checked against every destination the
            // copy actually lands on — duplicating must not be a back door
            // around the per-device layer limit "add" already guards.
            for d in &copy.outputs {
                if state.routed_to(d).len() as u32 >= limits.max_layers {
                    return Err(format!(
                        "this device is at its limit of {} layers",
                        limits.max_layers
                    ));
                }
            }
            // Its own z, not the original's, so the two do not collide in
            // draw order if they land on the same device — same "next free
            // z" rule "add" uses, against whichever device the copy
            // actually lands on first.
            let z = copy
                .outputs
                .first()
                .map(|d| state.next_z_on(d))
                .unwrap_or(0);
            copy.id = unique_layer_id(&state.layers, z);
            copy.z = z;
            // Same rule "add"/"route" follow, and only for a destination
            // named explicitly here — a device the copy merely inherited was
            // already showing (or, on automix, not showing) the original on
            // its own terms, and duplicating must not change that device's
            // mode as a side effect of inheriting its wiring.
            if let Some(d) = body["device"].as_str() {
                if let Some(stack) = state.stack_mut(d) {
                    if stack.mode == layers::DeviceMode::Automix {
                        stack.mode = layers::DeviceMode::Own;
                    }
                }
            }
            state.layers.push(copy.clone());
            created = Some(copy);
        }
        other => return Err(format!("no such op: {other}")),
    }

    Ok(created)
}

/// A globally-unique layer id built from its `z`, with a numeric tail
/// appended until it no longer collides with anything already in the pool.
///
/// `z` and the pool's length are both stable enough to look like an id and
/// neither is: raising a layer with nothing above it bumps its `z` by hand,
/// and removing from the middle moves the length backwards, so a sequence of
/// ordinary actions can land on a pair that has been used before. Two layers
/// with one id means every inline control on the second one edits the first,
/// silently.
///
/// The one id generator for a `Layer`, used by both `"add"` and
/// `"duplicate"` so there is exactly one place this is decided, not two that
/// could quietly drift apart — checked against `state.layers` as a whole,
/// not just one device's routed subset, since the design notes PARTE IV §17.1/§17.4:
/// ids are unique across the pool now, not per device.
pub fn unique_layer_id(layers: &[Layer], z: i32) -> String {
    let mut id = format!("l{z}-{}", layers.len());
    let mut n = 0;
    while layers.iter().any(|l| l.id == id) {
        n += 1;
        id = format!("l{z}-{}-{n}", layers.len());
    }
    id
}

/// A url-safe id from a human name, with an index to break ties.
pub fn slug(name: &str, n: usize) -> String {
    let base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let base = base.trim_matches('-').to_string();
    if base.is_empty() {
        format!("col-{n}")
    } else {
        format!("{base}-{n}")
    }
}

/// The assets list: every built model, then every planned one. Index into
/// this is what `/select` takes, and entries past `model_count` are the ones
/// it refuses — see the `UNBUILT` note.
pub fn asset_list(paint: &dyn Paint) -> Vec<Value> {
    let footage = paint.footage();
    // Which models the "backdrop as alpha" toggle does anything on —
    // the design notes PARTE IV §17.3. Same shape as `footage`, for the same reason:
    // the page needs a per-model answer and has no renderers of its own to
    // ask.
    let backdrop = paint.backdrop();
    let mut out: Vec<Value> = paint
        .ids()
        .iter()
        .zip(paint.names())
        .enumerate()
        // `footage` marks a transmission — a model that draws over source
        // material and has nothing to draw until a folder is chosen. The card
        // wears it, which is what turns "why is this one showing colour bars"
        // into a question with an answer on the same screen.
        .map(|(i, (id, name))| {
            json!({
                "id": id,
                "name": name,
                "built": true,
                "footage": footage.get(i).copied().unwrap_or(false),
                "backdrop": backdrop.get(i).copied().unwrap_or(false),
            })
        })
        .collect();
    for (id, name) in UNBUILT {
        out.push(
            json!({ "id": id, "name": name, "built": false, "footage": false, "backdrop": false }),
        );
    }
    out
}

/// A model's row, labelled by its schema, so the monitor reads "grosor 0.42"
/// and not "[14] 0.42". Colour slots and cameras are collapsed to one entry
/// each rather than twelve and seven unnamed numbers.
pub fn named_floats(schema: &metal_schema::Schema, row: &[f32]) -> Vec<Value> {
    use metal_schema::Block;
    let mut out = Vec::new();
    let mut at = 0usize;
    for block in schema.blocks {
        match block {
            Block::Floats(specs) => {
                for (k, spec) in specs.iter().enumerate() {
                    let v = row.get(at + k).copied().unwrap_or(0.0);
                    // Normalised too: the monitor draws a bar, and a bar needs
                    // to know where in the float's own range this sits. A
                    // float at 0.9 of 0..1 and one at 0.9 of 0..40 are not the
                    // same picture.
                    let span = (spec.max - spec.min).abs().max(1e-6);
                    out.push(json!({
                        "name": spec.name,
                        "v": v,
                        "unit": ((v - spec.min) / span).clamp(0.0, 1.0),
                    }));
                }
            }
            Block::ColorSlot(name, _) => {
                let hex: Vec<String> = row[at..at + 12]
                    .chunks(3)
                    .map(|c| {
                        let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
                        format!("#{:02x}{:02x}{:02x}", byte(c[0]), byte(c[1]), byte(c[2]))
                    })
                    .collect();
                out.push(json!({ "name": *name, "colours": hex }));
            }
            Block::Camera(_) => {
                out.push(json!({ "name": "camera", "fixed": true }));
            }
        }
        at += block.width();
    }
    out
}

/// The surface a venue screen actually shows: one picture, black around it,
/// nothing else. Served at `/out/<device>` so it can be dragged onto the
/// second screen and put fullscreen.
pub const OUT_PAGE: &str = include_str!("../out.html");

/// The one planned model. Every other entry in the assets list comes from the
/// renderer; this one is written down because it is the one the suite is
/// planned around and does not exist yet — `metal-render::registry` says so in
/// as many words. Listing it greyed is the honest state; omitting it would
/// quietly redefine the suite as one model smaller than it is.
///
/// **The one rig is not here either.** Don Joe has no renderer — he is the
/// one rig you author by grabbing a joint — so he is listed greyed like any
/// other planned entry.
pub const UNBUILT: &[(&str, &str)] = &[("donjoe", "Don Joe")];

#[cfg(test)]
mod node_manifest_memo_tests {
    /// `/catalog` parses `schema.json` through [`super::node_manifest`]: the
    /// parse is kept while the file keeps its stamp, so a second call hands
    /// back the same `Arc` instead of parsing the megabyte again.
    #[test]
    fn the_node_manifest_is_parsed_once_while_the_file_keeps_its_stamp() {
        let a = super::node_manifest().expect("data/schema.json parses");
        let b = super::node_manifest().expect("data/schema.json parses");
        assert!(
            std::sync::Arc::ptr_eq(&a, &b),
            "the second call is the memo's"
        );
        assert!(!a.models.is_empty());
    }
}
