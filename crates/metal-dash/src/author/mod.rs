//! Authoring — `fw-studio`, absorbed.
//!
//! Conjunto C (`the design notes` §10): the studio was a second binary
//! (`fw-studio --port 7979`) with the parameter panel, the lanes, the song
//! library, the rig, the Curator and the Router's training UI. `fw-app` is
//! `metal-dash` absorbing it, not the other way round, because the dash owns the
//! compositor, the devices, the live router and the no-GPU build — the things
//! the product actually needs at a venue. The studio brought authoring, which
//! is a panel.
//!
//! **This module is the studio's own dispatcher, moved whole.** Every route it
//! answers is served under `/author/*` by the outer `serve()` in
//! `crate::main` — see the match arm there that strips the prefix and calls
//! [`serve`] below. Nothing about how these routes work changed; only where
//! they live and what wraps them. The two servers both had `/state`,
//! `/select`, `/preset` — the prefix is the only cheap way past that, per
//! §10 Conjunto C1 (renaming would have broken both pages and both
//! pre-existing harnesses; by 15-sept-2026 only `/out.html` remains).
//!
//! **Gated on `gpu`.** The models mode inside here previews with the real
//! wgpu renderers (`metal_render::headless::Renderer`, unconditional in the old
//! `fw-studio` binary because that binary never had a no-GPU build). Rather
//! than teach this subsystem to run on `stand::Stand` too — a rewrite, not a
//! move, and out of scope for Conjunto C — `/author/*` is a 404 on a build
//! without `gpu`. The rig, the Curator and the Router's authoring UI *could*
//! run without a GPU (`metal-rig`, `metal-router`, `metal-reel` all do), but they are
//! dispatched from the one `Ctx`/`Renderer` pair the old binary built at
//! startup, so splitting them out is future work, not this pass.
//!
//! ⚠ `agent.rs` here is the *rig-authoring* agent (Don Joe's skeleton,
//! `turn`/`silent` protocol over a `Track`) — unrelated to any future
//! AI-harness-for-graph-editing. Moved as-is; see its own doc comment.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use metal_rig::node::Nodes;

pub mod agent;
pub mod audio;
pub mod curator;
pub mod dataset;
pub mod grade;
pub mod library;
pub mod live;
pub mod models;
pub mod palette;
pub mod render;
pub mod rig;
pub mod router;

use agent::Desk;
use audio::Ingest;

/// The one audio library. Both the author routes and `metal-dash` proper read it
/// — `metal-dash` when it is given a file to play from an `Offline` graph.
pub const SONGS: &str = "data/audio";
/// Skeleton tracks — rotations per node, on the 8 Hz grid.
pub const TRACKS: &str = "data/tracks";
/// Suite animations — a float row per tick, per model per song.
pub const ANIMATIONS: &str = "data/animations";
/// Modo edit's shows: which generator and which effects are alive over which
/// span of ticks. Deliberately a sibling of [`ANIMATIONS`] rather than a
/// subdirectory of it — a timeline *cites* examples by `(model, song)` and
/// owns none of them, and nesting it under the corpus would suggest a
/// containment that is not there. See [`models::timeline`].
pub const TIMELINES: &str = "data/timelines";
/// The reuse library. One file, both modes — see [`library`].
pub const LIBRARY: &str = "data/library/movements.json";
/// Saved colour slots. Beside the movements, deliberately not inside them —
/// [`palette`]'s module note has the argument, which is about `library::check`
/// being entirely about ticks and a palette having none.
pub const PALETTES: &str = "data/library/palettes.json";
/// The float manifest the models mode authors against, generated from the
/// renderers by `cargo run -p metal-render --bin emit-schema`.
pub const SCHEMA: &str = "data/schema.json";

/// What song and track are open. Behind a lock because the page can change it.
pub struct Open {
    pub audio: Option<PathBuf>,
    pub track: PathBuf,
}

/// Everything the shared and skeleton routes need, gathered so the dispatcher
/// does not take nine arguments. Unchanged from `fw-studio::Ctx`.
pub struct Ctx<'a> {
    pub skeleton: &'a metal_rig::skeleton::Skeleton,
    pub nodes: &'a Nodes,
    pub open: &'a Mutex<Open>,
    pub ingest: &'a Ingest,
    pub desk: &'a Desk,
}

/// Everything `main` builds once at startup and hands to [`serve`] on every
/// request — the old `fw-studio::main`'s locals, gathered so the outer dash
/// dispatcher can own them without exposing their fields.
pub struct State {
    pub(crate) skeleton: metal_rig::skeleton::Skeleton,
    pub(crate) nodes: Nodes,
    pub(crate) open: Mutex<Open>,
    pub(crate) ingest: Ingest,
    pub(crate) desk: Desk,
    /// Behind its own lock, so a grid or solo draw does not hold the state
    /// lock (the locking rule is on `app::Author`). Lock order: `state`, then
    /// `renderer`.
    pub(crate) renderer: std::sync::Arc<Mutex<render::Renderer>>,
    /// The renderer's model list, copied at construction. Routes that only
    /// need names, ids or schemas read this and never touch `renderer`.
    pub(crate) catalog: render::Catalog,
    /// G5: the renderer draws on the painter's device, so its solo pictures
    /// can be handed to the window as textures.
    pub(crate) on_painter: bool,
    pub(crate) models: models::State,
    pub(crate) preview: render::State,
    pub(crate) live: std::sync::Arc<live::Live>,
}

impl State {
    /// Builds the renderers. Called once, before the app binds its listener —
    /// same reasoning `fw-studio::main` had: a host with no usable GPU
    /// adapter must fail here, loudly, rather than serve a page of broken
    /// images. (On a `--no-default-features` build this whole module does
    /// not compile in, so this only ever runs when `gpu` is on.)
    pub fn new(audio: Option<PathBuf>, track: PathBuf) -> Self {
        Self::new_on(audio, track, None)
    }

    /// [`State::new`] with the renderers on the painter's own device when it
    /// has one (`gpu`, G5): a solo picture is then a texture the window can
    /// present. `None` keeps the renderers on a device of their own (pixels).
    pub fn new_on(audio: Option<PathBuf>, track: PathBuf, gpu: Option<metal_render::gpu::Gpu>) -> Self {
        let skeleton = metal_rig::rig::canonical();
        let nodes = Nodes::of(&skeleton);
        let open = Mutex::new(Open { audio, track });
        let ingest = Ingest::start(Path::new(SONGS));
        let desk = Desk::default();
        eprintln!("  author: building the renderers…");
        let on_painter = gpu.is_some();
        let renderer = match gpu {
            Some(gpu) => render::Renderer::with_gpu(gpu),
            None => render::Renderer::new(),
        };
        let models = models::State::new();
        let preview = render::State::new(&renderer);
        let catalog = render::Catalog::of(&renderer);
        let renderer = std::sync::Arc::new(Mutex::new(renderer));
        let live = std::sync::Arc::new(live::Live::default());
        State {
            skeleton,
            nodes,
            open,
            ingest,
            desk,
            renderer,
            catalog,
            on_painter,
            models,
            preview,
            live,
        }
    }

    /// Everything the author state holds on first paint — what the old
    /// merged HTML page (`dash.html`, deleted 15-sept-2026, S.U18) used to
    /// receive embedded as `FW_AUTHOR` on load. The native shell gets the
    /// same facts over `GET /author/pagedata`, which the rig stage in
    /// `panels/nodes.rs` pumps; this module answers routes, not HTML.
    pub fn page_data(&self) -> serde_json::Value {
        let song = self.open.lock().expect("open lock").audio.clone();
        let stem = song
            .as_deref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str());
        serde_json::json!({
            "rate": audio::RATE,
            "hasSong": song.is_some(),
            "songName": stem,
            "rigName": "Don Joe",
            "render": render::page_data(&self.catalog),
            "router": router::page_data(&self.catalog, stem),
            "curator": curator::page_data(stem),
            "models": models::page_data(&self.models, &self.catalog, song.as_deref()),
            "rig": rig::page_data(&Ctx {
                skeleton: &self.skeleton,
                nodes: &self.nodes,
                open: &self.open,
                ingest: &self.ingest,
                desk: &self.desk,
            }),
        })
    }
}

// the cleanup: the raw-socket `serve` dispatcher that used to live here (walking
// audio → palette → rig → router → curator → models → render in order) moved
// to `crate::call::author` as a typed route table (`AUTHOR_ROUTES`) — see
// that module's doc comment. `Ctx` and `State` above are unchanged; they are
// exactly what `call::author::Ctx::actx()` builds on every request now.

#[cfg(test)]
mod tests {
    /// `/pagedata` is the dispatcher's own, and no submodule may claim it.
    ///
    /// Every mode below owns either a prefix (`rig`, `models`, `render`,
    /// `router`, `curator`) or a short list of exact paths (`audio`), and the
    /// dispatcher walks them in order. If one of them ever grew a prefix this
    /// path fell under, the page would re-adopt that mode's payload instead of
    /// the whole one — a song change that half-lands is worse than the reload
    /// this route replaced, because nothing would say so.
    ///
    /// Written as the string property rather than by calling [`serve`] for the
    /// same reason `router.rs`'s own claiming test is: reaching `serve` means
    /// building a [`State`], which means building a GPU device and every
    /// model's pipelines, and a machine with no adapter would fail this for a
    /// reason that has nothing to do with routing.
    #[test]
    fn pagedata_is_the_dispatchers_own_and_no_mode_claims_it() {
        let path = "/pagedata";
        for prefix in ["/rig", "/models", "/render", "/router/", "/curator/"] {
            assert!(
                !path.starts_with(prefix),
                "{prefix} would have claimed {path}"
            );
        }
        // `audio.rs` matches whole paths, so the check against it is equality.
        for whole in [
            "/audio", "/songs", "/add", "/digest", "/open", "/trim", "/reveal",
        ] {
            assert_ne!(path, whole, "audio.rs would have claimed {path}");
        }
    }

    /// The same property for the palettes, which are dispatched second and so
    /// get first refusal on everything the modes below them own.
    ///
    /// It is the mirror of the test above and it matters in both directions:
    /// nothing else may claim `/palettes` (the palettes would answer nothing),
    /// and `/palettes` may not swallow anything else (whichever mode owns it
    /// would go silent, and a route that 404s from the wrong module reads
    /// exactly like a route that was never written). Written as the string
    /// property for the same reason — reaching `serve` means building every
    /// model's GPU pipelines.
    #[test]
    fn the_palettes_claim_their_own_prefix_and_nothing_claims_it_back() {
        let path = "/palettes";
        for prefix in ["/rig", "/models", "/render", "/router/", "/curator/"] {
            assert!(
                !path.starts_with(prefix),
                "{prefix} would have claimed {path}"
            );
        }
        for whole in [
            "/audio", "/songs", "/add", "/digest", "/open", "/trim", "/reveal",
        ] {
            assert_ne!(path, whole, "audio.rs would have claimed {path}");
        }
        assert_ne!(path, "/pagedata", "the dispatcher's own route");

        // And the other way: every route the modes below actually own must
        // fall straight through the palettes' prefix check.
        for theirs in [
            "/pagedata",
            "/audio",
            "/songs",
            "/rig/library",
            "/models/library",
            "/models/movement",
            "/router/take",
            "/curator/state",
            "/render/state",
        ] {
            assert!(
                !theirs.starts_with("/palettes"),
                "the palettes would have swallowed {theirs}"
            );
        }
    }
}
