//! `S.U9`: the graph canvas, in the [`super::Region::Nodes`] region
//! (`plan-sonnet.md` § S.U9, `plan-opus.md` § O.12.e).
//!
//! **Regla 5.** Layout and hit-testing live only in `metal-graph/src/layout.rs`,
//! reached through the door. This file paints exactly the `primitives`
//! `GET /graph` already resolved (`metal_graph::layout::Layout::primitives`,
//! wrapped by `crate::graph_reply`) and never computes a node box, a port
//! point or an edge curve itself — the one thing it *does* compute locally
//! is where the pointer landed in graph space, which is input, not layout,
//! and every such point is sent to `GET /graph/hit` for the door to resolve.
//! `pan`/`zoom` are a view transform only (`plan-opus.md` § O.12.e's own
//! words: "fit / zoom-at-pointer are view transforms, not layout") — where
//! the operator is looking, never a second source of node position.
//!
//! **Regla 6 / the transversal rule.** Every call here goes through
//! [`crate::call::Call`] — `door.call`/`door.json` — never a handler body.
//!
//! **Router/curator lane-painting ownership**, the finding
//! `plan-opus.md` § O.12.a's inventory left for this lote and `S.U10`:
//! `router.js` and `curator.js` paint onto the *same* physical lane canvas
//! `rig.js`/`suite.js` already own (`SHELL.laneWindow`). This file owns the
//! router node's graph-level surface — select it, move it, bypass/solo/
//! duplicate/remove it, flip its effects — exactly like any other node kind.
//! The router's own lane (`mode`/`picked`/`sensitivity`/train — `POST
//! /router`, `crate::call::transmit::router` — and the tick-by-tick pick
//! lane `router.js` paints) is explicitly **not** this lote's: it is the
//! same shared lane machinery `S.U10` already has to sort out for
//! `rig.js`/`suite.js`, and painting a second lane widget here would
//! re-split what the inventory found already shared. Registered as the
//! leftover row `S.U9b` in `plan-sonnet.md`, alongside the studio
//! live-preview grid (`shell.js`'s `layout`/`fitStage`/`buildOverlay`/…).

use std::collections::HashMap;

use egui::epaint::CubicBezierShape;
use egui::{Align2, Id, Pos2, Rect as EguiRect, Response, RichText, Sense, Shape, Ui, Vec2};
use serde_json::{json, Value};

use crate::call::{Call, Params};

use crate::shell::classes::Theme;
#[cfg(test)]
use crate::shell::classes::DRAG_CLEARANCE;
use crate::shell::components::{dropdown, hsb_picker, icon_button, panel_frame, swatch_chip, Icon};
use crate::shell::gallery;
use crate::shell::naming;

use metal_notation::color::Hsb;
use crate::shell::i18n::{tr, tr_dyn, tr_key, trf};
use crate::shell::panels::viewport;
use crate::shell::panels::Selection;

/// How far, in graph units, the pointer has to move between press and
/// release for a press-drag-release to count as a move rather than a click
/// that happened to jitter a pixel — `graphDragSlop` in the old HTML dashboard
/// (deleted 15-Sep-2026, S.U18; rule 11) picked the same kind of threshold for the
/// same reason: a click that moved the node by rounding error must not turn
/// into a `POST /graph/node` write.
const DRAG_SLOP: f32 = 3.0;

/// How often the canvas re-reads `GET /graph/live` while it is on screen. The
/// route is answered from the engine's last frame (`call::graph::graph_live`),
/// so this is a poll of a cache, not a render request; 4–5 Hz (N's brief) is
/// fast enough to watch a driven float move and slow enough not to flood the
/// door. The same `live_gap()`-shaped gate the panels' other pumps use.
const LIVE_POLL: std::time::Duration = std::time::Duration::from_millis(220);

/// How far, in graph units, a horizontal drag on a value field has to move
/// before it counts as an edit rather than a click that selected the row —
/// the same slop the node move keeps, for the same reason.
const PARAM_DRAG_SLOP: f32 = 2.0;

/// The fraction of the parameter's declared range a value-field drag covers
/// per graph unit of horizontal travel (N's brief: "range/200 per pixel").
const PARAM_DRAG_SPAN: f32 = 200.0;

/// One node being dragged: its id, where it stood in graph space before the
/// press (read off the cached graph, never recomputed), where the pointer
/// first landed (also graph space, from the door's own hit-test reply), and
/// where it is now — the two together give the delta `finish_drag` applies
/// on release.
struct Drag {
    node: String,
    start_node_pos: [f32; 2],
    start_pointer: [f32; 2],
    cur_pointer: [f32; 2],
}

/// A cable being stretched from one port: the endpoint the press resolved
/// (`GET /graph/hit`'s port answer — rule 5 all the way), where that port
/// sits in graph space (the reply's own `point`) and where the pointer is
/// now (screen space, for the ghost line). The release hit-test only
/// *connects* when it lands on a *different* port: a drop on the port the
/// press came from is a no-op, exactly what `add_edge_checked` would
/// refuse anyway — a cable needs two ends.
struct CableDrag {
    from_node: String,
    from_port: String,
    from_point: [f32; 2],
    /// Whether the pressed port was an output (`true`) or input (`false`) —
    /// `GET /graph/hit`'s own `dir` field. A cable can be stretched from
    /// either end of a link, so `finish_cable` needs this to know which side
    /// of the eventual `POST /graph/edge` body the pressed port belongs on.
    from_is_out: bool,
    cur_screen: Pos2,
}

/// An empty-space drag in flight: where the pointer pressed (screen space)
/// and what the view's pan was then. Moving the pointer pans the view — a
/// view transform only (`plan-opus.md` § O.12.e: "pan/zoom are a view
/// transform only *...*") — and commits nothing to the door.
struct PanDrag {
    start_screen: Pos2,
    start_pan: Vec2,
}

/// A drag on a parameter's value field (N): the row it edits, where the
/// pointer pressed (graph space), the value it started from, and the range
/// the drag scales against. Nothing reaches the door until release — one
/// `POST /graph/node { op: "param" }` per gesture, never a stream while the
/// pointer moves.
struct ParamDrag {
    node: String,
    name: String,
    start_value: f32,
    start_pointer: [f32; 2],
    cur_pointer: [f32; 2],
    min: f32,
    max: f32,
    /// The value the drag has produced so far, clamped to the range — what
    /// the field paints while the pointer is down.
    value: f32,
}

/// A cable stretched from an output that carries *several* variables, waiting
/// for the operator to pick which one binds to the parameter it was dropped
/// on (N). Holds the drop target and the producer's variables
/// (`GET /graph/io`'s `sources`, read fresh when the menu opens), plus where
/// on screen to paint the little chooser.
#[derive(Clone)]
struct BindMenu {
    /// The consumer whose parameter will be bound.
    node: String,
    param: String,
    /// The producer the variables leave.
    producer: String,
    /// The variable rows: `(port, var, port_ref)`.
    vars: Vec<(String, String, String)>,
    at: Pos2,
}

/// The colour picker a clicked node swatch opens (2026-10-08): the node and
/// the config key the chip stands for (`rgb` or `colors:<i>`), the seed RGB,
/// and where to paint. The canvas's own way to set a source's key/secondary
/// colours without going to the properties panel.
#[derive(Clone)]
struct ColourMenu {
    node: String,
    key: String,
    rgb: [f32; 3],
    at: Pos2,
}

/// The bar's `add before`/`add after` press, held while the "new" gallery is
/// open so the kind the operator picks lands spliced into the chain instead
/// of floating free. The gallery pick carries it back through
/// [`PanelState::add_from_gallery`], where the new node is wired into place.
#[derive(Clone)]
struct InsertRequest {
    /// The selected node the new kind is spliced around.
    node: String,
    /// `true` -> the new node goes downstream of `node` (its image consumers
    /// re-hang off the new node); `false` -> upstream, between `node` and
    /// whatever feeds its `image` input.
    after: bool,
}

/// One inverse edit the [`PanelState::undo`] stack can replay through the
/// same door routes the forward edits used — never a second path to a
/// mutation (rule 6 inside one panel). Built *before* a forward edit
/// commits, from the cached graph the panel already holds (never
/// recomputed — rule 5 stays clean: these are edit records, not layout).
enum Undo {
    /// A move is undone by moving the node back to where it stood before
    /// the drag (`start_node_pos`, not wherever the pointer was).
    Move { node: String, pos: [f32; 2] },
    /// An add (or duplicate — the reply already carries the new node's id)
    /// is undone by removing that node again; edges drop with the node
    /// server-side (`metal_graph::Graph::remove_node`).
    Remove { node: String },
    /// A remove is undone by re-adding: kind, position and the two boolean
    /// flags the node had — the only parts of a node this panel's own
    /// forward routes can express, so they are the only ones the inverse
    /// restores. The label/cost config lost with a remove does not come
    /// back (the door has no route to re-write those in one call), which
    /// is why the case is recorded at all: the button flips it *first*,
    /// honestly, not as an apology after the fact.
    Add {
        kind: String,
        pos: [f32; 2],
        bypass: bool,
        solo: bool,
    },
    /// An edge removal is undone by re-adding the cable between the same
    /// two endpoints (read off the cached graph before the remove).
    Edge {
        from: (String, String),
        to: (String, String),
    },
    /// A flag flip is undone by writing back the value it had before.
    Flag {
        node: String,
        field: String,
        was: bool,
    },
    /// An edge *add* (the cable drag's own commit) is undone by removing
    /// that edge again — the route's own id (`reply.edge`), the same one a
    /// selected-edge `quitar` would send.
    Unedge { edge: String },
}

/// Everything the nodes panel remembers between frames: which graph is
/// open, its last-fetched reply (`graph`/`primitives`/`issues`, the exact
/// shape `crate::graph_reply` returns), and the view transform plus the
/// in-flight drag if any. Owned by [`super::super::Shell`]
/// so it survives across frames the way `Shell::focus`/`Shell::rects` do.
pub struct PanelState {
    opened: bool,
    graph_ids: Vec<String>,
    /// Each listed collection's display name, by graph id.
    graph_names: HashMap<String, String>,
    selected_graph: Option<String>,
    graph: Option<Value>,
    pan: Vec2,
    zoom: f32,
    pressing: bool,
    drag: Option<Drag>,
    pan_drag: Option<PanDrag>,
    /// An active cable stretch (`CableDrag`) — the "drag puerto a puerto"
    /// gesture. Resolves on release through the same `GET /graph/hit` every
    /// press used, and commits at most one `POST /graph/edge { op: "add" }`.
    cable: Option<CableDrag>,
    /// The inverse-edit stack (`CTRL + Z`): one [`Undo`] per panel-side
    /// mutation, capped at [`Theme::graph_undo_depth`] (a graph edit is a
    /// disk write, the stack stays bounded), popped from the end. Replayed
    /// through the same door routes the forward edits used — see [`Undo`].
    undo: Vec<Undo>,

    /// A drag on a value field (N) — see [`ParamDrag`]. One write on release.
    param_drag: Option<ParamDrag>,
    /// The variable chooser a multi-variable output drop opened (N) — see
    /// [`BindMenu`].
    bind_menu: Option<BindMenu>,
    /// The colour picker a clicked node swatch opened (2026-10-08) — see
    /// [`ColourMenu`].
    colour_menu: Option<ColourMenu>,
    /// The last `GET /graph/live` reply, and when it landed — polled at
    /// [`LIVE_POLL`] while the canvas paints, so a driven float shows its
    /// current value without a request per frame.
    live: Option<Value>,
    live_at: Option<web_time::Instant>,

    /// Plan "Surface sources": a node another panel just made or pointed at.
    /// Set by [`PanelState::reveal`] (the shell drains the properties panel's
    /// request into it); the canvas consumes it on its next paint: it reloads
    /// the graph from the door, pans the node into view and starts [`pulse`].
    /// The selection is never touched here.
    ///
    /// [`pulse`]: PanelState::pulse
    reveal: Option<String>,
    /// The node whose outline pulses and when the pulse began — [`PULSE`] long.
    pulse: Option<(String, web_time::Instant)>,

    /// `R22.1`: the animation of the focused `gen.*` node, floating over the
    /// canvas's top-right corner — `GET /source.jpg?device=&layer=` through
    /// the shared [`crate::shell::preview::FloatingPreview`], painted only
    /// when the selection *is* a generator (nothing else has an "own
    /// animation" to show, and no other selection pumps the route at all).
    /// A click toggles it to the canvas's whole width.
    preview: crate::shell::preview::FloatingPreview,
    /// 2026-10-09: with an `fx.*` focused, the picture feeding its `image`
    /// input — the "before" beside [`Self::preview`]'s "after".
    preview_before: crate::shell::preview::FloatingPreview,
    /// `shared::writes()` as of the last graph read: a bump means the
    /// properties panel wrote and `graph` is stale.
    seen_writes: u64,
    /// The studio grid — one tile per model, in one texture
    /// (`GET /author/render/grid.jpg`, the pump the biblioteca used to
    /// paint as its own section). Here it is the *picture of each kind*:
    /// a `gen.*` node's box shows its model's tile, and the `+ node` menu
    /// floats the same tile beside the entry under the pointer. One fetch
    /// per cadence for every node on the canvas, cropped by `uv`.
    tiles: crate::shell::frames::Frame,
    /// The gallery's cards: the grid rendered at each `variant`, one picture
    /// per variant so each is cached on its own (see [`variant_to_fetch`]).
    gallery_tiles: [crate::shell::frames::Frame; GALLERY_VARIANTS],
    /// The fx/mat cards' own pictures, one frame per `(kind, variant)`: each is
    /// fetched once, lazily, while its card is on screen, and then kept.
    kind_tiles: HashMap<(String, usize), crate::shell::frames::Frame>,
    /// The kinds whose cards the gallery showed last frame (its own fetch list).
    gallery_kinds: Vec<String>,
    gallery_open: bool,
    /// Whether a gallery card is on screen last frame: the grid is fetched
    /// only then (scroll-aware, see [`variant_to_fetch`]).
    gallery_visible: bool,
    /// The frame clock (seconds) the gallery picture is read at.
    gallery_clock: f64,
    /// `GET /author/render/state`'s `names`, read once with the graph list
    /// — the model order the grid was laid out in, so a `gen.<id>` finds
    /// its tile by the same index the server drew it at (rule 5).
    model_names: Vec<String>,
    /// The studio's row of each model, in `model_names` order
    /// (`GET /author/render/state` `rows`): the base a node's own floats are
    /// laid over to make the row its tile is drawn from.
    model_rows: Vec<Vec<f32>>,
    /// Where each float of a `gen.*` kind sits in its row, `(name, index)`
    /// (`GET /catalog` `params`).
    row_slots: HashMap<String, Vec<(String, usize)>>,
    /// One picture per `gen.*` node, drawn from that node's own row
    /// (`GET /author/render/solo.jpg?row=`); the grid's cell is the fallback
    /// until it lands.
    node_tiles: HashMap<String, crate::shell::frames::Frame>,
    /// The canvas's own last-painted rect — test-only in effect (a kittest
    /// pointer test needs a real point inside the canvas, not the whole
    /// region's outer frame, to know a click landed past the toolbar), the
    /// same reasoning `Shell::region_rects` documents for why this is a
    /// plain getter rather than a `#[cfg(test)]` field.
    last_canvas_rect: Option<EguiRect>,
    /// The cable under the pointer this frame — lit like a selected one, so
    /// the operator sees which cable a click would pick before clicking.
    hover_edge: Option<String>,
    /// The Ctrl+click multi-selection: the node ids picked so far, in pick
    /// order. Only meaningful with two or more ids and the node selection
    /// on the last one (the primary) — see [`PanelState::picked_nodes`].
    pub(crate) picked: Vec<String>,
    /// The floating `new` button was pressed: the shell opens the gallery.
    new_requested: bool,
    /// Set together with `new_requested` by the bar's `add before`/`add
    /// after`: where the picked kind should be spliced. Cleared when the
    /// gallery closes without a pick (`set_gallery_open`), so a dismissed
    /// overlay never splices a later, unrelated add.
    insert_request: Option<InsertRequest>,
    /// The gear on a `venue.surface` node's focus bar was pressed: the shell
    /// flips this canvas into the venue scene (`panels::venue_canvas`) for
    /// that node's room. `(graph, node)`.
    ///
    /// **Two doors, one channel.** The `output.device` page's "abrir la
    /// escena" row (2026-10-22) writes the same field — the wire after the
    /// device asks for the room its `venue` output feeds — so the room is
    /// never opened from a second, panel-only state (rule 4).
    pub(crate) venue_requested: Option<(String, String)>,
    /// The canvas's **venue scene** state — `Some` while the canvas shows the
    /// whole room instead of the graph (`panels::venue_canvas`). A canvas view
    /// state, not a second canvas: the canvas keeps `Region::Nodes`. Nothing
    /// on disk persists it; it lives in memory with the rest of the panel's
    /// state (the graph view's `pan`/`zoom` are the shell's config, this is
    /// not), so a restart opens the graph.
    pub(crate) venue_scene: Option<VenueScene>,
    /// The rooms a starter scene was already posted for this session, so an
    /// empty room is filled once and never twice (`venue_canvas`).
    pub(crate) posted_rooms: std::collections::HashSet<String>,
    /// While `Some`, the venue gallery (`panels::venue_popup`) is open over
    /// the scene for that node: the bar's `+` toggles it.
    pub(crate) gallery: Option<String>,
    /// The gallery's own scratch (tab, name, throw, note), opened by the
    /// bar's `+`. In memory with the panel, like the rest of its state.
    pub(crate) venue_gallery: super::venue_popup::State,
}

/// The canvas's venue scene: which room, from where, and the one scratch fact
/// the painter keeps (the starter's note).
pub(crate) struct VenueScene {
    /// The orbit camera the scene is looked through.
    pub camera: viewport::OrbitCamera,
    /// The venue id painted — re-read from the graph every frame
    /// (`venue_map::venue_of_graph`), kept so the painter knows what changed.
    pub room: String,
    /// The item picked in the room, by id (`None` until picking is wired).
    pub selected: Option<String>,
    /// A one-line note under the room (the starter's "started with…").
    pub note: Option<String>,
    /// Whether the camera already took the room's own default framing.
    pub(crate) fitted: bool,
}

impl Default for PanelState {
    fn default() -> Self {
        PanelState {
            opened: false,
            graph_ids: Vec::new(),
            graph_names: HashMap::new(),
            selected_graph: None,
            graph: None,
            pan: Vec2::ZERO,
            zoom: 1.0,
            pressing: false,
            drag: None,
            pan_drag: None,
            cable: None,
            undo: Vec::new(),
            param_drag: None,
            bind_menu: None,
            colour_menu: None,
            live: None,
            live_at: None,
            reveal: None,
            pulse: None,
            preview: crate::shell::preview::FloatingPreview::default(),
            preview_before: crate::shell::preview::FloatingPreview::default(),
            seen_writes: super::properties::shared::writes(),
            tiles: crate::shell::frames::Frame::default(),
            gallery_tiles: std::array::from_fn(|_| crate::shell::frames::Frame::default()),
            kind_tiles: HashMap::new(),
            gallery_kinds: Vec::new(),
            gallery_open: false,
            gallery_visible: false,
            gallery_clock: 0.0,
            model_names: Vec::new(),
            model_rows: Vec::new(),
            row_slots: HashMap::new(),
            node_tiles: HashMap::new(),
            last_canvas_rect: None,
            hover_edge: None,
            picked: Vec::new(),
            new_requested: false,
            insert_request: None,
            venue_requested: None,
            venue_scene: None,
            posted_rooms: std::collections::HashSet::new(),
            gallery: None,
            venue_gallery: super::venue_popup::State::default(),
        }
    }
}

impl PanelState {
    /// The Ctrl+click multi-selection, when there is one: two or more nodes
    /// with the node selection on the last one picked. Empty otherwise, so
    /// a single selection (or a stale list) reads exactly as it always did.
    pub(crate) fn picked_nodes(&self, selection: &Selection) -> Vec<String> {
        match selection {
            Selection::Node(id) if self.picked.len() >= 2 && self.picked.last() == Some(id) => self.picked.clone(),
            _ => Vec::new(),
        }
    }

    /// The canvas's own rect as of the last frame it painted — `None`
    /// before the first frame. See the field's own doc comment.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn canvas_rect(&self) -> Option<EguiRect> {
        self.last_canvas_rect
    }

    /// Reads and clears the floating `new` button's press — the shell's one
    /// call per frame, which opens the gallery.
    pub(crate) fn take_new_request(&mut self) -> bool {
        std::mem::take(&mut self.new_requested)
    }

    /// Reads and clears the venue gear's press — the shell's one call per
    /// frame — and flips the canvas into the venue scene for that node's room
    /// in the same call, so the flip paints on this frame rather than the
    /// next (the request and the flip are one thing; nothing else reads it).
    pub(crate) fn take_venue_request(&mut self) -> Option<(String, String)> {
        let request = self.venue_requested.take();
        if request.is_some() {
            self.open_venue_scene();
        }
        request
    }

    /// Flips the canvas into the venue scene of the open graph's room. The
    /// scene is the canvas's view state: `Region::Nodes`, nothing else moves.
    pub(crate) fn open_venue_scene(&mut self) {
        let room = self
            .selected_graph
            .as_deref()
            .map(|gid| {
                crate::shell::panels::venue_map::venue_of_graph(gid, &self.graph_reply())
            })
            .unwrap_or_default();
        self.venue_scene = Some(VenueScene {
            camera: viewport::OrbitCamera::default(),
            room,
            selected: None,
            note: None,
            fitted: false,
        });
    }

    /// `nodos`: back to the graph. The gallery goes with it — it belongs to
    /// the scene.
    pub(crate) fn close_venue_scene(&mut self) {
        self.venue_scene = None;
        self.gallery = None;
        self.venue_gallery.close();
    }

    /// The room the venue scene is painting — test-only reader.
    #[cfg(test)]
    pub(crate) fn venue_room_for_test(&self) -> Option<String> {
        self.venue_scene.as_ref().map(|s| s.room.clone())
    }

    /// Whether the canvas is in its venue scene — test-only reader for the
    /// kittest that presses the gear.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn in_venue_scene(&self) -> bool {
        self.venue_scene.is_some()
    }

    /// Opens the venue gallery for `node` over the scene (test-only: the bar's
    /// `+` is the real door).
    #[cfg(test)]
    pub(crate) fn open_gallery_for_test(&mut self, node: &str) {
        let gid = self.selected_graph.clone().unwrap_or_default();
        self.venue_gallery.open(&gid, node, 1);
        self.gallery = Some(node.to_string());
    }

    /// Hands the shell a room request made by another panel — the
    /// `output.device` page's "abrir la escena" — so it drains through
    /// [`PanelState::take_venue_request`] like the gear's own press. One
    /// channel, one popup: the request never becomes a second door.
    pub(crate) fn offer_venue_request(&mut self, request: (String, String)) {
        self.venue_requested = Some(request);
    }


    /// The `GET /graph` reply the canvas last painted, for a caller that
    /// needs the same node list the canvas chose from (the room popup, which
    /// must read the node's config and the graph's venues from one reply).
    /// `Value::Null` before the first read.
    pub(crate) fn graph_reply(&self) -> serde_json::Value {
        self.graph.clone().unwrap_or(serde_json::Value::Null)
    }

    /// Which graph is open, as of the last frame this panel painted — read
    /// by `S.U12`'s properties panel (`super::properties`) so it knows
    /// which graph to fetch the selected node's data from without needing
    /// its own copy of "which graph is open". One frame behind a graph
    /// switch at worst (this reads before `nodes::paint` runs for the
    /// current frame, see `regions.rs::paint_top_row`), which a graph
    /// switch — an infrequent, deliberate action — never makes visible.
    pub(crate) fn selected_graph(&self) -> Option<&str> {
        self.selected_graph.as_deref()
    }

    /// Open graph `id` as the picker would (the header's listen dot asks for
    /// the graph its `audio.live` lives in).
    pub(crate) fn open_graph(&mut self, door: &dyn Call, id: &str) {
        if self.selected_graph.as_deref() != Some(id) {
            load_graph(door, self, id.to_string());
        }
    }

    /// Test-only writer: preopen a graph without simulating the picker's
    /// pixel geometry, so a kittest can reach the `R22.4` `+ create button`
    /// popover with a graph in hand.
    #[cfg(test)]
    pub(crate) fn set_selected_graph_for_test(&mut self, id: &str) {
        self.selected_graph = Some(id.to_string());
    }

    /// Asks the canvas to bring `node` into view and pulse it (plan "Surface
    /// sources", the cross-panel reveal). The next canvas paint does the
    /// work; the selection stays where it was.
    pub(crate) fn reveal(&mut self, node: String) {
        self.reveal = Some(node);
    }

    /// The node a pulse is running on, while [`PULSE`] has not run out.
    fn pulsing(&self) -> Option<(&str, f32)> {
        let (id, at) = self.pulse.as_ref()?;
        let left = PULSE.checked_sub(at.elapsed())?;
        Some((id.as_str(), left.as_secs_f32() / PULSE.as_secs_f32()))
    }
}

/// How long a revealed node's outline pulses.
const PULSE: std::time::Duration = std::time::Duration::from_millis(1500);

/// The pan that puts graph point `at` at the middle of a canvas `size` big,
/// under the view transform `screen = origin + pan + graph·zoom` (so the
/// canvas's own origin cancels out of the answer).
impl PanelState {
    /// The canvas view transform (`pan`, `zoom`), for the shell's layout memory.
    pub(crate) fn view(&self) -> (Vec2, f32) {
        (self.pan, self.zoom)
    }

    /// Restores a remembered view. The zoom is clamped to the same floor and
    /// ceiling the zoom buttons obey, so a hand-edited config cannot put the
    /// canvas somewhere the operator could not get back from.
    pub(crate) fn set_view(&mut self, pan: Vec2, zoom: f32, theme: &Theme) {
        self.pan = pan;
        self.zoom = zoom.clamp(theme.graph_zoom_min, theme.graph_zoom_max);
    }
}

fn pan_to_centre(size: Vec2, at: [f32; 2], zoom: f32) -> Vec2 {
    size * 0.5 - Vec2::new(at[0], at[1]) * zoom
}

/// Paint the whole `nodes` region — heading, graph picker, the control bar
/// (`+ node ▾`, the selected node's verbs) and the canvas with the
/// animation preview over it — and report the outer frame
/// as a clickable [`Response`], exactly [`crate::shell::components::region_placeholder`]'s
/// own contract, so [`super::super::regions`] learns a click here the same
/// way it learns one on any other region.
pub fn paint(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    fetcher: Option<&crate::shell::frames::Fetcher>,
    state: &mut PanelState,
    selection: &mut Selection,
    focused: bool,
) -> Response {
    let _span = crate::perf::Span::new("nodes");
    ui.push_id(Id::new(super::Region::Nodes.id()), |ui| {
        let mut frame = panel_frame(theme);
        if focused {
            frame = frame.stroke(theme.accent());
        }
        let inner = frame.show(ui, |ui| {
            ui.expand_to_include_rect(ui.max_rect());
            let full_rect = ui.max_rect();
            ensure_loaded(door, state);
            // `CTRL + Z` is the shell's global keybind (`shell::keybinds`),
            // which calls [`undo_last`] while this panel has the focus — the
            // same code the `revertir` button's edits go through.
            // Delete / Backspace remove what is selected — the node or the
            // cable — while this panel holds the focus and no text field is
            // typing (a button that kept the focus after a click does not
            // count, the same guard the sequence editor keeps).
            let typing = ui
                .ctx()
                .memory(|m| m.focused())
                .is_some_and(|id| egui::TextEdit::load_state(ui.ctx(), id).is_some());
            if focused
                && !typing
                && ui.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                        || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
                })
            {
                delete_selection(door, state, selection, theme);
            }
            // Ctrl+C copies the selected node(s) as one clip on the system
            // clipboard; Ctrl+V of that clip pastes them (see `paste_nodes`).
            if focused && !typing && ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
                copy_selection(ui.ctx(), state, selection);
            }
            ui.vertical(|ui| {
                let bg = toolbar(ui, theme, door, state, selection, full_rect);
                master_row(ui, theme, door);
                // The canvas takes everything the bars above left — with
                // no floor past the panel's own available height. An
                // earlier version floored this at `region_min.y`: whenever
                // the panel was ceded (or the window short), the frame's
                // contents spilled past the cell the grid handed the
                // region and painted over the row below — the "the
                // focused one sits over the other" spill. (Until 21-Sep-2026 a
                // 200 pt ops strip sat under the canvas as well, mostly
                // empty, and on a short panel hid the canvas outright; its
                // verbs moved up onto `canvas_bar`.)
                let canvas_h = ui.available_height().max(0.0);
                canvas(ui, theme, door, fetcher, state, selection, canvas_h);
                bg
            })
            .inner
        });
        inner.inner
    })
    .inner
}

// ---------------------------------------------------------------------------
// loading
// ---------------------------------------------------------------------------

/// Runs once per [`PanelState`] — the panel's own "boot sequence"
/// (the old page's `initGraphEditor`, deleted 15-Sep-2026, S.U18): list the
/// graphs, load one, and read the head note. Every later graph load
/// (picking a different graph, adding/moving/removing a node) goes through
/// [`load_graph`] directly, never back through here.
fn ensure_loaded(door: &dyn Call, state: &mut PanelState) {
    if state.opened {
        return;
    }
    refresh_graph_list(door, state);
    // The renderer's model order is fixed for the process, so one read
    // is the whole cadence; a glow-only build (no `/author/*`) simply
    // leaves it empty and no tile is ever painted.
    let models = get(door, "GET", "/author/render/state", &[]);
    // These reads are answered behind the frame on first use (`Call::is_pending`).
    // Until they land the panel shows its empty state and boots again next
    // frame, rather than latching an empty graph list for the whole run.
    if ["/graphs", "/graph", "/author/render/state"].iter().any(|p| door.is_pending(p)) {
        return;
    }
    state.opened = true;
    state.model_rows = models
        .as_ref()
        .and_then(|v| v["rows"].as_array())
        .map(|rows| {
            rows.iter()
                .map(|r| r.as_array().into_iter().flatten().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
                .collect()
        })
        .unwrap_or_default();
    state.row_slots = row_slots_of(get(door, "GET", "/catalog", &[]).as_ref());
    state.model_names = models
        .and_then(|v| v["names"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
}

/// The float slots of every `gen.*` kind in a `GET /catalog` reply: each
/// float param's name and its position in the model's row.
fn row_slots_of(catalog: Option<&Value>) -> HashMap<String, Vec<(String, usize)>> {
    let mut out = HashMap::new();
    for entry in catalog.and_then(|c| c["catalog"].as_array()).into_iter().flatten() {
        let Some(kind) = entry["kind"].as_str().filter(|k| k.starts_with("gen.")) else { continue };
        let slots = entry["params"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["kind"] == json!("float"))
            .filter_map(|p| Some((p["name"].as_str()?.to_string(), p["index"].as_u64()? as usize)))
            .collect();
        out.insert(kind.to_string(), slots);
    }
    out
}

/// The row a `gen.*` node's tile is drawn from: its model's studio row with
/// the node's own floats laid over it — the effective value the last frame
/// used (`GET /graph/live`), else the float it holds (`config.floats`).
/// Colour slots and the camera stay the studio's: the node config holds
/// neither (a colour slot cannot be held yet, the camera is no param).
fn node_row(state: &PanelState, node: &Value) -> Option<Vec<f32>> {
    let kind = node["kind"].as_str()?;
    let model = kind.strip_prefix("gen.")?;
    let at = state.model_names.iter().position(|n| n == model)?;
    let mut row = state.model_rows.get(at)?.clone();
    let held = node["config"]["floats"].as_object();
    let live = state.live.as_ref().and_then(|l| l["nodes"][node["id"].as_str()?]["in"].as_object());
    for (name, slot) in state.row_slots.get(kind).into_iter().flatten() {
        let value = live
            .and_then(|l| l.get(name))
            .or_else(|| {
                metal_schema::param_aliases::alias_keys(name).find_map(|k| held.and_then(|h| h.get(k)))
            })
            .and_then(Value::as_f64);
        if let (Some(v), Some(cell)) = (value, row.get_mut(*slot)) {
            *cell = v as f32;
        }
    }
    Some(row)
}

/// Asks for the picture of every `gen.*` node from its own row. Each node's
/// frame is rate-limited on its own by `gap`.
fn fetch_node_tiles(ctx: &egui::Context, door: &dyn Call, state: &mut PanelState, gap: std::time::Duration) {
    let nodes = state.graph.as_ref().and_then(|g| g["graph"]["nodes"].as_array()).cloned().unwrap_or_default();
    for node in &nodes {
        let (Some(id), Some(row)) = (node["id"].as_str(), node_row(state, node)) else { continue };
        let model = node["kind"].as_str().and_then(|k| k.strip_prefix("gen.")).unwrap_or_default();
        let query = [
            ("model", model.to_string()),
            ("w", TILE_W.to_string()),
            ("h", TILE_H.to_string()),
            ("row", row.iter().map(f32::to_string).collect::<Vec<_>>().join(",")),
        ];
        state.node_tiles.entry(id.to_string()).or_default().picture(ctx, door, "/author/render/solo.jpg", &query, gap);
    }
}

/// `GET /graphs`, then [`load_graph`] whichever id the picker should show —
/// the previously-selected one if it still exists, else the first. Mirrors
/// the old page's `refreshGraphPicker` (deleted 15-Sep-2026, S.U18): a graph list with nothing in it
/// leaves the canvas empty rather than guessing an id to load.
fn refresh_graph_list(door: &dyn Call, state: &mut PanelState) {
    // The picker lists collections only. A `published` graph, or any
    // `device-<screen>` id, is a generated copy the engine runs
    // (`metal_graph::collection`), not something to open or to pick.
    let rows: Vec<Value> = get(door, "GET", "/graphs", &[])
        .and_then(|v| v["graphs"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(is_pickable)
        .collect();
    state.graph_names = rows
        .iter()
        .filter_map(|g| {
            let id = g["id"].as_str()?;
            Some((id.to_string(), g["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(id).to_string()))
        })
        .collect();
    state.graph_ids = rows.iter().filter_map(|g| g["id"].as_str().map(str::to_string)).collect();
    let next = state
        .selected_graph
        .clone()
        .filter(|id| state.graph_ids.iter().any(|g| g == id))
        .or_else(|| state.graph_ids.first().cloned());
    match next {
        Some(id) => load_graph(door, state, id),
        None => {
            state.selected_graph = None;
            state.graph = None;
        }
    }
}

/// Whether a `GET /graphs` row is a collection the picker may list: not a
/// published copy (`published`), and not a `device-<screen>` id.
fn is_pickable(row: &Value) -> bool {
    row["published"].as_bool() != Some(true)
        && !row["id"].as_str().is_some_and(metal_graph::collection::is_device_graph_id)
}

/// The collection a graph reply names as its `source`, when it is not `id`
/// itself: the graph `id` is a published device copy of that collection.
fn published_source(id: &str, reply: Option<&Value>) -> Option<String> {
    reply
        .and_then(|v| v["graph"]["source"].as_str())
        .filter(|col| *col != id)
        .map(str::to_string)
}

/// `GET /graph?id=` and cache the reply. Selection is cleared outside this
/// panel when the graph changed underneath it — a node id from the graph
/// that was open is not necessarily a node id in the one that just opened
/// (the old page's `loadGraph` does the same: "selected = null").
fn load_graph(door: &dyn Call, state: &mut PanelState, id: String) {
    let mut reply = get(door, "GET", "/graph", &[("id", id.clone())]);
    // A published device graph (asked for by a listen dot, a stale selection)
    // is a generated copy: the canvas opens the collection it came from, so
    // the edits land on the one real graph (`metal_graph::collection`).
    let id = match published_source(&id, reply.as_ref()) {
        Some(col) => {
            reply = get(door, "GET", "/graph", &[("id", col.clone())]);
            col
        }
        None => id,
    };
    state.selected_graph = Some(id.clone());
    state.drag = None;
    state.picked.clear();
    state.param_drag = None;
    state.bind_menu = None;
    state.colour_menu = None;
    state.live = None;
    state.live_at = None;
    // The undo stack names nodes of *this* graph; opening another one makes
    // every recorded inverse a lie about ids that may not exist there.
    state.undo = Vec::new();
    match reply {
        Some(v) => {
            state.graph = Some(v);
        }
        None => {
            state.graph = None;
        }
    }
}

// ---------------------------------------------------------------------------
// toolbar: heading, graph picker, node ▾
// ---------------------------------------------------------------------------

/// The heading row plus the two note lines, and — the one thing this
/// function exists to explain — the click-to-focus catcher, sensed over the
/// *whole* region but interposed **between** the heading (passive) and the
/// graph picker/`node ▾` menu (interactive): egui resolves an overlapping
/// click to whichever sensing widget was added *most recently*, with no
/// fall-through to anything added earlier and no fall-through to anything
/// added later either — the topmost widget at a position claims the click
/// outright, sensing or not (a bare `ui.label` still occupies its slot).
/// Two consequences follow, and both are load-bearing: the catcher must be
/// added *after* the heading (so a click on `graph`'s own text — exactly
/// `focus_expands_and_others_cede`, `kittest.rs` — resolves to the catcher,
/// not swallowed by the label sitting on top of it) and *before* the graph
/// picker/`node ▾`/canvas/node-ops (so a click on any of those resolves to
/// *them*, not stolen by a catcher painted over the whole panel — this is
/// the bug a first version of this file shipped with: catcher added last,
/// after everything, and `node ▾` never opened again).
fn toolbar(
    ui: &mut Ui,
    theme: &Theme,
    _door: &dyn Call,
    _state: &mut PanelState,
    _selection: &mut Selection,
    full_rect: EguiRect,
) -> Response {
    // One graph per collection: no picker, no `+`, and no title text.
    let bg = ui
        .horizontal(|ui| ui.interact(full_rect, ui.id().with("click-bg"), Sense::click()))
        .inner;
    // The region keeps its accessible name; it is just not painted.
    let title = super::Region::Nodes.title();
    ui.response()
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, title));
    bg
}

impl PanelState {
    /// The "new" gallery's cards for the open graph, one [`gallery::Section`]
    /// per kind of node: the door's catalogue in relevance order for the
    /// selected node (its top three fits starred), filtered to the graph's
    /// domain. Empty while no graph is open.
    pub(crate) fn gallery_sections(&self, door: &dyn Call, selection: &Selection) -> Vec<gallery::Section> {
        let Some(gid) = self.selected_graph.clone() else {
            return Vec::new();
        };
        // Plan "Surface sources": the catalogue is asked in relevance order for
        // the open graph, anchored on the selected node (if any).
        let domain: Option<String> = open_domain(self).map(str::to_string);
        let mut query: Vec<(&str, String)> = match &domain {
            Some(d) => vec![("domain", d.clone())],
            None => Vec::new(),
        };
        query.push(("graph", gid));
        let anchored = matches!(selection, Selection::Node(_));
        if let Selection::Node(id) = selection {
            query.push(("anchor", id.clone()));
        }
        let mut catalog: Vec<Value> = get(door, "GET", "/catalog", &query)
            .and_then(|v| v["catalog"].as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter(|k| match &domain {
                Some(d) => kind_in_domain(k, d),
                None => true,
            })
            .collect();
        // Stable: equal relevance keeps the door's order.
        catalog.sort_by(|a, b| {
            let score = |k: &Value| k["relevance"].as_f64().unwrap_or(0.0);
            score(b).partial_cmp(&score(a)).unwrap_or(std::cmp::Ordering::Equal)
        });
        if anchored {
            for k in catalog.iter_mut().take(TOP_STARRED) {
                k["starred"] = json!(true);
            }
        }
        let ports = |k: &Value, side: &str| -> Vec<String> {
            k[side]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["name"].as_str().map(str::to_string))
                .collect()
        };
        SECTIONS
            .iter()
            .map(|section| gallery::Section {
                icon: section_icon(*section),
                label: tr(section_title(*section)).to_string(),
                items: catalog
                    .iter()
                    .filter(|k| section_of(k) == *section)
                    .map(|k| {
                        let kind = k["kind"].as_str().unwrap_or_default();
                        let label = k["label"].as_str().filter(|l| !l.is_empty());
                        gallery::Item {
                            kind: kind.to_string(),
                            label: label.map(tr_dyn).unwrap_or_else(|| naming::display_name(kind)),
                            family: naming::family_name(k["family"].as_str().unwrap_or("other")),
                            inputs: ports(k, "inputs"),
                            outputs: ports(k, "outputs"),
                            tiles: tiles_for_kind(self, kind),
                            starred: k["starred"].as_bool() == Some(true),
                            favourite: false,
                        }
                    })
                    .collect(),
            })
            .filter(|section| !section.items.is_empty())
            .collect()
    }

    /// The first landed picture of `kind`'s cards — what the sequence's
    /// gallery and the favourites show beside the nodes' own.
    pub(crate) fn tile_for(&self, kind: &str) -> Option<(egui::TextureId, [usize; 2], EguiRect)> {
        tiles_for_kind(self, kind).into_iter().flatten().next()
    }

    /// Every variant's picture of `kind`'s card (the 2x2 strip).
    pub(crate) fn tiles_for(&self, kind: &str) -> crate::shell::gallery::Tiles {
        tiles_for_kind(self, kind)
    }

    /// The shell tells the panel whether the "new" gallery is on screen, and
    /// which kinds' cards it shows: while it is, those cards' pictures are
    /// fetched, each variant on its own.
    pub(crate) fn set_gallery_open(&mut self, open: bool, visible: bool, now: f64, kinds: &[String]) {
        self.gallery_open = open;
        self.gallery_visible = visible;
        self.gallery_clock = now;
        self.gallery_kinds.clear();
        self.gallery_kinds.extend_from_slice(kinds);
        // A dismissed overlay leaves no splice behind: the request is only
        // meant to survive from the bar's press to the card's pick.
        if !open {
            self.insert_request = None;
        }
    }

    /// A card of the gallery was clicked: the node lands at the centre of the
    /// canvas and is selected. When the bar opened the gallery with `add
    /// before`/`add after`, the picked kind is spliced around the node
    /// instead — see [`insert_node`].
    pub(crate) fn add_from_gallery(&mut self, door: &dyn Call, selection: &mut Selection, kind: &str, theme: &Theme) {
        if let Some((file_kind, path)) = crate::shell::favorites::file_of_key(kind) {
            add_file_from_gallery(door, self, selection, file_kind, path, theme);
            return;
        }
        if let Some(req) = self.insert_request.take() {
            insert_node(door, self, selection, kind, &req, theme);
            return;
        }
        add_node(door, self, selection, kind, theme);
    }
}

/// How many entries of an anchored menu carry the relevance star.
const TOP_STARRED: usize = 3;

/// The section a catalog entry answers to, read from its own `family` field
/// (the same field `category_of` groups by; a family with no section is
/// `Section::Other`).
fn section_of(entry: &Value) -> Section {
    // A `mat.*` kind is a material whatever family the catalogue files it
    // under, so the section appears the moment the first one is catalogued.
    if entry["kind"].as_str().is_some_and(|k| k.starts_with("mat.")) {
        return Section::Materials;
    }
    let family = entry["family"].as_str().unwrap_or("other");
    section_for_family(family)
}

/// The section a family answers to: [`category_of`]'s five, plus
/// [`Section::Other`] for the leftover list.
fn section_for_family(family: &str) -> Section {
    match category_of(family) {
        Some("sources") => Section::Sources,
        Some("sensors") => Section::Sensors,
        Some("effects") => Section::Effects,
        Some("materials") => Section::Materials,
        Some("math") => Section::Math,
        Some("scripts") => Section::Scripts,
        Some("outputs") => Section::Outputs,
        _ => Section::Other,
    }
}

/// A section's glyph in the gallery's left column.
fn section_icon(section: Section) -> crate::shell::components::Icon {
    use crate::shell::components::Icon;
    match section {
        Section::Sources => Icon::Source,
        Section::Sensors => Icon::Sensor,
        Section::Effects => Icon::Effect,
        Section::Materials => Icon::Material,
        Section::Math => Icon::Math,
        Section::Scripts => Icon::Script,
        Section::Outputs => Icon::Output,
        Section::Other => Icon::Other,
    }
}

/// A section's name in the gallery's left column.
fn section_title(section: Section) -> &'static str {
    match section {
        Section::Sources => tr_key("sources"),
        Section::Sensors => tr_key("sensors"),
        Section::Effects => tr_key("effects"),
        Section::Materials => tr_key("materials"),
        Section::Math => tr_key("math"),
        Section::Scripts => tr_key("scripts"),
        Section::Outputs => tr_key("outputs"),
        Section::Other => tr_key("other"),
    }
}

/// The open graph's domain, as the cached `GET /graph` reply spells it —
/// `"live"`/`"offline"`, the two words `metal_graph::Domain` serializes to.
/// `None` only before a graph reply has landed (or one that predates the
/// field): the menu then asks for the whole catalog rather than guessing a
/// domain to filter by.
fn open_domain(state: &PanelState) -> Option<&str> {
    state.graph.as_ref()?["graph"]["domain"].as_str()
}

/// Whether a catalog entry may be offered in a graph of `domain` — the
/// panel's reading of the same two fields `GET /catalog?domain=` filters by:
/// `menu` (in the menu at all) and `domains` (names the domain). A reply
/// missing either field — an older server, a test's hand-written catalog —
/// is offered: unknown means "no rule to apply", never "hidden".
fn kind_in_domain(entry: &Value, domain: &str) -> bool {
    let offered = entry["menu"].as_bool().unwrap_or(true);
    let allowed = match entry["domains"].as_array() {
        Some(domains) if !domains.is_empty() => {
            domains.iter().any(|d| d.as_str() == Some(domain))
        }
        _ => true,
    };
    offered && allowed
}

/// Which of the five top sections a family answers to (21-Sep-2026): the
/// ask's own order — "first sources, sensors, effects, scripts,
/// outputs, then the list". `None` keeps a family in "the list" —
/// `router`/`camera_track`/`mask`/`compose`/`stack`, none of which an
/// operator reaches for by that kind of role the way they reach for
/// "wants a source".
fn category_of(family: &str) -> Option<&'static str> {
    match family {
        "audio" | "generator" | "reel" | "still" | "text" | "capture" => Some("sources"),
        "sensor" => Some("sensors"),
        "effect" => Some("effects"),
        // The `mat.*` family (the material notes): 3D materials, carried
        // to a model by its surfaces. Not in the catalogue until that lands.
        "material" => Some("materials"),
        // The math family (`source.*`): waves, constants, random draws, noise.
        "math" => Some("math"),
        "script" => Some("scripts"),
        "output_device" | "output_signal" | "export_video" | "venue" => Some("outputs"),
        _ => None,
    }
}

/// [`category_of`]'s five, in the order the menu paints them.
#[cfg_attr(not(test), allow(dead_code))]
const CATEGORIES: [&str; 7] = ["sources", "sensors", "effects", "materials", "math", "scripts", "outputs"];

/// One `+ node` menu per top section (22-Sep-2026, afternoon): the ask split
/// the single `+ node ▾` into one dropdown per section — `+ source ▾`,
/// `+ sensor ▾`, `+ effect ▾`, `+ script ▾`, `+ output ▾` — plus `+ other ▾`
/// for the families with no section, which used to be the flat tail of the
/// single menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Sources,
    Sensors,
    Effects,
    Materials,
    Math,
    Scripts,
    Outputs,
    Other,
}

/// Every section, in the bar's order — the six menus `canvas_bar` paints.
const SECTIONS: [Section; 8] = [
    Section::Sources,
    Section::Sensors,
    Section::Effects,
    Section::Materials,
    Section::Math,
    Section::Scripts,
    Section::Outputs,
    Section::Other,
];

/// Whether `kind` has its own preview on a gallery card: the fx and material
/// kinds, drawn over a test source (`GET /author/render/kind.jpg`).
fn has_kind_preview(kind: &str) -> bool {
    kind.starts_with("fx.") || kind.starts_with("mat.")
}

/// The card pictures of `kind`, one per variant: a `gen.<model>` kind's cells
/// of the grid each variant landed in (the crop follows the layout `GET
/// /author/render/grid.jpg` was asked for, [`TILE_COLS`] columns, one cell per
/// model), or an fx/mat kind's own frames. Empty for any other family, and a
/// variant still pending is `None` (a placeholder).
fn tiles_for_kind(state: &PanelState, kind: &str) -> crate::shell::gallery::Tiles {
    if let Some(model) = kind.strip_prefix("gen.") {
        let Some(index) = state.model_names.iter().position(|n| n == model) else {
            return Default::default();
        };
        let uv = tile_uv(index, state.model_names.len());
        return std::array::from_fn(|k| {
            state.gallery_tiles[k].current_picture().map(|(tex, size)| (tex, size, uv))
        });
    }
    if !has_kind_preview(kind) {
        return Default::default();
    }
    let whole = EguiRect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    std::array::from_fn(|k| {
        let frame = state.kind_tiles.get(&(kind.to_string(), k))?;
        frame.current_picture().map(|(tex, size)| (tex, size, whole))
    })
}

/// How many columns the grid is asked for: with 256 px tiles that keeps
/// the texture inside egui's 2048 px side limit up to 8 columns; four
/// keeps the rows short enough that the texture stays a few hundred
/// pixels tall for the model counts the registry has.
pub(crate) const TILE_COLS: usize = 4;
/// The node tiles' cadence while the floating preview is on screen: the
/// preview is the picture being watched, the tiles are thumbnails.
const TILES_BEHIND_PREVIEW: std::time::Duration = std::time::Duration::from_secs(1);
/// The node tiles' cadence with no preview on screen: 8 Hz. The grid renders
/// every model (~15 ms of GPU time per call), so tying it to the frame rate
/// starved the whole UI; thumbnails at this rate read as slightly choppy
/// motion, which is the trade for a steady frame rate.
const TILES_CADENCE: std::time::Duration = std::time::Duration::from_millis(125);
/// The float variants a gallery card cycles through: the server's own count
/// (`call::author::render::VARIANTS`), kept here because a panel may not
/// name a `call` submodule (the dependency gate).
pub(crate) const GALLERY_VARIANTS: usize = 4;
/// The gallery's fetch slot: while the gallery is open and a card is on
/// screen, one variant's grid is requested per slot, so each variant is
/// refreshed once every [`GALLERY_VARIANTS`] slots.
const GALLERY_CADENCE: std::time::Duration = std::time::Duration::from_millis(250);

/// The variant whose grid is requested at `time` (seconds), or `None` when
/// nothing is fetched: the gallery is closed, or no card is on screen (the
/// fetch follows the scroll). Round-robin over the variants, one per slot.
pub(crate) fn variant_to_fetch(time: f64, open: bool, visible: bool) -> Option<usize> {
    if !open || !visible {
        return None;
    }
    let slot = (time.max(0.0) / GALLERY_CADENCE.as_secs_f64()) as usize;
    Some(slot % GALLERY_VARIANTS)
}

/// The `(kind, variant)` a gallery card's preview fetches this slot: the
/// visible fx/mat kinds' variants still without a picture, one per slot,
/// round-robin. `None` when every visible one has landed.
pub(crate) fn kind_to_fetch(time: f64, kinds: &[String], landed: impl Fn(&str, usize) -> bool) -> Option<(String, usize)> {
    let pending: Vec<(String, usize)> = kinds
        .iter()
        .filter(|k| has_kind_preview(k))
        .flat_map(|k| (0..GALLERY_VARIANTS).filter(|v| !landed(k, *v)).map(|v| (k.clone(), v)).collect::<Vec<_>>())
        .collect();
    if pending.is_empty() {
        return None;
    }
    let slot = (time.max(0.0) / GALLERY_CADENCE.as_secs_f64()) as usize;
    pending.get(slot % pending.len()).cloned()
}

/// How long a kind's picture is kept before it is asked for again: its
/// clock is fixed, so it only changes when the code does.
const KIND_CADENCE: std::time::Duration = std::time::Duration::from_secs(30);


/// One tile's size as the server draws it (the 16:9 the studio kept).
pub(crate) const TILE_W: u32 = 256;
pub(crate) const TILE_H: u32 = 144;
/// One kind preview's size on a card (its strip cell, a little larger).
pub(crate) const KIND_W: u32 = 192;
pub(crate) const KIND_H: u32 = 108;

/// The `uv` rectangle of cell `index` in a grid of `TILE_COLS` columns and
/// `ceil(n / TILE_COLS)` rows — the same arithmetic `Renderer::grid` laid
/// the image out with.
pub(crate) fn tile_uv(index: usize, n: usize) -> EguiRect {
    let cols = TILE_COLS.min(n.max(1));
    let rows = n.max(1).div_ceil(cols);
    let (c, r) = (index % cols, index / cols);
    let (cw, rh) = (1.0 / cols as f32, 1.0 / rows as f32);
    EguiRect::from_min_max(
        Pos2::new(c as f32 * cw, r as f32 * rh),
        Pos2::new((c + 1) as f32 * cw, (r + 1) as f32 * rh),
    )
}

/// `POST /graph/node { op: "add" }` — the one path to adding a node
/// (the old page's `addGraphNode`, deleted 15-Sep-2026, S.U18). The reply is
/// already the same `graph`/`primitives`/`issues` shape `GET /graph`
/// answers (`crate::call::graph::graph_node` -> `crate::graph_reply`), plus
/// a `node` field naming the new id — so this caches it directly instead of
/// issuing a second `GET /graph`.
fn add_node(
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    kind: &str,
    theme: &Theme,
) {
    if state.selected_graph.is_none() {
        return;
    }
    // Centre of the canvas as it is on screen now; before the canvas has
    // ever been measured, the grid cell the first nodes always took.
    let pos = match state.last_canvas_rect {
        Some(canvas) => new_node_spot(canvas, state.pan, state.zoom),
        None => {
            let n = state
                .graph
                .as_ref()
                .and_then(|g| g["graph"]["nodes"].as_array())
                .map(Vec::len)
                .unwrap_or(0);
            [(n % 6) as f32 * 200.0, (n / 6) as f32 * 120.0]
        }
    };
    add_node_at(door, state, selection, kind, pos, theme);
}

/// [`add_node`] at a graph position — the drop of a gallery card puts the
/// node where the pointer was released.
fn add_node_at(
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    kind: &str,
    pos: [f32; 2],
    theme: &Theme,
) {
    let Some(gid) = state.selected_graph.clone() else {
        return;
    };
    let body = json!({ "op": "add", "graph": gid, "kind": kind, "pos": json!(pos) });
    if let Ok(reply) = door.json("POST", "/graph/node", Some(body)) {
        if let Some(id) = reply["node"].as_str().map(str::to_string) {
            *selection = Selection::Node(id.clone());
            push_undo(state, Undo::Remove { node: id }, theme);
        }
        state.graph = Some(reply);
    }
}

/// The bar's `add before`/`add after`: the picked kind lands spliced into the
/// chain through the same `POST /graph/node { op: "insert" }` route the
/// properties panel's `+ effect` uses. The route takes the node the new one
/// *follows* — for `after` that is the anchor itself; for `before` it is the
/// node feeding the anchor's `image` input, so the new node slots between
/// them. When there is nothing to splice into (a source with no image input,
/// or a kind the door will not insert), the node is still added, at the
/// canvas's centre, exactly as a plain pick would leave it.
fn insert_node(
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    kind: &str,
    req: &InsertRequest,
    theme: &Theme,
) {
    let Some(gid) = state.selected_graph.clone() else {
        return;
    };
    let splice_after = if req.after {
        Some(req.node.clone())
    } else {
        image_source(state, &req.node)
    };
    let Some(after) = splice_after else {
        add_node(door, state, selection, kind, theme);
        return;
    };
    let body = json!({ "op": "insert", "graph": gid, "after": after, "kind": kind });
    match door.json("POST", "/graph/node", Some(body)) {
        Ok(reply) => {
            if let Some(id) = reply["node"].as_str().map(str::to_string) {
                *selection = Selection::Node(id.clone());
                push_undo(state, Undo::Remove { node: id }, theme);
            }
            state.graph = Some(reply);
        }
        // The door refused the splice (not an image-to-image kind, an anchor
        // with no image output): fall back to the free-floating add rather
        // than leaving the operator with nothing.
        Err(_) => add_node(door, state, selection, kind, theme),
    }
}

/// The source node a library file of `file_kind` becomes (the same mapping a
/// drop of that file on the canvas makes), with its config.
fn source_for_file(door: &dyn Call, file_kind: &str, path: &str) -> Option<(&'static str, Value)> {
    match file_kind {
        "folder" => batch_from_dir(door, std::path::Path::new(path)),
        "image" => Some(("still", json!({ "file": path }))),
        "video" => Some(("reel.clip", json!({ "file": path }))),
        "audio" => Some(("audio.file", json!({ "file": path }))),
        _ => None,
    }
}

/// A library file or folder picked from the gallery: its source node lands at
/// the centre of the canvas, as an added kind does.
fn add_file_from_gallery(door: &dyn Call, state: &mut PanelState, selection: &mut Selection, file_kind: &str, path: &str, theme: &Theme) {
    let Some((kind, config)) = source_for_file(door, file_kind, path) else {
        return;
    };
    let pos = match state.last_canvas_rect {
        Some(canvas) => new_node_spot(canvas, state.pan, state.zoom),
        None => [0.0, 0.0],
    };
    let _ = create_source_at(door, state, selection, kind, pos, config, theme);
}

/// A card dragged out of the "new" gallery and released over the canvas
/// becomes that node at the pointer.
fn drop_node_on_canvas(
    ui: &Ui,
    response: &Response,
    rect: EguiRect,
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    theme: &Theme,
) {
    let Some(released) = response.dnd_release_payload::<gallery::NodeDrag>() else {
        return;
    };
    let at = ui.input(|i| i.pointer.interact_pos()).unwrap_or(rect.center());
    let pos = to_graph(rect.min, state.pan, state.zoom, at);
    add_node_at(door, state, selection, &released.item.kind, pos, theme);
}

// ---------------------------------------------------------------------------
// the canvas's control bar
// ---------------------------------------------------------------------------

/// 22-Sep-2026, afternoon: the nodes canvas as a drop target for a `files`
/// card. An image becomes a `still`, a video a `reel`, an audio an
/// `audio.file` — the three file-source kinds — added at the drop point and
/// given the card's path as config (`POST /graph/node {op:"add"}` then
/// `{op:"config", config:{file}}`, the same two routes the properties editor
/// already writes config through). A sequence or anything else is not a graph
/// source: it is left for the overlays grid / lanes to claim.
///
/// While the card hovers the canvas a label says what dropping it would make
/// (or nothing when it would make nothing), so the operator is not guessing
/// what a drag over the graph means.
fn drop_file_on_canvas(
    ui: &Ui,
    response: &Response,
    rect: EguiRect,
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    theme: &Theme,
) {
    // The hint is a hover-only nicety, but the release is not: egui clears
    // the payload on the release frame *before* widgets run, so the hover
    // query is `None` exactly when the drop needs handling. The two are read
    // separately — never gate the release behind the hover.
    if let Some(drag) = response.dnd_hover_payload::<crate::shell::panels::FileDrag>() {
        if drag.source_kind().is_some() {
            if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                ui.painter_at(rect).text(
                    p + Vec2::splat(theme.item_gap.x),
                    Align2::LEFT_TOP,
                    trf("create {kind} · {name}", &[("kind", &drag.source_kind().unwrap_or_default()), ("name", &drag.name)]),
                    theme.font_small.clone(),
                    theme.phosphor,
                );
            }
        }
    }
    let Some(released) = response.dnd_release_payload::<crate::shell::panels::FileDrag>() else {
        return;
    };
    let Some(kind) = released.source_kind() else {
        return;
    };
    let pos_screen = ui
        .input(|i| i.pointer.interact_pos())
        .unwrap_or(rect.center());
    let pos = to_graph(rect.min, state.pan, state.zoom, pos_screen);
    // A scene of a video carries its span: `reel.batch` and the reel read
    // `scene`/`start`/`end` beside `file`. A folder is a `reel.batch` over
    // the images and videos it holds.
    let (kind, config) = if released.kind == "folder" {
        match batch_from_dir(door, std::path::Path::new(&released.path)) {
            Some(built) => built,
            None => return,
        }
    } else {
        match &released.scene {
            Some((scene, start, end)) => (kind, json!({ "file": released.path, "scene": scene, "start": start, "end": end })),
            None => (kind, json!({ "file": released.path })),
        }
    };
    let _ = create_source_at(door, state, selection, kind, pos, config, theme);
}

/// The one write every input route shares (the library card's drop, an OS
/// file dropped on the canvas, a clipboard paste): add a source of `kind` at
/// the graph position `pos`, give it `config`, select it and record the
/// inverse edit. `Err` is the text a route that can speak (the clipboard)
/// shows; the drops ignore it and simply do nothing when no graph is open.
fn create_source_at(
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    kind: &str,
    pos: [f32; 2],
    config: Value,
    theme: &Theme,
) -> Result<(), String> {
    let Some(gid) = state.selected_graph.clone() else {
        return Err("open a graph to paste".to_string());
    };
    let (id, reply) = add_file_source(door, &gid, kind, pos, config)?;
    *selection = Selection::Node(id.clone());
    push_undo(state, Undo::Remove { node: id }, theme);
    state.graph = Some(reply);
    Ok(())
}

/// OS drag-and-drop onto the canvas (a file dropped from the desktop). Only
/// when the pointer is over the canvas — a drop aimed at the timeline or the
/// lanes is left for those panels. Each file becomes the same file-source
/// node the library card makes: by extension (image → `still`, video →
/// `reel.clip`, audio → `audio.file`, a scripting/text file → `text.source`
/// carrying its bytes), a folder → a `reel.batch` over the images and videos
/// `GET /fs` lists. The first lands where the pointer is, the rest on free
/// cells so they do not stack. While a drag hovers the canvas, a short hint
/// says what a drop there does.
fn os_drops_on_canvas(
    ui: &Ui,
    rect: EguiRect,
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    theme: &Theme,
) {
    let (dropped, hovered, pointer) = ui.input(|i| {
        (
            i.raw.dropped_files.clone(),
            i.raw.hovered_files.clone(),
            i.pointer.interact_pos(),
        )
    });
    let Some(pointer) = pointer else {
        return;
    };
    if !rect.contains(pointer) {
        return;
    }
    // The hover hint is a nicety over empty frames too, so it must not touch
    // the drop list at all.
    if !hovered.is_empty() {
        ui.painter_at(rect).text(
            pointer + Vec2::splat(theme.item_gap.x),
            Align2::LEFT_TOP,
            tr("drop to add sources"),
            theme.font_small.clone(),
            theme.phosphor,
        );
    }
    if dropped.is_empty() {
        return;
    }
    let mut first = true;
    for file in &dropped {
        let Some((kind, config)) = source_from_path(door, file.path()) else {
            continue;
        };
        let pos = if first {
            first = false;
            to_graph(rect.min, state.pan, state.zoom, pointer)
        } else {
            free_spot(state.graph.as_ref())
        };
        let _ = create_source_at(door, state, selection, kind, pos, config, theme);
    }
}

/// The key a node clip is written under on the system clipboard.
const NODE_CLIP_KEY: &str = "metalvj_nodes";

/// Ctrl+C: the selected node(s) — the multi-selection when there is one —
/// go to the system clipboard as one JSON clip of `kind`, `config` and a
/// position relative to the clip's top-left corner.
fn copy_selection(ctx: &egui::Context, state: &PanelState, selection: &Selection) {
    let mut ids = state.picked_nodes(selection);
    if ids.is_empty() {
        match selection {
            Selection::Node(id) => ids.push(id.clone()),
            _ => return,
        }
    }
    let records: Vec<(Value, Value, [f32; 2])> = ids
        .iter()
        .filter_map(|id| {
            let node = node_record(state, id)?;
            Some((node["kind"].clone(), node["config"].clone(), find_node_pos(state, id)?))
        })
        .collect();
    if records.is_empty() {
        return;
    }
    let min_x = records.iter().map(|r| r.2[0]).fold(f32::INFINITY, f32::min);
    let min_y = records.iter().map(|r| r.2[1]).fold(f32::INFINITY, f32::min);
    let nodes: Vec<Value> = records
        .iter()
        .map(|(kind, config, pos)| json!({ "kind": kind, "config": config, "pos": [pos[0] - min_x, pos[1] - min_y] }))
        .collect();
    ctx.copy_text(json!({ NODE_CLIP_KEY: nodes }).to_string());
}

/// The nodes of a clip [`copy_selection`] wrote, when `text` is one.
fn parse_node_clip(text: &str) -> Option<Vec<Value>> {
    let clip: Value = serde_json::from_str(text.trim()).ok()?;
    clip.get(NODE_CLIP_KEY)?.as_array().cloned()
}

/// Pastes a node clip at the canvas centre, each node added and given its
/// config (one undo step each). The pasted nodes become the selection.
fn paste_nodes(
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    clip: &[Value],
    theme: &Theme,
) -> Result<(), String> {
    let Some(gid) = state.selected_graph.clone() else {
        return Err("open a graph to paste".to_string());
    };
    let anchor = match state.last_canvas_rect {
        Some(rect) => new_node_spot(rect, state.pan, state.zoom),
        None => free_spot(state.graph.as_ref()),
    };
    let mut ids = Vec::new();
    for node in clip {
        let Some(kind) = node["kind"].as_str().filter(|k| !k.is_empty()) else {
            continue;
        };
        let rel = [node["pos"][0].as_f64().unwrap_or(0.0) as f32, node["pos"][1].as_f64().unwrap_or(0.0) as f32];
        let pos = [anchor[0] + rel[0], anchor[1] + rel[1]];
        let body = json!({ "op": "add", "graph": gid, "kind": kind, "pos": [pos[0], pos[1]] });
        let Ok(reply) = door.json("POST", "/graph/node", Some(body)) else {
            continue;
        };
        let Some(id) = reply["node"].as_str().map(str::to_string) else {
            continue;
        };
        let mut reply = reply;
        if node["config"].is_object() {
            let patch = json!({ "op": "config", "graph": gid, "node": id, "config": node["config"] });
            reply = door.json("POST", "/graph/node", Some(patch)).unwrap_or(reply);
        }
        push_undo(state, Undo::Remove { node: id.clone() }, theme);
        state.graph = Some(reply);
        ids.push(id);
    }
    let Some(last) = ids.last().cloned() else {
        return Err("nothing to paste".to_string());
    };
    *selection = Selection::Node(last);
    state.picked = if ids.len() >= 2 { ids } else { Vec::new() };
    Ok(())
}

/// Clipboard text (Ctrl+V) into the canvas. A node clip pastes its nodes
/// (see [`paste_nodes`]). A single-line text that names an
/// existing file, or a `file://` URL, pastes as that file's source; anything
/// else pastes as a `text.source` carrying the text verbatim. `Err` is the
/// Spanish note the shell shows on the library's line.
pub(crate) fn paste_into_canvas(
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    text: &str,
    theme: &Theme,
) -> Result<(), String> {
    if state.selected_graph.is_none() {
        return Err("open a graph to paste".to_string());
    }
    if let Some(clip) = parse_node_clip(text) {
        return paste_nodes(door, state, selection, &clip, theme);
    }
    let trimmed = text.trim();
    let path_source = (!trimmed.contains('\n'))
        .then(|| std::path::Path::new(trimmed.strip_prefix("file://").unwrap_or(trimmed)))
        .filter(|p| trimmed.starts_with("file://") || p.is_file())
        .and_then(|p| source_from_path(door, p));
    let (kind, config) = match path_source {
        Some(pair) => pair,
        None => ("text.source", json!({ "content": text })),
    };
    let pos = match state.last_canvas_rect {
        Some(rect) => new_node_spot(rect, state.pan, state.zoom),
        None => free_spot(state.graph.as_ref()),
    };
    create_source_at(door, state, selection, kind, pos, config, theme)
}

/// The closed extension sets `/fs` recognises, copied from
/// `call/transmit.rs`'s `fs_kind` plus the scripting/text set. A path's
/// extension alone decides its source kind — never a probe.
const FS_AUDIO: [&str; 7] = ["wav", "mp3", "flac", "ogg", "m4a", "opus", "aiff"];
const FS_VIDEO: [&str; 10] = [
    "mp4", "mov", "mkv", "avi", "webm", "m4v", "mpg", "mpeg", "wmv", "flv",
];
const FS_IMAGE: [&str; 6] = ["jpg", "jpeg", "png", "gif", "webp", "bmp"];
const FS_TEXT: [&str; 8] = ["txt", "md", "json", "csv", "rs", "py", "js", "log"];

/// What a path is, as a graph source — image → `still`, video → `reel.clip`,
/// audio → `audio.file`, a scripting/text file → `text.source` with its UTF-8
/// bytes, a folder → `reel.batch` over the images and videos `GET /fs` lists.
/// `None` for a folder with nothing playable in it or an unrecognised file.
pub(crate) fn source_from_path(door: &dyn Call, path: &std::path::Path) -> Option<(&'static str, Value)> {
    if path.is_dir() {
        return batch_from_dir(door, path);
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let file = path.to_string_lossy().into_owned();
    if FS_IMAGE.contains(&ext.as_str()) {
        Some(("still", json!({ "file": file })))
    } else if FS_VIDEO.contains(&ext.as_str()) {
        Some(("reel.clip", json!({ "file": file })))
    } else if FS_AUDIO.contains(&ext.as_str()) {
        Some(("audio.file", json!({ "file": file })))
    } else if FS_TEXT.contains(&ext.as_str()) {
        let content = String::from_utf8(std::fs::read(path).ok()?).ok()?;
        Some(("text.source", json!({ "content": content })))
    } else {
        None
    }
}

/// A folder as a `reel.batch`: one `GET /fs` listing, its images and videos
/// kept as `{ kind, path }` items — exactly the config the library's own
/// `import` builds (`shell/mod.rs`). `None` when the folder has none.
fn batch_from_dir(door: &dyn Call, dir: &std::path::Path) -> Option<(&'static str, Value)> {
    let mut query = HashMap::new();
    query.insert("path".to_string(), dir.to_string_lossy().into_owned());
    let reply = door.call(
        "GET",
        "/fs",
        Params { query, body: Vec::new(), tail: None },
    )?;
    if reply.status != 200 {
        return None;
    }
    let listing: Value = crate::json_memo::parse(&reply.body).ok()?;
    let items: Vec<Value> = listing["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let kind = f["kind"].as_str()?;
            if !matches!(kind, "image" | "video") {
                return None;
            }
            let path = dir.join(f["file"].as_str()?);
            Some(json!({ "kind": kind, "path": path.to_string_lossy() }))
        })
        .collect();
    if items.is_empty() {
        None
    } else {
        Some((metal_graph::REEL_BATCH_KIND, json!({ "items": items })))
    }
}

/// The two calls a graph source takes — the canvas drop's and the library's
/// `import`: `POST /graph/node {op:"add"}` at `pos`, then `{op:"config"}`
/// with the file. A config the engine refuses leaves the bare node standing,
/// the reply of the add (the drop never lost its node that way).
pub(crate) fn add_file_source(
    door: &dyn Call,
    gid: &str,
    kind: &str,
    pos: [f32; 2],
    config: Value,
) -> Result<(String, Value), String> {
    let body = json!({ "op": "add", "graph": gid, "kind": kind, "pos": [pos[0], pos[1]] });
    let reply = door.json("POST", "/graph/node", Some(body)).map_err(|(_, e)| e)?;
    let id = reply["node"].as_str().map(str::to_string).ok_or_else(|| "no node id".to_string())?;
    let patch = json!({ "op": "config", "graph": gid, "node": id, "config": config });
    let reply = door.json("POST", "/graph/node", Some(patch)).unwrap_or(reply);
    Ok((id, reply))
}

/// The first grid cell (the same 200×120 grid `add_node` fills) with no node
/// within reach of it — a new source never lands on top of another node.
pub(crate) fn free_spot(graph: Option<&Value>) -> [f32; 2] {
    let taken: Vec<[f64; 2]> = graph
        .and_then(|g| g["graph"]["nodes"].as_array())
        .map(|ns| {
            ns.iter()
                .filter_map(|n| Some([n["pos"][0].as_f64()?, n["pos"][1].as_f64()?]))
                .collect()
        })
        .unwrap_or_default();
    let mut i = 0usize;
    loop {
        let pos = [(i % 6) as f32 * 200.0, (i / 6) as f32 * 120.0];
        let clear = taken
            .iter()
            .all(|t| (t[0] - f64::from(pos[0])).abs() >= 100.0 || (t[1] - f64::from(pos[1])).abs() >= 60.0);
        if clear {
            return pos;
        }
        i += 1;
    }
}

/// `import` (library, 2026-10-10): the file source `src_kind` (`still`,
/// `reel`, `audio.file`, or `reel.batch` for a folder) is added to the
/// selected graph at a free spot. Image, video and folder sources get an
/// `fx.forge` to their right, wired image-out → image-in, running the
/// passthrough preset, so a runtime can be attached at once. Selects the
/// source. `Err` is the text the library shows.
pub(crate) fn import_source(
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    src_kind: &str,
    config: Value,
) -> Result<(), String> {
    let Some(gid) = state.selected_graph.clone() else {
        return Err("open a graph to import".to_string());
    };
    let pos = free_spot(state.graph.as_ref());
    let (src, mut reply) = add_file_source(door, &gid, src_kind, pos, config)?;
    if src_kind != "audio.file" {
        // The grid is row-major, so with the source just placed the first
        // free cell is the one to its right.
        let forge_pos = free_spot(Some(&reply));
        let (fx, _) = add_file_source(
            door,
            &gid,
            "fx.forge",
            forge_pos,
            json!({ "shader": "presets/passthrough" }),
        )?;
        let edge = json!({
            "op": "add", "graph": gid,
            "from": { "node": src, "port": "image" },
            "to": { "node": fx, "port": "image" },
        });
        reply = door.json("POST", "/graph/edge", Some(edge)).map_err(|(_, e)| e)?;
    }
    *selection = Selection::Node(src.clone());
    state.graph = Some(reply);
    Ok(())
}

/// The floating bars over the canvas (2026-10-16), replacing the text bar that
/// sat above it:
///
/// - [`tool_bar`]: a vertical strip of glyph buttons at the canvas's left —
///   pause all, zoom in, zoom out, `new` (opens the gallery) and revert.
/// - [`focus_bar`]: a horizontal strip floating above the focused node —
///   the generator's `mode`, bypass, solo, duplicate, remove.
/// - [`hold_bars`]: the resting version over every *unfocused* node whose
///   bypass or solo is on, so the state reads without selecting the node.
///
/// They are `Area`s over the canvas, never laid out in it, so the canvas
/// keeps the whole panel.
fn float_bars(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    canvas: EguiRect,
) {
    tool_bar(ui, theme, door, state, selection, canvas);
    if let Selection::Node(id) = selection.clone() {
        focus_bar(ui, theme, door, state, selection, canvas, &id);
    }
    let focused = match &*selection {
        Selection::Node(id) => Some(id.clone()),
        _ => None,
    };
    hold_bars(ui, theme, door, state, canvas, focused.as_deref());
}

/// The vertical tool strip.
fn tool_bar(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    canvas: EguiRect,
) {
    // The label reflects `running` read live from `GET /state` — the same
    // read the spacebar's toggle makes. Nothing here caches it: the bar, the
    // spacebar and the engine can all freeze the clock between two frames.
    let running = door
        .json("GET", "/state", None)
        .ok()
        .and_then(|s| s["running"].as_bool())
        .unwrap_or(true);
    let edge = match &*selection {
        Selection::Edge(id) => Some(id.clone()),
        _ => None,
    };
    // Middle, not Foreground: a draggable overlay dragged across the bar
    // must cover it.
    egui::Area::new(Id::new("shell/region/nodes/tools"))
        .order(egui::Order::Middle)
        .fixed_pos(canvas.min + theme.item_gap)
        .constrain_to(canvas)
        .show(ui.ctx(), |ui| {
            theme.card_frame().inner_margin(theme.bar_margin()).show(ui, |ui| {
                ui.spacing_mut().item_spacing = theme.item_gap;
                ui.vertical(|ui| {
                    let (icon, tip) = if running {
                        (Icon::Pause, tr("pause all"))
                    } else {
                        (Icon::Play, tr("play all"))
                    };
                    if icon_button(ui, theme, icon, !running, tip).clicked() {
                        let _ = door.json("POST", "/transport", None);
                    }
                    if icon_button(ui, theme, Icon::ZoomIn, false, tr("zoom in")).clicked() {
                        let from_center = state.last_canvas_rect.map(|r| r.center() - r.min);
                        zoom_view(state, theme.graph_zoom_step, from_center, theme);
                    }
                    if icon_button(ui, theme, Icon::ZoomOut, false, tr("zoom out")).clicked() {
                        let from_center = state.last_canvas_rect.map(|r| r.center() - r.min);
                        zoom_view(state, theme.graph_zoom_step.recip(), from_center, theme);
                    }
                    if icon_button(ui, theme, Icon::New, false, tr("new")).clicked() {
                        state.new_requested = true;
                    }
                    if icon_button(ui, theme, Icon::Revert, false, tr("revert")).clicked() {
                        undo_last(door, state);
                    }
                    // A selected cable's one verb, in the strip like the rest.
                    if let (Some(id), Some(gid)) = (edge, state.selected_graph.clone()) {
                        if icon_button(ui, theme, Icon::Remove, false, tr("remove cable")).clicked() {
                            remove_edge(door, state, selection, &gid, &id, theme);
                        }
                    }
                });
            });
        });
}

/// Where a node's box sits on screen under the view transform, `None` when
/// the cached graph does not know the node.
fn node_screen_rect(state: &PanelState, canvas: EguiRect, id: &str) -> Option<EguiRect> {
    let pos = find_node_pos(state, id)?;
    let min = to_screen(canvas.min, state.pan, state.zoom, pos);
    let size = Vec2::new(metal_graph::layout::NODE_W, node_box_h(state, pos)) * state.zoom;
    Some(EguiRect::from_min_size(min, size))
}

/// The screen offset a node's box is painted at while it is dragged — the
/// same gesture preview [`canvas`] hands [`paint_primitive`] (graph delta ×
/// zoom), so the floating bar can follow its node instead of sitting at the
/// committed position until release.
fn drag_screen_offset(state: &PanelState, id: &str) -> Vec2 {
    match &state.drag {
        Some(d) if d.node == id => {
            Vec2::new(
                d.cur_pointer[0] - d.start_pointer[0],
                d.cur_pointer[1] - d.start_pointer[1],
            ) * state.zoom
        }
        _ => Vec2::ZERO,
    }
}

/// The horizontal strip floating above the focused node (below it when the
/// canvas has no room above): the generator's `mode` dropdown, then the
/// splice verbs (`add before` / `add after`), bypass, solo, duplicate and
/// remove as glyph buttons. Bypass and solo are booleans and light up while
/// on. The strip rides the node while it is dragged and is itself a drag
/// handle for the node — the same one-write-on-release `finish_drag` the
/// canvas gesture commits through.
fn focus_bar(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    canvas: EguiRect,
    id: &str,
) {
    let Some(gid) = state.selected_graph.clone() else {
        return;
    };
    let Some(node) = node_screen_rect(state, canvas, id) else {
        return;
    };
    // While this node is being dragged its box is painted at the gesture's
    // offset; the bar follows the same offset so it never detaches. The one
    // case the off-canvas early return must not cut short is an active drag
    // of this very node: dropping the bar then would strand `state.drag`
    // with no widget left to report its release.
    let dragging = state.drag.as_ref().is_some_and(|d| d.node == id);
    let node = node.translate(drag_screen_offset(state, id));
    if !canvas.intersects(node) && !dragging {
        return;
    }
    let bar_h = theme.icon_button_size.y + theme.bar_margin().sum().y as f32 + theme.item_gap.y;
    let (anchor, pivot) = if node.top() - canvas.top() > bar_h {
        (Pos2::new(node.center().x, node.top() - theme.item_gap.y), Align2::CENTER_BOTTOM)
    } else {
        (Pos2::new(node.center().x, node.bottom() + theme.item_gap.y), Align2::CENTER_TOP)
    };
    let kind = node_record(state, id)
        .and_then(|n| n["kind"].as_str().map(str::to_string))
        .unwrap_or_default();
    let bypass = node_flag(state, id, "bypass").unwrap_or(false);
    let solo = node_flag(state, id, "solo").unwrap_or(false);
    let area = egui::Area::new(Id::new(("shell/region/nodes/focus", id)))
        .order(egui::Order::Foreground)
        .pivot(pivot)
        .fixed_pos(anchor)
        .constrain_to(canvas)
        // The strip is a drag handle for the node: its own response senses a
        // drag (the buttons inside are added after, so a click on one still
        // resolves to that button — the same "most recently added wins" rule
        // `toolbar` documents).
        .sense(Sense::click_and_drag())
        .show(ui.ctx(), |ui| {
            theme.card_frame().inner_margin(theme.bar_margin()).show(ui, |ui| {
                ui.spacing_mut().item_spacing = theme.item_gap;
                ui.horizontal(|ui| {
                    // A `gen.*` model node's own `mode`: the three-way choice
                    // the properties page also paints, on the canvas where the
                    // node is. `ai` = fitted colourist, `ai_fx` = deterministic
                    // map, `manual` = the held row. One field, one route.
                    if kind.starts_with("gen.") {
                        let mut mode = match node_record(state, id)
                            .and_then(|n| n["config"]["mode"].as_str().map(str::to_string))
                            .as_deref()
                        {
                            Some("ai") => 0,
                            Some("manual") => 2,
                            _ => 1,
                        };
                        let picked = mode;
                        let options = [(0usize, tr("AI")), (1, tr("AI FX")), (2, tr("manual"))];
                        dropdown(ui, theme, ("nodes/mode", id), &mut mode, &options);
                        if mode != picked {
                            let value = match mode {
                                0 => "ai",
                                2 => "manual",
                                _ => "ai_fx",
                            };
                            set_node_mode(door, state, &gid, id, value);
                        }
                    }
                    // Splice verbs: each opens the "new" gallery carrying
                    // where the picked kind goes, so a card lands wired into
                    // the chain rather than floating free (`insert_request`,
                    // `add_from_gallery`).
                    if icon_button(ui, theme, Icon::InsertBefore, false, tr("add node before")).clicked() {
                        state.insert_request = Some(InsertRequest { node: id.to_string(), after: false });
                        state.new_requested = true;
                    }
                    if icon_button(ui, theme, Icon::InsertAfter, false, tr("add node after")).clicked() {
                        state.insert_request = Some(InsertRequest { node: id.to_string(), after: true });
                        state.new_requested = true;
                    }
                    if icon_button(ui, theme, Icon::Bypass, bypass, tr("bypass")).clicked() {
                        flag_node(door, state, &gid, id, "bypass", theme);
                    }
                    if icon_button(ui, theme, Icon::Solo, solo, tr("solo")).clicked() {
                        flag_node(door, state, &gid, id, "solo", theme);
                    }
                    if icon_button(ui, theme, Icon::Duplicate, false, tr("duplicate")).clicked() {
                        duplicate_node(door, state, selection, &gid, id, theme);
                    }
                    // The 3D scene is the surface's own (`venue_map`): the gear
                    // sits here, on the node the picture is warped onto, and
                    // flips the canvas into the venue scene of that node's
                    // room (`venue_canvas`). The fixtures that feed it are a
                    // separate node kind with their own picker, wired *before*
                    // this one.
                    if kind == metal_graph::VENUE_SURFACE_KIND
                        && icon_button(ui, theme, Icon::Settings, false, tr("paint the room")).clicked()
                    {
                        state.venue_requested = Some((gid.clone(), id.to_string()));
                    }
                    if icon_button(ui, theme, Icon::Remove, false, tr("remove")).clicked() {
                        remove_node(door, state, selection, &gid, id, theme);
                    }
                });
            });
        });
    // Drive the node drag from the strip. The press landed on a foreground
    // area, so the canvas's own `handle_pointer` never sees it; the drag is
    // started and carried here, and released through the same `finish_drag`
    // (one `POST /graph/node { op: "move" }`, never a stream).
    let bar = area.response;
    if bar.drag_started() {
        if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
            let gp = to_graph(canvas.min, state.pan, state.zoom, p);
            let start_node_pos = find_node_pos(state, id).unwrap_or(gp);
            *selection = Selection::Node(id.to_string());
            state.drag = Some(Drag {
                node: id.to_string(),
                start_node_pos,
                start_pointer: gp,
                cur_pointer: gp,
            });
        }
    }
    if bar.dragged() {
        if let (Some(drag), Some(p)) = (state.drag.as_mut(), ui.input(|i| i.pointer.interact_pos())) {
            drag.cur_pointer = to_graph(canvas.min, state.pan, state.zoom, p);
        }
    }
    if bar.drag_stopped() {
        finish_drag(door, state, selection, theme);
    }
}

/// The resting bar: over every node that is *not* focused and has bypass or
/// solo on, a small strip of just those lit glyphs. A click on one turns it
/// off, so a forgotten solo is never more than a glance and a click away.
fn hold_bars(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    canvas: EguiRect,
    focused: Option<&str>,
) {
    let Some(gid) = state.selected_graph.clone() else {
        return;
    };
    let flagged: Vec<(String, bool, bool)> = state
        .graph
        .as_ref()
        .and_then(|g| g["graph"]["nodes"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let id = n["id"].as_str()?;
            let (bypass, solo) = (n["bypass"].as_bool()?, n["solo"].as_bool()?);
            ((bypass || solo) && Some(id) != focused).then(|| (id.to_string(), bypass, solo))
        })
        .collect();
    for (id, bypass, solo) in flagged {
        let Some(node) = node_screen_rect(state, canvas, &id) else {
            continue;
        };
        if !canvas.intersects(node) {
            continue;
        }
        let hold_h = theme.icon_button_size.y + theme.hold_bar_margin().sum().y as f32 + theme.item_gap.y;
        let (anchor, pivot) = if node.top() - canvas.top() > hold_h {
            (Pos2::new(node.left(), node.top() - theme.item_gap.y), Align2::LEFT_BOTTOM)
        } else {
            (Pos2::new(node.left(), node.bottom() + theme.item_gap.y), Align2::LEFT_TOP)
        };
        egui::Area::new(Id::new(("shell/region/nodes/hold", &id)))
            .order(egui::Order::Middle)
            .pivot(pivot)
            .fixed_pos(anchor)
            .constrain_to(canvas)
            .show(ui.ctx(), |ui| {
                theme.card_frame().inner_margin(theme.hold_bar_margin()).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = theme.item_gap;
                    ui.horizontal(|ui| {
                        if bypass && icon_button(ui, theme, Icon::Bypass, true, tr("bypass")).clicked() {
                            flag_node(door, state, &gid, &id, "bypass", theme);
                        }
                        if solo && icon_button(ui, theme, Icon::Solo, true, tr("solo")).clicked() {
                            flag_node(door, state, &gid, &id, "solo", theme);
                        }
                    });
                });
            });
    }
}

/// Delete / Backspace on the canvas: the selected node or cable goes, through
/// the same routes (and the same undo) as the focus bar's remove.
fn delete_selection(door: &dyn Call, state: &mut PanelState, selection: &mut Selection, theme: &Theme) {
    let Some(gid) = state.selected_graph.clone() else {
        return;
    };
    let picked = state.picked_nodes(selection);
    if !picked.is_empty() {
        for id in &picked {
            remove_node(door, state, selection, &gid, id, theme);
        }
        state.picked.clear();
        return;
    }
    match selection.clone() {
        Selection::Node(id) => remove_node(door, state, selection, &gid, &id, theme),
        Selection::Edge(id) => remove_edge(door, state, selection, &gid, &id, theme),
        Selection::None | Selection::File(_) => {}
    }
}

/// One zoom step, about `from_center` — the canvas's own centre as an
/// offset from the canvas origin (a `Vec2`, so the algebra below is the
/// same one `to_screen` defines for a point: screen = origin + pan +
/// graph·zoom, and holding the point under the anchor put is pan' =
/// pan·f + anchor·(1−f)). `None` means no anchor yet — the bar can paint
/// before the canvas has ever measured; the clamp is where a runaway
/// double-click meets the floor/ceiling the theme picked.
fn zoom_view(state: &mut PanelState, factor: f32, from_center: Option<Vec2>, theme: &Theme) {
    let cur = state.zoom;
    let next = (cur * factor).clamp(theme.graph_zoom_min, theme.graph_zoom_max);
    let f = next / cur;
    if let Some(anchor) = from_center {
        state.pan = state.pan * f + anchor * (1.0 - f);
    }
    state.zoom = next;
}

/// The mouse wheel (and a trackpad pinch) zooms the canvas about the pointer,
/// through the same [`zoom_view`] the bar's buttons step — one notch is one
/// `graph_zoom_step`. The wheel is consumed while the pointer is over the
/// canvas, so the panel's scroll area doesn't also scroll under it.
fn wheel_zoom(ui: &Ui, theme: &Theme, state: &mut PanelState, rect: EguiRect, response: &egui::Response) {
    let Some(p) = response.hover_pos() else { return };
    let (wheel, pinch) = ui.input_mut(|i| {
        let wheel = i.smooth_scroll_delta.y;
        i.smooth_scroll_delta = Vec2::ZERO;
        (wheel, i.zoom_delta())
    });
    // egui hands the wheel over a few frames (smooth scrolling), so the zoom
    // is continuous: `WHEEL_NOTCH` points of wheel is one bar-button step.
    let factor = pinch * theme.graph_zoom_step.powf(wheel / WHEEL_NOTCH);
    if factor != 1.0 {
        zoom_view(state, factor, Some(p - rect.min), theme);
    }
}

/// Points of wheel scroll one zoom step takes — a mouse notch is about this.
const WHEEL_NOTCH: f32 = 50.0;

/// The master row's label for one screen: ceded wins over where it
/// is shown, which wins over "local" — mirrors the order the operator cares
/// about (a screen someone else is driving matters more than which box
/// composes it).
fn chip_label(id: &str, node: Option<&str>, ceded_to: Option<&str>) -> String {
    match (ceded_to, node) {
        (Some(to), _) => trf("{id} · handed to {to}", &[("id", &id), ("to", &to)]),
        (None, Some(n)) => trf("{id} · on {n}", &[("id", &id), ("n", &n)]),
        (None, None) => trf("{id} · local", &[("id", &id)]),
    }
}

/// O.19: one chip per `outputs[]`, showing where it shows and a button to
/// give it up or take it back — read fresh off `GET /state` every frame,
/// the same live-read `canvas_bar`'s own `running` toggle just above it
/// uses, for the same reason: two operators (or this bar and `lanes.rs`'s
/// own `outputs` row) can change this between any two frames.
fn master_row(ui: &mut Ui, theme: &Theme, door: &dyn Call) {
    let outputs: Vec<Value> = door
        .json("GET", "/state", None)
        .ok()
        .and_then(|s| s["outputs"].as_array().cloned())
        .unwrap_or_default();
    if outputs.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for o in &outputs {
            let Some(id) = o["id"].as_str() else { continue };
            let node = o["node"].as_str();
            let ceded = o["ceded_to"].as_str();
            ui.label(
                RichText::new(chip_label(id, node, ceded))
                    .font(theme.font_small.clone())
                    .color(theme.ink),
            );
            let verb = if ceded.is_some() { tr("take back") } else { tr("hand over") };
            if ui
                .button(RichText::new(verb).font(theme.font_small.clone()))
                .clicked()
            {
                let to = if ceded.is_some() {
                    Value::Null
                } else {
                    json!("otro programa")
                };
                let _ = door.json(
                    "POST",
                    "/outputs",
                    Some(json!({ "op": "cede", "id": id, "to": to })),
                );
            }
        }
    });
}

// ---------------------------------------------------------------------------
// the canvas
// ---------------------------------------------------------------------------

/// The reveal (plan "Surface sources"): reload the open graph from the door,
/// centre `node` on the canvas and start its pulse. A node the fresh graph
/// does not have yet is not pulsed (the reload is what makes it exist), and
/// the request is dropped rather than retried on every frame.
fn reveal_node(door: &dyn Call, state: &mut PanelState, rect: EguiRect, node: &str) {
    let Some(gid) = state.selected_graph.clone() else {
        return;
    };
    if let Some(v) = get(door, "GET", "/graph", &[("id", gid)]) {
        state.graph = Some(v);
    }
    let Some(at) = find_node_pos(state, node) else {
        return;
    };
    state.pan = pan_to_centre(rect.size(), at, state.zoom);
    state.pulse = Some((node.to_string(), web_time::Instant::now()));
}

fn canvas(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    fetcher: Option<&crate::shell::frames::Fetcher>,
    state: &mut PanelState,
    selection: &mut Selection,
    canvas_h: f32,
) {
    // The graph writer read the open graph back once its queued writes had
    // settled: take it in place of the copy this canvas drew optimistically.
    if let Some((gid, graph)) = door.graph_refresh() {
        if state.selected_graph.as_deref() == Some(gid.as_str()) {
            state.graph = Some(graph);
        }
    }
    // The properties panel wrote since the last read: re-read the graph so
    // picture_for/image_source see the edit.
    let wrote = super::properties::shared::writes();
    if wrote != state.seen_writes {
        state.seen_writes = wrote;
        if let Some(gid) = state.selected_graph.clone() {
            if let Some(v) = get(door, "GET", "/graph", &[("id", gid)]) {
                state.graph = Some(v);
            }
        }
    }
    // Edits still on their way to the engine (a colour pick, a properties
    // field) show over whatever graph the cache served, until it agrees.
    if let Some(graph) = state.graph.as_mut() {
        super::properties::shared::overlay_pending(graph);
    }
    super::properties::shared::repaint_while_pending(ui.ctx());
    // A stable id of its own, not egui's ambient auto-id counter: the
    // toolbar above (`toolbar`) paints a different number of widgets from
    // frame to frame (the note/head-note labels are conditional), which
    // would otherwise shift the canvas's auto id every frame it changed and
    // break the drag tracking below — `handle_pointer` reads
    // `response.is_pointer_button_down_on()` across frames, which only
    // means anything if the same widget answers it each time.
    let (rect, response) = ui
        .push_id(Id::new("shell/region/nodes/canvas"), |ui| {
            let desired = Vec2::new(ui.available_width(), canvas_h);
            ui.allocate_exact_size(desired, Sense::click_and_drag())
        })
        .inner;
    state.last_canvas_rect = Some(rect);
    // The venue scene: the same canvas, the same rect, the whole room instead
    // of the graph. It paints its own background, bar and gallery, and only
    // reports back here — the region, the focus and every hit-test rule stay
    // `Region::Nodes`'s.
    if state.venue_scene.is_some() {
        crate::shell::preview::set_canvas_rect(ui.ctx(), rect);
        crate::shell::preview::mark_canvas(ui.ctx());
        super::venue_canvas::paint(ui, theme, door, state, rect);
        return;
    }
    // TASK 1: publish the canvas rect so the biblioteca's file preview can
    // float over this canvas rather than inside its own panel (`preview.rs`'s
    // `set_canvas_rect`/`canvas_rect`, stamped with this frame so a stale rect
    // from a hidden canvas is ignored).
    crate::shell::preview::set_canvas_rect(ui.ctx(), rect);
    if let Some(node) = state.reveal.take() {
        reveal_node(door, state, rect, &node);
    }
    if state.pulsing().is_some() {
        ui.ctx().request_repaint();
    }
    wheel_zoom(ui, theme, state, rect, &response);
    // The cable under the pointer, while nothing is being dragged — it lights
    // exactly like the selected one, so a single cable can be singled out.
    state.hover_edge = match response.hover_pos() {
        Some(p) if state.drag.is_none() && state.cable.is_none() => {
            let at = to_graph(rect.min, state.pan, state.zoom, p);
            edge_near(state, at, EDGE_HOVER_PX / state.zoom.max(0.01))
        }
        _ => None,
    };
    let painter = ui.painter_at(rect);
    // The Wire ground: its own token, a step darker than the panel, so the
    // node cards read as raised on it (`nodes-reference.png`).
    painter.rect_filled(rect, theme.corner_radius(), theme.node_canvas);
    paint_grid(&painter, theme, rect, state);
    // N: poll `GET /graph/live` at 4–5 Hz while the canvas is on screen, cache
    // the last reply and paint live parameter values from it. The route is the
    // engine's last frame, so this is a poll of a cache the engine already
    // keeps — never a render request of its own.
    poll_live(door, state);
    // The model tiles, one texture for every `gen.*` box below. Ticked
    // only when the graph has a generator to show — an empty graph pumps
    // nothing.
    let tiles: Option<(egui::TextureId, [usize; 2])> = if state.model_names.is_empty() || !has_generator(state) {
        None
    } else {
        let query = [
            ("w", TILE_W.to_string()),
            ("h", TILE_H.to_string()),
            ("cols", TILE_COLS.to_string()),
            ("cells", state.model_names.len().to_string()),
        ];
        // 2026-10-06: the grid renders every model (~60 ms in release) on
        // the same worker and studio lock as the floating preview; at 15 Hz
        // it left the preview about one frame between grids. While the
        // preview paints, the tiles drop to [`TILES_BEHIND_PREVIEW`]; otherwise
        // they run at [`TILES_CADENCE`], not at the fps cap.
        let gap = if state.preview.rect().is_some() {
            TILES_BEHIND_PREVIEW
        } else {
            TILES_CADENCE
        };
        // G5: the grid is a texture on the painter's device when the window
        // shares it; the JPEG route answers otherwise.
        fetch_node_tiles(ui.ctx(), door, state, gap);
        state
            .tiles
            .picture(ui.ctx(), door, "/author/render/grid.jpg", &query, gap)
    };
    // The gallery's cards: while it is open, the grid cycles through the
    // four float variants, one per second, each fetched lazily on its own
    // (the cards show the one that landed last). Closed, nothing is fetched.
    let now = ui.ctx().input(|i| i.time);
    state.gallery_clock = now;
    if let Some(k) = variant_to_fetch(now, state.gallery_open, state.gallery_visible && !state.model_names.is_empty()) {
        let query = [
            ("w", TILE_W.to_string()),
            ("h", TILE_H.to_string()),
            ("cols", TILE_COLS.to_string()),
            ("cells", state.model_names.len().to_string()),
            ("variant", k.to_string()),
        ];
        state.gallery_tiles[k].picture(ui.ctx(), door, "/author/render/grid.jpg", &query, GALLERY_CADENCE);
    }
    // The fx/mat cards: one visible kind-variant without a picture per slot.
    if state.gallery_open && state.gallery_visible {
        let landed = |kind: &str, k: usize| {
            state.kind_tiles.get(&(kind.to_string(), k)).is_some_and(|f| f.current_picture().is_some())
        };
        if let Some((kind, k)) = kind_to_fetch(now, &state.gallery_kinds, landed) {
            let query = [
                ("kind", kind.clone()),
                ("variant", k.to_string()),
                ("w", KIND_W.to_string()),
                ("h", KIND_H.to_string()),
            ];
            state
                .kind_tiles
                .entry((kind, k))
                .or_default()
                .picture(ui.ctx(), door, "/author/render/kind.jpg", &query, KIND_CADENCE);
        }
    }
    if state.gallery_open && state.gallery_visible && !state.model_names.is_empty() {
        ui.ctx().request_repaint_after(GALLERY_CADENCE);
    }
    // A node drag in flight moves nothing on the door until release
    // (`finish_drag`'s own doc comment — one write, never a spam of
    // partial ones) but the operator needs to see it moving *now*, not
    // once it lands. `(start_node_pos, delta)` translates only the
    // primitives that belong to the dragged node — its own box, label,
    // ports, and the end of any cable that lands on one of those ports —
    // by the same graph-space delta `finish_drag` will eventually commit.
    // Not a second layout (rule 5 stays the door's): every primitive's
    // *position* is still exactly what the door resolved, this only
    // translates the ones that are the dragged node's, the same "gesture
    // preview, not layout" the cable's own ghost line already is.
    let drag_offset = state.drag.as_ref().map(|d| {
        (
            d.start_node_pos,
            [
                d.cur_pointer[0] - d.start_pointer[0],
                d.cur_pointer[1] - d.start_pointer[1],
            ],
            node_box_h(state, d.start_node_pos),
        )
    });

    // The selected node's id, so a cable touching it draws in the accent and
    // its box carries the selection outline (`nodes-reference.png`: the
    // selected node is outlined, its cables orange, the rest grey).
    let selected_edge = match &*selection {
        Selection::Edge(id) => Some(id.clone()),
        _ => None,
    };
    let selected_node: Option<&str> = match selection {
        Selection::Node(id) => Some(id.as_str()),
        _ => None,
    };
    // The cables lit on their own: the selected cable and the hovered one.
    let hover_edge = state.hover_edge.clone();
    let lit_edges: [Option<&str>; 2] = [selected_edge.as_deref(), hover_edge.as_deref()];

    // Cables first, then the node cards on top of them — Wire draws the wires
    // under the boxes so a cable disappears behind a node instead of over it.
    // Two passes over the same tagged list, so nothing is derived twice.
    match state.graph.clone() {
        Some(graph_val) => {
            if let Some(prims) = graph_val["primitives"].as_array() {
                for p in prims.iter().filter(|p| p["type"] == "path") {
                    paint_primitive(&painter, theme, rect, state, p, tiles.as_ref(), drag_offset, selected_node, lit_edges, live_of(state));
                }
                for p in prims.iter().filter(|p| p["type"] != "path" && !node_body_numeric(p)) {
                    paint_primitive(&painter, theme, rect, state, p, tiles.as_ref(), drag_offset, selected_node, lit_edges, live_of(state));
                }
            }
        }
        None => {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                tr("no graph"),
                theme.font_small.clone(),
                theme.ink_weak,
            );
        }
    }

    // F10: a `venue.fixture` node's card carries its fixture name, profile and
    // address and a live swatch off the last packet — painted after the
    // layout's own primitives, which still hold the geometry.
    paint_fixture_cards(&painter, theme, door, rect, state, drag_offset);

    // The stretched cable itself: from the press's port point to the
    // pointer's last position, a straight ghost line — a gesture preview,
    // not layout (rule 5's geometry stays the door's); committed cables
    // keep the layout's own curves.
    if let Some(cable) = &state.cable {
        painter.line_segment(
            [
                to_screen(rect.min, state.pan, state.zoom, cable.from_point),
                cable.cur_screen,
            ],
            theme.node_cable_stroke(true),
        );
    }

    // The variable chooser a multi-variable output drop opened (N) — a small
    // floating list under the pointer, one button per variable the producer
    // offers. Painted last so it sits over the canvas.
    paint_bind_menu(ui, &painter, theme, door, state, rect);

    // The colour picker a clicked node swatch opened (2026-10-08) — a source's
    // key/secondary colours set on the canvas itself.
    paint_colour_menu(ui, theme, door, state, rect);

    // `R22.1`, widened 2026-09-30: every focus change moves the picture. A
    // focused `gen.*` shows its own animation, the `output.device` its
    // composed frame, and any other node carrying an image (an `fx.*`, a
    // `mask`) its own isolated stage via `/node.jpg`. A node with no picture
    // anywhere upstream (audio, a lone script) paints and pumps nothing.
    crate::shell::preview::mark_canvas(ui.ctx());
    // 2026-10-01: the sequence editor's picture lands here too — one
    // preview, not a second one floating over the timeline. It wins while
    // the timeline is focused, or when the focus has no picture of its own.
    let sequence = crate::shell::preview::sequence(ui.ctx());
    let node_picture = selected_picture(state, selection);
    let sequence = sequence.filter(|seq| seq.focused || node_picture.is_none());
    let picture: Option<(String, Vec<(String, String)>)> = match &sequence {
        Some(seq) => Some((seq.route.clone(), seq.query.clone())),
        None => node_picture
            .map(|(r, q)| (r.to_string(), q.into_iter().map(|(k, v)| (k.to_string(), v)).collect())),
    };
    // A press that lands on the picture is the picture's: the widget's own
    // click toggles its size, and the graph underneath must not also run its
    // hit-test — an empty-space press clears the selection, which would hide
    // the very preview the click was aimed at before its `clicked()` fires.
    let on_preview = picture.is_some()
        && state
            .preview
            .hit_rect()
            .is_some_and(|r| ui.input(|i| i.pointer.interact_pos().is_some_and(|p| r.contains(p))));
    if !on_preview {
        handle_pointer(ui, &response, rect, door, state, selection, theme);
    }
    // 22-Sep-2026, afternoon: a card dropped from `files` becomes the
    // matching file-source node at the drop point — the same `POST /graph
    // {op:"add"}` path the catalog menu uses, with the card's path as config.
    drop_file_on_canvas(ui, &response, rect, door, state, selection, theme);
    // 2026-10-15: a file dropped from the OS becomes the same source node, at
    // the point it landed, when the pointer is over the canvas.
    os_drops_on_canvas(ui, rect, door, state, selection, theme);
    // A card dragged out of the "new" gallery lands as that node, where the
    // pointer was released.
    drop_node_on_canvas(ui, &response, rect, door, state, selection, theme);
    // 2026-10-09: an `fx.*` in focus compares — the picture feeding its
    // `image` input beside its own stage — and a grade asks its stage for a
    // histogram, which the grade page reads through egui memory.
    let before = match (&sequence, &*selection) {
        (None, Selection::Node(id)) if node_kind(state, id).is_some_and(|k| k.starts_with("fx.")) => {
            image_source(state, id).and_then(|up| picture_for(state, &up))
        }
        _ => None,
    };
    let grading = matches!(&*selection, Selection::Node(id) if node_kind(state, id) == Some("fx.grade"));
    state.preview.want_histogram(grading && sequence.is_none());
    // P-G: the bar names a focused node's picture, or the sequence's, and
    // exports it.
    let subject = match (&sequence, &*selection) {
        (Some(seq), _) => seq.subject.clone(),
        (None, Selection::Node(id)) => preview_subject(state, door, id),
        _ => None,
    };
    state.preview.set_subject(subject);
    let picture_shown = picture.is_some();
    if let Some((route, query)) = picture {
        let query: Vec<(&str, String)> = query.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        match &before {
            Some((broute, bquery)) => {
                let bquery: Vec<(&str, String)> = bquery.iter().map(|(k, v)| (*k, v.clone())).collect();
                let PanelState { preview, preview_before, .. } = state;
                preview.show_compare(
                    preview_before,
                    ui,
                    theme,
                    door,
                    fetcher,
                    rect,
                    (&route, &query),
                    (broute, &bquery),
                );
            }
            None => state.preview.show(ui, theme, door, fetcher, rect, &route, &query),
        }
        if let Some(h) = state.preview.histogram() {
            crate::shell::preview::publish_histogram(ui.ctx(), h);
        }
        if let (Some(seq), Some(r)) = (&sequence, state.preview.rect()) {
            if crate::shell::preview::play_control(ui, theme, r, seq.playing) {
                crate::shell::preview::request_play_toggle(ui.ctx());
            }
        }
    }
    // The floating bars come last, over the nodes — but not over the big
    // preview, which they have nothing to do with while it is up.
    if !(picture_shown && state.preview.big()) {
        float_bars(ui, theme, door, state, selection, rect);
    }
}

/// What the floating preview shows for the current focus: the route and its
/// query. `gen.*` → `/source.jpg` for that node; `output.*` → `/output.jpg`
/// for the graph's device; anything else with a `gen.*` up its `image`
/// wires → `/node.jpg`, that node's own isolated stage; `None` when nothing
/// upstream draws. Read off the cached graph reply
/// (rule 5: kinds and wires are the door's).
fn selected_picture(
    state: &PanelState,
    selection: &Selection,
) -> Option<(&'static str, Vec<(&'static str, String)>)> {
    let Selection::Node(id) = selection else {
        return None;
    };
    picture_for(state, id)
}

/// The floating preview bar's subject for node `id`: "name · kind" and the
/// device/node pair `POST /frame/export` draws.
fn preview_subject(state: &PanelState, door: &dyn Call, id: &str) -> Option<crate::shell::preview::PreviewSubject> {
    let node = state.graph.as_ref()?["graph"]["nodes"]
        .as_array()?
        .iter()
        .find(|n| n["id"].as_str() == Some(id))?;
    let kind = node["kind"].as_str().unwrap_or_default();
    let name = node["label"].as_str().filter(|s| !s.is_empty()).unwrap_or(id);
    let device = state_device(state, Some(id));
    // The device's own resolution (what the export writes), off `/state`.
    let size = crate::shell::preview::output_size(door, &device);
    Some(crate::shell::preview::PreviewSubject {
        label: crate::shell::i18n::trf("{name} · {kind}", &[("name", &name), ("kind", &kind)]),
        export: Some(serde_json::json!({ "device": device, "node": id })),
        size,
        fps: None,
    })
}

/// The screen node `id` of the open graph draws on (`shell::collection`).
fn state_device(state: &PanelState, node: Option<&str>) -> String {
    let gid = state.selected_graph.as_deref().unwrap_or_default();
    let reply = state.graph.clone().unwrap_or(Value::Null);
    crate::shell::collection::device_of(gid, &reply, node).unwrap_or_else(|| gid.to_string())
}

/// The kind of node `id` in the open graph.
fn node_kind<'a>(state: &'a PanelState, id: &str) -> Option<&'a str> {
    state.graph.as_ref()?["graph"]["nodes"]
        .as_array()?
        .iter()
        .find(|n| n["id"].as_str() == Some(id))
        .and_then(|n| n["kind"].as_str())
}

/// The node wired into `id`'s `image` input, if any.
fn image_source(state: &PanelState, id: &str) -> Option<String> {
    state.graph.as_ref()?["graph"]["edges"]
        .as_array()?
        .iter()
        .find(|e| e["to"]["node"].as_str() == Some(id) && e["to"]["port"].as_str() == Some("image"))
        .and_then(|e| e["from"]["node"].as_str())
        .map(str::to_string)
}

/// [`selected_picture`] for any node `id`.
fn picture_for(state: &PanelState, id: &String) -> Option<(&'static str, Vec<(&'static str, String)>)> {
    let graph = state.graph.as_ref()?.get("graph")?;
    let nodes = graph.get("nodes")?.as_array()?;
    let kind_of = |id: &str| {
        nodes
            .iter()
            .find(|n| n["id"].as_str() == Some(id))
            .and_then(|n| n["kind"].as_str())
    };
    let device = state_device(state, Some(id));
    let gid = state.selected_graph.clone().unwrap_or_default();
    if kind_of(id)?.starts_with("output.") {
        return Some(("/output.jpg", vec![("device", device)]));
    }
    let edges = graph.get("edges").and_then(Value::as_array);
    // Breadth-first up the image wires, so the closest generator wins.
    let mut queue = std::collections::VecDeque::from([id.clone()]);
    let mut seen = std::collections::HashSet::new();
    while let Some(cur) = queue.pop_front() {
        if !seen.insert(cur.clone()) {
            continue;
        }
        // 2026-10-01: a `capture` draws on its own (camera/screen/window),
        // so it roots a picture the way a generator does — its stage, or
        // the stage of whatever downstream node is focused.
        if kind_of(&cur) == Some("capture") {
            return Some(("/node.jpg", vec![("device", device), ("graph", gid.clone()), ("node", id.clone())]));
        }
        if kind_of(&cur).is_some_and(|k| k.starts_with("gen.")) {
            // The focused node itself is the generator: its own animation.
            // Anything downstream of one (an `fx.*`, a `mask`): that node's
            // isolated stage, `GET /node.jpg` — the picture right after it.
            return Some(if cur == *id {
                ("/source.jpg", vec![("device", device), ("graph", gid.clone()), ("layer", cur)])
            } else {
                ("/node.jpg", vec![("device", device), ("graph", gid.clone()), ("node", id.clone())])
            });
        }
        for e in edges.into_iter().flatten() {
            if e["to"]["node"].as_str() == Some(cur.as_str())
                && e["to"]["port"].as_str() == Some("image")
            {
                if let Some(from) = e["from"]["node"].as_str() {
                    queue.push_back(from.to_string());
                }
            }
        }
    }
    None
}

/// Does the open graph have a `gen.*` node at all — the one case the tile
/// grid is worth pumping for.
fn has_generator(state: &PanelState) -> bool {
    state
        .graph
        .as_ref()
        .and_then(|g| g["graph"]["nodes"].as_array())
        .map(|ns| {
            ns.iter()
                .any(|n| n["kind"].as_str().is_some_and(|k| k.starts_with("gen.")))
        })
        .unwrap_or(false)
}

/// The canvas's background grid: hairlines every `theme.graph_grid` graph
/// units, transformed by the same pan/zoom view transform the primitives
/// follow (which is why it lines up with the nodes it sits under). The
/// visible graph-space window comes off the same two corners `to_graph`
/// already inverts for hit-testing; the lines' own ink is a theme stroke,
/// never a literal (gate 2).
fn paint_grid(painter: &egui::Painter, theme: &Theme, rect: EguiRect, state: &PanelState) {
    // A grid finer than the panel's own padding is sub-pixel noise: paint
    // nothing rather than thousands of hairlines the antialiaser blurs
    // into mush.
    if theme.graph_grid * state.zoom < f32::from(theme.pad) {
        return;
    }
    let step = theme.graph_grid;
    let [min_x, min_y] = to_graph(rect.min, state.pan, state.zoom, rect.min);
    let [max_x, max_y] = to_graph(rect.min, state.pan, state.zoom, rect.max);
    let first_x = (min_x / step).floor() * step;
    let first_y = (min_y / step).floor() * step;
    let mut kx = first_x;
    while kx < max_x {
        let px = to_screen(rect.min, state.pan, state.zoom, [kx, min_x]).x;
        painter.line_segment(
            [Pos2::new(px, rect.min.y), Pos2::new(px, rect.max.y)],
            theme.rule_stroke(),
        );
        kx += step;
    }
    let mut ky = first_y;
    while ky < max_y {
        let py = to_screen(rect.min, state.pan, state.zoom, [min_x, ky]).y;
        painter.line_segment(
            [Pos2::new(rect.min.x, py), Pos2::new(rect.max.x, py)],
            theme.rule_stroke(),
        );
        ky += step;
    }
}

/// One node's live values in the last frame the engine drew, off the cached
/// `GET /graph/live` reply: `(params by name, output vars by name)`. Empty
/// before the first poll or for a graph nobody renders.
fn live_of(state: &PanelState) -> LiveRef<'_> {
    let Some(live) = &state.live else {
        return LiveRef::default();
    };
    LiveRef { nodes: live.get("nodes") }
}

/// A borrowed view of `GET /graph/live`'s `nodes` map — `{<node>: {in: {},
/// out: {}}}` — with a reader per node. Kept as a plain struct over the
/// cached `Value` so nothing is copied per primitive.
#[derive(Default, Clone, Copy)]
struct LiveRef<'a> {
    nodes: Option<&'a Value>,
}

impl<'a> LiveRef<'a> {
    /// `node`'s effective value of parameter `name` this frame, if the graph
    /// is rendering and the engine answered one.
    fn param(&self, node: &str, name: &str) -> Option<f32> {
        self.nodes?.get(node)?.get("in")?.get(name)?.as_f64().map(|v| v as f32)
    }
}

/// Whether a primitive is one of a node body's numeric fields — the value
/// readout and the number it shows (§4): the canvas hides these, so a node
/// shows its title, its ports and the mode chip only. A port label stays; it
/// names a port, not a number.
fn node_body_numeric(p: &Value) -> bool {
    matches!(p["tag"].as_str(), Some("value_field") | Some("value"))
}

/// Paints one already-resolved [`metal_graph::layout::Primitive`], read back as
/// JSON (`primitive_json` in `main.rs`) — never recomputed. Every primitive
/// carries its **tag** (N), so this function never guesses from a size: it
/// reads `tag` and paints the Wire shape the tag names.
#[allow(clippy::too_many_arguments)]
fn paint_primitive(
    painter: &egui::Painter,
    theme: &Theme,
    origin: EguiRect,
    state: &PanelState,
    p: &Value,
    tiles: Option<&(egui::TextureId, [usize; 2])>,
    drag: Option<DragOffset>,
    selected: Option<&str>,
    lit_edges: [Option<&str>; 2],
    live: LiveRef<'_>,
) {
    match p["tag"].as_str() {
        Some("node") => paint_node_box(painter, theme, origin, state, p, tiles, drag, selected),
        Some("header") => {
            let r = shifted_rect(origin, state, p, [num(p, "x"), num(p, "y")], drag);
            painter.rect_filled(r, theme.corner_radius(), theme.node_header);
        }
        Some("port") => paint_port(painter, theme, origin, state, p, drag),
        Some("param_dot") => paint_param_dot(painter, theme, origin, state, p, drag, live),
        Some("value_field") => {
            paint_value_field(painter, theme, origin, state, p, drag, live)
        }
        Some("swatch") => paint_swatch(painter, theme, origin, state, p, drag),
        Some("tile") => paint_tile(painter, theme, origin, state, p, tiles, drag),
        Some("chip") => paint_chip(painter, theme, origin, state, p, drag, selected),
        Some("more") => {
            let r = shifted_rect(origin, state, p, [num(p, "x"), num(p, "y")], drag);
            painter.rect_filled(r, theme.corner_radius(), theme.node_body);
        }
        _ => match p["type"].as_str() {
            Some("path") => paint_cable(painter, theme, origin, state, p, drag, selected, lit_edges),
            Some("text") => paint_text(painter, theme, origin, state, p, drag),
            _ => {}
        },
    }
}

/// A screen rect for a tagged rect primitive, its top-left shifted by the
/// drag when it belongs to the dragged node.
fn shifted_rect(
    origin: EguiRect,
    state: &PanelState,
    p: &Value,
    top_left: [f32; 2],
    drag: Option<DragOffset>,
) -> EguiRect {
    let w = num(p, "w");
    let h = num(p, "h");
    let center = [top_left[0] + w / 2.0, top_left[1] + h / 2.0];
    let [sx, sy] = drag_shift(top_left, center, drag);
    let min = to_screen(origin.min, state.pan, state.zoom, [sx, sy]);
    EguiRect::from_min_size(min, Vec2::new(w, h) * state.zoom)
}

/// The node box: the raised dark-grey card, outlined in the accent when it is
/// the selection (`nodes-reference.png`: the selected node has an orange
/// outline). A generator's model tile is painted inside it (see `tile`).
fn paint_node_box(
    painter: &egui::Painter,
    theme: &Theme,
    origin: EguiRect,
    state: &PanelState,
    p: &Value,
    _tiles: Option<&(egui::TextureId, [usize; 2])>,
    drag: Option<DragOffset>,
    selected: Option<&str>,
) {
    let r = shifted_rect(origin, state, p, [num(p, "x"), num(p, "y")], drag);
    // The box is a rect at the node's own `pos`; the tag does not name the
    // node (its `x`/`y` *are* the position), but the selection is keyed by
    // id, so membership is tested by the box's own corner against every
    // selected node's cached position.
    let at_box = |id: &str| {
        find_node_pos(state, id).is_some_and(|pos| pos[0] == num(p, "x") && pos[1] == num(p, "y"))
    };
    // A Ctrl+click multi-selection outlines every picked node, not just the primary.
    let is_selected = selected.is_some_and(at_box) || state.picked.iter().any(|id| at_box(id));
    let stroke = if is_selected {
        theme.node_select_stroke()
    } else {
        theme.node_box_stroke()
    };
    painter.rect(r, theme.corner_radius(), theme.node_body, stroke, egui::StrokeKind::Middle);
    // The reveal's pulse: the accent outline, fading over [`PULSE`].
    if let Some((id, left)) = state.pulsing() {
        let here = find_node_pos(state, id)
            .is_some_and(|pos| pos[0] == num(p, "x") && pos[1] == num(p, "y"));
        if here {
            painter.rect_stroke(
                r.expand(theme.hairline),
                theme.corner_radius(),
                theme.reveal_pulse_stroke(left),
                egui::StrokeKind::Outside,
            );
        }
    }
}

/// A port dot: the accent when the port is wired or is an output offering a
/// cable, grey when nothing touches it (Wire's on/off dot). Outputs get a
/// small ring so a generation's render reads as "live" (N's activity hint).
fn paint_port(
    painter: &egui::Painter,
    theme: &Theme,
    origin: EguiRect,
    state: &PanelState,
    p: &Value,
    drag: Option<DragOffset>,
) {
    let r = shifted_rect(origin, state, p, [num(p, "x"), num(p, "y")], drag);
    let out = p["out"].as_bool().unwrap_or(false);
    let wired = port_is_wired(state, p);
    let fill = if wired || out { theme.node_dot_on } else { theme.node_dot_off };
    painter.circle_filled(r.center(), r.width() / 2.0, fill);
    if out {
        painter.circle_stroke(
            r.center(),
            r.width() / 2.0 + theme.hairline,
            theme.node_port_ring_stroke(),
        );
    }
}

/// A parameter's dot, on the row's left edge — `param_dot` (the field's twin,
/// the drop target a cable binds into).
fn paint_param_dot(
    painter: &egui::Painter,
    theme: &Theme,
    origin: EguiRect,
    state: &PanelState,
    p: &Value,
    drag: Option<DragOffset>,
    live: LiveRef<'_>,
) {
    let r = shifted_rect(origin, state, p, [num(p, "x"), num(p, "y")], drag);
    let node = p["node"].as_str().unwrap_or_default();
    let name = p["name"].as_str().unwrap_or_default();
    let wired = param_is_wired(state, node, name);
    // Live: a bound parameter whose value is moving this frame gets the lit
    // accent, so a driven row reads at a glance (N's "play this row" hint).
    let lit = live.param(node, name).is_some();
    let fill = if wired || lit { theme.node_dot_on } else { theme.node_dot_off };
    painter.circle_filled(r.center(), r.width() / 2.0, fill);
}

/// A parameter's value field: the recessed readout on the row's right. While a
/// drag edits it the in-flight value paints; otherwise the cached value, and a
/// live value if the engine is driving it (`GET /graph/live`).
fn paint_value_field(
    painter: &egui::Painter,
    theme: &Theme,
    origin: EguiRect,
    state: &PanelState,
    p: &Value,
    drag: Option<DragOffset>,
    live: LiveRef<'_>,
) {
    let r = shifted_rect(origin, state, p, [num(p, "x"), num(p, "y")], drag);
    let node = p["node"].as_str().unwrap_or_default();
    let name = p["name"].as_str().unwrap_or_default();
    painter.rect_filled(r, theme.corner_radius(), theme.node_field);
    let editing = state
        .param_drag
        .as_ref()
        .filter(|d| d.node == node && d.name == name);
    let shown = editing.map(|d| d.value).or_else(|| live.param(node, name));
    if let Some(v) = shown {
        painter.text(
            r.right_center() - Vec2::new(theme.node_label_inset * 0.4, 0.0),
            Align2::RIGHT_CENTER,
            format_number(v),
            theme.font_small.clone(),
            if editing.is_some() { theme.node_dot_on } else { theme.node_field_ink },
        );
    }
}

/// A colour swatch — the RGB the node's config carries, painted as a chip.
fn paint_swatch(
    painter: &egui::Painter,
    theme: &Theme,
    origin: EguiRect,
    state: &PanelState,
    p: &Value,
    drag: Option<DragOffset>,
) {
    let r = shifted_rect(origin, state, p, [num(p, "x"), num(p, "y")], drag);
    let color = p["color"].as_array().map(|a| {
        let c = |i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32;
        theme.node_rgb(c(0), c(1), c(2))
    });
    match color {
        Some(c) => {
            painter.rect_filled(r, theme.corner_radius(), c);
            painter.rect_stroke(
                r,
                theme.corner_radius(),
                theme.node_swatch_stroke(),
                egui::StrokeKind::Middle,
            );
        }
        None => {
            painter.rect_filled(r, theme.corner_radius(), theme.node_field);
        }
    }
}

/// A generator's model tile — the grid texture crop inside the node's body.
fn paint_tile(
    painter: &egui::Painter,
    theme: &Theme,
    origin: EguiRect,
    state: &PanelState,
    p: &Value,
    tiles: Option<&(egui::TextureId, [usize; 2])>,
    drag: Option<DragOffset>,
) {
    let r = shifted_rect(origin, state, p, [num(p, "x"), num(p, "y")], drag);
    painter.rect_filled(r, theme.corner_radius(), theme.node_field);
    // The node's own picture, drawn from its row, once it has landed.
    let own = node_id_at(state, [num(p, "x"), num(p, "y")])
        .and_then(|id| state.node_tiles.get(&id))
        .and_then(|f| f.current_picture());
    if let Some((tex, _)) = own {
        painter.image(tex, r, EguiRect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), theme.image_tint);
    } else if let Some((tex, _)) = tiles {
        // The tile's uv is found from the node's own position, the same join
        // `node_tile_at` makes for the old painter: the rect's `x`/`y` are
        // the node's `pos`.
        if let Some(uv) = node_tile_at(state, [num(p, "x"), num(p, "y")]) {
            painter.image(*tex, r, uv, theme.image_tint);
        }
    }
}

/// An embedded `ai.*` (`ai.embed`) as a chip on its source: a small pill with
/// the AI's name, painted above the source's top-right corner. It is not a
/// free node — the layout gives it no box of its own — so this is the only
/// thing that draws it.
fn paint_chip(
    painter: &egui::Painter,
    theme: &Theme,
    origin: EguiRect,
    state: &PanelState,
    p: &Value,
    drag: Option<DragOffset>,
    selected: Option<&str>,
) {
    let r = shifted_rect(origin, state, p, [num(p, "x"), num(p, "y")], drag);
    let node = p["node"].as_str().unwrap_or_default();
    let stroke = if selected == Some(node) {
        theme.node_select_stroke()
    } else {
        theme.node_box_stroke()
    };
    painter.rect(r, theme.corner_radius(), theme.node_header, stroke, egui::StrokeKind::Middle);
    let ai = p["ai"].as_str().unwrap_or("ai");
    painter.text(
        r.center(),
        Align2::CENTER_CENTER,
        ai.strip_prefix("ai.").unwrap_or(ai),
        theme.font_small.clone(),
        theme.ink,
    );
}

/// The source an embedded `ai.*` node hangs off (`ai.embed`), read off the
/// cached `GET /graph` reply — never recomputed (rule 5). `None` for a free
/// node.
fn embedded_source(state: &PanelState, node: &str) -> Option<String> {
    let nodes = state.graph.as_ref()?["graph"]["nodes"].as_array()?;
    nodes
        .iter()
        .find(|n| n["id"].as_str() == Some(node))
        .and_then(|n| n["embedded_in"].as_str().map(str::to_string))
}

/// Whether a cable primitive is one of an embedded `ai.*`'s wires, and so is
/// hidden unless its source is the selection (plan A3.2). The wires exist in
/// the layout so a selected source can show them; everything else hides them.
fn embedded_cable_hidden(p: &Value, state: &PanelState, selected: Option<&str>) -> bool {
    for end in ["from", "to"] {
        let Some(id) = p[end]["node"].as_str() else { continue };
        if let Some(source) = embedded_source(state, id) {
            return selected != Some(source.as_str());
        }
    }
    false
}

/// Whether a cable primitive touches the selected node at either end — the
/// "selected node's cables draw in the accent" rule (`nodes-reference.png`),
/// factored out so it is testable without a painter.
fn cable_touches_selection(p: &Value, selected: Option<&str>) -> bool {
    selected.is_some_and(|id| {
        p["from"]["node"].as_str() == Some(id) || p["to"]["node"].as_str() == Some(id)
    })
}

/// Whether a cable draws in the accent: an end touches the selected node, or
/// the cable itself is one of `lit_edges` (the selected cable, the hovered
/// one). Before 2026-09-30 only the node rule existed, so a selected cable
/// never lit up on its own.
fn cable_lit(p: &Value, selected: Option<&str>, lit_edges: [Option<&str>; 2]) -> bool {
    let edge = p["edge"].as_str();
    cable_touches_selection(p, selected) || (edge.is_some() && lit_edges.contains(&edge))
}

/// How near (screen pixels) the pointer must be to a cable to hover it.
const EDGE_HOVER_PX: f32 = 6.0;

/// The point at `t` on a cubic bezier given in graph space.
fn bezier_at(c: [[f32; 2]; 4], t: f32) -> [f32; 2] {
    let mt = 1.0 - t;
    let (a, b, cc, d) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
    [
        a * c[0][0] + b * c[1][0] + cc * c[2][0] + d * c[3][0],
        a * c[0][1] + b * c[1][1] + cc * c[2][1] + d * c[3][1],
    ]
}

/// The cable whose curve passes within `tol` (graph units) of `at`, nearest
/// first — the curve the door already resolved, sampled, never re-laid out.
fn edge_near(state: &PanelState, at: [f32; 2], tol: f32) -> Option<String> {
    let prims = state.graph.as_ref()?["primitives"].as_array()?;
    let mut best: Option<(f32, String)> = None;
    for p in prims.iter().filter(|p| p["type"] == "path") {
        let Some(edge) = p["edge"].as_str() else { continue };
        // A hidden embedded `ai.*` wire is not hoverable either: it must not
        // light up on its own while the source is unselected.
        if embedded_cable_hidden(p, state, None) {
            continue;
        }
        let c = [pt(p, "p0"), pt(p, "p1"), pt(p, "p2"), pt(p, "p3")];
        let mut prev = bezier_at(c, 0.0);
        for i in 1..=32 {
            let next = bezier_at(c, i as f32 / 32.0);
            let d = dist_to_segment(at, prev, next);
            if d <= tol && best.as_ref().is_none_or(|(b, _)| d < *b) {
                best = Some((d, edge.to_string()));
            }
            prev = next;
        }
    }
    best.map(|(_, e)| e)
}

fn dist_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0) };
    let (x, y) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    (x * x + y * y).sqrt()
}

/// A cable: a smooth bezier, solid for an image cable and dashed for a
/// control or param cable (N), in the accent when either end touches the
/// selected node and grey otherwise (`nodes-reference.png`).
fn paint_cable(
    painter: &egui::Painter,
    theme: &Theme,
    origin: EguiRect,
    state: &PanelState,
    p: &Value,
    drag: Option<DragOffset>,
    selected: Option<&str>,
    lit_edges: [Option<&str>; 2],
) {
    // An embedded `ai.*`'s wires are hidden unless its source is selected
    // (plan A3.2) — the chip is the reaction's only visible mark otherwise.
    if embedded_cable_hidden(p, state, selected) {
        return;
    }
    let raw = [pt(p, "p0"), pt(p, "p1"), pt(p, "p2"), pt(p, "p3")];
    // Each end of a cable moves with whichever node it lands on — `p0`/`p1`
    // together if the *from* port is the dragged node's, `p2`/`p3` together
    // if the *to* port is — so a cable stays attached to a moving box.
    let mut gp = raw;
    if let Some((node_pos, delta, node_h)) = drag {
        if in_dragged_node(raw[0], node_pos, node_h) {
            gp[0] = add(raw[0], delta);
            gp[1] = add(raw[1], delta);
        }
        if in_dragged_node(raw[3], node_pos, node_h) {
            gp[3] = add(raw[3], delta);
            gp[2] = add(raw[2], delta);
        }
    }
    let screen = gp.map(|xy| to_screen(origin.min, state.pan, state.zoom, xy));
    // The endpoints name their nodes; a cable whose from or to node is the
    // selection draws in the accent. A param edge draws dashed too.
    let touches_selection = cable_lit(p, selected, lit_edges);
    let kind = p["kind"].as_str().unwrap_or("image");
    let stroke = theme.node_cable_stroke(touches_selection);
    if kind == "image" {
        let shape = CubicBezierShape::from_points_stroke(
            screen,
            false,
            theme.transparent(),
            stroke,
        );
        painter.add(Shape::CubicBezier(shape));
    } else {
        // Dashed: sample the curve and draw the on-segments — the same
        // bezier, a Wire look for a control/param cable.
        let mut pts = Vec::with_capacity(33);
        for i in 0..=32 {
            let t = i as f32 / 32.0;
            let mt = 1.0 - t;
            let x = mt * mt * mt * screen[0].x
                + 3.0 * mt * mt * t * screen[1].x
                + 3.0 * mt * t * t * screen[2].x
                + t * t * t * screen[3].x;
            let y = mt * mt * mt * screen[0].y
                + 3.0 * mt * mt * t * screen[1].y
                + 3.0 * mt * t * t * screen[2].y
                + t * t * t * screen[3].y;
            pts.push(Pos2::new(x, y));
        }
        let dash = theme.node_cable_dash.max(1.0);
        let mut drawn = 0.0f32;
        for w in pts.windows(2) {
            let seg = (w[1] - w[0]).length();
            if (drawn / dash) as i32 % 2 == 0 {
                painter.line_segment([w[0], w[1]], stroke);
            }
            drawn += seg;
        }
    }
}

/// A text primitive, by its tag (`primitive_json`): a `title` centred in the
/// header band in the accent, a `row` label on the left of its line in the
/// row ink, a `value`/`more` readout, a `plain` label left-anchored.
fn paint_text(
    painter: &egui::Painter,
    theme: &Theme,
    origin: EguiRect,
    state: &PanelState,
    p: &Value,
    drag: Option<DragOffset>,
) {
    let raw = [num(p, "x"), num(p, "y")];
    let shifted = drag_shift(raw, raw, drag);
    let pos = to_screen(origin.min, state.pan, state.zoom, shifted);
    let content = p["content"].as_str().unwrap_or_default();
    let (align, font, ink) = match p["tag"].as_str() {
        Some("title") => (Align2::CENTER_CENTER, theme.font_small.clone(), theme.node_title_ink),
        Some("row") => (Align2::LEFT_CENTER, theme.font_small.clone(), theme.node_row_ink),
        Some("value") => (Align2::LEFT_CENTER, theme.font_small.clone(), theme.node_field_ink),
        Some("more") => (Align2::LEFT_CENTER, theme.font_small.clone(), theme.node_more_ink),
        _ => (Align2::LEFT_TOP, theme.font_small.clone(), theme.ink),
    };
    // A node's title label is painted at the header's centre; a row label at
    // its row's left inset. Both anchors came from the layout.
    let display = if p["tag"].as_str() == Some("row") || p["tag"].as_str() == Some("title") {
        tr_dyn(content)
    } else {
        content.to_string()
    };
    painter.text(pos, align, display, font, ink);
}

/// The last packet a `venue.fixture` node sent, through the door:
/// `GET /venue/fixture-packet?graph=<gid>&node=<id>` (F10/F11/F14). The reply
/// carries the swatch decoded from the packet on the bus, never a
/// recomputation.
fn fixture_packet(door: &dyn Call, gid: &str, node: &str) -> Option<Value> {
    let mut query = HashMap::new();
    query.insert("graph".to_string(), gid.to_string());
    query.insert("node".to_string(), node.to_string());
    let params = Params { query, body: Vec::new(), tail: None };
    door.call("GET", "/venue/fixture-packet", params)
        .filter(|r| r.status == 200)
        .and_then(|r| crate::json_memo::parse(&r.body).ok())
}

/// F10: a `venue.fixture` node's card extras, painted over the layout's own
/// card (rule 5 keeps the geometry the door's): the fixture's **name** as the
/// title, `profile · U<universe>.<address>` as the subtitle, and a live swatch
/// of the colour it is sending — the last packet off the bus, never
/// recomputed. A bypassed or orphan node paints grey.
fn paint_fixture_cards(
    painter: &egui::Painter,
    theme: &Theme,
    door: &dyn Call,
    origin: EguiRect,
    state: &PanelState,
    drag: Option<DragOffset>,
) {
    let Some(graph_val) = state.graph.as_ref() else { return };
    let gid = graph_val["graph"]["id"].as_str().unwrap_or_default();
    let (Some(nodes), Some(prims)) = (
        graph_val["graph"]["nodes"].as_array(),
        graph_val["primitives"].as_array(),
    ) else {
        return;
    };
    for n in nodes.iter().filter(|n| n["kind"] == json!(metal_graph::VENUE_FIXTURE_KIND)) {
        let Some(id) = n["id"].as_str() else { continue };
        let pos = [
            n["pos"][0].as_f64().unwrap_or(0.0) as f32,
            n["pos"][1].as_f64().unwrap_or(0.0) as f32,
        ];
        let find = |tag: &str| {
            prims
                .iter()
                .find(|p| p["tag"] == json!(tag) && num(p, "x") == pos[0] && num(p, "y") == pos[1])
        };
        let Some(header) = find("header") else { continue };
        let h = shifted_rect(origin, state, header, pos, drag);
        let Some(pkt) = fixture_packet(door, gid, id) else { continue };
        let state_word = pkt["state"].as_str().unwrap_or("silent");
        let muted = matches!(state_word, "bypassed" | "orphan");
        let name = pkt["name"].as_str().filter(|s| !s.is_empty()).unwrap_or(id);
        painter.text(h.center(), Align2::CENTER_CENTER, name, theme.font_small.clone(), theme.node_title_ink);
        let profile = pkt["profile"].as_str().unwrap_or("—").to_string();
        let uni = pkt["universe"].as_u64().unwrap_or(0).to_string();
        let addr = pkt["address"].as_u64().unwrap_or(0).to_string();
        painter.text(
            egui::pos2(h.left(), h.bottom() + theme.item_gap.y),
            Align2::LEFT_TOP,
            trf("{profile} · U{universe}.{address}", &[
                ("profile", &profile), ("universe", &uni), ("address", &addr),
            ]),
            theme.font_small.clone(),
            if muted { theme.ink_faint } else { theme.ink_weak },
        );
        let rgb = pkt["colour"]
            .as_array()
            .map(|a| {
                [
                    a.first().and_then(Value::as_f64).unwrap_or(0.0) as f32,
                    a.get(1).and_then(Value::as_f64).unwrap_or(0.0) as f32,
                    a.get(2).and_then(Value::as_f64).unwrap_or(0.0) as f32,
                ]
            })
            .unwrap_or([0.0; 3]);
        let size = theme.swatch_size;
        let swatch = egui::Rect::from_min_size(
            egui::pos2(h.right() - size.x, h.top() + (h.height() - size.y) / 2.0),
            size,
        );
        painter.rect_filled(swatch, theme.radius, if muted { theme.ink_faint } else { theme.color_of(rgb) });
        painter.rect_stroke(swatch, theme.radius, theme.node_swatch_stroke(), egui::StrokeKind::Middle);
    }
}

/// Whether a port primitive's port is wired to anything, read off the cached
/// graph's edges — the dot's on/off style. A port is "on" when an edge leaves
/// or lands on it, matching Wire's lit dot.
fn port_is_wired(state: &PanelState, p: &Value) -> bool {
    let Some(node) = p["node"].as_str() else { return false };
    let Some(port) = p["port"].as_str() else { return false };
    let out = p["out"].as_bool().unwrap_or(false);
    edges_of(state).iter().any(|e| {
        if out {
            e["from"]["node"].as_str() == Some(node) && e["from"]["port"].as_str() == Some(port)
        } else {
            e["to"]["node"].as_str() == Some(node) && e["to"]["port"].as_str() == Some(port)
        }
    })
}

/// Whether a parameter row already has a binding (`param:<name>` edge).
fn param_is_wired(state: &PanelState, node: &str, name: &str) -> bool {
    let to_port = format!("param:{name}");
    edges_of(state).iter().any(|e| {
        e["to"]["node"].as_str() == Some(node) && e["to"]["port"].as_str() == Some(&to_port)
    })
}

/// A compact fixed-point number for a value field ("0", "0.5", "3") — the
/// same shape the layout's own readout uses.
fn format_number(v: f32) -> String {
    if (v - v.round()).abs() < 0.001 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// The variable chooser a multi-variable output drop opened: a small floating
/// list at the drop point, one button per variable the producer offers, each
/// posting the bind op with that variable's `port.var` (`metal_graph::bind`). A
/// pick (or a click anywhere else, or `Esc`) closes it.
fn paint_bind_menu(
    ui: &Ui,
    painter: &egui::Painter,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    rect: EguiRect,
) {
    let Some(menu) = state.bind_menu.clone() else { return };
    let panel = EguiRect::from_min_size(menu.at, Vec2::new(theme.menu_max_height * 0.6, 0.0));
    // The chooser is a real egui area so its buttons are clickable and named
    // for the kittest suite; painted over the canvas, above everything.
    let mut picked: Option<(String, String, String)> = None;
    let mut close = false;
    egui::Area::new(Id::new("shell/region/nodes/bind-menu"))
        .fixed_pos(panel.min)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(&ui.style()).show(ui, |ui| {
                ui.label(
                    RichText::new(trf("bind {param}", &[("param", &menu.param)]))
                        .font(theme.font_small.clone())
                        .color(theme.ink_weak),
                );
                for (port, var, _port_ref) in &menu.vars {
                    let label = format!("{port}.{var}");
                    if ui.button(RichText::new(&label).font(theme.font_small.clone())).clicked() {
                        picked = Some((port.clone(), var.clone(), String::new()));
                    }
                }
                if ui.button(RichText::new(tr("cancel")).font(theme.font_small.clone())).clicked() {
                    close = true;
                }
            });
        });
    // Clicking the canvas outside the menu, or a pick, dismisses it.
    if picked.is_none()
        && ui.input(|i| i.pointer.any_click())
        && ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| !panel.contains(p) && rect.contains(p))
    {
        close = true;
    }
    if let Some((port, var, _)) = picked {
        bind_param(
            door,
            state,
            &menu.node,
            &menu.param,
            Some((menu.producer.as_str(), port.as_str(), var.as_str())),
        );
        close = true;
    } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if close {
        state.bind_menu = None;
    }
    let _ = painter;
}

/// The `[r, g, b]` a node's config key holds: `rgb` or `colors:<i>`, the two
/// spellings [`metal_graph::layout`] tags a swatch with.
fn swatch_rgb(node: &Value, key: &str) -> Option<[f32; 3]> {
    let triple = |v: &Value| -> Option<[f32; 3]> {
        let a = v.as_array()?;
        Some([
            a.first()?.as_f64()? as f32,
            a.get(1)?.as_f64()? as f32,
            a.get(2)?.as_f64()? as f32,
        ])
    };
    if key == "rgb" {
        return node["config"]["rgb"].as_array().and_then(|_| triple(&node["config"]["rgb"]));
    }
    let i = key.strip_prefix("colors:")?.parse::<usize>().ok()?;
    node["config"]["colors"].as_array()?.get(i).and_then(triple)
}

/// Every colour a palette node's config holds, as `[r, g, b]`.
fn config_colors(node: &Value) -> Vec<[f32; 3]> {
    node["config"]["colors"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    let c = c.as_array()?;
                    Some([
                        c.first()?.as_f64()? as f32,
                        c.get(1)?.as_f64()? as f32,
                        c.get(2)?.as_f64()? as f32,
                    ])
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The canvas's own colour write (2026-10-08): the same `config` merge the
/// properties panel posts — `rgb` (+ h/s/b) for a `color` node, the whole
/// `colors` array with slot `i` replaced for a `palette` node.
fn post_swatch(door: &dyn Call, state: &mut PanelState, node: &str, key: &str, rgb: [f32; 3]) {
    let Some(gid) = state.selected_graph.clone() else {
        return;
    };
    let config = if key == "rgb" {
        let hsb = Hsb::from_rgb(rgb[0], rgb[1], rgb[2]);
        json!({ "rgb": rgb, "h": hsb.h, "s": hsb.s, "b": hsb.b })
    } else if let Some(i) = key.strip_prefix("colors:").and_then(|s| s.parse::<usize>().ok()) {
        let mut colors = node_record(state, node).map(|n| config_colors(&n)).unwrap_or_default();
        if colors.len() <= i {
            colors.resize(i + 1, [1.0, 1.0, 1.0]);
        }
        colors[i] = rgb;
        let colors: Vec<Value> =
            colors.iter().map(|c| json!([c[0], c[1], c[2]])).collect();
        json!({ "colors": colors })
    } else {
        return;
    };
    super::properties::shared::post_node_config(door, &gid, node, config);
}

/// The label a swatch's key wears in the picker: a `color` node's lone chip is
/// just `colour`, and a palette's first two slots are the key and the
/// secondary; anything else falls back to the raw key.
fn swatch_label(key: &str) -> String {
    match key {
        "rgb" => tr("colour").to_string(),
        "colors:0" => tr("key").to_string(),
        "colors:1" => tr("secondary").to_string(),
        _ => trf("colour {key}", &[("key", &key)]),
    }
}

/// The picker a clicked node swatch opens (2026-10-08) — a real egui area over
/// the canvas, the same pattern [`paint_bind_menu`] uses, so the `hsb_picker`
/// is draggable and its numbers are legible. It stays open across gestures
/// (each release posts once; the chip beside the picker tracks the drag), and
/// closes only on `cancel`, Escape or a click on the canvas outside it.
fn paint_colour_menu(
    ui: &Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    rect: EguiRect,
) {
    let Some(menu) = state.colour_menu.clone() else {
        return;
    };
    let panel = EguiRect::from_min_size(
        menu.at,
        Vec2::new(theme.picker_square.x + theme.picker_gap + theme.picker_hue.x, 0.0),
    );
    let mut hsb = Hsb::from_rgb(menu.rgb[0], menu.rgb[1], menu.rgb[2]);
    let mut changed = false;
    let mut close = false;
    egui::Area::new(Id::new("shell/region/nodes/colour-menu"))
        .fixed_pos(panel.min)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(&ui.style()).show(ui, |ui| {
                ui.label(
                    RichText::new(swatch_label(&menu.key))
                        .font(theme.font_small.clone())
                        .color(theme.ink_weak),
                );
                let _ = swatch_chip(ui, theme, &hsb, true);
                if hsb_picker(ui, theme, &mut hsb).changed() {
                    changed = true;
                }
                ui.label(
                    RichText::new(hsb.hex())
                        .font(theme.font_small.clone())
                        .color(theme.ink),
                );
                if ui
                    .button(RichText::new(tr("cancel")).font(theme.font_small.clone()))
                    .clicked()
                {
                    close = true;
                }
            });
        });
    if changed {
        // The picker keeps its own seed: without moving it the square snaps
        // back to the old colour on the next frame and a pick never lands.
        if let Some(m) = state.colour_menu.as_mut() {
            m.rgb = hsb.to_rgb();
        }
        post_swatch(door, state, &menu.node, &menu.key, hsb.to_rgb());
    }
    // Clicking the canvas outside the menu dismisses it, like the bind menu.
    if !close
        && ui.input(|i| i.pointer.any_click())
        && ui
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| !panel.contains(p) && rect.contains(p))
    {
        close = true;
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if close {
        state.colour_menu = None;
    }
}

/// Poll `GET /graph/live?id=` at [`LIVE_POLL`] while the canvas paints and
/// cache the reply. The route reads the engine's last frame, so this is the
/// cheapest way to show a driven value move without a layout recompute; a
/// graph nobody renders answers `t: null` and is cached like any other.
fn poll_live(door: &dyn Call, state: &mut PanelState) {
    let now = web_time::Instant::now();
    if state.live_at.is_some_and(|t| now.duration_since(t) < LIVE_POLL) {
        return;
    }
    let Some(gid) = state.selected_graph.clone() else { return };
    state.live = get(door, "GET", "/graph/live", &[("id", gid)]);
    state.live_at = Some(now);
}

fn num(v: &Value, key: &str) -> f32 {
    v[key].as_f64().unwrap_or(0.0) as f32
}

fn pt(v: &Value, key: &str) -> [f32; 2] {
    let a = v[key].as_array();
    let x = a
        .and_then(|a| a.first())
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as f32;
    let y = a
        .and_then(|a| a.get(1))
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as f32;
    [x, y]
}

/// Graph space -> screen space: the pan/zoom view transform, applied on top
/// of a position that already came from the door.
fn to_screen(origin: Pos2, pan: Vec2, zoom: f32, p: [f32; 2]) -> Pos2 {
    origin + pan + Vec2::new(p[0], p[1]) * zoom
}

/// Where a node added from the `+ node` menu lands: the graph-space point
/// under the centre of the canvas as it is on screen now — the inverse of
/// [`to_screen`] applied to the canvas's own centre.
pub(crate) fn new_node_spot(canvas: EguiRect, pan: Vec2, zoom: f32) -> [f32; 2] {
    to_graph(canvas.min, pan, zoom, canvas.center())
}

/// Screen space -> graph space: the inverse of [`to_screen`], used only to
/// turn a pointer position into the `x`/`y` this file sends to `GET
/// /graph/hit` — never to compute a box or a curve.
fn to_graph(origin: Pos2, pan: Vec2, zoom: f32, screen: Pos2) -> [f32; 2] {
    let v = (screen - origin - pan) / zoom;
    [v.x, v.y]
}

// ---------------------------------------------------------------------------
// pointer: click selects, drag moves (rule 5: every hit through the door)
// ---------------------------------------------------------------------------

/// `response.is_pointer_button_down_on()` is true from the press frame
/// onward with no click-vs-drag decision window (unlike `drag_started`,
/// which only fires once the pointer has moved past egui's click-distance
/// threshold) — exactly what a graph editor needs: the hit-test has to run
/// at the press, not somewhere after the drag is already underway, because
/// the door's answer (which node, if any) is what a drag needs at its
/// start. `state.pressing` turns that level into the two edges this file
/// actually acts on: press-started (hit-test) and press-ended (commit).
fn handle_pointer(
    ui: &Ui,
    response: &Response,
    rect: EguiRect,
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    theme: &Theme,
) {
    let down_now = response.is_pointer_button_down_on();

    if down_now && !state.pressing {
        state.pressing = true;
        let press_pos = ui.input(|i| i.pointer.press_origin()).unwrap_or(rect.min);
        let ctrl = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
        press_hit_test(door, state, selection, rect, press_pos, ctrl);
    }

    if down_now {
        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
            // The "drag to pan" gesture: a press that landed
            // on empty canvas is a pan-drag, so the view follows the
            // pointer frame by frame — a pure view transform, nothing
            // committed on release (there is no `finish` to it at all).
            if let Some(pan) = &state.pan_drag {
                state.pan = pan.start_pan + (pos - pan.start_screen);
            }
            if let Some(drag) = &mut state.drag {
                drag.cur_pointer = to_graph(rect.min, state.pan, state.zoom, pos);
            }
            if let Some(cable) = &mut state.cable {
                cable.cur_screen = pos;
            }
            // A value-field drag: the value follows the pointer's horizontal
            // travel, scaled against the range (Shift, finer — N). Still
            // nothing sent: `finish_param_drag` posts once on release.
            if let Some(drag) = &mut state.param_drag {
                drag.cur_pointer = to_graph(rect.min, state.pan, state.zoom, pos);
                let dx = drag.cur_pointer[0] - drag.start_pointer[0];
                let shift = ui.input(|i| i.modifiers.shift);
                let per = (drag.max - drag.min) / PARAM_DRAG_SPAN * if shift { 0.25 } else { 1.0 };
                drag.value = (drag.start_value + dx * per).clamp(drag.min, drag.max);
            }
        }
    }

    if !down_now && state.pressing {
        state.pressing = false;
        state.pan_drag = None;
        finish_drag(door, state, selection, theme);
        finish_cable(door, state, theme);
        finish_param_drag(door, state);
    }
}

/// A release resolves the cable the same way every press does: `GET
/// /graph/hit` at the endpoint's last screen position. A port landed on,
/// and not the one the stretch started from, is one `POST /graph/edge {
/// op: "add" }` — never a re-derivation of which port sits under the
/// pointer (rule 5). The drag's inverse — the `edge` id the route
/// answers — is recorded so `CTRL + Z` can quitar it. Anything else
/// (empty canvas, the same port, no reply) is a no-op: the ghost line
/// dies with the gesture, the graph stays as it was.
fn finish_cable(door: &dyn Call, state: &mut PanelState, theme: &Theme) {
    let Some(cable) = state.cable.take() else {
        return;
    };
    if state.selected_graph.is_none() {
        return;
    }
    let origin = state.last_canvas_rect.map(|r| r.min).unwrap_or_default();
    let [gx, gy] = to_graph(origin, state.pan, state.zoom, cable.cur_screen);
    let gid = state.selected_graph.clone().unwrap_or_default();
    let hit = hit_at(door, state, &gid, gx, gy);
    let lands = hit.as_ref().and_then(|h| {
        Some((
            h["node"].as_str()?.to_string(),
            h["port"].as_str()?.to_string(),
            h["dir"].as_str()? == "out",
        ))
    });
    let fresh = lands.filter(|(node, port, _)| *node != cable.from_node || *port != cable.from_port);

    // A drop onto (or from) a `param:<name>` port is a **binding** (N), not a
    // cable: the parameter is an input, so the producer is always the other
    // end. `param:` stripped is the parameter name.
    let param_end = |node: &str, port: &str| -> Option<(String, String)> {
        port.strip_prefix("param:")
            .filter(|n| !n.is_empty())
            .map(|name| (node.to_string(), name.to_string()))
    };
    if let Some((node, port, to_is_out)) = fresh.clone() {
        let binding = if cable.from_is_out {
            // Started on an output: the drop is the parameter (its dot answers
            // a `param:` port) — bind output -> param.
            param_end(&node, &port)
                .map(|(consumer, p)| (consumer, p, cable.from_node.clone(), cable.from_port.clone()))
        } else {
            // Started on a parameter dot: the drop has to be an output port.
            match (param_end(&cable.from_node, &cable.from_port), to_is_out) {
                (Some((consumer, p)), true) => Some((consumer, p, node.clone(), port.clone())),
                (Some(_), false) => {
                    return;
                }
                _ => None,
            }
        };
        if let Some((consumer, param, producer, producer_port)) = binding {
            bind_from_output(door, state, &consumer, &param, &producer, &producer_port, cable.cur_screen);
            return;
        }
    }

    // Whichever end the operator grabbed first, the edge always has to be
    // sent output -> input: an out/out or in/in drop is not a backwards
    // request, it is not a link at all, so it is refused here rather than
    // sent to `add_edge_checked` to reject.
    if let Some((node, port, to_is_out)) = fresh {
        let (from, to) = match (cable.from_is_out, to_is_out) {
            (true, false) => (
                (cable.from_node.clone(), cable.from_port.clone()),
                (node, port),
            ),
            (false, true) => (
                (node, port),
                (cable.from_node.clone(), cable.from_port.clone()),
            ),
            _ => {
                return;
            }
        };
        let body = json!({
            "op": "add",
            "graph": gid,
            "from": { "node": from.0, "port": from.1 },
            "to": { "node": to.0, "port": to.1 },
        });
        match door.json("POST", "/graph/edge", Some(body)) {
            Ok(reply) => {
                if let Some(id) = reply["edge"].as_str().map(str::to_string) {
                    push_undo(state, Undo::Unedge { edge: id }, theme);
                }
                state.graph = Some(reply);
            }
            // Refused — a lint error the edge itself introduced (not just
            // one already standing elsewhere in the graph, since
            // `add_edge_checked` only rejects a *new* one). Silently
            // dropping the cable here left the operator with no idea why
            // nothing happened; the route's own message says exactly what
            // is wrong (`call::graph::edge_add`'s refusal text).
            Err(_) => {}
        }
    }
}

/// `GET /graph/hit` at the press point (rule 5) — the one call that proves
/// this file never re-derives what is under the pointer. A node hit starts
/// a drag (queued, not sent — see [`finish_drag`]); an edge hit only
/// selects; a port or empty hit clears the drag and, for empty, the
/// selection too. Wire-dragging a port into an edge (`graphDrag.kind ===
/// "wire"` in the old page, deleted with it) is not implemented here — registered as an
/// `S.U9b` leftover.
/// Ctrl+click on node `id`: toggles it in the multi-selection. The set starts
/// from the current selection (a lone node, or the picked list when it is
/// still valid), the node selection lands on the last id left (or nothing).
fn toggle_picked(state: &mut PanelState, selection: &mut Selection, id: &str) {
    let mut picked = match state.picked_nodes(selection).as_slice() {
        [] => match &*selection {
            Selection::Node(cur) => vec![cur.clone()],
            _ => Vec::new(),
        },
        many => many.to_vec(),
    };
    match picked.iter().position(|p| p == id) {
        Some(at) => {
            picked.remove(at);
        }
        None => picked.push(id.to_string()),
    }
    *selection = match picked.last() {
        Some(last) => Selection::Node(last.clone()),
        None => Selection::None,
    };
    state.picked = if picked.len() >= 2 { picked } else { Vec::new() };
}

fn press_hit_test(
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    rect: EguiRect,
    press_pos: Pos2,
    ctrl: bool,
) {
    state.drag = None;
    state.pan_drag = None;
    state.param_drag = None;
    let Some(gid) = state.selected_graph.clone() else {
        return;
    };
    let [gx, gy] = to_graph(rect.min, state.pan, state.zoom, press_pos);
    let hit = hit_at(door, state, &gid, gx, gy);
    // Ctrl+click on a node toggles it in the multi-selection and starts no
    // drag; every other press starts a fresh single selection.
    let hit_node = match hit.as_ref() {
        Some(h) if h["hit"] == "node" => h["node"].as_str(),
        _ => None,
    };
    if let (true, Some(id)) = (ctrl, hit_node) {
        toggle_picked(state, selection, id);
        return;
    }
    state.picked.clear();

    match hit.as_ref().and_then(|h| h["hit"].as_str()) {
        // A value field (N): the drag edits the held value. The range and
        // start value come from `GET /graph/io` — the door's own read, never
        // a second copy of the parameter in the panel.
        Some("param") => {
            let (Some(node), Some(name)) = (
                hit.as_ref().and_then(|h| h["node"].as_str()),
                hit.as_ref().and_then(|h| h["name"].as_str()),
            ) else {
                return;
            };
            *selection = Selection::Node(node.to_string());
            if let Some((min, max, value)) = param_bounds(door, &gid, node, name) {
                state.param_drag = Some(ParamDrag {
                    node: node.to_string(),
                    name: name.to_string(),
                    start_value: value,
                    start_pointer: [gx, gy],
                    cur_pointer: [gx, gy],
                    min,
                    max,
                    value,
                });
            }
        }
        // A colour swatch (2026-10-08): select the node and open the picker
        // seeded from the key the chip stands for.
        Some("swatch") => {
            let (Some(node), Some(name)) = (
                hit.as_ref().and_then(|h| h["node"].as_str()),
                hit.as_ref().and_then(|h| h["name"].as_str()),
            ) else {
                return;
            };
            *selection = Selection::Node(node.to_string());
            let rgb = node_record(state, node)
                .and_then(|n| swatch_rgb(&n, name))
                .unwrap_or([1.0, 1.0, 1.0]);
            state.colour_menu = Some(ColourMenu {
                node: node.to_string(),
                key: name.to_string(),
                rgb,
                at: press_pos,
            });
        }
        Some("node") => {
            let Some(id) = hit
                .as_ref()
                .and_then(|h| h["node"].as_str())
                .map(str::to_string)
            else {
                return;
            };
            let start_node_pos = find_node_pos(state, &id).unwrap_or([gx, gy]);
            *selection = Selection::Node(id.clone());
            state.drag = Some(Drag {
                node: id,
                start_node_pos,
                start_pointer: [gx, gy],
                cur_pointer: [gx, gy],
            });
        }
        Some("edge") => {
            if let Some(id) = hit.as_ref().and_then(|h| h["edge"].as_str()) {
                *selection = Selection::Edge(id.to_string());
            }
        }
        Some("port") => {
            // A port press is a cable stretch, not a node drag: the reply
            // names the endpoint (`node`/`port`) and where the port sat in
            // graph space (`point`), both the door's own words. Nothing is
            // selected or cleared — the cable decides its own fate on
            // release, the same press-and-resolve every press used.
            if let (Some(n), Some(pt_port), Some(dir), Some(point)) = (
                hit.as_ref().and_then(|h| h["node"].as_str()),
                hit.as_ref().and_then(|h| h["port"].as_str()),
                hit.as_ref().and_then(|h| h["dir"].as_str()),
                hit.as_ref().and_then(|h| h["point"].as_array()).and_then(
                    |a| -> Option<[f32; 2]> {
                        let x = a.first()?.as_f64()? as f32;
                        let y = a.get(1)?.as_f64()? as f32;
                        Some([x, y])
                    },
                ),
            ) {
                state.cable = Some(CableDrag {
                    from_node: n.to_string(),
                    from_port: pt_port.to_string(),
                    from_point: point,
                    from_is_out: dir == "out",
                    cur_screen: press_pos,
                });
            }
        }
        // Empty canvas: the press clears the selection *and* becomes an
        // empty-space pan-drag — the "drag to pan" gesture
        // (`PanDrag`): the view transform follows the pointer while it is
        // down, releases commit nothing.
        _ => {
            *selection = Selection::None;
            state.pan_drag = Some(PanDrag {
                start_screen: press_pos,
                start_pan: state.pan,
            });
        }
    }
}

/// On release: a drag that never crossed [`DRAG_SLOP`] was a click, already
/// handled by [`press_hit_test`] selecting on press — nothing left to send.
/// One that did move sends exactly one `POST /graph/node { op: "move" }`,
/// against the node's position *before* the drag plus the accumulated
/// delta — never against wherever the last frame's pointer happened to be —
/// the same "compare against where it started" fix the old page's own
/// `mouseup` handler documents (read-only reference).
fn finish_drag(door: &dyn Call, state: &mut PanelState, selection: &mut Selection, theme: &Theme) {
    let Some(drag) = state.drag.take() else {
        return;
    };
    let Some(gid) = state.selected_graph.clone() else {
        return;
    };
    let dx = drag.cur_pointer[0] - drag.start_pointer[0];
    let dy = drag.cur_pointer[1] - drag.start_pointer[1];
    if dx.abs() <= DRAG_SLOP && dy.abs() <= DRAG_SLOP {
        return;
    }
    push_undo(
        state,
        Undo::Move {
            node: drag.node.clone(),
            pos: drag.start_node_pos,
        },
        theme,
    );
    let dropped = [drag.start_node_pos[0] + dx, drag.start_node_pos[1] + dy];
    let size = [metal_graph::layout::NODE_W, node_box_h(state, drag.start_node_pos)];
    let pos = json!(clear_of_others(dropped, size, &other_boxes(state, drag.start_node_pos)));
    let body = json!({ "op": "move", "graph": gid, "node": drag.node, "pos": pos });
    apply_node_op_locally(state, &body);
    door.post_async("/graph/node", body);
    *selection = Selection::Node(drag.node);
}

/// Breathing room a dropped node keeps from the one it was pushed off.
const DROP_GAP: f32 = 16.0;

/// Every laid-out node box `[x, y, w, h]` except the one whose top-left is
/// `skip` (the node being dragged).
fn other_boxes(state: &PanelState, skip: [f32; 2]) -> Vec<[f32; 4]> {
    state
        .graph
        .as_ref()
        .and_then(|g| g["primitives"].as_array())
        .map(|prims| {
            prims
                .iter()
                .filter(|p| p["tag"] == "node" && [num(p, "x"), num(p, "y")] != skip)
                .map(|p| [num(p, "x"), num(p, "y"), num(p, "w"), num(p, "h")])
                .collect()
        })
        .unwrap_or_default()
}

/// Nodes collide on drop: a box of `size` dropped at `pos` that lands on
/// another is pushed just below it (plus [`DROP_GAP`]), again until it
/// overlaps nothing — so no node can end up hidden under another (the
/// 2026-09-30 video: two gens stacked on one spot read as one node).
fn clear_of_others(mut pos: [f32; 2], size: [f32; 2], others: &[[f32; 4]]) -> [f32; 2] {
    let hits = |p: [f32; 2], o: &[f32; 4]| {
        p[0] < o[0] + o[2] && p[0] + size[0] > o[0] && p[1] < o[1] + o[3] && p[1] + size[1] > o[1]
    };
    for _ in 0..=others.len() {
        match others.iter().find(|o| hits(pos, o)) {
            Some(o) => pos[1] = o[1] + o[3] + DROP_GAP,
            None => break,
        }
    }
    pos
}

fn find_node_pos(state: &PanelState, id: &str) -> Option<[f32; 2]> {
    let nodes = state.graph.as_ref()?["graph"]["nodes"].as_array()?;
    let n = nodes.iter().find(|n| n["id"].as_str() == Some(id))?;
    let arr = n["pos"].as_array()?;
    Some([arr.first()?.as_f64()? as f32, arr.get(1)?.as_f64()? as f32])
}

/// `(min, max, value)` of parameter `name` on `node`, read from
/// `GET /graph/io`'s `params` — the door's own read of the node's
/// parameters, so the panel never keeps a second copy of a range. `value` is
/// the current effective value (bound, held or driven); `min` when the engine
/// answered none.
fn param_bounds(door: &dyn Call, gid: &str, node: &str, name: &str) -> Option<(f32, f32, f32)> {
    let io = get(door, "GET", "/graph/io", &[("id", gid.to_string()), ("node", node.to_string())])?;
    let p = io["params"]
        .as_array()?
        .iter()
        .find(|p| p["name"].as_str() == Some(name))?;
    let min = p["min"].as_f64().unwrap_or(0.0) as f32;
    let max = p["max"].as_f64().unwrap_or(1.0) as f32;
    let value = p["live"]
        .as_f64()
        .or_else(|| p["value"].as_f64())
        .map(|v| v as f32)
        .unwrap_or(min);
    Some((min, max, value))
}

/// Finalises a value-field drag (N): one `POST /graph/node { op: "param" }`
/// with the value the drag produced — never a stream while the pointer moved.
/// A drag that never crossed [`PARAM_DRAG_SLOP`] was a click that already
/// selected the row, so nothing is sent. The inverse records the value the row
/// held before the edit, so `CTRL + Z` restores it through the same op.
fn finish_param_drag(door: &dyn Call, state: &mut PanelState) {
    let Some(drag) = state.param_drag.take() else {
        return;
    };
    let Some(gid) = state.selected_graph.clone() else {
        return;
    };
    let dx = drag.cur_pointer[0] - drag.start_pointer[0];
    if dx.abs() <= PARAM_DRAG_SLOP {
        return;
    }
    let body = json!({
        "op": "param",
        "graph": gid,
        "node": drag.node,
        "name": drag.name,
        "value": drag.value,
    });
    door.post_async("/graph/node", body);
}

/// A cable dropped from an output onto a parameter row (or a param dot
/// dragged onto an output) is a **binding** (N). The producer's variables on
/// that port are read from the **consumer's** `GET /graph/io` `sources` (a
/// producer wired straight in appears there at hops 1); when the port carries
/// one float the bind happens at once, when it carries several the variable
/// chooser opens so the operator picks which one. Nothing posts until a pick.
fn bind_from_output(
    door: &dyn Call,
    state: &mut PanelState,
    consumer: &str,
    param: &str,
    producer: &str,
    port: &str,
    at: Pos2,
) {
    let Some(gid) = state.selected_graph.clone() else { return };
    // The consumer's sources carry every upstream variable, nearest first —
    // the port's own variables are the ones whose `port` matches, a `port_ref`
    // of `<port>.<var>`.
    let vars: Vec<(String, String, String)> = get(
        door,
        "GET",
        "/graph/io",
        &[("id", gid.clone()), ("node", consumer.to_string())],
    )
    .and_then(|io| {
        io["sources"].as_array().and_then(|sources| {
            sources
                .iter()
                .find(|s| s["node"].as_str() == Some(producer))
                .and_then(|s| s["vars"].as_array().cloned())
        })
    })
    .map(|vars| {
        vars.iter()
            .filter(|v| v["port"].as_str() == Some(port) && v["kind"].as_str() == Some("float"))
            .filter_map(|v| {
                Some((
                    v["port"].as_str()?.to_string(),
                    v["name"].as_str()?.to_string(),
                    v["port_ref"].as_str()?.to_string(),
                ))
            })
            .collect()
    })
    .unwrap_or_default();

    match vars.len() {
        0 => {
        }
        1 => {
            let (p, var, _) = &vars[0];
            bind_param(door, state, consumer, param, Some((producer, p, var)));
        }
        _ => {
            state.bind_menu = Some(BindMenu {
                node: consumer.to_string(),
                param: param.to_string(),
                producer: producer.to_string(),
                vars,
                at,
            });
        }
    }
}

/// One `POST /graph/node { op: "bind" }` (N): binds `node`'s parameter
/// `param` to `from = (producer, port, var)`, or unbinds it with `None`. The
/// route's own writer (`metal_graph::bind`) replaces whatever drove it and
/// refuses a cycle or an unknown variable by name; a refusal lands on the
/// panel's note line.
fn bind_param(
    door: &dyn Call,
    state: &mut PanelState,
    node: &str,
    param: &str,
    from: Option<(&str, &str, &str)>,
) {
    let Some(gid) = state.selected_graph.clone() else {
        return;
    };
    let from_json = match from {
        Some((p, port, var)) => json!({ "node": p, "port": port, "var": var }),
        None => Value::Null,
    };
    let body = json!({ "op": "bind", "graph": gid, "node": node, "param": param, "from": from_json });
    match door.json("POST", "/graph/node", Some(body)) {
        Ok(reply) => {
            state.graph = Some(reply);
        }
        Err(_) => {}
    }
}

// ---------------------------------------------------------------------------
// undo: record the inverse before each edit, replay it on CTRL + Z
// ---------------------------------------------------------------------------

/// Record one inverse edit, bounded at [`Theme::graph_undo_depth`] (a graph
/// edit is a disk write, so the stack is not a `Vec` growing to the
/// session's length): the oldest inverse falls off first.
fn push_undo(state: &mut PanelState, inverse: Undo, theme: &Theme) {
    state.undo.push(inverse);
    while state.undo.len() > theme.graph_undo_depth {
        state.undo.remove(undo_slot_first());
    }
}

fn undo_slot_first() -> usize {
    0
}

/// Pop and replay one inverse edit through the door — the same routes the
/// forward edits went through (`POST /graph/node`, `POST /graph/edge`),
/// refreshing the cached graph from whatever each route answers. A dry
/// stack is a no-op: the button is simply pressed and nothing happens.
pub(crate) fn undo_last(door: &dyn Call, state: &mut PanelState) {
    let Some(inverse) = state.undo.pop() else {
        return;
    };
    let Some(gid) = state.selected_graph.clone() else {
        return;
    };
    match inverse {
        Undo::Move { node, pos } => {
            let body = json!({ "op": "move", "graph": gid, "node": node, "pos": json!(pos) });
            apply_node_op_locally(state, &body);
            door.post_async("/graph/node", body);
        }
        Undo::Remove { node } => {
            let body = json!({ "op": "remove", "graph": gid, "node": node });
            door.post_async("/graph/node", body);
        }
        Undo::Add {
            kind,
            pos,
            bypass,
            solo,
        } => {
            let body = json!({ "op": "add", "graph": gid, "kind": kind, "pos": json!(pos) });
            if let Ok(reply) = door.json("POST", "/graph/node", Some(body)) {
                if let Some(id) = reply["node"].as_str().map(str::to_string) {
                    for (field, on) in [("bypass", bypass), ("solo", solo)] {
                        if on {
                            let body = json!({ "op": "flag", "graph": gid, "node": id, "field": field, "on": true });
                            if let Ok(r) = door.json("POST", "/graph/node", Some(body)) {
                                state.graph = Some(r);
                            }
                        }
                    }
                }
                state.graph = Some(reply);
            }
        }
        Undo::Edge { from, to } => {
            let body = json!({
                "op": "add",
                "graph": gid,
                "from": { "node": from.0, "port": from.1 },
                "to": { "node": to.0, "port": to.1 },
            });
            if let Ok(reply) = door.json("POST", "/graph/edge", Some(body)) {
                state.graph = Some(reply);
            }
        }
        Undo::Unedge { edge } => {
            let body = json!({ "op": "remove", "graph": gid, "edge": edge });
            if let Ok(reply) = door.json("POST", "/graph/edge", Some(body)) {
                state.graph = Some(reply);
            }
        }
        Undo::Flag { node, field, was } => {
            let body =
                json!({ "op": "flag", "graph": gid, "node": node, "field": field, "on": was });
            apply_node_op_locally(state, &body);
            door.post_async("/graph/node", body);
        }
    }
}

fn flag_node(
    door: &dyn Call,
    state: &mut PanelState,
    gid: &str,
    id: &str,
    field: &str,
    theme: &Theme,
) {
    let was = node_flag(state, id, field).unwrap_or(false);
    push_undo(
        state,
        Undo::Flag {
            node: id.to_string(),
            field: field.to_string(),
            was,
        },
        theme,
    );
    let body = json!({ "op": "flag", "graph": gid, "node": id, "field": field });
    apply_node_op_locally(state, &body);
    door.post_async("/graph/node", body);
}

/// The canvas's own write of a `gen.*` node's `mode` (2026-10-08) — the same
/// `config` merge the properties page posts, so the bar and the panel are one
/// route (rule 6).
fn set_node_mode(door: &dyn Call, state: &mut PanelState, gid: &str, id: &str, mode: &str) {
    let body = json!({
        "op": "config", "graph": gid, "node": id, "config": { "mode": mode },
    });
    door.post_async("/graph/node", body);
}

/// Applies a `move` or `flag` body to the open graph at once, so the canvas
/// shows it before the writer has run it. The writer's read-back replaces
/// this copy when the write has settled. A `flag` without `on` toggles.
fn apply_node_op_locally(state: &mut PanelState, body: &Value) {
    let Some(id) = body["node"].as_str() else {
        return;
    };
    let Some(node) = state
        .graph
        .as_mut()
        .and_then(|g| g["graph"]["nodes"].as_array_mut())
        .and_then(|nodes| nodes.iter_mut().find(|n| n["id"].as_str() == Some(id)))
    else {
        return;
    };
    match body["op"].as_str() {
        Some("move") => node["pos"] = body["pos"].clone(),
        Some("flag") => {
            let Some(field) = body["field"].as_str() else {
                return;
            };
            let on = body["on"].as_bool().unwrap_or_else(|| !node[field].as_bool().unwrap_or(false));
            node[field] = json!(on);
        }
        _ => {}
    }
}

fn node_flag(state: &PanelState, id: &str, field: &str) -> Option<bool> {
    let n = node_record(state, id)?;
    n[field].as_bool()
}

/// Is `point` (graph space) part of the node whose top-left is `node_pos`
/// — its box, or a port on its rim? A small pad (`PORT_R`, a port's own
/// radius) covers a port dot's centre sitting exactly on the box's edge,
/// so a `NodeKind` with ports on the left column still reads as inside.
/// `node_h` is the dragged box's own laid-out height (header + one `ROW_H`
/// per row), not `NODE_H`: a node with parameter rows is taller than the
/// bare header, and measuring it by `NODE_H` left its rows behind while the
/// header moved — they only caught up on release (2026-09-30 video).
fn in_dragged_node(point: [f32; 2], node_pos: [f32; 2], node_h: f32) -> bool {
    let pad = metal_graph::layout::PORT_R;
    point[0] >= node_pos[0] - pad
        && point[0] <= node_pos[0] + metal_graph::layout::NODE_W + pad
        && point[1] >= node_pos[1] - pad
        && point[1] <= node_pos[1] + node_h + pad
}

/// The live drag preview's input: the dragged node's `pos` before the drag,
/// the pointer delta so far, and the node box's laid-out height.
type DragOffset = ([f32; 2], [f32; 2], f32);

/// The laid-out height of the node box whose top-left is `pos` — read off
/// the door's own `node` primitive, `NODE_H` if none matches.
fn node_box_h(state: &PanelState, pos: [f32; 2]) -> f32 {
    state
        .graph
        .as_ref()
        .and_then(|g| g["primitives"].as_array())
        .and_then(|prims| {
            prims
                .iter()
                .find(|p| p["tag"] == "node" && [num(p, "x"), num(p, "y")] == pos)
                .map(|p| num(p, "h"))
        })
        .unwrap_or(metal_graph::layout::NODE_H)
}

fn add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

/// `point` translated by the drag's delta if `membership_point` falls
/// inside the dragged node's box — `point` and `membership_point` differ
/// for a port dot (whose drawn corner is offset from its own centre) but
/// are the same value for a node box or a label anchor.
fn drag_shift(
    point: [f32; 2],
    membership_point: [f32; 2],
    drag: Option<DragOffset>,
) -> [f32; 2] {
    match drag {
        Some((node_pos, delta, node_h)) if in_dragged_node(membership_point, node_pos, node_h) => add(point, delta),
        _ => point,
    }
}

/// The tile `uv` of the `gen.*` node whose `pos` is exactly `pos` — the
/// join between a layout rect (whose `x`/`y` *are* the node's `pos`,
/// `metal_graph::layout`) and the node record it came from. `None` for any
/// other family or a model the renderer does not list.
fn node_tile_at(state: &PanelState, pos: [f32; 2]) -> Option<EguiRect> {
    let nodes = state.graph.as_ref()?["graph"]["nodes"].as_array()?;
    let node = nodes.iter().find(|n| pt(n, "pos") == pos)?;
    let model = node["kind"].as_str()?.strip_prefix("gen.")?;
    let index = state.model_names.iter().position(|n| n == model)?;
    Some(tile_uv(index, state.model_names.len()))
}

/// The id of the `gen.*` node whose `pos` is exactly `pos` (see [`node_tile_at`]).
fn node_id_at(state: &PanelState, pos: [f32; 2]) -> Option<String> {
    let nodes = state.graph.as_ref()?["graph"]["nodes"].as_array()?;
    let node = nodes.iter().find(|n| pt(n, "pos") == pos)?;
    node["kind"].as_str()?.starts_with("gen.").then(|| node["id"].as_str().map(str::to_string))?
}

fn node_record(state: &PanelState, id: &str) -> Option<Value> {
    let nodes = state.graph.as_ref()?["graph"]["nodes"].as_array()?;
    nodes.iter().find(|n| n["id"].as_str() == Some(id)).cloned()
}

fn duplicate_node(
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    gid: &str,
    id: &str,
    theme: &Theme,
) {
    let body = json!({ "op": "duplicate", "graph": gid, "node": id });
    if let Ok(reply) = door.json("POST", "/graph/node", Some(body)) {
        if let Some(new_id) = reply["node"].as_str().map(str::to_string) {
            push_undo(
                state,
                Undo::Remove {
                    node: new_id.clone(),
                },
                theme,
            );
            *selection = Selection::Node(new_id);
        }
        state.graph = Some(reply);
    }
}

fn remove_node(
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    gid: &str,
    id: &str,
    theme: &Theme,
) {
    // The inverse is read off the cached graph *before* the remove — once
    // the reply lands, the node this closure needs to re-create is gone.
    if let Some(n) = node_record(state, id) {
        push_undo(
            state,
            Undo::Add {
                kind: n["kind"].as_str().unwrap_or_default().to_string(),
                pos: find_node_pos(state, id).unwrap_or([0.0, 0.0]),
                bypass: n["bypass"].as_bool().unwrap_or(false),
                solo: n["solo"].as_bool().unwrap_or(false),
            },
            theme,
        );
    }
    let body = json!({ "op": "remove", "graph": gid, "node": id });
    door.post_async("/graph/node", body);
    *selection = Selection::None;
}

fn remove_edge(
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    gid: &str,
    id: &str,
    theme: &Theme,
) {
    let endpoints = edges_of(state)
        .into_iter()
        .find(|e| e["id"].as_str() == Some(id))
        .map(|e| {
            let v = |k: &str, w: &str| e[k][w].as_str().unwrap_or_default().to_string();
            Undo::Edge {
                from: (v("from", "node"), v("from", "port")),
                to: (v("to", "node"), v("to", "port")),
            }
        });
    if let Some(inverse) = endpoints {
        push_undo(state, inverse, theme);
    }
    let body = json!({ "op": "remove", "graph": gid, "edge": id });
    if let Ok(reply) = door.json("POST", "/graph/edge", Some(body)) {
        state.graph = Some(reply);
    }
    *selection = Selection::None;
}

fn edges_of(state: &PanelState) -> Vec<Value> {
    state
        .graph
        .as_ref()
        .and_then(|g| g["graph"]["edges"].as_array().cloned())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// the door, GET with a query
// ---------------------------------------------------------------------------

/// [`Call::json`] only builds a JSON *body* — every `GET` route this panel
/// needs (`/graph?id=`, `/graph/hit?id=&x=&y=`)
/// takes its arguments as a query string instead, so this file builds
/// [`Params`] directly and calls [`Call::call`] itself, exactly the surface
/// gate 1 allows (`Params`/`Call` are both named in the door's public
/// surface, `crate::call`'s top-level re-exports).
/// What is under graph-space `(x, y)` on the open graph `gid`. Answered on
/// this thread from the `GET /graph` reply the canvas paints
/// (`shell::graph_hit::local_hit`, the server's own rule), so a press costs no
/// round trip. `GET /graph/hit` asks the door when that reply is not the open
/// graph, or does not describe it.
fn hit_at(door: &dyn Call, state: &PanelState, gid: &str, gx: f32, gy: f32) -> Option<Value> {
    state
        .graph
        .as_ref()
        .filter(|g| g["graph"]["id"].as_str() == Some(gid))
        .and_then(|g| crate::shell::graph_hit::local_hit(g, gx, gy))
        .or_else(|| {
            get(
                door,
                "GET",
                "/graph/hit",
                &[("id", gid.to_string()), ("x", gx.to_string()), ("y", gy.to_string())],
            )
        })
}

fn get(door: &dyn Call, method: &str, path: &str, query: &[(&str, String)]) -> Option<Value> {
    let mut q = HashMap::new();
    for (k, v) in query {
        q.insert((*k).to_string(), v.clone());
    }
    let params = Params {
        query: q,
        body: Vec::new(),
        tail: None,
    };
    match door.call(method, path, params) {
        Some(reply) if reply.status == 200 => serde_json::from_slice(&reply.body).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
        use crate::shell::components::{glyph, Icon};
    /// The gallery fetches only while it is open AND a card is on screen,
    /// one variant per slot round-robin; each variant comes once per cycle.
    #[test]
    fn gallery_fetches_follow_visibility_and_cycle_the_four_variants() {
        assert_eq!(variant_to_fetch(0.0, true, false), None, "no card on screen, no fetch");
        assert_eq!(variant_to_fetch(0.0, false, true), None, "closed, no fetch");
        let slots: Vec<Option<usize>> = (0..8).map(|n| variant_to_fetch(n as f64 * 0.25, true, true)).collect();
        assert_eq!(slots, [0, 1, 2, 3, 0, 1, 2, 3].map(Some).to_vec());
        assert_eq!(variant_to_fetch(0.1, true, true), Some(0), "still in slot 0");
    }

    /// Each visible fx/mat kind's variant is fetched once: the next slot picks
    /// one that has not landed, round-robin, and nothing once all have.
    #[test]
    fn kind_previews_fetch_each_missing_variant_lazily() {
        let kinds = vec!["fx.posterize".to_string(), "gen.rings".to_string(), "mat.wood".to_string()];
        assert_eq!(kind_to_fetch(0.0, &[], |_, _| false), None, "no card on screen, no fetch");
        assert_eq!(kind_to_fetch(0.0, &kinds[1..2], |_, _| false), None, "a model has no kind preview");
        let mut seen = Vec::new();
        for slot in 0..8 {
            seen.push(kind_to_fetch(slot as f64 * 0.25, &kinds, |_, _| false).unwrap());
        }
        assert_eq!(seen[0], ("fx.posterize".to_string(), 0));
        assert_eq!(seen[1], ("fx.posterize".to_string(), 1), "the next slot takes the next variant");
        assert_eq!(seen[4], ("mat.wood".to_string(), 0), "after the effect's four, the material's");
        assert!(kind_to_fetch(0.0, &kinds, |_, _| true).is_none(), "landed, nothing to fetch");
        let only_mat = |k: &str, v: usize| !(k == "fx.posterize" && v == 2);
        assert_eq!(kind_to_fetch(0.0, &kinds, only_mat).unwrap().0, "fx.posterize", "the one missing variant is asked");
    }


    #[test]
    fn a_new_node_lands_at_the_centre_of_the_visible_canvas() {
        // Canvas at (10, 20), 200x100 on screen; no pan, zoom 1: the centre is
        // screen (110, 70), i.e. graph (100, 50) from the canvas origin.
        let canvas = EguiRect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(200.0, 100.0));
        assert_eq!(new_node_spot(canvas, Vec2::ZERO, 1.0), [100.0, 50.0]);
    }

    #[test]
    fn a_new_node_follows_pan_and_zoom() {
        // Panned 40 right and 30 down, zoom 2: the centre (110, 70) sits at
        // graph ((110 - 10 - 40) / 2, (70 - 20 - 30) / 2) = (30, 10).
        let canvas = EguiRect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(200.0, 100.0));
        assert_eq!(new_node_spot(canvas, Vec2::new(40.0, 30.0), 2.0), [30.0, 10.0]);
    }

    #[test]
    fn a_new_node_maps_back_to_the_canvas_centre() {
        let canvas = EguiRect::from_min_size(Pos2::new(5.0, 5.0), Vec2::new(640.0, 480.0));
        let (pan, zoom) = (Vec2::new(-123.0, 77.0), 1.5);
        let [gx, gy] = new_node_spot(canvas, pan, zoom);
        assert!((to_screen(canvas.min, pan, zoom, [gx, gy]) - canvas.center()).length() < 1e-3);
    }
    use super::*;
    use crate::shell::{FakeDoor, Shell};
    use egui_kittest::kittest::Queryable;

    /// A `color` node's swatch pick posts `rgb` (+ h/s/b) — the canvas's own
    /// colour write, the same `config` merge the properties panel posts.
    #[test]
    fn a_swatch_pick_posts_the_colour_config() {
        let door = FakeDoor::new();
        let mut state = PanelState::default();
        state.selected_graph = Some("g1".to_string());
        post_swatch(&door, &mut state, "color-0", "rgb", [1.0, 0.0, 0.0]);
        let posted = door.calls.borrow().iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/graph/node"
                && serde_json::from_slice::<Value>(&params.body)
                    .map(|b| {
                        b["op"] == "config"
                            && b["node"] == "color-0"
                            && b["config"]["rgb"] == json!([1.0, 0.0, 0.0])
                    })
                    .unwrap_or(false)
        });
        assert!(posted, "a swatch pick posts the colour config");
    }

    /// A `palette` node's slot pick replaces only that slot, keeping the
    /// others — key and secondary colours set on the node in the canvas.
    #[test]
    fn a_palette_slot_pick_replaces_only_that_slot() {
        let door = FakeDoor::new();
        let mut state = PanelState::default();
        state.selected_graph = Some("g1".to_string());
        state.graph = Some(json!({
            "graph": { "nodes": [ {
                "id": "pal-0",
                "config": { "colors": [[1.0, 1.0, 1.0], [0.0, 0.0, 0.0]] }
            } ] }
        }));
        post_swatch(&door, &mut state, "pal-0", "colors:1", [0.25, 0.5, 0.75]);
        let posted = door.calls.borrow().iter().find_map(|(m, p, params)| {
            if m == "POST" && p == "/graph/node" {
                serde_json::from_slice::<Value>(&params.body).ok()
            } else {
                None
            }
        });
        assert_eq!(
            posted.unwrap()["config"]["colors"],
            json!([[1.0, 1.0, 1.0], [0.25, 0.5, 0.75]])
        );
    }

    /// The canvas's own `mode` write (2026-10-08) posts the same `config`
    /// merge the properties page posts — one field, one route.
    #[test]
    fn the_canvas_modo_posts_the_config_merge() {
        let door = FakeDoor::new();
        let mut state = PanelState::default();
        set_node_mode(&door, &mut state, "g1", "gen-0", "manual");
        let posted = door.calls.borrow().iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/graph/node"
                && serde_json::from_slice::<Value>(&params.body)
                    .map(|b| {
                        b["op"] == "config"
                            && b["node"] == "gen-0"
                            && b["config"]["mode"] == "manual"
                    })
                    .unwrap_or(false)
        });
        assert!(posted, "the canvas mode posts the config merge");
    }

    /// The five sections cover the families the ask named, in the order it
    /// named them, and nothing double-books — a family answers to at most
    /// one section, and every family the enum has ends up either in a
    /// section or in `category_of`'s `None` (the leftover "list").
    #[test]
    fn every_category_family_is_unique_and_the_order_matches_the_ask() {
        assert_eq!(
            CATEGORIES,
            ["sources", "sensors", "effects", "materials", "math", "scripts", "outputs"],
            "the ask's own order, with math between materials and scripts"
        );
        assert_eq!(category_of("math"), Some("math"), "the oscillator/constant/random/noise family");
        let all = [
            "audio",
            "generator",
            "reel",
            "still",
            "text",
            "capture",
            "sensor",
            "effect",
            "script",
            "output_device",
            "output_signal",
            "export_video",
            "venue",
            "router",
            "camera_track",
            "mask",
            "compose",
            "stack",
            "math",
        ];
        let mut seen: Vec<&str> = Vec::new();
        for fam in all {
            if let Some(cat) = category_of(fam) {
                assert!(
                    CATEGORIES.contains(&cat),
                    "{fam} maps to {cat}, not one of CATEGORIES"
                );
            }
            assert!(!seen.contains(&fam), "{fam} listed twice in this test");
            seen.push(fam);
        }
        assert_eq!(
            category_of("router"),
            None,
            "router stays in the leftover list, not a section"
        );
    }

    /// A point inside a node's box, or a port sitting right on its rim, both
    /// read as that node's; a point a whole node-width away does not.
    #[test]
    fn in_dragged_node_covers_the_box_and_its_own_ports_only() {
        let pos = [100.0, 40.0];
        assert!(
            in_dragged_node([pos[0] + 1.0, pos[1] + 1.0], pos, metal_graph::layout::NODE_H),
            "inside the box"
        );
        assert!(
            in_dragged_node([pos[0] - metal_graph::layout::PORT_R, pos[1]], pos, metal_graph::layout::NODE_H),
            "a port dot right on the left edge"
        );
        assert!(
            !in_dragged_node([pos[0] + metal_graph::layout::NODE_W * 2.0, pos[1]], pos, metal_graph::layout::NODE_H),
            "a whole node-width away is a different node"
        );
    }

    /// Every focus moves the picture: a generator shows itself, the output
    /// its composed frame, an effect its own isolated stage, and a node
    /// with no image upstream nothing (the 2026-09-30 video: the thumbnail
    /// never changed as focus moved around the canvas).
    #[test]
    fn every_focus_picks_its_own_picture() {
        let mut s = PanelState::default();
        s.selected_graph = Some("device-HDMI-A-1".to_string());
        s.graph = Some(json!({ "graph": {
            "nodes": [
                { "id": "audio-0", "kind": "audio.live" },
                { "id": "gen-0", "kind": "gen.escape" },
                { "id": "gen-2", "kind": "gen.cyberspace" },
                { "id": "fx-1", "kind": "fx.ghost" },
                { "id": "out-0", "kind": "output.device" }
            ],
            "edges": [
                { "from": { "node": "audio-0", "port": "control" }, "to": { "node": "gen-2", "port": "control" } },
                { "from": { "node": "gen-2", "port": "image" }, "to": { "node": "fx-1", "port": "image" } },
                { "from": { "node": "fx-1", "port": "image" }, "to": { "node": "out-0", "port": "image" } }
            ]
        }}));
        let pick = |id: &str| selected_picture(&s, &Selection::Node(id.to_string()));
        let layer = |id: &str| pick(id).map(|(r, q)| (r, q.into_iter().find(|(k, _)| *k == "layer").map(|(_, v)| v)));
        assert_eq!(layer("gen-0"), Some(("/source.jpg", Some("gen-0".to_string()))));
        assert_eq!(layer("gen-2"), Some(("/source.jpg", Some("gen-2".to_string()))));
        assert_eq!(
            pick("fx-1"),
            Some(("/node.jpg", vec![("device", "HDMI-A-1".to_string()), ("graph", "device-HDMI-A-1".to_string()), ("node", "fx-1".to_string())])),
            "an effect shows its own isolated stage"
        );
        assert_eq!(
            pick("out-0"),
            Some(("/output.jpg", vec![("device", "HDMI-A-1".to_string())])),
            "the output shows its composed frame"
        );
        assert_eq!(pick("audio-0"), None, "no image upstream: no picture");
    }

    /// A drop onto another node slides below it — past a whole column if
    /// need be — and a drop onto empty canvas stays exactly where it fell.
    #[test]
    fn dropped_node_never_lands_on_another() {
        let size = [100.0, 60.0];
        let others = [[0.0, 0.0, 100.0, 60.0], [0.0, 76.0, 100.0, 80.0]];
        assert_eq!(clear_of_others([10.0, 10.0], size, &others), [10.0, 172.0]);
        assert_eq!(clear_of_others([500.0, 10.0], size, &others), [500.0, 10.0]);
    }

    /// A parameter row sits below `NODE_H` on a tall node; it must still
    /// read as the dragged node's, or the rows lag behind the header until
    /// release (the 2026-09-30 video: header moved, body stayed put).
    #[test]
    fn in_dragged_node_covers_rows_below_the_header() {
        use metal_graph::layout::{NODE_H, ROW_H};
        let pos = [0.0, 0.0];
        let tall = NODE_H + ROW_H * 8.0;
        let row = [10.0, NODE_H + ROW_H * 5.5];
        assert!(in_dragged_node(row, pos, tall), "a row of a tall node moves with it");
        assert!(!in_dragged_node([10.0, tall + ROW_H * 3.0], pos, tall), "below the box does not");
    }

    /// The live drag preview only moves what belongs to the dragged node:
    /// a point inside its box shifts by the delta, a point outside it (the
    /// other end of a cable, say) is untouched.
    #[test]
    fn drag_shift_moves_only_the_dragged_nodes_own_geometry() {
        let node_pos = [0.0, 0.0];
        let delta = [12.0, -5.0];
        let inside = [10.0, 10.0];
        let outside = [900.0, 900.0];
        assert_eq!(
            drag_shift(inside, inside, Some((node_pos, delta, metal_graph::layout::NODE_H))),
            [22.0, 5.0],
            "a point inside the dragged node's box moves by the delta"
        );
        assert_eq!(
            drag_shift(outside, outside, Some((node_pos, delta, metal_graph::layout::NODE_H))),
            outside,
            "a point outside the dragged node's box does not move"
        );
        assert_eq!(
            drag_shift(inside, inside, None),
            inside,
            "no drag in flight: nothing moves"
        );
    }

    /// A zoom step keeps the point under the zoom anchor where it was —
    /// the whole point of zooming *at* a spot — and leaves the view
    /// transform invertible (`to_graph(to_screen(p)) == p` for any graph
    /// point), which is the precondition `press_hit_test` leans on: a view
    /// that had drifted off would send the door coordinates nobody clicked.
    #[test]
    fn zoom_view_keeps_the_anchor_still_and_stays_invertible() {
        let theme = Theme::phosphor();
        // Each number named apart from any `Vec2::new(` call so none of
        // them opens with a digit — gate 2 bans that shape outside
        // `classes.rs`, test code included.
        let (pan_x, pan_y, anchor_x, anchor_y) = (6.0, -3.0, 88.0, 44.0);
        for factor in [theme.graph_zoom_step, theme.graph_zoom_step.recip()] {
            let mut s = PanelState::default();
            s.pan = Vec2::new(pan_x, pan_y);
            let anchor = Pos2::ZERO + Vec2::new(anchor_x, anchor_y);
            let under = to_graph(Pos2::ZERO, s.pan, s.zoom, anchor);
            zoom_view(&mut s, factor, Some(anchor - Pos2::ZERO), &theme);
            let back = to_graph(Pos2::ZERO, s.pan, s.zoom, anchor);
            assert!(
                (back[0] - under[0]).abs() < 1e-4,
                "anchor point drifted in x"
            );
            assert!(
                (back[1] - under[1]).abs() < 1e-4,
                "anchor point drifted in y"
            );
            let screen = to_screen(Pos2::ZERO, s.pan, s.zoom, under);
            let again = to_graph(Pos2::ZERO, s.pan, s.zoom, screen);
            assert!((again[0] - under[0]).abs() < 1e-4 && (again[1] - under[1]).abs() < 1e-4);
            assert!(s.zoom >= theme.graph_zoom_min && s.zoom <= theme.graph_zoom_max);
        }
    }

    /// `Ctrl+Z` replays the last inverse through the same `POST /graph/node`
    /// route the forward edit used, and pops it off the stack.
    #[test]
    fn ctrl_z_replays_the_last_edit_through_the_door() {
        let door = FakeDoor::new();
        door.respond("POST", "/graph/node", json!({ "graph": { "id": "col-a", "nodes": [], "edges": [] } }));
        let theme = Theme::phosphor();
        let mut state = PanelState::default();
        state.selected_graph = Some("col-a".to_string());
        push_undo(&mut state, Undo::Move { node: "gen-0".to_string(), pos: [10.0, 20.0] }, &theme);
        undo_last(&door, &mut state);
        assert!(state.undo.is_empty(), "the inverse is popped");
        let posted = door.calls.borrow().iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/graph/node"
                && serde_json::from_slice::<Value>(&params.body)
                    .map(|b| b["op"] == "move" && b["node"] == "gen-0" && b["graph"] == "col-a")
                    .unwrap_or(false)
        });
        assert!(posted, "the move is put back on the collection");
    }

    /// A published device copy is not something the picker lists, and the
    /// canvas that opens one (a listen dot names `device-<screen>`) opens the
    /// collection it was published from instead.
    #[test]
    fn the_picker_lists_collections_and_a_device_copy_opens_its_collection() {
        assert!(is_pickable(&json!({ "id": "col-show", "published": false })));
        assert!(!is_pickable(&json!({ "id": "col-show", "published": true })));
        assert!(!is_pickable(&json!({ "id": "device-A", "published": false })), "a released device graph is not a collection");
        let copy = json!({ "graph": { "id": "device-A", "source": "col-show" } });
        assert_eq!(published_source("device-A", Some(&copy)).as_deref(), Some("col-show"));
        assert_eq!(published_source("col-show", Some(&json!({ "graph": { "id": "col-show" } }))), None);
        assert_eq!(published_source("device-A", None), None);
    }

    /// The undo stack stays bounded at the theme's depth — the oldest
    /// inverse falls off, never a flat `Vec` growing for a session's worth
    /// of disk writes.
    #[test]
    fn push_undo_is_bounded() {
        let theme = Theme::phosphor();
        let mut s = PanelState::default();
        let runs = theme.graph_undo_depth * usize::from(3u8);
        for i in 0..runs {
            push_undo(
                &mut s,
                Undo::Move {
                    node: format!("n-{i}"),
                    pos: [0.0, 0.0],
                },
                &theme,
            );
        }
    }

    #[test]
    fn to_screen_and_to_graph_round_trip() {
        // Named apart from the `Vec2::new(`/`Pos2::new(` calls below so
        // none of them opens with a digit — gate 2 (`shell/gates.rs`) bans
        // exactly that shape outside `classes.rs`, test code included.
        let (origin_x, origin_y) = (40.0, 20.0);
        let (pan_x, pan_y) = (10.0, -5.0);
        let origin = Pos2::new(origin_x, origin_y);
        let pan = Vec2::new(pan_x, pan_y);
        let zoom = 2.0;
        let p = [30.0, 44.0];
        let screen = to_screen(origin, pan, zoom, p);
        let back = to_graph(origin, pan, zoom, screen);
        assert!((back[0] - p[0]).abs() < 1e-4);
        assert!((back[1] - p[1]).abs() < 1e-4);
    }

    #[test]
    fn chip_label_shows_local_when_neither_node_nor_ceded_to_is_set() {
        assert_eq!(chip_label("screen-1", None, None), "screen-1 · local");
    }

    #[test]
    fn chip_label_shows_the_node_when_nobody_holds_it_ceded() {
        assert_eq!(
            chip_label("screen-1", Some("otro-pc"), None),
            "screen-1 · on otro-pc"
        );
    }

    /// Ceded wins over where it is shown — the order the operator cares
    /// about, per this helper's own doc comment.
    #[test]
    fn chip_label_prefers_ceded_to_over_node_when_both_are_set() {
        assert_eq!(
            chip_label("screen-1", Some("otro-pc"), Some("resolume")),
            "screen-1 · handed to resolume"
        );
    }

    /// The same harness `properties.rs`/`lanes.rs` each build beside their
    /// own panel (`properties.rs::tests::shell_harness`) — this module had
    /// none before the master row landed, since every earlier test here was
    /// a plain pure-function test. Sized to the desktop viewport, which is
    /// enough: `master_row` paints right after `canvas_bar`, well above the
    /// panel's own fold.
    fn shell_harness(door: Box<dyn Call>) -> egui_kittest::Harness<'static, Shell> {
        let theme = Theme::phosphor();
        let size = egui::Vec2::new(
            crate::shell::DESKTOP_VIEWPORT.0 as f32,
            crate::shell::DESKTOP_VIEWPORT.1 as f32,
        );
        let harness = egui_kittest::Harness::builder()
            .with_size(size)
            .build_ui_state(
                |ui, shell: &mut Shell| shell.ui(ui),
                Shell::new(theme.clone(), door),
            );
        theme.apply(&harness.ctx);
        harness
    }

    /// The floating preview's pump always schedules the next fetch
    /// (`frames.rs`'s `request_repaint_after` duty), so a harness that shows
    /// one never reaches a repaint-free frame and `Harness::run` hits its
    /// `max_steps` panic. Bound the frames with `run_steps` instead — the
    /// same call `kittest::run_real` makes for a real door.
    fn settle(harness: &mut egui_kittest::Harness<'static, Shell>) {
        harness.run_steps(3);
    }

    /// O.19's control: an output nobody has ceded shows `cede`, and
    /// clicking it posts `POST /outputs {op: "cede", id, to: "otro
    /// programa"}` — `master_row`'s own body for the not-yet-ceded case.
    #[test]
    fn clicking_cede_posts_outputs_cede_with_id_and_to_otro_programa() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/state",
            json!({ "outputs": [{
                "id": "screen-1", "label": "screen-1", "width": 1920, "height": 1080,
                "calibrating": false, "mode": { "kind": "automix" }, "layers": [],
                "masks": [], "node": "otro-pc", "composed_on": null
            }] }),
        );
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        harness.get_by_label("hand over").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls.iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/outputs"
                && serde_json::from_slice::<Value>(&params.body)
                    .map(|b| b == json!({ "op": "cede", "id": "screen-1", "to": "otro programa" }))
                    .unwrap_or(false)
        });
        assert!(
            posted,
            "clicking cede should POST /outputs {{op: cede, id, to: \"otro programa\"}}"
        );
    }

    /// The other half: an output already ceded shows `take back` instead, and
    /// clicking it posts the same op with `to: null` — taking it back.
    #[test]
    fn clicking_take_back_posts_outputs_cede_with_to_null() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/state",
            json!({ "outputs": [{
                "id": "screen-1", "label": "screen-1", "width": 1920, "height": 1080,
                "calibrating": false, "mode": { "kind": "automix" }, "layers": [],
                "masks": [], "node": "otro-pc", "composed_on": null, "ceded_to": "otro programa"
            }] }),
        );
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        harness.get_by_label("take back").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls.iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/outputs"
                && serde_json::from_slice::<Value>(&params.body)
                    .map(|b| b == json!({ "op": "cede", "id": "screen-1", "to": null }))
                    .unwrap_or(false)
        });
        assert!(
            posted,
            "clicking take back should POST /outputs {{op: cede, id, to: null}}"
        );
    }

    // -- R22.1: the floating preview ----------------------------------------

    /// One `gen.*` and one `fx.*` node in one cached graph — the two kinds
    /// the preview rule tells apart (a generator has its own animation; an
    /// effect has only its place in the chain). The graph carries its
    /// `domain`, as a real `GET /graph` reply does: `+ node` filters its
    /// catalog by it (R22.5a).
    fn script_gen_and_effect_graph(recorder: &FakeDoor) {
        recorder.respond(
            "GET",
            "/graphs",
            json!({ "graphs": [{ "id": "empty-1", "name": "empty-1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            json!({
                "graph": { "id": "empty-1", "domain": "live", "nodes": [
                    { "id": "gen-0", "kind": "gen.turing", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false, "config": null },
                    { "id": "fx-0", "kind": "fx.shake", "pos": [1.0, 0.0], "label": "", "bypass": false, "solo": false, "config": null }
                ] },
                "primitives": [],
                "issues": []
            }),
        );
    }

    /// A `gen.*` node's tile is requested with the node's own row, not the
    /// model's studio row: its held float lands in the float's slot, the
    /// rest of the row stays the studio's.
    #[test]
    fn a_generator_tile_is_requested_with_the_nodes_own_row() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/graphs",
            json!({ "graphs": [{ "id": "empty-1", "name": "empty-1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            json!({
                "graph": { "id": "empty-1", "domain": "live", "nodes": [
                    { "id": "gen-0", "kind": "gen.turing", "pos": [0.0, 0.0], "label": "", "bypass": false,
                      "solo": false, "config": { "floats": { "scale": 7.5 } } }
                ] },
                "primitives": [],
                "issues": []
            }),
        );
        recorder.respond(
            "GET",
            "/author/render/state",
            json!({ "names": ["turing"], "rows": [[0.25, 1.0, 0.5, 0.75]] }),
        );
        recorder.respond(
            "GET",
            "/catalog",
            json!({ "catalog": [{ "kind": "gen.turing", "params": [
                { "name": "scale", "kind": "float", "index": 1 },
                { "name": "tint", "kind": "colour", "index": 2 }
            ] }] }),
        );
        recorder.respond_bytes(
            "GET",
            "/author/render/solo.jpg",
            "image/jpeg",
            crate::paint::encode_jpeg(&[0u8; 4 * 4 * 4], 4, 4),
        );
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        settle(&mut harness);
        let rows: Vec<String> = recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "GET" && p == "/author/render/solo.jpg")
            .filter_map(|(_, _, params)| params.query.get("row").cloned())
            .collect();
        assert!(!rows.is_empty(), "the node's tile is asked for");
        assert!(rows.iter().all(|r| r == "0.25,7.5,0.5,0.75"), "{rows:?}");
        let model = recorder
            .calls
            .borrow()
            .iter()
            .find(|(_, p, _)| p == "/author/render/solo.jpg")
            .and_then(|(_, _, params)| params.query.get("model").cloned());
        assert_eq!(model.as_deref(), Some("turing"));
    }

    fn source_jpg_calls(recorder: &FakeDoor) -> usize {
        recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "GET" && p == "/source.jpg")
            .count()
    }

    /// No selection, and a selection that is not a generator, both pump
    /// nothing: the preview is not merely hidden, the route is never asked
    /// (`R22.1` decision 1).
    #[test]
    fn no_preview_without_a_selected_generator() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        assert_eq!(source_jpg_calls(&recorder), 0, "no selection: nothing pumps");

        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("fx-0".to_string()));
        harness.run();
        assert_eq!(
            source_jpg_calls(&recorder),
            0,
            "an effect has no own animation to pump"
        );
    }

    /// A focused generator asks `GET /source.jpg` for *that node*: its id as
    /// the layer, its graph's device (minus the `device-` prefix) as the
    /// device, and the paint width.
    #[test]
    fn selecting_a_generator_asks_source_jpg_for_that_node() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("gen-0".to_string()));
        settle(&mut harness);

        let calls = recorder.calls.borrow();
        let preview = calls
            .iter()
            .find(|(m, p, _)| m == "GET" && p == "/source.jpg")
            .expect("a focused generator should ask /source.jpg");
        assert_eq!(
            preview.2.query.get("layer").map(String::as_str),
            Some("gen-0"),
            "the picture is the node's, not the device's composed frame"
        );
        assert_eq!(
            preview.2.query.get("device").map(String::as_str),
            Some("empty-1"),
            "the graph id is the device, minus the `device-` prefix it lacks"
        );
        assert!(preview.2.query.contains_key("w"), "the paint width travels");
    }

    /// Switching from a generator to an effect stops the pump: no new
    /// `/source.jpg` request, and nothing paints.
    #[test]
    fn selecting_an_effect_hides_the_preview() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("gen-0".to_string()));
        settle(&mut harness);
        let shown = source_jpg_calls(&recorder);
        assert!(shown > 0, "the generator pumps its own picture");

        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("fx-0".to_string()));
        settle(&mut harness);
        assert_eq!(
            source_jpg_calls(&recorder),
            shown,
            "an effect must not pump the preview"
        );
    }

    /// A click on the picture toggles it to the canvas's whole width and
    /// back — the same gesture the old `preview_pip` had, now the shared
    /// widget's.
    #[test]
    fn clicking_the_preview_toggles_big() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        // A 4×4 stand-in JPEG: the widget only paints once a route answered.
        let jpeg = crate::paint::encode_jpeg(&[0u8; 4 * 4 * 4], 4, 4);
        recorder.respond_bytes("GET", "/source.jpg", "image/jpeg", jpeg);
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        // A press and its release are one click however slow the frames are.
        harness.ctx.memory_mut(|m| {
            m.options.input_options.max_click_duration = 3600.0;
            m.options.input_options.max_double_click_delay = 3600.0;
        });
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("gen-0".to_string()));
        settle(&mut harness);
        assert!(!harness.state().nodes.preview.big(), "starts small");

        let point = harness
            .state()
            .nodes
            .preview
            .rect()
            .expect("the picture should have painted a rect")
            .left_top()
            + egui::Vec2::splat(f32::from(Theme::phosphor().pad));
        harness.drag_at(point);
        settle(&mut harness);
        harness.drop_at(point);
        settle(&mut harness);
        assert!(harness.state().nodes.preview.big(), "a click expands it");
    }

    /// P-G: a focused generator's picture carries a bar naming it (name ·
    /// kind, resolution, fps) whose camera button posts `POST /frame/export
    /// {device, node}` through the door and raises a toast with the path.
    #[test]
    fn the_preview_bar_names_the_node_and_exports_its_frame() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        let jpeg = crate::paint::encode_jpeg(&[0u8; 4 * 4 * 4], 4, 4);
        recorder.respond_bytes("GET", "/source.jpg", "image/jpeg", jpeg);
        recorder.respond("POST", "/frame/export", json!({ "path": "/tmp/x/gen-0-1.png" }));
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness
            .state_mut()
            .set_selection_for_test(Selection::Node("gen-0".to_string()));
        settle(&mut harness);

        use egui_kittest::kittest::NodeT as _;
        let readout = harness.get_by(|n| {
            n.role() == egui::accesskit::Role::Label && n.value().is_some_and(|l| l.starts_with("gen-0 · gen.turing · 4×4 · ") && l.ends_with(" fps"))
        });
        drop(readout);
        harness.get_by_label(&glyph(Icon::Camera).to_string()).click();
        settle(&mut harness);

        let calls = recorder.calls.borrow();
        let post = calls
            .iter()
            .find(|(m, p, _)| m == "POST" && p == "/frame/export")
            .expect("the camera posts the export route");
        let body: Value = serde_json::from_slice(&post.2.body).expect("json body");
        assert_eq!(body, json!({ "device": "empty-1", "node": "gen-0" }));
        drop(calls);
        let toast = harness.state().nodes.preview.toast().map(str::to_string);
        assert_eq!(toast.as_deref(), Some("saved /tmp/x/gen-0-1.png"));
    }

    /// P-G follow-up: while the preview shows an authored sequence (the
    /// ingest lane's picture, `R22.8`), the bar names the sequence, reads
    /// its output resolution (not the preview's size), and the camera posts
    /// the sequence's export body through the same route.
    #[test]
    fn the_preview_bar_names_a_sequence_and_exports_its_frame() {
        use crate::shell::preview::{publish_sequence, PreviewSubject};
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        let jpeg = crate::paint::encode_jpeg(&[0u8; 4 * 4 * 4], 4, 4);
        recorder.respond_bytes("GET", "/author/render/solo.jpg", "image/jpeg", jpeg);
        recorder.respond("POST", "/frame/export", json!({ "path": "/tmp/x/cancion-1.png" }));
        let export = json!({ "model": "turing", "device": "empty-1", "name": "cancion" });
        let subject = PreviewSubject {
            label: "cancion · sequence".to_string(),
            export: Some(export.clone()),
            size: Some((1920, 1080)),
            fps: None,
        };
        let theme = Theme::phosphor();
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::Vec2::new(
                crate::shell::DESKTOP_VIEWPORT.0 as f32,
                crate::shell::DESKTOP_VIEWPORT.1 as f32,
            ))
            .build_ui_state(
                move |ui, shell: &mut Shell| {
                    let q = vec![("model".to_string(), "turing".to_string())];
                    publish_sequence(ui.ctx(), "/author/render/solo.jpg", q, true, false, Some(subject.clone()));
                    shell.ui(ui)
                },
                Shell::new(theme.clone(), Box::new(std::rc::Rc::clone(&recorder))),
            );
        theme.apply(&harness.ctx);
        settle(&mut harness);

        use egui_kittest::kittest::NodeT as _;
        drop(harness.get_by(|n| {
            n.role() == egui::accesskit::Role::Label
                && n.value().is_some_and(|l| l.starts_with("cancion · sequence · 1920×1080 · ") && l.ends_with(" fps"))
        }));
        harness.get_by_label(&glyph(Icon::Camera).to_string()).click();
        settle(&mut harness);

        let calls = recorder.calls.borrow();
        let post = calls
            .iter()
            .find(|(m, p, _)| m == "POST" && p == "/frame/export")
            .expect("the camera posts the export route");
        let body: Value = serde_json::from_slice(&post.2.body).expect("json body");
        assert_eq!(body, export);
    }

    // -- R22.5a: the menu offers what fits the domain -----------------

    /// R22.5a + L22.9: an Offline-only kind never shows in a Live graph's
    /// menu, and `stack.mode`/`compose.*` — retired from the menus entirely —
    /// do not either. The catalog is scripted with one of each plus a normal
    /// `gen.*`, so it is the menu's own filter that removes them, not a door
    /// that answered less; the same script proves the query asks for the
    /// graph's domain. Since 22-Sep-2026, afternoon the entries live under one
    /// menu per section, so the probe opens `+ source ▾` (where `gen.turing`
    /// and `audio.file` would land) and `+ other ▾` (where the rows with a
    /// retired family would).
    #[test]
    fn the_live_menu_has_no_export_video_audio_file_stack_or_compose() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        recorder.respond(
            "GET",
            "/catalog",
            json!({ "catalog": [
                { "kind": "gen.turing", "family": "generator", "label": "Turing", "inputs": [], "outputs": [], "menu": true, "domains": ["live", "offline"] },
                { "kind": "stack.mode", "family": "stack", "label": "Stack (mode)", "inputs": [], "outputs": [], "menu": false, "domains": ["live"] },
                { "kind": "compose.layers", "family": "compose", "label": "Compósito (capas)", "inputs": [], "outputs": [], "menu": false, "domains": ["live"] },
                { "kind": "export.video", "family": "export_video", "label": "Export (video)", "inputs": [], "outputs": [], "menu": true, "domains": ["offline"] },
                { "kind": "audio.file", "family": "audio", "label": "Audio (file)", "inputs": [], "outputs": [], "menu": true, "domains": ["offline"] }
            ] }),
        );
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        harness.get_by_label("new").click();
        harness.run();

        assert!(
            harness.query_by_label("Turing").is_some(),
            "a kind that serves the domain stays in the menu"
        );
        for hidden in ["Stack (mode)", "Compósito (capas)", "Export (video)", "Audio (file)"] {
            assert!(
                harness.query_by_label(hidden).is_none(),
                "{hidden} must not be in a Live graph's + source menu"
            );
        }
        let asked = recorder.calls.borrow().iter().any(|(m, p, params)| {
            m == "GET"
                && p == "/catalog"
                && params.query.get("domain").map(String::as_str) == Some("live")
        });
        assert!(asked, "the menu must ask the catalog for the open graph's own domain");
    }

    /// L22.9: the retired `stack`/`compose` families and the Offline-only
    /// kinds never appear under `+ other ▾` either — the menu the leftovers
    /// land in asks for the same domain filter as every other.
    #[test]
    fn the_leftover_menu_answers_the_same_domain_filter() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        recorder.respond(
            "GET",
            "/catalog",
            json!({ "catalog": [
                { "kind": "mask.poly", "family": "mask", "label": "Máscara", "inputs": [], "outputs": [], "menu": true, "domains": ["live", "offline"] },
                { "kind": "stack.mode", "family": "stack", "label": "Stack (mode)", "inputs": [], "outputs": [], "menu": false, "domains": ["live"] }
            ] }),
        );
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        harness.get_by_label("new").click();
        harness.run();
        harness.get_by_label("other").click();
        harness.run();

        assert!(
            harness.query_by_label("Máscara").is_some(),
            "a family with no section lands in + other"
        );
        assert!(
            harness.query_by_label("Stack (mode)").is_none(),
            "a retired kind stays out of + other too"
        );
    }

    // -- L22.11: drag of `files` onto the canvas ------------------------------

    /// A drag from `files` to the nodes canvas. The pointer presses on the
    /// source, moves past egui's click-distance threshold (so `drag_started`
    /// fires and the payload is set), travels to the target and releases
    /// there — the four frames a real drag takes.
    fn drag_between(
        harness: &mut egui_kittest::Harness<'static, Shell>,
        from: Pos2,
        to: Pos2,
    ) {
        harness.drag_at(from);
        harness.run();
        harness.hover_at(from + Vec2::splat(DRAG_CLEARANCE));
        harness.run();
        harness.hover_at(to);
        harness.run();
        // Release without `drop_at`'s follow-up `PointerGone`: the target's
        // `dnd_release_payload` reads `contains_pointer`, which a gone cursor
        // would clear on the very frame the release is processed.
        harness.event(egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run();
    }

    /// An image dropped on the canvas adds a `still` node with the card's
    /// path — the map `image ⇒ still` `L22.11` names.
    #[test]
    fn dropping_an_image_on_the_canvas_adds_a_still_with_its_path() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        recorder.respond(
            "GET",
            "/fs",
            json!({ "dirs": [], "files": [{ "file": "photo.png", "kind": "image" }] }),
        );
        // The real route answers the new node's id so the drop's second
        // write (the config) has a node to name.
        recorder.respond(
            "POST",
            "/graph/node",
            json!({ "graph": { "id": "empty-1", "domain": "live", "nodes": [] }, "primitives": [], "issues": [], "node": "still-0" }),
        );
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.state_mut().library_state_mut().set_cwd("/media");
        harness.run();

        let card = harness.get_by_label("photo.png").rect().center();
        let canvas = harness
            .state()
            .nodes_canvas_rect()
            .expect("the canvas should have painted a rect");
        drag_between(&mut harness, card, canvas.center());

        let calls = recorder.calls.borrow();
        let added = calls
            .iter()
            .find(|(m, p, params)| {
                m == "POST"
                    && p == "/graph/node"
                    && serde_json::from_slice::<Value>(&params.body)
                        .map(|b| b["op"] == "add" && b["kind"] == "still")
                        .unwrap_or(false)
            })
            .map(|(_, _, params)| params.clone())
            .expect("an image drop should POST /graph/node {op:add, kind:still}");
        let body: Value = serde_json::from_slice(&added.body).expect("json body");
        assert_eq!(
            body["kind"], "still",
            "an image becomes a still source node"
        );
        assert!(
            calls.iter().any(|(m, p, params)| {
                m == "POST"
                    && p == "/graph/node"
                    && serde_json::from_slice::<Value>(&params.body)
                        .map(|b| b["op"] == "config" && b["config"]["file"] == "/media/photo.png")
                        .unwrap_or(false)
            }),
            "the still's config carries the card's absolute path"
        );
    }

    /// The OS-drop double: a `DroppedFile` over a path on disk, the same
    /// shape `egui-winit` hands egui.
    #[derive(Debug)]
    struct DroppedPath(std::path::PathBuf);
    impl egui::DroppedFile for DroppedPath {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }

    fn os_file(path: &str) -> egui::DroppedFileHandle {
        std::sync::Arc::new(DroppedPath(std::path::PathBuf::from(path)))
    }

    /// A file dropped from the OS onto the canvas — with the pointer over it —
    /// adds the same `still` a library image drop does, at the pointer's own
    /// graph position.
    #[test]
    fn an_os_file_drop_on_the_canvas_adds_a_still_with_its_path() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        recorder.respond(
            "POST",
            "/graph/node",
            json!({ "graph": { "id": "empty-1", "domain": "live", "nodes": [] }, "primitives": [], "issues": [], "node": "still-0" }),
        );
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        let canvas = harness
            .state()
            .nodes_canvas_rect()
            .expect("the canvas should have painted a rect");
        harness.hover_at(canvas.center());
        harness.run();
        harness
            .input_mut()
            .dropped_files
            .push(os_file("/media/dropped.png"));
        harness.run();

        let calls = recorder.calls.borrow();
        let added = calls
            .iter()
            .find(|(m, p, params)| {
                m == "POST"
                    && p == "/graph/node"
                    && serde_json::from_slice::<Value>(&params.body)
                        .map(|b| b["op"] == "add" && b["kind"] == "still")
                        .unwrap_or(false)
            })
            .map(|(_, _, params)| params.clone())
            .expect("an OS image drop should POST /graph/node {op:add, kind:still}");
        let body: Value = serde_json::from_slice(&added.body).expect("json body");
        assert!(
            body["pos"].is_array(),
            "the still lands at the pointer's own graph position"
        );
        assert!(
            calls.iter().any(|(m, p, params)| {
                m == "POST"
                    && p == "/graph/node"
                    && serde_json::from_slice::<Value>(&params.body)
                        .map(|b| b["op"] == "config" && b["config"]["file"] == "/media/dropped.png")
                        .unwrap_or(false)
            }),
            "the still's config carries the dropped path"
        );
    }

    /// A library file picked from the gallery becomes the same source node a
    /// drop of that file makes: an image a still, a video a reel clip, audio
    /// an audio file; anything else is no source.
    #[test]
    fn a_library_file_from_the_gallery_becomes_its_source_kind() {
        let door = std::rc::Rc::new(FakeDoor::new());
        assert_eq!(
            source_for_file(&door, "image", "/m/p.png"),
            Some(("still", json!({ "file": "/m/p.png" })))
        );
        assert_eq!(source_for_file(&door, "video", "/m/c.mp4").map(|(k, _)| k), Some("reel.clip"));
        assert_eq!(source_for_file(&door, "audio", "/m/a.wav").map(|(k, _)| k), Some("audio.file"));
        assert_eq!(source_for_file(&door, "other", "/m/x"), None);
        assert_eq!(
            crate::shell::favorites::file_of_key(&crate::shell::favorites::file_key("video", "/m/c.mp4")),
            Some(("video", "/m/c.mp4")),
            "the gallery key routes back to kind and path"
        );
    }

    /// Ctrl+V pastes plain text as a `text.source` carrying it verbatim.
    #[test]
    fn a_clipboard_paste_of_text_adds_a_text_source_with_its_content() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        recorder.respond(
            "POST",
            "/graph/node",
            json!({ "graph": { "id": "empty-1", "domain": "live", "nodes": [] }, "primitives": [], "issues": [], "node": "text-0" }),
        );
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        harness.event(egui::Event::Paste("hola".to_string()));
        harness.run();

        let calls = recorder.calls.borrow();
        assert!(
            calls.iter().any(|(m, p, params)| {
                m == "POST"
                    && p == "/graph/node"
                    && serde_json::from_slice::<Value>(&params.body)
                        .map(|b| b["op"] == "add" && b["kind"] == "text.source")
                        .unwrap_or(false)
            }),
            "a text paste should POST /graph/node {{op:add, kind:text.source}}"
        );
        assert!(
            calls.iter().any(|(m, p, params)| {
                m == "POST"
                    && p == "/graph/node"
                    && serde_json::from_slice::<Value>(&params.body)
                        .map(|b| b["op"] == "config" && b["config"]["content"] == "hola")
                        .unwrap_or(false)
            }),
            "the text.source's config carries the pasted content"
        );
    }

    /// Ctrl+V of a single-line `file://` path pastes that file's source, by
    /// extension — not a text node.
    #[test]
    fn a_clipboard_paste_of_a_file_url_adds_the_matching_source() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        recorder.respond(
            "POST",
            "/graph/node",
            json!({ "graph": { "id": "empty-1", "domain": "live", "nodes": [] }, "primitives": [], "issues": [], "node": "still-0" }),
        );
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        harness.event(egui::Event::Paste("file:///media/pasted.png".to_string()));
        harness.run();

        let calls = recorder.calls.borrow();
        assert!(
            calls.iter().any(|(m, p, params)| {
                m == "POST"
                    && p == "/graph/node"
                    && serde_json::from_slice::<Value>(&params.body)
                        .map(|b| b["op"] == "config" && b["config"]["file"] == "/media/pasted.png")
                        .unwrap_or(false)
            }),
            "a file:// PNG path pastes as its file source, not as text"
        );
    }

    // -- N: the Wire canvas ------------------------------------------------

    /// A graph with two nodes, scripted for the panel's boot sequence: the
    /// picker lands on it, `GET /graph` answers it, `+ node`'s catalog is
    /// empty (these tests never open it) and `/graph/live` answers nothing.
    fn script_two_node_graph(recorder: &FakeDoor) {
        recorder.respond(
            "GET",
            "/graphs",
            json!({ "graphs": [{ "id": "g1", "name": "g1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            json!({
                "graph": { "id": "g1", "domain": "live", "nodes": [
                    { "id": "gen-0", "kind": "gen.turing", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false, "config": null },
                    { "id": "fx-0", "kind": "fx.vignette", "pos": [260.0, 0.0], "label": "", "bypass": false, "solo": false, "config": null }
                ], "edges": [] },
                "primitives": [],
                "issues": [],
            }),
        );
        recorder.respond("GET", "/catalog", json!({ "catalog": [] }));
        recorder.respond("GET", "/graph/live", json!({ "t": null, "age_ms": null, "nodes": {} }));
    }

    /// §4: the canvas hides the numeric fields on node bodies — a
    /// `value_field` rect and the `value` text it carries never paint, while
    /// a node's title, ports and chip do.
    #[test]
    fn node_bodies_hide_their_numeric_fields() {
        let field = json!({ "type": "rect", "tag": "value_field" });
        let value = json!({ "type": "text", "tag": "value" });
        let title = json!({ "type": "text", "tag": "title" });
        let port = json!({ "type": "rect", "tag": "port" });
        let chip = json!({ "type": "rect", "tag": "chip" });
        assert!(node_body_numeric(&field), "a value field is a numeric field");
        assert!(node_body_numeric(&value), "a value readout is a numeric field");
        assert!(!node_body_numeric(&title), "the title stays");
        assert!(!node_body_numeric(&port), "a port stays");
        assert!(!node_body_numeric(&chip), "the mode chip stays");
    }

    /// One horizontal drag across a value field posts exactly one
    /// `POST /graph/node { op: "param" }`, on release, with the value the drag
    /// produced — the range scaled per [`PARAM_DRAG_SPAN`] graph units.
    #[test]
    fn dragging_a_value_field_posts_the_param_op_once_on_release() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_two_node_graph(&recorder);
        recorder.respond(
            "GET",
            "/graph/hit",
            json!({ "hit": "param", "node": "fx-0", "name": "amount", "point": [260.0, 66.0] }),
        );
        recorder.respond(
            "GET",
            "/graph/io",
            json!({ "params": [{ "name": "amount", "min": 0.0, "max": 2.0, "live": 0.5 }] }),
        );
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        let rect = harness
            .state()
            .nodes_canvas_rect()
            .expect("the canvas should have painted a rect by now");
        let start = rect.center();
        let dx = 100.0;
        let end = start + egui::vec2(dx, 0.0);
        harness.drag_at(start);
        harness.run();
        harness.hover_at(end);
        harness.run();
        harness.drop_at(end);
        harness.run();

        let calls = recorder.calls.borrow();
        let params: Vec<Value> = calls
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
            .filter_map(|(_, _, params)| serde_json::from_slice::<Value>(&params.body).ok())
            .filter(|b: &Value| b["op"] == "param")
            .collect();
        assert_eq!(params.len(), 1, "one drag posts one param op, not a stream");
        assert_eq!(params[0]["node"], json!("fx-0"));
        assert_eq!(params[0]["name"], json!("amount"));
        // 0.5 start + 100 units × (2.0/200) = 1.5.
        let v = params[0]["value"].as_f64().unwrap();
        assert!((v - 1.5).abs() < 1e-3, "the drag scaled against the range: {v}");
    }

    /// A drop from an output that carries several variables onto a parameter
    /// dot opens the variable chooser (it does not post yet); picking one
    /// posts the bind op with that variable's `port.var`.
    #[test]
    fn dropping_on_a_param_dot_from_a_multi_variable_port_offers_the_variables() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_two_node_graph(&recorder);
        recorder.respond_seq(
            "GET",
            "/graph/hit",
            json!({ "hit": "port", "node": "gen-0", "port": "row", "dir": "out", "point": [176.0, 66.0] }),
        );
        recorder.respond_seq(
            "GET",
            "/graph/hit",
            json!({ "hit": "port", "node": "fx-0", "port": "param:amount", "dir": "in", "point": [260.0, 66.0] }),
        );
        recorder.respond(
            "GET",
            "/graph/io",
            json!({ "node": "fx-0", "sources": [{
                "node": "gen-0", "hops": 1, "vars": [
                    { "name": "scale", "port": "row", "port_ref": "row.scale", "kind": "float" },
                    { "name": "step", "port": "row", "port_ref": "row.step", "kind": "float" }
                ]
            }] }),
        );
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();

        let rect = harness
            .state()
            .nodes_canvas_rect()
            .expect("the canvas should have painted a rect by now");
        let start = egui::pos2(rect.left() + 176.0, rect.top() + 66.0);
        let landing = egui::pos2(rect.left() + 260.0, rect.top() + 66.0);
        harness.drag_at(start);
        harness.run();
        harness.hover_at(landing);
        harness.run();
        harness.drop_at(landing);
        harness.run();
        // The menu paints the frame after the drop sets it.
        harness.run();

        let bind_bodies: Vec<Value> = recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
            .filter_map(|(_, _, params)| serde_json::from_slice::<Value>(&params.body).ok())
            .filter(|b| b["op"] == "bind")
            .collect();
        assert!(bind_bodies.is_empty(), "nothing posts until a variable is picked");

        harness.get_by_label("row.scale").click();
        harness.run();

        let bound = recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
            .filter_map(|(_, _, params)| serde_json::from_slice::<Value>(&params.body).ok())
            .find(|b| b["op"] == "bind")
            .expect("picking a variable posts the bind op");
        assert_eq!(bound["node"], json!("fx-0"));
        assert_eq!(bound["param"], json!("amount"));
        assert_eq!(bound["from"]["node"], json!("gen-0"));
        assert_eq!(bound["from"]["port"], json!("row"));
        assert_eq!(bound["from"]["var"], json!("scale"));
    }

    /// A cable draws in the accent exactly when one of its ends is the
    /// selected node — the Wire rule (`nodes-reference.png`).
    #[test]
    fn a_cable_draws_in_the_accent_only_when_an_end_is_the_selected_node() {
        let cable = json!({
            "type": "path",
            "from": { "node": "gen-0", "port": "image" },
            "to": { "node": "fx-0", "port": "image" },
            "kind": "image",
        });
        assert!(cable_touches_selection(&cable, Some("gen-0")), "the from end");
        assert!(cable_touches_selection(&cable, Some("fx-0")), "the to end");
        assert!(!cable_touches_selection(&cable, Some("other")), "neither end");
        assert!(!cable_touches_selection(&cable, None), "nothing selected");
    }

    /// A selected or hovered cable lights on its own, with no node selected —
    /// the 2026-09-30 fix: only the node rule existed before.
    #[test]
    fn a_selected_or_hovered_cable_lights_on_its_own() {
        let cable = json!({
            "type": "path", "edge": "e1",
            "from": { "node": "gen-0", "port": "image" },
            "to": { "node": "fx-0", "port": "image" },
            "kind": "image",
        });
        assert!(cable_lit(&cable, None, [Some("e1"), None]), "selected cable");
        assert!(cable_lit(&cable, None, [None, Some("e1")]), "hovered cable");
        assert!(!cable_lit(&cable, None, [Some("e2"), None]), "another cable");
        assert!(cable_lit(&cable, Some("gen-0"), [None, None]), "the node rule still holds");
    }

    /// The pointer over a cable's curve finds that cable; far from it, none.
    #[test]
    fn the_pointer_near_a_curve_hovers_that_cable() {
        let mut state = PanelState::default();
        state.graph = Some(json!({ "primitives": [{
            "type": "path", "edge": "e1", "kind": "image",
            "from": { "node": "a", "port": "image" }, "to": { "node": "b", "port": "image" },
            "p0": [0.0, 0.0], "p1": [50.0, 0.0], "p2": [50.0, 0.0], "p3": [100.0, 0.0],
        }] }));
        assert_eq!(edge_near(&state, [40.0, 2.0], 6.0).as_deref(), Some("e1"));
        assert_eq!(edge_near(&state, [40.0, 40.0], 6.0), None);
    }

    /// The live poll's read: a cached `GET /graph/live` reply answers a
    /// node's in-value by name, and nothing before a poll landed.
    #[test]
    fn live_values_are_read_off_the_cached_graph_live_reply() {
        let mut state = PanelState::default();
        assert_eq!(live_of(&state).param("fx-0", "amount"), None, "nothing before a poll");
        state.live = Some(json!({ "nodes": { "fx-0": { "in": { "amount": 0.75 } } } }));
        assert_eq!(live_of(&state).param("fx-0", "amount"), Some(0.75));
        assert_eq!(live_of(&state).param("fx-0", "other"), None);
        assert_eq!(live_of(&state).param("gen-0", "amount"), None);
    }

    /// Plan "Surface sources" step 5: the canvas bar's `+ … ▾` menus ask the
    /// door for the open graph and the selected node as the anchor, so the
    /// entries come back in relevance order.
    #[test]
    fn the_canvas_menu_asks_for_the_graph_and_the_selected_anchor() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        recorder.respond(
            "GET",
            "/catalog",
            json!({ "catalog": [
                { "kind": "gen.turing", "family": "generator", "label": "Turing", "inputs": [], "outputs": [], "menu": true, "domains": ["live"], "relevance": 0.4 },
                { "kind": "gen.rings", "family": "generator", "label": "Anillos", "inputs": [], "outputs": [], "menu": true, "domains": ["live"], "relevance": 2.5 }
            ] }),
        );
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();

        harness.get_by_label("new").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let asked = calls.iter().rev().find(|(m, p, _)| m == "GET" && p == "/catalog").expect("the menu asks");
        assert_eq!(asked.2.query.get("graph").map(String::as_str), Some("empty-1"));
        assert_eq!(asked.2.query.get("anchor").map(String::as_str), Some("gen-0"));
        // The higher relevance is listed first (cards run left to right).
        use egui_kittest::kittest::NodeT as _;
        let card_x = |label: &str| {
            harness
                .query_all_by_label(label)
                .filter(|n| n.accesskit_node().role() == egui::accesskit::Role::Button)
                .map(|n| n.rect().min.x)
                .next()
        };
        let rings = card_x("Anillos");
        let turing = card_x("Turing");
        assert!(matches!((rings, turing), (Some(a), Some(t)) if a < t), "relevance order: {rings:?} {turing:?}");
    }

    /// The reveal's pan puts the node at the middle of the canvas, whatever
    /// the zoom and wherever the canvas sits on screen.
    #[test]
    fn a_revealed_node_is_centred_on_the_canvas() {
        let (width, height) = (800.0_f32, 400.0_f32);
        let size = Vec2::new(width, height);
        let origin = Pos2::new(10.0, 20.0);
        for (at, zoom) in [([300.0, 120.0], 1.0), ([-40.0, 900.0], 1.5), ([0.0, 0.0], 0.5)] {
            let pan = pan_to_centre(size, at, zoom);
            let screen = to_screen(origin, pan, zoom, at);
            assert!(
                (screen - (origin + size * 0.5)).length() < 1e-3,
                "{at:?} at zoom {zoom} lands at {screen:?}"
            );
        }
    }

    // -- 2026-10-16: the floating bars, the keyboard delete, the gallery -----

    fn posted_node_op(recorder: &FakeDoor, op: &str, field: Option<&str>) -> bool {
        recorder.calls.borrow().iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/graph/node"
                && serde_json::from_slice::<Value>(&params.body)
                    .map(|b| b["op"] == op && field.is_none_or(|f| b["field"] == f))
                    .unwrap_or(false)
        })
    }

    /// The first `POST path` body a recorder saw, for the splice tests.
    fn posted_body(recorder: &FakeDoor, path: &str) -> Value {
        recorder
            .calls
            .borrow()
            .iter()
            .find_map(|(m, p, params)| {
                (m == "POST" && p == path)
                    .then(|| serde_json::from_slice::<Value>(&params.body).ok())
                    .flatten()
            })
            .expect("a POST was recorded")
    }

    /// `add after` splices the picked kind downstream of the anchor through
    /// the door's own `insert` op, carrying the anchor id as its payload.
    #[test]
    fn add_after_splices_the_picked_kind_after_the_anchor() {
        let door = FakeDoor::new();
        let mut state = PanelState::default();
        state.selected_graph = Some("g1".to_string());
        state.graph = Some(json!({
            "graph": { "id": "g1", "nodes": [
                { "id": "gen-0", "kind": "gen.live", "pos": [0.0, 0.0], "config": null },
                { "id": "fx-0", "kind": "fx.grade", "pos": [220.0, 0.0], "config": null }
            ] },
            "primitives": [], "issues": []
        }));
        let mut sel = Selection::Node("fx-0".to_string());
        state.insert_request = Some(InsertRequest { node: "fx-0".to_string(), after: true });
        state.add_from_gallery(&door, &mut sel, "fx.vignette", &Theme::phosphor());
        let posted = posted_body(&door, "/graph/node");
        assert_eq!(posted["op"], json!("insert"));
        assert_eq!(posted["after"], json!("fx-0"));
        assert_eq!(posted["kind"], json!("fx.vignette"));
        assert!(state.insert_request.is_none(), "the request is consumed by the pick");
    }

    /// `add before` splices between the anchor and the node feeding its image
    /// input — the same `insert` op, anchored on that producer.
    #[test]
    fn add_before_splices_between_the_anchor_and_its_upstream() {
        let door = FakeDoor::new();
        let mut state = PanelState::default();
        state.selected_graph = Some("g1".to_string());
        state.graph = Some(json!({
            "graph": { "id": "g1", "nodes": [
                { "id": "gen-0", "kind": "gen.live", "pos": [0.0, 0.0], "config": null },
                { "id": "fx-0", "kind": "fx.grade", "pos": [220.0, 0.0], "config": null }
            ], "edges": [
                { "id": "e0", "from": { "node": "gen-0", "port": "image" }, "to": { "node": "fx-0", "port": "image" } }
            ] },
            "primitives": [], "issues": []
        }));
        let mut sel = Selection::Node("fx-0".to_string());
        state.insert_request = Some(InsertRequest { node: "fx-0".to_string(), after: false });
        state.add_from_gallery(&door, &mut sel, "fx.vignette", &Theme::phosphor());
        let posted = posted_body(&door, "/graph/node");
        assert_eq!(posted["op"], json!("insert"));
        assert_eq!(posted["after"], json!("gen-0"));
        assert_eq!(posted["kind"], json!("fx.vignette"));
    }

    /// A dismissed gallery leaves no splice behind: the next plain pick adds
    /// freely, and a click on the bar's buttons opens the gallery with the
    /// anchor in the payload.
    #[test]
    fn a_closed_gallery_clears_the_pending_splice() {
        let mut state = PanelState::default();
        state.insert_request = Some(InsertRequest { node: "fx-0".to_string(), after: true });
        state.set_gallery_open(false, false, 0.0, &[]);
        assert!(state.insert_request.is_none(), "closing the overlay drops the splice");
    }

    /// The floating bar rides its node's drag: the same graph delta the canvas
    /// paints the box at, scaled into screen space.
    #[test]
    fn the_focus_bar_follows_a_dragged_node() {
        let mut state = PanelState::default();
        state.zoom = 2.0;
        state.drag = Some(Drag {
            node: "n0".to_string(),
            start_node_pos: [0.0, 0.0],
            start_pointer: [10.0, 20.0],
            cur_pointer: [13.0, 24.0],
        });
        let off = drag_screen_offset(&state, "n0");
        assert_eq!((off.x, off.y), (6.0, 8.0));
        assert_eq!(drag_screen_offset(&state, "another"), Vec2::ZERO);
    }

    /// The focused node carries a floating bar of icon buttons; bypass posts
    /// the flag op and a solo-less node reads as off.
    #[test]
    fn the_focused_node_has_a_floating_bar_with_bypass_solo_duplicate_remove() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        assert!(harness.query_by_label("duplicate").is_none(), "no bar without a focused node");
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();
        for tip in ["bypass", "solo", "duplicate", "remove"] {
            harness.get_by_label(tip);
        }
        harness.get_by_label("bypass").click();
        harness.run();
        assert!(posted_node_op(&recorder, "flag", Some("bypass")), "bypass posts the flag op");
    }

    /// A node with bypass on shows the resting bar even when it is not the
    /// focused one, and clicking it turns the flag off again.
    #[test]
    fn an_unfocused_node_with_bypass_on_shows_the_hold_bar() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/graphs", json!({ "graphs": [{ "id": "empty-1", "name": "empty-1", "domain": "live" }] }));
        recorder.respond(
            "GET",
            "/graph",
            json!({
                "graph": { "id": "empty-1", "domain": "live", "nodes": [
                    { "id": "fx-0", "kind": "fx.shake", "pos": [300.0, 200.0], "label": "", "bypass": true, "solo": false, "config": null }
                ] },
                "primitives": [], "issues": []
            }),
        );
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness.get_by_label("bypass").click();
        harness.run();
        assert!(posted_node_op(&recorder, "flag", Some("bypass")));
    }

    /// Delete on the focused canvas removes the selected node.
    #[test]
    fn delete_removes_the_selected_node_while_the_canvas_has_the_focus() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.state_mut().focus = Some(crate::shell::panels::Region::Nodes);
        harness.run();
        harness.key_press(egui::Key::Delete);
        harness.run();
        assert!(posted_node_op(&recorder, "remove", None), "Delete posts the remove op");
    }

    /// `new` opens the gallery; a card's click adds that node.
    #[test]
    fn the_new_button_opens_the_gallery_and_a_card_adds_the_node() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        recorder.respond(
            "GET",
            "/catalog",
            json!({ "catalog": [
                { "kind": "gen.turing", "family": "generator", "label": "Turing", "inputs": [], "outputs": [{ "name": "image" }], "menu": true },
                { "kind": "mat.metal", "family": "material", "label": "Brushed", "inputs": [], "outputs": [], "menu": true }
            ] }),
        );
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness.get_by_label("new").click();
        harness.run();
        harness.get_by_label("materials").click();
        harness.run();
        harness.get_by_label("Brushed").click();
        harness.run();
        let added = recorder.calls.borrow().iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/graph/node"
                && serde_json::from_slice::<Value>(&params.body)
                    .map(|b| b["op"] == "add" && b["kind"] == "mat.metal")
                    .unwrap_or(false)
        });
        assert!(added, "a mat.* kind lands in the materials section and adds");
    }

    /// The card grid has between two and five columns.
    #[test]
    fn the_gallery_grid_has_two_to_five_columns() {
        let theme = Theme::phosphor();
        assert_eq!(crate::shell::gallery::columns_for(&theme, 10.0), 2);
        assert_eq!(crate::shell::gallery::columns_for(&theme, theme.gallery_card_min_w * 3.5), 3);
        assert_eq!(crate::shell::gallery::columns_for(&theme, 100_000.0), 5);
    }

    // -- the Ctrl+click multi-selection ---------------------------------------

    /// Ctrl+click toggles membership; the node selection follows the last id
    /// left, and a lone survivor is a plain single selection again.
    #[test]
    fn ctrl_click_toggles_a_node_in_and_out_of_the_selection() {
        let mut state = PanelState::default();
        let mut selection = Selection::Node("gen-0".to_string());
        toggle_picked(&mut state, &mut selection, "fx-0");
        assert_eq!(state.picked_nodes(&selection), ["gen-0", "fx-0"]);
        assert!(matches!(&selection, Selection::Node(id) if id == "fx-0"));
        toggle_picked(&mut state, &mut selection, "gen-0");
        assert!(state.picked_nodes(&selection).is_empty(), "one node left is a single selection");
        assert!(matches!(&selection, Selection::Node(id) if id == "fx-0"));
        toggle_picked(&mut state, &mut selection, "fx-0");
        assert!(matches!(selection, Selection::None), "the last one toggled off clears the selection");
    }

    /// A stale multi-selection (the selection moved to another node since)
    /// reads as no multi-selection at all.
    #[test]
    fn a_stale_picked_list_is_not_a_multi_selection() {
        let mut state = PanelState::default();
        state.picked = vec!["gen-0".to_string(), "fx-0".to_string()];
        assert_eq!(state.picked_nodes(&Selection::Node("fx-0".to_string())), ["gen-0", "fx-0"]);
        assert!(state.picked_nodes(&Selection::Node("gen-0".to_string())).is_empty());
        assert!(state.picked_nodes(&Selection::None).is_empty());
    }

    /// Delete with several nodes picked removes each of them, one remove op
    /// per node, and leaves no multi-selection behind.
    #[test]
    fn delete_removes_every_picked_node() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        script_gen_and_effect_graph(&recorder);
        let mut harness = shell_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("fx-0".to_string()));
        harness.state_mut().nodes.picked = vec!["gen-0".to_string(), "fx-0".to_string()];
        harness.state_mut().focus = Some(crate::shell::panels::Region::Nodes);
        harness.run();
        harness.key_press(egui::Key::Delete);
        harness.run();
        let removed: Vec<String> = recorder
            .calls
            .borrow()
            .iter()
            .filter_map(|(m, p, params)| {
                let body: Value = serde_json::from_slice(&params.body).ok()?;
                (m == "POST" && p == "/graph/node" && body["op"] == "remove")
                    .then(|| body["node"].as_str().unwrap_or_default().to_string())
            })
            .collect();
        assert_eq!(removed, ["gen-0", "fx-0"], "each picked node is removed");
        let shell = harness.state_mut();
        assert!(shell.nodes.picked_nodes(&shell.selection).is_empty());
    }

    /// A node clip (what Ctrl+C writes) pastes its nodes, each with its config,
    /// and the pasted set becomes the multi-selection; plain text still pastes
    /// as a text source (see the paste tests above).
    #[test]
    fn a_pasted_node_clip_adds_each_node_and_selects_them() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond_seq("POST", "/graph/node", json!({ "node": "gen-1" }));
        recorder.respond_seq("POST", "/graph/node", json!({ "node": "fx-1" }));
        recorder.respond_seq("POST", "/graph/node", json!({ "graph": { "id": "empty-1", "nodes": [] } }));
        let mut state = PanelState::default();
        state.set_selected_graph_for_test("empty-1");
        let mut selection = Selection::None;
        let theme = Theme::phosphor();
        let clip = json!({ NODE_CLIP_KEY: [
            { "kind": "gen.turing", "config": null, "pos": [0.0, 0.0] },
            { "kind": "fx.shake", "config": { "amount": 2 }, "pos": [40.0, 20.0] },
        ] })
        .to_string();
        let parsed = parse_node_clip(&clip).expect("the clip parses back into its nodes");
        paste_nodes(&*recorder, &mut state, &mut selection, &parsed, &theme).expect("the clip pastes");

        let calls = recorder.calls.borrow();
        let posted: Vec<Value> = calls
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
            .map(|(_, _, params)| serde_json::from_slice(&params.body).expect("json body"))
            .collect();
        assert_eq!(posted[0]["op"], "add");
        assert_eq!(posted[0]["kind"], "gen.turing");
        assert_eq!(posted[1]["kind"], "fx.shake");
        assert_eq!(posted[2]["op"], "config", "only a node with a config is given one");
        assert_eq!(posted[2]["node"], "fx-1");
        assert_eq!(posted[2]["config"]["amount"], 2);
        assert_eq!(state.picked_nodes(&selection), ["gen-1", "fx-1"]);
        assert!(matches!(&selection, Selection::Node(id) if id == "fx-1"));
    }

    /// Anything that is not a node clip is not one: plain clipboard text
    /// keeps its own paste.
    #[test]
    fn only_a_node_clip_is_read_as_nodes() {
        assert!(parse_node_clip("hola").is_none());
        assert!(parse_node_clip("{\"other\": []}").is_none());
        assert!(parse_node_clip(&json!({ NODE_CLIP_KEY: [] }).to_string()).is_some());
    }

    /// A drag's move and a flag show on the canvas at once, before the writer
    /// has run them: the node keeps its place and its flag is set, not reset
    /// to the graph read before the write.
    #[test]
    fn a_move_and_a_flag_show_at_once_on_the_open_graph() {
        let mut state = PanelState::default();
        state.graph = Some(json!({ "graph": { "nodes": [
            { "id": "gen-0", "kind": "gen.turing", "pos": [10.0, 20.0], "bypass": false },
            { "id": "fx-0", "kind": "fx.grade", "pos": [0.0, 0.0] },
        ] } }));
        apply_node_op_locally(&mut state, &json!({ "op": "move", "graph": "g", "node": "gen-0", "pos": [300.0, 40.0] }));
        apply_node_op_locally(&mut state, &json!({ "op": "flag", "graph": "g", "node": "gen-0", "field": "bypass" }));
        let graph = state.graph.as_ref().unwrap();
        let node = &graph["graph"]["nodes"][0];
        assert_eq!(node["pos"], json!([300.0, 40.0]));
        assert_eq!(node["bypass"], json!(true), "a flag without `on` toggles");
        assert_eq!(node["kind"], "gen.turing", "the node itself is still there");
        assert_eq!(graph["graph"]["nodes"][1]["pos"], json!([0.0, 0.0]), "other nodes untouched");
    }
}
