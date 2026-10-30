//! The native shell (`O.12`), in three layers that a test keeps apart.
//!
//! **The rule** (`plan-opus.md` § O.12.d): *shell* — this file and
//! `panels/` — declares the structure of the chrome and where each region
//! lives; *components* (`components.rs`) are the reusable pieces it is built
//! from; *classes* (`classes.rs`) hold every visual constant as a [`Theme`].
//! Dependencies point one way — `classes` imports nothing from `shell`,
//! `components` imports only `classes` — so changing the style of a panel
//! never requires touching the file that declares where that panel lives.
//! The rule is enforced, not observed: `gates.rs` runs three tests over this
//! directory and nothing else — (1) the dependency direction, parsed from
//! the `use` paths; (2) a ban on colour, stroke, margin, radius and spacing
//! literals outside `classes.rs`, with `literal-allowlist.txt` as the one
//! place an exception can be written; (3) a restyle drill that paints
//! [`Shell::ui`] under two themes and asserts the AccessKit widget tree is
//! structurally identical while the themes differ.
//!
//! `UiState::build` in `metal-render/src/app.rs` is the egui precedent in this
//! tree and is *not* subject to these gates: it draws its whole panel in one
//! function, and its split into `Rendering`/`UiState` (`app.rs:40-45`) is
//! load-bearing for the borrow checker — `run_ui` needs `&mut Rendering`
//! while the closure needs `&mut UiState` — not a layering. `S.U5` cites it
//! and does not refactor it (`plan-opus.md` § O.12.d, last paragraph).
//!
//! Nothing here calls a handler body or does I/O directly. The header
//! (`header.rs`) and the ceding grid (`regions.rs`, `S.U8`) go through
//! [`crate::call::Call`]/[`crate::call::Door`] — the one door
//! `plan-opus.md` § `O.12.b′` requires — never a `call::<submodule>::`
//! path, `crate::serve`, `metal_web` or `TcpStream`. `S.U7b` is what makes
//! that door reachable at all (`Shared: Send`, see its doc comment in
//! `main.rs`); `S.U8` is the first lote that actually calls through it,
//! from exactly two places — the `más ▾` freeze toggle and the global
//! spacebar shortcut, both funnelled through [`Shell::toggle_transport`] so
//! there is one code path, not two, for the same `POST /transport`.
//!
//! `window.rs` (`S.U6`) is the platform layer underneath: the `winit`
//! window and `wgpu` surface that hand [`Shell::ui`] its `Ui` every frame.
//! It is a fourth file under the same three gates, not a fourth layer.
//! `header.rs` and `regions.rs` (`S.U8`) are two more: neither is `classes`
//! or `components`, so gate 1 only checks that they do not reach a banned
//! door surface, and gate 2 bans a literal in them exactly as it would in
//! `mod.rs` itself.

pub mod classes;
pub mod collection;
pub mod components;
pub mod frames;
pub mod favorites;
pub mod gallery;
pub mod gallery_anim;
pub mod graph_hit;
pub(crate) use crate::i18n;
pub mod insets;
mod keybinds;
pub mod lanes;
pub mod media;
pub mod naming;
pub mod panels;
pub mod present;
pub mod preview;
mod perf_hud;
mod graph_writer;
mod lane_worker;
mod reads;
pub mod restricted;
pub mod song_pull;
pub mod timeline;
#[cfg(not(target_arch = "wasm32"))]
pub mod window;

mod header;
mod layout;
mod regions;

#[cfg(test)]
mod gates;
mod mention;
#[cfg(all(test, any(feature = "gpu", feature = "glow")))]
mod kittest;

use std::collections::{HashMap, HashSet};

use egui::{CentralPanel, Id, Panel, Rect, Ui};

use crate::call::Call;
use classes::{Placement, Theme};
fn lanes_import_sections(door: &dyn Call, nodes: &nodes::PanelState) -> Vec<gallery::Section> {
    panels::lanes::import_sections(door, nodes)
}

use components::{ground_frame, header_frame, panel_frame, side_panel, SidebarSide};
use header::HeaderEvents;
/// The lanes *panel* (`panels/lanes.rs`) under a name that leaves the bare
/// `lanes` for the lane *canvas* module (`shell/lanes.rs`) — the panel is
/// re-exported nowhere else, so the alias stays private to this module's
/// own `lanes: PanelState` field and its accessors.
use panels::lanes as lanes_panel;
use panels::{chat, library, nodes, properties, settings, Region, Selection};

/// The desktop viewport, in logical pixels: `window.rs`'s initial window
/// size, the `screen_rect` gate 3 (`gates.rs`) paints under, and the
/// `egui_kittest` harness's canvas (`kittest.rs`, `S.U7`) — one constant so
/// the three cannot drift apart. Theme-independent on purpose: how big the
/// *window* is is a platform decision, not a style one (`window.rs`'s own
/// comment on the matter), so it does not belong in `classes.rs`'s `Theme`.
pub(crate) const DESKTOP_VIEWPORT: (f64, f64) = (1600.0, 950.0);

/// The header's Edit|Show toggle (`header.rs`). `Edit` is the whole shell;
/// `Show` paints only the header and the overlays grid over the full area,
/// for playing a show (no sidebars, no graph canvas, no region ceding).
/// Persisted through `call::settings` as `"shell_view"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShellView {
    #[default]
    Edit,
    Show,
}

impl ShellView {
    /// The wire strings `call::settings` accepts, in toggle order — one
    /// vocabulary spelled once here, checked against the route by a test.
    pub const CODES: [&'static str; 2] = ["edit", "show"];

    pub fn code(self) -> &'static str {
        Self::CODES[self as usize]
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "edit" => Some(ShellView::Edit),
            "show" => Some(ShellView::Show),
            _ => None,
        }
    }
}

/// The state that is no panel's: the theme, the one door a panel is ever
/// allowed to call a handler through (`S.U7b`), which region is focused
/// (`S.U8` — see `regions.rs`), the last-painted rect of each (read by the
/// kittest ceding test, `region_rects`), and a test-only hook
/// (`debug_text_focus_probe`) `kittest.rs` uses to prove the spacebar
/// shortcut backs off when a text field really is focused.
///
/// `door` is `Box<dyn Call>` rather than a concrete `call::Door` so this
/// struct does not have to become generic and so a test can hand it a
/// recording stand-in (`FakeDoor`, below) instead of a real `Door` — the
/// same trade `paint::Paint` made for [`crate::stand::Stand`]. `window.rs`
/// is the one caller that ever supplies a real one; `gates.rs`'s restyle
/// drill and `kittest.rs`'s harness both default to `FakeDoor`, which is
/// honest about what they are testing (structure and style) and does not
/// pay a real painter's GPU cost to do it — `kittest.rs`'s own real-`Door`
/// test (`S.U8`) is the one place that pays it, on purpose.
pub struct Shell {
    theme: Theme,
    /// The house theme the settings' palette recolours (`Theme::with_palette`).
    base_theme: Theme,
    door: Box<dyn Call>,
    /// The preview pump's worker, when there is one (`set_fetcher`) — the
    /// off-thread half of [`frames::Frame`]. `None` in tests, where the
    /// pumped images fetch inline through `door` as before.
    fetcher: Option<frames::Fetcher>,
    focus: Option<Region>,
    /// TASK 3: the focus as of the last painted frame, so `ui` can tell when a
    /// *different* region has taken it and dismiss the library's media
    /// preview. A click inside the library never changes a non-library focus,
    /// so the operator's own pick is never the cause of a dismissal.
    last_focus: Option<Region>,
    rects: HashMap<Region, Rect>,
    /// What the graph canvas has selected (`S.U9`) — a node, an edge, or
    /// nothing. Shared, not owned by `panels::nodes`, so a later panel
    /// (`S.U12`'s properties) can read it without reaching into another
    /// panel's private state — see `panels::Selection`'s own doc comment.
    selection: Selection,
    /// TASK 3: the selection as of the last painted frame — a move off
    /// `Selection::File` (a node or edge picked elsewhere) dismisses the
    /// library's media preview, the same as a focus move.
    last_selection: Selection,
    /// The nodes panel's own state (`S.U9`): which graph is open, its
    /// last-fetched reply, the view transform, any drag in flight. Held here
    /// for the same reason `focus`/`rects` are — it has to survive across
    /// frames, and `Shell` is where every panel's per-frame state currently
    /// lives (`S.U10`–`S.U12` will each add their own field the same way).
    nodes: nodes::PanelState,
    /// The room gallery's standalone state (`panels::venue_popup::paint`'s
    /// own door). The canvas's scene paints the gallery from its own scratch
    /// (`nodes::PanelState::venue_gallery`); kept so the module's wrapper has
    /// a state to be called with, closed while no scene shows it.
    #[allow(dead_code)]
    venue_popup: panels::venue_popup::State,
    /// The lanes panel's own state (`S.U10`): trigger/reel/mask-grid form
    /// fields and the last-fetched `GET /reel/files` reply — everything
    /// else it paints (`GET /state`) is re-read fresh every frame rather
    /// than cached here, see `panels::lanes::PanelState`'s own doc comment.
    lanes: lanes_panel::PanelState,
    /// The library panel's own state (`S.U11`): footage/collections/
    /// songs form fields, the last-fetched `GET /reel/files` reply and
    /// the inline `confirmish` stand-in — everything else it paints (`GET
    /// /state`, `GET /author/songs`, `GET /author/curator/state`) is
    /// re-read fresh every frame, see `panels::library::PanelState`'s own
    /// doc comment.
    library: library::PanelState,
    /// The properties panel's own state (`S.U12`) — empty today
    /// ([`properties::PanelState`] is a marker: everything it shows is
    /// re-read fresh every frame off the selection and the nodes panel's
    /// open graph, see that module's own doc comment). Held here anyway,
    /// the same reason `nodes`/`lanes`/`library` are: a panel's state is a
    /// `Shell` field, not a local the paint call would have to re-invent.
    properties: properties::PanelState,
    /// The background graph writer (`graph_writer`): its failed writes reach
    /// the properties status line in [`Shell::ui`]. `None` in tests, where
    /// `Call::post_async` writes inline.
    graph_writes: Option<graph_writer::GraphWriter>,
    /// `S.AP`: the Colourist ingest lane's own state — the song digest, the
    /// authored keyframes, the live float row. A `Shell` field rather than
    /// nested inside `lanes`'s own `PanelState` (22-sept-2026 correction):
    /// the columns/keyframe canvas is `lanes`' to paint but the float
    /// sliders are `properties`' (*"the floats controls should be loaded
    /// in the properties panel and not in the lanes"*), and both panels
    /// need to reach the same `row`/`model` for that to be one object
    /// instead of two copies drifting apart.
    ingest: crate::shell::lanes::IngestLane,
    /// The AI harness sidebar's own state (`S.U14`): the prompt still
    /// being typed and each tool's raw-JSON params buffer — everything else
    /// it paints (`GET /ai-config`, `GET /ai/log`)
    /// is re-read fresh every frame, see `panels::chat::PanelState`'s
    /// own doc comment.
    chat: chat::PanelState,
    /// Set by a test to add an always-focusable text field to the frame, so
    /// `space_does_not_toggle_with_a_text_field_focused` (`kittest.rs`) can
    /// focus it for real and check the spacebar shortcut backs off — never
    /// set outside a test, always `false` in the shipped app.
    pub(crate) debug_text_focus_probe: bool,

    // --- responsive delegation (`S.U13`, `O.12.f`) -------------------------
    /// Whether the library sidebar is expanded or collapsed to its rail,
    /// once [`classes::Arrangement::library`] has delegated it out of the
    /// grid — the header's own toggle (`header.rs`) is the only thing that
    /// flips this.
    library_sidebar_open: bool,
    /// Which of `graph`/`lanes` the header's two-tab selector currently
    /// shows, once [`classes::Arrangement::tabs`] has delegated. Always one
    /// of [`Region::Nodes`]/[`Region::Lanes`] — nothing else ever writes
    /// this field.
    active_tab: Region,
    /// Whether the properties long-press overlay is open, once
    /// [`classes::Arrangement::properties`] has delegated it out of the
    /// grid.
    properties_overlay_open: bool,
    /// When the properties long-press button (`header.rs`) went down, read
    /// off `ui.input(|i| i.time)` — `None` while it is up. A run past
    /// `theme.long_press` is what opens `properties_overlay_open`; letting
    /// go before that is an ordinary tap and opens nothing, matching the
    /// long-press-vs-tap gesture everywhere else on a touch surface.
    properties_press_started_at: Option<f64>,
    /// Whether the *current* held press already opened the overlay once —
    /// see `Shell::ui`'s own comment on why this, and not just
    /// `properties_overlay_open`, guards the long-press check. Reset only
    /// on release.
    properties_hold_opened_overlay: bool,
    /// Test-only override of whether the properties long-press button is
    /// down this frame — `None` in the shipped app, always reading the real
    /// header response instead. See `Shell::ui`'s own comment on the frame
    /// this stands in for.
    pub(crate) debug_properties_press_probe: Option<bool>,
    /// Whether the AI harness sidebar/overlay is open (`S.U14`) — the
    /// header's `IA` button (`header.rs`) is the only thing that flips
    /// this, the same single-writer trade `library_sidebar_open` already
    /// makes. Starts `true`: unlike library (which starts open because
    /// it is the panel an operator reaches for first), this starts open so
    /// the harness is visible the moment the window opens — `S.U14`'s own
    /// verification screenshot needs it visible with no synthetic input,
    /// and there is nothing about a fresh session where starting collapsed
    /// would be the more honest default.
    harness_open: bool,
    /// The settings overlay's own state (`S.AF`/`O.16`'s first slice):
    /// open/closed for the overlay itself, plus the cached `ShellMode`
    /// override (`data/settings.json` via `GET`/`POST /settings` — see
    /// `panels::settings::PanelState`'s own doc comment) that every
    /// frame's `arrangement_forced` call reads. Starts `Auto` — the
    /// shipped arrangement is the default until the operator (or the door,
    /// first frame) says otherwise.
    settings: settings::PanelState,
    settings_open: bool,
    /// The "new" gallery overlay (`shell::gallery`): opened by the nodes
    /// canvas's `new` button and the sequence's `import`.
    gallery: gallery::State,
    /// The global favourites and recents the galleries read (`favorites.rs`).
    favorites: favorites::Store,
    /// The pairing overlay's cache and its open flag (`panels::pairing`):
    /// the chrome's sync indicator and the Devices page read the same
    /// cache, refreshed once a second through the door.
    pairing: panels::pairing::PanelState,
    pairing_open: bool,
    /// The header's own state (`P4`): the `colecciones` menu's inline rename
    /// and "save current to" form, moved out of the library panel.
    header: header::PanelState,
    /// O.10b: the keyboard table (plan point 4) — the defaults merged with
    /// `data/keybinds.json`, held across frames and driven once per frame.
    keybinds: keybinds::Keybinds,
    /// The live key triggers the last `GET /state` reported, cached so the
    /// keyboard matches the same triggers HTTP fires without the shell
    /// walking app state itself (`regla 9`).
    grid_triggers: Vec<keybinds::KnownTrigger>,
    /// Which view the header's Edit|Show toggle is on. `Show` paints only the
    /// header and the overlays grid (`ui`); see [`ShellView`].
    view: ShellView,
    /// Whether the persisted view has been applied yet — `ui` does it once,
    /// on the first frame, so a `Show` saved last run opens in Show without
    /// a frame of `Edit` first and without re-applying on every frame.
    view_synced: bool,
    /// Devices whose monitor window this session has already asked for
    /// (`enter_show`). `POST /monitor` spawns a process and does not dedupe,
    /// so this set is what keeps a second entry from opening a second window.
    opened_monitors: HashSet<String>,
    /// The config file the layout is remembered in, once the app has set one
    /// (`remember_layout`). `None` in tests: nothing is ever written then.
    layout_file: Option<std::path::PathBuf>,
    /// The layout as last read from or written to `layout_file`; a frame
    /// writes only when the live layout differs from it.
    layout_saved: Option<metal_config::Layout>,
    /// A remembered layout not yet applied to egui and the canvas — applied
    /// on the first frame, before any panel is shown.
    layout_seed: Option<metal_config::Layout>,
    /// While `layout_seed` waits to settle: the viewport it was last applied
    /// at, and how many frames it has held. The compositor resizes the window
    /// after the first frames, and a panel clamped to a too-small viewport
    /// would store (and then save) the clamped size over the remembered one.
    layout_settle: Option<(egui::Vec2, u32)>,
}

impl Shell {
    pub fn new(theme: Theme, door: Box<dyn Call>) -> Self {
        Shell {
            base_theme: theme.clone(),
            theme,
            door,
            fetcher: None,
            focus: None,
            last_focus: None,
            rects: HashMap::new(),
            selection: Selection::None,
            last_selection: Selection::None,
            nodes: nodes::PanelState::default(),
            venue_popup: panels::venue_popup::State::default(),
            lanes: lanes_panel::PanelState::default(),
            library: library::PanelState::default(),
            properties: properties::PanelState::default(),
            graph_writes: None,
            ingest: crate::shell::lanes::IngestLane::default(),
            chat: chat::PanelState::default(),
            debug_text_focus_probe: false,
            library_sidebar_open: true,
            active_tab: Region::Nodes,
            properties_overlay_open: false,
            properties_press_started_at: None,
            properties_hold_opened_overlay: false,
            debug_properties_press_probe: None,
            harness_open: true,
            settings: settings::PanelState::default(),
            settings_open: false,
            gallery: gallery::State::default(),
            favorites: favorites::Store::load(&favorites::path()),
            pairing: panels::pairing::PanelState::default(),
            pairing_open: false,
            header: header::PanelState::default(),
            keybinds: keybinds::Keybinds::load(),
            grid_triggers: Vec::new(),
            view: ShellView::Edit,
            view_synced: false,
            layout_file: None,
            layout_saved: None,
            layout_seed: None,
            layout_settle: None,
            opened_monitors: HashSet::new(),
        }
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Hands the shell the preview worker (`window::run` is the one caller).
    /// Without one — the kittest harness, the restyle drill — every pumped
    /// image fetches inline on the frame's thread, exactly as it did before
    /// the worker existed; with one, [`frames::Frame::tick`] only polls.
    /// Hands the pairing overlay's POSTs to their own worker, so a `metal-sync`
    /// dial never holds a frame (`panels::pairing::PairWorker`).
    pub fn set_pair_worker(&mut self, worker: panels::pairing::PairWorker) {
        self.pairing.set_worker(worker);
    }

    /// Hands the lanes panel its worker (the preview's POSTs and the song
    /// fetch run there). A kittest shell never calls this and runs them inline.
    pub(crate) fn set_lane_worker(&mut self, worker: lane_worker::LaneWorker) {
        self.lanes.set_worker(worker);
    }

    /// Hands the shell the graph writer (`window::run` is the one caller).
    pub(crate) fn set_graph_writer(&mut self, writer: graph_writer::GraphWriter) {
        self.graph_writes = Some(writer);
    }

    /// Makes the layout persistent: `layout` is applied on the first frame,
    /// and every later change to panel sizes or the canvas view is written
    /// back to `file` once the pointer is released. Called once by the window
    /// at startup; a kittest shell never calls it.
    pub fn remember_layout(&mut self, file: std::path::PathBuf, layout: metal_config::Layout) {
        self.layout_file = Some(file);
        self.layout_saved = Some(layout.clone());
        self.layout_seed = Some(layout);
    }

    /// Applies a remembered layout on the first frame and writes any change
    /// back to the config file. A write waits for the pointer to come up, so
    /// a splitter drag or a canvas pan costs one write, not one per frame.
    fn sync_layout(&mut self, ctx: &egui::Context) {
        if let Some(seed) = self.layout_seed.clone() {
            let viewport = ctx.input(|i| i.viewport_rect()).size();
            let first = self.layout_settle.is_none();
            let (at, held) = self.layout_settle.unwrap_or((viewport, 0));
            if first || at != viewport {
                layout::seed(ctx, &seed);
                self.layout_settle = Some((viewport, 0));
            } else {
                self.layout_settle = Some((at, held + 1));
            }
            if first {
                if let Some(open) = seed.harness_open {
                    self.harness_open = open;
                }
                if let Some(open) = seed.library_sidebar_open {
                    self.library_sidebar_open = open;
                }
                if let Some(c) = seed.canvas {
                    self.nodes.set_view(egui::Vec2::new(c.pan[0], c.pan[1]), c.zoom, &self.theme);
                }
            }
            if held < 30 && !ctx.input(|i| i.pointer.any_down()) {
                ctx.request_repaint();
                return;
            }
            self.layout_seed = None;
        }
        let Some(path) = self.layout_file.clone() else {
            return;
        };
        if ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        let (pan, zoom) = self.nodes.view();
        let mut now = layout::capture(ctx, Some(metal_config::CanvasView { pan: [pan.x, pan.y], zoom }));
        now.harness_open = Some(self.harness_open);
        now.library_sidebar_open = Some(self.library_sidebar_open);
        if self.layout_saved.as_ref() == Some(&now) {
            return;
        }
        match layout::write(&path, now.clone()) {
            Ok(()) => self.layout_saved = Some(now),
            Err(e) => eprintln!("metal-dash: cannot remember the layout: {e}"),
        }
    }

    pub fn set_fetcher(&mut self, fetcher: frames::Fetcher) {
        self.fetcher = Some(fetcher);
    }

    /// What the header (`header.rs`) and, from `S.U9` on, a panel calls a
    /// route through — never a handler body, never a submodule of `call` by
    /// name. See [`crate::call::Call`]'s own doc comment for the shape and
    /// [`crate::call::Door`] for what answers on the other end of a real
    /// window.
    pub(crate) fn door(&self) -> &dyn Call {
        self.door.as_ref()
    }

    /// Every region's last-painted rect, for the kittest ceding test
    /// (`focus_expands_and_others_cede`, `kittest.rs`) — nothing in the
    /// shipped app reads its own layout back, so this is test-only in
    /// effect even though it isn't `#[cfg(test)]` in name (a plain getter
    /// is simpler than threading a second, cfg'd copy of `regions::paint`'s
    /// output).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn region_rects(&self) -> &HashMap<Region, Rect> {
        &self.rects
    }

    /// The graph canvas's own rect as of the last frame — `S.U9`'s pointer
    /// tests need a point inside the canvas itself, past the toolbar, not
    /// just somewhere inside `Region::Nodes`'s whole outer frame.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn nodes_canvas_rect(&self) -> Option<Rect> {
        self.nodes.canvas_rect()
    }

    /// Whether the nodes canvas is in its venue scene — test-only reader.
    #[cfg(test)]
    pub(crate) fn nodes_venue_scene_on(&self) -> bool {
        self.nodes.in_venue_scene()
    }

    /// The room the venue scene is painting — test-only reader.
    #[cfg(test)]
    pub(crate) fn nodes_venue_room_for_test(&self) -> Option<String> {
        self.nodes.venue_room_for_test()
    }

    /// Whether the venue gallery is open over the scene — test-only reader.
    #[cfg(test)]
    pub(crate) fn nodes_gallery_open(&self) -> bool {
        self.nodes.gallery.is_some()
    }

    /// A mask's own points canvas rect as of the last frame — `S.U10b`'s
    /// pointer tests need a point inside that specific mask's canvas, the
    /// same trade `nodes_canvas_rect` makes for the graph canvas. Moved to
    /// `properties` (22-sept-2026, `S.AP` correction): mask editing now
    /// lives under the output device's own properties section, not lanes.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn mask_canvas_rect(&self, mask_id: &str) -> Option<Rect> {
        self.properties.mask_canvas_rect(mask_id)
    }

    /// The wall-drawing venue editor's canvas rect as of the last frame —
    /// P4.4's kittest needs a point inside that canvas, the same trade
    /// `mask_canvas_rect` makes.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn venue_canvas_rect(&self) -> Option<Rect> {
        self.properties.venue_canvas_rect()
    }

    /// The lanes panel's own state — test-only writer, so a kittest test can
    /// preload a form field (`panels::lanes::PanelState::set_trigger_id`)
    /// without simulating keystrokes into a `TextEdit`.
    #[cfg(test)]
    pub(crate) fn lanes_state_mut(&mut self) -> &mut lanes_panel::PanelState {
        &mut self.lanes
    }

    /// The nodes panel's own state — test-only writer, so a kittest can
    /// preopen a graph (`R22.4`'s `+ crear botón`) without the picker.
    #[cfg(test)]
    pub(crate) fn nodes_state_mut(&mut self) -> &mut nodes::PanelState {
        &mut self.nodes
    }

    /// The properties panel's own state — test-only writer, so a kittest test
    /// can arm the overlay trigger id (`P5`) without simulating keystrokes.
    #[cfg(test)]
    pub(crate) fn properties_state_mut(&mut self) -> &mut properties::PanelState {
        &mut self.properties
    }

    /// The Colourist ingest lane's own state — test-only writer, the same
    /// trade `lanes_state_mut`/`properties_state_mut` make: a kittest test
    /// can arm the model/song/keyframes directly, bypassing UI interaction
    /// for setup.
    #[cfg(test)]
    pub(crate) fn ingest_state_mut(&mut self) -> &mut crate::shell::lanes::IngestLane {
        &mut self.ingest
    }

    /// The library panel's own state — test-only writer, the same trade
    /// `lanes_state_mut` makes: a kittest test can arm
    /// `panels::library::PanelState::set_confirm_open` without depending on
    /// where a synthetic click into a `menu_button` needs to land.
    #[cfg(test)]
    pub(crate) fn library_state_mut(&mut self) -> &mut library::PanelState {
        &mut self.library
    }

    /// What the graph canvas has selected — test-only reader, the mirror of
    /// `nodes_canvas_rect` above, for a kittest test to confirm a click
    /// actually changed `Shell::selection` and not just recorded a call.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn selection(&self) -> &Selection {
        &self.selection
    }

    /// The shell's current selection as a chat context ref `{kind, id, graph,
    /// label}`, or `None` when nothing is selected.
    /// The `@` candidates the chat composer offers: every node of the open
    /// graph, with the selected node marked focused. Library files, overlays
    /// and properties are not offered yet (see the module doc of `mention`).
    fn mention_candidates(&self) -> Vec<mention::Candidate> {
        let focus = self.chat_selection().and_then(|s| s["id"].as_str().map(str::to_string));
        let graph = self.nodes.graph_reply();
        let Some(nodes) = graph["graph"]["nodes"].as_array() else {
            return Vec::new();
        };
        nodes
            .iter()
            .filter_map(|n| {
                let id = n["id"].as_str()?;
                let label = n["label"].as_str().filter(|l| !l.is_empty()).unwrap_or(id);
                Some(mention::Candidate {
                    token: id.to_string(),
                    label: label.to_string(),
                    kind: mention::Kind::Node,
                    focused: focus.as_deref() == Some(id),
                    recent: 0,
                })
            })
            .collect()
    }

    fn chat_selection(&self) -> Option<serde_json::Value> {
        match &self.selection {
            Selection::None => None,
            Selection::Node(id) => Some(serde_json::json!({
                "kind": "node", "id": id, "graph": self.nodes.selected_graph(), "label": id,
            })),
            Selection::Edge(id) => Some(serde_json::json!({
                "kind": "edge", "id": id, "graph": self.nodes.selected_graph(), "label": id,
            })),
            Selection::File(f) => Some(serde_json::json!({
                "kind": "file", "id": f.path, "graph": null, "label": f.name,
            })),
        }
    }

    /// Applies a ref a chat chip asked for: a node or edge is selected (its
    /// graph opened first when the ref names one). Other kinds are ignored.
    fn chat_focus(&mut self, r: &serde_json::Value) {
        let Some(id) = r["id"].as_str().filter(|s| !s.is_empty()) else {
            return;
        };
        let selection = match r["kind"].as_str() {
            Some("node") => Selection::Node(id.to_string()),
            Some("edge") => Selection::Edge(id.to_string()),
            _ => return,
        };
        if let Some(graph) = r["graph"].as_str() {
            self.nodes.open_graph(&*self.door, graph);
        }
        self.selection = selection;
    }

    /// Test-only writer — sets `Shell::selection` directly, mirroring
    /// `lanes_state_mut`/`library_state_mut`'s own trade: a kittest test
    /// for the properties panel (`S.U12`) can hand it a selection without
    /// simulating a graph-canvas click all the way through a scripted
    /// `GET /graph/hit` reply, which `panels::nodes`'s own selection tests
    /// already cover end to end.
    #[cfg(test)]
    pub(crate) fn set_selection_for_test(&mut self, selection: Selection) {
        self.selection = selection;
    }

    /// Test-only writer — opens the venue gallery for `node` of `gid` over the
    /// canvas's venue scene without simulating the bar's `+` or a card click,
    /// the same trade `set_selection_for_test` makes.
    #[cfg(test)]
    pub(crate) fn request_venue_popup_for_test(&mut self, gid: &str, node: &str) {
        self.nodes.set_selected_graph_for_test(gid);
        self.nodes.open_venue_scene();
        self.nodes.open_gallery_for_test(node);
    }

    /// Test-only reader — the room popup's outer rect on its last painted
    /// frame, `the_gallery_keeps_its_height`'s hold on the fixed size.
    #[cfg(test)]
    pub(crate) fn venue_popup_rect_for_test(&self) -> Option<egui::Rect> {
        // The scene owns the gallery now, so the rect that matters is the one
        // the gallery component laid out last frame — whatever opened it.
        self.nodes
            .venue_gallery
            .rect_for_test()
            .or_else(|| self.venue_popup.rect_for_test())
    }

    /// Test-only writer — puts the room popup on a tab without clicking the
    /// tab list (the list's own click path is `nav_list`'s, tested once).
    #[cfg(test)]
    pub(crate) fn venue_popup_tab_for_test(&mut self, tab: panels::venue_popup::Tab) {
        self.venue_popup.set_tab(tab);
        self.nodes.venue_gallery.set_tab(tab);
    }

    /// Test-only drain — the node the room popup asked the scene to reveal.
    #[cfg(test)]
    pub(crate) fn venue_popup_reveal_for_test(&mut self) -> Option<String> {
        // The scene's gallery is the one that paints now; the standalone
        // wrapper's scratch is the fallback for a caller that still opens it.
        self.nodes
            .venue_gallery
            .take_reveal()
            .or_else(|| self.venue_popup.take_reveal())
    }

    /// Which of `graph`/`lanes` the header's tab selector currently shows —
    /// test-only reader, `S.U13`'s `kittest.rs` suite.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn active_tab(&self) -> Region {
        self.active_tab
    }

    /// Whether the library sidebar is expanded — test-only reader.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn library_sidebar_open(&self) -> bool {
        self.library_sidebar_open
    }

    /// Whether the properties long-press overlay is open — test-only
    /// reader.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn properties_overlay_open(&self) -> bool {
        self.properties_overlay_open
    }

    /// Whether the AI harness sidebar/overlay is open — test-only
    /// reader, `S.U14`'s `kittest.rs` suite.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn harness_open(&self) -> bool {
        self.harness_open
    }

    /// Test-only writer, mirroring `set_selection_for_test`: lets a kittest
    /// test force the harness closed (or open) without going through the
    /// header's own click, the same trade every other panel's `_mut`/`set_*`
    /// test hook in this file already makes.
    #[cfg(test)]
    pub(crate) fn set_harness_open_for_test(&mut self, open: bool) {
        self.harness_open = open;
    }

    /// Test-only writer — preloads the prompt box without simulating
    /// keystrokes into a `TextEdit`, the same trade
    /// `library::PanelState::set_add_path` makes for the library panel's
    /// own text field.
    #[cfg(test)]
    pub(crate) fn harness_prompt_for_test(&self) -> &str {
        self.chat.prompt_for_test()
    }

    pub(crate) fn set_harness_prompt_for_test(&mut self, text: &str) {
        self.chat.set_prompt_for_test(text);
    }

    /// The settings overlay's own state — test-only reader for the
    /// overlay-style asserts (`S.AF`'s kittest suite), plus the one-shot
    /// cached override a scripted-door test can preload.
    #[cfg(test)]
    pub(crate) fn settings_state_mut(&mut self) -> &mut settings::PanelState {
        &mut self.settings
    }

    /// The pairing overlay's open flag, for the sync chip's kittest.
    #[cfg(test)]
    pub(crate) fn pairing_open(&self) -> bool {
        self.pairing_open
    }

    #[cfg(test)]
    pub(crate) fn settings_open(&self) -> bool {
        self.settings_open
    }

    /// The one place `POST /transport` is called from — the `más ▾` freeze
    /// toggle (`header.rs`) and the global spacebar shortcut both funnel
    /// here, so there is exactly one code path for a capability that used
    /// to have exactly one (`#play`, `main.rs:1600` before `S.U4a`) — regla
    /// 6 in miniature, inside a single lote.
    fn toggle_transport(&mut self) {
        let _ = self.door().json("POST", "/transport", None);
    }

    /// Entering `Show` (the Edit→Show transition only, `ui`): asks for one
    /// monitor window per output that is not the preview, and unfreezes the
    /// clock if it is frozen — a show that opens on a frozen clock would
    /// look broken. A device already asked for this session is skipped, so
    /// a second entry without leaving adds no window.
    fn enter_show(&mut self) {
        let state = self.door().json("GET", "/state", None).ok();
        let devices: Vec<String> = state
            .as_ref()
            .and_then(|s| s["outputs"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|o| o["id"].as_str())
            .filter(|id| *id != crate::PREVIEW)
            .map(str::to_string)
            .collect();
        for id in devices {
            if self.opened_monitors.insert(id.clone()) {
                let _ = self.door().json("POST", "/monitor", Some(serde_json::json!({ "op": "open", "device": id })));
            }
        }
        // The header's freeze reading (`más ▾`) is `running` off the same
        // `GET /state`; only an explicit `false` is a frozen clock.
        if state.as_ref().and_then(|s| s["running"].as_bool()) == Some(false) {
            self.toggle_transport();
        }
    }

    /// Drains the three hooks the library panel raises
    /// (`paneles-inferiores.md` § P3): `tocar` an audio flips lanes to its
    /// own state; `cargar` loads a saved sequence into the scratch; `ver`
    /// shows one on the strip. Pure over the two panel states plus the door,
    /// so it is testable without a window.
    ///
    /// `R22.7`: the song arrives as its **stem** (the library's own
    /// `last_opened_song`), and a song with no model picked takes one — the
    /// selected `gen.*` node's model first, the manifest's first otherwise —
    /// so the ingest columns have everything `needs_load` asks for on the
    /// very next frame.
    fn route_library_intents(&mut self, ctx: &egui::Context) {
        let door: &dyn Call = self.door.as_ref();
        if let Some(name) = self.library.take_last_opened_song() {
            // The scratch is not cleared: the operator can be mid-phrase and
            // still change the song under it.
            self.lanes.force_mode(lanes::Mode::Lanes);
            self.lanes.mv.note = Some(format!("audio: {name}"));
            // `S.AP` correction (22-sept-2026): "the ingestion of the songs
            // should happen after load the lanes" — opening a song from
            // library also arms the ingest lane's own song field, so
            // `dataset_section`'s `needs_load` fires on the very next frame
            // if a model is already picked, with nothing further to click.
            self.ingest.song = name.clone();
            if self.ingest.model.is_empty() {
                if let Some((model, defaults)) = self.default_ingest_model(door) {
                    self.ingest.switch_model(&model, defaults);
                    self.lanes.mv.note = Some(format!("audio: {name} · model: {model}"));
                }
            }
        }
        if let Some(drag) = self.library.take_clip_to_layer() {
            // The same move a library video dropped on the timeline makes:
            // a clip on a new layer at the playhead.
            self.lanes.force_mode(lanes::Mode::Lanes);
            let len = crate::shell::timeline::video_ticks(drag.seconds);
            let at = self.ingest.playhead;
            self.ingest.add_video_clip(&drag.path, at, len, None);
            self.ingest.note = Some(format!("{} on layer {}", drag.name, self.ingest.layer + 1));
        }
        if let Some(file) = self.library.take_picked_file() {
            self.selection = Selection::File(file);
        }
        // `import` (2026-10-10): a library file or folder becomes a source in
        // the open graph. A folder is a `reel.batch` over the images and videos
        // `GET /fs` lists for it; a file is the same source the canvas drop
        // makes. No open graph is said on the library's own note line.
        if let Some(import) = self.library.take_import() {
            if self.nodes.selected_graph().is_none() {
                self.library.set_note("open a graph to import".to_string());
            } else {
                let built = match import.kind.as_str() {
                    "image" => Some(("still", serde_json::json!({ "file": import.path }))),
                    "video" => Some(("reel.clip", serde_json::json!({ "file": import.path }))),
                    "audio" => Some(("audio.file", serde_json::json!({ "file": import.path }))),
                    _ => {
                        let listing = get_json(door, "/fs", &[("path", import.path.clone())]).unwrap_or_default();
                        let items: Vec<serde_json::Value> = listing["files"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|f| {
                                let kind = f["kind"].as_str()?;
                                if !matches!(kind, "image" | "video") {
                                    return None;
                                }
                                let path = std::path::Path::new(&import.path).join(f["file"].as_str()?);
                                Some(serde_json::json!({ "kind": kind, "path": path.to_string_lossy() }))
                            })
                            .collect();
                        if items.is_empty() {
                            self.library.set_note("no images or videos in that folder".to_string());
                            None
                        } else {
                            Some(("reel.batch", serde_json::json!({ "items": items })))
                        }
                    }
                };
                if let Some((src_kind, config)) = built {
                    if let Err(e) = nodes::import_source(door, &mut self.nodes, &mut self.selection, src_kind, config) {
                        self.library.set_note(e);
                    }
                }
            }
        }
        if let Some(name) = self.library.take_sequence_to_load() {
            if let Some(m) = get_json(door, "/author/models/movement", &[("name", name.clone())]) {
                if self.lanes.mv.load(&m) {
                    self.lanes.force_mode(lanes::Mode::Lanes);
                    self.lanes.mv.note = Some(format!("opened: {name}"));
                }
            }
        }
        let shown = self.library.take_sequence_to_show().or_else(|| self.lanes.sequence_dropped.take());
        if let Some(name) = shown {
            if let Some(m) = get_json(door, "/author/models/movement", &[("name", name)]) {
                self.lanes.show_sequence(m);
                self.lanes.force_mode(lanes::Mode::Lanes);
            }
        }
        // 22-sept-2026 (tarde): an `archivos` **audio** card dropped on the
        // overlays grid raises the same intent `tocar` does, but the lanes
        // panel cannot reach `self.ingest` — so it holds the name and this
        // drain runs the same block the library's own `tocar` runs.
        if let Some(name) = self.lanes.pending_song.take() {
            self.lanes.force_mode(lanes::Mode::Lanes);
            self.lanes.mv.note = Some(format!("audio: {name}"));
            self.ingest.song = name.clone();
            if self.ingest.model.is_empty() {
                if let Some((model, defaults)) = self.default_ingest_model(door) {
                    self.ingest.switch_model(&model, defaults);
                    self.lanes.mv.note = Some(format!("audio: {name} · model: {model}"));
                }
            }
        }
        // The simplified sequence's `enable effects` (2026-10-15): the media
        // board's clips become video clips on the authoring timeline, so the
        // full float authoring opens over exactly what was reproduced.
        if let Some(clips) = crate::shell::panels::sequence::take_enable_effects(ctx) {
            if !clips.is_empty() {
                self.lanes.force_mode(lanes::Mode::Lanes);
                for clip in &clips {
                    let (a, b) = clip.span();
                    let start = crate::shell::timeline::video_ticks(Some(a));
                    let len = crate::shell::timeline::video_ticks(Some(b - a));
                    self.ingest.add_video_clip(&clip.path, start, len, None);
                }
                self.ingest.note = Some(format!("{} clips · effects", clips.len()));
            }
        }
    }

    /// The model a freshly-opened song should be authored against when the
    /// ingest lane has none picked (`R22.7` punto 4): the selected node's own
    /// model when the selection is a `gen.*`, the manifest's first entry
    /// otherwise — with that model's own `defaults`, the same body
    /// `dataset_section`'s combo reads. `None` when the manifest itself is
    /// unreachable, which leaves the model combo at `—` rather than guessing.
    fn default_ingest_model(&self, door: &dyn Call) -> Option<(String, Vec<f32>)> {
        let manifest = get_json(door, "/author/models/state", &[])?;
        let models = manifest["models"].as_array()?.clone();
        let selected = match &self.selection {
            Selection::Node(id) => self.nodes.selected_graph().and_then(|gid| {
                let graph = get_json(door, "/graph", &[("id", gid.to_string())])?;
                let kind = graph["graph"]["nodes"]
                    .as_array()?
                    .iter()
                    .find(|n| n["id"].as_str() == Some(id.as_str()))
                    .and_then(|n| n["kind"].as_str())?;
                kind.strip_prefix("gen.").map(str::to_string)
            }),
            _ => None,
        };
        let pick = selected
            .and_then(|m| {
                models
                    .iter()
                    .find(|x| x["id"].as_str() == Some(m.as_str()))
                    .cloned()
            })
            .or_else(|| models.first().cloned())?;
        let id = pick["id"].as_str()?.to_string();
        let defaults = pick["defaults"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_f64())
            .map(|v| v as f32)
            .collect();
        Some((id, defaults))
    }

    /// Paint one frame: the fixed header (`header.rs`), the spacebar
    /// shortcut, and the ceding grid (`regions.rs`). Takes the root [`Ui`]
    /// rather than a `Context` because that is what egui 0.36's
    /// `Panel::show`/`CentralPanel::show` take — the caller (`window.rs`,
    /// or a test harness) hands it the `Ui` that `Context::run_ui`
    /// provides.
    pub fn ui(&mut self, ui: &mut Ui) {
        let theme = self.theme.clone();
        // A controller key pressed while the window idles must paint at once:
        // the MIDI thread wakes this context through the surface's waker.
        static WAKER: std::sync::Once = std::sync::Once::new();
        WAKER.call_once(|| {
            let ctx = ui.ctx().clone();
            crate::input::set_surface_waker(move || ctx.request_repaint());
            // A read the door answers in the background wakes the UI when it
            // lands, so a panel that showed its empty state adopts the reply
            // (`reads::CachedDoor`).
            let ctx = ui.ctx().clone();
            self.door.set_wake(std::sync::Arc::new(move || ctx.request_repaint()));
        });
        perf_hud::paint(ui, &theme);

        // The viewport `classes::arrangement` decides against — the whole
        // window, not `ui`'s own rect (`ui` here is the root `Ui` for the
        // frame, but reading it before the header/grid panels have taken
        // their own space is what makes this "the viewport" rather than
        // "whatever is left of it"). `window.rs`'s `redraw` hands egui the
        // real current inner size every frame (`egui_winit::State::take_egui_input`
        // reads `window.inner_size()` live), so this follows a real resize
        // with no extra plumbing on this file's part.
        // the real current inner size every frame
        // (`egui_winit::State::take_egui_input` reads `window.inner_size()`
        // live), so this follows a real resize with no extra plumbing on
        // this file's part.
        //
        // The settings cache (`S.AF`): one `GET /settings` on the first
        // frame with a door at all, then only `POST`s from the overlay's
        // own dropdown keep it true — the arrangement below reads it every
        // frame, which is exactly why it is a cache and not a per-frame
        // re-fetch (`panels::settings::PanelState::refresh`'s own doc
        // comment). Read before the arrangement picks, so a manual mode
        // saved on a previous run forces itself from the window's very
        // first frame, not a frame late.
        let door_read: &dyn Call = self.door.as_ref();
        self.settings.refresh(door_read);
        // The Theme setting changed (or loaded): rebuild the theme from its
        // palette and push it into egui, so the whole client recolours.
        if let Some(colors) = self.settings.take_theme_change() {
            self.theme = if colors.is_empty() { self.base_theme.clone() } else { self.base_theme.with_palette(&colors) };
            self.theme.apply(ui.ctx());
        }
        // The global UI scale: egui's own keyboard zoom is off so Ctrl +/-/0
        // go through the one settings path (`keybinds::scale_steps`).
        ui.ctx().options_mut(|o| o.zoom_with_keyboard = false);
        if let Some(steps) = keybinds::scale_steps(ui.ctx()) {
            self.settings.step_ui_scale(door_read, steps);
        }
        if (ui.ctx().zoom_factor() - crate::ui_scale::factor()).abs() > f32::EPSILON {
            ui.ctx().set_zoom_factor(crate::ui_scale::factor());
        }
        // The persisted view, applied once on the first frame: a Show saved
        // last run opens in Show, and its monitors are asked for here — the
        // first frame that paints — never before (`enter_show`).
        if !self.view_synced {
            self.view_synced = true;
            if self.settings.shell_view() == ShellView::Show {
                self.view = ShellView::Show;
                self.enter_show();
            }
        }
        if let Some(error) = self.graph_writes.as_ref().and_then(|w| w.take_errors().pop()) {
            self.properties.set_write_note(error);
        }
        self.sync_layout(ui.ctx());
        self.pairing.refresh_due(self.door.as_ref(), std::time::Instant::now());
        if self.pairing.busy() {
            // The worker's answer lands on no event of its own; keep frames
            // coming until it does (the spinner needs them anyway).
            frames::repaint_after(ui.ctx(), std::time::Duration::from_millis(100));
        }

        let viewport = ui.input(|i| i.viewport_rect()).size();
        // The settings override (`S.AF`) can force the shell mode against
        // the viewport's own answer — that is what the dropdown exists
        // for. `Auto` hands straight through to the `S.U13` thresholds.
        let arrangement =
            classes::arrangement_forced(&theme, viewport, self.settings.mode_override());

        if arrangement.properties != Placement::Delegated {
            // Not delegated any more (a resize back to a wider window) —
            // nothing to hold a press timer or an open overlay for.
            self.properties_overlay_open = false;
            self.properties_press_started_at = None;
            self.properties_hold_opened_overlay = false;
        }

        let active_tab = self.active_tab;
        let library_sidebar_open = self.library_sidebar_open;
        let harness_open = self.harness_open;
        let view = self.view;
        let header_door: &dyn Call = self.door.as_ref();
        let header_state = &mut self.header;
        // System bars (edge-to-edge on Android): reserve their height so the
        // header and the bottom edge sit clear of them. Zero on desktop.
        let (bar_top, bar_bottom) = insets::system_bars_px();
        let ppp = ui.ctx().pixels_per_point();
        if bar_top > 0.0 {
            Panel::top(Id::new("shell/sysbar-top"))
                .exact_size(bar_top / ppp)
                .frame(ground_frame(&theme))
                .show(ui, |_| {});
        }
        if bar_bottom > 0.0 {
            Panel::bottom(Id::new("shell/sysbar-bottom"))
                .exact_size(bar_bottom / ppp)
                .frame(ground_frame(&theme))
                .show(ui, |_| {});
        }
        let pairing_state = &self.pairing;
        let mut events: HeaderEvents = Panel::top(Id::new("shell/header"))
            .exact_size(theme.header_height)
            .frame(header_frame(&theme))
            .show(ui, |ui| {
                header::paint(
                    ui,
                    &theme,
                    header_door,
                    header_state,
                    arrangement,
                    active_tab,
                    library_sidebar_open,
                    harness_open,
                    view,
                    pairing_state,
                )
            })
            .inner;

        if events.sidebar_toggled {
            self.library_sidebar_open = !self.library_sidebar_open;
        }
        if let Some(tab) = events.tab_selected {
            self.active_tab = tab;
        }
        if events.harness_toggled {
            self.harness_open = !self.harness_open;
        }
        // 2026-10-01, the phone shell: library on the left, the harness on
        // the right, both sidebars — and a phone holds one of them beside the
        // content, never two. The one just opened wins; at boot (both start
        // open) the library does, since it is what an operator reaches first.
        if arrangement.harness == Placement::Delegated
            && arrangement.library == Placement::Delegated
            && self.library_sidebar_open
            && self.harness_open
        {
            if events.harness_toggled {
                self.library_sidebar_open = false;
            } else {
                self.harness_open = false;
            }
        }
        if events.settings_toggled {
            self.settings_open = !self.settings_open;
        }
        // The Edit|Show toggle. Entering Show is the transition that asks for
        // the monitors (`enter_show`); the choice is persisted so the next
        // run opens the same way.
        if let Some(view) = events.view_selected {
            if view != self.view {
                self.view = view;
                self.settings.set_shell_view(self.door.as_ref(), view);
                if view == ShellView::Show {
                    self.enter_show();
                }
            }
        }
        let show = self.view == ShellView::Show;
        if events.pairing_toggled {
            self.pairing_open = !self.pairing_open;
        }
        if let Some((graph, node)) = events.select_node.take() {
            self.nodes.open_graph(&*self.door, &graph);
            self.selection = Selection::Node(node);
        }

        // The AI harness's docked form (`S.U14`, `O.12.f`): a right
        // `side_panel`, painted before the ground `CentralPanel` below so it
        // claims its width first — the same ordering `library`'s own left
        // sidebar (`regions::paint_delegated`) uses, and the reason this has
        // to happen here rather than inside `regions::paint`: the harness is
        // not one of `Region::ALL` and never competes for `focus`/`cede`
        // space, so it sits outside the ceding grid entirely (see
        // `classes::Arrangement::harness`'s own doc comment). Its overlay
        // form (`Placement::Delegated`, phone-sized) paints later instead,
        // after the grid, the same way properties' overlay does below.
        if self.harness_open && !show {
            let chat_selection = self.chat_selection();
            let width = theme.sidebar_width(viewport.x);
            let (min, max) = theme.sidebar_bounds(viewport.x);
            side_panel(
                Id::new("shell/harness-sidebar"),
                SidebarSide::Right,
                width,
                min,
                max,
            )
            .show(ui, |ui| {
                self.chat.sources = self.mention_candidates();
                chat::paint(ui, &theme, self.door.as_ref(), &mut self.chat, chat_selection.as_ref());
            });
            // A ref chip clicked in the sidebar: select it on the canvas.
            if let Some(r) = self.chat.pending_focus.take() {
                self.chat_focus(&r);
            }
        }

        // The long-press gesture (`S.U13`, `O.12.f`): `events.properties_button_down`
        // is only ever `true` while `header::paint` painted the button *and*
        // the pointer is down on it this frame — a run of that past
        // `theme.long_press`, timed against `ui.input(|i| i.time)` (not a
        // frame count, so this reads the same under any frame rate,
        // including a kittest harness that never renders a real frame),
        // opens the overlay. Letting go before the threshold is an
        // ordinary tap and opens nothing.
        // `debug_properties_press_probe`, when set, stands in for the real
        // button's `is_pointer_button_down_on()` — the same trade
        // `debug_text_focus_probe` makes above for keyboard focus:
        // synthesising a *held* pointer press against a specific widget
        // across several kittest frames (rather than a single press+release
        // pair `egui_kittest::Node::click` sends together) runs into
        // `egui`'s hit-testing lag-by-one-frame against **last** frame's
        // widget rects, which every other click-based test in this file
        // sidesteps by sending press and release together. This lote's own
        // long-press *timing* (`Shell::ui`'s threshold math below) is what a
        // test needs to drive directly; the button's own sensing is the
        // same `ui.button` mechanism every other tested button already
        // proves works.
        let button_down = self
            .debug_properties_press_probe
            .unwrap_or(events.properties_button_down);
        // 2026-10-01: on a touch surface the long-press is on the *content*,
        // not only on the header button — hold a node, a clip, a file, still,
        // and its properties come up. A press that moves is a drag (the
        // canvas pans, a clip slides) and never counts; a press on the header
        // is the header's own.
        let content_held = ui.input(|i| {
            i.pointer.primary_down()
                && !i.pointer.is_decidedly_dragging()
                && i.pointer.press_origin().is_some_and(|p| p.y > theme.header_height)
                && i.pointer.press_start_time().is_some_and(|t| i.time - t >= f64::from(theme.long_press))
        });
        let released = ui.input(|i| i.pointer.primary_released());
        // The release of the hold that opened the overlay is a click outside
        // it; that one click must not close what it just opened.
        let hold_releasing = released && self.properties_hold_opened_overlay;
        if content_held && !show && arrangement.properties == Placement::Delegated && !self.properties_hold_opened_overlay {
            self.properties_overlay_open = true;
            self.properties_hold_opened_overlay = true;
        }
        if content_held || (self.properties_hold_opened_overlay && ui.input(|i| i.pointer.primary_down())) {
            // Held: the button path below must not reset the guard.
        } else if button_down {
            let now = ui.input(|i| i.time);
            let started = *self.properties_press_started_at.get_or_insert(now);
            // `properties_hold_opened_overlay` guards against the same held
            // press reopening the overlay the instant Esc/click-outside
            // closes it: without it, a press that is still down and still
            // past `theme.long_press` would satisfy this condition again on
            // the very next frame after a manual close. It only resets on
            // release, below — a fresh press earns a fresh long-press.
            if !self.properties_hold_opened_overlay && now - started >= f64::from(theme.long_press)
            {
                self.properties_overlay_open = true;
                self.properties_hold_opened_overlay = true;
            }
        } else {
            self.properties_press_started_at = None;
            self.properties_hold_opened_overlay = false;
        }

        // The global shortcut, `plan-opus.md` § O.12.a point 1: space
        // toggles the transport freeze exactly like `más ▾`'s item —
        // *except* while a text field has keyboard focus, so typing a
        // space into a future panel's text box never doubles as this.
        // `ui.memory(|m| m.focused())` is the same mechanism a `TextEdit`
        // registers itself under when clicked, which is what
        // `debug_text_focus_probe` exercises for real in `kittest.rs`
        // rather than standing in for it.
        let space_pressed = ui.input(|i| i.key_pressed(egui::Key::Space));
        let text_field_focused = ui.memory(|m| m.focused().is_some());
        let toggled_by_space = space_pressed && !text_field_focused;

        if self.debug_text_focus_probe {
            let id = Id::new("shell/debug/text-probe");
            let mut buf = String::new();
            ui.add(egui::TextEdit::singleline(&mut buf).id(id));
        }

        // Direct field access, not `self.door()` — that method takes `&self`
        // for the whole struct, which would collide with the `&mut
        // self.nodes`/`&mut self.selection` borrows regions::paint also
        // needs this frame. The three fields are disjoint, so borrowing
        // them this way (rather than through a method) is what lets the
        // compiler see that.
        let door: &dyn Call = self.door.as_ref();
        let fetcher = self.fetcher.as_ref();
        if let Some(f) = fetcher {
            frames::publish_fetcher(ui.ctx(), f);
        }
        // `Ctrl+Z`: the focused view's own undo (`keybinds::undo_target`).
        // Global, so it works wherever the focus is, and it backs off while
        // a text field is typing (`keybinds::undo_pressed`).
        if keybinds::undo_pressed(ui.ctx()) {
            match keybinds::undo_target(self.focus) {
                Some(Region::Nodes) => nodes::undo_last(door, &mut self.nodes),
                Some(Region::Lanes) => self.ingest.undo(),
                _ => {}
            }
        }
        if show {
            // `Show`: the overlays grid alone, over the whole central area.
            // Regions, sidebars and the properties overlay are not painted, so
            // no region has a rect this frame (`region_rects` reads these).
            self.rects.clear();
            CentralPanel::default_margins()
                .frame(ground_frame(&theme))
                .show(ui, |ui| {
                    lanes_panel::paint_show_grid(
                        ui,
                        &theme,
                        door,
                        &mut self.lanes,
                        self.nodes.selected_graph(),
                    );
                });
        } else {
            CentralPanel::default_margins()
                .frame(ground_frame(&theme))
                .show(ui, |ui| {
                    let (clicked, rects) = regions::paint(
                        ui,
                        &theme,
                        self.focus,
                        arrangement,
                        self.active_tab,
                        self.library_sidebar_open,
                        door,
                        fetcher,
                        &mut self.nodes,
                        &mut self.lanes,
                        &mut self.library,
                        &mut self.properties,
                        &mut self.ingest,
                        &mut self.selection,
                    );
                    self.rects = rects;
                    // A click on empty lanes space while the lanes region
                    // has the focus drops it (the layer card goes with it);
                    // from elsewhere it focuses the lanes as before.
                    let blank = self.lanes.take_blank_click();
                    if blank && self.focus == Some(Region::Lanes) {
                        self.focus = None;
                    } else if let Some(region) = clicked {
                        self.focus = Some(region);
                    }
                });

            // TASK 3: the library's media preview belongs to the library.
            // The moment another region takes the focus — or the selection
            // moves off a `File` — drop the board (and the panel's own
            // preview) so `preview_overlay` finds no subject and stops
            // painting. Both are transitions: a click inside the library, or
            // on a file row while another region already holds the focus,
            // changes neither focus nor selection away from `File`, so the
            // operator's own pick never dismisses the preview it raised.
            let focused_away = self.focus != self.last_focus
                && self.focus != Some(Region::Library);
            let selected_away = self.selection != self.last_selection
                && !matches!(&self.selection, Selection::File(_));
            if focused_away || selected_away {
                media::clear(ui.ctx());
                self.library.dismiss_preview();
            }
            self.last_focus = self.focus;
            self.last_selection = self.selection.clone();
        }

        // The properties browser's clicked table rows ask to select a node —
        // the smallest selection plumbing that keeps `regions.rs` untouched:
        // the panel holds the request and this drains it once per frame.
        if let Some(node) = self.properties.take_pending_selection() {
            self.selection = Selection::Node(node);
        }
        // The properties panel's cross-panel reveal (plan "Surface sources"):
        // a node it just made is brought into view by the nodes canvas on its
        // next paint. The selection is not touched — the editing stays put.
        if let Some(node) = self.properties.take_pending_reveal() {
            self.nodes.reveal(node);
        }
        // F8: the Settings "En el grafo" column's "go to node" — same reveal,
        // drained the same way.
        if let Some(node) = self.settings.take_pending_reveal() {
            self.selection = Selection::Node(node.clone());
            self.nodes.reveal(node);
        }

        // The long-press overlay itself: painted last, so it lands on top
        // of the grid — `panels::properties::PanelState::ui` reused as-is
        // (that module's own doc comment names this as the exact seam
        // `S.U13` was left to use), never forked. Closed by Esc or a click
        // outside it (`Response::clicked_elsewhere`) — never by letting go
        // of the long-press button, so a still-held press does not fight a
        // user who has started reading the panel.
        if arrangement.properties == Placement::Delegated && self.properties_overlay_open && !show {
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            let overlay_focus = self.lanes.focused_overlay_name().map(str::to_string);
            let lanes_focused = self.focus == Some(Region::Lanes);
            let area = egui::Area::new(Id::new("shell/properties-overlay"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ui.ctx(), |ui| {
                    panel_frame(&theme).show(ui, |ui| {
                        ui.set_width(theme.region_min.x.max(viewport.x * theme.properties_share));
                        let full_rect = ui.max_rect();
                        self.properties.ui(
                            ui,
                            &theme,
                            door,
                            &mut self.ingest,
                            &self.selection,
                            &self.nodes.picked_nodes(&self.selection),
                            self.nodes.selected_graph(),
                            overlay_focus.as_deref(),
                            lanes_focused,
                            full_rect,
                        )
                    })
                });
            if escape || (!hold_releasing && area.response.clicked_elsewhere()) {
                self.properties_overlay_open = false;
            }
        }

        // The AI harness on a phone (2026-10-01): the same right sidebar
        // as on a desktop, painted above — no overlay of its own any more.
        // Esc still closes it there, as the overlay used to.
        if arrangement.harness == Placement::Delegated
            && self.harness_open
            && !self.properties_overlay_open
            && !self.settings_open
            && ui.input(|i| i.key_pressed(egui::Key::Escape))
        {
            self.harness_open = false;
        }

        // The settings overlay itself (`S.AF`): the same foreground-`Area`
        // mechanism the two overlays above use, closed the same two ways
        // (Esc, click outside) and guarded the same one frame against the
        // click that opened it (`clicked_elsewhere()` would otherwise see
        // that click and close the overlay the instant after it opened —
        // see the harness overlay's own comment for the exact race).
        // Painted last of the three on purpose: a settings window belongs
        // on top of an (already open) harness overlay. Its shell-mode
        // dropdown re-enters the layout through `arrangement_forced` at
        // the *next* frame — the current frame's grid was already laid out
        // by the time this paint ran, which is the honest behaviour for
        // the properties long-press state a mode flip would otherwise
        // strand half-open.
        if self.settings_open {
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            let width = theme.region_min.x.max(viewport.x * theme.properties_share);
            let mut open = true;
            // The floating window the "create" button's overlay is: its header drags it
            // and carries the close "x"; foreground so it sits above every other panel.
            let window = gallery::overlay_window("shell/settings-overlay", i18n::tr("settings").to_string())
                .default_width(width)
                .order(egui::Order::Foreground)
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    ui.set_width(width);
                    let open_graph = self.nodes.selected_graph().map(str::to_string);
                    panels::settings::paint(
                        ui,
                        &theme,
                        door,
                        &mut self.settings,
                        &self.pairing,
                        open_graph.as_deref(),
                    );
                });
            if !open {
                self.settings_open = false;
            } else if !events.settings_toggled && (escape || window.is_some_and(|w| w.response.clicked_elsewhere())) {
                self.settings_open = false;
            }
        }

        // The "new" gallery (`shell::gallery`): the nodes canvas's `new` and the
        // sequence's `import` ask for it; a pick is routed back to whoever
        // asked. A card dragged out closes it and lands on the canvas.
        let ctx = ui.ctx().clone();
        if self.nodes.take_new_request() {
            self.gallery.open(gallery::Target::Nodes, ctx.cumulative_pass_nr());
        }
        if self.lanes.take_import_request() {
            self.gallery.open(gallery::Target::Sequence, ctx.cumulative_pass_nr());
        }
        // Stars toggled this frame (a gallery card or a Properties card) land
        // in the global store, which is written at once.
        let toggles = favorites::take_toggles(&ctx);
        if !toggles.is_empty() {
            for entry in toggles {
                self.favorites.toggle(entry);
            }
            if let Err(e) = self.favorites.save(&favorites::path()) {
                eprintln!("metal-dash: favourites not saved: {e}");
            }
        }
        favorites::publish(&ctx, &self.favorites);
        // The nodes panel cycles its gallery pictures through the model's float
        // variants only while the gallery is on screen.
        self.nodes.set_gallery_open(
            self.gallery.is_open(),
            self.gallery.cards_visible(),
            ctx.input(|i| i.time),
            self.gallery.visible_kinds(),
        );
        if let Some(target) = self.gallery.target() {
            let (title, mut sections) = match target {
                gallery::Target::Nodes => (i18n::tr("new node"), self.nodes.gallery_sections(door, &self.selection)),
                gallery::Target::Sequence => (i18n::tr("import"), lanes_import_sections(door, &self.nodes)),
            };
            let nodes = &self.nodes;
            let mut leading = favorites::sections(&self.favorites, &|kind| nodes.tiles_for(kind));
            leading.append(&mut sections);
            sections = leading;
            favorites::mark(&mut sections, &self.favorites);
            if let gallery::Outcome::Pick(kind) = gallery::paint(&ctx, &theme, &mut self.gallery, title, &sections) {
                ctx.request_repaint();
                if let Some(item) = sections.iter().flat_map(|s| s.items.iter()).find(|i| i.kind == kind) {
                    self.favorites.note_recent(favorites::entry_of(item));
                    if let Err(e) = self.favorites.save(&favorites::path()) {
                        eprintln!("metal-dash: recents not saved: {e}");
                    }
                }
                match target {
                    gallery::Target::Nodes => self.nodes.add_from_gallery(door, &mut self.selection, &kind, &theme),
                    gallery::Target::Sequence => self.lanes.import_pick(kind),
                }
            }
        }
        gallery::paint_drag_ghost(&ctx, &theme);

        // The venue scene (`panels::venue_canvas`) is the canvas's own view
        // state, entered by the gear on a `venue.surface` node's floating bar
        // — the room of the open graph, painted whole where the graph was.
        // The gear's press flips inside `take_venue_request` itself.
        let _ = self.nodes.take_venue_request();
        // The `output.device` page's "abrir la escena" asks for the room its
        // `venue` output feeds (2026-10-22). Hand the request to the canvas's
        // one channel so the gear and the page share a single door (rule 4).
        if let Some(request) = self.properties.take_venue_request() {
            self.nodes.offer_venue_request(request);
            let _ = self.nodes.take_venue_request();
        }
        if self.properties.take_venue_popup_request() {
            self.nodes.open_venue_scene();
        }
        // The standalone gallery wrapper is never open beside the scene (the
        // scene paints the gallery from its own scratch): a state left open
        // here is one a graph replaced the scene under.
        if self.venue_popup.is_open() && self.nodes.venue_scene.is_none() {
            self.venue_popup.close();
        }

        // The pairing overlay (`panels::pairing`): the same foreground `Area`
        // as settings, opened by the header's sync indicator and closed the
        // same two ways, with the same one-frame guard against its own click.
        if self.pairing_open {
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            let width = theme.region_min.x.max(viewport.x * theme.properties_share);
            let area = egui::Area::new(Id::new("shell/pairing-overlay"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ui.ctx(), |ui| {
                    ui.set_width(width);
                    panels::pairing::paint(ui, &theme, door, &mut self.pairing);
                });
            if !events.pairing_toggled && (escape || area.response.clicked_elsewhere()) {
                self.pairing_open = false;
            }
        }

        // O.10b: the keyboard as a reader of the same `Trigger` (plan point
        // 4). Once per frame, after the grid painted and `focus` is this
        // frame's, collect the pressed keys and drive the table. Kept before
        // the transport toggle below so this frame's `door` borrow is let go
        // before `toggle_transport` takes `&mut self`.
        let key_events: Vec<(egui::Key, egui::Modifiers)> = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| {
                    if let egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } = e
                    {
                        Some((*key, modifiers.clone()))
                    } else {
                        None
                    }
                })
                .collect()
        });
        // Refresh the cached live triggers on the same cadence the header
        // already polls `/state` (every frame; the production `CachedDoor`
        // dedups within its TTL).
        if let Ok(state_val) = door.json("GET", "/state", None) {
            self.grid_triggers = keybinds::triggers_from_state(&state_val);
        }
        let learning = crate::input::learn().lock().ok().and_then(|g| g.clone());
        // Never fire while a text field has keyboard focus — the same guard
        // the `Space` shortcut keeps.
        if !text_field_focused {
            keybinds::drive(
                &self.keybinds,
                keybinds::KbContext::from_focus(self.focus),
                &key_events,
                door,
                learning.as_ref(),
                &self.grid_triggers,
            );
        }
        // Ctrl+V: egui-winit intercepts the paste chord itself and emits
        // `Event::Paste` (a keybind row for it would never fire), so the
        // clipboard is read here, once per frame, and pasted into the open
        // graph — the text, or the file a single-line path names. Same
        // no-text-field guard as the shortcuts above.
        if !text_field_focused {
            let pasted: Option<String> = ui.input(|i| {
                i.events.iter().find_map(|e| {
                    if let egui::Event::Paste(t) = e {
                        Some(t.clone())
                    } else {
                        None
                    }
                })
            });
            if let Some(text) = pasted {
                if let Err(e) =
                    nodes::paste_into_canvas(door, &mut self.nodes, &mut self.selection, &text, &theme)
                {
                    self.library.set_note(e);
                }
            }
        }
        // The smallest sign that a key learn is armed: "waiting…" over the
        // grid, through `tr` like every painted string (`regla 5`).
        if let Some(arm) = learning.as_ref() {
            if arm.kind == "key" {
                if let Some(rect) = self.rects.get(&Region::Lanes) {
                    egui::Area::new(Id::new("shell/keybinds/waiting"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(rect.min)
                        .show(ui.ctx(), |ui| {
                            ui.label(crate::shell::i18n::tr("waiting…"));
                        });
                }
            }
        }

        if toggled_by_space {
            self.toggle_transport();
        }

        // The library panel's intents, drained once per frame after it
        // painted (`P3`): a `tocar`/`cargar`/`ver` this frame is acted on
        // before the next frame's lanes paint. Here, not beside the grid
        // paint, because the frame's `door` borrow is still live up there.
        self.route_library_intents(ui.ctx());

        // The MIDI controller drives the overlays grid even while it is not on
        // screen (the sequence editor open): a no-op when the grid painted.
        lanes_panel::drive_offscreen(ui, self.door.as_ref(), &mut self.lanes);
    }
}

/// A `GET` with a query string, for the wiring's two movement lookups — the
/// door's `json` helper only builds a JSON body, and these two routes read
/// their argument off the query (`params.param("name")`).
fn get_json(door: &dyn Call, path: &str, query: &[(&str, String)]) -> Option<serde_json::Value> {
    let mut q = HashMap::new();
    for (k, v) in query {
        q.insert((*k).to_string(), v.clone());
    }
    let reply = door.call(
        "GET",
        path,
        crate::call::Params {
            query: q,
            body: Vec::new(),
            tail: None,
        },
    )?;
    if reply.status == 200 {
        crate::json_memo::parse(&reply.body).ok()
    } else {
        None
    }
}

/// A recording stand-in for [`Call`], for tests that build a [`Shell`] and
/// care about structure or style — `gates.rs`'s restyle drill, `kittest.rs`'s
/// harness — not about what a real handler does. Building a real
/// [`crate::call::Door`] means a real painter (`paint::painter()`, a real
/// wgpu adapter) and a real `Author::new()`; those tests are not testing
/// that, so paying for it would only make them slower and, on a host whose
/// adapter is flaky, more likely to fail for a reason that has nothing to do
/// with what they assert. `call::door::tests::the_door_is_send_and_locks_per_call`
/// is where the real `Door` gets exercised.
/// `S.U9` extended this from a plain `(method, path)` recording to also
/// keep each call's [`crate::call::Params`] (so a test can read a `GET
/// /graph/hit`'s `x`/`y`, proving the pointer coordinates that reached the
/// door — `plan-sonnet.md` § S.U9's own test list) and to let a test script
/// a canned JSON reply per `(method, path)` — without that, `S.U9`'s panel
/// (which decides what to load next from what `GET /graphs` answered) would
/// only ever see the bare `{}` every earlier test was content with, and the
/// "opening the nodes panel requests `/graphs` and `/graph`" test would
/// have nothing to chain the second call from.
#[cfg(test)]
pub(crate) struct FakeDoor {
    pub(crate) calls: std::cell::RefCell<Vec<(String, String, crate::call::Params)>>,
    responses: std::cell::RefCell<std::collections::HashMap<(String, String), serde_json::Value>>,
    responses_bytes:
        std::cell::RefCell<std::collections::HashMap<(String, String), (&'static str, Vec<u8>)>>,
    /// An ordered queue per `(method, path)` — a route a test needs to
    /// answer *differently* call by call (the cable drag's two
    /// `GET /graph/hit` answers: press resolves the port the drag starts
    /// on, release the port it lands on). Popped one reply per call; once
    /// empty the sticky `responses` map answers again.
    responses_seq: std::cell::RefCell<
        std::collections::HashMap<(String, String), std::collections::VecDeque<serde_json::Value>>,
    >,
    /// A scripted *refusal* for one `(method, path)` — `(status, message)`,
    /// checked before every other map so a test can prove a panel actually
    /// surfaces a door's `Err` (`Call::json`'s own `(status, message)`
    /// shape) instead of swallowing it silently.
    responses_error: std::cell::RefCell<std::collections::HashMap<(String, String), (u16, String)>>,
}

#[cfg(test)]
impl FakeDoor {
    pub(crate) fn new() -> Self {
        FakeDoor {
            calls: std::cell::RefCell::new(Vec::new()),
            responses: std::cell::RefCell::new(std::collections::HashMap::new()),
            responses_bytes: std::cell::RefCell::new(std::collections::HashMap::new()),
            responses_seq: std::cell::RefCell::new(std::collections::HashMap::new()),
            responses_error: std::cell::RefCell::new(std::collections::HashMap::new()),
        }
    }

    /// Script a refusal for one `(method, path)` — the next (and every
    /// following) call answers `status` with `message` as the plain-text
    /// body, exactly the shape [`Call::json`]'s default method turns into
    /// `Err((status, message))`. Checked first, ahead of every success
    /// map, so scripting a refusal for a route already scripted to succeed
    /// is unambiguous about which one wins.
    pub(crate) fn respond_error(&self, method: &str, path: &str, status: u16, message: &str) {
        self.responses_error
            .borrow_mut()
            .insert((method.to_string(), path.to_string()), (status, message.to_string()));
    }

    /// Script a canned `200` JSON reply for one `(method, path)` this door
    /// will be asked for — everything not scripted still answers `200 {}`,
    /// exactly as before `S.U9`.
    pub(crate) fn respond(&self, method: &str, path: &str, body: serde_json::Value) {
        self.responses
            .borrow_mut()
            .insert((method.to_string(), path.to_string()), body);
    }

    /// Queue one JSON reply for `(method, path)` — the next call to the
    /// route answers it, FIFO, before the sticky map's ever-constant
    /// answer is consulted. Writing the queue first (not instead of
    /// `respond`) keeps every scripted test's vocabulary the same shape.
    pub(crate) fn respond_seq(&self, method: &str, path: &str, body: serde_json::Value) {
        self.responses_seq
            .borrow_mut()
            .entry((method.to_string(), path.to_string()))
            .or_default()
            .push_back(body);
    }

    /// Script a canned `200` **bytes** reply (a JPEG for S.U9b's
    /// `Frame::tick`) — the same idea `respond` is, for a reply with
    /// nothing to parse. The recorded call carries the query, so a test
    /// can assert the grid is fetched every `min_gap` (not more).
    pub(crate) fn respond_bytes(
        &self,
        method: &str,
        path: &str,
        kind: &'static str,
        body: Vec<u8>,
    ) {
        self.responses_bytes
            .borrow_mut()
            .insert((method.to_string(), path.to_string()), (kind, body));
    }
}

#[cfg(test)]
impl Call for FakeDoor {
    fn call(
        &self,
        method: &str,
        path: &str,
        params: crate::call::Params,
    ) -> Option<crate::call::Reply> {
        self.calls
            .borrow_mut()
            .push((method.to_string(), path.to_string(), params));
        if let Some((status, message)) = self
            .responses_error
            .borrow()
            .get(&(method.to_string(), path.to_string()))
        {
            return Some(crate::call::Reply {
                status: *status,
                kind: "text/plain",
                body: message.clone().into_bytes(),
            });
        }
        // A scripted *bytes* reply (S.U9b) wins over a scripted object —
        // the reply has nothing to parse and its `kind` is the half-names
        // mime type the caller asked for.
        if let Some((kind, body)) = self
            .responses_bytes
            .borrow()
            .get(&(method.to_string(), path.to_string()))
        {
            return Some(crate::call::Reply {
                status: 200,
                kind,
                body: body.clone(),
            });
        }
        let body = self
            .responses_seq
            .borrow_mut()
            .get_mut(&(method.to_string(), path.to_string()))
            .and_then(|q| q.pop_front())
            .or_else(|| {
                self.responses
                    .borrow()
                    .get(&(method.to_string(), path.to_string()))
                    .cloned()
            })
            .unwrap_or_else(|| serde_json::json!({}));
        Some(crate::call::Reply {
            status: 200,
            kind: "application/json",
            body: body.to_string().into_bytes(),
        })
    }
}

/// So a test can hand [`Shell`] a *shared* [`FakeDoor`] — `Box<dyn Call>`
/// wants ownership, but `kittest.rs`'s toggle tests need to keep reading
/// `calls` after the harness owns the door. `Rc` rather than `Arc`: nothing
/// here crosses a thread (the kittest harness runs on the calling thread),
/// so the lighter refcount is the honest one.
#[cfg(test)]
impl Call for std::rc::Rc<FakeDoor> {
    fn call(
        &self,
        method: &str,
        path: &str,
        params: crate::call::Params,
    ) -> Option<crate::call::Reply> {
        FakeDoor::call(self, method, path, params)
    }
}
