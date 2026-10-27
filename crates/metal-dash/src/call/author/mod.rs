//! The call layer for `/author/*` (`O.12.b′`, lote `the cleanup`).
//!
//! Continues `S.U4a`: the seven `author::{mod,audio,curator,rig,router,palette,
//! render}::serve` dispatchers (plus `author::models::serve`, which `mod`'s own
//! dispatcher walked into) used to write straight to the socket handed to
//! `author::serve`. Their match arms move here as typed functions — `&mut
//! Ctx` (this module's own, distinct from `crate::call::Ctx`: the absorbed
//! studio has its own `State`, gated on `gpu`, and forcing it through the
//! top-level `Ctx` would mean the top-level app depending on `author` even on
//! a build that never compiles it in) plus [`Params`] in, `Result<Value,
//! CallError>` / `Result<Bytes, CallError>` out.
//!
//! `main.rs`'s `Author::dispatch` is the one caller: it locks
//! `author::State` (unchanged — still a `Mutex`, still `#[cfg(feature =
//! "gpu")]`, still `Author`'s own idiom), builds a [`Ctx`], and walks
//! [`AUTHOR_ROUTES`]. A second table rather than folding these rows into
//! `crate::call::ROUTES`: the two dispatchers close over two different state
//! types, and `crate::call::ROUTES`'s own rows would otherwise need an
//! `Author`-shaped `Ctx` variant just for these ~fifty.

use serde_json::Value;

use crate::call::{Bytes, CallError, Params, Reply};

pub mod audio;
#[cfg(any(target_os = "android", feature = "native-audio"))]
pub mod audio_out;
pub mod curator;
pub mod dataset;
pub mod grade;
pub mod image;
pub mod ingest;
pub mod models;
pub mod palette;
pub mod playout;
pub mod render;
pub mod rig;
pub mod router;
pub mod scenes;

/// What every `/author/*` handler is handed. A handler reads only the
/// `author::State` fields its own body needs.
pub struct Ctx<'a> {
    pub state: &'a mut crate::author::State,
}

impl<'a> Ctx<'a> {
    /// The little context `author::rig`'s and `author::models`'s own
    /// functions were already written against, built the same way
    /// `author::mod::serve` built it inline on every request.
    pub fn actx(&self) -> crate::author::Ctx<'_> {
        crate::author::Ctx {
            skeleton: &self.state.skeleton,
            nodes: &self.state.nodes,
            open: &self.state.open,
            ingest: &self.state.ingest,
            desk: &self.state.desk,
        }
    }
}

/// `/pagedata` is the one route `author::mod::serve` answered itself rather
/// than handing to a submodule — see that module's own note on why (it is
/// the *assembly* of every mode's payload, which only the whole `State` has
/// the pieces of).
pub fn pagedata(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(ctx.state.page_data())
}

#[derive(Clone, Copy)]
pub enum Handler {
    Json(fn(&mut Ctx, &Params) -> Result<Value, CallError>),
    Bytes(fn(&mut Ctx, &Params) -> Result<Bytes, CallError>),
    /// A JSON read that never looks at the author state (it reads the disk
    /// only). [`dispatch_free`] runs it without the state lock, so a slow disk
    /// read cannot hold up the UI thread's own author reads.
    Free(fn(&Params) -> Result<Value, CallError>),
}

/// A route's answer before it is turned into bytes. Building the reply (the
/// JSON serialisation, the largest part of a big read) happens in
/// [`Built::into_reply`], which the caller runs after releasing the state lock.
pub enum Built {
    Json(Result<Value, CallError>),
    Bytes(Result<Bytes, CallError>),
}

impl Built {
    pub fn into_reply(self) -> Reply {
        match self {
            Built::Json(Ok(v)) => Reply::json_ok(v),
            Built::Json(Err(e)) => Reply::error(e),
            Built::Bytes(Ok(b)) => Reply::bytes_ok(b),
            Built::Bytes(Err(e)) => Reply::error(e),
        }
    }
}

pub struct Route {
    pub method: &'static str,
    pub path: &'static str,
    /// `true` when `path` is a prefix rather than the whole path — none of
    /// today's `/author/*` rows need one, but the shape matches
    /// `crate::call::Route` so the two dispatchers read the same way.
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

/// The [`Free`](Handler::Free) route `path` names, if the first route that
/// matches it is one. `None` for every other path, which the caller then
/// serves through [`build`] under the state lock.
pub fn dispatch_free(method: &str, path: &str, params: &Params) -> Option<Reply> {
    let route = AUTHOR_ROUTES.iter().find(|r| r.matches(method, path))?;
    let Handler::Free(f) = route.handler else {
        return None;
    };
    Some(Built::Json(f(params)).into_reply())
}

/// Runs `path` (already stripped of the outer `/author` prefix, exactly as
/// `author::mod::serve` always received it) against the state, and returns
/// the answer before it is serialised. The caller holds the state lock for
/// this call only. `None` when nothing matches.
pub fn build(method: &str, path: &str, params: &Params, ctx: &mut Ctx) -> Option<Built> {
    // An external push, if anyone is making them, is adopted before the
    // request is served — same ordering `author::mod::serve` always had.
    ctx.state
        .preview
        .adopt_live(&ctx.state.live, &ctx.state.catalog);

    let route = AUTHOR_ROUTES.iter().find(|r| r.matches(method, path))?;
    let params = Params {
        tail: route.prefix.then(|| path[route.path.len()..].to_string()),
        ..params.clone()
    };
    let params = &params;
    Some(match route.handler {
        Handler::Json(f) => Built::Json(f(ctx, params)),
        Handler::Bytes(f) => Built::Bytes(f(ctx, params)),
        Handler::Free(f) => Built::Json(f(params)),
    })
}


/// Every `/author/*` route, on the path with the outer prefix already
/// stripped. The dispatch order the old per-module `serve` chain had
/// (audio → palette → rig → router → curator → models → render) does not
/// matter here — a table has no order dependency because no two rows answer
/// the same `(method, path)` (`no_duplicate_method_path_rows`, below).
pub static AUTHOR_ROUTES: &[Route] = &[
    Route {
        method: "GET",
        path: "/pagedata",
        prefix: false,
        handler: Handler::Json(pagedata),
    },
    // audio.rs — shared between the rig and the models mode.
    Route {
        method: "GET",
        path: "/audio",
        prefix: false,
        handler: Handler::Bytes(audio::audio),
    },
    Route {
        method: "GET",
        path: "/songs",
        prefix: false,
        handler: Handler::Json(audio::songs),
    },
    Route {
        method: "POST",
        path: "/add",
        prefix: false,
        handler: Handler::Json(audio::add),
    },
    Route {
        method: "POST",
        path: "/digest",
        prefix: false,
        handler: Handler::Json(audio::digest),
    },
    Route {
        method: "POST",
        path: "/open",
        prefix: false,
        handler: Handler::Json(audio::open),
    },
    Route {
        method: "GET",
        path: "/trim",
        prefix: false,
        handler: Handler::Json(audio::trim_get),
    },
    Route {
        method: "POST",
        path: "/trim",
        prefix: false,
        handler: Handler::Json(audio::trim_post),
    },
    Route {
        method: "POST",
        path: "/reveal",
        prefix: false,
        handler: Handler::Json(audio::reveal),
    },
    // palette.rs
    Route {
        method: "GET",
        path: "/palettes",
        prefix: false,
        handler: Handler::Free(palette::list),
    },
    Route {
        method: "POST",
        path: "/palettes",
        prefix: false,
        handler: Handler::Json(palette::save),
    },
    Route {
        method: "POST",
        path: "/palettes/delete",
        prefix: false,
        handler: Handler::Json(palette::delete),
    },
    // rig.rs — the skeleton mode.
    Route {
        method: "POST",
        path: "/rig/track",
        prefix: false,
        handler: Handler::Json(rig::track),
    },
    // The whole open track (frames included) — the shell's rig lane reads
    // it (`S.U9c`'s authoring half). Distinct from `/rig/tracks` below,
    // which is `examples`' own alias.
    Route {
        method: "GET",
        path: "/rig/track",
        prefix: false,
        handler: Handler::Json(rig::track_get),
    },
    Route {
        method: "GET",
        path: "/rig/examples",
        prefix: false,
        handler: Handler::Json(rig::examples),
    },
    Route {
        method: "GET",
        path: "/rig/tracks",
        prefix: false,
        handler: Handler::Json(rig::examples),
    },
    Route {
        method: "GET",
        path: "/rig/library",
        prefix: false,
        handler: Handler::Free(rig::library_get),
    },
    Route {
        method: "POST",
        path: "/rig/library",
        prefix: false,
        handler: Handler::Json(rig::library_post),
    },
    Route {
        method: "GET",
        path: "/rig/state",
        prefix: false,
        handler: Handler::Json(rig::state),
    },
    Route {
        method: "POST",
        path: "/rig/proposal",
        prefix: false,
        handler: Handler::Json(rig::proposal),
    },
    Route {
        method: "POST",
        path: "/rig/undo",
        prefix: false,
        handler: Handler::Json(rig::undo),
    },
    // router.rs — model 0.
    Route {
        method: "GET",
        path: "/router/state",
        prefix: false,
        handler: Handler::Json(router::state),
    },
    Route {
        method: "GET",
        path: "/router/take",
        prefix: false,
        handler: Handler::Json(router::take_get),
    },
    Route {
        method: "POST",
        path: "/router/take",
        prefix: false,
        handler: Handler::Bytes(router::take_post),
    },
    Route {
        method: "GET",
        path: "/router/read",
        prefix: false,
        handler: Handler::Json(router::read),
    },
    Route {
        method: "POST",
        path: "/router/train",
        prefix: false,
        handler: Handler::Json(router::train),
    },
    // grade.rs — the Colourist, `S.AP` point 2.
    Route {
        method: "GET",
        path: "/grade/state",
        prefix: false,
        handler: Handler::Json(grade::state),
    },
    Route {
        method: "POST",
        path: "/grade/train",
        prefix: false,
        handler: Handler::Json(grade::train),
    },
    Route {
        method: "GET",
        path: "/grade/matrix",
        prefix: false,
        handler: Handler::Json(grade::matrix),
    },
    // dataset.rs — the Colourist dataset's authored parts.
    Route {
        method: "GET",
        path: "/dataset",
        prefix: false,
        handler: Handler::Free(dataset::list),
    },
    Route {
        method: "GET",
        path: "/dataset/example",
        prefix: false,
        handler: Handler::Free(dataset::example),
    },
    Route {
        method: "POST",
        path: "/dataset/status",
        prefix: false,
        handler: Handler::Json(dataset::status),
    },
    // curator.rs
    Route {
        method: "GET",
        path: "/curator/state",
        prefix: false,
        handler: Handler::Json(curator::state),
    },
    Route {
        method: "GET",
        path: "/curator/scenes",
        prefix: false,
        handler: Handler::Json(curator::scenes),
    },
    Route {
        method: "GET",
        path: "/curator/picks",
        prefix: false,
        handler: Handler::Json(curator::picks_get),
    },
    Route {
        method: "POST",
        path: "/curator/picks",
        prefix: false,
        handler: Handler::Bytes(curator::picks_post),
    },
    Route {
        method: "GET",
        path: "/curator/read",
        prefix: false,
        handler: Handler::Json(curator::read),
    },
    Route {
        method: "POST",
        path: "/curator/train",
        prefix: false,
        handler: Handler::Json(curator::train),
    },
    // models/mod.rs — the float suite and modo edit.
    Route {
        method: "GET",
        path: "/models/state",
        prefix: false,
        handler: Handler::Json(models::state),
    },
    Route {
        method: "GET",
        path: "/models/example",
        prefix: false,
        handler: Handler::Json(models::example_get),
    },
    Route {
        method: "POST",
        path: "/models/example",
        prefix: false,
        handler: Handler::Json(models::example_post),
    },
    Route {
        method: "GET",
        path: "/models/song-hits",
        prefix: false,
        handler: Handler::Json(models::song_hits_get),
    },
    Route {
        method: "POST",
        path: "/models/song-hits",
        prefix: false,
        handler: Handler::Json(models::song_hits_post),
    },
    Route {
        method: "POST",
        path: "/models/export",
        prefix: false,
        handler: Handler::Json(models::export),
    },
    Route {
        method: "POST",
        path: "/models/showing",
        prefix: false,
        handler: Handler::Json(models::showing),
    },
    Route {
        method: "GET",
        path: "/models/library",
        prefix: false,
        handler: Handler::Free(models::library_get),
    },
    Route {
        method: "POST",
        path: "/models/library",
        prefix: false,
        handler: Handler::Json(models::library_post),
    },
    Route {
        method: "GET",
        path: "/models/movement",
        prefix: false,
        handler: Handler::Json(models::movement),
    },
    Route {
        method: "POST",
        path: "/models/lift",
        prefix: false,
        handler: Handler::Json(models::lift),
    },
    Route {
        method: "GET",
        path: "/models/timelines",
        prefix: false,
        handler: Handler::Json(models::timelines),
    },
    Route {
        method: "GET",
        path: "/models/timeline",
        prefix: false,
        handler: Handler::Json(models::timeline_get),
    },
    Route {
        method: "GET",
        path: "/models/timeline/resolve",
        prefix: false,
        handler: Handler::Json(models::timeline_resolve),
    },
    Route {
        method: "POST",
        path: "/models/timeline",
        prefix: false,
        handler: Handler::Json(models::timeline_post),
    },
    Route {
        method: "POST",
        path: "/models/timeline/new",
        prefix: false,
        handler: Handler::Json(models::timeline_new),
    },
    Route {
        method: "POST",
        path: "/models/timeline/delete",
        prefix: false,
        handler: Handler::Json(models::timeline_delete),
    },
    Route {
        method: "POST",
        path: "/models/timeline/segment",
        prefix: false,
        handler: Handler::Json(models::timeline_segment),
    },
    Route {
        method: "POST",
        path: "/models/timeline/export",
        prefix: false,
        handler: Handler::Json(models::timeline_export),
    },
    // render.rs — the live preview.
    Route {
        method: "GET",
        path: "/render/state",
        prefix: false,
        handler: Handler::Json(render::state),
    },
    Route {
        method: "GET",
        path: "/render/grid.jpg",
        prefix: false,
        handler: Handler::Bytes(render::grid_jpg),
    },
    Route {
        method: "GET",
        path: "/render/solo.jpg",
        prefix: false,
        handler: Handler::Bytes(render::solo_jpg),
    },
    Route {
        method: "POST",
        path: "/render/floats",
        prefix: false,
        handler: Handler::Json(render::floats),
    },
    Route {
        method: "POST",
        path: "/render/effects",
        prefix: false,
        handler: Handler::Json(render::effects_retired),
    },
    Route {
        method: "POST",
        path: "/render/select",
        prefix: false,
        handler: Handler::Json(render::select),
    },
    Route {
        method: "POST",
        path: "/render/reset",
        prefix: false,
        handler: Handler::Json(render::reset),
    },
    Route {
        method: "POST",
        path: "/render/pause",
        prefix: false,
        handler: Handler::Json(render::pause),
    },
    Route {
        method: "POST",
        path: "/render/preset",
        prefix: false,
        handler: Handler::Json(render::preset),
    },
    // playout.rs — the sequence editor's sound, video frames and project file.
    Route {
        method: "POST",
        path: "/play",
        prefix: false,
        handler: Handler::Json(playout::play),
    },
    Route {
        method: "GET",
        path: "/play",
        prefix: false,
        handler: Handler::Json(playout::play_clock),
    },
    Route {
        method: "GET",
        path: "/song/pull",
        prefix: false,
        handler: Handler::Json(playout::song_pull_get),
    },
    Route {
        method: "POST",
        path: "/song/pull",
        prefix: false,
        handler: Handler::Json(playout::song_pull_post),
    },
    Route {
        method: "POST",
        path: "/video/seek",
        prefix: false,
        handler: Handler::Json(playout::video_seek),
    },
    Route {
        method: "GET",
        path: "/video/frame.jpg",
        prefix: false,
        handler: Handler::Bytes(playout::video_frame),
    },
    // image.rs — a still file served as JPEG for the preview and the gallery.
    Route {
        method: "GET",
        path: "/image.jpg",
        prefix: false,
        handler: Handler::Bytes(image::image),
    },
    // ingest.rs — queue a clip's proxy build (the node route and the shell).
    Route {
        method: "POST",
        path: "/ingest",
        prefix: false,
        handler: Handler::Json(ingest::ingest_post),
    },
    Route {
        method: "GET",
        path: "/project",
        prefix: false,
        handler: Handler::Json(playout::project_get),
    },
    Route {
        method: "POST",
        path: "/project",
        prefix: false,
        handler: Handler::Json(playout::project_post),
    },
    // scenes.rs — a video's scenes and the book of kept ones.
    Route {
        method: "GET",
        path: "/scenes",
        prefix: false,
        handler: Handler::Json(scenes::get),
    },
    Route {
        method: "POST",
        path: "/scenes",
        prefix: false,
        handler: Handler::Json(scenes::post),
    },
    Route {
        method: "GET",
        path: "/scenes/poster.jpg",
        prefix: false,
        handler: Handler::Bytes(scenes::poster),
    },
    Route {
        method: "POST",
        path: "/scenes/export",
        prefix: false,
        handler: Handler::Json(scenes::export),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_duplicate_method_path_rows() {
        let mut seen = std::collections::HashSet::new();
        for r in AUTHOR_ROUTES {
            assert!(
                seen.insert((r.method, r.path, r.prefix)),
                "duplicate row: {} {} (prefix={})",
                r.method,
                r.path,
                r.prefix
            );
        }
    }

    /// The `/author/*` twin of `crate::call::tests::every_route_dispatches`.
    /// Same reasoning: walk the table, build a synthetic request per row,
    /// assert none 404s. See that test for why a 404 specifically is the one
    /// status that means "this row did not actually dispatch", and why the
    /// cwd is set to the repo root before `author::State::new()` — it reads
    /// `data/schema.json` off a bare relative path.
    #[test]
    fn every_author_route_dispatches() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for author::State::new()");
        // `GET /audio` (unlike a genuinely-unrouted path) 404s honestly when
        // no song is open — indistinguishable from "did not dispatch" by
        // status alone. A scratch file, opened the same way `POST /open`
        // would, gives it something real to read.
        let scratch_dir =
            std::env::temp_dir().join(format!("metal-dash-author-routes-test-{}", std::process::id()));
        metal_vfs::create_dir_all(&scratch_dir).expect("scratch dir");
        let scratch_audio = scratch_dir.join("probe.wav");
        metal_vfs::write(&scratch_audio, b"not really a wav, just needs to exist")
            .expect("scratch audio");

        let mut state = crate::author::State::new(
            Some(scratch_audio),
            std::path::PathBuf::from("data/tracks/untitled.track.json"),
        );
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        for route in AUTHOR_ROUTES {
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
                query: Default::default(),
                body: Vec::new(),
                tail: None,
            };
            let mut ctx = Ctx { state: &mut state };
            let reply = build(method, &path, &params, &mut ctx).map(Built::into_reply);
            let status = reply.map(|r| r.status);
            assert_ne!(status, Some(404), "{method} {path} 404d — did not dispatch");
            assert!(
                status.is_some(),
                "{method} {path} matched no row in AUTHOR_ROUTES"
            );
        }
    }

    /// 2026-10-06: the sequence editor's preview addresses a model by id.
    /// Its list (`GET /author/models/state`) skips the registry's retired
    /// tombstones, so a position in it is not a renderer index: `rings` sat
    /// at a position that drew Puzzle. `model=<id>` resolves the real row.
    #[test]
    fn the_preview_routes_resolve_a_model_by_id_past_the_tombstones() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for author::State::new()");
        let mut state = crate::author::State::new(None, std::path::PathBuf::from("data/tracks/untitled.track.json"));
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let ids = state.catalog.ids();
        let real = ids.iter().position(|m| *m == "rings").expect("rings is registered");
        let manifest = state.models.manifest.models.iter().position(|m| m.id == "rings").expect("rings in the manifest");
        assert_ne!(real, manifest, "the two lists disagree on rings' position (the bug this pins)");

        let width = state.preview.rows[real].len();
        let row = vec![0.0f32; width];
        let mut params = Params {
            query: [("model".to_string(), "rings".to_string())].into_iter().collect(),
            body: serde_json::to_vec(&row).unwrap(),
            tail: None,
        };
        let mut ctx = Ctx { state: &mut state };
        render::floats(&mut ctx, &params).expect("rings' own width is accepted by id");
        assert_eq!(ctx.state.preview.rows[real], row, "the row landed on rings, not on another model");

        params.query = [("model".to_string(), "nope".to_string())].into_iter().collect();
        assert!(render::floats(&mut ctx, &params).is_err(), "an unknown id is refused, never guessed");
    }

    /// The models listing is built once and then answered from memory: the
    /// second reply is the first, and both have one entry per manifest model.
    #[test]
    fn the_models_listing_is_built_once() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for author::State::new()");
        let mut state = crate::author::State::new(None, std::path::PathBuf::from("data/tracks/untitled.track.json"));
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let want = state.models.manifest.models.len();
        let mut ctx = Ctx { state: &mut state };
        assert!(ctx.state.models.listing.get().is_none(), "nothing is built before the first read");
        let first = models::state(&mut ctx, &Params::default()).expect("listing");
        assert!(ctx.state.models.listing.get().is_some(), "the first read fills the memo");
        let second = models::state(&mut ctx, &Params::default()).expect("listing");
        assert_eq!(first, second);
        assert_eq!(first["models"].as_array().map(Vec::len), Some(want));
    }
}
