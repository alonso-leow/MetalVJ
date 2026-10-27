//! The live preview: every model rendered headlessly, side by side or one
//! large, with the float row under your hands.
//!
//! This was a third binary — a headless-GPU control room on its own port,
//! which the float studio pushed rows to over a socket it hoped someone was
//! listening on. It is a module now, and the push is
//! [`State::showing`]: a function call that returns an error you can see
//! instead of a datagram that vanishes.
//!
//! Everything here runs on the request thread. There is no render loop and no
//! background worker: the browser asks for a frame, this draws it, encodes it
//! and hands it back. The page requesting the next frame only when the last
//! one arrived is what paces it, so a slow model slows its own preview instead
//! of piling up work nobody is waiting for any more.

use web_time::Instant;

use metal_render::model::Block;
use serde_json::{json, Value};

use crate::author::live::{self, Adopt, Live};
pub use metal_render::headless::{encode_jpeg, Renderer};

/// The renderer's model list, copied out once when the renderer is built.
///
/// The set of models never changes after construction, so every route that
/// only needs names, ids or schemas reads this copy instead of the renderer.
/// That keeps those routes off the renderer's lock, which a grid render holds
/// for about 20 ms while it draws.
pub struct Catalog {
    names: Vec<&'static str>,
    ids: Vec<&'static str>,
    schemas: Vec<&'static metal_render::model::Schema>,
}

impl Catalog {
    pub fn of(renderer: &Renderer) -> Catalog {
        Catalog {
            names: renderer.names(),
            ids: renderer.ids(),
            schemas: renderer.schemas(),
        }
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.names.clone()
    }

    pub fn ids(&self) -> Vec<&'static str> {
        self.ids.clone()
    }

    pub fn schemas(&self) -> Vec<&'static metal_render::model::Schema> {
        self.schemas.clone()
    }
}

pub struct State {
    pub(crate) rows: Vec<Vec<f32>>,
    pub(crate) started: Instant,
    /// Time is served from here rather than from the browser so every model in
    /// the grid is drawn at the same instant. Pausing freezes it in place.
    pub(crate) paused_at: Option<f32>,
    pub(crate) status: String,
    /// Which model the page is looking at. Server-side because the models mode
    /// can change it, and the page has to be able to find that out.
    pub(crate) selected: usize,
    /// Bumped by every adopted row. The page watches it to know its sliders
    /// are stale — cheaper and more honest than pushing to the page, which
    /// would need a second socket for a value it only reads.
    pub(crate) live_seq: u64,
}

impl State {
    pub fn new(renderer: &Renderer) -> State {
        State {
            rows: renderer.schemas().iter().map(|s| s.defaults()).collect(),
            started: Instant::now(),
            paused_at: None,
            status: String::new(),
            selected: 0,
            live_seq: 0,
        }
    }

    pub(crate) fn time(&self) -> f32 {
        self.paused_at
            .unwrap_or_else(|| self.started.elapsed().as_secs_f32())
    }

    /// Adopts a row as what a model is currently showing, and selects it.
    ///
    /// The in-process replacement for the `/live` socket. Both failures are
    /// returned rather than swallowed: from the authoring side a silent no-op
    /// used to be indistinguishable from "the renderer is not running", and
    /// telling those apart was the entire value of the old bridge.
    pub fn showing(
        &mut self,
        id: &str,
        floats: Vec<f32>,
        catalog: &Catalog,
    ) -> Result<usize, String> {
        let ids = catalog.ids();
        let widths: Vec<usize> = catalog.schemas().iter().map(|s| s.width()).collect();
        match live::resolve(id, floats.len(), &ids, &widths) {
            Adopt::Row(index) => {
                self.rows[index] = floats;
                self.selected = index;
                self.live_seq += 1;
                self.status = format!("showing: {id}");
                Ok(index)
            }
            Adopt::Unknown => Err(format!("no model {id:?} in this build")),
            Adopt::WrongWidth { expected, got } => Err(format!(
                "{id} sent {got} floats, this build's schema takes {expected} — \
                 run `cargo run -p metal-render --bin emit-schema`"
            )),
        }
    }

    /// Takes whatever an *external* pusher last sent, if `--live-port` is on.
    ///
    /// Kept for the one case a socket was ever the right answer: a separate
    /// process driving this preview. The two studios that used to need it are
    /// one process now and call [`State::showing`] directly.
    pub fn adopt_live(&mut self, live: &Live, catalog: &Catalog) {
        let Some((id, floats)) = live.take() else {
            return;
        };
        if let Err(e) = self.showing(&id, floats, catalog) {
            self.status = format!("/live: {e}");
        }
    }
}

// the cleanup: the raw-socket `serve` dispatcher that used to live here moved to
// `crate::call::author::render` as typed handler functions over `State`'s
// fields (now `pub(crate)`) and `time()` below.

/// The one payload the preview needs to build itself: every model's name and
/// the shape of its float row, **as this binary's renderers actually declare
/// it**. The page has no list of models and no list of sliders — adding a
/// float to a `Schema` makes its slider appear here.
///
/// Deliberately not the same source as `models::page_data`, which reads
/// `data/schema.json`. That file is generated from these very schemas, so the
/// two agreeing is the invariant and their disagreement is the bug worth
/// surfacing rather than papering over.
pub fn page_data(catalog: &Catalog) -> Value {
    let names = catalog.names();
    let ids = catalog.ids();
    let models: Vec<Value> = catalog
        .schemas()
        .iter()
        .enumerate()
        .map(|(i, schema)| {
            let blocks: Vec<Value> = schema
                .blocks
                .iter()
                .map(|b| match b {
                    Block::Floats(specs) => json!({
                        "kind": "floats",
                        "specs": specs.iter().map(|s| json!({
                            "name": s.name, "min": s.min, "max": s.max, "default": s.default,
                        })).collect::<Vec<_>>(),
                    }),
                    Block::ColorSlot(name, _) => json!({ "kind": "color", "name": name }),
                    Block::Camera(_) => json!({ "kind": "camera" }),
                })
                .collect();
            json!({
                // The id as well as the name: the merged page authors
                // keyframes against the same models it previews, and a
                // keyframe is filed under the id. Two payloads, one carrying
                // the id and the other the sliders, is how the page ended up
                // reconciling two lists of models by index.
                "id": ids[i],
                "name": names[i],
                "width": schema.width(),
                "defaults": schema.defaults(),
                "blocks": blocks,
            })
        })
        .collect();
    json!({ "models": models })
}

// ---------------------------------------------------------------------------
// the little bits of HTTP this needs and no more
// ---------------------------------------------------------------------------
