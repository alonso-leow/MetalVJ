//! `S.U10`: the timeline and mask-grid panel, in the [`super::Region::Lanes`]
//! region (`plan-sonnet.md` § S.U10, `plan-opus.md` § O.12.e).
//!
//! **Scope, in the brief's own priority order.** (1) The timeline surface:
//! the author floats' lanes (movements) and active overlays (`POST
//! /overlay`). Not here since 2026-09-21: the transport clock, the drive
//! dial and the router's mode/picked/sensitivity — none of them is a lane.
//! Play/pause is the nodes canvas bar's (`POST /transport`, one route
//! behind the spacebar, `more ▾` and that button); the drive knobs are the
//! `audio.live` node's config (`the design notes` decision 12, `S.AI`); the router's
//! mode and picks are the `ai.router` node's (`S.AM.b`) — a node's
//! controls live on its properties, one surface per object. (2) The mask grid (`S.F`
//! item 2): `POST /mask/grid` lays a rectangular grid of empty masks over
//! one or more outputs; this panel reads the resulting list back off `GET
//! /state`'s `outputs[].masks`.
//!
//! **Two states, one panel** (`paneles-inferiores.md` § P2): the panel is
//! either authoring a *scratch* sequence — `lanes`: the tick canvas, `save as sequence`, masks, mix — or performing
//! saved ones — `overlays`: one button grid, the fixed row (`blackout` ·
//! `+` · `stop all`) followed by the movement library,
//! the saved sequences and the switches (`R22.3` decision 2). There is no
//! tab row: [`resolve_mode`] reads [`PanelState::mode_override`], which
//! defaults to `overlays`. Only context writes `lanes`: `+ create sequence`,
//! opening or dropping a saved sequence, or importing a song into the editor.
//! A selection never moves it (`R22.7` item 5).
//!
//! **Regla 5.** Every value painted here is exactly what a route already
//! resolved — `state.time()`'s semantics, the router's `regroup_in`/`plays`
//! clock math, a mask's `bounds_in` — nothing here recomputes a timeline
//! position or a mask geometry; a control writes an operator's *intent*
//! (toggle, slider, grid size) and reads the door's reply back.
//!
//! **Regla 6.** Every call goes through [`crate::call::Call`] —
//! `door.call`/`door.json` — never a handler body.
//!
//! **What is deliberately not here — priority 3, the shared lane painting.**
//! `plan-opus.md` § O.12.a's finding (repeated in `S.U9`'s own doc comment)
//! is that `rig.js`/`suite.js`/`router.js`/`curator.js` all paint onto one
//! physical tick-by-tick canvas, `SHELL.laneWindow`: a waveform-scale strip
//! with picks/segments drawn per tick and drag/click geometry (the old
//! dashboard's `modeHitTest`/`modeTickAt`/`modeClampRange`, deleted with
//! `page/dash.html` 2026-09-15) that is
//! a second, pixel-precise layout surface in its own right — not a form of
//! buttons and sliders like the transport/drive/mask controls above. Measured
//! while writing this file: reproducing it needs the tick-rate data
//! (`metal_router::features::RATE`), the take/pick geometry
//! (`call::author::router::{take_get, take_post}`, `/author/router/take`)
//! and the curator's per-folder cue geometry (`call::author::curator`) all
//! joined against one shared view transform, which is a lane widget of its
//! own comparable in size to `nodes.rs`'s canvas — landing it here would
//! blow this batch's priority order (the brief lands 1 fully before touching
//! 3). (The router's *coarse* controls — `mode`/`picked`/`sensitivity` —
//! sat here as a form until 2026-09-21; they are the `ai.router` node's
//! now, `S.AM.b`/`S.AP`.) Registered as the leftover row `S.U10b`, alongside the mask grid's cell-by-cell point editing (today's
//! `POST /mask/grid` creates rectangular cells — moving a placed mask's
//! corners is `call::outputs::mask` (op `"mask"`, `points`), not landed
//! here).
//!
//! **Two sections that closed the deferred halves of O.1 and O.3** (the
//! phases `plan-opus.md` closed on the data side on 2026-09-06, each with
//! a control "against the native shell when it exists"):
//!
//! - [`mask_editor`] (O.1): per mask on the selected output, the three
//!   fields `Mask` grew in O.1 — `parent`/`alpha`/`compose` — written
//!   through the same `POST /outputs { op: "mask" }` the page's paint tool
//!   already wrote `points` through, one key per control
//!   (`call::outputs::edit_mask`). H.10's Venn diagram is a `compose` combo
//!   here; H.5b's nested contour is the `inside of` combo.
//! - [`mix_row`] (O.3 / `S.D` item 1): per `model`/`media` segment of a
//!   timeline (`GET /author/models/timelines`), the segment's `mix` —
//!   mask, blend, opacity (`SegmentMix`) — written by resubmitting the
//!   *whole* segment through `POST /author/models/timeline/segment { op:
//!   "move" }`, byte for byte the body the `set_layer_mix` tool builds
//!   (`author::models::agent_tools`), which is `S.D` item 3: *"the two
//!   things together or neither"*. There is no timeline lane here to hang it
//!   on yet (that is `S.U10b`), so the section is a list, not a strip.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use egui::{Align, Align2, Frame, Id, Layout, Popup, Pos2, Rect, Response, RichText, ScrollArea, Sense, Ui, UiBuilder, Vec2};
use serde_json::{json, Value};

use crate::call::{Call, Params};

use crate::shell::classes::Theme;
use crate::shell::components::{
    dropdown, label_row, lucide, num_track, panel_frame, panel_head, section_heading, slider_row,
    toggle_row, Icon,
};
use crate::shell::i18n::{tr, tr_dyn, tr_key, trf};
use crate::shell::panels::builtins;
use crate::shell::panels::Selection;

/// Everything the lanes panel remembers between frames — all of it is either
/// a form field the operator is mid-typing (never itself sent until a
/// button is pressed) or a door reply cached from the last time it was
/// fetched. `/state` itself is *not* cached here: unlike `nodes.rs`'s graph
/// (edited in place by this panel's own writes), the clock keeps moving
/// whether or not this panel touches it, so every frame re-reads it fresh —
/// the same "read it fresh while shown" idiom `header.rs`'s collections/
/// `more ▾` already use for the same reason.
pub struct PanelState {
    /// Which of the panel's two states the operator forced by hand — the
    /// tab row and the library's own gestures write it. `None` means
    /// `lanes`; `R22.7` item 5 retired the selection-follows rule, so a
    /// selection change no longer clears it.
    mode_override: Option<crate::shell::lanes::Mode>,
    /// The grid button whose properties a later phase shows — set by a click
    /// on a sequence button (`Tab` walks it once that lands).
    pub(crate) focused_overlay: Option<String>,
    /// The movement scratch the two-mode section edits — `S.U10b`'s
    /// timeline/overlays decision (plan-opus.md § Closed and in force, item
    /// 7), with the whole of its memory in [`crate::shell::lanes::MovementLane`]
    /// so the canvas module can hold its own pure ops to tests.
    pub(crate) mv: crate::shell::lanes::MovementLane,
    /// The full movement shown on the strip, fetched off `GET
    /// /author/models/movement?name=` when a sequence is loaded from the
    /// library — a cache, invalidated by loading another. The grid no longer
    /// writes it (a loaded sequence's strip is `lanes`' business).
    mv_shown: Option<Value>,
    /// `hold`-kind switch ids the pointer is down on, as of last frame — the
    /// edge (not the level) is what should fire `POST /switch`: a `hold`
    /// button re-posting `on` every frame it stays pressed would be
    /// harmless but wasteful, and never firing `off` would leave a strobo
    /// stuck lit the instant a frame is dropped between "down" and "up".
    held_switches: std::collections::HashSet<String>,
    /// The song whose saved project (`GET /author/project`) was last
    /// asked for — a new song asks once.
    project_loaded_for: Option<String>,
    /// The library sequence last adopted as a track, so it is adopted once.
    adopted_shown: Option<String>,
    /// `create sequence` (overlays) asks the editor for a fresh track.
    pub(crate) new_track: bool,
    /// `+ › add loop` asks the editor for an empty loop of this many ticks
    /// (see [`crate::shell::lanes::IngestLane::start_loop`]).
    pub(crate) new_loop: Option<usize>,
    /// `import` was pressed: the shell opens the "new" gallery for the
    /// sequence.
    import_requested: bool,
    /// The gallery's pick (`gen.<model>` or `fx`), waiting for the editor —
    /// which owns the model defaults — to turn it into a clip.
    pending_import: Option<String>,
    /// A `+ layer` click asks the shell to focus the lanes region — drained by
    /// [`paint`] into the click response, since a button click is not the
    /// click-to-focus catcher's own.
    pub(crate) focus_lanes: bool,
    /// A click landed on empty space in this panel (the frame's own
    /// catcher, or a blank row of the timeline). The shell reads it once a
    /// frame and clears the layer focus the lanes region had, so a click on
    /// nothing puts the properties panel back on the selection.
    blank_click: bool,
    /// 2026-10-01: what the song's audio player was last told —
    /// `Some(tick)` playing from that tick, `None` stopped.
    audio_from: Option<usize>,
    /// Whether the audio clock has answered `native` since the song started:
    /// an undigested song stops when its native audio ends (`ingest_play`).
    audio_native: bool,
    /// P-D: playback was started by an incoming timecode (`clock.timecode`),
    /// so it stops when the timecode does.
    tc_driving: bool,
    /// The video frame the preview last asked for — `(path, ticks into the
    /// clip)`, posted only on change.
    video_posted: Option<(String, usize)>,
    /// The worker that runs the preview's POSTs and the song fetch, when the
    /// shell has one. Without it (the kittest harness) both run inline.
    pub(crate) worker: Option<crate::shell::lane_worker::LaneWorker>,
    /// The generation of the last floats POST the preview asked for, and of
    /// the last seek POST: the picture waits for these to land.
    preview_gen: u64,
    seek_gen: u64,
    /// The picture last shown, so a frame whose write has not landed yet
    /// keeps drawing it.
    last_picture: Option<(String, Vec<(String, String)>)>,
    /// The song fetch on the worker: what it is for, and its answer.
    song_flight: Option<(SongJob, std::sync::mpsc::Receiver<SongLoad>)>,
    /// The controller scene whose editor popup is open: the graph id it is
    /// keyed by. The popup is painted at the grid's end, anchored near
    /// [`Self::controller_edit_pos`]; a `⚙` in the scene cell or a right-click
    /// on it sets it.
    pub(crate) editing_controller: Option<String>,
    /// Where the open controller editor popup sits — the scene cell's own
    /// top-left when it was opened.
    pub(crate) controller_edit_pos: Option<Pos2>,
    /// The controller editor's name field, and the graph it was seeded for, so
    /// an untouched buffer reseeds when the editor switches controller and an
    /// edited one is left alone.
    controller_name_buffer: String,
    controller_name_for: Option<String>,
    /// 2026-09-22 (afternoon): a `files` audio card dropped on the overlays
    /// grid, held until `shell/mod.rs` drains it into the ingest lane — the
    /// lanes panel cannot reach `Shell::ingest`, so it raises the intent the
    /// same way the library raises `play`.
    pub(crate) pending_song: Option<String>,
    /// A library sequence dropped on the timeline, for `shell/mod.rs` to
    /// open the way the library's `ver` does.
    pub(crate) sequence_dropped: Option<String>,
    /// 2026-09-22 (afternoon): the `video`-drop popover's draft — the output
    /// (or mask) a dropped video should be triggered on. `None` when the
    /// popover is closed.
    pub(crate) new_video: Option<NewVideo>,
    /// Each button's size, `(w, h)` in cells (one of `surface::SPANS`),
    /// keyed as [`Cell::key`]. Read once from `GET /settings` and replaced by
    /// each `POST`'s reply; a key with no entry is 1×1.
    pub(crate) grid_spans: BTreeMap<String, (f32, f32)>,
    /// The control surface's stored half (`grid_surface`): fixed cols × rows,
    /// order, pages, control cells, MIDI bindings, recorded loops.
    pub(crate) layout: crate::surface::Layout,
    /// The grid's saved layouts (`grid_layouts`), the one up being `layout`.
    pub(crate) layouts: crate::surface::Layouts,
    /// The name popup open over the grid: on a new layout, or `rename`.
    layout_name: Option<LayoutName>,
    /// The layout the delete popup asks about.
    layout_delete: Option<String>,
    /// Its live half: held, frozen, recording, playing.
    pub(crate) surface: crate::surface::Surface,
    /// What this frame's gestures asked the grid to do, run once the cells
    /// are painted.
    grid_outs: Vec<crate::surface::Out>,
    /// Piano keys pressed with the pointer, fed through the same path a
    /// hardware key takes.
    pad_inputs: Vec<(String, metal_midi::Edge)>,
    /// The grid's edit mode: a click selects a cell instead of firing it.
    pub(crate) grid_edit: bool,
    /// The cell the edit mode selected.
    pub(crate) grid_selected: Option<String>,
    /// A cell waiting for the next MIDI control (or piano key) to bind.
    pub(crate) grid_learn: Option<String>,
    /// A box's slot waiting for the next MIDI control: what it sends.
    slot_learn: Option<usize>,
    /// Learning the whole face: the slot waiting for its control (the
    /// slots' count is the piano's lowest key), and the control just
    /// learned, which a turn or release keeps sending.
    face_learn: Option<usize>,
    face_learn_last: Option<String>,
    /// The last MIDI message heard, `input · port`, for the status row.
    last_heard: Option<String>,
    /// The grid's own buttons, `(key, label)`, a face's control can hold.
    face_pool: Vec<(String, String)>,
    /// The free grid's controller buttons lit now (a switch's toggle, a hold's press).
    lit_controllers: BTreeSet<String>,
    /// The last thing the surface wants the operator to read.
    grid_note: Option<String>,
    /// A dial's part-step, by cell key, between frames.
    dial_acc: HashMap<String, f32>,
    /// The note each piano cell's pointer is down on.
    piano_held: HashMap<String, Option<u8>>,
    /// CC-mode pads above half, by input id: a CC pad's level becomes edges.
    cc_down: BTreeSet<String>,
    /// The box's controls as last heard, by input id: what is pressed, and
    /// where each knob, fader and strip sits (0..=1) — a box face lights and
    /// turns with the hardware even where no cell is.
    hw_down: BTreeSet<String>,
    hw_levels: HashMap<String, f32>,
    /// A fader moved by MIDI waits this long without moving before its
    /// value is saved.
    controllers_dirty: Option<f64>,
    /// The egui pass the grid last drove the surface in, so the off-screen
    /// drive does not run a second time that frame.
    driven_pass: Option<u64>,
    /// The saved node controllers, read once from `GET /settings`'s
    /// `grid_controllers` (rule 5, the same trade `grid_spans` makes) and
    /// replaced by each `POST`'s reply. Keyed by an arbitrary string; each
    /// value is `{ graph, name, mode, targets: [{ node, on? } | { node, param,
    /// value } | { node, fixture|group, on }] }`. Absent or `null` settings
    /// mean none.
    pub(crate) controllers: BTreeMap<String, Value>,
    /// Point 7: the node ids whose folder row is open in the IDE-style node
    /// tree, and the venue group rows' collapse keys (`group:<name>`) in the
    /// `controller button` menu. A node not in the set is
    /// collapsed, which is where every node starts; a group not in the set is
    /// open. The set persists across frames while a menu is open. Keyed by node
    /// id, or `group:<name>`.
    pub(crate) expanded_nodes: BTreeSet<String>,
    /// The overlays grid's HSB faders, `[h, s, b]` in 0..1 — the value a
    /// drag leaves on the track, posted on release (`builtins.rs`).
    pub(crate) builtin_hsb: [f32; 3],
    /// One `GET /settings` per run, the same trade `settings::PanelState`
    /// makes: the cache is the disk's truth from then on.
    spans_fetched: bool,
    /// `4b`/`4c`: which mode the header's name field is seeded for, so the
    /// buffer reseeds when the panel switches state.
    name_mode: Option<crate::shell::lanes::Mode>,
    /// The header name field's text: the overlays collection's name, or the
    /// sequence's (a shown movement's, else the song's).
    name_buffer: String,
    /// The name the field was seeded from or last saved as — a save fires
    /// only when the typed text differs from it, never every frame.
    name_saved: String,
    /// What the buffer was seeded from, so a still-untouched field reseeds
    /// when the natural default changes (a movement's name arriving a frame
    /// late), while an edited one is left alone.
    name_seed: String,
}

/// The mode switch: a track and knob, lit when `on` (toggle) and dark when off
/// (hold). Only the hold state carries text beside it (`Press to activate`);
/// the toggle is the bare switch. The row's accessible label is that same text
/// (`switch` when there is none), so it stays the click target.
fn switch_row(ui: &mut Ui, theme: &Theme, on: &mut bool) -> Response {
    let text = if *on { tr("switch") } else { tr("Press to activate") };
    let height = ui.spacing().interact_size.y;
    let track = Vec2::new(height * 1.8, height * 0.9);
    let gap = ui.spacing().item_spacing.x;
    let galley = (!*on).then(|| ui.painter().layout_no_wrap(text.to_string(), theme.font_small.clone(), theme.ink));
    let text_w = galley.as_ref().map_or(0.0, |g| gap + g.size().x);
    let text_h = galley.as_ref().map_or(0.0, |g| g.size().y);
    let size = Vec2::new(track.x + text_w, height.max(text_h));
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let track_rect = Rect::from_center_size(Pos2::new(rect.left() + track.x / 2.0, rect.center().y), track);
    let painter = ui.painter();
    painter.rect_filled(track_rect, track.y / 2.0, if *on { theme.phosphor } else { theme.inset });
    let knob_x = if *on { track_rect.right() - track.y / 2.0 } else { track_rect.left() + track.y / 2.0 };
    painter.circle_filled(Pos2::new(knob_x, track_rect.center().y), track.y * 0.4, theme.ink);
    if let Some(galley) = galley {
        painter.galley(
            Pos2::new(track_rect.right() + gap, rect.center().y - galley.size().y / 2.0),
            galley,
            theme.ink,
        );
    }
    if response.clicked() {
        *on = !*on;
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Checkbox, true, text));
    response
}

/// The name popup's draft: the layout it names and the text typed.
#[derive(Clone, Debug)]
struct LayoutName {
    id: String,
    text: String,
    /// Opened on a layout just made (`name the layout`), not by `rename`.
    fresh: bool,
    /// The field takes the keyboard on the popup's first frame.
    focus: bool,
}

/// 2026-09-22 (afternoon): the `video`-drop popover's draft. `path`/`name` are
/// the dropped card's; `target` is the `output.device` (or `mask`) id the
/// operator picked as the destination, and `mask: true` marks that the id
/// names a mask rather than an output device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewVideo {
    pub name: String,
    pub path: String,
    pub target: String,
    pub mask: bool,
}

impl NewVideo {
    fn new(drag: &crate::shell::panels::FileDrag) -> Self {
        NewVideo {
            name: drag.name.clone(),
            path: drag.path.clone(),
            target: String::new(),
            mask: false,
        }
    }
}

impl Default for PanelState {
    fn default() -> Self {
        PanelState {
            mode_override: None,
            focused_overlay: None,
            mv: crate::shell::lanes::MovementLane::default(),
            mv_shown: None,
            held_switches: std::collections::HashSet::new(),
            project_loaded_for: None,
            adopted_shown: None,
            new_track: false,
            new_loop: None,
            import_requested: false,
            pending_import: None,
            focus_lanes: false,
            blank_click: false,
            audio_from: None,
            audio_native: false,
            tc_driving: false,
            video_posted: None,
            worker: None,
            preview_gen: 0,
            seek_gen: 0,
            last_picture: None,
            song_flight: None,
            editing_controller: None,
            controller_edit_pos: None,
            controller_name_buffer: String::new(),
            controller_name_for: None,
            pending_song: None,
            sequence_dropped: None,
            new_video: None,
            grid_spans: BTreeMap::new(),
            layout: crate::surface::Layout::default(),
            layouts: crate::surface::Layouts::default(),
            layout_name: None,
            layout_delete: None,
            surface: crate::surface::Surface::default(),
            grid_outs: Vec::new(),
            pad_inputs: Vec::new(),
            grid_edit: false,
            grid_selected: None,
            grid_learn: None,
            slot_learn: None,
            face_learn: None,
            face_learn_last: None,
            last_heard: None,
            face_pool: Vec::new(),
            lit_controllers: BTreeSet::new(),
            grid_note: None,
            dial_acc: HashMap::new(),
            piano_held: HashMap::new(),
            cc_down: BTreeSet::new(),
            hw_down: BTreeSet::new(),
            hw_levels: HashMap::new(),
            controllers_dirty: None,
            driven_pass: None,
            controllers: BTreeMap::new(),
            expanded_nodes: BTreeSet::new(),
            builtin_hsb: [0.0, 1.0, 1.0],
            spans_fetched: false,
            name_mode: None,
            name_buffer: String::new(),
            name_saved: String::new(),
            name_seed: String::new(),
        }
    }
}

impl PanelState {
    /// Hands the panel its worker; the shell calls this once at startup.
    pub(crate) fn set_worker(&mut self, worker: crate::shell::lane_worker::LaneWorker) {
        self.worker = Some(worker);
    }

    /// One `GET /settings`, on the first frame the panel paints: the cell
    /// sizes the grid lays out with. A failed read leaves every cell 1×1.
    pub(crate) fn refresh(&mut self, door: &dyn Call) {
        if self.spans_fetched {
            return;
        }
        self.spans_fetched = true;
        if let Ok(reply) = door.json("GET", "/settings", None) {
            self.grid_spans = parse_spans(&reply["grid_spans"]);
            self.controllers = parse_controllers(&reply["grid_controllers"]);
            self.layout = crate::surface::Layout::from_value(&reply["grid_surface"]);
            self.layouts = crate::surface::Layouts::from_value(&reply["grid_layouts"]);
        }
        // An install with none stored opens on the grid it had, named.
        if self.layouts.list.is_empty() {
            let id = self.layouts.create("", self.layout.clone(), layout_default_name);
            self.layouts.active = id;
        }
    }

    /// The grid button a click focused — the properties panel reads it
    /// (`P5`), and the kittest suite pins that a click focuses the button it
    /// hit.
    pub(crate) fn focused_overlay_name(&self) -> Option<&str> {
        self.focused_overlay.as_deref()
    }

    /// Test-only alias of [`Self::focused_overlay_name`] — the name the
    /// existing grid tests already use.
    #[cfg(test)]
    pub(crate) fn focused_overlay(&self) -> Option<&str> {
        self.focused_overlay_name()
    }

    /// Test-only: focus a grid button without a click, so the properties
    /// tests can show its section (`P5`).
    #[cfg(test)]
    pub(crate) fn set_focused_overlay_for_test(&mut self, name: &str) {
        self.focused_overlay = Some(name.to_string());
    }

    /// Forces the panel's state by hand — the wiring (`P3`) uses it when the
    /// library panel opens an audio or loads a sequence.
    pub(crate) fn force_mode(&mut self, mode: crate::shell::lanes::Mode) {
        self.mode_override = Some(mode);
    }

    /// Reads and clears the `import` press — the shell's one call per
    /// frame, which opens the gallery.
    pub(crate) fn take_import_request(&mut self) -> bool {
        std::mem::take(&mut self.import_requested)
    }

    /// The gallery picked `kind` (`gen.<model>` or `fx`) for the sequence.
    pub(crate) fn import_pick(&mut self, kind: String) {
        self.pending_import = Some(kind);
    }

    /// Reads and clears [`Self::blank_click`] — the shell's one call per frame.
    pub(crate) fn take_blank_click(&mut self) -> bool {
        std::mem::take(&mut self.blank_click)
    }

    /// Caches a saved sequence's full movement so the strip paints it next
    /// frame (`P3`'s "view" from the library).
    pub(crate) fn show_sequence(&mut self, movement: Value) {
        self.mv_shown = Some(movement);
    }
}

/// Which of the two states the panel paints: an explicit override (the tab
/// row, the `play`/`load`/`view`/`+ create sequence` gestures) is the only
/// thing that picks — `R22.7` item 5 retired the "a selected node means
/// overlays" rule, so a selection no longer moves the tab under the
/// operator. Pure, so the rule is testable without a door or a frame.
fn resolve_mode(forced: Option<crate::shell::lanes::Mode>) -> crate::shell::lanes::Mode {
    forced.unwrap_or(crate::shell::lanes::Mode::Overlays)
}

/// The schemas this build has, once per frame: what "save as sequence"
/// is checked against (`library::check` needs the width) and what the model
/// combo offers. A missing reply leaves the combo empty rather than guessing
/// a width — rule 5: a width this panel did not get is not a width it
/// invents.
fn model_choices(door: &dyn Call) -> Vec<(String, String, usize)> {
    let manifest = crate::shell::reads::get_shared(door, "/author/models/state", &[]);
    let empty = Vec::new();
    let models = manifest.as_ref().and_then(|v| v["models"].as_array()).unwrap_or(&empty);
    models
        .iter()
        .filter_map(|m| {
            Some((
                m["id"].as_str()?.to_string(),
                m["label"].as_str().unwrap_or("?").to_string(),
                m["width"].as_u64()? as usize,
            ))
        })
        .collect()
}

/// A library file or folder picked for the sequence (`importar`). An image is
/// a still clip at the playhead (a clip over the picture file, held for its
/// length); a video is a clip; an audio file makes its song the sequence's
/// (picked by name, as the song picker does); a folder adds each importable
/// file in it, one after another from the playhead.
fn import_file(door: &dyn Call, ingest: &mut crate::shell::lanes::IngestLane, file_kind: &str, path: &str) {
    match file_kind {
        "image" | "video" => ingest.add_video_at_playhead(path),
        "audio" => ingest.set_song_at_playhead(&song_name_of(path)),
        "folder" => {
            for (kind, file) in importable_in(door, path) {
                match kind.as_str() {
                    "audio" => ingest.set_song_at_playhead(&song_name_of(&file)),
                    _ => ingest.add_video_at_playhead(&file),
                }
            }
        }
        _ => {}
    }
}

/// A song's name in the library: the file's stem (`mix.wav` is `mix`).
fn song_name_of(path: &str) -> String {
    std::path::Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or(path).to_string()
}

/// The importable files of a folder, in the listing's order: `(kind, path)`
/// for every image, video or audio file the `/fs` route lists (a folder
/// inside is not expanded again).
fn importable_in(door: &dyn Call, dir: &str) -> Vec<(String, String)> {
    let mut query = std::collections::HashMap::new();
    query.insert("path".to_string(), dir.to_string());
    let Some(reply) = door.call("GET", "/fs", crate::call::Params { query, body: Vec::new(), tail: None }) else {
        return Vec::new();
    };
    if reply.status != 200 {
        return Vec::new();
    }
    let Ok(listing) = crate::json_memo::parse(&reply.body) else {
        return Vec::new();
    };
    listing["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let kind = f["kind"].as_str()?;
            let file = f["file"].as_str()?;
            matches!(kind, "image" | "video" | "audio")
                .then(|| (kind.to_string(), std::path::Path::new(dir).join(file).to_string_lossy().into_owned()))
        })
        .collect()
}

/// The sequence's `import` gallery: every animation (model) as a card, the
/// fx catalogue (`fx.*`, one card per effect) and `fx` — a new layer over the
/// others — beside them. The model pictures come from the nodes canvas's own
/// tile grid, the same ones its gallery shows.
pub(crate) fn import_sections(door: &dyn Call, nodes: &super::nodes::PanelState) -> Vec<crate::shell::gallery::Section> {
    use crate::shell::gallery::{Item, Section};
    let models = model_choices(door)
        .into_iter()
        .map(|(id, label, _)| {
            let kind = format!("gen.{id}");
            Item {
                label,
                family: tr("animation").to_string(),
                inputs: vec!["control".to_string()],
                outputs: vec!["image".to_string()],
                tiles: nodes.tiles_for(&kind),
                starred: false,
                favourite: false,
                kind,
            }
        })
        .collect();
    let fx = Item {
        kind: "fx".to_string(),
        label: "fx".to_string(),
        family: tr("layer").to_string(),
        inputs: vec!["image".to_string()],
        outputs: vec!["image".to_string()],
        tiles: Default::default(),
        starred: false,
        favourite: false,
    };
    let effects: Vec<Item> = fx_catalog(door)
        .into_iter()
        .map(|(kind, label, family)| Item {
            tiles: nodes.tiles_for(&kind),
            kind,
            label,
            family,
            inputs: vec!["image".to_string()],
            outputs: vec!["image".to_string()],
            starred: false,
            favourite: false,
        })
        .collect();
    let mut fx_items = vec![fx];
    fx_items.extend(effects);
    vec![
        Section { icon: crate::shell::components::Icon::Video, label: tr("animations").to_string(), items: models },
        Section { icon: crate::shell::components::Icon::Effect, label: tr("fx layers").to_string(), items: fx_items },
    ]
}

/// The fx catalogue: `(kind, label, family)` for every `fx.*` kind the door
/// lists, in the door's order. Empty when the catalogue does not answer.
fn fx_catalog(door: &dyn Call) -> Vec<(String, String, String)> {
    get(door, "GET", "/catalog", &[])
        .and_then(|v| v["catalog"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|k| {
            let kind = k["kind"].as_str()?;
            kind.starts_with("fx.").then(|| {
                let label = k["label"].as_str().filter(|l| !l.is_empty()).unwrap_or(kind).to_string();
                (kind.to_string(), tr_dyn(&label), tr("effect").to_string())
            })
        })
        .collect()
}

/// Paint the whole `lanes` region and report the outer frame as a clickable
/// [`Response`] — the same contract [`super::nodes::paint`] and
/// [`crate::shell::components::region_placeholder`] both keep, so
/// [`super::super::regions`] learns a click here exactly the way it learns
/// one anywhere else.
#[allow(clippy::too_many_arguments)]
pub fn paint(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    ingest: &mut crate::shell::lanes::IngestLane,
    selection: &Selection,
    open_graph: Option<&str>,
    focused: bool,
) -> Response {
    let _span = crate::perf::Span::new("lanes");
    ui.push_id(Id::new(super::Region::Lanes.id()), |ui| {
        let mut frame = panel_frame(theme);
        if focused {
            frame = frame.stroke(theme.accent());
        }
        let inner = frame.show(ui, |ui| {
            ui.expand_to_include_rect(ui.max_rect());
            let full_rect = ui.max_rect();
            let _s_state = crate::perf::Span::new("lanes.state");
            state.refresh(door);
            let state_val = get(door, "GET", "/state", &[]).unwrap_or_else(|| json!({}));
            drop(_s_state);
            ui.vertical(|ui| {
                // The click-to-focus catcher goes right after the passive
                // heading and before every interactive row — `nodes.rs`'s
                // `toolbar` doc comment has the full finding: egui gives an
                // overlapping click to whichever sensing widget was added
                // most recently, so a catcher added last would win every
                // click in this panel, `create grid`/the grid's buttons
                // included.
                let mut bg = ui.interact(full_rect, ui.id().with("click-bg"), Sense::click());
                // `4b`/`4c`: the header's one compact line — an editable name
                // field for the current mode's root object (the overlays
                // collection, or the sequence), in the biblioteca search
                // fields' own style. The bar keeps the region's AccessKit
                // label (a test's `get_by_label("lanes / overlays")`), so the
                // field sits inside its closure.
                let mode = resolve_mode(state.mode_override);
                panel_head(ui, theme, super::Region::Lanes.title(), |ui| {
                    ui.horizontal(|ui| {
                        header_name_field(ui, theme, door, state, ingest, mode);
                        // While the sequence is open, the way back to the
                        // buttons grid sits flush right in the head.
                        if mode == crate::shell::lanes::Mode::Lanes {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .button(RichText::new(tr("back to overlays")).font(theme.font_small.clone()))
                                    .on_hover_text(tr("close the sequence and show the buttons grid"))
                                    .clicked()
                                {
                                    state.force_mode(crate::shell::lanes::Mode::Overlays);
                                }
                            });
                        }
                    });
                });
                // The head's shadow falls on the first row of the component
                // below: a gap keeps it off the sequence's top edge.
                ui.add_space(theme.pad as f32);
                // `max_height` pinned to what is actually left in this
                // frame: without it a `ScrollArea` defaults to filling
                // whatever height its content asks for, and this panel's
                // content (unlike `region_placeholder`'s one line) can ask
                // for *more* than the row gave it once a list rewraps
                // under a narrower cede width — which
                // would grow this region's own outer rect exactly when it
                // should be ceding, the failure `focus_expands_and_others_cede`
                // (`kittest.rs`) is built to catch.
                let remaining = ui.available_height();
                // The scroll area is never wider than the column: its scrollbar
                // stays on the column's right edge, and a row wider than the
                // column can't widen the area under it.
                let width = ui.available_width();
                // The sequence has its own scrollbar (the timeline's); the
                // parent's bar stays hidden there and only takes the wheel.
                let bars = if mode == crate::shell::lanes::Mode::Lanes {
                    egui::scroll_area::ScrollBarVisibility::AlwaysHidden
                } else {
                    egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded
                };
                ScrollArea::vertical()
                    .max_width(width)
                    .max_height(remaining)
                    .scroll_bar_visibility(bars)
                    .id_salt("shell/region/lanes/scroll")
                    .show(ui, |ui| {
                        // No tab row: the state comes from context (`resolve_mode`).
                        match mode {
                            crate::shell::lanes::Mode::Lanes => {
                                // No transport, drive or router here
                                // (removed 2026-09-21): the lanes are the
                                // author floats over ticks — see the module
                                // doc for where each of those three went.
                                sequence_editor(ui, theme, door, state, ingest, selection, &state_val, remaining, open_graph, focused);
                            }
                            crate::shell::lanes::Mode::Overlays => {
                                overlays_grid(ui, theme, door, state, &state_val, open_graph);
                            }
                        }
                    });
                // `+ layer` asks for the focus; any other click that reached
                // the frame's catcher landed on empty space.
                if std::mem::take(&mut state.focus_lanes) {
                    bg.flags.insert(egui::response::Flags::FAKE_PRIMARY_CLICKED);
                } else if bg.clicked() {
                    state.blank_click = true;
                }
                bg
            })
            .inner
        });
        inner.inner
    })
    .inner
}

/// `4b`/`4c`: the header's name field — one compact singleline `TextEdit` in
/// the biblioteca search fields' style, seeded per mode ([`resolve_mode`]):
/// **Overlays** holds the current collection's name, **Lanes** the
/// sequence's (a shown movement's name, else the song). It is that header's
/// "root form".
///
/// Saving is Enter or blur: egui's singleline surrenders focus on Enter, so
/// `lost_focus()` already covers both; the explicit `Enter` check is the belt
/// to that brace. A save fires only when the text differs from
/// [`PanelState::name_saved`], never every frame.
fn header_name_field(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    ingest: &mut crate::shell::lanes::IngestLane,
    mode: crate::shell::lanes::Mode,
) {
    let (hint, default_name) = match mode {
        crate::shell::lanes::Mode::Lanes => (
            tr("sequence name"),
            state
                .mv_shown
                .as_ref()
                .and_then(|m| m["name"].as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| ingest.song.trim().to_string()),
        ),
        crate::shell::lanes::Mode::Overlays => (tr("collection name"), "collection".to_string()),
    };
    // Reseed on a mode switch; and while the field is still untouched, follow
    // the natural default (a shown movement's name can arrive a frame late).
    if state.name_mode != Some(mode) || (state.name_buffer == state.name_seed && default_name != state.name_seed) {
        state.name_buffer = default_name.clone();
        state.name_seed = default_name;
        state.name_saved = state.name_buffer.clone();
        state.name_mode = Some(mode);
    }
    ui.horizontal(|ui| {
        let resp = ui.add(
            crate::shell::components::header_edit(theme, &mut state.name_buffer, hint)
                .id(Id::new("shell/region/lanes/name")),
        );
        let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (resp.lost_focus() || enter) && state.name_buffer != state.name_saved {
            save_name(door, ingest, mode, &state.name_buffer);
            state.name_saved = state.name_buffer.clone();
        }
    });
}

/// `4b`/`4c`: persist one edited header name.
///
/// The **sequence** goes through the project save — `POST /author/project`,
/// whose `song` field is its name; re-posting carries every clip, so the
/// project file is rewritten under the typed name rather than clobbered
/// (a "save as", since the panel's own `ingest.song` key is left alone).
/// The **overlays collection** has no id open in this panel, so its name
/// stays in [`PanelState`] with an `ingest.note` saying there is no route.
fn save_name(
    door: &dyn Call,
    ingest: &mut crate::shell::lanes::IngestLane,
    mode: crate::shell::lanes::Mode,
    name: &str,
) {
    let name = name.trim().to_string();
    if name.is_empty() {
        ingest.note = Some("the name cannot be empty".into());
        return;
    }
    ingest.note = match mode {
        crate::shell::lanes::Mode::Lanes => match ingest.project_body() {
            None => Some("sequence without a song: the name stays in the panel, with no project to save it".into()),
            Some(mut body) => {
                body["song"] = json!(name.clone());
                match door.json("POST", "/author/project", Some(body)) {
                    Ok(_) => Some(format!("sequence saved as {name}")),
                    Err((_, msg)) => Some(msg),
                }
            }
        },
        crate::shell::lanes::Mode::Overlays => {
            Some("collection with no open id: the name stays in the panel, with no rename route here".into())
        }
    };
}

/// The overlays grid painted alone over the whole area: `Show` mode's body
/// (`shell/mod.rs`). Reads `GET /state` the way [`paint`] does and hands the
/// same grid over, so the grid's own pagination and spans carry over
/// unchanged; `ui`'s clip rect is what sizes it, and a `CentralPanel` gives
/// it the whole central area.
pub(crate) fn paint_show_grid(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    open_graph: Option<&str>,
) {
    let state_val = get(door, "GET", "/state", &[]).unwrap_or_else(|| json!({}));
    overlays_grid(ui, theme, door, state, &state_val, open_graph);
}

// ---------------------------------------------------------------------------
// the sequence editor (`Mode::Lanes`, 2026-09-30)
// ---------------------------------------------------------------------------

/// Height the compact transport bar and, when there is one, the note take
/// under the timeline. What sits above it (tabs, top bar, pages) is measured,
/// not guessed; so is this, from the theme's compact row.
fn editor_foot_h(ui: &Ui, theme: &Theme, has_note: bool) -> f32 {
    let gap = ui.spacing().item_spacing.y;
    let note = if has_note { ui.text_style_height(&egui::TextStyle::Small) + gap } else { 0.0 };
    theme.compact_row_height + gap + note
}

/// Shrinks the buttons of the row painted in `ui` to the theme's compact row.
fn compact_row(ui: &mut Ui, theme: &Theme) {
    let spacing = ui.spacing_mut();
    spacing.interact_size.y = theme.compact_row_height;
    spacing.button_padding = theme.compact_row_pad;
}
/// The timeline never shrinks below this, whatever the panel's height.
const TIMELINE_MIN_H: f32 = 120.0;

/// The ticks panel as one sequence editor: a narrow **left icon column** for
/// what acts on the playhead or the marked range (keypoint toggle, clipboard,
/// delete, undo, save), a **top toolbar** for context (the import menu with
/// its `animation` model submenu, zoom), the **timeline**
/// ([`crate::shell::timeline`]) as the whole body, and a **transport bar**
/// under it. The floats themselves are edited in `propiedades`, at the
/// playhead, on the active track — the row a keypoint files.
#[allow(clippy::too_many_arguments)]
fn sequence_editor(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    ingest: &mut crate::shell::lanes::IngestLane,
    selection: &Selection,
    state_val: &Value,
    panel_h: f32,
    open_graph: Option<&str>,
    focused: bool,
) {
    let _s_sequence = crate::perf::Span::new("lanes.sequence");
    let choices = model_choices(door);
    let manifest_reply = crate::shell::reads::get_shared(door, "/author/models/state", &[]);
    let no_models = Vec::new();
    let manifest = manifest_reply.as_ref().and_then(|v| v["models"].as_array()).unwrap_or(&no_models);
    let defaults_of = |id: &str| -> Vec<f32> {
        manifest
            .iter()
            .find(|m| m["id"].as_str() == Some(id))
            .and_then(|m| m["defaults"].as_array().cloned())
            .map(|a| a.iter().filter_map(|v| v.as_f64()).map(|v| v as f32).collect())
            .unwrap_or_default()
    };
    // The timeline asks for a model's fields once per float row; the manifest
    // and its blocks are parsed once per frame here, not once per row.
    let fields_memo = std::cell::RefCell::new(HashMap::<String, Vec<crate::shell::timeline::Field>>::new());
    let fields_uncached = |id: &str| -> Vec<crate::shell::timeline::Field> {
        let Some(m) = manifest.iter().find(|m| m["id"].as_str() == Some(id)) else {
            return Vec::new();
        };
        let blocks = m["blocks"].as_array().cloned().unwrap_or_default();
        let fields = crate::shell::timeline::fields_of(&blocks);
        if !fields.is_empty() {
            return fields;
        }
        // A manifest entry with a width but no blocks still gets its rows.
        (0..m["width"].as_u64().unwrap_or(0) as usize)
            .map(|i| crate::shell::timeline::Field {
                name: format!("float {i}"),
                offset: i,
                min: 0.0,
                max: 1.0,
                color: false,
            })
            .collect()
    };
    let fields_of = |id: &str| -> Vec<crate::shell::timeline::Field> {
        fields_memo
            .borrow_mut()
            .entry(id.to_string())
            .or_insert_with(|| fields_uncached(id))
            .clone()
    };
    let layer = match selection {
        Selection::Node(id) => Some(id.clone()),
        _ => None,
    };
    let outputs = state_val["outputs"].as_array().cloned().unwrap_or_default();

    // A fresh track asked for by `create sequence`, and a library sequence
    // (`view`/`load`/a drop) adopted as a track of its own.
    if let Some(ticks) = state.new_loop.take() {
        ingest.start_loop(ticks);
    }
    if std::mem::take(&mut state.new_track) && !ingest.is_blank() {
        ingest.add_track();
    }
    if let Some(shown) = state.mv_shown.clone() {
        let name = shown["name"].as_str().unwrap_or_default().to_string();
        if state.adopted_shown.as_deref() != Some(name.as_str()) {
            state.adopted_shown = Some(name.clone());
            if let (Some(model), Some(ticks)) = (shown["model"].as_str(), shown["ticks"].as_array()) {
                let keys = ticks
                    .iter()
                    .enumerate()
                    .filter_map(|(i, row)| {
                        Some((i, row.as_array()?.iter().filter_map(|v| v.as_f64()).map(|v| v as f32).collect()))
                    })
                    .collect();
                ingest.adopt_sequence(model, keys, shown["mask"].as_str().map(str::to_string));
                ingest.note = Some(format!("sequence {name} opened as a track"));
            }
        }
    }

    let _s_load = crate::perf::Span::new("lanes.load");
    load_song(ui, door, ingest, layer.as_deref(), state);
    drop(_s_load);
    // The song's saved project — every page's clips — once per song.
    if ingest.song.trim() != state.project_loaded_for.as_deref().unwrap_or_default() {
        let song = ingest.song.trim().to_string();
        state.project_loaded_for = Some(song.clone());
        if !song.is_empty() {
            if let Some(project) = get(door, "GET", "/author/project", &[("song", song.clone())]) {
                let n = ingest.adopt_project(&project);
                if n > 0 {
                    ingest.note = Some(format!("sequence of {song} opened — {n} clip(s), {} page(s)", ingest.pages().len()));
                }
            }
        }
    }

    // The whole sequence component, for the OS-file drop below.
    let body = ui.available_rect_before_wrap();
    // The left icon column reclaims the panel: the playhead/range verbs are
    // glyph buttons stacked narrow, the timeline and its rows take the rest.
    ui.horizontal(|ui| {
        icon_column(ui, theme, door, ingest, layer.as_deref());
        ui.vertical(|ui| {
            // Bar 1 — the page strip on the left and the edit controls
            // (`import ▾` and the zoom) flush right. Only two bars sit over
            // the timeline; everything that can share an edge shares one, so
            // the timeline keeps the panel.
            let mut import_clicked = false;
            let _s_bars = crate::perf::Span::new("lanes.bars");
            top_bar(ui, theme, door, ingest, &mut import_clicked, &outputs);
            state.import_requested |= import_clicked;
            // The gallery's pick: a model becomes a clip at the playhead, `fx`
            // a new layer over the others.
            if let Some(kind) = state.pending_import.take() {
                if kind == "fx" {
                    ingest.add_track();
                    state.focus_lanes = true;
                } else if kind.starts_with("fx.") {
                    // The fx catalogue: the same layer, carrying that effect.
                    ingest.add_effect_at_playhead(&kind);
                    state.focus_lanes = true;
                } else if let Some((file_kind, path)) = crate::shell::favorites::file_of_key(&kind) {
                    import_file(door, ingest, file_kind, path);
                } else if let Some(id) = kind.strip_prefix("gen.") {
                    // The clip first — it starts at the playhead — then the
                    // model lands on it (`add_clip` adopts the new, empty
                    // clip, so `switch_model` aims that one).
                    ingest.add_clip();
                    ingest.switch_model(id, defaults_of(id));
                }
            }
            // V1: the layer editor — the selected page's device stack, right
            // under the pages it belongs to.
            layer_editor(ui, theme, door, ingest, &outputs);
            // G2/G3: a fixture or rig page's own row — the `+ lane ` menu
            // and the `value` toggle (keyframes or a held constant).
            fixture_row(ui, theme, door, ingest, open_graph);

            // 2026-10-01: playback — the preview's play control (or the
            // toolbar's) walks the playhead at the digest's own rate, so the
            // picture plays the authored row tick by tick.
            let now = ui.input(|i| i.time);
            if crate::shell::preview::take_play_toggle(ui.ctx()) {
                ingest.toggle_play(now);
            }
            // Space plays and pauses the sequence, unless a text field is
            // typing (a focused button doesn't count: ⏵ keeps focus after a
            // click).
            let typing = ui
                .ctx()
                .memory(|m| m.focused())
                .is_some_and(|id| egui::TextEdit::load_state(ui.ctx(), id).is_some());
            if !typing && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space))
            {
                ingest.toggle_play(now);
            }
            // Delete / Backspace run the 🗑 delete — the marked range
            // cleared and the active track removed, as one undoable op —
            // but only while this region holds the focus and no text field
            // is typing, so a mask editor's own Delete (or a `TextEdit`'s)
            // is never swallowed (`consume_key` is what keeps one handler
            // from firing twice in the same frame).
            if focused
                && !typing
                && ui.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                        || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
                })
            {
                let n = ingest.delete_range_and_track();
                ingest.note = Some(format!("{n} keypoint(s) deleted · active clip removed"));
            }
            let mut audible = audible_clock(door, ingest);
            // P-D: a locked `clock.timecode` drives the playhead (chase) —
            // over the audio clock — and starts/stops playback with the
            // timecode.
            match timecode_clock(door) {
                Some(at) => {
                    if ingest.playing.is_none() {
                        let tick = (at.max(0.0) * f64::from(crate::shell::lanes::EDITOR_RATE)) as usize;
                        ingest.playing = Some((now, tick));
                        state.tc_driving = true;
                    }
                    audible = Some((at, true));
                }
                None if state.tc_driving => {
                    ingest.playing = None;
                    state.tc_driving = false;
                }
                None => {}
            }
            // An undigested song has no length but its audio: when the native
            // audio that was started for it has ended, the playback ends too.
            if let Some((_, sounding)) = audible {
                state.audio_native |= sounding;
            }
            if ingest.end_tick() == 0 && state.audio_native && audible.is_none_or(|(_, s)| !s) {
                ingest.playing = None;
                state.audio_native = false;
            }
            if ingest.advance_to(now, audible.map(|(at, _)| at)) {
                crate::shell::frames::animate(ui.ctx());
            }
            crate::playhead::publish(ingest.playhead, ingest.playing.is_some());

            // The timeline takes what is left once the rows above it and the
            // bar under it are counted, so the panel never outgrows its
            // region and grows a second vertical scroll bar beside the
            // timeline's own.
            let used = ui.cursor().top() - ui.min_rect().top();
            let timeline_h = (panel_h - used - editor_foot_h(ui, theme, ingest.note.is_some())).max(TIMELINE_MIN_H);
            drop(_s_bars);
            let _s_tl = crate::perf::Span::new("lanes.timeline");
            let events = crate::shell::timeline::paint(ui, theme, ingest, &fields_of, timeline_h);
            drop(_s_tl);
            if let Some(drop) = events.dropped {
                drop_on_timeline(ingest, state, drop);
            }
            os_drops_on_sequence(ui, body, door, ingest, theme);
            if events.new_layer {
                state.focus_lanes = true;
            }
            if events.blank {
                state.blank_click = true;
            }
            sync_audio(door, state, ingest);
            // The floating preview over the timeline: the row being authored,
            // cut by the active track's mask.
            let _s_prev = crate::perf::Span::new("lanes.preview");
            ingest_preview(ui, theme, door, state, ingest, state_val, &choices, focused);
            drop(_s_prev);

            // Bar 2 — the transport and the tick/seconds readout on the left,
            // the dataset pickers flush right.
            if let Some(part) = bottom_bar(ui, theme, door, ingest) {
                open_part(door, state, ingest, &part);
            }
            if let Some(note) = ingest.note.clone() {
                ui.label(RichText::new(note).font(theme.font_small.clone()).color(theme.ink_weak));
            }
            crate::shell::song_pull::line(ui, theme, door, &ingest.song);
        });
    });
}

/// Opens a Colourist dataset part (`GET /author/dataset/example`) as a clip
/// of its song, marks on its span. The song's saved project is not loaded
/// over it: the part is what was asked to be seen.
fn open_part(door: &dyn Call, state: &mut PanelState, ingest: &mut crate::shell::lanes::IngestLane, part: &str) {
    let Some(ex) = get(door, "GET", "/author/dataset/example", &[("part", part.to_string())]) else {
        ingest.note = Some(format!("part {part}: could not open"));
        return;
    };
    let (Some(song), Some(model)) = (ex["song"].as_str(), ex["model"].as_str()) else {
        ingest.note = Some(format!("part {part}: no song or no model"));
        return;
    };
    let keys: std::collections::BTreeMap<usize, Vec<f32>> = ex["ticks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let floats = t["floats"].as_array()?.iter().filter_map(|v| v.as_f64()).map(|v| v as f32).collect();
            Some((t["tick"].as_u64()? as usize, floats))
        })
        .collect();
    let first = keys.keys().next().copied().unwrap_or(0);
    let last = keys.keys().next_back().copied().unwrap_or(first);
    let span = ex["span"]
        .as_array()
        .and_then(|s| Some((s.first()?.as_u64()? as usize, s.get(1)?.as_u64()? as usize)))
        .unwrap_or((first, last));
    let n = keys.len();
    ingest.adopt_part(song, model, keys, span);
    state.project_loaded_for = Some(song.to_string());
    ingest.note = Some(format!(
        "part {part} · {model} · {} — {n} keypoint(s), ticks {}–{}",
        ex["kind"].as_str().unwrap_or("?"),
        span.0,
        span.1
    ));
}

/// Fetches the song (matrix, hits) and the active track's keypoints once
/// a model and a song are both set — and, while the song is still being
/// digested, says how far along it is.
fn load_song(ui: &Ui, door: &dyn Call, ingest: &mut crate::shell::lanes::IngestLane, layer: Option<&str>, state: &mut PanelState) {
    if ingest.waiting_digest {
        let song = ingest.song.trim().to_string();
        let row = get(door, "GET", "/author/songs", &[])
            .and_then(|v| v["songs"].as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .find(|s| s["name"].as_str() == Some(song.as_str()));
        match row.as_ref().and_then(|s| s["status"].as_str()) {
            Some("digested") => {
                ingest.waiting_digest = false;
                ingest.note = Some(format!("{song} digested — loading…"));
            }
            Some(status) => {
                let hint = match status {
                    "queued" => "queued".to_string(),
                    "digesting" => "digesting…".to_string(),
                    "failed" => format!(
                        "failed — {}",
                        row.as_ref()
                            .and_then(|s| s["error"].as_str().or(s["note"].as_str()))
                            .unwrap_or("no reason")
                    ),
                    "undigested" => "undigested".to_string(),
                    other => other.to_string(),
                };
                ingest.note = Some(format!("{hint} — waiting for {song}"));
                crate::shell::frames::repaint_after(ui.ctx(), std::time::Duration::from_secs(1));
            }
            None => ingest.note = Some(format!("{song} is not in the library")),
        }
    }
    // The fetch itself: a model's keyframes and the song's digest. Off the UI
    // thread when the shell has a worker — the lane keeps its state until the
    // answer is polled, then adopts it here.
    let song = ingest.song.trim().to_string();
    let job = if !ingest.needs_load() {
        // No model to load keypoints for: the song's own rows still load.
        if !ingest.needs_matrix() {
            return;
        }
        SongJob { song: song.clone(), keyframes: None }
    } else {
        let mut query = vec![("model".to_string(), ingest.model.clone()), ("song".to_string(), song.clone())];
        if let (Some(d), Some(l)) = (&ingest.device, layer) {
            query.push(("device".to_string(), d.clone()));
            query.push(("layer".to_string(), l.to_string()));
        }
        SongJob { song: song.clone(), keyframes: Some(query) }
    };
    let loaded = match state.worker.clone() {
        None => Some(fetch_song(door, &job)),
        Some(worker) => {
            if state.song_flight.as_ref().is_none_or(|(flying, _)| *flying != job) {
                let answer = worker.fetch(job.clone());
                state.song_flight = Some((job.clone(), answer));
            }
            let polled = state.song_flight.as_ref().map(|(_, answer)| answer.try_recv());
            match polled {
                Some(Ok(load)) => {
                    state.song_flight = None;
                    Some(load)
                }
                Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => None,
                // The worker is gone: fetch inline rather than wait forever.
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                    state.song_flight = None;
                    Some(fetch_song(door, &job))
                }
            }
        }
    };
    let Some(load) = loaded else {
        ingest.note = Some(format!("loading {song}…"));
        crate::shell::frames::repaint_after(ui.ctx(), std::time::Duration::from_millis(30));
        return;
    };
    match load.keyframes {
        None => ingest.load_matrix(load.matrix, load.hits),
        Some(keyframes) => {
            ingest.load(load.matrix, load.hits, keyframes);
            ingest.note = Some(if ingest.waiting_digest {
                "no matrix — waiting for the digest".to_string()
            } else {
                format!("loaded — {} keyframe(s), {} hit(s)", ingest.keyframes.len(), ingest.song_hits.len())
            });
        }
    }
}

/// What one song load fetches: the digest and the song's hit ticks, and — when
/// a model is picked — its keyframes, `keyframes` being the query for them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SongJob {
    pub song: String,
    pub keyframes: Option<Vec<(String, String)>>,
}

/// The answer to a [`SongJob`]. `keyframes` is `None` for a matrix-only job.
pub(crate) struct SongLoad {
    pub matrix: Option<crate::shell::lanes::DigestMatrix>,
    pub hits: std::collections::BTreeSet<usize>,
    pub keyframes: Option<BTreeMap<usize, Vec<f32>>>,
}

/// The reads one song load makes, in the order they always were: the digest,
/// then (with a model) the keyframes, then the hits. Runs on the UI thread
/// without a worker, and on the worker's own door with one.
pub(crate) fn fetch_song(door: &dyn Call, job: &SongJob) -> SongLoad {
    let matrix = get(door, "GET", "/author/grade/matrix", &[("song", job.song.clone())]).and_then(|reply| serde_json::from_value(reply).ok());
    let keyframes = job.keyframes.as_ref().map(|query| {
        let query: Vec<(&str, String)> = query.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        get(door, "GET", "/author/models/example", &query)
            .and_then(|reply| reply["ticks"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|t| {
                let floats = t["floats"].as_array()?.iter().filter_map(|v| v.as_f64()).map(|v| v as f32).collect();
                Some((t["tick"].as_u64()? as usize, floats))
            })
            .collect()
    });
    let hits = get(door, "GET", "/author/models/song-hits", &[("song", job.song.clone())])
        .and_then(|reply| reply["ticks"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_u64())
        .map(|v| v as usize)
        .collect();
    SongLoad { matrix, hits, keyframes }
}

/// A labelled combo of `(id, label)` choices; `None` is the `—` entry when
/// `none` names it. Returns the picked id (`Some(None)` for the `—`).
fn combo(
    ui: &mut Ui,
    theme: &Theme,
    salt: &str,
    label: &str,
    current: Option<&str>,
    none: Option<&str>,
    choices: &[(String, String)],
    hover: Option<&str>,
) -> Option<Option<String>> {
    ui.label(RichText::new(label).font(theme.font_small.clone()).color(theme.ink_weak));
    let shown = current
        .and_then(|c| choices.iter().find(|(id, _)| id == c).map(|(_, l)| l.clone()).or(Some(c.to_string())))
        .unwrap_or_else(|| none.unwrap_or("—").to_string());
    let mut picked = None;
    let combo = crate::shell::components::combo_box(theme, Id::new("shell/region/lanes").with(salt))
        .width(theme.control_width)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            if let Some(n) = none {
                if ui.selectable_label(current.is_none(), n).clicked() {
                    picked = Some(None);
                }
            }
            for (id, l) in choices {
                if ui.selectable_label(current == Some(id.as_str()), l.as_str()).clicked() {
                    picked = Some(Some(id.clone()));
                }
            }
        });
    if let Some(text) = hover {
        combo.response.on_hover_text(text);
    }
    picked
}

/// Bar 1 — the page strip on the left and the edit controls flush right:
/// `import ▾` (the models, as the second-level `animation` menu, each pick
/// stacking a clip aimed at it) merged with the zoom `−`/`+` in one cluster.
/// Returns true when `fx` asks for a new layer, so the caller focuses the
/// region.
fn top_bar(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    ingest: &mut crate::shell::lanes::IngestLane,
    import_clicked: &mut bool,
    outputs: &[Value],
) {
    ui.horizontal(|ui| {
        // Right cluster reserved first, then the pages fill the space left of
        // it. The column flows right-to-left, so paint the cluster, then the
        // pages' own left-to-right strip in what remains. Reserving first keeps
        // the bar a single row whatever the theme's spacing: the height above
        // the timeline never depends on a per-style wrap.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Added right-to-left, so the cluster reads `import ▾ − +`.
            if ui
                .button(RichText::new("+").font(theme.font_small.clone()))
                .on_hover_text("zoom in (wheel · ctrl + wheel: row height · shift: move · alt: up/down)")
                .clicked()
            {
                ingest.span = (ingest.span / 2).max(crate::shell::timeline::SPAN_MIN);
            }
            if ui
                .button(RichText::new("−").font(theme.font_small.clone()))
                .on_hover_text("zoom out (wheel · ctrl + wheel: row height · shift: move · alt: up/down)")
                .clicked()
            {
                ingest.span = (ingest.span * 2).min(crate::shell::timeline::SPAN_MAX);
            }
            if ui
                .button(RichText::new("import").font(theme.font_small.clone()))
                .on_hover_text("add a clip with a model, or an fx layer")
                .clicked()
            {
                *import_clicked = true;
            }
            // The page strip fills the space left of the cluster, one row.
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                pages_row(ui, theme, door, ingest, outputs);
            });
        });
    });
}

/// G2/G3: a fixture or rig page's own row. To the left, the `+ lane ` menu
/// places a lane for a venue fixture (`◉`) or a `Fixture.group`/rig (`▣`), one
/// page each — the fixture family's own dropdown. When the current page belongs
/// to a fixture or rig, the `value` toggle follows: the lane drives its fixture
/// either as a run of authored keyframes, or as one constant held while its grid
/// button is on (read back through the one `fixture_value_at`).
fn fixture_row(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    ingest: &mut crate::shell::lanes::IngestLane,
    open_graph: Option<&str>,
) {
    use crate::shell::lanes::FixtureValue;
    let fixtures = venue_fixture_list(door);
    let page = ingest.current_page();
    let fixture_page = page.fixture.is_some() || page.group.is_some();
    if fixtures.is_empty() && !fixture_page {
        return;
    }
    ui.horizontal(|ui| {
        if !fixtures.is_empty() {
            ui.menu_button(RichText::new("+ lane ").font(theme.font_small.clone()), |ui| {
                ui.label(RichText::new("fixtures").font(theme.font_small.clone()).color(theme.ink_weak));
                for (id, name, _, profile) in &fixtures {
                    if ui.button(RichText::new(format!("◉ {name}")).font(theme.font_small.clone())).clicked() {
                        let columns = fixture_columns_for(door, profile.as_deref());
                        ingest.add_fixture_lane(Some(id.clone()), None, columns);
                        ui.close();
                    }
                }
                // F6: a rig lane takes the first member's own columns.
                let group_columns = |g: &str| {
                    let profile = fixtures.iter().find(|(_, _, fg, _)| fg.as_deref() == Some(g)).and_then(|(_, _, _, p)| p.clone());
                    fixture_columns_for(door, profile.as_deref())
                };
                let mut groups: Vec<String> = fixtures.iter().filter_map(|(_, _, g, _)| g.clone()).collect();
                groups.sort();
                groups.dedup();
                if !groups.is_empty() {
                    ui.separator();
                    ui.label(RichText::new("groups").font(theme.font_small.clone()).color(theme.ink_weak));
                    for g in &groups {
                        if ui.button(RichText::new(format!("▣ {g}")).font(theme.font_small.clone())).clicked() {
                            let columns = group_columns(g);
                            ingest.add_fixture_lane(None, Some(g.clone()), columns);
                            ui.close();
                        }
                    }
                }
            })
            .response
            .on_hover_text("a lane for a fixture or a group of the venue");
        }
        if fixture_page {
            if !fixtures.is_empty() {
                ui.separator();
            }
            ui.label(RichText::new("value").font(theme.font_small.clone()).color(theme.ink_weak));
            for (mode, label) in [
                (FixtureValue::Keyframe, "keyframes"),
                (FixtureValue::Constant, "constant"),
            ] {
                if ui
                    .selectable_label(ingest.value_mode == mode, RichText::new(label).font(theme.font_small.clone()))
                    .clicked()
                {
                    ingest.set_value_mode(mode);
                }
            }
            if ingest.value_mode == FixtureValue::Constant {
                let mut held = ingest.value;
                if crate::shell::components::float_row(ui, &mut held, 0.0..=1.0, crate::shell::components::Float::Slider).changed() {
                    ingest.set_fixture_value(held);
                }
            }
            // F6: a fixture page's lane has one column per selector its
            // profile declares; the header names them and shows the value the
            // lane drives at the playhead.
            if !ingest.columns.is_empty() {
                ui.label(
                    RichText::new(ingest.columns.join(" · "))
                        .font(theme.font_small.clone())
                        .color(theme.ink_weak),
                );
            }
            if let Some(v) = ingest.fixture_value_at(ingest.active, ingest.playhead) {
                let text = if ingest.columns.is_empty() {
                    format!("{:.2}", v.first().copied().unwrap_or(0.0))
                } else {
                    v.iter().map(|x| format!("{x:.2}")).collect::<Vec<_>>().join(" · ")
                };
                ui.label(RichText::new(text).font(theme.font_small.clone()).color(theme.ink_weak));
            }
            // F7: a fixture page can say where its colour comes from — the
            // picture it samples, a model's own colour slots, or a signal row
            // — without the operator wiring a node by hand. It writes
            // `colour_from` (and the model pick's `spread`/`pick`) on the
            // fixture's own node through the same routes the canvas uses,
            // creating the node from the F9 add route when it is missing.
            if let (Some(fid), Some(gid)) = (page.fixture.clone(), open_graph) {
                fixture_colour_control(ui, theme, door, gid, &fid);
            }
        }
    });
}

/// F7: the colour source a fixture page's lane can pick, written on the
/// fixture's own `venue.fixture` node. `picture`/`signal` just set
/// `colour_from`; a model also wires the model's colour slot into the node's
/// `colour` input and sets `spread = "same"`, `pick = slot`, so "this fixture
/// follows the Aurora gradient" needs no hand wiring. The node is created from
/// the F9 add route when it is missing; every write is a route call (rule 6).
fn fixture_colour_control(ui: &mut Ui, theme: &Theme, door: &dyn Call, gid: &str, fixture: &str) {
    use crate::shell::panels::props_venue;
    let Some(graph) = get(door, "GET", "/graph", &[("id", gid.to_string())]) else { return };
    let venue = get(door, "GET", "/venue", &[("id", "last".to_string())])
        .as_ref()
        .and_then(|v| v["venue"]["venue"]["id"].as_str())
        .unwrap_or_default()
        .to_string();
    let node = graph["graph"]["nodes"].as_array().into_iter().flatten().find(|n| {
        n["kind"] == json!("venue.fixture")
            && n["config"]["venue"] == json!(venue)
            && n["config"]["fixture"] == json!(fixture)
    });
    let node_id = node.and_then(|n| n["id"].as_str()).map(str::to_string);
    let from = node
        .and_then(|n| n["config"]["colour_from"].as_str())
        .unwrap_or("frame")
        .to_string();
    let models = props_venue::colour_model_nodes(door, &graph);
    let current_model = node_id.as_deref().and_then(|id| props_venue::colour_edge_source(&graph, id));

    ui.separator();
    ui.label(
        RichText::new(tr("colour")).font(theme.font_small.clone()).color(theme.ink_weak),
    );
    let picked = if from == "model" {
        tr("from model")
    } else if from == "signal" {
        tr("from signal")
    } else {
        tr("from picture")
    };
    ui.menu_button(
        RichText::new(format!("{picked} ")).font(theme.font_small.clone()),
        |ui| {
            for (value, label) in [
                ("frame", tr("from picture")),
                ("signal", tr("from signal")),
            ] {
                if ui.selectable_label(from == value, label).clicked() {
                    if let Some(id) = ensure_fixture_node(door, gid, &venue, fixture) {
                        props_venue::post_config(door, gid, &id, json!({ "colour_from": value }));
                    }
                    ui.close();
                }
            }
            if !models.is_empty() {
                ui.separator();
                ui.label(RichText::new(tr("from model")).font(theme.font_small.clone()).color(theme.ink_weak));
                for (id, kind) in &models {
                    let label = format!("{id} · {kind}");
                    if ui.selectable_label(current_model.as_deref() == Some(id.as_str()), label).clicked() {
                        if let Some(node_id) = ensure_fixture_node(door, gid, &venue, fixture) {
                            props_venue::wire_colour_model(door, gid, &node_id, &graph, id, kind);
                        }
                        ui.close();
                    }
                }
            }
        },
    );
    if from == "model" && node_id.is_some() {
        let id = node_id.clone().unwrap_or_default();
        let slot = node.and_then(|n| n["config"]["pick"].as_u64()).unwrap_or(0);
        ui.menu_button(
            RichText::new(trf("slot {n} ", &[("n", &slot.to_string())])).font(theme.font_small.clone()),
            |ui| {
                for s in 0u64..4 {
                    if ui.selectable_label(slot == s, format!("{s}")).clicked() {
                        props_venue::post_config(door, gid, &id, json!({ "pick": s, "spread": "same" }));
                        ui.close();
                    }
                }
            },
        );
    }
}

/// F7: the fixture's own `venue.fixture` node id, creating it from the F9 add
/// route when the open graph has none — the same `POST /graph/node {op:"add"}`
/// the settings "add node" button uses.
fn ensure_fixture_node(door: &dyn Call, gid: &str, venue: &str, fixture: &str) -> Option<String> {
    if let Some(g) = get(door, "GET", "/graph", &[("id", gid.to_string())]) {
        let existing = g["graph"]["nodes"].as_array().into_iter().flatten().find(|n| {
            n["kind"] == json!("venue.fixture")
                && n["config"]["venue"] == json!(venue)
                && n["config"]["fixture"] == json!(fixture)
        });
        if let Some(id) = existing.and_then(|n| n["id"].as_str()) {
            return Some(id.to_string());
        }
    }
    let reply = door
        .json(
            "POST",
            "/graph/node",
            Some(json!({
                "op": "add", "graph": gid, "kind": "venue.fixture",
                "config": { "venue": venue, "fixture": fixture },
            })),
        )
        .ok()?;
    reply["node"].as_str().map(str::to_string)
}

/// Bar 2 — the transport and the tick/seconds readout on the left, the
/// Colourist dataset's status filter and part picker flush right. Returns the
/// picked dataset part, if any (the caller opens it).
fn bottom_bar(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    ingest: &mut crate::shell::lanes::IngestLane,
) -> Option<String> {
    // The Colourist dataset's authored parts — each opens as a clip of its
    // song on its own span (`GET /author/dataset`). A status filter, so with
    // thousands of parts the ones left to review are one pick away;
    // `approve` files the open part (`POST /author/dataset/status`).
    let listed = get(door, "GET", "/author/dataset", &[]).and_then(|v| v["parts"].as_array().cloned()).unwrap_or_default();
    let filter_id = ui.id().with("dataset-filter");
    let open_id = ui.id().with("dataset-open");
    let filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_else(|| "all".to_string());
    let parts: Vec<(String, String)> = listed
        .iter()
        .filter(|p| filter == "all" || p["status"].as_str().unwrap_or("pending") == filter)
        .filter_map(|p| {
            let id = p["part"].as_str()?.to_string();
            let title = p["title"].as_str().or(p["song"].as_str()).unwrap_or_default();
            let mark = if p["status"].as_str() == Some("approved") { "" } else { "" };
            let label = format!("{mark} {id} · {} · {} · {title}", p["model"].as_str().unwrap_or("?"), p["kind"].as_str().unwrap_or("?"));
            Some((id, label))
        })
        .collect();
    let filters: Vec<(String, String)> = if listed.is_empty() {
        Vec::new()
    } else {
        let pending = listed.iter().filter(|p| p["status"].as_str() != Some("approved")).count();
        vec![
            ("all".to_string(), format!("all ({})", listed.len())),
            ("pending".to_string(), format!("pending ({pending})")),
            ("approved".to_string(), format!("approved ({})", listed.len() - pending)),
        ]
    };
    let mut picked_part = None;
    ui.horizontal(|ui| {
        compact_row(ui, theme);
        // Left: the transport and the tick/seconds readout.
        if ui.button(lucide(Icon::SkipBack, theme.font_small.size)).on_hover_text("to the start").clicked() {
            ingest.goto(0);
        }
        let play = if ingest.playing.is_some() { Icon::Pause } else { Icon::Play };
        if ui.button(lucide(play, theme.font_small.size)).on_hover_text("play the sequence in the preview (space)").clicked() {
            ingest.toggle_play(ui.input(|i| i.time));
        }
        if ui.button(lucide(Icon::StepBack, theme.font_small.size)).on_hover_text("previous keypoint").clicked() {
            if let Some(t) = ingest.neighbour_keyframe(false) {
                ingest.goto(t);
            }
        }
        let range = match ingest.mark {
            Some((a, b)) if a != b => format!("{a}–{b}"),
            _ => ingest.playhead.to_string(),
        };
        ui.label(
            RichText::new(format!("tick {range} · {}", crate::shell::timeline::clock(ingest.playhead)))
                .font(theme.font_small.clone())
                .color(theme.ink),
        );
        if ui.button(lucide(Icon::StepForward, theme.font_small.size)).on_hover_text("next keypoint").clicked() {
            if let Some(t) = ingest.neighbour_keyframe(true) {
                ingest.goto(t);
            }
        }
        // Right: the dataset filter and picker (added right-to-left, so
        // `status` sits left of `dataset`).
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if listed.is_empty() {
                return;
            }
            let hover = "a part of the Colourist dataset: opens its song, its model and its span";
            if let Some(Some(part)) = combo(ui, theme, "dataset", "dataset", None, Some("— part"), &parts, Some(hover)) {
                ui.data_mut(|d| d.insert_temp(open_id, part.clone()));
                picked_part = Some(part);
            }
            if let Some(Some(f)) = combo(ui, theme, "dataset-filter", "status", Some(filter.as_str()), None, &filters, None) {
                ui.data_mut(|d| d.insert_temp(filter_id, f));
            }
            let open: Option<String> = ui.data(|d| d.get_temp(open_id));
            if let Some(open) = open {
                let approved = listed.iter().any(|p| p["part"].as_str() == Some(open.as_str()) && p["status"].as_str() == Some("approved"));
                let (text, to) = if approved { (format!(" {open} approved"), "pending") } else { (format!("approve {open}"), "approved") };
                let hover = if approved { "back to pending" } else { "mark the open part as approved" };
                if ui.button(RichText::new(text).font(theme.font_small.clone())).on_hover_text(hover).clicked() {
                    match door.json("POST", "/author/dataset/status", Some(json!({ "part": open, "status": to }))) {
                        Ok(_) => ingest.note = Some(format!("part {open}: {}", if to == "approved" { "approved" } else { "pending" })),
                        Err((_, msg)) => ingest.note = Some(format!("status of {open}: {msg}")),
                    }
                }
            }
        });
    });
    picked_part
}

/// The sequence editor's left icon column: the controls that act on the
/// playhead or the marked range, stacked as glyph-only buttons so the
/// timeline keeps the panel. Each glyph's meaning is its hover text.
fn icon_column(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    ingest: &mut crate::shell::lanes::IngestLane,
    layer: Option<&str>,
) {
    let icon = |i: Icon| lucide(i, theme.font_small.size);
    panel_frame(theme).show(ui, |ui| {
        ui.vertical(|ui| {
            // Six glyph buttons spread down the full panel height, so the
            // column uses all the space the timeline isn't: the controls are
            // as tall a target as the panel allows instead of a short stack
            // at the top.
            let n = 6.0_f32;
            let btn = ui.spacing().interact_size.y;
            let gap = ((ui.available_height() - n * btn) / (n - 1.0)).max(theme.item_gap.y);
            // Keypoint: file or clear the active track's floats at the
            // playhead — one toggle that reads the map it edits.
            let at = ingest.playhead;
            let has = ingest.keyframes.contains_key(&at);
            let kp_hover = if has {
                "remove the keypoint at this tick"
            } else {
                "set the properties floats at this tick"
            };
            if ui.button(icon(Icon::Keypoint)).on_hover_text(kp_hover).clicked() {
                if has {
                    ingest.clear_keyframe();
                } else if ingest.model.is_empty() {
                    ingest.note = Some("pick a model first".into());
                } else {
                    ingest.write_keyframe();
                    ingest.note = None;
                }
            }
            ui.add_space(gap);
            if ui.button(icon(Icon::Copy)).on_hover_text("the marked range on the ruler").clicked() {
                let n = ingest.copy_range();
                ingest.note = Some(format!("{n} keypoint(s) copiados"));
            }
            ui.add_space(gap);
            if ui.button(icon(Icon::Paste)).on_hover_text("at the playhead").clicked() {
                let n = ingest.paste();
                ingest.note = Some(format!("{n} keypoint(s) pegados"));
            }
            ui.add_space(gap);
            if ui.button(icon(Icon::Trash)).on_hover_text("delete the marked range and remove the active clip").clicked() {
                // One undo step: the clear and the track removal are one
                // gesture to the operator (`delete_range_and_track`).
                let n = ingest.delete_range_and_track();
                ingest.note = Some(format!("{n} keypoint(s) deleted · active clip removed"));
            }
            ui.add_space(gap);
            if ui.button(icon(Icon::Undo)).on_hover_text("undo").clicked() {
                ingest.undo();
            }
            ui.add_space(gap);
            let save = ui.menu_button(icon(Icon::Save), |ui| save_menu(ui, theme, door, ingest, layer));
            save.response.on_hover_text("save: sequence, dataset, hits");
        });
    });
}

/// `save`: every persistence verb in one place.
fn save_menu(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    ingest: &mut crate::shell::lanes::IngestLane,
    layer: Option<&str>,
) {
    let small = |t: &str| RichText::new(t.to_string()).font(theme.font_small.clone());
    if ui
        .button(small("save sequence"))
        .on_hover_text("all pages, layers and clips of the song, in one file")
        .clicked()
    {
        ingest.note = match ingest.project_body() {
            None => Some("pick a song first".into()),
            Some(body) => {
                let n = body["clips"].as_array().map_or(0, Vec::len);
                match door.json("POST", "/author/project", Some(body)) {
                    Ok(_) => Some(format!("sequence saved — {n} clip(s), {} page(s)", ingest.pages().len())),
                    Err((_, msg)) => Some(msg),
                }
            }
        };
        ui.close();
    }
    if ui.button(small("save dataset")).on_hover_text("all tracks, each to its device").clicked() {
        let bodies = ingest.all_example_bodies(8.0, layer);
        ingest.note = if bodies.is_empty() {
            Some("pick a model, a song and author at least one tick".into())
        } else {
            let n = bodies.len();
            let failed: Vec<String> = bodies
                .into_iter()
                .filter_map(|b| door.json("POST", "/author/models/example", Some(b)).err().map(|(_, m)| m))
                .collect();
            Some(if failed.is_empty() { format!("dataset saved — {n} track(s)") } else { failed.join(" · ") })
        };
        ui.close();
    }
    if ui.button(small("save as sequence")).on_hover_text("the active track, as an overlay button").clicked() {
        let name = format!("{}-{}", ingest.model, ingest.song.trim());
        match ingest.movement_body(&name) {
            None => ingest.note = Some("pick a model and author at least one tick".into()),
            Some(body) => {
                ingest.note = match door.json("POST", "/author/models/library", Some(body)) {
                    Ok(_) => Some(format!("saved as sequence: {name}")),
                    Err((_, msg)) => Some(msg),
                };
            }
        }
        ui.close();
    }
    if ui.button(small("save hits")).clicked() {
        let song = ingest.song.trim().to_string();
        ingest.note = if song.is_empty() {
            Some("pick a song first".into())
        } else {
            match door.json("POST", "/author/models/song-hits", Some(crate::shell::timeline::hits_body(&song, &ingest.song_hits))) {
                Ok(_) => Some(format!("hits saved — {}", ingest.song_hits.len())),
                Err((_, msg)) => Some(msg),
            }
        };
        ui.close();
    }
}

/// The venue's fixtures off `GET /venue?id=last` as `(id, name, group,
/// profile)` — what both the `+ page` menu and the G2 `+ lane` dropdown
/// offer. A `Venue` nests its fixtures under `venue.fixture` (the `Meta`
/// table), the shape [`fixture_controller_rows`] already reads. F6 carries the
/// row's profile so a fixture lane can lay out the profile's columns.
fn venue_fixture_list(door: &dyn Call) -> Vec<(String, String, Option<String>, Option<String>)> {
    get(door, "GET", "/venue", &[("id", "last".to_string())])
        .as_ref()
        .and_then(|v| v["venue"]["fixture"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|f| {
            let id = f["id"].as_str()?.to_string();
            let name = f["name"].as_str().unwrap_or(&id).to_string();
            let group = f["group"].as_str().map(str::to_string);
            let profile = f["profile"].as_str().map(str::to_string);
            Some((id, name, group, profile))
        })
        .collect()
}

/// F6: the columns a fixture page's lane offers — the profile's declared
/// selectors (`GET /fixtures`'s `selectors` map), filtered to
/// [`fixture_columns`]'s order. Empty when the profile is unknown, so a lane
/// with no profile keeps the legacy single-value row.
fn fixture_columns_for(door: &dyn Call, profile: Option<&str>) -> Vec<String> {
    let Some(profile) = profile else { return Vec::new() };
    let selectors = get(door, "GET", "/fixtures", &[])
        .as_ref()
        .and_then(|v| v["selectors"].get(profile).and_then(|s| s.as_array()).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    crate::shell::lanes::fixture_columns(&selectors)
}

/// The sequence's pages — one per final output (a device, or a mask of
/// it), each with its own layers — as tabs, and `+ page` for the next
/// output that has none. G2: the menu also offers a **page per fixture** and
/// a **page per rig** (`Fixture.group`), one each that has no page yet.
fn pages_row(ui: &mut Ui, theme: &Theme, door: &dyn Call, ingest: &mut crate::shell::lanes::IngestLane, outputs: &[Value]) {
    use crate::shell::lanes::Page;
    let current = ingest.current_page();
    let pages = ingest.pages();
    // A plain `horizontal`, never `horizontal_wrapped`: this strip shares its
    // row with the edit cluster to its right, so it must stay a single line.
    // A per-style reflow here would change the bar's height (and so the
    // timeline's), which the restyle gate forbids.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = theme.segment_gap;
        for page in &pages {
            let active = page == &current;
            if page_tab(ui, theme, &page.label(), active)
                .on_hover_text("the layers of this output")
                .clicked()
                && !active
            {
                ingest.select_page(page);
            }
        }
        // Every output, then every mask of it, that has no page yet; then
        // G2's every venue fixture and every `Fixture.group`.
        let output_pages: Vec<Page> = outputs
            .iter()
            .filter_map(|o| o["id"].as_str().map(|d| (d, o)))
            .flat_map(|(d, o)| {
                let masks = o["masks"].as_array().cloned().unwrap_or_default();
                std::iter::once(Page { device: Some(d.to_string()), mask: None, ..Page::default() }).chain(
                    masks
                        .into_iter()
                        .filter_map(|m| m["id"].as_str().map(str::to_string))
                        .map(move |m| Page { device: Some(d.to_string()), mask: Some(m), ..Page::default() }),
                )
            })
            .collect();
        let mut group_names: Vec<String> = Vec::new();
        let mut fixture_pages: Vec<Page> = Vec::new();
        for (id, _, group, _) in venue_fixture_list(door) {
            fixture_pages.push(Page { fixture: Some(id), ..Page::default() });
            if let Some(g) = group {
                if !group_names.contains(&g) {
                    group_names.push(g);
                }
            }
        }
        let rig_pages = group_names.into_iter().map(|g| Page { group: Some(g), ..Page::default() });
        let free: Vec<Page> = output_pages
            .into_iter()
            .chain(fixture_pages)
            .chain(rig_pages)
            .filter(|p| !pages.contains(p))
            .collect();
        ui.menu_button(RichText::new(tr("+ page")).font(theme.font_small.clone()), |ui| {
            if free.is_empty() {
                ui.label(RichText::new("each output, fixture and group already has its page").font(theme.font_small.clone()).color(theme.ink_weak));
            }
            for p in &free {
                if ui.button(RichText::new(p.label()).font(theme.font_small.clone())).clicked() {
                    ingest.select_page(p);
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text(tr("add a page"));
    });
}

/// One page tab in [`pages_row`]: a quiet selectable label when inactive, and
/// the current page's own lit state (the theme's selection wash) with an
/// accent rule painted under it. Every colour is a `Theme` token (gate 2).
fn page_tab(ui: &mut Ui, theme: &Theme, text: &str, active: bool) -> Response {
    let response = ui.selectable_label(
        active,
        RichText::new(text)
            .font(theme.font_small.clone())
            .color(if active { theme.ink } else { theme.ink_weak }),
    );
    if active {
        let rect = response.rect;
        ui.painter().line_segment(
            [Pos2::new(rect.left(), rect.bottom()), Pos2::new(rect.right(), rect.bottom())],
            theme.accent(),
        );
    }
    response
}

/// V1 — the layer editor: the selected page's device stack, one row per
/// layer with reorder, blend, opacity, mask, sector, active and solo.
///
/// **Regla 4.** The stack is a *reading* of the device's layers, not a second
/// state: it comes off `GET /state`'s `outputs[].layers` every frame and is
/// ordered by `z` (front first, the reading every layer panel works in), so
/// the table and the render can never disagree. Every control posts the
/// operator's intent through the same `POST /layers` the AI's `edit_layer`
/// tool uses (rule 6), via `call::door::Door` and never a handler (rule 9).
fn layer_editor(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    ingest: &crate::shell::lanes::IngestLane,
    outputs: &[Value],
) {
    let Some(device) = ingest.current_page().device else {
        return;
    };
    let Some(output) = outputs
        .iter()
        .find(|o| o["id"].as_str() == Some(device.as_str()))
    else {
        return;
    };
    let mut layers: Vec<Value> = output["layers"].as_array().cloned().unwrap_or_default();
    // Front first: `layers` arrives in creation order, `z` is the order.
    layers.sort_by(|a, b| b["z"].as_i64().unwrap_or(0).cmp(&a["z"].as_i64().unwrap_or(0)));

    ui.separator();
    section_heading(ui, theme, tr("layer stack"));
    if layers.is_empty() {
        ui.label(
            RichText::new(tr("no layers on this page"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        return;
    }

    // The four blend modes, each with its own key so the label never
    // collides with a same-spelled word elsewhere in the tables.
    const BLEND_LABELS: [(&str, &str); 4] = [
        ("normal", tr_key("blend normal")),
        ("add", tr_key("blend add")),
        ("multiply", tr_key("blend multiply")),
        ("screen", tr_key("blend screen")),
    ];
    let blend_options: Vec<(String, &str)> = BLEND_LABELS
        .iter()
        .map(|(v, k)| ((*v).to_string(), tr(*k)))
        .collect();
    let mut mask_options: Vec<(String, &str)> = vec![(String::new(), tr("none"))];
    if let Some(masks) = output["masks"].as_array() {
        for m in masks {
            let Some(id) = m["id"].as_str() else { continue };
            mask_options.push((id.to_string(), m["name"].as_str().unwrap_or(id)));
        }
    }

    for (i, layer) in layers.iter().enumerate() {
        let id = layer["id"].as_str().unwrap_or_default().to_string();
        let name = layer["name"].as_str().unwrap_or(&id).to_string();

        // The header row is both the reorder control (the arrows, focusable
        // and clickable) and a drag handle: dropping one layer's row onto
        // another moves it to that slot through the same `raise`/`lower`
        // ops, so both gestures post the one route.
        let row = ui.dnd_drag_source(Id::new(("layer-drag", id.as_str())), id.clone(), |ui| {
            ui.horizontal(|ui| {
                let up = ui
                    .add_enabled(
                        i > 0,
                        egui::Button::new(RichText::new("\u{25b2}").font(theme.font_small.clone())),
                    )
                    .on_hover_text(tr("raise layer"));
                if up.clicked() {
                    post_layer_op(door, &device, &id, "raise");
                }
                let down = ui
                    .add_enabled(
                        i + 1 < layers.len(),
                        egui::Button::new(RichText::new("\u{25bc}").font(theme.font_small.clone())),
                    )
                    .on_hover_text(tr("lower layer"));
                if down.clicked() {
                    post_layer_op(door, &device, &id, "lower");
                }
                ui.label(
                    RichText::new(&name)
                        .font(theme.font_body.clone())
                        .color(theme.ink),
                );
            })
        });
        if let Some(dragged) = row.response.dnd_release_payload::<String>() {
            let dragged = dragged.as_str();
            if dragged != id {
                if let Some(from) = layers.iter().position(|l| l["id"].as_str() == Some(dragged)) {
                    let steps = (i as isize - from as isize).unsigned_abs();
                    let op = if i < from { "raise" } else { "lower" };
                    for _ in 0..steps {
                        post_layer_op(door, &device, dragged, op);
                    }
                }
            }
        }

        let mut active = layer["active"].as_bool().unwrap_or(true);
        if toggle_row(ui, theme, tr("layer active"), &mut active).changed() {
            post_layer_set(door, &device, &id, json!({ "active": active }));
        }
        let mut solo = layer["solo"].as_bool().unwrap_or(false);
        if toggle_row(ui, theme, tr("solo layer"), &mut solo).changed() {
            post_layer_set(door, &device, &id, json!({ "solo": solo }));
        }

        let current_blend = layer["blend"].as_str().unwrap_or("normal").to_string();
        let mut blend = current_blend.clone();
        let _ = label_row(ui, theme, tr("blend"), |ui| {
            dropdown(ui, theme, ("layer-blend", id.as_str()), &mut blend, &blend_options)
        });
        if blend != current_blend {
            post_layer_set(door, &device, &id, json!({ "blend": blend }));
        }

        let mut opacity = layer["opacity"].as_f64().unwrap_or(1.0) as f32;
        let opacity_resp = slider_row(ui, theme, tr("opacity"), &mut opacity, 0.0..=1.0, true);
        if opacity_resp.changed() {
            post_layer_set(door, &device, &id, json!({ "opacity": opacity }));
        }

        let current_mask = layer["mask"].as_str().unwrap_or_default().to_string();
        let mut mask = current_mask.clone();
        let _ = label_row(ui, theme, tr("mask"), |ui| {
            dropdown(ui, theme, ("layer-mask", id.as_str()), &mut mask, &mask_options)
        });
        if mask != current_mask {
            post_layer_set(door, &device, &id, json!({ "mask": mask }));
        }

        let sector = &layer["sector"];
        let mut x = sector["x"].as_f64().unwrap_or(0.0) as f32;
        let mut y = sector["and"].as_f64().unwrap_or(0.0) as f32;
        let mut w = sector["w"].as_f64().unwrap_or(1.0) as f32;
        let mut h = sector["h"].as_f64().unwrap_or(1.0) as f32;
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(tr("sector"))
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
            );
            let rx = num_track(ui, theme, &mut x, 0.0..=1.0, "x", 2, 0.0, true);
            let ry = num_track(ui, theme, &mut y, 0.0..=1.0, "and", 2, 0.0, true);
            let rw = num_track(ui, theme, &mut w, 0.0..=1.0, "w", 2, 1.0, true);
            let rh = num_track(ui, theme, &mut h, 0.0..=1.0, "h", 2, 1.0, true);
            if rx.changed() || ry.changed() || rw.changed() || rh.changed() {
                post_layer_set(
                    door,
                    &device,
                    &id,
                    json!({ "sector": { "x": x, "and": y, "w": w, "h": h } }),
                );
            }
        });
    }
}

/// One layer edit through `POST /layers`, the same route `edit_layer` posts —
/// `set` with only the field that changed, so `apply_fields`'s "only touch
/// what came" leaves the rest alone.
fn post_layer_set(door: &dyn Call, device: &str, layer: &str, fields: Value) {
    let mut body = json!({ "op": "set", "device": device, "layer": layer });
    if let Value::Object(map) = fields {
        for (key, value) in map {
            body[key] = value;
        }
    }
    let _ = door.json("POST", "/layers", Some(body));
}

/// One reorder step through `POST /layers` — the `raise`/`lower` ops, never a
/// renumber: a hand-set `z` elsewhere in the stack survives.
fn post_layer_op(door: &dyn Call, device: &str, layer: &str, op: &str) {
    let _ = door.json(
        "POST",
        "/layers",
        Some(json!({ "op": op, "device": device, "layer": layer })),
    );
}

/// A library file dropped on the timeline: a video becomes a clip at the
/// drop's tick (on that layer, or a new one); a sequence opens as a clip
/// the way `ver` does; an audio file is the song.
fn drop_on_timeline(ingest: &mut crate::shell::lanes::IngestLane, state: &mut PanelState, drop: crate::shell::timeline::Drop) {
    match drop.file.kind.as_str() {
        // An image is a clip too: the still plays for the clip's length, as
        // the gallery's `import` makes it (`import_file`).
        "video" | "image" => {
            let len = crate::shell::timeline::video_ticks(drop.file.seconds);
            ingest.add_video_clip(&drop.file.path, drop.tick, len, drop.layer);
            ingest.note = Some(format!("{} on layer {}", drop.file.name, ingest.layer + 1));
        }
        "audio" => {
            let stem = std::path::Path::new(&drop.file.name).file_stem().and_then(|s| s.to_str()).unwrap_or(&drop.file.name);
            state.pending_song = Some(stem.to_string());
        }
        "sequence" => state.sequence_dropped = Some(drop.file.name.clone()),
        _ => ingest.note = Some(format!("{}: only videos, sequences or audio go on the timeline", drop.file.name)),
    }
}

/// OS files dropped on the sequence — the same drop the nodes canvas takes
/// (`os_drops_on_canvas`), classified by the same `source_from_path`. Each
/// file imports the way the gallery's `import` does ([`import_file`]): an
/// image or video a clip at the playhead, an audio file the song, a folder
/// its importable files. Only while the pointer is over the sequence; a
/// hover hint says what a drop does.
fn os_drops_on_sequence(ui: &Ui, rect: Rect, door: &dyn Call, ingest: &mut crate::shell::lanes::IngestLane, theme: &Theme) {
    let (dropped, hovered, pointer) = ui.input(|i| {
        (i.raw.dropped_files.clone(), i.raw.hovered_files.clone(), i.pointer.interact_pos())
    });
    let Some(pointer) = pointer else {
        return;
    };
    if !rect.contains(pointer) {
        return;
    }
    if !hovered.is_empty() {
        ui.painter_at(rect).text(
            pointer + Vec2::splat(theme.item_gap.x),
            Align2::LEFT_TOP,
            tr("drop to add to the sequence"),
            theme.font_small.clone(),
            theme.phosphor,
        );
    }
    for file in &dropped {
        let path = file.path();
        let kind = if path.is_dir() {
            "folder"
        } else {
            match super::nodes::source_from_path(door, path) {
                Some(("still", _)) => "image",
                Some(("reel.clip", _)) => "video",
                Some(("audio.file", _)) => "audio",
                _ => continue,
            }
        };
        import_file(door, ingest, kind, &path.to_string_lossy());
    }
}

/// The audio clock, while the song sounds (`GET /author/play`: `native` and
/// `at`): the stream position less its output latency on Android, the wall
/// clock from the spawn on desktop ffplay. With whether it is still `sounding`.
/// `None` when nothing is playing, and the playhead keeps to the wall clock.
/// P-D: the sequence position a locked `clock.timecode` asks for, off `GET
/// /timecode` (the route the AI's `timecode_status` reads too).
fn timecode_clock(door: &dyn Call) -> Option<f64> {
    let v = door.json("GET", "/timecode", None).ok()?;
    v["at"].as_f64()
}

fn audible_clock(door: &dyn Call, ingest: &crate::shell::lanes::IngestLane) -> Option<(f64, bool)> {
    ingest.playing?;
    let v = door.json("GET", "/author/play", None).ok()?;
    if v["native"].as_bool() != Some(true) {
        return None;
    }
    Some((v["at"].as_f64()?, v["playing"].as_bool() == Some(true)))
}

/// Keeps the song's audio in step with playback: starts the player from
/// the playhead when playback starts (or jumps), stops it when it stops.
fn sync_audio(door: &dyn Call, state: &mut PanelState, ingest: &mut crate::shell::lanes::IngestLane) {
    let want = ingest.playing.map(|(_, from)| from);
    if want == state.audio_from {
        return;
    }
    state.audio_from = want;
    state.audio_native = false;
    let song = ingest.song.trim();
    let body = match want {
        Some(from) if !song.is_empty() => json!({ "song": song, "from": from as f64 / f64::from(crate::shell::lanes::EDITOR_RATE) }),
        _ => json!({ "stop": true }),
    };
    // The musical clock follows the song's beat map while it plays (T15).
    if let Err((_, msg)) = door.json("POST", "/loops/song", Some(body.clone())) {
        eprintln!("  loops clock: {msg}");
    }
    // A song without its audio file in `data/audio` plays silent: say so in
    // the editor, not only on stderr.
    if let Err((_, msg)) = door.json("POST", "/author/play", Some(body)) {
        eprintln!("  audio: {msg}");
        ingest.note = Some(format!("audio: {msg}"));
    }
}

/// `R22.8`: the floating preview over the ingest canvas. With a model chosen
/// (`R22.8` point 3) it shows — anchored to `ingest.canvas_rect`, expanded
/// on click — the row being authored, through `GET
/// /author/render/solo.jpg?model=<id>&device=&mask=&w=`; each time the row
/// (or the chosen mask) changes it first pushes it with `POST
/// /author/render/floats?model=<id>` (body = the row) so the route renders
/// what the panel is editing, never a per-frame post. While a saved sequence
/// is shown and running (`mv_shown` + a live `overlay.tick`), the picture is
/// the real output (`GET /output.jpg?device=&mask=`) — "the real preview of the sequence".
#[allow(clippy::too_many_arguments)]
fn ingest_preview(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    ingest: &mut crate::shell::lanes::IngestLane,
    state_val: &Value,
    choices: &[(String, String, usize)],
    focused: bool,
) {
    let Some(host) = ingest.canvas_rect else {
        return;
    };
    // 2026-10-01: playing, the picture is the clip on top at the playhead
    // (the highest layer of the page whose marks hold it); authoring, the
    // active clip.
    let tick = ingest.playhead;
    let pick = ingest.playing.and_then(|_| ingest.top_at(tick)).unwrap_or(ingest.active);
    let picked = ingest.track_views().into_iter().find(|t| t.index == pick).map(|t| {
        (t.model.to_string(), t.video.map(str::to_string), t.bounds.map_or(0, |(a, _)| a))
    });
    let Some((model, video, clip_in)) = picked else {
        return;
    };
    if let Some(path) = video {
        // A video clip: the frame at the playhead's offset into the clip,
        // pushed first (only on change) so the picture's query stays put.
        let at = tick.saturating_sub(clip_in);
        if state.video_posted.as_ref() != Some(&(path.clone(), at)) {
            let t = at as f64 / f64::from(crate::shell::lanes::EDITOR_RATE);
            match &state.worker {
                Some(worker) => state.seek_gen = worker.post("seek", "/author/video/seek", vec![], json!({ "path": path, "t": t }).to_string().into_bytes()),
                None => {
                    let _ = door.json("POST", "/author/video/seek", Some(json!({ "path": path, "t": t })));
                }
            }
            state.video_posted = Some((path.clone(), at));
        }
        let ready = state.worker.as_ref().is_none_or(|w| w.landed("seek") >= state.seek_gen);
        let query = vec![("path".to_string(), path)];
        // A video clip's frame is the decoder's, not a render: no camera.
        let subject = sequence_subject(door, state, ingest, None);
        let Some((route, query)) = picture_for(state, ready, ("/author/video/frame.jpg", query)) else {
            repaint_until_landed(ui);
            return;
        };
        if !ready {
            repaint_until_landed(ui);
        }
        show_preview(ui, theme, door, ingest, host, &route, query, focused, subject);
        return;
    }
    if model.is_empty() {
        return;
    }
    let Some(index) = choices.iter().position(|(id, _, _)| id == &model) else {
        // A model this build does not render is not a model this preview can
        // show — rule 5: no guessing an index.
        return;
    };
    let Some(row) = ingest.row_of(pick, tick) else {
        return;
    };
    // The renderer refuses a row of the wrong width, and the picture would
    // then keep the old one: fit the row to the model's schema first.
    let row = fit_width(row, choices[index].2);

    // Point 3: a running, shown sequence takes priority — the real output.
    let shown_running = state.mv_shown.as_ref().and_then(|m| {
        let name = m["name"].as_str()?;
        playhead_for(name, state_val).map(|_| name.to_string())
    });
    let device = ingest
        .device
        .clone()
        .filter(|d| state_val["outputs"].as_array().is_some_and(|os| os.iter().any(|o| o["id"] == json!(d))));

    let (route, query) = match (&shown_running, &device) {
        (Some(_), Some(d)) => {
            let mut q = vec![("device".to_string(), d.clone())];
            if let Some(mask) = &ingest.mask {
                q.push(("mask".to_string(), mask.clone()));
            }
            ("/output.jpg", q)
        }
        _ => {
            // Push the row before showing it — only when it changed.
            let key = (index, row.clone(), ingest.mask.clone());
            if ingest.preview_posted.as_ref() != Some(&key) {
                let body = serde_json::to_vec(&row).unwrap_or_default();
                // By id, not by this list's position: the renderer's own
                // index counts retired tombstones this list skips.
                match &state.worker {
                    Some(worker) => {
                        state.preview_gen = worker.post("floats", "/author/render/floats", vec![("model".to_string(), model.clone())], body);
                    }
                    None => {
                        let mut params = crate::call::Params {
                            query: std::collections::HashMap::new(),
                            body,
                            tail: None,
                        };
                        params.query.insert("model".to_string(), model.clone());
                        let reply = door.call("POST", "/author/render/floats", params);
                        if let Some(r) = reply.filter(|r| r.status >= 400) {
                            eprintln!("[lanes] write refused: /author/render/floats: {} {}", r.status, String::from_utf8_lossy(&r.body).trim());
                        }
                    }
                }
                ingest.preview_posted = Some(key);
            }
            let mut q = vec![("model".to_string(), model.clone())];
            if let Some(d) = &device {
                q.push(("device".to_string(), d.clone()));
            }
            if let Some(mask) = &ingest.mask {
                q.push(("mask".to_string(), mask.clone()));
            }
            ("/author/render/solo.jpg", q)
        }
    };
    let export = match (&shown_running, &device) {
        (Some(_), Some(d)) => json!({ "device": d }),
        _ => {
            let mut b = json!({ "model": model });
            if let Some(d) = &device {
                b["device"] = json!(d);
            }
            if let Some(m) = &ingest.mask {
                b["mask"] = json!(m);
            }
            b
        }
    };
    // The solo picture waits until the row it draws has landed; until then the
    // last picture stays on screen.
    let ready = route != "/author/render/solo.jpg" || state.worker.as_ref().is_none_or(|w| w.landed("floats") >= state.preview_gen);
    let subject = sequence_subject(door, state, ingest, Some(export));
    let refused = state.worker.as_ref().and_then(|w| w.failure("floats"));
    let Some((route, query)) = picture_for(state, ready, (route, query)) else {
        repaint_until_landed(ui);
        return;
    };
    if !ready {
        repaint_until_landed(ui);
    }
    let solo = route == "/author/render/solo.jpg";
    show_preview(ui, theme, door, ingest, host, &route, query, focused, subject);
    if let Some(why) = refused.filter(|_| solo) {
        // The picture is older than the row the panel shows: say so.
        ui.painter_at(host).text(
            host.left_bottom() + egui::vec2(f32::from(theme.pad), -f32::from(theme.pad)),
            egui::Align2::LEFT_BOTTOM,
            format!("preview stale: {why}"),
            theme.font_small.clone(),
            theme.ink,
        );
    }
}

/// `row` cut or padded with zeros to `width` floats, the width the renderer
/// accepts for the model.
fn fit_width(mut row: Vec<f32>, width: usize) -> Vec<f32> {
    if width > 0 {
        row.resize(width, 0.0);
    }
    row
}

/// The picture a frame draws: `want` once its write has landed (and it is
/// then remembered), else the picture last drawn. `None` while the write is
/// out and nothing has been drawn yet.
fn picture_for(
    state: &mut PanelState,
    ready: bool,
    want: (&str, Vec<(String, String)>),
) -> Option<(String, Vec<(String, String)>)> {
    if ready {
        let picture = (want.0.to_string(), want.1);
        state.last_picture = Some(picture.clone());
        Some(picture)
    } else {
        state.last_picture.clone()
    }
}

/// A write is still out: come back soon, so the picture follows it.
fn repaint_until_landed(ui: &Ui) {
    crate::shell::frames::repaint_after(ui.ctx(), std::time::Duration::from_millis(30));
}

/// P-G: the preview bar's subject for the authored sequence — its name
/// (the shown sequence's, else the song's), the export body (named after
/// it) and its device's resolution.
fn sequence_subject(
    door: &dyn Call,
    state: &PanelState,
    ingest: &crate::shell::lanes::IngestLane,
    export: Option<Value>,
) -> crate::shell::preview::PreviewSubject {
    let name = state
        .mv_shown
        .as_ref()
        .and_then(|m| m["name"].as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| ingest.song.trim().to_string());
    let name = if name.is_empty() { ingest.model.clone() } else { name };
    let export = export.map(|mut b| {
        b["name"] = json!(name);
        b
    });
    let size = ingest
        .device
        .as_deref()
        .and_then(|d| crate::shell::preview::output_size(door, d));
    crate::shell::preview::PreviewSubject {
        label: crate::shell::i18n::trf("{name} · sequence", &[("name", &name)]),
        export,
        size,
        fps: None,
    }
}

/// Hands the sequence's picture to the one preview — the nodes canvas's —
/// or, while that canvas is not on screen (the tabbed layout), paints it
/// over the timeline with the play control.
#[allow(clippy::too_many_arguments)]
fn show_preview(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    ingest: &mut crate::shell::lanes::IngestLane,
    host: egui::Rect,
    route: &str,
    query: Vec<(String, String)>,
    focused: bool,
    subject: crate::shell::preview::PreviewSubject,
) {
    let playing = ingest.playing.is_some();
    if crate::shell::preview::canvas_showing(ui.ctx()) {
        crate::shell::preview::publish_sequence(ui.ctx(), route, query, focused, playing, Some(subject));
        return;
    }
    ingest.preview.set_subject(Some(subject));
    let query: Vec<(&str, String)> = query.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    ingest
        .preview
        .show(ui, theme, door, None, host, route, &query);
    if let Some(r) = ingest.preview.rect() {
        if crate::shell::preview::play_control(ui, theme, r, playing) {
            ingest.toggle_play(ui.input(|i| i.time));
        }
    }
}

/// One button in the overlays grid — the whole row `R22.3` decision 2 asked
/// for: `sequences_row`/`switches_row` stop painting and hand their cells
/// back instead, so every control in the panel lands in the same
/// [`egui::Grid`].
#[derive(Clone)]
struct Cell {
    /// What a click reaches, with the ids and targets its builder resolved.
    kind: CellKind,
    /// The key its size is saved under in `grid_spans`: `fixed:<name>` for the
    /// fixed row, `mv:<name>`, `seq:<id>`, `sw:<id>` for the rest.
    key: String,
    /// Size in cells, `(w, h)` — the fixed row stays 1×1. Filled from
    /// [`PanelState::grid_spans`] by [`overlays_grid`] before the grid is laid out.
    span: (f32, f32),
    /// The button's text — the icon `O.28` point 4 distinguishes a kind by
    /// rides here the day one is picked.
    label: String,
    /// Painted `theme.phosphor` — the same "on" a blackout or a switch has.
    on: bool,
    /// Painted with the accent stroke — a movement or a sequence in the air.
    running: bool,
    /// A running movement's playhead, 0..=1 — the bar at the foot.
    progress: Option<f32>,
}

impl Cell {
    /// The `+` that grows the grid: fixed 1×1, no size menu, painted in the
    /// page row beside the next-page button rather than in a content row.
    fn add() -> Cell {
        Cell {
            kind: CellKind::Add,
            key: "fixed:add".into(),
            span: (1.0, 1.0),
            label: "+".into(),
            on: false,
            running: false,
            progress: None,
        }
    }

    /// One page button, turning the page to `to` (clamped by the caller).
    fn page(label: String, to: usize) -> Cell {
        Cell {
            kind: CellKind::PageTo(to),
            key: format!("fixed:page-{to}"),
            span: (1.0, 1.0),
            label,
            on: false,
            running: false,
            progress: None,
        }
    }

    /// The fixed row (`blackout`, `identify`, `+`, `stop all`): always
    /// 1×1 and no size menu.
    fn is_fixed(&self) -> bool {
        matches!(
            self.kind,
            CellKind::Blackout
                | CellKind::Identify
                | CellKind::Add
                | CellKind::StopAll
                | CellKind::PageTo(_)
        )
    }
}

/// What a click on a [`Cell`] does.
#[derive(Clone)]
enum CellKind {
    /// `GET`/`POST /overlay/blackout`; the cell's own `on` is the state.
    Blackout,
    /// M6.3: `POST /venue { op: "identify" }` — flash the venue's fixtures so
    /// the operator (or a camera) can see where each one is. A sensor of the
    /// same pixel map the blackout switch is a control of.
    Identify,
    /// The one `+` that opens the ways to fill the grid — create a sequence
    /// (the clean authoring scratch), author the open graph's `controller
    /// button`, or add a loop. It sits in the page row, one column left of the
    /// next-page button.
    Add,
    /// `POST /overlay {op:"stop-all"}`.
    StopAll,
    /// One of the two page buttons: `‹`/`›` in the grid's last row. Clicking
    /// stores the page it names (`grid_page` in settings, the same value the
    /// `grid-page-next`/`-prev` triggers step).
    PageTo(usize),
    /// `POST /overlay` — `play`/`stop` a movement on `device`/`layer`.
    Movement {
        name: String,
        device: String,
        layer: String,
        looping: bool,
    },
    /// `POST /sequences` — `run`/`stop` a saved sequence on `device`.
    Sequence { id: String, device: String },
    /// `POST /switch` — `on`/`off`, or the hold edges.
    Switch { id: String, kind: String },
    /// G1: a `fixture` button — one saved switch (`kind = "fixture"`) whose
    /// `set` names the `venue.fixtures` node driving the venue fixture `id`.
    /// It fires exactly like a `toggle`/`hold` switch (`POST /switch
    /// {op:on|off}`), and the venue fixture's name is what it paints. G3's
    /// `hold` is the switch's own mode: `true` posts on the press/release edge
    /// rather than a click.
    Fixture { id: String, hold: bool },
    /// G1: a `rig` button — one saved switch (`kind = "rig"`) driving a
    /// `Fixture.group` as one. Same firing as a `toggle`/`hold` switch; `id`
    /// is the saved switch's own id (the group it drives lives in the switch,
    /// not here), `hold` its G3 mode.
    Rig { id: String, hold: bool },
    /// One saved controller (`state.controllers`): the whole stored state as
    /// one **scene** cell. `key` names the controller (its graph id); pressing
    /// the cell posts every target's stored state at once. Its `⚙` and its
    /// right-click open [`controller_editor`].
    Controller { key: String },
    /// A control cell (`surface::Role`): `rec`, `drop rec`, the dials, `fn`,
    /// `load`, `folders`, the piano. Its gestures go to the surface, not the
    /// door.
    Role { role: crate::surface::Role },
    /// A recorded loop (`surface::Loop`): a press plays or stops it.
    Loop,
}

/// The overlays (performance) state: one grid, `O.28` point 4's *"it is a
/// button grid"* — every control and every saved button in the same
/// [`egui::Grid`], distinguished only by what a click does and by the two
/// states the old loose rows each painted themselves (a `phosphor` fill for
/// on, the accent stroke for running, a movement's progress bar at the
/// foot). The fixed row leads — `blackout`, `+ create sequence`, `stop
/// all` — then, in that order and in the same grid, the movement library,
/// the saved sequences (`/sequences`) and the switches (`/switch`).
/// Nothing here writes to the library — starring stays the operator's
/// confirmation, the save route forces `favourite: false` on a posted
/// movement and this panel never posts a whole library.
///
/// The per-button target (`device`/`layer`/`loop`) is resolved the way the
/// old combos did — first output, first layer, `mv.looping` — until a later
/// phase reads it from the sequence's own trigger.
fn overlays_grid(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    state_val: &Value,
    open_graph: Option<&str>,
) {
    let GridPlan { cells, pins, placed, pages, page, wired, filled, slot_of } = grid_plan(door, state, state_val);
    let (cols, rows) = (state.layout.cols as usize, state.layout.rows as usize);
    state.driven_pass = Some(ui.ctx().cumulative_pass_nr());
    // What a face's control can be given from the grid's own buttons.
    state.face_pool = cells
        .iter()
        .filter(|c| !c.is_fixed() && !crate::surface::is_face_cell(&c.key) && !matches!(c.kind, CellKind::Role { .. }))
        .map(|c| (c.key.clone(), c.label.replace('\n', " ")))
        .collect();

    // The hardware first: what the controller sent since the last frame, and
    // the loops' playback, fire before this frame paints what they changed.
    drive_surface(ui, door, state, &cells, page, pages, &wired);
    if let Some(drag) = super::pads::take_loaded(ui.ctx()) {
        apply_overlay_drop(door, state, &drag);
    }

    // The cells stretch to the area: the columns share the width, the rows
    // the height above the page bar, never taller than wide.
    let origin = ui.cursor().min;
    let gap = theme.item_gap;
    let width = ui.available_width();
    let visible = (ui.clip_rect().bottom() - origin.y).max(0.0);
    let bar_h = theme.pager_height.max(ui.spacing().interact_size.y);
    let cell_w = ((width - gap.x * (cols as f32 - 1.0)) / cols as f32).max(theme.gear_size);
    let cell_h = ((visible - bar_h - gap.y * rows as f32) / rows as f32)
        .clamp(theme.gear_size, cell_w.max(theme.gear_size));
    let cell = Vec2::new(cell_w, cell_h);
    let grid_h = rows as f32 * cell_h + (rows as f32 - 1.0) * gap.y;

    let block = Rect::from_min_size(origin, Vec2::new(width, grid_h + gap.y + bar_h));
    ui.scope_builder(UiBuilder::new().max_rect(block), |ui| {
        ui.allocate_exact_size(block.size(), Sense::hover());
        let (cu, ru) = (cols * crate::surface::UNITS, rows * crate::surface::UNITS);
        let mut occupied = vec![false; cu * ru];
        for (((c, p), pin), slot) in cells.iter().zip(&placed).zip(&pins).zip(&slot_of) {
            if p.page != page && *pin != crate::surface::Pin::Every {
                continue;
            }
            for y in p.y..p.y + p.h {
                for x in p.x..p.x + p.w {
                    occupied[y * cu + x] = true;
                }
            }
            let rect = unit_rect(origin, cell, gap, p.x, p.y, p.w, p.h);
            paint_cell(ui, theme, door, state, c, rect, open_graph, *slot);
        }
        // A box's face: its empty slots paint as the control they are (a pad
        // still takes a dropped card), and its mark where the panel has it.
        if let Some(device) = state.layout.device.as_deref().and_then(metal_midi::devices::device) {
            for (i, slot) in state.layout.slots.clone().iter().enumerate() {
                if !filled.get(i).copied().unwrap_or(false) {
                    let [x, y, w, h] = slot.rect.map(|v| v as usize);
                    paint_empty_slot(ui, theme, door, state, i, slot, unit_rect(origin, cell, gap, x, y, w, h), page, open_graph);
                }
            }
            if let Some(slot) = state.face_learn.and_then(|i| state.layout.slots.get(i)) {
                let [x, y, w, h] = slot.rect.map(|v| v as usize);
                let r = unit_rect(origin, cell, gap, x, y, w, h);
                ui.painter().rect_stroke(r, theme.corner_radius(), theme.card_picked_stroke(), egui::StrokeKind::Outside);
            }
            for logo in device.controls.iter().filter(|c| c.part == metal_midi::devices::Part::Logo) {
                let [x, y, w, h] = logo.rect.map(|v| v as usize);
                ui.painter().text(
                    unit_rect(origin, cell, gap, x, y, w, h).center(),
                    Align2::CENTER_CENTER,
                    &logo.label,
                    theme.font_small.clone(),
                    theme.ink_weak,
                );
            }
        }
        // A whole cell nothing covers is a phantom: an empty square that
        // takes a dropped library card.
        let u = crate::surface::UNITS;
        let free_grid = state.layout.device.is_none();
        for row in (0..rows).filter(|_| free_grid) {
            for col in 0..cols {
                let free = (row * u..(row + 1) * u).all(|y| (col * u..(col + 1) * u).all(|x| !occupied[y * cu + x]));
                if free {
                    let rect = unit_rect(origin, cell, gap, col * u, row * u, u, u);
                    let _ = paint_phantom_cell(ui, theme, door, state, row * cols + col, rect);
                }
            }
        }
        let bar = Rect::from_min_size(
            Pos2::new(origin.x, origin.y + grid_h + gap.y),
            Vec2::new(width, bar_h),
        );
        page_bar(ui, theme, door, state, bar, page, pages, open_graph);
    });

    // The gestures this frame's paint raised, now that every cell is known.
    let outs = std::mem::take(&mut state.grid_outs);
    run_outs(ui, door, state, &cells, outs, page, pages);

    // The `video`-drop destination popover, if one is open. It needs no
    // anchor of its own: it is a small window over the grid, and the output
    // (or mask) combo it holds is the same `GET /state` listing the grid
    // already resolved.
    create_video_popover(ui, theme, door, state, state_val, open_graph);
    // The layouts menu's name and delete popups.
    layout_popups(ui, theme, door, state);
    // A scene cell's `⚙` or right-click opens the one controller editor as a
    // popup anchored near that cell.
    controller_editor_popup(ui, theme, door, state);
    // The edit mode's inspector for the selected cell.
    cell_editor_popup(ui, theme, door, state, &cells, page, pages);
}

/// The grid as laid out this frame: the cells in order, where each may land
/// and where it did, how many pages there are and which one is shown.
struct GridPlan {
    cells: Vec<Cell>,
    pins: Vec<crate::surface::Pin>,
    placed: Vec<crate::surface::Placed>,
    pages: usize,
    page: usize,
    /// On a box's face: each slot input on the shown page, the cell in
    /// that slot (which the control fires unless a binding says otherwise)
    /// and how the control reports a turn.
    wired: Vec<(String, String, crate::surface::DialMode)>,
    /// Which of the face's slots hold a cell on the shown page.
    filled: Vec<bool>,
    /// The face's slot each cell sits in.
    slot_of: Vec<Option<usize>>,
}

fn grid_plan(door: &dyn Call, state: &PanelState, state_val: &Value) -> GridPlan {
    let cells = grid_cells(door, state, state_val);

    // Fixed columns × rows (`grid_surface`), in sub-cells so the halves and
    // thirds pack beside whole cells. Pinned cells land on their page first;
    // the rest flow in the stored order.
    let (cols, rows) = (state.layout.cols as usize, state.layout.rows as usize);
    let spans: Vec<(usize, usize)> = cells
        .iter()
        .map(|c| (crate::surface::units(c.span.0), crate::surface::units(c.span.1)))
        .collect();
    let (pins, placed, slot_of) = if state.layout.device.is_some() {
        // A box's face: its controls where the panel has them, the grid's
        // own buttons in its pads (a fader in its knobs and faders).
        use crate::surface::Pin;
        let places: Vec<Option<[u16; 4]>> = cells.iter().map(|c| state.layout.places.get(&c.key).copied()).collect();
        let value: Vec<bool> = cells
            .iter()
            .map(|c| matches!(&c.kind, CellKind::Controller { key } if controller_is_fader(state, key)))
            .collect();
        let pins: Vec<Pin> = cells
            .iter()
            .zip(&places)
            .map(|(c, at)| match (at, state.layout.pages.get(&c.key)) {
                (_, Some(&p)) => Pin::Page(p),
                (Some(_), None) => Pin::Every,
                (None, None) => Pin::Flow,
            })
            .collect();
        let (mut placed, mut slot_of): (Vec<_>, Vec<_>) =
            crate::surface::pack_device(&places, &value, &pins, &state.layout.slots).into_iter().unzip();
        // A face shows only what the operator gave its controls: the grid's
        // own buttons wait in each control's menu ([`slot_menu`]).
        for (i, at) in places.iter().enumerate() {
            if at.is_none() {
                placed[i].page = usize::MAX;
                slot_of[i] = None;
            }
        }
        (pins, placed, slot_of)
    } else {
        // Control cells hold one place on every page, like the hardware
        // buttons they stand for — a `fn` that turns the page is still there
        // to release.
        let pins: Vec<crate::surface::Pin> = cells
            .iter()
            .map(|c| match (&c.kind, state.layout.pages.get(&c.key)) {
                (CellKind::Role { .. }, _) => crate::surface::Pin::Every,
                (_, Some(&p)) => crate::surface::Pin::Page(p),
                _ => crate::surface::Pin::Flow,
            })
            .collect();
        let placed = crate::surface::pack(&spans, &pins, cols * crate::surface::UNITS, rows * crate::surface::UNITS);
        let none = vec![None; placed.len()];
        (pins, placed, none)
    };
    let pages = placed
        .iter()
        .filter(|p| p.page != usize::MAX)
        .map(|p| p.page + 1)
        .max()
        .unwrap_or(1)
        .max(state.layout.page_count)
        .max(1);
    // The stored page, clamped to the pages this layout holds: a removed
    // button or a trigger stepping past an end cannot strand it. The clamped
    // value is written back so the stored and shown page agree.
    let stored = stored_page(door);
    let page = stored.clamp(0, pages as i64 - 1) as usize;
    if stored != page as i64 {
        set_stored_page(door, page);
    }

    let mut filled = vec![false; state.layout.slots.len()];
    let mut wired = Vec::new();
    for (((c, p), pin), s) in cells.iter().zip(&placed).zip(&pins).zip(&slot_of) {
        let Some(s) = *s else { continue };
        if p.page != page && *pin != crate::surface::Pin::Every {
            continue;
        }
        filled[s] = true;
        let slot = &state.layout.slots[s];
        if let Some(input) = &slot.input {
            wired.push((input.clone(), c.key.clone(), slot.dial));
        }
    }
    GridPlan { cells, pins, placed, pages, page, wired, filled, slot_of }
}

/// MIDI keeps driving the grid while it is off screen — the sequence editor
/// open, or a layout without the lanes region. The shell calls this once a
/// frame after the panels painted; it does nothing when the grid itself was
/// painted (and so already drove), or when nothing is waiting: no control
/// heard, no loop playing, no take recording, no library pick to load.
pub(crate) fn drive_offscreen(ui: &Ui, door: &dyn Call, state: &mut PanelState) {
    if state.driven_pass == Some(ui.ctx().cumulative_pass_nr()) {
        return;
    }
    let busy = crate::input::surface_pending()
        || !state.pad_inputs.is_empty()
        || state.surface.is_recording()
        || state.controllers_dirty.is_some()
        || state.layout.loops.values().any(|l| l.playing)
        || super::pads::has_loaded(ui.ctx());
    if !busy {
        return;
    }
    state.refresh(door);
    let state_val = get(door, "GET", "/state", &[]).unwrap_or_else(|| json!({}));
    let plan = grid_plan(door, state, &state_val);
    drive_surface(ui, door, state, &plan.cells, plan.page, plan.pages, &plan.wired);
    if let Some(drag) = super::pads::take_loaded(ui.ctx()) {
        apply_overlay_drop(door, state, &drag);
    }
}

/// Every cell the grid holds, in the stored order, sized: the movement
/// library, the saved sequences, the switches, the controllers, then the
/// control cells and the recorded loops.
fn grid_cells(door: &dyn Call, state: &PanelState, state_val: &Value) -> Vec<Cell> {
    // The targets, off `/state` (rule 5): outputs, and the routed layers of
    // the first one. The `play` route re-resolves the layer itself and
    // refuses a bad name.
    let outputs = state_val["outputs"].as_array().cloned().unwrap_or_default();
    let device = state
        .mv
        .device
        .clone()
        .filter(|d| outputs.iter().any(|o| o["id"].as_str() == Some(d.as_str())))
        .or_else(|| outputs.first()?["id"].as_str().map(str::to_string));
    let layers = outputs
        .iter()
        .find(|o| o["id"].as_str() == Some(device.as_deref().unwrap_or_default()))
        .and_then(|o| o["layers"].as_array().cloned())
        .unwrap_or_default();
    let layer = state
        .mv
        .layer
        .clone()
        .filter(|l| {
            layers
                .iter()
                .any(|ly| ly["id"].as_str() == Some(l.as_str()))
        })
        .or_else(|| layers.first()?["id"].as_str().map(str::to_string));

    // No built-in cells: blackout, identify, stop all, the HSB faders, the
    // palettes and RGB are all buttons made through the creation pipeline now.
    let mut cells: Vec<Cell> = Vec::new();
    cells.extend(movement_cells(door, state_val, device.as_deref(), layer.as_deref()));
    cells.extend(sequences_cells(door, device.as_deref()));
    cells.extend(switches_cells(door));
    cells.extend(controller_cells(state));
    cells.extend(surface_cells(state));

    for cell in &mut cells {
        cell.span = if cell.is_fixed() {
            (1.0, 1.0)
        } else {
            state.grid_spans.get(&cell.key).copied().unwrap_or(cell.span)
        };
    }
    let keys: Vec<String> = cells.iter().map(|c| c.key.clone()).collect();
    let mut slots: Vec<Option<Cell>> = cells.into_iter().map(Some).collect();
    state
        .layout
        .ordered(&keys)
        .into_iter()
        .filter_map(|i| slots[i].take())
        .collect()
}

impl PanelState {
    /// The overlays grid's cells as `(key, label)`, for the chat's `@` completion.
    #[cfg(test)]
    pub(crate) fn set_layout_for_test(&mut self, layout: crate::surface::Layout) {
        self.layout = layout;
    }

    pub(crate) fn mention_cells(&self) -> Vec<(String, String)> {
        surface_cells(self).into_iter().map(|c| (c.key, c.label)).collect()
    }
}

/// The control cells and the recorded loops, off the stored layout.
fn surface_cells(state: &PanelState) -> Vec<Cell> {
    let roles = state.layout.roles.iter().map(|(key, role)| Cell {
        kind: CellKind::Role { role: role.clone() },
        key: key.clone(),
        span: role.default_span(),
        // A control says its job; a box's control kept from before the
        // faces were blank says nothing.
        label: if role.is_spare() { String::new() } else { tr_dyn(&role.label()).to_string() },
        on: false,
        running: false,
        progress: None,
    });
    let loops = state.layout.loops.iter().map(|(key, lp)| Cell {
        kind: CellKind::Loop,
        key: key.clone(),
        span: (1.0, 1.0),
        label: trf("{name} · {n} ticks", &[("name", &lp.name), ("n", &lp.ticks)]),
        on: lp.playing,
        running: lp.playing,
        progress: state.surface.loop_phase(key, lp),
    });
    roles.chain(loops).collect()
}

/// The rect of a block of sub-cells: whole cells plus the gaps between them,
/// so two halves side by side keep one gap between them.
fn unit_rect(origin: Pos2, cell: Vec2, gap: Vec2, x: usize, y: usize, w: usize, h: usize) -> Rect {
    let u = crate::surface::UNITS as f32;
    let step = cell + gap;
    Rect::from_min_size(
        origin + Vec2::new(x as f32 / u * step.x, y as f32 / u * step.y),
        Vec2::new(w as f32 / u * step.x - gap.x, h as f32 / u * step.y - gap.y),
    )
}

/// The row under the grid: `‹ n/m` flush left, then the edit toggle and the
/// surface's status (MIDI ports, recording, frozen buttons, a pending bind),
/// the grid's columns × rows and page count in edit mode, and `+` beside
/// `n/m ›` flush right.
#[allow(clippy::too_many_arguments)]
fn page_bar(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    bar: Rect,
    page: usize,
    pages: usize,
    open_graph: Option<&str>,
) {
    let side = bar.height().max(theme.overlay_cell.x.min(bar.width() / 4.0));
    let prev = Rect::from_min_size(bar.min, Vec2::new(side, bar.height()));
    let next = Rect::from_min_size(Pos2::new(bar.right() - side, bar.top()), Vec2::new(side, bar.height()));
    let add = Rect::from_min_size(
        Pos2::new(next.left() - theme.item_gap.x - bar.height(), bar.top()),
        Vec2::splat(bar.height()),
    );
    for (rect, cell) in [
        (prev, Cell::page(format!("‹ {}/{pages}", page + 1), page.saturating_sub(1))),
        (add, Cell::add()),
        (next, Cell::page(format!("{}/{pages} ›", page + 1), (page + 1).min(pages - 1))),
    ] {
        paint_cell(ui, theme, door, state, &cell, rect, open_graph, None);
    }
    // `+` names what it does: the same items as the free grid's add menu.
    ui.interact(add, Id::new("shell/region/lanes/add-tip"), Sense::hover()).on_hover_text(tr("Add new control"));
    let middle = Rect::from_min_max(
        Pos2::new(prev.right() + theme.item_gap.x, bar.top()),
        Pos2::new(add.left() - theme.item_gap.x, bar.bottom()),
    );
    ui.scope_builder(UiBuilder::new().max_rect(middle).layout(Layout::left_to_right(Align::Center)), |ui| {
        // What does not fit is cut, never laid over `+` and `›`.
        ui.set_clip_rect(middle.intersect(ui.clip_rect()));
        layouts_menu(ui, theme, door, state);
        // Walks a box's controls from the top (one at a time, `skip` or stop).
        if state.face_learn.is_none() && state.layout.device.is_some() {
            if ui.button(RichText::new(tr("learn the face")).font(theme.font_small.clone())).clicked() {
                state.face_learn = Some(0);
                state.face_learn_last = None;
                state.slot_learn = None;
            }
        }
        if state.face_learn.is_some() {
            face_learn_row(ui, theme, door, state);
        }
        let edit = ui.selectable_label(state.grid_edit, RichText::new(tr("edit grid")).font(theme.font_small.clone()));
        if edit.clicked() {
            state.grid_edit = !state.grid_edit;
            state.grid_learn = None;
            if !state.grid_edit {
                state.grid_selected = None;
            }
        }
        if state.grid_edit && state.face_learn.is_none() {
            grid_dims_row(ui, theme, door, state, pages);
        }
        surface_status(ui, theme, state);
    });
}

/// While `learn the face` walks the box's controls (started from the
/// controller menu; the one waiting is outlined on the face): `skip` leaves
/// a control as it was, `stop learning` ends the walk.
fn face_learn_row(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    let Some(i) = state.face_learn else { return };
    let small = |t: String| RichText::new(t).font(theme.font_small.clone());
    if ui.button(small(tr("skip").into())).clicked() {
        face_learn_next(door, state, i);
    }
    if ui.button(small(tr("stop learning").into())).clicked() {
        state.face_learn = None;
        save_layout(door, state);
    }
}

/// The walk moves past step `i`: to the next control, the piano, or done.
fn face_learn_next(door: &dyn Call, state: &mut PanelState, i: usize) {
    let piano = state.layout.roles.values().any(|r| matches!(r, crate::surface::Role::Piano { .. }));
    let last = state.layout.slots.len() + usize::from(piano);
    if i + 1 >= last {
        state.face_learn = None;
        state.grid_note = Some(tr("the face is learned").to_string());
        save_layout(door, state);
    } else {
        state.face_learn = Some(i + 1);
    }
}

/// What the walk asks for now, for the status row.
fn face_learn_prompt(state: &PanelState) -> Option<String> {
    let i = state.face_learn?;
    let n = state.layout.slots.len();
    Some(match state.layout.slots.get(i) {
        Some(s) => {
            let name = if s.label.is_empty() { s.id.clone() } else { s.label.clone() };
            trf("{n}/{m}: touch {name} on the controller", &[("n", &(i + 1)), ("m", &n), ("name", &name)])
        }
        None => tr("now press the piano's lowest key").to_string(),
    })
}

/// The grid's layouts: the one up names the button, each other brings
/// itself up. `create a new layout` opens a free grid, `create a new layout
/// from hardware ▸` a known controller's face as its template (the one
/// plugged in is marked; hovering one says what mode the box must be in);
/// either asks for its name. `rename` and `delete` act on the one up.
fn layouts_menu(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    let small = |t: String| RichText::new(t).font(theme.font_small.clone());
    let label = state.layouts.current().map_or_else(|| layout_default_name(1), |s| s.name.clone());
    let ports = crate::input::midi_ports();
    let (response, _) = plain_menu(ui, theme, RichText::new(format!("{label} ▾")), |ui| {
        for saved in state.layouts.list.clone() {
            let up = saved.id == state.layouts.active;
            let item = ui.selectable_label(up, small(saved.name.clone()));
            let item = match saved.surface.device.as_deref().and_then(metal_midi::devices::device) {
                Some(d) => item.on_hover_text(tr_dyn(d.name)),
                None => item.on_hover_text(tr("free grid")),
            };
            if item.clicked() {
                if !up {
                    switch_layout(door, state, &saved.id);
                    state.grid_note = Some(trf("{name} is up", &[("name", &saved.name)]));
                }
                ui.close();
            }
        }
        ui.separator();
        if plain_item(ui, theme, small(tr("Create a new layout").into())).clicked() {
            create_layout(door, state, crate::surface::Layout::default());
            ui.close();
        }
        let _ = plain_menu(ui, theme, small(tr("Create a new layout from hardware").into()), |ui| {
            let mut maker = "";
            for d in metal_midi::devices::catalogue() {
                // A heading per maker, the generic layouts last.
                if d.maker != maker {
                    if !maker.is_empty() {
                        ui.separator();
                    }
                    maker = d.maker;
                    ui.label(RichText::new(tr_dyn(maker)).font(theme.font_small.clone()).color(theme.ink_weak));
                }
                let shown = tr_dyn(d.name).to_string();
                let name = if ports.iter().any(|p| d.is_port(p)) {
                    trf("{name} · connected", &[("name", &shown)])
                } else {
                    shown
                };
                if ui.button(small(name)).on_hover_text(tr_dyn(d.note)).clicked() {
                    let mut face = crate::surface::Layout::default();
                    face.apply_device(&d);
                    create_layout(door, state, face);
                    ui.close();
                }
            }
        });
        ui.separator();
        if plain_item(ui, theme, small(tr("Rename").into())).clicked() {
            if let Some(cur) = state.layouts.current() {
                state.layout_name =
                    Some(LayoutName { id: cur.id.clone(), text: cur.name.clone(), fresh: false, focus: true });
            }
            ui.close();
        }
        let last = state.layouts.list.len() < 2;
        let delete = ui
            .add_enabled(!last, egui::Button::new(small(tr("Delete").into())).frame(true))
            .on_disabled_hover_text(tr("the last layout stays"));
        if delete.clicked() {
            state.layout_delete = Some(state.layouts.active.clone());
            ui.close();
        }
    });
    response.on_hover_text(tr("the grid's layouts: each a setup of its own"));
}

/// `Custom Grid {n}`: what a new layout is called until it is named.
fn layout_default_name(n: usize) -> String {
    trf("Custom Grid {n}", &[("n", &n)])
}

/// Copies the layout up (its surface and its face's controllers) into its
/// entry, so the list holds every layout whole.
fn stash_layout(state: &mut PanelState) {
    let face: BTreeMap<String, Value> =
        state.controllers.iter().filter(|(k, _)| is_hw_controller(k)).map(|(k, v)| (k.clone(), v.clone())).collect();
    let layout = state.layout.clone();
    let active = state.layouts.active.clone();
    if let Some(saved) = state.layouts.get_mut(&active) {
        saved.surface = layout;
        saved.controllers = face;
    }
}

/// Brings up `id` in place of the layout up (which is stashed first unless
/// it is gone): its surface, its face's controllers, a fresh live half.
fn bring_up_layout(state: &mut PanelState, id: &str) {
    let Some(saved) = state.layouts.get(id).cloned() else { return };
    state.controllers.retain(|k, _| !is_hw_controller(k));
    state.controllers.extend(saved.controllers);
    state.layout = saved.surface;
    state.layouts.active = saved.id;
    state.surface = crate::surface::Surface::default();
    state.grid_selected = None;
    state.grid_learn = None;
    state.slot_learn = None;
    state.face_learn = None;
    state.face_learn_last = None;
}

fn switch_layout(door: &dyn Call, state: &mut PanelState, id: &str) {
    stash_layout(state);
    bring_up_layout(state, id);
    save_layouts(door, state);
}

/// Adds `surface` as a new layout, brings it up and asks for its name.
fn create_layout(door: &dyn Call, state: &mut PanelState, surface: crate::surface::Layout) {
    stash_layout(state);
    let id = state.layouts.create("", surface, layout_default_name);
    bring_up_layout(state, &id);
    save_layouts(door, state);
    let name = state.layouts.current().map(|s| s.name.clone()).unwrap_or_default();
    state.grid_note = Some(trf("{name} is up", &[("name", &name)]));
    state.layout_name = Some(LayoutName { id, text: name, fresh: true, focus: true });
}

/// Posts the layout up, its controllers and the whole list in one
/// `POST /settings`, adopting what the reply carries.
fn save_layouts(door: &dyn Call, state: &mut PanelState) {
    stash_layout(state);
    let body = json!({
        "grid_surface": state.layout.to_value(),
        "grid_controllers": Value::Object(state.controllers.clone().into_iter().collect()),
        "grid_layouts": state.layouts.to_value(),
    });
    match door.json("POST", "/settings", Some(body)) {
        Ok(reply) => {
            if reply["grid_surface"].is_object() {
                state.layout = crate::surface::Layout::from_value(&reply["grid_surface"]);
            }
            if reply["grid_controllers"].is_object() {
                state.controllers = parse_controllers(&reply["grid_controllers"]);
            }
            if reply["grid_layouts"].is_object() {
                let layouts = crate::surface::Layouts::from_value(&reply["grid_layouts"]);
                if !layouts.list.is_empty() {
                    state.layouts = layouts;
                }
            }
        }
        Err((_, why)) => state.grid_note = Some(trf("the layouts were not saved: {why}", &[("why", &why)])),
    }
}

/// The name popup (a new layout's, or `rename`'s) and the delete popup,
/// small windows over the grid.
fn layout_popups(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    let small = |t: String| RichText::new(t).font(theme.font_small.clone());
    if let Some(draft) = state.layout_name.as_mut() {
        let title = if draft.fresh { tr("Name the layout") } else { tr("Rename layout") };
        let (mut save, mut cancel, mut open) = (false, false, true);
        egui::Window::new(title)
            .id(Id::new("shell/region/lanes/layout-name"))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.set_min_width(theme.text_input_width * 1.5);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut draft.text)
                        .hint_text(tr("layout name"))
                        .desired_width(theme.text_input_width * 1.5),
                );
                if draft.focus {
                    field.request_focus();
                    draft.focus = false;
                }
                let named = !draft.text.trim().is_empty();
                save |= named && field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    save |= ui.add_enabled(named, egui::Button::new(small(tr("save").into()))).clicked();
                    cancel |= ui.button(small(tr("cancel").into())).clicked();
                });
            });
        if save {
            let (id, text) = (draft.id.clone(), draft.text.clone());
            if state.layouts.rename(&id, &text) {
                save_layouts(door, state);
            }
        }
        if save || cancel || !open {
            state.layout_name = None;
        }
    }
    if let Some(id) = state.layout_delete.clone() {
        let name = state.layouts.get(&id).map(|s| s.name.clone()).unwrap_or_default();
        let (mut delete, mut cancel, mut open) = (false, false, true);
        egui::Window::new(tr("Delete layout"))
            .id(Id::new("shell/region/lanes/layout-delete"))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.label(small(trf("Delete {name}? Its bindings and loops go with it.", &[("name", &name)])));
                ui.horizontal(|ui| {
                    delete |= ui.button(small(tr("delete").into())).clicked();
                    cancel |= ui.button(small(tr("cancel").into())).clicked();
                });
            });
        if delete && state.layouts.list.len() > 1 {
            // The one up goes with its face's controllers: its neighbour
            // comes up in their place.
            if let Some(next) = state.layouts.remove(&id) {
                bring_up_layout(state, &next);
            }
            save_layouts(door, state);
            state.grid_note = Some(trf("{name} was deleted", &[("name", &name)]));
        }
        if delete || cancel || !open {
            state.layout_delete = None;
        }
    }
}

/// `does ▸`: the surface's jobs (`fn dial`, `rec`, `page ›`, …) for a box's
/// slot, on every page — what it held goes. Answers whether one was picked.
fn job_menu(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState, slot: usize) -> bool {
    let small = |t: String| RichText::new(t).font(theme.font_small.clone());
    let Some(id) = state.layout.slots.get(slot).map(|s| s.id.clone()) else { return false };
    let mut picked = false;
    let _ = plain_menu(ui, theme, small(tr("configure general controls").into()), |ui| {
        for job in crate::surface::Role::catalogue().into_iter().filter(|r| !matches!(r, crate::surface::Role::Piano { .. })) {
            if ui.button(small(tr_dyn(&job.label()).to_string())).clicked() {
                let key = format!("hw:{id}");
                let gone = state.layout.place(&key, slot, None);
                drop_controllers(door, state, &gone);
                state.layout.roles.insert(key, job);
                save_layout(door, state);
                picked = true;
                ui.close();
            }
        }
    });
    picked
}

/// The box's slot a control's controller (`hw:<id>:<page>`) sits in.
fn slot_of_controller(state: &PanelState, key: &str) -> Option<usize> {
    let (id, _) = key.strip_prefix("hw:")?.rsplit_once(':')?;
    state.layout.slots.iter().position(|s| s.id == id)
}

/// A box's slot with no cell in it on this page, painted unnamed as the
/// control it is: a pad or button (that takes a dropped card, like a free
/// grid's empty cell) lit while pressed, a knob turned and a fader or strip
/// filled to where the hardware sits. Its right-click (any click in edit
/// mode) gives it a cell ([`slot_menu`]).
#[allow(clippy::too_many_arguments)]
fn paint_empty_slot(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    index: usize,
    slot: &crate::surface::Slot,
    rect: Rect,
    page: usize,
    open_graph: Option<&str>,
) {
    use crate::surface::SlotKind;
    let input = slot.input.as_deref().unwrap_or_default();
    let down = !input.is_empty() && state.hw_down.iter().any(|h| metal_midi::same_control(input, h));
    let level = hw_level(state, input);
    let response = match slot.kind {
        SlotKind::Pad | SlotKind::Button => {
            let response = paint_phantom_cell(ui, theme, door, state, index, rect);
            if down {
                ui.painter().rect_filled(rect, theme.corner_radius(), theme.phosphor);
            }
            response
        }
        SlotKind::Knob => {
            let id = Id::new(("shell/region/lanes/slot", index));
            let mut acc = 0.0;
            let value = level.map(|v| format!("{:.0}", v * 127.0)).unwrap_or_default();
            super::pads::dial(ui, theme, id, rect, "", &value, level, false, &mut acc).0
        }
        SlotKind::Fader | SlotKind::Strip => {
            super::pads::strip(ui, theme, rect, "", level);
            ui.interact(rect, Id::new(("shell/region/lanes/slot", index)), Sense::click())
        }
    };
    let response = if slot.label.is_empty() { response } else { response.on_hover_text(&slot.label) };
    slot_popup(theme, door, state, &response, index, page, open_graph);
}

/// Opens [`slot_menu`] off a slot's right-click, or any click in edit mode.
#[allow(clippy::too_many_arguments)]
fn slot_popup(
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    response: &Response,
    slot: usize,
    page: usize,
    open_graph: Option<&str>,
) {
    let popup = if state.grid_edit { Popup::menu(response) } else { Popup::context_menu(response) };
    popup.show(|ui| slot_menu(ui, theme, door, state, slot, page, open_graph));
}

/// What a box's control can hold: a button or fader of the open graph's
/// nodes, params and colours (its own, on this page — the same editor the
/// free grid's `controller button` opens), any of the surface's control
/// jobs (on every page), or nothing. `learn` takes the next control heard
/// as what this one sends.
fn slot_menu(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState, slot: usize, page: usize, open_graph: Option<&str>) {
    let small = |t: String| RichText::new(t).font(theme.font_small.clone());
    let Some(id) = state.layout.slots.get(slot).map(|s| s.id.clone()) else { return };
    let held = state.layout.in_slot(slot);
    match open_graph {
        Some(gid) => {
            if ui.button(small(tr("configure nodes control").into())).clicked() {
                let key = slot_controller(&id, page);
                if !state.controllers.contains_key(&key) {
                    state.controllers.insert(key.clone(), json!({ "graph": gid, "name": "", "mode": "switch", "targets": [] }));
                    save_controllers(door, state);
                }
                let gone = state.layout.place(&format!("ctrl:{key}"), slot, Some(page));
                drop_controllers(door, state, &gone);
                save_layout(door, state);
                state.editing_controller = Some(key);
                state.controller_edit_pos = ui.input(|i| i.pointer.interact_pos());
                ui.close();
            }
        }
        None => {
            ui.label(small(tr("— open a graph to control its nodes —").into()).color(theme.ink_weak));
        }
    }
    let pool = state.face_pool.clone();
    if !pool.is_empty() {
        let _ = plain_menu(ui, theme, small(tr("import button").into()), |ui| {
            ScrollArea::vertical().max_height(theme.text_input_width * 2.0).show(ui, |ui| {
                for (key, label) in &pool {
                    if ui.button(small(label.clone())).clicked() {
                        let gone = state.layout.place(key, slot, Some(page));
                        drop_controllers(door, state, &gone);
                        save_layout(door, state);
                        ui.close();
                    }
                }
            });
        });
    }
    job_menu(ui, theme, door, state, slot);
    let learning = state.slot_learn == Some(slot);
    if ui.selectable_label(learning, small(tr("learn input signal").into())).clicked() {
        state.slot_learn = if learning { None } else { Some(slot) };
        ui.close();
    }
    if held.iter().any(|(_, on)| on.is_none() || *on == Some(page)) && ui.button(small(tr("remove").into())).clicked() {
        let gone = state.layout.clear_slot(slot, Some(page));
        drop_controllers(door, state, &gone);
        save_layout(door, state);
        ui.close();
    }
}

/// Where the box's control on `input` last sat, 0..=1.
fn hw_level(state: &PanelState, input: &str) -> Option<f32> {
    if input.is_empty() {
        return None;
    }
    state
        .hw_levels
        .iter()
        .find(|(heard, _)| metal_midi::same_control(input, heard))
        .map(|(_, v)| *v)
}

/// Edit mode's grid-wide controls: columns, rows and the pages kept.
fn grid_dims_row(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState, pages: usize) {
    let mut changed = false;
    let small = |t: &str| RichText::new(t.to_string()).font(theme.font_small.clone());
    let stepper = |ui: &mut Ui, label: String, value: usize, lo: usize, hi: usize| -> Option<usize> {
        let mut out = None;
        if ui.add_enabled(value > lo, egui::Button::new(small("−"))).clicked() {
            out = Some(value - 1);
        }
        ui.label(small(&label));
        if ui.add_enabled(value < hi, egui::Button::new(small("+"))).clicked() {
            out = Some(value + 1);
        }
        out
    };
    let (lo, hi) = (crate::surface::MIN_DIM as usize, crate::surface::MAX_DIM as usize);
    if let Some(v) = stepper(ui, trf("{n} cols", &[("n", &state.layout.cols)]), state.layout.cols as usize, lo, hi) {
        state.layout.cols = v as u8;
        changed = true;
    }
    if let Some(v) = stepper(ui, trf("{n} rows", &[("n", &state.layout.rows)]), state.layout.rows as usize, lo, hi) {
        state.layout.rows = v as u8;
        changed = true;
    }
    if let Some(v) = stepper(ui, trf("{n} pages", &[("n", &pages)]), pages, 1, 64) {
        state.layout.page_count = v;
        changed = true;
    }
    if changed {
        save_layout(door, state);
    }
}

/// What the surface is doing, in a word each: the MIDI ports open, the take
/// being recorded, how many buttons are frozen, a bind waiting for a control.
fn surface_status(ui: &mut Ui, theme: &Theme, state: &PanelState) {
    let small = |t: String, c| RichText::new(t).font(theme.font_small.clone()).color(c);
    let ports = crate::input::midi_ports();
    let midi = if !crate::input::midi_built() {
        tr("midi: not in this build").to_string()
    } else if ports.is_empty() {
        tr("midi: no controller").to_string()
    } else {
        let names: Vec<String> = ports
            .iter()
            .map(|p| metal_midi::profile(p).map(str::to_string).unwrap_or_else(|| p.clone()))
            .collect();
        trf("midi: {ports}", &[("ports", &names.join(", "))])
    };
    ui.label(small(midi, theme.ink_weak));
    // While mapping: exactly what the box last sent, and on which port.
    if state.grid_edit || state.face_learn.is_some() {
        if let Some(heard) = &state.last_heard {
            ui.label(small(trf("heard {what}", &[("what", heard)]), theme.ink_weak));
        }
    }
    if state.surface.is_recording() {
        ui.label(small(tr("● rec").to_string(), theme.phosphor));
    }
    let frozen = state.surface.frozen_count();
    if frozen > 0 {
        ui.label(small(trf("{n} frozen", &[("n", &frozen)]), theme.phosphor));
    }
    if let Some(prompt) = face_learn_prompt(state) {
        ui.label(small(prompt, theme.phosphor));
    } else if state.grid_learn.is_some() || state.slot_learn.is_some() {
        ui.label(small(tr("press a key, pad or knob…").to_string(), theme.phosphor));
    } else if let Some(note) = &state.grid_note {
        ui.label(small(note.clone(), theme.ink_weak));
    }
}

/// Seconds per tick: the base level the loops are counted in.
fn seconds_per_tick() -> f64 {
    metal_schema::TickRate::Base.seconds_per_tick()
}

/// Feeds the surface what the hardware (and the on-screen piano) sent since
/// the last frame, ticks the loops, and keeps painting while either moves.
/// Moves the piano by whole octaves until note `n` is on it; whether it moved.
fn follow_octave(layout: &mut crate::surface::Layout, n: u8) -> bool {
    let mut moved = false;
    for role in layout.roles.values_mut() {
        if let crate::surface::Role::Piano { base, keys } = role {
            let top = base.saturating_add(*keys);
            let to = if n < *base {
                base.saturating_sub((*base - n).div_ceil(12) * 12)
            } else if n >= top {
                base.saturating_add((n - top) / 12 * 12 + 12)
            } else {
                continue;
            };
            if to != *base && to.saturating_add(*keys) <= 128 + 12 {
                *base = to;
                moved = true;
            }
        }
    }
    moved
}

fn drive_surface(
    ui: &Ui,
    door: &dyn Call,
    state: &mut PanelState,
    cells: &[Cell],
    page: usize,
    pages: usize,
    wired: &[(String, String, crate::surface::DialMode)],
) {
    let now = ui.input(|i| i.time);
    let events = crate::input::drain_surface();
    if let Some(e) = events.last() {
        state.last_heard = Some(format!("{} · {}", e.input, e.port));
    }
    // `METALVJ_MIDI_LOG=1` prints every control heard, raw, for diagnosing a mapping.
    let log = std::env::var_os("METALVJ_MIDI_LOG").is_some();
    if log {
        for e in &events {
            eprintln!("  midi log: {} on {} {:?}", e.input, e.port, e.edge);
        }
    }
    let mut inputs: Vec<(String, metal_midi::Edge)> = events.into_iter().map(|e| (e.input, e.edge)).collect();
    inputs.append(&mut state.pad_inputs);
    // A swept knob sends dozens of values a frame: only its last one counts,
    // unless it is an encoder whose every step adds up.
    let relative = |input: &str| {
        let dial = crate::surface::DialMode::Relative;
        state.layout.slots.iter().any(|s| s.dial == dial && s.input.as_deref().is_some_and(|i| metal_midi::same_control(i, input)))
            || state.layout.binds.iter().any(|b| b.dial == dial && metal_midi::same_control(&b.input, input))
    };
    // (A value across the half from the next one stays: a CC button's press.)
    let mut later: HashMap<String, bool> = HashMap::new();
    let mut keep = vec![true; inputs.len()];
    for (i, (input, edge)) in inputs.iter().enumerate().rev() {
        if let metal_midi::Edge::Value { value, .. } = edge {
            let high = *value > 0.5;
            if later.insert(input.clone(), high) == Some(high) && !relative(input) {
                keep[i] = false;
            }
        }
    }
    let mut keep = keep.into_iter();
    inputs.retain(|_| keep.next().unwrap_or(true));
    let heard = !inputs.is_empty();
    for (input, edge) in inputs {
        let outs = surface_input(door, state, cells, wired, &input, edge, now, page);
        state.grid_outs.extend(outs);
    }
    let outs = state.surface.tick(&state.layout, now, seconds_per_tick());
    state.grid_outs.extend(outs);
    let outs = std::mem::take(&mut state.grid_outs);
    run_outs(ui, door, state, cells, outs, page, pages);
    if let Some(at) = state.controllers_dirty {
        if now - at > 0.3 {
            state.controllers_dirty = None;
            save_controllers(door, state);
        }
    }
    let playing = state.layout.loops.values().any(|l| l.playing);
    if heard {
        ui.ctx().request_repaint();
    }
    if playing || state.surface.is_recording() || state.controllers_dirty.is_some() {
        crate::shell::frames::repaint_after(ui.ctx(), std::time::Duration::from_millis(16));
    }
}

/// One control heard: bound while a bind waits, else fired through its
/// binding on this page — or, on a box's face, through the slot it sits
/// under (`wired`).
#[allow(clippy::too_many_arguments)]
fn surface_input(
    door: &dyn Call,
    state: &mut PanelState,
    cells: &[Cell],
    wired: &[(String, String, crate::surface::DialMode)],
    input: &str,
    edge: metal_midi::Edge,
    now: f64,
    page: usize,
) -> Vec<crate::surface::Out> {
    use crate::surface::Gesture;
    use metal_midi::Edge;
    // A pad of the box's own slots is not a piano key, whatever its note.
    let slot_input = state
        .layout
        .slots
        .iter()
        .any(|s| s.input.as_deref().is_some_and(|i| metal_midi::same_control(i, input)));
    if let Some(n) = metal_midi::input_note(input).filter(|_| !slot_input) {
        match edge {
            Edge::Press { .. } => {
                state.surface.notes.insert(n);
                // The box's octave buttons send nothing of their own: a key
                // heard past the piano moves it by whole octaves to follow.
                if state.face_learn.is_none() && follow_octave(&mut state.layout, n) {
                    save_layout(door, state);
                }
            }
            Edge::Release => {
                state.surface.notes.remove(&n);
            }
            Edge::Value { .. } => {}
        }
    }
    // A CC pad's level, turned into edges.
    let level = match edge {
        Edge::Value { value, .. } => {
            let was = state.cc_down.contains(input);
            match (value > 0.5, was) {
                (true, false) => {
                    state.cc_down.insert(input.to_string());
                    Some(true)
                }
                (false, true) => {
                    state.cc_down.remove(input);
                    Some(false)
                }
                _ => None,
            }
        }
        Edge::Press { .. } => Some(true),
        Edge::Release => Some(false),
    };
    match level {
        Some(true) => {
            state.hw_down.insert(input.to_string());
        }
        Some(false) => {
            state.hw_down.remove(input);
        }
        None => {}
    }
    if let Edge::Value { value, .. } = edge {
        state.hw_levels.insert(input.to_string(), value);
    }

    if let Some(i) = state.face_learn {
        // A turned knob keeps sending, a button sends its release: only a
        // control not just learned moves the walk on.
        let fresh = !state.face_learn_last.as_deref().is_some_and(|l| metal_midi::same_control(l, input));
        if fresh && !matches!(edge, Edge::Release) {
            if i < state.layout.slots.len() {
                state.layout.learn_slot(i, input);
                state.face_learn_last = Some(input.to_string());
                face_learn_next(door, state, i);
            } else if let Some(n) = metal_midi::input_note(input) {
                for role in state.layout.roles.values_mut() {
                    if let crate::surface::Role::Piano { base, keys } = role {
                        *base = n.min(128 - *keys);
                    }
                }
                face_learn_next(door, state, i);
            }
        }
        return Vec::new();
    }
    if let Some(slot) = state.slot_learn {
        if matches!(edge, Edge::Release) {
            return Vec::new();
        }
        state.slot_learn = None;
        if slot < state.layout.slots.len() {
            state.layout.learn_slot(slot, input);
            state.grid_note = Some(trf("{input} → {cell}", &[("input", &input), ("cell", &state.layout.slots[slot].id)]));
            save_layout(door, state);
        }
        return Vec::new();
    }
    if let Some(cell) = state.grid_learn.clone() {
        if matches!(edge, Edge::Release) {
            return Vec::new();
        }
        // A control cell answers on every page; a content cell only on the
        // page it was bound from.
        let on = (!state.layout.roles.contains_key(&cell)).then_some(page);
        state.layout.bind(input, on, &cell);
        state.grid_learn = None;
        state.grid_note = Some(trf("{input} → {cell}", &[("input", &input), ("cell", &cell)]));
        save_layout(door, state);
        return Vec::new();
    }
    let wired_bind = || {
        wired.iter().find(|(i, _, _)| metal_midi::same_control(i, input)).map(|(i, cell, dial)| crate::surface::Bind {
            input: i.clone(),
            page: Some(page),
            cell: cell.clone(),
            dial: *dial,
        })
    };
    // A piano key holding a button of its own on this page.
    let key_bind = || {
        let n = metal_midi::input_note(input).filter(|_| !slot_input)?;
        let key = key_controller(n, page);
        state.controllers.contains_key(&key).then(|| crate::surface::Bind {
            input: input.to_string(),
            page: Some(page),
            cell: format!("ctrl:{key}"),
            dial: crate::surface::DialMode::Absolute,
        })
    };
    let Some(bind) = state.layout.bound(input, page).cloned().or_else(wired_bind).or_else(key_bind) else {
        return Vec::new();
    };
    if let Some(role) = state.layout.roles.get(&bind.cell).cloned() {
        let gesture = if role.is_dial() {
            match edge {
                Edge::Value { raw, .. } => match state.surface.dial_steps(input, bind.dial, raw) {
                    0 => None,
                    n => Some(Gesture::Steps(n)),
                },
                _ => None,
            }
        } else {
            level.map(|down| if down { Gesture::Down } else { Gesture::Up })
        };
        return match gesture {
            Some(g) => state.surface.role(&state.layout, &bind.cell, &role, g, now, page),
            None => Vec::new(),
        };
    }
    if state.layout.loops.contains_key(&bind.cell) {
        return if level == Some(true) { state.surface.loop_pressed(&bind.cell) } else { Vec::new() };
    }
    let Some(cell) = find_cell(state, cells, &bind.cell) else {
        return Vec::new();
    };
    let cell = &cell;
    // A fader takes a knob's position; everything else takes the press.
    let fader = matches!(&cell.kind, CellKind::Controller { key } if controller_is_fader(state, key));
    let gesture = match (edge, fader) {
        (Edge::Value { value, .. }, true) => Some(Gesture::Value(value)),
        _ => match (level, holds(state, cell)) {
            (Some(true), true) => Some(Gesture::Down),
            (Some(false), true) => Some(Gesture::Up),
            (Some(true), false) => Some(Gesture::Click),
            _ => None,
        },
    };
    let _ = door;
    match gesture {
        Some(g) => state.surface.content(&cell.key, g, now, seconds_per_tick()),
        None => Vec::new(),
    }
}

/// Runs what the surface answered: fires cells, turns pages, nudges the
/// library, stores loops.
fn run_outs(
    ui: &Ui,
    door: &dyn Call,
    state: &mut PanelState,
    cells: &[Cell],
    outs: Vec<crate::surface::Out>,
    page: usize,
    pages: usize,
) {
    use crate::surface::Out;
    let mut save = false;
    for out in outs {
        match out {
            Out::Fire { cell, gesture } => {
                if state.layout.loops.contains_key(&cell) {
                    continue;
                }
                if let Some(c) = find_cell(state, cells, &cell) {
                    fire_cell(ui.input(|i| i.time), door, state, &c, gesture);
                }
            }
            Out::Page(p) => set_stored_page(door, p.min(pages.saturating_sub(1))),
            Out::PageStep(d) => {
                let to = (page as i64 + d as i64).clamp(0, pages as i64 - 1) as usize;
                set_stored_page(door, to);
            }
            Out::Library(intent) => super::pads::push_library_intent(ui.ctx(), intent),
            Out::LoopTicks { key, ticks } => {
                match key.and_then(|k| state.layout.loops.get_mut(&k)) {
                    Some(lp) => lp.ticks = ticks,
                    None => state.layout.loop_ticks = ticks,
                }
                state.grid_note = Some(trf("loop length · {n} ticks", &[("n", &ticks)]));
                save = true;
            }
            Out::RecStart => state.grid_note = Some(tr("recording a loop…").to_string()),
            Out::RecStop(lp) => {
                let key = state.layout.fresh_key("loop");
                state.grid_note = Some(trf("{name} recorded · {n} presses", &[("name", &lp.name), ("n", &lp.events.len())]));
                state.surface.focus_loop = Some(key.clone());
                state.layout.loops.insert(key, lp);
                save = true;
            }
            Out::LoopToggle(key) => {
                if let Some(lp) = state.layout.loops.get_mut(&key) {
                    lp.playing = !lp.playing;
                    save = true;
                }
            }
            Out::Glitch { hold, on } => {
                let body = match (hold, on) {
                    (_, false) => json!({ "op": "glitch-release" }),
                    (hold, true) => json!({ "op": "glitch", "hold": hold }),
                };
                let _ = door.json("POST", "/overlay", Some(body));
            }
            Out::Octave(step) => {
                for role in state.layout.roles.values_mut() {
                    if let crate::surface::Role::Piano { base, keys } = role {
                        let top = 127 - (*keys as i32 - 1);
                        *base = (*base as i32 + 12 * step).clamp(0, top.max(0)) as u8;
                        state.grid_note = Some(trf("piano from {note}", &[("note", &metal_midi::note_name(*base))]));
                        save = true;
                    }
                }
            }
        }
    }
    if save {
        save_layout(door, state);
    }
}

/// Posts the whole surface layout (`POST /settings {grid_surface}`), adopting
/// the reply's, the same contract [`set_grid_span`] keeps.
fn save_layout(door: &dyn Call, state: &mut PanelState) {
    let body = json!({ "grid_surface": state.layout.to_value() });
    if let Ok(reply) = door.json("POST", "/settings", Some(body)) {
        if reply["grid_surface"].is_object() {
            state.layout = crate::surface::Layout::from_value(&reply["grid_surface"]);
        }
    }
}

/// Whether a cell fires on its press/release edges (a `hold` button) rather
/// than once per press.
fn holds(state: &PanelState, cell: &Cell) -> bool {
    match &cell.kind {
        CellKind::Switch { kind, .. } => kind == "hold",
        CellKind::Fixture { hold, .. } | CellKind::Rig { hold, .. } => *hold,
        CellKind::Controller { key } => controller_mode(state, key) == "hold",
        _ => false,
    }
}

/// Whether a controller cell paints as a fader (it has a parameter target).
fn controller_is_fader(state: &PanelState, key: &str) -> bool {
    !is_scene(state, key)
        && state
            .controllers
            .get(key)
            .and_then(|c| c["targets"].as_array())
            .is_some_and(|ts| ts.iter().any(|t| t["param"].is_string()))
}

/// Runs one gesture on a content cell — the one place a click, a hardware
/// key and a replayed loop event all end.
fn fire_cell(now: f64, door: &dyn Call, state: &mut PanelState, cell: &Cell, g: crate::surface::Gesture) {
    use crate::surface::Gesture;
    let press = matches!(g, Gesture::Down | Gesture::Click);
    match &cell.kind {
        CellKind::Movement { name, device, layer, looping } => {
            if !press {
                return;
            }
            state.focused_overlay = Some(name.clone());
            let body = if cell.running {
                json!({ "op": "stop", "device": device, "layer": layer })
            } else {
                json!({ "op": "play", "device": device, "layer": layer, "movement": name, "loop": looping })
            };
            let _ = door.json("POST", "/overlay", Some(body));
        }
        CellKind::Sequence { id, device } => {
            if !press {
                return;
            }
            let body = if cell.running {
                json!({ "op": "stop", "device": device })
            } else {
                json!({ "op": "run", "id": id, "device": device })
            };
            let _ = door.json("POST", "/sequences", Some(body));
        }
        // G1: a fixture/rig button fires the same `POST /switch {op}` a
        // `toggle` switch does; G3: a `hold` one fires on the press/release
        // edge — the value applies only while pressed.
        CellKind::Switch { id, kind } => fire_switch(door, id, kind == "hold", cell.on, g),
        CellKind::Fixture { id, hold } | CellKind::Rig { id, hold } => fire_switch(door, id, *hold, cell.on, g),
        CellKind::Controller { key } if is_scene(state, key) => fire_scene(door, state, key, g),
        CellKind::Controller { key } => match g {
            Gesture::Value(v) => {
                if let ControllerFace::Fader { node, param, min, max, .. } = controller_face(door, state, key) {
                    let value = min + v.clamp(0.0, 1.0) * (max - min).max(0.0);
                    let graph = state.controllers.get(key).and_then(|c| c["graph"].as_str()).unwrap_or(key).to_string();
                    let _ = door.json(
                        "POST",
                        "/graph/node",
                        Some(json!({ "op": "param", "graph": graph, "node": node, "name": param, "value": value })),
                    );
                    let target = json!({ "node": node, "param": param, "value": value });
                    set_controller_target_value(state, key, &target, value);
                    state.controllers_dirty = Some(now);
                }
            }
            Gesture::Down | Gesture::Click => {
                // A switch toggles its light on each click; a hold lights while pressed.
                match (controller_mode(state, key).as_str(), g) {
                    ("hold", _) => {
                        state.lit_controllers.insert(key.clone());
                    }
                    (_, Gesture::Click) if !state.lit_controllers.remove(key) => {
                        state.lit_controllers.insert(key.clone());
                    }
                    _ => {}
                }
                apply_scene(door, state, key, false)
            }
            Gesture::Up => {
                if controller_mode(state, key) == "hold" {
                    state.lit_controllers.remove(key);
                    apply_scene(door, state, key, true);
                }
            }
            Gesture::Steps(_) => {}
        },
        CellKind::Blackout => {
            if press {
                let _ = door.json("POST", "/overlay/blackout", Some(json!({ "on": !cell.on })));
            }
        }
        CellKind::Identify => {
            if press {
                // M6.3: the last projected venue's fixtures, flashed.
                let _ = door.json("POST", "/venue", Some(json!({ "op": "identify", "id": "last" })));
            }
        }
        CellKind::StopAll => {
            if press {
                let _ = door.json("POST", "/overlay", Some(json!({ "op": "stop-all" })));
            }
        }
        CellKind::Add | CellKind::PageTo(_) | CellKind::Role { .. } | CellKind::Loop => {}
    }
}

/// `POST /switch` for one gesture: a hold switch follows the edges, a toggle
/// flips on each press.
fn fire_switch(door: &dyn Call, id: &str, hold: bool, on: bool, g: crate::surface::Gesture) {
    use crate::surface::Gesture;
    let op = match (hold, g) {
        (true, Gesture::Down) => "on",
        (true, Gesture::Up) => "off",
        (_, Gesture::Click) | (false, Gesture::Down) => {
            if on {
                "off"
            } else {
                "on"
            }
        }
        _ => return,
    };
    let _ = door.json("POST", "/switch", Some(json!({ "op": op, "id": id })));
}

/// The controller editor as a window over the grid, opened by the `+` menu's
/// `controller button`, a scene cell's `⚙`, or a right-click on the cell. It
/// holds [`controller_editor`] prefilled with the controller's whole payload,
/// so every reach edits the button it belongs to.
fn controller_editor_popup(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    let Some(key) = state.editing_controller.clone() else {
        return;
    };
    let mut open = true;
    crate::shell::gallery::overlay_window("shell/region/lanes/controller-editor", tr("nodes control").to_string())
        .current_pos(state.controller_edit_pos.unwrap_or(egui::pos2(theme.pad as f32, theme.pad as f32)))
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            ui.set_min_width(theme.text_input_width * 2.0);
            // Many params scroll inside the window, never push it off the screen.
            let max = ui.ctx().content_rect().height() * 0.7;
            ScrollArea::vertical().max_height(max).auto_shrink([false, true]).show(ui, |ui| {
                controller_editor(ui, theme, door, state, &key);
            });
        });
    if !open {
        state.editing_controller = None;
    }
}

/// One placeholder square: the inset fill of an empty cell, no gesture —
/// except it is a **drop target** for an `files` card (2026-09-22,
/// afternoon). A dropped `sequence` loads into the scratch and opens the lanes
/// (the `load` path, without the button); a dropped `audio` raises the
/// same intent `play` does (`pending_song`, drained by `shell/mod.rs`); a
/// dropped `video` opens the destination popover (`new_video`). An image or
/// anything else is refused — the overlays grid is not a graph canvas.
///
/// `slot` makes each phantom its own widget: the grid paints many of these
/// with one shape, and a shared id would let only the first answer a drop.
fn paint_phantom_cell(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    slot: usize,
    rect: Rect,
) -> Response {
    let response = ui.interact(
        rect,
        ui.id().with(("shell/region/lanes/overlay-phantom", slot)),
        Sense::click(),
    );
    ui.painter()
        .rect_filled(rect, theme.corner_radius(), theme.rule_bright);
    // The hover/refusal hint — what dropping here would make. Hover and
    // release are read separately: egui clears the payload on the release
    // frame before widgets run, so gating the release behind the hover would
    // never see a drop.
    if let Some(drag) = response.dnd_hover_payload::<crate::shell::panels::FileDrag>() {
        if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
            let what = match drag.kind.as_str() {
                "sequence" => format!("load {}", drag.name),
                "audio" => format!("open {}", drag.name),
                "video" => format!("create button · {}", drag.name),
                _ => String::new(),
            };
            if !what.is_empty() {
                ui.painter_at(rect).text(
                    p + Vec2::splat(theme.item_gap.x),
                    Align2::LEFT_TOP,
                    what,
                    theme.font_small.clone(),
                    theme.phosphor,
                );
            }
        }
    }
    if let Some(drag) = response.dnd_release_payload::<crate::shell::panels::FileDrag>() {
        apply_overlay_drop(door, state, &drag);
    }
    response
}

/// What an `files` card dropped on the overlays grid does — split out of
/// [`paint_phantom_cell`] so the three destinations are testable without a
/// painted grid. A `sequence` loads into the scratch and opens the lanes
/// (the `load` path, without the button); an `audio` raises the same intent
/// `play` does (`pending_song`, drained by `shell/mod.rs`); a `video` opens
/// the destination popover (`new_video`). Anything else is refused — the
/// overlays grid is not a graph canvas.
fn apply_overlay_drop(door: &dyn Call, state: &mut PanelState, drag: &crate::shell::panels::FileDrag) {
    match drag.kind.as_str() {
        "sequence" => {
            if let Some(m) = get(
                door,
                "GET",
                "/author/models/movement",
                &[("name", drag.name.clone())],
            ) {
                if state.mv.load(&m) {
                    // The editor adopts it as a track of its own.
                    state.mv_shown = Some(m.clone());
                    state.mode_override = Some(crate::shell::lanes::Mode::Lanes);
                    state.mv.note = Some(format!("opened: {}", drag.name));
                }
            }
        }
        "audio" => {
            state.pending_song = Some(drag.name.clone());
            state.mode_override = Some(crate::shell::lanes::Mode::Lanes);
        }
        "video" => {
            state.new_video = Some(NewVideo::new(drag));
        }
        _ => {}
    }
}

/// Paints one cell and turns what the pointer did into a gesture. The three
/// states the old rows each painted themselves live here once: a `phosphor`
/// fill (`on`), the accent stroke (`running`) and a movement's playhead bar
/// (`progress`). A content cell's gesture goes through the surface (freeze,
/// recording) before it fires; in edit mode a click selects the cell instead.
fn paint_cell(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    cell: &Cell,
    rect: Rect,
    open_graph: Option<&str>,
    slot: Option<usize>,
) {
    use crate::surface::{Gesture, Role};
    let now = ui.input(|i| i.time);
    let editing = state.grid_edit && !cell.is_fixed();
    let page = stored_page(door).max(0) as usize;
    // The box's control under the cell, held down right now.
    let slot_kind = slot.and_then(|i| state.layout.slots.get(i)).map(|s| s.kind);
    let slot_down = slot
        .and_then(|i| state.layout.slots.get(i))
        .and_then(|s| s.input.as_deref())
        .is_some_and(|input| state.hw_down.iter().any(|h| metal_midi::same_control(input, h)));
    let response = match &cell.kind {
        CellKind::Controller { key } => paint_controller_cell(ui, theme, door, state, key, rect, slot_kind, slot_down),
        CellKind::Role { role: Role::Piano { base, keys } } => {
            let lit = |n: u8| state.surface.notes.contains(&n);
            // A key holding a button shows it: its colours, or a mark.
            let mark = |n: u8| {
                let key = key_controller(n, page);
                state.controllers.contains_key(&key).then(|| match controller_face(door, state, &key) {
                    ControllerFace::Palette { stops } => stops,
                    _ => Vec::new(),
                })
            };
            let mut held = state.piano_held.get(&cell.key).copied().flatten();
            let id = Id::new(("shell/region/lanes/piano", &cell.key));
            let (response, edges) = super::pads::piano(ui, theme, id, rect, *base, *keys, &lit, &mark, &mut held);
            state.piano_held.insert(cell.key.clone(), held);
            // A key's right-click gives it a button of its own on this page.
            if response.secondary_clicked() && !editing {
                let note = ui
                    .input(|i| i.pointer.interact_pos())
                    .and_then(|p| super::pads::note_at(rect, *base, *keys, theme.piano_black_depth, p));
                match (note, open_graph) {
                    (Some(n), Some(gid)) => {
                        let key = key_controller(n, page);
                        if !state.controllers.contains_key(&key) {
                            state.controllers.insert(key.clone(), json!({ "graph": gid, "name": "", "mode": "switch", "targets": [] }));
                            save_controllers(door, state);
                        }
                        state.editing_controller = Some(key);
                        state.controller_edit_pos = Some(rect.min);
                    }
                    (Some(_), None) => state.grid_note = Some(tr("— open a graph to control its nodes —").to_string()),
                    _ => {}
                }
            }
            for e in edges {
                let (n, edge) = match e {
                    super::pads::KeyEdge::Press(n) => (n, metal_midi::Edge::Press { velocity: 1.0 }),
                    super::pads::KeyEdge::Release(n) => (n, metal_midi::Edge::Release),
                };
                state.pad_inputs.push((metal_midi::note_input(n), edge));
            }
            response
        }
        CellKind::Role { role } if role.is_dial() => {
            let (value, frac) = dial_face(state, &cell.key, role);
            // A knob turned past half is not a knob pressed.
            let lit = false;
            let mut acc = state.dial_acc.get(&cell.key).copied().unwrap_or(0.0);
            let id = Id::new(("shell/region/lanes/dial", &cell.key));
            let (response, steps) = super::pads::dial(ui, theme, id, rect, &cell.label, &value, frac, lit, &mut acc);
            state.dial_acc.insert(cell.key.clone(), acc);
            if steps != 0 && !editing {
                let out = state.surface.role(&state.layout, &cell.key, role, Gesture::Steps(steps), now, page);
                state.grid_outs.extend(out);
            }
            response
        }
        _ => {
            let lit = match &cell.kind {
                CellKind::Role { role } => state.surface.role_lit(&cell.key, role) || hw_pressed(state, &cell.key) || slot_down,
                _ => cell.on || slot_down,
            };
            // Filled buttons are darker than the gray empty slots.
            let text = RichText::new(&cell.label).font(theme.font_small.clone());
            // A box's face is packed tight: a name keeps to two rows inside
            // its button, cut with `…`, never grown over the control below.
            let text: egui::WidgetText = if state.layout.device.is_some() {
                let mut job = std::sync::Arc::unwrap_or_clone(egui::WidgetText::from(text).into_layout_job(ui.style(), egui::FontSelection::Default, Align::Center));
                job.wrap = egui::text::TextWrapping {
                    max_width: (rect.width() - 2.0 * ui.spacing().button_padding.x).max(theme.hairline),
                    max_rows: 2,
                    break_anywhere: true,
                    overflow_character: Some('…'),
                };
                egui::WidgetText::Galley(ui.fonts_mut(|f| f.layout_job(job)))
            } else {
                text.into()
            };
            let mut button = egui::Button::new(text).min_size(rect.size()).fill(theme.inset);
            if lit {
                button = button.fill(theme.accent_vivid);
            }
            if cell.running {
                button = button.stroke(theme.accent());
            }
            // A stable id per cell key: the grid's cells come and go, and an
            // auto id that shifted with them would orphan an open popup (the
            // `+` menu) mid-pick.
            ui.push_id(("shell/region/lanes/cell", &cell.key), |ui| ui.put(rect, button)).inner
        }
    };
    if state.surface.is_frozen(&cell.key) {
        let lock = Rect::from_min_size(rect.left_top(), Vec2::splat(theme.gear_size));
        ui.put(lock, egui::Label::new(lucide(Icon::Lock, theme.font_small.size).color(theme.phosphor)));
    }
    if state.grid_selected.as_deref() == Some(cell.key.as_str()) && state.grid_edit {
        ui.painter().rect_stroke(rect, theme.corner_radius(), theme.card_picked_stroke(), egui::StrokeKind::Inside);
    }
    if let Some(frac) = cell.progress {
        let bar = egui::Rect::from_min_size(
            egui::pos2(response.rect.left(), response.rect.bottom() - theme.hairline),
            egui::vec2(response.rect.width() * frac, theme.hairline),
        );
        ui.painter().rect_filled(bar, theme.corner_radius(), theme.phosphor);
    }
    // Edit mode: a click on a box's control opens what it can hold; any
    // other click (or right click) selects; nothing fires.
    if editing {
        if let Some(i) = slot {
            slot_popup(theme, door, state, &response, i, page, open_graph);
            return;
        }
        if response.clicked() || response.secondary_clicked() || response.drag_started() {
            state.grid_selected = Some(cell.key.clone());
        }
        return;
    }
    // A scene cell opens its shared editor on a right-click (the size menu
    // and delete live in [`controller_editor`]); a box's control what it can
    // hold ([`slot_menu`]); the piano gives its key a button; every other
    // sized cell keeps the size/remove menu.
    let piano = matches!(cell.kind, CellKind::Role { role: Role::Piano { .. } });
    match slot {
        _ if piano => {}
        Some(i) => slot_popup(theme, door, state, &response, i, page, open_graph),
        None if matches!(cell.kind, CellKind::Controller { .. }) => {}
        None if !cell.is_fixed() && state.layout.device.is_none() => {
            response.context_menu(|ui| grid_cell_menu(ui, theme, door, state, cell));
        }
        None => {}
    }

    // The pointer's gesture: a hold button (and `rec`, `drop rec`, `fn`) on
    // its press/release edges, everything else once per click.
    let edges = match &cell.kind {
        CellKind::Role { role } => matches!(role, Role::Rec | Role::DropRec | Role::Fn { .. } | Role::Glitch { hold: true }),
        _ => holds(state, cell),
    };
    let gesture = if edges {
        let down_now = response.is_pointer_button_down_on();
        let was_down = state.held_switches.contains(&cell.key);
        if down_now && !was_down {
            state.held_switches.insert(cell.key.clone());
            Some(Gesture::Down)
        } else if !down_now && was_down {
            state.held_switches.remove(&cell.key);
            Some(Gesture::Up)
        } else {
            None
        }
    } else if response.clicked() {
        Some(Gesture::Click)
    } else {
        None
    };

    match &cell.kind {
        CellKind::PageTo(to) => {
            if response.clicked() {
                set_stored_page(door, *to);
            }
        }
        CellKind::Add => {
            // The ways to fill the grid, in one menu anchored to this cell.
            let frame = Frame::popup(ui.style())
                .stroke((theme.hairline, theme.phosphor_dim))
                .inner_margin(theme.pad as f32);
            Popup::from_toggle_button_response(&response)
                .kind(egui::PopupKind::Menu)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .frame(frame)
                .show(|ui| {
                    ui.set_min_width(theme.text_input_width * 2.0);
                    add_menu(ui, theme, door, state, open_graph);
                });
        }
        CellKind::Role { role } => {
            if let Some(g) = gesture {
                let out = state.surface.role(&state.layout, &cell.key, role, g, now, page);
                state.grid_outs.extend(out);
            }
        }
        CellKind::Loop => {
            if gesture.is_some() {
                let out = state.surface.loop_pressed(&cell.key);
                state.grid_outs.extend(out);
            }
        }
        // A fader writes on its own drag (in `paint_controller_cell`).
        CellKind::Controller { key } if controller_is_fader(state, key) => {}
        _ => {
            if let Some(g) = gesture {
                let out = state.surface.content(&cell.key, g, now, seconds_per_tick());
                state.grid_outs.extend(out);
            }
        }
    }
}

/// Whether a control the cell is bound to is held on the box right now.
fn hw_pressed(state: &PanelState, key: &str) -> bool {
    state
        .layout
        .binds_of(key)
        .any(|b| state.hw_down.iter().any(|h| metal_midi::same_control(&b.input, h)))
}

/// A dial's centre text and arc: the loop length for `speed`; a box's spare
/// knob where the hardware sits.
fn dial_face(state: &PanelState, key: &str, role: &crate::surface::Role) -> (String, Option<f32>) {
    use crate::surface::{Role, LOOP_TICKS};
    match role {
        Role::SpeedDial => {
            let ticks = state
                .surface
                .focus_loop
                .as_ref()
                .and_then(|k| state.layout.loops.get(k))
                .map_or(state.layout.loop_ticks, |l| l.ticks);
            let at = LOOP_TICKS.iter().position(|&t| t >= ticks).unwrap_or(LOOP_TICKS.len() - 1);
            (format!("{ticks}"), Some(at as f32 / (LOOP_TICKS.len() - 1) as f32))
        }
        Role::Knob { .. } => {
            let level = state.layout.binds_of(key).find_map(|b| hw_level(state, &b.input));
            (level.map(|v| format!("{:.0}", v * 127.0)).unwrap_or_default(), level)
        }
        _ => (String::new(), None),
    }
}

/// The label a controller scene cell paints: its `name` when it has one, else
/// its `graph` id, else the key itself. A box's control or a piano key says
/// nothing until it is named.
fn controller_label(state: &PanelState, key: &str) -> String {
    if is_hw_controller(key) {
        return state.controllers.get(key).and_then(|c| c["name"].as_str()).unwrap_or_default().to_string();
    }
    state
        .controllers
        .get(key)
        .and_then(|c| {
            c["name"]
                .as_str()
                .filter(|n| !n.is_empty())
                .or_else(|| c["graph"].as_str().filter(|g| !g.is_empty()))
                .map(str::to_string)
        })
        .unwrap_or_else(|| key.to_string())
}

/// How a scene cell paints, read off the shapes of its targets: a **fader**
/// when any target is a parameter, the **palette** look (a colour strip along
/// the foot) when every target is a recalled palette/colour node, a plain
/// **button** otherwise.
enum ControllerFace {
    Button,
    Fader { node: String, param: String, min: f32, max: f32, value: f32 },
    Palette { stops: Vec<[f32; 3]> },
}

/// The face a controller cell wears — see [`ControllerFace`]. The first
/// `param` target decides the fader (its `GET /graph/io` range maps the stored
/// value in); an all-palette/colour target set decides the strip.
fn controller_face(door: &dyn Call, state: &PanelState, key: &str) -> ControllerFace {
    let Some(ctrl) = state.controllers.get(key) else {
        return ControllerFace::Button;
    };
    // A scene is a plain button: its floats are a snapshot, not a fader.
    if is_scene(state, key) {
        return ControllerFace::Button;
    }
    let graph = ctrl["graph"].as_str().unwrap_or(key).to_string();
    let targets = ctrl["targets"].as_array().cloned().unwrap_or_default();
    // The first target whose node and param still exist: one deleted (its
    // node or float gone) is skipped, so a param added after it takes the dial.
    for t in targets.iter().filter(|t| t["param"].is_string()) {
        let node = t["node"].as_str().unwrap_or_default().to_string();
        let param = t["param"].as_str().unwrap_or_default().to_string();
        let stored = t["value"].as_f64().unwrap_or(0.0) as f32;
        let io = get(door, "GET", "/graph/io", &[("id", graph.clone()), ("node", node.clone())]).unwrap_or(Value::Null);
        let Some(f) = super::dials::floats(&io).into_iter().find(|f| f.name == param) else { continue };
        let value = stored.clamp(f.min, f.max);
        return ControllerFace::Fader { node, param, min: f.min, max: f.max, value };
    }
    if !targets.is_empty()
        && targets.iter().all(|t| {
            t["param"].is_null()
                && t["fixture"].is_null()
                && t["group"].is_null()
                && (t.get("colors").is_some() || t.get("rgb").is_some())
        })
    {
        return ControllerFace::Palette { stops: targets.iter().flat_map(stops_of).collect() };
    }
    ControllerFace::Button
}

/// The stops one recalled palette/colour target carries: a palette node's four
/// `colors`, a colour node's single `rgb`, as `[r, g, b]`.
fn stops_of(t: &Value) -> Vec<[f32; 3]> {
    let rgb = |v: &Value| {
        let a = v.as_array();
        let c = |i: usize| a.and_then(|a| a.get(i)).and_then(Value::as_f64).unwrap_or(1.0) as f32;
        [c(0), c(1), c(2)]
    };
    if let Some(colors) = t.get("colors").and_then(Value::as_array) {
        return colors.iter().map(rgb).collect();
    }
    if let Some(rgb_value) = t.get("rgb") {
        return vec![rgb(rgb_value)];
    }
    Vec::new()
}

/// Paints one controller as **one scene cell** and runs its gesture. The face
/// follows the targets ([`controller_face`]): a parameter makes it the grid's
/// fader — a drag posts `{op:"param"}` live and saves on release; an
/// all-palette/colour set paints the palette strip and reapplies the node's
/// captured colours through `POST /graph/node {op:"config"}` (node
/// conventions, never the global colour rule); otherwise it is a plain
/// button. Pressing a button or palette applies the whole stored state at once
/// in `switch` mode, on the press/release edge in `hold` mode — the opposite
/// `on` on release. The `⚙` corner and a right-click both open
/// [`controller_editor`]. In a box's knob a fader is the knob itself — a
/// drag or the wheel turns it — and `held` lights a button while its box
/// control is down. Returns the cell's response.
#[allow(clippy::too_many_arguments)]
fn paint_controller_cell(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    key: &str,
    rect: Rect,
    shape: Option<crate::surface::SlotKind>,
    held: bool,
) -> Response {
    if !state.controllers.contains_key(key) {
        return ui.interact(
            rect,
            ui.id().with(("shell/region/lanes/controller", key)),
            Sense::hover(),
        );
    }
    let label = controller_label(state, key);
    let face = controller_face(door, state, key);
    let fill = if held { theme.accent_vivid } else { theme.inset };
    let response = match face {
        ControllerFace::Fader { min, max, value, .. } if shape == Some(crate::surface::SlotKind::Knob) => {
            let range = (max - min).max(0.0);
            let frac = if range > 0.0 { ((value - min) / range).clamp(0.0, 1.0) } else { 0.0 };
            let mut acc = state.dial_acc.get(key).copied().unwrap_or(0.0);
            let id = Id::new(("shell/region/lanes/controller-knob", key));
            let (response, steps) = super::pads::dial(ui, theme, id, rect, &label, &format!("{value:.2}"), Some(frac), false, &mut acc);
            state.dial_acc.insert(key.to_string(), acc);
            if steps != 0 && !state.grid_edit {
                // A hundred steps sweep the whole range, as the fader's drag does.
                apply_controller_fraction(door, state, key, (frac + steps as f32 / 100.0).clamp(0.0, 1.0));
                state.controllers_dirty = Some(ui.input(|i| i.time));
            }
            response
        }
        ControllerFace::Fader { min, max, value, .. } => {
            let range = (max - min).max(0.0);
            let frac = if range > 0.0 { ((value - min) / range).clamp(0.0, 1.0) } else { 0.0 };
            let f = builtins::fader(
                ui,
                theme,
                Id::new(("shell/region/lanes/controller-fader", key)),
                &label,
                frac,
                rect,
                false,
                Sense::click_and_drag(),
            );
            if f.response.dragged() || f.response.clicked() || f.response.changed() {
                apply_controller_fraction(door, state, key, f.value);
            }
            if f.response.drag_stopped() {
                save_controllers(door, state);
            }
            f.response
        }
        ControllerFace::Palette { stops } => {
            let button = egui::Button::new(RichText::new(&label).font(theme.font_small.clone()))
                .min_size(rect.size())
                .fill(fill);
            let response = ui.put(rect, button);
            builtins::paint_stops(ui, theme, &stops, rect);
            response
        }
        ControllerFace::Button => {
            let button = egui::Button::new(RichText::new(&label).font(theme.font_small.clone()))
                .min_size(rect.size())
                .fill(fill);
            ui.put(rect, button)
        }
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label.clone()));

    // The `⚙` corner: the same editor a right-click opens, one click away.
    // Inset from the corner with its own outline, so it reads as a control
    // of its own rather than a smudge on the button's edge.
    let pad = theme.gear_size * 0.2;
    let gear = Rect::from_min_size(
        rect.right_top() + egui::vec2(-(theme.gear_size + pad), pad),
        egui::vec2(theme.gear_size, theme.gear_size),
    );
    ui.painter().rect_stroke(gear, theme.corner_radius(), theme.rule_stroke(), egui::StrokeKind::Inside);
    let gear_response = ui.put(
        gear.shrink(pad * 0.5),
        egui::Button::new(lucide(Icon::Settings, theme.font_small.size)).frame(false),
    );
    gear_response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr("controller settings"))
    });
    // On a box's control the right-click is the control's menu instead.
    if gear_response.clicked() || (response.secondary_clicked() && shape.is_none()) {
        state.editing_controller = Some(key.to_string());
        state.controller_edit_pos = Some(rect.min);
    }

    // A fader writes on its own drag; every other face applies the scene
    // through `paint_cell`'s gesture (the click, or the hold edges), so the
    // surface can freeze and record it like any other button.
    response
}

/// The numeric targets of a controller that still exist (their node and float
/// are there), with the range each one's float reports.
fn controller_float_targets(door: &dyn Call, state: &PanelState, key: &str) -> Vec<(String, String, f32, f32)> {
    let Some(ctrl) = state.controllers.get(key) else { return Vec::new() };
    let graph = ctrl["graph"].as_str().unwrap_or(key).to_string();
    let mut out = Vec::new();
    for t in ctrl["targets"].as_array().into_iter().flatten().filter(|t| t["param"].is_string()) {
        let node = t["node"].as_str().unwrap_or_default().to_string();
        let param = t["param"].as_str().unwrap_or_default().to_string();
        let io = get(door, "GET", "/graph/io", &[("id", graph.clone()), ("node", node.clone())]).unwrap_or(Value::Null);
        if let Some(f) = super::dials::floats(&io).into_iter().find(|f| f.name == param) {
            out.push((node, param, f.min, f.max));
        }
    }
    out
}

/// Sets every numeric target of a controller to the same place in its own
/// range (`frac`, 0..1). The dial and the fader both write through here, so a
/// knob and a fader of one controller do the same thing, and a controller with
/// several params moves all of them together.
fn apply_controller_fraction(door: &dyn Call, state: &mut PanelState, key: &str, frac: f32) {
    let graph = state.controllers.get(key).and_then(|c| c["graph"].as_str()).unwrap_or(key).to_string();
    for (node, param, min, max) in controller_float_targets(door, state, key) {
        let new = min + frac * (max - min).max(0.0);
        let _ = door.json(
            "POST",
            "/graph/node",
            Some(json!({ "op": "param", "graph": graph, "node": node, "name": param, "value": new })),
        );
        let target = json!({ "node": node, "param": param, "value": new });
        set_controller_target_value(state, key, &target, new);
    }
}

/// Applies a controller's whole stored state through the one node write each
/// target needs: `{op:"config"}` for a recalled palette/colour target,
/// `{op:"param"}` for a numeric target, `{op:"flag", field:"bypass"}` for
/// every other. `inverse` posts the opposite `on` for the non-numeric targets
/// (a `hold` controller's release) and leaves held params and recalled
/// colours where they are.
fn apply_scene(door: &dyn Call, state: &PanelState, key: &str, inverse: bool) {
    let Some(ctrl) = state.controllers.get(key) else {
        return;
    };
    let graph = ctrl["graph"].as_str().unwrap_or(key).to_string();
    let targets = ctrl["targets"].as_array().cloned().unwrap_or_default();
    for t in targets {
        let Some(node) = t["node"].as_str() else { continue };
        if let Some(param) = t["param"].as_str() {
            if inverse {
                continue;
            }
            let value = t["value"].as_f64().unwrap_or(0.0);
            let _ = door.json(
                "POST",
                "/graph/node",
                Some(json!({ "op": "param", "graph": graph, "node": node, "name": param, "value": value })),
            );
        } else if t.get("colors").is_some() || t.get("rgb").is_some() {
            // A recalled palette/colour: the button writes the node's own
            // captured config back — never `POST /colour/apply` and never the
            // global colour rule.
            if inverse {
                continue;
            }
            let mut config = json!({});
            if let Some(colors) = t.get("colors") {
                config["colors"] = colors.clone();
            }
            if let Some(rgb) = t.get("rgb") {
                config["rgb"] = rgb.clone();
            }
            let _ = door.json(
                "POST",
                "/graph/node",
                Some(json!({ "op": "config", "graph": graph, "node": node, "config": config })),
            );
        } else {
            let on = t["on"].as_bool().unwrap_or(true) != inverse;
            let _ = door.json(
                "POST",
                "/graph/node",
                Some(json!({ "op": "flag", "graph": graph, "node": node, "field": "bypass", "on": on })),
            );
        }
    }
}

/// Whether `key` is a **scene**: a saved controller flagged `scene: true`. A
/// scene is the same entry in the same `grid_controllers` map as a controller
/// button; it differs only in what its targets are (one per float of the
/// chosen top parent nodes) and in what a press does ([`fire_scene`]).
fn is_scene(state: &PanelState, key: &str) -> bool {
    state.controllers.get(key).is_some_and(|c| c["scene"].as_bool() == Some(true))
}

/// The key a new scene of `graph` lives under: `scene:<graph>:<n>`, the first
/// `n` not yet taken.
fn new_scene_key(state: &PanelState, graph: &str) -> String {
    (1..).map(|n| format!("scene:{graph}:{n}")).find(|k| !state.controllers.contains_key(k)).expect("an unused scene key")
}

/// The nodes a graph's cables feed (the `to` end of an edge). A scene lists
/// only the graph's top parents, so these are the children it leaves out.
fn fed_nodes(graph: &Value) -> std::collections::BTreeSet<String> {
    graph["graph"]["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["to"]["node"].as_str().map(str::to_string))
        .collect()
}

/// `node`'s float parameters as the scene reads them: each with its live value
/// (else its default), and the value it **holds** — `None` when it is on auto
/// (driven by weights, a source or a binding).
fn node_floats(door: &dyn Call, graph: &str, node: &str) -> Vec<(super::dials::Float, Option<f32>)> {
    let io = get(door, "GET", "/graph/io", &[("id", graph.to_string()), ("node", node.to_string())]).unwrap_or(Value::Null);
    let params = io["params"].as_array().cloned().unwrap_or_default();
    super::dials::floats(&io)
        .into_iter()
        .map(|f| {
            let held = params
                .iter()
                .find(|p| p["name"].as_str() == Some(f.name.as_str()))
                .and_then(|p| p["value"].as_f64())
                .map(|v| v as f32);
            (f, held)
        })
        .collect()
}

/// Whether the scene at `key` already lists a float of `node`.
fn scene_has_node(state: &PanelState, key: &str, node: &str) -> bool {
    state
        .controllers
        .get(key)
        .and_then(|c| c["targets"].as_array())
        .is_some_and(|ts| ts.iter().any(|t| t["node"].as_str() == Some(node)))
}

/// Toggles one top parent `node` in the scene: picking it captures every float
/// it has now (one target per float, the live value), unpicking drops them all.
fn toggle_scene_node(door: &dyn Call, state: &mut PanelState, key: &str, graph: &str, node: &str) {
    let picked = scene_has_node(state, key, node);
    let captured: Vec<Value> = if picked {
        Vec::new()
    } else {
        node_floats(door, graph, node)
            .into_iter()
            .map(|(f, _)| json!({ "node": node, "param": f.name, "value": f.value }))
            .collect()
    };
    let Some(targets) = state.controllers.get_mut(key).and_then(|c| c["targets"].as_array_mut()) else {
        return;
    };
    if picked {
        targets.retain(|t| t["node"].as_str() != Some(node));
    } else {
        targets.extend(captured);
    }
    save_controllers(door, state);
}

/// `⚙` › `capture again`: every target's value is read again from its node's
/// current floats, so the scene keeps its own floats and takes the new numbers.
fn capture_scene_again(door: &dyn Call, state: &mut PanelState, key: &str) {
    let Some(ctrl) = state.controllers.get(key).cloned() else { return };
    let graph = ctrl["graph"].as_str().unwrap_or(key).to_string();
    let mut targets = ctrl["targets"].as_array().cloned().unwrap_or_default();
    for t in &mut targets {
        let (Some(node), Some(param)) = (t["node"].as_str(), t["param"].as_str()) else { continue };
        if let Some((f, _)) = node_floats(door, &graph, node).into_iter().find(|(f, _)| f.name == param) {
            t["value"] = json!(f.value);
        }
    }
    if let Some(entry) = state.controllers.get_mut(key) {
        entry["targets"] = json!(targets);
    }
    save_controllers(door, state);
}

/// Whether the scene is on: it holds the `before` snapshot a press wrote.
fn scene_on(state: &PanelState, key: &str) -> bool {
    state.controllers.get(key).is_some_and(|c| c["before"].is_array())
}

/// A press turns the scene on or off. A `switch` scene toggles on a click; a
/// `hold` scene is on while pressed (`Down`, released on `Up`). Each direction
/// is one burst of `POST /graph/node {op: "param"}`, one per float — no ticks,
/// no fade.
fn fire_scene(door: &dyn Call, state: &mut PanelState, key: &str, g: crate::surface::Gesture) {
    use crate::surface::Gesture;
    let hold = controller_mode(state, key) == "hold";
    match g {
        Gesture::Down | Gesture::Click if hold => {
            state.lit_controllers.insert(key.to_string());
            if !scene_on(state, key) {
                apply_scene_floats(door, state, key);
            }
        }
        Gesture::Up if hold => {
            state.lit_controllers.remove(key);
            restore_scene_floats(door, state, key);
        }
        Gesture::Click => {
            if scene_on(state, key) {
                state.lit_controllers.remove(key);
                restore_scene_floats(door, state, key);
            } else {
                state.lit_controllers.insert(key.to_string());
                apply_scene_floats(door, state, key);
            }
        }
        _ => {}
    }
}

/// Sets every target of the scene at `key` to its captured value. Before each
/// write the float's held value is read (`null` when it was on auto) and kept on
/// the scene as `before`, so [`restore_scene_floats`] can put it back.
fn apply_scene_floats(door: &dyn Call, state: &mut PanelState, key: &str) {
    let Some(ctrl) = state.controllers.get(key).cloned() else { return };
    let graph = ctrl["graph"].as_str().unwrap_or(key).to_string();
    let mut read: std::collections::HashMap<String, Vec<(super::dials::Float, Option<f32>)>> = Default::default();
    let mut before = Vec::new();
    let mut updates = Vec::new();
    for t in ctrl["targets"].as_array().into_iter().flatten() {
        let (Some(node), Some(param)) = (t["node"].as_str(), t["param"].as_str()) else { continue };
        let floats = read.entry(node.to_string()).or_insert_with(|| node_floats(door, &graph, node));
        let held = floats.iter().find(|(f, _)| f.name == param).and_then(|(_, h)| *h);
        before.push(json!({ "node": node, "param": param, "value": held }));
        let value = t["value"].as_f64().unwrap_or(0.0);
        updates.push(json!({ "node": node, "name": param, "value": value }));
    }
    // One write for the whole scene: `op: "params"` saves the graph once.
    if !updates.is_empty() {
        let _ = door.json("POST", "/graph/node", Some(json!({ "op": "params", "graph": graph, "updates": updates })));
    }
    if let Some(entry) = state.controllers.get_mut(key) {
        entry["before"] = json!(before);
    }
    save_controllers(door, state);
}

/// Puts every float back as [`apply_scene_floats`] found it: a float that held
/// a number gets that number, one that was on auto gets `null`, which releases
/// it to auto again. Then the `before` snapshot is cleared (the scene is off).
fn restore_scene_floats(door: &dyn Call, state: &mut PanelState, key: &str) {
    let Some(before) = state.controllers.get(key).and_then(|c| c["before"].as_array()).cloned() else { return };
    let graph = state.controllers.get(key).and_then(|c| c["graph"].as_str()).unwrap_or(key).to_string();
    let updates: Vec<Value> = before
        .iter()
        .filter_map(|b| Some(json!({ "node": b["node"].as_str()?, "name": b["param"].as_str()?, "value": b["value"] })))
        .collect();
    if !updates.is_empty() {
        let _ = door.json("POST", "/graph/node", Some(json!({ "op": "params", "graph": graph, "updates": updates })));
    }
    if let Some(entry) = state.controllers.get_mut(key).and_then(|c| c.as_object_mut()) {
        entry.remove("before");
    }
    save_controllers(door, state);
}

/// The sizes a cell offers (`surface::SPANS`: whole cells, the halves and
/// thirds, the wide strips), the current one checked. Returns the freshly
/// picked span, if any — the one place both a cell's right-click menu and the
/// controller editor's size row keep the same set.
fn size_choices(ui: &mut Ui, theme: &Theme, current: (f32, f32)) -> Option<(f32, f32)> {
    ui.label(RichText::new(tr("size")).font(theme.font_small.clone()));
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        for span in crate::surface::SPANS {
            let label = crate::surface::span_label(span);
            if ui
                .radio(current == span, RichText::new(label).font(theme.font_small.clone()))
                .clicked()
            {
                picked = Some(span);
            }
        }
    });
    picked
}

/// The right-click menu of a sized non-scene cell: `size` (the four spans, the
/// current one checked) and, for a switch, `remove` — the `POST /switch
/// {op:"remove"}` the properties editor posts. A scene cell does not come here:
/// its right-click opens [`controller_editor`] instead.
fn grid_cell_menu(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState, cell: &Cell) {
    if let Some(span) = size_choices(ui, theme, cell.span) {
        set_grid_span(door, state, &cell.key, span);
        ui.close();
    }
    if let CellKind::Switch { id, .. }
    | CellKind::Fixture { id, .. }
    | CellKind::Rig { id, .. } = &cell.kind
    {
        ui.separator();
        if ui
            .button(RichText::new("remove").font(theme.font_small.clone()))
            .clicked()
        {
            let _ = door.json("POST", "/switch", Some(json!({ "op": "remove", "id": id })));
            ui.close();
        }
    }
}

/// Stores one cell's size and posts the whole map (`POST /settings
/// {grid_spans}`). The reply's map is adopted when it carries one, so the
/// route stays the place that knows what landed.
fn set_grid_span(door: &dyn Call, state: &mut PanelState, key: &str, span: (f32, f32)) {
    state.grid_spans.insert(key.to_string(), span);
    let body = json!({ "grid_spans": spans_json(&state.grid_spans) });
    if let Ok(reply) = door.json("POST", "/settings", Some(body)) {
        if reply["grid_spans"].is_object() {
            state.grid_spans = parse_spans(&reply["grid_spans"]);
        }
    }
}

/// The `grid_spans` object a settings reply carries, read leniently: an entry
/// that is not a `[w,h]` pair off `surface::SPANS` is dropped rather than
/// painted.
fn parse_spans(v: &Value) -> BTreeMap<String, (f32, f32)> {
    v.as_object()
        .map(|obj| {
            obj.iter()
                .filter_map(|(key, pair)| {
                    let a = pair.as_array()?;
                    let w = a.first()?.as_f64()?;
                    let h = a.get(1)?.as_f64()?;
                    crate::surface::valid_span(w, h).then(|| (key.clone(), (w as f32, h as f32)))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A span as JSON: whole cells as integers (`[2, 1]`), fractions as the
/// one-decimal number they are picked as (`[1, 0.3]`).
fn spans_json(spans: &BTreeMap<String, (f32, f32)>) -> Value {
    let n = |v: f32| if v.fract() == 0.0 { json!(v as u64) } else { json!((v as f64 * 10.0).round() / 10.0) };
    Value::Object(
        spans
            .iter()
            .map(|(key, &(w, h))| (key.clone(), json!([n(w), n(h)])))
            .collect(),
    )
}

/// The `grid_controllers` object a settings reply carries, read leniently:
/// an entry that is not an object, or carries a non-array `targets`, is
/// dropped rather than painted. An absent or `null` field (the build before
/// the settings agent lands) is an empty map, so the grid builds either way.
fn parse_controllers(v: &Value) -> BTreeMap<String, Value> {
    v.as_object()
        .map(|obj| {
            obj.iter()
                .filter(|(_, c)| {
                    c.as_object().is_some_and(|o| {
                        o.get("targets").is_none_or(|t| t.as_array().is_some())
                    })
                })
                .map(|(key, c)| (key.clone(), c.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Stores the whole controller map and posts it (`POST /settings
/// {grid_controllers}`), adopting the reply's map when it carries one — the
/// same "the route is the place that knows what landed" contract
/// [`set_grid_span`] keeps.
fn save_controllers(door: &dyn Call, state: &mut PanelState) {
    let body = json!({ "grid_controllers": Value::Object(state.controllers.clone().into_iter().collect()) });
    if let Ok(reply) = door.json("POST", "/settings", Some(body)) {
        if reply["grid_controllers"].is_object() {
            state.controllers = parse_controllers(&reply["grid_controllers"]);
        }
    }
}

/// The key a controller lives under: the graph id itself, so a graph's one
/// controller and its one scene cell share the key.
fn controller_key(graph: &str) -> String {
    graph.to_string()
}

/// The controller entry for `graph`, created (named empty, mode `switch`, no
/// targets) on the first touch — a name, a mode flip or a pick all lazily make
/// the button.
fn controller_entry<'a>(state: &'a mut PanelState, graph: &str) -> &'a mut Value {
    let entry = state
        .controllers
        .entry(controller_key(graph))
        .or_insert_with(|| json!({ "graph": graph, "name": "", "mode": "switch", "targets": [] }));
    if entry["graph"].as_str().is_none() {
        entry["graph"] = json!(graph);
    }
    if entry["name"].as_str().is_none() {
        entry["name"] = json!("");
    }
    if entry["mode"].as_str().is_none() {
        entry["mode"] = json!("switch");
    }
    if !entry["targets"].is_array() {
        entry["targets"] = json!([]);
    }
    entry
}

/// Whether two targets name the same thing — the same node, and the same
/// `param`/`fixture`/`group` (each absent or equal). `on`/`value` differ, so a
/// re-pick updates in place rather than duplicating.
fn same_target(a: &Value, b: &Value) -> bool {
    let field = |v: &Value, name: &str| v.get(name).and_then(Value::as_str).map(str::to_string);
    field(a, "node") == field(b, "node")
        && field(a, "param") == field(b, "param")
        && field(a, "fixture") == field(b, "fixture")
        && field(a, "group") == field(b, "group")
}

/// Whether `graph`'s controller already holds `target`.
fn controller_has_target(state: &PanelState, graph: &str, target: &Value) -> bool {
    state
        .controllers
        .get(&controller_key(graph))
        .and_then(|c| c["targets"].as_array())
        .is_some_and(|ts| ts.iter().any(|t| same_target(t, target)))
}

/// The stored `on` of `target` on `graph`'s controller, defaulting to `true`
/// for a target that predates the field.
fn controller_target_on(state: &PanelState, graph: &str, target: &Value) -> bool {
    state
        .controllers
        .get(&controller_key(graph))
        .and_then(|c| c["targets"].as_array())
        .and_then(|ts| ts.iter().find(|t| same_target(t, target)))
        .and_then(|t| t["on"].as_bool())
        .unwrap_or(true)
}

/// Adds (or updates in place) one target on `graph`'s controller. A target is
/// the `{node, on?}` a node pick adds, the `{node, param, value}` a parameter
/// pick adds, or the `{node, fixture|group, on}` a venue pick adds.
fn add_controller_target(door: &dyn Call, state: &mut PanelState, graph: &str, target: Value) {
    {
        let entry = controller_entry(state, graph);
        let targets = entry["targets"].as_array_mut().expect("targets is an array");
        match targets.iter_mut().find(|t| same_target(t, &target)) {
            Some(t) => *t = target,
            None => targets.push(target),
        }
    }
    save_controllers(door, state);
}

/// Removes `target` from `graph`'s controller when it is there.
fn remove_controller_target(door: &dyn Call, state: &mut PanelState, graph: &str, target: &Value) {
    if let Some(entry) = state.controllers.get_mut(&controller_key(graph)) {
        if let Some(targets) = entry["targets"].as_array_mut() {
            targets.retain(|t| !same_target(t, target));
        }
    }
    save_controllers(door, state);
}

/// Toggles membership of `target` on `graph`'s controller — the gesture every
/// list row makes.
fn toggle_controller_target(door: &dyn Call, state: &mut PanelState, graph: &str, target: Value) {
    if controller_has_target(state, graph, &target) {
        remove_controller_target(door, state, graph, &target);
    } else {
        add_controller_target(door, state, graph, target);
    }
}

/// Writes `target`'s stored `on`, if it is a target of `graph`'s controller.
fn set_controller_target_on(door: &dyn Call, state: &mut PanelState, graph: &str, target: &Value, on: bool) {
    if let Some(entry) = state.controllers.get_mut(&controller_key(graph)) {
        if let Some(targets) = entry["targets"].as_array_mut() {
            if let Some(t) = targets.iter_mut().find(|t| same_target(t, target)) {
                t["on"] = json!(on);
            }
        }
    }
    save_controllers(door, state);
}

/// Writes `target`'s stored `value` in place (no save — a drag saves on
/// release).
fn set_controller_target_value(state: &mut PanelState, graph: &str, target: &Value, value: f32) {
    if let Some(entry) = state.controllers.get_mut(&controller_key(graph)) {
        if let Some(targets) = entry["targets"].as_array_mut() {
            if let Some(t) = targets.iter_mut().find(|t| same_target(t, target)) {
                t["value"] = json!(value);
            }
        }
    }
}

/// Stores `graph`'s controller name.
fn set_controller_name(door: &dyn Call, state: &mut PanelState, graph: &str, name: &str) {
    controller_entry(state, graph)["name"] = json!(name);
    save_controllers(door, state);
}

/// Removes `graph`'s controller from the grid (and from the box's control
/// it sat in).
fn delete_controller(door: &dyn Call, state: &mut PanelState, graph: &str) {
    state.controllers.remove(&controller_key(graph));
    save_controllers(door, state);
    let cell = format!("ctrl:{}", controller_key(graph));
    if state.layout.places.contains_key(&cell) {
        state.layout.forget(&cell);
        save_layout(door, state);
    }
}

/// The `switch`/`hold` mode `graph`'s controller carries, `switch` (a click
/// applies) when it has none.
fn controller_mode(state: &PanelState, graph: &str) -> String {
    state
        .controllers
        .get(&controller_key(graph))
        .and_then(|c| c["mode"].as_str())
        .unwrap_or("switch")
        .to_string()
}

/// Stores `graph`'s controller mode (`switch`/`hold`), creating the controller
/// on the first flip.
fn set_controller_mode(door: &dyn Call, state: &mut PanelState, graph: &str, hold: bool) {
    controller_entry(state, graph)["mode"] = json!(if hold { "hold" } else { "switch" });
    save_controllers(door, state);
}

/// The overlays grid's stored page (`grid_page` in settings, unclamped: a
/// trigger may have stepped it past an end). Unreadable settings read as the
/// first page.
fn stored_page(door: &dyn Call) -> i64 {
    get(door, "GET", "/settings", &[])
        .and_then(|v| v["grid_page"].as_i64())
        .unwrap_or(0)
}

/// Stores the overlays grid's page; a failed write leaves the page where it was.
fn set_stored_page(door: &dyn Call, page: usize) {
    let _ = door.json("POST", "/settings", Some(json!({ "grid_page": page })));
}


/// Edit mode's inspector for the selected cell: its size, its page and place
/// in the order, the MIDI controls bound to it (with `learn`), its own
/// settings (a `fn`'s page, a piano's octave, a loop's length) and `remove`
/// where the cell can go.
fn cell_editor_popup(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    cells: &[Cell],
    page: usize,
    pages: usize,
) {
    use crate::surface::{DialMode, Role};
    if !state.grid_edit {
        return;
    }
    let Some(key) = state.grid_selected.clone() else {
        return;
    };
    let Some(cell) = cells.iter().find(|c| c.key == key) else {
        state.grid_selected = None;
        return;
    };
    let keys: Vec<String> = cells.iter().map(|c| c.key.clone()).collect();
    let role = state.layout.roles.get(&key).cloned();
    let mut changed = false;
    let mut open = true;
    crate::shell::gallery::overlay_window("shell/region/lanes/cell-editor", cell.label.clone())
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            ui.set_min_width(theme.text_input_width * 2.0);
            let small = |t: String| RichText::new(t).font(theme.font_small.clone());

            // On a box's face the panel sizes everything.
            if state.layout.device.is_none() {
                if let Some(span) = size_choices(ui, theme, cell.span) {
                    set_grid_span(door, state, &key, span);
                }
                ui.separator();
            }
            // A control cell's job: any of the surface's, so a box's button
            // or knob does what the operator wants of it.
            // A box's control holding a button: the same jobs, in its place.
            if let CellKind::Controller { key: ctrl } = &cell.kind {
                if let Some(slot) = slot_of_controller(state, ctrl) {
                    if job_menu(ui, theme, door, state, slot) {
                        state.grid_selected = None;
                    }
                    ui.separator();
                }
            }
            if let Some(current) = role.as_ref().filter(|r| !matches!(r, Role::Piano { .. })) {
                ui.horizontal(|ui| {
                    ui.label(small(tr("general control").into()));
                    let _ = plain_menu(ui, theme, small(tr_dyn(&current.label()).to_string()), |ui| {
                        let jobs = Role::catalogue()
                            .into_iter()
                            .filter(|r| !matches!(r, Role::Piano { .. }))
                            .chain(current.is_spare().then(|| current.clone()));
                        for job in jobs {
                            if ui.selectable_label(&job == current, small(tr_dyn(&job.label()).to_string())).clicked() {
                                state.layout.roles.insert(key.clone(), job);
                                changed = true;
                                ui.close();
                            }
                        }
                    });
                });
                ui.separator();
            }

            // Its page: pinned cells stay on theirs; the rest follow the order.
            // A control cell is on every page.
            if role.is_some() {
                ui.label(small(tr("every page").into()));
            }
            if role.is_none() {
            ui.horizontal(|ui| {
                let pinned = state.layout.pages.get(&key).copied();
                let at = pinned.unwrap_or(page);
                if ui.button(small("‹".into())).clicked() {
                    state.layout.pages.insert(key.clone(), at.saturating_sub(1));
                    changed = true;
                }
                ui.label(small(match pinned {
                    Some(p) => trf("page {n}", &[("n", &(p + 1))]),
                    None => tr("page: follows the order").to_string(),
                }));
                if ui.button(small("›".into())).clicked() {
                    state.layout.pages.insert(key.clone(), at + 1);
                    state.layout.page_count = state.layout.page_count.max(at + 2).max(pages);
                    changed = true;
                }
                if pinned.is_some() && ui.button(small(tr("unpin").into())).clicked() {
                    state.layout.pages.remove(&key);
                    changed = true;
                }
            });
            }
            ui.horizontal(|ui| {
                ui.label(small(tr("order").into()));
                if ui.button(small(tr("earlier").into())).clicked() {
                    state.layout.shift(&keys, &key, -1);
                    changed = true;
                }
                if ui.button(small(tr("later").into())).clicked() {
                    state.layout.shift(&keys, &key, 1);
                    changed = true;
                }
            });
            ui.separator();

            // The controls bound to it, per page (a control cell answers on
            // every page).
            ui.label(small(tr("midi").into()));
            let binds: Vec<crate::surface::Bind> = state.layout.binds_of(&key).cloned().collect();
            for b in &binds {
                ui.horizontal_wrapped(|ui| {
                    let on = match b.page {
                        Some(p) => trf("page {n}", &[("n", &(p + 1))]),
                        None => tr("every page").to_string(),
                    };
                    ui.label(small(format!("{} · {on}", b.input)));
                    if role.as_ref().is_some_and(Role::is_dial) {
                        for mode in DialMode::ALL {
                            if ui.selectable_label(b.dial == mode, small(tr_dyn(mode.label()))).clicked() {
                                if let Some(x) = state.layout.binds.iter_mut().find(|x| *x == b) {
                                    x.dial = mode;
                                }
                                changed = true;
                            }
                        }
                    }
                    if ui.button(lucide(Icon::Close, theme.font_small.size)).on_hover_text(tr("unbind")).clicked() {
                        state.layout.binds.retain(|x| x != b);
                        changed = true;
                    }
                });
            }
            let learning = state.grid_learn.as_deref() == Some(key.as_str());
            if ui.selectable_label(learning, small(tr("learn input signal").into())).clicked() {
                state.grid_learn = if learning { None } else { Some(key.clone()) };
            }

            // Its own settings.
            match &role {
                Some(Role::Fn { page: to }) => {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(small(tr("goes to").into()));
                        let target = to.unwrap_or(0);
                        if ui.button(small("‹".into())).clicked() {
                            state.layout.roles.insert(key.clone(), Role::Fn { page: Some(target.saturating_sub(1)) });
                            changed = true;
                        }
                        ui.label(small(match to {
                            Some(p) => trf("page {n}", &[("n", &(p + 1))]),
                            None => tr("the next page").to_string(),
                        }));
                        if ui.button(small("›".into())).clicked() {
                            let next = to.map_or(0, |p| p + 1);
                            state.layout.roles.insert(key.clone(), Role::Fn { page: Some(next) });
                            state.layout.page_count = state.layout.page_count.max(next + 1);
                            changed = true;
                        }
                        if to.is_some() && ui.button(small(tr("next page").into())).clicked() {
                            state.layout.roles.insert(key.clone(), Role::Fn { page: None });
                            changed = true;
                        }
                    });
                }
                Some(Role::Piano { base, keys }) => {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(small(tr("octave").into()));
                        if ui.add_enabled(*base >= 12, egui::Button::new(small("−".into()))).clicked() {
                            state.layout.roles.insert(key.clone(), Role::Piano { base: base - 12, keys: *keys });
                            changed = true;
                        }
                        ui.label(small(format!(
                            "{}–{}",
                            metal_midi::note_name(*base),
                            metal_midi::note_name(base.saturating_add(keys.saturating_sub(1)))
                        )));
                        if ui
                            .add_enabled((*base as i32) + (*keys as i32) + 11 <= 127, egui::Button::new(small("+".into())))
                            .clicked()
                        {
                            state.layout.roles.insert(key.clone(), Role::Piano { base: base + 12, keys: *keys });
                            changed = true;
                        }
                    });
                }
                _ => {}
            }
            if let Some(lp) = state.layout.loops.get(&key).cloned() {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(small(tr("length").into()));
                    if ui.button(small("−".into())).clicked() {
                        state.layout.loops.get_mut(&key).expect("checked").ticks = crate::surface::step_ticks(lp.ticks, -1);
                        changed = true;
                    }
                    ui.label(small(trf("{n} ticks", &[("n", &lp.ticks)])));
                    if ui.button(small("+".into())).clicked() {
                        state.layout.loops.get_mut(&key).expect("checked").ticks = crate::surface::step_ticks(lp.ticks, 1);
                        changed = true;
                    }
                });
                ui.label(small(trf("{n} presses", &[("n", &lp.events.len())])));
            }

            // Remove: a control cell or a loop leaves the layout; a switch
            // goes through its own route.
            let switch_id = match &cell.kind {
                CellKind::Switch { id, .. } | CellKind::Fixture { id, .. } | CellKind::Rig { id, .. } => Some(id.clone()),
                _ => None,
            };
            let removable = role.is_some() || state.layout.loops.contains_key(&key) || switch_id.is_some();
            if removable {
                ui.separator();
                if ui.button(small(tr("remove").into())).clicked() {
                    if let Some(id) = &switch_id {
                        let _ = door.json("POST", "/switch", Some(json!({ "op": "remove", "id": id })));
                    }
                    state.layout.forget(&key);
                    state.grid_spans.remove(&key);
                    state.grid_selected = None;
                    changed = true;
                }
            }
        });
    if !open {
        state.grid_selected = None;
        state.grid_learn = None;
    }
    if changed {
        save_layout(door, state);
    }
}

/// Adds one control cell to the layout, beside the cells already there.
fn add_role(door: &dyn Call, state: &mut PanelState, role: crate::surface::Role) {
    let key = state.layout.fresh_key(role.tag());
    let span = role.default_span();
    state.layout.roles.insert(key.clone(), role);
    save_layout(door, state);
    if span != (1.0, 1.0) {
        set_grid_span(door, state, &key, span);
    }
    state.grid_note = Some(trf("added {cell}", &[("cell", &key)]));
}

/// A nested dropdown entry of the `+` menu: the bare label, its list opening
/// beside it. Inside a menu it is a submenu, else a menu button. The menu's
/// own style (set in [`add_menu`]) keeps the outline off the resting state and
/// a theme-token wash on hover.
fn plain_menu<R>(
    ui: &mut Ui,
    theme: &Theme,
    label: RichText,
    content: impl FnOnce(&mut Ui) -> R,
) -> (Response, Option<R>) {
    let button = egui::Button::new(label.font(theme.font_small.clone())).frame(true);
    let (response, inner) = if egui::containers::menu::is_in_menu(ui) {
        egui::containers::menu::SubMenuButton::from_button(button).ui(ui, content)
    } else {
        egui::containers::menu::MenuButton::from_button(button).ui(ui, content)
    };
    (response, inner.map(|i| i.inner))
}

/// A menu entry with no submenu — the leaf sibling of [`plain_menu`], so
/// `+ create sequence` sits on the same left margin as its two submenu
/// neighbours (`+` → controller button, add loop).
fn plain_item(ui: &mut Ui, theme: &Theme, label: RichText) -> Response {
    // A framed button, so its hover fill paints; the resting fill is clear
    // (set in `add_menu`), so only the hovered row shows a box.
    ui.add(egui::Button::new(label.font(theme.font_small.clone())).frame(true))
}

/// The whole `+` menu: `create sequence`, `controller button` (which opens the
/// one editor window), and `add loop`. Every entry reads on the dark theme —
/// its resting fill the popup's own, its hover the theme's dim phosphor wash.
fn add_menu(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState, open_graph: Option<&str>) {
    // Hover/resting widget fills from theme tokens only: a frame-less button
    // would never show a hover, and the theme's own inactive fill is a box the
    // popup already carries.
    let mut style = (**ui.style()).clone();
    style.visuals.widgets.inactive.weak_bg_fill = theme.transparent();
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    style.visuals.widgets.hovered.weak_bg_fill = theme.phosphor_deep;
    style.visuals.widgets.hovered.bg_stroke = theme.rule_bright_stroke();
    style.visuals.widgets.hovered.expansion = 0.0;
    ui.set_style(style);

    if plain_item(ui, theme, RichText::new(tr("create sequence"))).clicked() {
        state.mode_override = Some(crate::shell::lanes::Mode::Lanes);
        state.mv = crate::shell::lanes::MovementLane::default();
        state.mv.note = Some("new sequence".into());
        state.new_track = true;
        ui.close();
    }
    // `controller button` — opens the one editor, the same window the scene
    // cell's `⚙` and right-click reach, prefilled with the graph's controller.
    match open_graph {
        Some(gid) => {
            if plain_item(ui, theme, RichText::new(tr("nodes control"))).clicked() {
                state.editing_controller = Some(gid.to_string());
                state.controller_edit_pos = Some(ui.cursor().min);
                ui.close();
            }
        }
        None => {
            let _ = plain_item(ui, theme, RichText::new(tr("nodes control")));
            ui.label(
                RichText::new(tr("— open a graph to control its nodes —"))
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
            );
        }
    }
    // `scene` — the same editor, but a new entry flagged `scene`: it lists the
    // open graph's top parents only and captures their floats as they pick.
    if let Some(gid) = open_graph {
        if plain_item(ui, theme, RichText::new(tr("scene"))).clicked() {
            let key = new_scene_key(state, gid);
            state.controllers.insert(
                key.clone(),
                json!({ "graph": gid, "name": "", "mode": "switch", "targets": [], "scene": true }),
            );
            save_controllers(door, state);
            state.editing_controller = Some(key);
            state.controller_edit_pos = Some(ui.cursor().min);
            ui.close();
        }
    }
    // `add loop` — the loop lengths; each opens an empty sequence view limited
    // to that many ticks, with no ingestion (`IngestLane::start_loop`).
    let _ = plain_menu(ui, theme, RichText::new(tr("add loop")), |ui| {
        for ticks in crate::shell::lanes::IngestLane::LOOP_TICKS {
            let label = trf("{n} ticks", &[("n", &ticks)]);
            if ui.selectable_label(false, RichText::new(label).font(theme.font_small.clone())).clicked() {
                state.mode_override = Some(crate::shell::lanes::Mode::Lanes);
                state.new_loop = Some(ticks);
                        ui.close();
            }
        }
    });
    // `controls` — the hardware's own buttons as grid cells: the piano, rec
    // and drop rec, the dials, fn, and the library's search/load/folders.
    let _ = plain_menu(ui, theme, RichText::new(tr("general controls")), |ui| {
        for role in crate::surface::Role::catalogue() {
            let label = tr_dyn(&role.label());
            if ui.selectable_label(false, RichText::new(label).font(theme.font_small.clone())).clicked() {
                add_role(door, state, role);
                ui.close();
            }
        }
    });
}

/// What [`node_folder_row`] reported about the one row it painted.
struct NodeRow {
    /// The collapse chevron was clicked: flip the node's expanded state.
    toggle: bool,
    /// The row NAME was clicked: the caller's pick ran.
    name: bool,
    /// The on/off checkbox holds this value now, when one was drawn and
    /// changed.
    checked: Option<bool>,
}

/// One row of the controller editor's tree: the collapse chevron (only when
/// `expanded` is `Some`, i.e. the node/group actually has a sublist), then the
/// title, then — at the RIGHT end, only when `on` is `Some` (a boolean target)
/// — the on/off checkbox. `picked` highlights the row (a target of any shape,
/// a palette/colour one included); it is separate from `on`, which only a
/// boolean target carries. Clicking the name is the caller's pick; the
/// chevron only toggles expansion; the checkbox only the stored `on`.
fn node_folder_row(
    ui: &mut Ui,
    theme: &Theme,
    title: &str,
    expanded: Option<bool>,
    red: bool,
    picked: bool,
    on: Option<bool>,
) -> NodeRow {
    let mut row = NodeRow { toggle: false, name: false, checked: None };
    ui.horizontal(|ui| {
        if let Some(expanded) = expanded {
            let icon = if expanded { Icon::ChevronUp } else { Icon::ChevronDown };
            let chevron = ui.add(egui::Button::new(lucide(icon, theme.font_small.size)).frame(false));
            let what = if expanded { "collapse" } else { "expand" };
            chevron.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{what} {title}"))
            });
            if chevron.clicked() {
                row.toggle = true;
            }
        }
        let text = if red {
            RichText::new(format!("● {title}")).font(theme.font_small.clone()).color(theme.accent_vivid)
        } else {
            RichText::new(title).font(theme.font_small.clone())
        };
        if ui.selectable_label(picked, text).clicked() {
            row.name = true;
        }
        if let Some(current) = on {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut checked = current;
                let response = crate::shell::components::toggle_row(ui, theme, "", &mut checked);
                if response.changed() {
                    row.checked = Some(checked);
                }
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Checkbox, true, format!("on {title}"))
                });
            });
        }
    });
    row
}

/// The one controller editor, reached three ways with the same payload: the
/// `+ › controller button` item, the scene cell's `⚙`, and a right-click on the
/// scene cell — all of them open [`controller_editor_popup`] on this content.
/// Top to bottom: the
/// **name** field, the **Preview** `switch`/`hold` mode, the **size** spans
/// (`grid_spans[ctrl:<graph>]`), the collapsible node/fixture/group/param list,
/// and — editing an existing controller — its **delete**. Prefilled from the
/// controller's whole payload, so editing changes a button rather than starting
/// again.
fn controller_editor(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState, key: &str) {
    // A free grid's controller is keyed by its graph; a box's control or a
    // piano key names the graph it reaches.
    let graph = state.controllers.get(key).and_then(|c| c["graph"].as_str()).unwrap_or(key).to_string();
    let graph = graph.as_str();
    controller_name_field(ui, theme, door, state, key);
    controller_mode_header(ui, theme, door, state, key);
    // A free grid's controller takes its input signal the same way a bound
    // cell does (the next control heard is bound to it).
    if !is_hw_controller(key) {
        let cell = format!("ctrl:{}", controller_key(key));
        let learning = state.grid_learn.as_deref() == Some(cell.as_str());
        if ui.selectable_label(learning, RichText::new(tr("learn input signal")).font(theme.font_small.clone())).clicked() {
            state.grid_learn = if learning { None } else { Some(cell) };
        }
    }
    // A box's controls are the size the panel has them.
    if !is_hw_controller(key) && state.layout.device.is_none() {
        let span_key = format!("ctrl:{key}");
        let current = state.grid_spans.get(&span_key).copied().unwrap_or((1.0, 1.0));
        if let Some(span) = size_choices(ui, theme, current) {
            set_grid_span(door, state, &span_key, span);
        }
    }
    // A box's control can trade its button for one of the surface's jobs.
    if let Some(slot) = slot_of_controller(state, key) {
        if job_menu(ui, theme, door, state, slot) {
            state.editing_controller = None;
            return;
        }
    }
    ui.separator();
    controller_list(ui, theme, door, state, key, graph);
    ui.separator();
    // `save` commits the name (and is one more way out beside a click
    // outside); `delete` removes the whole controller from the grid.
    ui.horizontal(|ui| {
        if ui
            .button(RichText::new(tr("save")).font(theme.font_small.clone()))
            .clicked()
        {
            commit_controller_name(door, state, key);
            state.editing_controller = None;
        }
        // A scene's `capture again` re-reads its floats from the nodes now.
        if is_scene(state, key)
            && ui.button(RichText::new(tr("capture again")).font(theme.font_small.clone())).clicked()
        {
            capture_scene_again(door, state, key);
        }
        if state.controllers.contains_key(&controller_key(key))
            && ui
                .button(RichText::new(tr("delete controller")).font(theme.font_small.clone()))
                .clicked()
        {
            delete_controller(door, state, key);
            state.editing_controller = None;
        }
    });
}

/// Commits the editor's name buffer to `graph`'s controller. An empty name
/// only writes when the controller already exists — an untouched empty editor
/// must not mint a nameless button.
fn commit_controller_name(door: &dyn Call, state: &mut PanelState, graph: &str) {
    let name = state.controller_name_buffer.trim().to_string();
    if !name.is_empty() || state.controllers.contains_key(&controller_key(graph)) {
        set_controller_name(door, state, graph, &name);
    }
}

/// The editor's name field, seeded from `graph`'s controller the first time it
/// is painted for that graph (so an untouched buffer follows a switch, an
/// edited one is left alone). Saving is Enter, a blur once the buffer differs
/// from the stored name, or the editor's `save` button ([`commit_controller_name`]);
/// an empty name still creates the button once a target exists.
fn controller_name_field(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState, graph: &str) {
    if state.controller_name_for.as_deref() != Some(graph) {
        state.controller_name_buffer = state
            .controllers
            .get(&controller_key(graph))
            .and_then(|c| c["name"].as_str())
            .unwrap_or("")
            .to_string();
        state.controller_name_for = Some(graph.to_string());
    }
    ui.horizontal(|ui| {
        let resp = ui.add(
            crate::shell::components::header_edit(theme, &mut state.controller_name_buffer, tr("controller name"))
                .id(Id::new("shell/region/lanes/controller-name")),
        );
        let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        // A blur commits even when `lost_focus` did not fire (a click on a
        // non-focusable row can leave the edit focused on some platforms):
        // the buffer differs from what is stored.
        let stored = state
            .controllers
            .get(&controller_key(graph))
            .and_then(|c| c["name"].as_str())
            .unwrap_or("")
            .to_string();
        let dirty = state.controller_name_buffer.trim() != stored.trim();
        if enter || (!resp.has_focus() && dirty) {
            commit_controller_name(door, state, graph);
        }
    });
}

/// The `controller button` editor's mode row: a `Hold` / `Switcher` dropdown,
/// shown whenever the button's payload is not a fader (a fader writes on its
/// own drag and has no press). The pick is stored on the graph's controller
/// and the grid cell honours it (`paint_controller_cell`).
fn controller_mode_header(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState, gid: &str) {
    if matches!(controller_face(door, state, &controller_key(gid)), ControllerFace::Fader { .. }) {
        return;
    }
    let mut mode = controller_mode(state, gid);
    ui.horizontal(|ui| {
        ui.label(RichText::new(tr("mode")).font(theme.font_small.clone()).color(theme.ink_weak));
        let options = [("hold".to_string(), "Hold"), ("switch".to_string(), "Switcher")];
        if crate::shell::components::dropdown(ui, theme, "shell/region/lanes/controller-mode", &mut mode, &options)
            .changed()
        {
            set_controller_mode(door, state, gid, mode == "hold");
        }
    });
    ui.separator();
}

/// The editor's unified list: the open graph's nodes as IDE-style rows — a
/// chevron only when the node has float parameters, which expand to one fader
/// row each — then the venue's fixtures and groups in the same list: a group a
/// folder row over its fixtures, an ungrouped fixture a peer row. Every pick
/// **toggles membership** of the controller's targets; a node/group/fixture row
/// that is a target carries its `on` checkbox. Params are not booleans and
/// carry the grid's one fader instead.
fn controller_list(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState, key: &str, graph: &str) {
    let scene = is_scene(state, key);
    let graph_json = get(door, "GET", "/graph", &[("id", graph.to_string())]).unwrap_or(Value::Null);
    // A scene lists only its top parents: the nodes no cable feeds.
    let fed = fed_nodes(&graph_json);
    let nodes: Vec<(String, Option<String>, String, Value)> = graph_json["graph"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            Some((
                n["id"].as_str()?.to_string(),
                n["label"].as_str().filter(|l| !l.is_empty()).map(str::to_string),
                n["kind"].as_str().unwrap_or_default().to_string(),
                n["config"].clone(),
            ))
        })
        .filter(|(id, ..)| !scene || !fed.contains(id))
        .collect();
    if nodes.is_empty() {
        ui.label(RichText::new(tr("— the graph has no nodes —")).font(theme.font_small.clone()).color(theme.ink_weak));
    }
    let assigned = assigned_nodes(door);
    for (id, label, kind, config) in nodes {
        let title = label.unwrap_or_else(|| crate::shell::naming::display_name(&kind));
        // The node's own `GET /graph/io`: its float params are what a chevron
        // reveals, so it is read before the row is drawn (a node with none
        // gets no chevron at all — `gen.*` floats included).
        let io = get(door, "GET", "/graph/io", &[("id", graph.to_string()), ("node", id.clone())]).unwrap_or(Value::Null);
        let floats = super::dials::floats(&io);
        if scene {
            if !floats.is_empty() {
                scene_node_row(ui, theme, door, state, key, graph, &id, &title, &floats, assigned.contains(&id));
            }
            continue;
        }
        let expanded = state.expanded_nodes.contains(&id);
        // A palette/colour node is not a boolean: its pick captures the
        // node's own colours as a `{node, colors|rgb}` target the button
        // recalls through `POST /graph/node {op:"config"}` — node
        // conventions, never the global colour rule. Every other node picks
        // the boolean `{node, on: true}`; the stored `on` wins when it is
        // already a target (point 4/7).
        let colour = colour_target(&kind, &id, &config);
        let is_colour = colour.is_some();
        let target = colour.unwrap_or_else(|| json!({ "node": id, "on": true }));
        let picked = controller_has_target(state, key, &target);
        let on = (!is_colour).then(|| controller_target_on(state, key, &target));
        let row = node_folder_row(
            ui,
            theme,
            &title,
            (!floats.is_empty()).then_some(expanded),
            assigned.contains(&id),
            picked,
            on,
        );
        if row.toggle {
            if expanded {
                state.expanded_nodes.remove(&id);
            } else {
                state.expanded_nodes.insert(id.clone());
            }
        }
        if row.name {
            toggle_controller_target(door, state, key, target.clone());
        }
        if let Some(on) = row.checked {
            set_controller_target_on(door, state, key, &target, on);
        }
        let expanded = if row.toggle { !expanded } else { expanded };
        if !expanded || floats.is_empty() {
            continue;
        }
        ui.indent(("shell/region/lanes/controller-node", id.as_str()), |ui| {
            for p in &floats {
                controller_param_row(ui, theme, door, state, key, graph, &id, p);
            }
        });
    }
    // The venue half of the same list: a group folder row over its fixtures,
    // an ungrouped fixture a peer row beside the node rows. A scene holds
    // floats only, so it has no venue half.
    if !scene {
        fixture_controller_rows(ui, theme, door, state, key, graph);
    }
}

/// One top parent row of a scene's editor: the chevron (when it has floats),
/// the name — a pick captures or drops all its floats — and, expanded, the same
/// [`controller_param_row`] each float has in a controller. No `on` checkbox: a
/// scene's floats are values, not booleans.
#[allow(clippy::too_many_arguments)]
fn scene_node_row(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    key: &str,
    graph: &str,
    id: &str,
    title: &str,
    floats: &[super::dials::Float],
    assigned: bool,
) {
    let expanded = state.expanded_nodes.contains(id);
    let picked = scene_has_node(state, key, id);
    let row = node_folder_row(ui, theme, title, Some(expanded), assigned, picked, None);
    if row.toggle {
        if expanded {
            state.expanded_nodes.remove(id);
        } else {
            state.expanded_nodes.insert(id.to_string());
        }
    }
    if row.name {
        toggle_scene_node(door, state, key, graph, id);
    }
    let expanded = if row.toggle { !expanded } else { expanded };
    if !expanded {
        return;
    }
    ui.indent(("shell/region/lanes/controller-node", id), |ui| {
        for p in floats {
            controller_param_row(ui, theme, door, state, key, graph, id, p);
        }
    });
}

/// One parameter row of the editor: the name (a pick that toggles the
/// `{node, param, value}` target) and the grid's one fader
/// ([`builtins::fader`], horizontal) editing the value. A drag posts
/// `{op:"param"}` live and updates the stored value; the release saves the
/// controller. Params are never booleans, so no checkbox.
#[allow(clippy::too_many_arguments)]
fn controller_param_row(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    key: &str,
    graph: &str,
    node: &str,
    p: &super::dials::Float,
) {
    let target = json!({ "node": node, "param": p.name, "value": p.value });
    let picked = controller_has_target(state, key, &target);
    let stored = state
        .controllers
        .get(&controller_key(key))
        .and_then(|c| c["targets"].as_array())
        .and_then(|ts| ts.iter().find(|t| same_target(t, &target)))
        .and_then(|t| t["value"].as_f64())
        .map(|v| v as f32)
        .unwrap_or(p.value);
    ui.horizontal(|ui| {
        let text = if picked {
            RichText::new(format!("● {}", p.name)).font(theme.font_small.clone()).color(theme.accent_vivid)
        } else {
            RichText::new(&p.name).font(theme.font_small.clone())
        };
        if ui.selectable_label(picked, text).clicked() {
            toggle_controller_target(door, state, key, target.clone());
        }
        let range = (p.max - p.min).max(0.0);
        let frac = if range > 0.0 { ((stored - p.min) / range).clamp(0.0, 1.0) } else { 0.0 };
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(theme.control_width, theme.compact_row_height),
            Sense::hover(),
        );
        let f = builtins::fader(
            ui,
            theme,
            ui.id().with(("controller-param", node, p.name.as_str())),
            "",
            frac,
            rect,
            true,
            Sense::click_and_drag(),
        );
        if f.response.dragged() || f.response.clicked() || f.response.changed() {
            let value = p.min + f.value * range;
            let _ = door.json(
                "POST",
                "/graph/node",
                Some(json!({ "op": "param", "graph": graph, "node": node, "name": p.name, "value": value })),
            );
            set_controller_target_value(state, key, &target, value);
        }
        if f.response.drag_stopped() {
            save_controllers(door, state);
        }
        ui.label(
            RichText::new(format!("{stored:.2}"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    });
}

/// The `{node, colors}` / `{node, rgb}` a **palette**/**color** node pick
/// captures: the node's own colours, frozen into the target so the button can
/// recall them through `POST /graph/node {op:"config"}` — node conventions,
/// no global colour rule. `None` for every other kind (and for a colour node
/// with no colours yet), so the row falls back to the boolean target.
fn colour_target(kind: &str, node: &str, config: &Value) -> Option<Value> {
    match kind {
        "palette" => {
            let colors = config["colors"].as_array()?;
            (!colors.is_empty()).then(|| json!({ "node": node, "colors": colors }))
        }
        "color" => {
            let rgb = config["rgb"].as_array()?;
            (!rgb.is_empty()).then(|| json!({ "node": node, "rgb": rgb }))
        }
        _ => None,
    }
}

/// The node ids a saved switch in the collection already drives — the red dot
/// [`controller_list`] leads with. Read fresh off `GET /switch` (rule 4).
fn assigned_nodes(door: &dyn Call) -> std::collections::BTreeSet<String> {
    get(door, "GET", "/switch", &[])
        .and_then(|v| v["switches"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|sw| sw["set"].as_array().cloned())
        .flatten()
        .filter_map(|e| e["node"].as_str().map(str::to_string))
        .collect()
}

/// The venue half of [`controller_list`]: every `GET /venue` group as a folder
/// row (its fixtures nested under it) and every ungrouped fixture as a peer
/// row. A fixture pick toggles a `{node, fixture, on}` target on the resolved
/// `venue.fixtures` node; a group pick toggles a `{node, group, on}` one.
/// `expanded_nodes` holds the group rows' collapse state (keyed `group:<name>`
/// so it cannot collide with a node id); a group starts open and always has
/// fixtures, so it always carries a chevron.
fn fixture_controller_rows(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState, ctrl: &str, gid: &str) {
    let fixtures = venue_fixture_list(door);
    if fixtures.is_empty() {
        return;
    }
    let Some(venue) = get(door, "GET", "/venue", &[("id", "last".to_string())]) else {
        return;
    };
    let venue_id = venue["venue"]["venue"]["id"].as_str().unwrap_or_default().to_string();
    let nodes = venue_fixture_nodes(
        &get(door, "GET", "/graph", &[("id", gid.to_string())]).unwrap_or(Value::Null),
    );
    // One fixture leaf: a row whose pick toggles the target on its resolved
    // node, carrying the on/off checkbox when it is already a target.
    let fixture_row = |ui: &mut Ui, state: &mut PanelState, id: &str, name: &str, group: Option<&str>| {
        let Some(node) = fixture_button_target(&nodes, &venue_id, Some(id), group, None) else {
            return;
        };
        // A fixture target names the fixture itself (the node it resolved to
        // already encodes the group), the same shape the old pick made.
        let target = json!({ "node": node, "fixture": id, "on": true });
        let picked = controller_has_target(state, ctrl, &target);
        let on = controller_target_on(state, ctrl, &target);
        let row = node_folder_row(ui, theme, &format!("◉ {name}"), None, false, picked, picked.then_some(on));
        if row.name {
            toggle_controller_target(door, state, ctrl, target.clone());
        }
        if let Some(on) = row.checked {
            set_controller_target_on(door, state, ctrl, &target, on);
        }
    };
    let mut groups: Vec<String> = fixtures.iter().filter_map(|(_, _, g, _)| g.clone()).collect();
    groups.sort();
    groups.dedup();
    for group in &groups {
        let key = format!("group:{group}");
        // A group starts **open** — its fixtures are what the operator came
        // for — so membership means collapsed here, the mirror of a node's
        // own default.
        let collapsed = state.expanded_nodes.contains(&key);
        let open = !collapsed;
        let target = group_target(&nodes, &venue_id, group);
        let picked = target.as_ref().is_some_and(|t| controller_has_target(state, ctrl, t));
        let on = target.as_ref().is_some_and(|t| controller_target_on(state, ctrl, t));
        let row = node_folder_row(ui, theme, &format!("▣ {group}"), Some(open), false, picked, picked.then_some(on));
        if row.toggle {
            if collapsed {
                state.expanded_nodes.remove(&key);
            } else {
                state.expanded_nodes.insert(key.clone());
            }
        }
        if row.name {
            if let Some(t) = &target {
                toggle_controller_target(door, state, ctrl, t.clone());
            }
        }
        if let (Some(on), Some(t)) = (row.checked, &target) {
            set_controller_target_on(door, state, ctrl, t, on);
        }
        let open = if row.toggle { !open } else { open };
        if !open {
            continue;
        }
        ui.indent(("shell/region/lanes/controller-group", group.as_str()), |ui| {
            for (id, name, _, _) in fixtures.iter().filter(|(_, _, g, _)| g.as_deref() == Some(group.as_str())) {
                fixture_row(ui, state, id, name, Some(group));
            }
        });
    }
    // A fixture with no group is a peer row: the list is one level, the
    // groups only nest their own members.
    for (id, name, _, _) in fixtures.iter().filter(|(_, _, g, _)| g.is_none()) {
        fixture_row(ui, state, id, name, None);
    }
}

/// The `{node, group, on}` a group row toggles — the group's own
/// `venue.fixtures` node. `None` when the graph has none.
fn group_target(
    nodes: &[(String, String, Option<String>, Option<String>)],
    venue_id: &str,
    group: &str,
) -> Option<Value> {
    fixture_button_target(nodes, venue_id, None, None, Some(group)).map(|node| {
        json!({ "node": node, "group": group, "on": true })
    })
}

/// The fixture nodes of a `GET /graph` reply as `(node id, venue, group,
/// fixture)` — what a G1 button's `set` can name. F12 adds the singular
/// `venue.fixture` nodes: a group node carries `group`, a per-fixture node
/// carries `fixture`; exactly one of the two is ever set.
fn venue_fixture_nodes(graph: &Value) -> Vec<(String, String, Option<String>, Option<String>)> {
    graph["graph"]["nodes"]
        .as_array()
        .map(|ns| {
            ns.iter()
                .filter_map(|n| {
                    let kind = n["kind"].as_str()?;
                    let id = n["id"].as_str()?.to_string();
                    let venue = n["config"]["venue"].as_str().unwrap_or_default().to_string();
                    match kind {
                        "venue.fixtures" => {
                            let group = n["config"]["group"].as_str().map(str::to_string);
                            Some((id, venue, group, None))
                        }
                        "venue.fixture" => {
                            let fixture = n["config"]["fixture"].as_str().map(str::to_string);
                            Some((id, venue, None, fixture))
                        }
                        _ => None,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The node a G1 button drives (F12): a `rig` names its group's node exactly;
/// a `fixture` prefers its **own** `venue.fixture` node, then its group's
/// `venue.fixtures` node, then the venue-wide node (`group: null`). `None`
/// when the graph has none of them.
fn fixture_button_target(
    nodes: &[(String, String, Option<String>, Option<String>)],
    venue: &str,
    fixture_id: Option<&str>,
    fixture_group: Option<&str>,
    rig_group: Option<&str>,
) -> Option<String> {
    if let Some(g) = rig_group {
        return nodes
            .iter()
            .find(|n| n.1 == venue && n.2.as_deref() == Some(g))
            .map(|n| n.0.clone());
    }
    if let Some(fid) = fixture_id {
        if let Some(n) = nodes
            .iter()
            .find(|n| n.1 == venue && n.3.as_deref() == Some(fid))
        {
            return Some(n.0.clone());
        }
    }
    if let Some(fg) = fixture_group {
        if let Some(n) = nodes
            .iter()
            .find(|n| n.1 == venue && n.2.as_deref() == Some(fg))
        {
            return Some(n.0.clone());
        }
    }
    nodes
        .iter()
        .find(|n| n.1 == venue && n.2.is_none())
        .map(|n| n.0.clone())
}

/// 2026-09-22 (afternoon): the popover a dropped **video** opens. One step —
/// pick the destination off the same `GET /state` listing the grid already
/// resolved (every output, and each output's own `masks[]`), then `create`
/// turns the drop into a saved button: a `reel` source node for the video in
/// the open graph, and a `toggle` switch over it (`POST /graph/node
/// {op:"add"}` + `{op:"config"}`, then `POST /switch {op:"add"}`). The chosen
/// destination rides in the switch's own name — "video: <file> → <dest>" —
/// so the cell says where it fires without a route that does not exist yet.
///
/// Without an open graph the popover says so and offers no `create`: the
/// switch's `graph` needs one.
fn create_video_popover(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    state_val: &Value,
    open_graph: Option<&str>,
) {
    if state.new_video.is_none() {
        return;
    }
    let mut close = false;
    let mut create = false;
    let mut open = true;
    egui::Window::new("new video button")
        .id(Id::new("shell/region/lanes/new-video"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            let Some(draft) = state.new_video.as_mut() else { return };
            ui.set_min_width(theme.text_input_width * 2.0);
            ui.label(
                RichText::new(format!("video: {}", draft.name))
                    .font(theme.font_small.clone())
                    .color(theme.ink),
            );
            // The destinations: each output, and each output's own masks.
            let mut options: Vec<(String, String, bool)> = Vec::new();
            for o in state_val["outputs"].as_array().cloned().unwrap_or_default() {
                if let Some(id) = o["id"].as_str() {
                    options.push((id.to_string(), id.to_string(), false));
                    for m in o["masks"].as_array().cloned().unwrap_or_default() {
                        if let Some(mid) = m["id"].as_str() {
                            options.push((mid.to_string(), format!("{id} · mask {mid}"), true));
                        }
                    }
                }
            }
            ui.label(
                RichText::new("target (output or mask)")
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
            );
            egui::ScrollArea::vertical()
                .max_height(theme.overlay_cell.y * 4.0)
                .id_salt("shell/region/lanes/new-video-targets")
                .show(ui, |ui| {
                    for (id, label, is_mask) in &options {
                        if ui
                            .selectable_label(draft.target == *id, RichText::new(label).font(theme.font_small.clone()))
                            .clicked()
                        {
                            draft.target = id.clone();
                            draft.mask = *is_mask;
                        }
                    }
                });
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(RichText::new("create").font(theme.font_small.clone()))
                    .clicked()
                {
                    create = true;
                }
                if ui
                    .button(RichText::new("cancel").font(theme.font_small.clone()))
                    .clicked()
                {
                    close = true;
                }
            });
        });
    if create {
        if let Some(draft) = state.new_video.clone() {
            match create_video_button(door, open_graph, &draft) {
                Ok(name) => {
                    state.mv.note = Some(format!("button created: {name}"));
                    close = true;
                }
                Err(msg) => state.mv.note = Some(msg),
            }
        }
    }
    if close || !open {
        state.new_video = None;
    }
}

/// The write half of [`create_video_popover`]: adds a `reel` node with the
/// dropped video's path and a `toggle` switch over it, the switch's name
/// naming the destination. `graph` is the open graph's id; `None` is the
/// refusal the popover already spells out.
fn create_video_button(
    door: &dyn Call,
    graph: Option<&str>,
    draft: &NewVideo,
) -> Result<String, String> {
    let gid = graph.ok_or_else(|| "— open a graph for the video button —".to_string())?;
    let add = json!({ "op": "add", "graph": gid, "kind": "reel.clip", "pos": [0.0, 0.0] });
    let reply = door
        .json("POST", "/graph/node", Some(add))
        .map_err(|(_, m)| m)?;
    let node = reply["node"]
        .as_str()
        .ok_or_else(|| "the graph did not return the node".to_string())?
        .to_string();
    let patch = json!({
        "op": "config", "graph": gid, "node": node, "config": { "file": draft.path },
    });
    door.post_async("/graph/node", patch);
    let dest = if draft.mask {
        format!("mask {}", draft.target)
    } else {
        draft.target.clone()
    };
    let name = format!("video: {} → {dest}", draft.name);
    let body = json!({
        "op": "add", "id": format!("sw-video-{node}"), "name": name,
        "kind": "toggle", "graph": gid,
        "set": [{ "node": node, "on": true }],
    });
    door.json("POST", "/switch", Some(body)).map_err(|(_, m)| m)?;
    Ok(name)
}

/// The grid's fixed content row (`R22.3` decision 2): `blackout`,
/// `identificar` and `stop all` — controls, not content, so they lead the
/// grid and never move with what is saved.
///
/// The one `+` that grows the grid no longer lives here: it sits in the page
/// row beside the next-page button (see [`overlays_grid`]), and opens the
/// `+ create sequence` / `+ controller button` / `+ add loop` menu
/// (`CellKind::Add`).
///

/// One cell per movement in the library, read fresh each frame — never the
/// ticks themselves, only the count sizes the progress bar. A movement in
/// the air is the lit one; its tick is the progress bar at the foot.
/// `playhead_for` is the one place the `/state` overlay list is read (rule
/// 5). The empty-library line this used to also report is gone (2026-09-22,
/// afternoon): the grid's own phantom squares show it is empty now.
fn movement_cells(
    door: &dyn Call,
    state_val: &Value,
    device: Option<&str>,
    layer: Option<&str>,
) -> Vec<Cell> {
    let listing = get(door, "GET", "/author/models/library", &[]);
    let movements = listing
        .as_ref()
        .and_then(|v| v["movements"].as_array().cloned())
        .unwrap_or_default();
    movements
        .iter()
        .filter_map(|mv| {
            let name = mv["name"].as_str()?.to_string();
            let ticks = mv["ticks"].as_u64().unwrap_or(0).max(1);
            let star = if mv["favourite"].as_bool().unwrap_or(false) {
                " ★"
            } else {
                ""
            };
            let running_tick = playhead_for(&name, state_val);
            // `P5`: the sequence's own trigger decides where it plays;
            // without one, the first output / first layer / not-looping
            // are the defaults.
            let (dev, lay, looping) = trigger_target(state_val, &name).unwrap_or_else(|| {
                (
                    device.unwrap_or_default().to_string(),
                    layer.unwrap_or_default().to_string(),
                    false,
                )
            });
            Some(Cell {
                kind: CellKind::Movement {
                    name: name.clone(),
                    device: dev,
                    layer: lay,
                    looping,
                },
                key: format!("mv:{name}"),
                span: (1.0, 1.0),
                label: format!("{name}{star}"),
                on: false,
                running: running_tick.is_some(),
                progress: running_tick.map(|tick| (tick / ticks as f32).clamp(0.0, 1.0)),
            })
        })
        .collect()
}

/// `S.O.4a`: one cell per saved `POST /sequences {op:"save"}` entry, off
/// `GET /sequences` — the grid's own sequences, distinct from the movement
/// library. Click arms `run` on the panel's current device; a running
/// sequence's button is accented, and a second click `stop`s it.
fn sequences_cells(door: &dyn Call, device: Option<&str>) -> Vec<Cell> {
    let listing = get(door, "GET", "/sequences", &[]);
    let sequences = listing
        .as_ref()
        .and_then(|v| v["sequences"].as_array().cloned())
        .unwrap_or_default();
    let running = listing
        .as_ref()
        .and_then(|v| v["running"].as_array().cloned())
        .unwrap_or_default();
    sequences
        .iter()
        .filter_map(|seq| {
            let id = seq["id"].as_str()?;
            let name = seq["name"].as_str().unwrap_or(id);
            let is_running = running.iter().any(|r| r["sequence"].as_str() == Some(id));
            Some(Cell {
                kind: CellKind::Sequence {
                    id: id.to_string(),
                    device: device.unwrap_or_default().to_string(),
                },
                key: format!("seq:{id}"),
                span: (1.0, 1.0),
                label: name.to_string(),
                on: false,
                running: is_running,
                progress: None,
            })
        })
        .collect()
}

/// `S.AK`/`O.28`: one cell per saved switch, off `GET /switch` — "the grid
/// does not tell movement, sequence, blackout or switch apart except by the
/// icon: it is a button grid" (`O.28` point 4). `toggle` fires on `clicked()`,
/// the same gesture a sequence button already uses; `hold` fires on the
/// press/release edge of `is_pointer_button_down_on()` —
/// [`PanelState::held_switches`] is what turns the level that primitive
/// reports into the one `on`/`off` post each edge deserves.
fn switches_cells(door: &dyn Call) -> Vec<Cell> {
    let switches = get(door, "GET", "/switch", &[])
        .and_then(|v| v["switches"].as_array().cloned())
        .unwrap_or_default();
    switches
        .iter()
        .filter_map(|sw| {
            let id = sw["id"].as_str()?;
            let name = sw["name"].as_str()?;
            let kind = sw["kind"].as_str().unwrap_or("toggle");
            // G1: a fixture/rig button keeps the switch's own key and id (so
            // its size persists in `grid_spans` and it fires through
            // `POST /switch`), but paints its own glyph and label — the
            // fixture name — and reads its target from `fixture`/`group`.
            // G3: `mode = "hold"` makes it fire on the press/release edge.
            let hold = sw["mode"].as_str() == Some("hold");
            let (cell_kind, label) = match kind {
                "fixture" => (
                    CellKind::Fixture { id: id.to_string(), hold },
                    format!("◉ {name}"),
                ),
                "rig" => (
                    CellKind::Rig { id: id.to_string(), hold },
                    format!("▣ {name}"),
                ),
                _ => (
                    CellKind::Switch {
                        id: id.to_string(),
                        kind: kind.to_string(),
                    },
                    name.to_string(),
                ),
            };
            Some(Cell {
                kind: cell_kind,
                key: format!("sw:{id}"),
                span: (1.0, 1.0),
                label,
                on: sw["on"].as_bool().unwrap_or(false),
                running: false,
                progress: None,
            })
        })
        .collect()
}

/// **One scene cell per saved controller** (`state.controllers`): the whole
/// stored state behind one button. Its label is the controller's `name` (the
/// graph id, else the key, when it has none) and its stable key is
/// `ctrl:<graph>`, so `grid_spans` sizes it like every other non-fixed cell.
fn controller_cells(state: &PanelState) -> Vec<Cell> {
    // A piano key's button lives in its key, not in the grid.
    state
        .controllers
        .keys()
        .filter(|key| !key.starts_with("key:"))
        .map(|key| Cell {
            kind: CellKind::Controller { key: key.clone() },
            key: format!("ctrl:{key}"),
            span: (1.0, 1.0),
            label: controller_label(state, key),
            on: state.lit_controllers.contains(key),
            running: false,
            progress: None,
        })
        .collect()
}

/// The controller a piano key holds on `page`.
fn key_controller(note: u8, page: usize) -> String {
    format!("key:{note}:{page}")
}

/// The controller a box's control (slot `id`) holds on `page`.
fn slot_controller(id: &str, page: usize) -> String {
    format!("hw:{id}:{page}")
}

/// Whether a controller belongs to a box's control or a piano key rather
/// than to the free grid.
fn is_hw_controller(key: &str) -> bool {
    key.starts_with("hw:") || key.starts_with("key:")
}

/// The cell `key` names: one the grid laid out, or a piano key's button
/// (never laid out, fired from its key).
fn find_cell(state: &PanelState, cells: &[Cell], key: &str) -> Option<Cell> {
    if let Some(c) = cells.iter().find(|c| c.key == key) {
        return Some(c.clone());
    }
    let ctrl = key.strip_prefix("ctrl:").filter(|k| k.starts_with("key:") && state.controllers.contains_key(*k))?;
    Some(Cell {
        kind: CellKind::Controller { key: ctrl.to_string() },
        key: key.to_string(),
        span: (1.0, 1.0),
        label: controller_label(state, ctrl),
        on: false,
        running: false,
        progress: None,
    })
}

/// Drops the controllers behind cells a slot let go of (`ctrl:<key>`).
fn drop_controllers(door: &dyn Call, state: &mut PanelState, cells: &[String]) {
    let mut any = false;
    for c in cells {
        if let Some(k) = c.strip_prefix("ctrl:").filter(|k| is_hw_controller(k)) {
            any |= state.controllers.remove(k).is_some();
        }
    }
    if any {
        save_controllers(door, state);
    }
}

/// The playhead tick for the running overlay whose `movement == name` — the
/// `/state` report's `tick` field, exactly what `overlay::Play::describe`
/// resolved (rule 5). `None` when nothing by that name is in the air.
fn playhead_for(name: &str, state_val: &Value) -> Option<f32> {
    state_val["overlays"]
        .as_array()?
        .iter()
        .find(|ov| ov["movement"].as_str() == Some(name))
        .and_then(|ov| ov["tick"].as_u64())
        .map(|t| t as f32)
}

/// The `(device, layer, loop)` a sequence's own trigger carries, read off
/// `/state`'s per-screen `triggers` (`P5`, rule 5). `None` when there is no
/// trigger yet — the grid falls back to the first output / first layer /
/// not-looping.
fn trigger_target(state_val: &Value, name: &str) -> Option<(String, String, bool)> {
    let target = format!("overlay-{name}");
    for o in state_val["outputs"].as_array()? {
        if let Some(t) = o["triggers"]
            .as_array()
            .and_then(|ts| ts.iter().find(|t| t["target"] == json!(target)))
        {
            let p = &t["params"];
            let device = p["device"].as_str().unwrap_or_default().to_string();
            let layer = p["layer"].as_str().unwrap_or_default().to_string();
            let looping = p["loop"].as_bool().unwrap_or(false);
            if !device.is_empty() && !layer.is_empty() {
                return Some((device, layer, looping));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// the door, GET with a query — identical helper to `nodes.rs`'s own `get`
// ---------------------------------------------------------------------------

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
        Some(reply) if reply.status == 200 => crate::json_memo::parse(&reply.body).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    /// A keyframe row of another width than the schema's is fitted to it, so
    /// the renderer does not refuse it and leave the old picture up.
    #[test]
    fn a_row_is_fitted_to_the_schema_width() {
        assert_eq!(fit_width(vec![1.0, 2.0, 3.0], 2), vec![1.0, 2.0]);
        assert_eq!(fit_width(vec![1.0], 3), vec![1.0, 0.0, 0.0]);
        assert_eq!(fit_width(vec![1.0], 0), vec![1.0], "an unknown width leaves the row alone");
    }

        use crate::shell::components::{glyph, Icon};
    use super::*;
    use crate::shell::{FakeDoor, Shell};
    use egui_kittest::kittest::Queryable;

    /// The same harness `shell::kittest::harness_with_door` builds — the
    /// desktop viewport, the house theme pushed into the context — built
    /// here rather than reached through that module because this panel's
    /// tests live beside the panel (the same place `router_modes_matches…`
    /// already sits), not in the shared file. Every test in this module
    /// asks for its own height (`movements`/the dataset section both run
    /// past the default viewport's fold) rather than the plain
    /// `DESKTOP_VIEWPORT` default, so there is no bare `shell_harness` any
    /// more — only this one, parameterised.
    fn shell_harness_of_height(
        door: Box<dyn Call>,
        height: f32,
    ) -> egui_kittest::Harness<'static, Shell> {
        let theme = Theme::phosphor();
        let size = egui::Vec2::new(crate::shell::DESKTOP_VIEWPORT.0 as f32, height);
        let harness = egui_kittest::Harness::builder()
            .with_size(size)
            .build_ui_state(
                |ui, shell: &mut Shell| shell.ui(ui),
                Shell::new(theme.clone(), door),
            );
        theme.apply(&harness.ctx);
        harness
    }

    /// [`shell_harness_of_height`] with the width chosen too — the §2 toolbar
    /// test paints at a narrow viewport to prove the rows wrap instead of
    /// scrolling sideways.
    fn shell_harness_of_size(
        door: Box<dyn Call>,
        width: f32,
        height: f32,
    ) -> egui_kittest::Harness<'static, Shell> {
        let theme = Theme::phosphor();
        let size = egui::Vec2::new(width, height);
        let harness = egui_kittest::Harness::builder()
            .with_size(size)
            .build_ui_state(
                |ui, shell: &mut Shell| shell.ui(ui),
                Shell::new(theme.clone(), door),
            );
        theme.apply(&harness.ctx);
        harness
    }

    fn body_of(params: &Params) -> Value {
        serde_json::from_slice(&params.body).unwrap_or(Value::Null)
    }

    // -- movements: the lanes scratch and the overlays grid (P2) ---------

    /// `resolve_mode`: the operator's explicit override is the only thing
    /// that picks — `R22.7` item 5 retired the selection-follows rule.
    #[test]
    fn resolve_mode_reads_only_the_operators_override() {
        use crate::shell::lanes::Mode;
        assert_eq!(resolve_mode(None), Mode::Overlays);
        assert_eq!(resolve_mode(Some(Mode::Overlays)), Mode::Overlays);
        assert_eq!(resolve_mode(Some(Mode::Lanes)), Mode::Lanes);
    }

    // -- R22.4: `+ controller button` over the open graph's nodes -------------

    /// A door with an open graph and its nodes, in Overlays mode — the setup
    /// every `+ controller button` test shares.
    fn button_door() -> std::rc::Rc<FakeDoor> {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond(
            "GET",
            "/graph",
            json!({ "graph": { "id": "g1", "nodes": [
                { "id": "gen-1", "kind": "gen.turing", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false, "config": {} },
                { "id": "gen-2", "kind": "gen.mandel", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false, "config": {} }
            ] }, "primitives": [], "issues": [] }),
        );
        recorder.respond("GET", "/switch", json!({ "switches": [] }));
        recorder.respond("GET", "/overlay/blackout", json!({ "on": false }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        recorder.respond("GET", "/sequences", json!({ "sequences": [], "running": [] }));
        recorder
    }

    fn open_overlays_with_graph(
        recorder: &std::rc::Rc<FakeDoor>,
        graph: Option<&str>,
    ) -> egui_kittest::Harness<'static, Shell> {
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(recorder)), 2600.0);
        harness.run();
        if let Some(g) = graph {
            harness.state_mut().nodes_state_mut().set_selected_graph_for_test(g);
        }
        harness.run();
        harness
    }

/// Open `+ › controller button` — the one authoring surface every pick test
/// below drives.
fn open_controller_menu(harness: &mut egui_kittest::Harness<'static, Shell>) {
    the_overlays_plus(harness).scroll_to_me();
    harness.run();
    the_overlays_plus(harness).click();
    harness.run();
    harness.get_by_label("nodes control").click();
    harness.run();
}

/// The unified list: one mode switch (the `Preview` header) and — with a graph
/// open and a venue in hand — the open graph's nodes and the venue's
/// fixtures/groups all in the same list, with no `node`/`A fixture`/`A group`
/// scope selector.
#[test]
fn controller_menu_lists_nodes_fixtures_and_groups() {
    let recorder = fixture_button_door();
    let mut harness = open_overlays_with_graph(&recorder, Some("g1"));

    open_controller_menu(&mut harness);

    assert_eq!(harness.get_all_by_value("Switcher").count(), 1, "the mode appears once");
    assert!(harness.query_by_label("scope").is_none(), "the scope row is gone");
    assert!(harness.query_by_label("A fixture").is_none(), "no scope option any more");
    assert!(harness.query_by_label("A group").is_none(), "no scope option any more");
    // The open graph's node and the venue's group + fixtures, in one list.
    harness.get_by_label("Fixtures");
    harness.get_by_label("▣ front-wash");
    harness.get_by_label("◉ wash 1");
    harness.get_by_label("◉ spot 1");
}

/// Picking two node NAME rows accumulates two on/off targets on the one
/// controller — there is no "one target only" rule at the node level.
#[test]
fn picking_node_rows_accumulates_on_off_targets() {
    let recorder = button_door();
    let mut harness = open_overlays_with_graph(&recorder, Some("g1"));
    open_controller_menu(&mut harness);
    harness.get_by_label("Turing").click();
    harness.run();
    harness.get_by_label("Mandel").click();
    harness.run();
    assert_eq!(
        harness.state_mut().lanes_state_mut().controllers["g1"]["targets"],
        json!([{ "node": "gen-1", "on": true }, { "node": "gen-2", "on": true }]),
        "both node picks accumulate, in pick order"
    );
    assert_eq!(harness.state_mut().lanes_state_mut().controllers["g1"]["mode"], json!("switch"));
}

/// A controller holding a node, a fixture, a group and a parameter persists
/// all four target shapes through the one `/settings` write.
#[test]
fn a_controller_persists_node_fixture_group_and_param_shapes() {
    let recorder = std::rc::Rc::new(FakeDoor::new());
    recorder.respond("GET", "/settings", json!({ "grid_spans": {}, "grid_controllers": {} }));
    let mut state = PanelState::default();
    add_controller_target(recorder.as_ref(), &mut state, "g1", json!({ "node": "gen-1", "on": true }));
    add_controller_target(recorder.as_ref(), &mut state, "g1", json!({ "node": "gen-2", "param": "scale", "value": 0.5 }));
    add_controller_target(recorder.as_ref(), &mut state, "g1", json!({ "node": "venue-1", "fixture": "wash-1", "on": true }));
    add_controller_target(recorder.as_ref(), &mut state, "g1", json!({ "node": "venue-1", "group": "front-wash", "on": true }));

    let four = json!([
        { "node": "gen-1", "on": true },
        { "node": "gen-2", "param": "scale", "value": 0.5 },
        { "node": "venue-1", "fixture": "wash-1", "on": true },
        { "node": "venue-1", "group": "front-wash", "on": true },
    ]);
    assert_eq!(state.controllers["g1"]["targets"], four);
    let posted = recorder
        .calls
        .borrow()
        .iter()
        .filter(|(m, p, _)| m == "POST" && p == "/settings")
        .map(|(_, _, params)| body_of(params))
        .last()
        .expect("each pick posts /settings");
    assert_eq!(posted["grid_controllers"]["g1"]["targets"], four);
}

/// Adding the same fixture target twice updates it in place rather than
/// duplicating it — the de-dupe runs on `(node, param, fixture, group)`.
#[test]
fn picking_a_target_twice_does_not_duplicate_it() {
    let recorder = std::rc::Rc::new(FakeDoor::new());
    recorder.respond("GET", "/settings", json!({ "grid_spans": {}, "grid_controllers": {} }));
    let mut state = PanelState::default();
    let t = json!({ "node": "venue-1", "fixture": "wash-1", "on": true });
    add_controller_target(recorder.as_ref(), &mut state, "g1", t.clone());
    add_controller_target(recorder.as_ref(), &mut state, "g1", t);
    assert_eq!(state.controllers["g1"]["targets"].as_array().unwrap().len(), 1);
}

// -- Scenes: a controller flagged `scene`, one target per float of its top parents

/// A graph `g1` with `gen-1` (a top parent) feeding `fx-1`, and `gen-1`'s one
/// float `scale` read as `held` (`null` = on auto) at a live value of `0.25`.
fn scene_feature_door(held: Value) -> std::rc::Rc<FakeDoor> {
    let recorder = std::rc::Rc::new(FakeDoor::new());
    recorder.respond("GET", "/settings", json!({ "grid_spans": {}, "grid_controllers": {} }));
    recorder.respond(
        "GET",
        "/graph",
        json!({ "graph": {
            "id": "g1",
            "nodes": [
                { "id": "gen-1", "kind": "gen.turing", "label": "", "config": {} },
                { "id": "fx-1", "kind": "fx.blur", "label": "", "config": {} }
            ],
            "edges": [{ "id": "e1", "from": { "node": "gen-1", "port": "image" }, "to": { "node": "fx-1", "port": "image" } }]
        } }),
    );
    recorder.respond(
        "GET",
        "/graph/io",
        json!({ "params": [{ "name": "scale", "min": 0.0, "max": 1.0, "default": 0.5, "kind": "float", "live": 0.25, "value": held }] }),
    );
    recorder
}

/// The bodies of every `POST /graph/node` the door saw, in order.
fn posted_node_ops(recorder: &FakeDoor) -> Vec<Value> {
    recorder
        .calls
        .borrow()
        .iter()
        .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
        .map(|(_, _, params)| body_of(params))
        .collect()
}

/// A scene entry under `key`, on `g1`, with the given targets.
fn scene_entry(state: &mut PanelState, key: &str, targets: Value) {
    state.controllers.insert(
        key.into(),
        json!({ "graph": "g1", "name": "", "mode": "switch", "targets": targets, "scene": true }),
    );
}

/// Picking a top parent captures each of its floats at the live value, one
/// target per float.
#[test]
fn a_scene_captures_each_float_of_a_picked_node_at_its_live_value() {
    let recorder = scene_feature_door(Value::Null);
    let mut state = PanelState::default();
    scene_entry(&mut state, "scene:g1:1", json!([]));
    toggle_scene_node(recorder.as_ref(), &mut state, "scene:g1:1", "g1", "gen-1");
    assert_eq!(
        state.controllers["scene:g1:1"]["targets"],
        json!([{ "node": "gen-1", "param": "scale", "value": 0.25 }])
    );
    toggle_scene_node(recorder.as_ref(), &mut state, "scene:g1:1", "g1", "gen-1");
    assert_eq!(state.controllers["scene:g1:1"]["targets"], json!([]), "a second pick drops the node's floats");
}

/// Only the top parents are listed: a node a cable feeds (`fx-1`) is a child
/// whose floats the scene does not offer.
#[test]
fn a_scene_lists_only_the_top_parent_nodes() {
    let recorder = scene_feature_door(Value::Null);
    let graph = get(recorder.as_ref(), "GET", "/graph", &[("id", "g1".to_string())]).expect("the graph");
    let fed = fed_nodes(&graph);
    assert!(fed.contains("fx-1"), "the cable feeds fx-1: it is a child");
    assert!(!fed.contains("gen-1"), "gen-1 is a top parent and stays listed");
}

/// A press sets every float to its captured value, and remembers what each
/// held before (`null` for auto); the next press puts the auto float back to
/// auto, not to a frozen number.
#[test]
fn a_scene_press_sets_the_floats_and_restores_an_auto_float_to_auto() {
    let recorder = scene_feature_door(Value::Null);
    let mut state = PanelState::default();
    scene_entry(&mut state, "scene:g1:1", json!([{ "node": "gen-1", "param": "scale", "value": 0.9 }]));
    fire_scene(recorder.as_ref(), &mut state, "scene:g1:1", crate::surface::Gesture::Click);
    assert!(scene_on(&state, "scene:g1:1"));
    assert_eq!(
        state.controllers["scene:g1:1"]["before"],
        json!([{ "node": "gen-1", "param": "scale", "value": null }])
    );
    fire_scene(recorder.as_ref(), &mut state, "scene:g1:1", crate::surface::Gesture::Click);
    let ops = posted_node_ops(&recorder);
    assert_eq!(ops.len(), 2, "one batch on, one batch off");
    assert_eq!(ops[0]["op"], json!("params"));
    assert_eq!((ops[0]["updates"][0]["name"].clone(), ops[0]["updates"][0]["value"].clone()), (json!("scale"), json!(0.9)));
    assert_eq!(ops[1]["updates"][0]["value"], Value::Null, "the auto float is released back to auto");
    assert!(!scene_on(&state, "scene:g1:1"));
}

/// A float that held a number goes back to that number when the scene turns off.
#[test]
fn a_scene_restores_a_held_float_to_its_previous_number() {
    let recorder = scene_feature_door(json!(0.4));
    let mut state = PanelState::default();
    scene_entry(&mut state, "scene:g1:1", json!([{ "node": "gen-1", "param": "scale", "value": 0.9 }]));
    apply_scene_floats(recorder.as_ref(), &mut state, "scene:g1:1");
    restore_scene_floats(recorder.as_ref(), &mut state, "scene:g1:1");
    let ops = posted_node_ops(&recorder);
    let restored = ops.last().expect("the restore write")["updates"][0]["value"].as_f64().map(|v| v as f32);
    assert_eq!(restored, Some(0.4), "the float returns to the number it held");
}

/// The scene's entry is the controller map's own entry, so a `POST /settings`
/// round trip keeps its flag, its targets and its `before` snapshot.
#[test]
fn a_scene_persists_through_the_one_settings_write() {
    let recorder = scene_feature_door(Value::Null);
    let mut state = PanelState::default();
    scene_entry(&mut state, "scene:g1:1", json!([{ "node": "gen-1", "param": "scale", "value": 0.25 }]));
    state.controllers.get_mut("scene:g1:1").expect("the scene").as_object_mut().expect("an object")
        .insert("before".into(), json!([{ "node": "gen-1", "param": "scale", "value": 0.4 }]));
    save_controllers(recorder.as_ref(), &mut state);
    let posted = recorder
        .calls
        .borrow()
        .iter()
        .filter(|(m, p, _)| m == "POST" && p == "/settings")
        .map(|(_, _, params)| body_of(params))
        .last()
        .expect("the save posts /settings");
    let stored = parse_controllers(&posted["grid_controllers"]);
    assert_eq!(stored.get("scene:g1:1"), state.controllers.get("scene:g1:1"));
    assert_eq!(stored["scene:g1:1"]["scene"], json!(true));
}

/// The `+` menu offers `scene` beside `controller button`; picking it adds a
/// scene entry on the open graph and opens its editor.
#[test]
fn the_plus_menu_adds_a_scene_on_the_open_graph() {
    let recorder = fixture_button_door();
    let mut harness = open_overlays_with_graph(&recorder, Some("g1"));
    the_overlays_plus(&harness).scroll_to_me();
    harness.run();
    the_overlays_plus(&harness).click();
    harness.run();
    harness.get_by_label("scene").click();
    harness.run();
    let scenes: Vec<_> = harness.state_mut().lanes_state_mut().controllers.iter().filter(|(_, c)| c["scene"] == json!(true)).map(|(k, _)| k.clone()).collect();
    assert_eq!(scenes, vec!["scene:g1:1".to_string()]);
}

/// A controller is **one scene cell**: any mix of targets still builds exactly
/// one cell, keyed `ctrl:<graph>` for its size and labelled with the controller.
#[test]
fn a_controller_is_one_scene_cell() {
    let mut state = PanelState::default();
    state.controllers.insert(
        "g1".into(),
        json!({ "graph": "g1", "name": "", "targets": [
            { "node": "gen-1", "on": true },
            { "node": "gen-1", "param": "scale", "value": 0.5 },
            { "node": "venue-1", "fixture": "wash-1", "on": true }
        ] }),
    );
    let cells = controller_cells(&state);
    assert_eq!(cells.len(), 1, "one controller is one cell");
    assert_eq!(cells[0].key, "ctrl:g1");
    assert_eq!(cells[0].label, "g1");
    assert!(matches!(&cells[0].kind, CellKind::Controller { key } if key == "g1"));
}

/// Pressing the scene applies the whole stored state at once: every non-numeric
/// target posts its node's bypass to its stored `on`, every `param` target its
/// stored value — one node write each.
#[test]
fn pressing_a_scene_applies_every_stored_target() {
    let recorder = FakeDoor::new();
    let mut state = PanelState::default();
    state.controllers.insert(
        "g1".into(),
        json!({ "graph": "g1", "name": "", "mode": "switch", "targets": [
            { "node": "gen-1", "on": true },
            { "node": "gen-2", "param": "scale", "value": 2.5 },
            { "node": "gen-3", "on": false }
        ] }),
    );
    apply_scene(&recorder, &state, "g1", false);
    let calls = recorder.calls.borrow();
    let posts: Vec<Value> = calls
        .iter()
        .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
        .map(|(_, _, params)| body_of(params))
        .collect();
    assert_eq!(posts.len(), 3, "one write per target");
    assert!(posts.contains(&json!({ "op": "flag", "graph": "g1", "node": "gen-1", "field": "bypass", "on": true })));
    assert!(posts.contains(&json!({ "op": "param", "graph": "g1", "node": "gen-2", "name": "scale", "value": 2.5 })));
    assert!(posts.contains(&json!({ "op": "flag", "graph": "g1", "node": "gen-3", "field": "bypass", "on": false })));
}

/// A palette/colour node pick captures the node's own colours, not a bypass
/// toggle — the button recalls them; a generator is not a colour target.
#[test]
fn a_colour_node_pick_captures_its_colours() {
    let colors = json!([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 0.0]]);
    let palette = colour_target("palette", "pal-1", &json!({ "colors": colors })).expect("a palette target");
    assert_eq!(palette["node"], json!("pal-1"));
    assert_eq!(palette["colors"], colors);
    assert!(palette.get("on").is_none(), "a colour target is not a boolean");

    let colour = colour_target("color", "col-1", &json!({ "rgb": [0.2, 0.4, 0.6] })).expect("a colour target");
    assert_eq!(colour, json!({ "node": "col-1", "rgb": [0.2, 0.4, 0.6] }));

    assert!(colour_target("gen.turing", "g-1", &json!({})).is_none(), "a generator is not a colour target");
    assert!(colour_target("palette", "pal-1", &json!({})).is_none(), "a palette with no colours yet falls back");
}

/// A scene over palette/colour targets writes each node's captured config —
/// never `POST /colour/apply`, never the global colour rule.
#[test]
fn a_scene_over_colour_nodes_writes_their_config() {
    let recorder = FakeDoor::new();
    let mut state = PanelState::default();
    let colors = json!([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 0.0]]);
    state.controllers.insert(
        "g1".into(),
        json!({ "graph": "g1", "targets": [
            { "node": "pal-1", "colors": colors },
            { "node": "col-1", "rgb": [0.1, 0.2, 0.3] }
        ] }),
    );
    apply_scene(&recorder, &state, "g1", false);
    let calls = recorder.calls.borrow();
    assert!(!calls.iter().any(|(_, p, _)| p == "/colour/apply"), "the colour rule is not used");
    let posts: Vec<Value> = calls
        .iter()
        .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
        .map(|(_, _, params)| body_of(params))
        .collect();
    assert!(posts.contains(&json!({ "op": "config", "graph": "g1", "node": "pal-1", "config": { "colors": colors } })));
    assert!(posts.contains(&json!({ "op": "config", "graph": "g1", "node": "col-1", "config": { "rgb": [0.1, 0.2, 0.3] } })));
}

/// The face follows the targets: a parameter makes it the fader, an
/// all-palette/colour set the palette strip, anything else a plain button.
#[test]
fn the_controller_face_follows_its_targets() {
    let recorder = FakeDoor::new();
    recorder.respond(
        "GET",
        "/graph/io",
        json!({ "node": "gen-1", "params": [{ "name": "scale", "min": 0.0, "max": 10.0, "kind": "float", "live": 5.0 }] }),
    );
    let mut state = PanelState::default();

    state.controllers.insert(
        "g1".into(),
        json!({ "graph": "g1", "targets": [
            { "node": "pal-1", "colors": [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 0.0]] }
        ] }),
    );
    assert!(matches!(controller_face(&recorder, &state, "g1"), ControllerFace::Palette { ref stops } if stops.len() == 4));

    state.controllers.insert(
        "g1".into(),
        json!({ "graph": "g1", "targets": [{ "node": "gen-1", "param": "scale", "value": 2.5 }] }),
    );
    match controller_face(&recorder, &state, "g1") {
        ControllerFace::Fader { param, min, max, value, .. } => {
            assert_eq!((param.as_str(), min, max, value), ("scale", 0.0, 10.0, 2.5));
        }
        _ => panic!("a param target makes a fader"),
    }

    state.controllers.insert("g1".into(), json!({ "graph": "g1", "targets": [{ "node": "gen-1", "on": true }] }));
    assert!(matches!(controller_face(&recorder, &state, "g1"), ControllerFace::Button));
}

/// Point 4/7: a target's `on` boolean survives a `/settings` round-trip — it
/// rides the posted body and reads back off it.
#[test]
fn a_targets_on_survives_a_settings_round_trip() {
    let recorder = FakeDoor::new();
    recorder.respond("GET", "/settings", json!({ "grid_spans": {}, "grid_controllers": {} }));
    let mut state = PanelState::default();
    add_controller_target(&recorder, &mut state, "g1", json!({ "node": "gen-1", "on": true }));
    set_controller_target_on(&recorder, &mut state, "g1", &json!({ "node": "gen-1" }), false);
    let posted = recorder
        .calls
        .borrow()
        .iter()
        .filter(|(m, p, _)| m == "POST" && p == "/settings")
        .map(|(_, _, params)| body_of(params))
        .last()
        .expect("the flip posts /settings");
    assert_eq!(posted["grid_controllers"]["g1"]["targets"][0]["on"], json!(false));
    assert_eq!(
        parse_controllers(&posted["grid_controllers"])["g1"]["targets"][0]["on"],
        json!(false),
        "the round-tripped map keeps the target's on"
    );
    assert_eq!(state.controllers["g1"]["targets"][0]["on"], json!(false));
}

/// Point 6: a generator node's float parameters are exposed — the real
/// `gen.*` catalog entry's params carry their ranges, so `dials::floats` reads
/// them (and a node with none gets no chevron).
#[test]
fn a_generator_node_exposes_its_float_params() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json");
    let manifest = metal_schema::Manifest::parse(&metal_vfs::read_to_string(path).unwrap()).unwrap();
    let catalog = metal_graph::NodeKind::catalog(&manifest);
    let mut g = metal_graph::Graph::new("g1", "g1", metal_graph::Domain::Live);
    let id = g.add_node("gen-1", "gen.turing", [0.0, 0.0]);
    let io = metal_graph::bind::io(&g, &id, &catalog, None).unwrap();
    let v = serde_json::to_value(&io).unwrap();
    let floats = crate::shell::panels::dials::floats(&v);
    assert!(!floats.is_empty(), "a generator exposes its float params");
    assert!(floats.iter().any(|f| f.name == "scale"), "and names its own floats");
}

/// Point 3: a node with no float params draws no chevron at all — the pick row
/// is the whole row.
#[test]
fn a_node_without_params_shows_no_chevron() {
    let recorder = button_door();
    let mut harness = open_overlays_with_graph(&recorder, Some("g1"));
    open_controller_menu(&mut harness);
    assert!(harness.query_by_label("expand Turing").is_none(), "no chevron on a floatless node");
    assert!(harness.query_by_label("collapse Turing").is_none());
}

/// The editor's parameter row reuses the grid's one fader: a drag on it posts
/// the param op with the moved value.
#[test]
fn a_parameter_row_uses_the_grids_fader() {
    let recorder = button_door();
    recorder.respond(
        "GET",
        "/graph/io",
        json!({ "node": "gen-1", "params": [{ "name": "scale", "min": 0.0, "max": 10.0, "default": 1.0, "kind": "float", "live": 5.0 }] }),
    );
    let mut harness = open_overlays_with_graph(&recorder, Some("g1"));
    open_controller_menu(&mut harness);
    harness.get_by_label("expand Turing").click();
    harness.run();
    harness.get_by_label("scale").click();
    harness.run();
    assert_eq!(
        harness.state_mut().lanes_state_mut().controllers["g1"]["targets"],
        json!([{ "node": "gen-1", "param": "scale", "value": 5.0 }]),
        "the parameter pick lands its target"
    );
    // The fader sits to the right of the name; drag it toward the far end.
    // The picked row leads with the accent dot.
    let theme = Theme::phosphor();
    let name = harness.get_by_label("● scale").rect();
    let at = egui::pos2(
        name.right() + theme.item_gap.x + theme.control_width * 0.9,
        name.center().y,
    );
    harness.hover_at(at);
    harness.drag_at(at);
    harness.run();
    harness.drop_at(at);
    harness.run();
    let calls = recorder.calls.borrow();
    let posted = calls
        .iter()
        .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
        .map(|(_, _, params)| body_of(params))
        .find(|b| b["op"] == "param")
        .expect("the fader should post the param op");
    assert_eq!(posted["node"], "gen-1");
    assert_eq!(posted["name"], "scale");
    assert!(posted["value"].as_f64().unwrap() > 5.0, "dragging right raises the value");
}

/// A single click on a NODE row (its dropdown is the parameter sublist)
/// creates the controller and lands the node's on/off target.
#[test]
fn clicking_a_node_row_adds_an_on_off_target() {
    let recorder = button_door();
    let mut harness = open_overlays_with_graph(&recorder, Some("g1"));
    the_overlays_plus(&mut harness).scroll_to_me();
    harness.run();
    the_overlays_plus(&mut harness).click();
    harness.run();
    harness.get_by_label("nodes control").click();
    harness.run();
    harness.get_by_label("Turing").click();
    harness.run();
    let controllers = &harness.state_mut().lanes_state_mut().controllers;
    assert_eq!(
        controllers["g1"]["targets"],
        json!([{ "node": "gen-1", "on": true }]),
        "the node row pick lands its on/off target"
    );
}

/// A node a saved switch already drives leads the `controller button` list
/// with the red marker (`assigned_nodes` off `GET /switch`).
#[test]
fn a_node_in_a_switch_leads_with_the_red_marker() {
    let recorder = button_door();
    recorder.respond(
        "GET",
        "/switch",
        json!({ "switches": [
            { "id": "sw-1", "name": "b", "kind": "toggle", "on": false,
              "set": [{ "node": "gen-1", "on": true }] }
        ] }),
    );
    let mut harness = open_overlays_with_graph(&recorder, Some("g1"));
    the_overlays_plus(&mut harness).scroll_to_me();
    harness.run();
    the_overlays_plus(&mut harness).click();
    harness.run();
    harness.get_by_label("nodes control").click();
    harness.run();
    // The marker rides the node row's label.
    harness.get_by_label("● Turing");
}

/// Point 7: a node's folder row starts collapsed — none of its float
/// parameters are painted — and the LEFT chevron expands it to reveal them.
#[test]
fn a_node_starts_collapsed_and_expands_to_its_float_rows() {
    let recorder = button_door();
    recorder.respond(
        "GET",
        "/graph/io",
        json!({ "node": "gen-1", "params": [
            { "name": "scale", "min": 0.0, "max": 10.0, "default": 1.0, "kind": "float", "live": 5.0 }
        ] }),
    );
    let mut harness = open_overlays_with_graph(&recorder, Some("g1"));
    the_overlays_plus(&mut harness).scroll_to_me();
    harness.run();
    the_overlays_plus(&mut harness).click();
    harness.run();
    harness.get_by_label("nodes control").click();
    harness.run();
    // Collapsed: the chevron is `expand` and the float row is not painted.
    harness.get_by_label("expand Turing");
    assert!(harness.query_by_label("scale").is_none(), "a collapsed node hides its floats");
    harness.get_by_label("expand Turing").click();
    harness.run();
    // Expanded: the chevron flipped up and the float row is in.
    harness.get_by_label("collapse Turing");
    harness.get_by_label("scale");
}

/// A door with one saved controller named `escena` — the scene cell the
/// right-click editor tests share.
fn scene_door() -> std::rc::Rc<FakeDoor> {
    let recorder = std::rc::Rc::new(FakeDoor::new());
    recorder.respond("GET", "/state", json!({ "outputs": [] }));
    recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
    recorder.respond("GET", "/switch", json!({ "switches": [] }));
    recorder.respond("GET", "/sequences", json!({ "sequences": [], "running": [] }));
    recorder.respond("GET", "/graphs", json!({ "graphs": [] }));
    recorder.respond(
        "GET",
        "/graph",
        json!({ "graph": { "id": "g1", "nodes": [] }, "primitives": [], "issues": [] }),
    );
    recorder.respond(
        "GET",
        "/settings",
        json!({ "grid_spans": {}, "grid_controllers": {
            "g1": { "graph": "g1", "name": "escena", "mode": "switch", "targets": [
                { "node": "gen-1", "param": "scale", "value": 0.5 }
            ] }
        } }),
    );
    recorder
}

/// A right-click on a scene cell opens the one editor; its size row writes
/// `grid_spans[ctrl:<graph>]`.
#[test]
fn a_scene_cell_right_click_opens_the_editor_and_sizes_it() {
    let recorder = scene_door();
    let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
    harness.run();
    harness.get_by_label("lanes / overlays").click();
    harness.run();

    harness.get_by_label("escena").click_secondary();
    harness.run();
    harness.get_by_label("2×1").click();
    harness.run();

    let calls = recorder.calls.borrow();
    let posted = calls
        .iter()
        .find(|(m, p, _)| m == "POST" && p == "/settings")
        .map(|(_, _, params)| body_of(params))
        .expect("choosing a size posts /settings");
    assert_eq!(posted["grid_spans"], json!({ "ctrl:g1": [2, 1] }));
}

/// The editor's delete drops the controller from the grid — the scene cell
/// goes with it.
#[test]
fn the_editors_delete_removes_the_controller() {
    let recorder = scene_door();
    let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
    harness.run();
    harness.get_by_label("lanes / overlays").click();
    harness.run();

    harness.get_by_label("escena").click_secondary();
    harness.run();
    harness.get_by_label("delete controller").click();
    harness.run();

    assert!(
        harness.state_mut().lanes_state_mut().controllers.get("g1").is_none(),
        "delete removes the controller"
    );
    let calls = recorder.calls.borrow();
    let posted = calls
        .iter()
        .filter(|(m, p, _)| m == "POST" && p == "/settings")
        .map(|(_, _, params)| body_of(params))
        .last()
        .expect("delete posts /settings");
    assert_eq!(posted["grid_controllers"], json!({}));
}

/// The editor's `save` commits the typed name onto the controller, and the
/// scene cell prints it (the assigned name reaches the button).
#[test]
fn the_editors_save_commits_the_name_to_the_button() {
    let recorder = scene_door();
    let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
    harness.run();
    harness.get_by_label("lanes / overlays").click();
    harness.run();

    harness.get_by_label("escena").click_secondary();
    harness.run();
    // The typed name: the buffer is the editor's own field.
    {
        let s = harness.state_mut().lanes_state_mut();
        s.controller_name_buffer = "otra".into();
        s.controller_name_for = Some("g1".into());
    }
    harness.run();
    harness.get_by_label("save").click();
    harness.run();

    assert_eq!(
        harness.state_mut().lanes_state_mut().controllers["g1"]["name"],
        json!("otra"),
        "save commits the name"
    );
    // The scene cell now prints the assigned name.
    harness.get_by_label("otra");
}

/// Without an open graph the `controller button` dropdown says so instead of
/// offering nodes.
#[test]
fn without_an_open_graph_create_button_says_so() {
    let recorder = button_door();
    let mut harness = open_overlays_with_graph(&recorder, None);

    the_overlays_plus(&mut harness).scroll_to_me();
    harness.run();
    the_overlays_plus(&mut harness).click();
    harness.run();
    harness.get_by_label("nodes control").click();
    harness.run();

    harness.get_by_label("— open a graph to control its nodes —");
    assert!(
        harness.query_by_label("Turing").is_none(),
        "no node rows without a graph"
    );
}

    /// The G1 fixture-family setup: an open graph with one `venue.fixtures`
    /// node and a venue of two fixtures sharing one group — what the
    /// `fixture`/`rig` options read.
    fn fixture_button_door() -> std::rc::Rc<FakeDoor> {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond(
            "GET",
            "/graph",
            json!({ "graph": { "id": "g1", "nodes": [
                { "id": "venue-1", "kind": "venue.fixtures", "pos": [0.0, 0.0], "label": "",
                  "bypass": false, "solo": false,
                  "config": { "venue": "v1", "group": "front-wash" } }
            ] }, "primitives": [], "issues": [] }),
        );
        recorder.respond(
            "GET",
            "/venue",
            json!({ "venue": {
                "venue": { "id": "v1", "name": "v1", "setting": "indoor" },
                "fixture": [
                    { "id": "wash-1", "name": "wash 1", "kind": "wash", "group": "front-wash" },
                    { "id": "spot-1", "name": "spot 1", "kind": "spot", "group": "front-wash" }
                ]
            } }),
        );
        recorder.respond("GET", "/switch", json!({ "switches": [] }));
        recorder.respond("GET", "/overlay/blackout", json!({ "on": false }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        recorder.respond("GET", "/sequences", json!({ "sequences": [], "running": [] }));
        recorder
    }

    /// The unified list carries the open graph's nodes AND the venue's
    /// fixtures/groups — a group folder row over its fixtures, fixture rows
    /// peers of the node rows — with no scope selector. A fixture row pick
    /// adds a `fixture` target on the resolved `venue.fixtures` node.
    #[test]
    fn the_controller_menu_offers_fixtures_and_groups_in_one_list() {
        let recorder = fixture_button_door();
        let mut harness = open_overlays_with_graph(&recorder, Some("g1"));
        open_controller_menu(&mut harness);

        assert!(harness.query_by_label("A fixture").is_none(), "no scope option any more");
        assert!(harness.query_by_label("A group").is_none(), "no scope option any more");
        // The venue group and its fixtures, in the same list as the nodes.
        harness.get_by_label("▣ front-wash");
        harness.get_by_label("◉ wash 1");
        harness.get_by_label("◉ spot 1");
    }

    /// G1: a fixture row pick adds `{node, fixture, on}` to the graph's
    /// controller — the node resolved off the graph's `venue.fixtures` node.
    #[test]
    fn a_fixture_row_pick_adds_a_fixture_target() {
        let recorder = fixture_button_door();
        let mut harness = open_overlays_with_graph(&recorder, Some("g1"));
        open_controller_menu(&mut harness);
        harness.get_by_label("◉ wash 1").click();
        harness.run();
        assert_eq!(
            harness.state_mut().lanes_state_mut().controllers["g1"]["targets"],
            json!([{ "node": "venue-1", "fixture": "wash-1", "on": true }])
        );
        // The saved controller is one scene cell, still under `ctrl:g1`.
        let mut state = PanelState::default();
        state.controllers.insert(
            "g1".into(),
            json!({ "graph": "g1", "name": "wash", "targets": [{ "node": "venue-1", "fixture": "wash-1", "on": true }] }),
        );
        let cells = controller_cells(&state);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].key, "ctrl:g1");
        assert_eq!(cells[0].label, "wash");
    }

    /// G1: a group row pick adds `{node, group, on}` on the group's own
    /// `venue.fixtures` node.
    #[test]
    fn a_group_row_pick_adds_a_group_target() {
        let recorder = fixture_button_door();
        let mut harness = open_overlays_with_graph(&recorder, Some("g1"));
        open_controller_menu(&mut harness);
        harness.get_by_label("▣ front-wash").click();
        harness.run();
        assert_eq!(
            harness.state_mut().lanes_state_mut().controllers["g1"]["targets"],
            json!([{ "node": "venue-1", "group": "front-wash", "on": true }])
        );
    }

    /// The `Preview` mode switch is the popover's own, moved into the submenu
    /// header; flipping it stores `hold` on the graph's controller.
    #[test]
    fn the_preview_mode_switch_stores_hold_on_the_controller() {
        let recorder = fixture_button_door();
        let mut harness = open_overlays_with_graph(&recorder, Some("g1"));
        open_controller_menu(&mut harness);
        harness.get_by_value("Switcher").click();
        harness.run();
        harness.get_by_label("Hold").click();
        harness.run();
        assert_eq!(harness.state_mut().lanes_state_mut().controllers["g1"]["mode"], json!("hold"));
    }

    /// G3: `switches_cells` reads the switch's `mode` into the fixture/rig
    /// cell, so a hold button fires on the press/release edge and a switch
    /// button on a click.
    #[test]
    fn fixture_cells_carry_the_button_mode() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/switch",
            json!({ "switches": [
                { "id": "fx-1", "name": "wash 1", "kind": "fixture", "on": false, "mode": "hold" },
                { "id": "rig-1", "name": "front", "kind": "rig", "on": false, "mode": "switch" }
            ] }),
        );
        let cells = switches_cells(recorder.as_ref());
        let fx = cells.iter().find(|c| c.key == "sw:fx-1").unwrap();
        assert!(matches!(&fx.kind, CellKind::Fixture { hold: true, .. }), "a fixture `mode:hold` is a holding fixture cell");
        let rig = cells.iter().find(|c| c.key == "sw:rig-1").unwrap();
        assert!(matches!(&rig.kind, CellKind::Rig { hold: false, .. }), "a fixture `mode:switch` is a latching rig cell");
    }

    /// G1/F12: the node a fixture/rig button drives — a `rig` names its
    /// group exactly; a `fixture` prefers its **own** `venue.fixture` node,
    /// then its group's `venue.fixtures` node, then the venue-wide one;
    /// nothing in this venue is `None`.
    #[test]
    fn fixture_button_target_picks_the_fixture_then_group_then_venue_node() {
        let nodes = vec![
            ("whole".to_string(), "v1".to_string(), None, None),
            ("front".to_string(), "v1".to_string(), Some("front-wash".to_string()), None),
            ("other".to_string(), "v2".to_string(), Some("front-wash".to_string()), None),
            ("wash-1".to_string(), "v1".to_string(), None, Some("wash-1".to_string())),
        ];
        // A rig names its group exactly.
        assert_eq!(
            fixture_button_target(&nodes, "v1", None, None, Some("front-wash")),
            Some("front".to_string())
        );
        // A fixture with its own node takes it, over the group and venue nodes.
        assert_eq!(
            fixture_button_target(&nodes, "v1", Some("wash-1"), Some("front-wash"), None),
            Some("wash-1".to_string())
        );
        // Without its own node, the group's, then the venue-wide one.
        assert_eq!(
            fixture_button_target(&nodes, "v1", Some("wash-9"), Some("front-wash"), None),
            Some("front".to_string())
        );
        assert_eq!(
            fixture_button_target(&nodes, "v1", Some("wash-9"), Some("back-wash"), None),
            Some("whole".to_string())
        );
        assert_eq!(fixture_button_target(&nodes, "v3", None, None, Some("front-wash")), None);
    }

    /// `save ▾ → save as sequence` posts the active track as the
    /// one-movement body `POST /author/models/library` takes — name, model,
    /// one row per keypoint in tick order.
    #[test]
    fn saving_the_active_track_as_a_sequence_posts_the_library_body() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond(
            "GET",
            "/author/models/state",
            json!({ "models": [{ "id": "geoterrain", "label": "GeoTerrain", "width": 2 }] }),
        );
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();
        {
            let ingest = harness.state_mut().ingest_state_mut();
            ingest.model = "geoterrain".into();
            ingest.keyframes.insert(0, vec![0.5, 0.0]);
        }
        harness.run();
        open_save_menu(&mut harness);
        harness.get_by_label("save as sequence").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/author/models/library")
            .map(|(_, _, params)| body_of(params))
            .next()
            .expect("save as sequence should POST /author/models/library");
        assert_eq!(posted, json!({ "name": "geoterrain-", "model": "geoterrain", "ticks": [[0.5, 0.0]] }));
    }

    /// The sequence editor: a click on the ruler moves the playhead to that
    /// tick, a drag on it marks a range, and `+ track` stacks a second model
    /// track over the same song.
    #[test]
    fn the_timeline_scrubs_marks_and_stacks_tracks() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond(
            "GET",
            "/author/models/state",
            json!({ "models": [{ "id": "geoterrain", "label": "GeoTerrain", "width": 2 }] }),
        );
        let n = 400;
        recorder.respond(
            "GET",
            "/author/grade/matrix",
            json!({
                "song": "s", "rate": 8.0, "ticks": n,
                "rms": vec![0.2; n], "bandwidth": vec![0.1; n], "onset": vec![0.0; n], "centroid": vec![0.3; n],
                "chroma": vec![[0.0f32; 12]; n],
            }),
        );
        recorder.respond("GET", "/author/models/example", json!({ "ticks": [{ "tick": 0, "floats": [0.5, 0.25] }] }));
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();
        {
            let ingest = harness.state_mut().ingest_state_mut();
            ingest.model = "geoterrain".into();
            ingest.song = "s".into();
        }
        harness.run();
        harness.run();

        let (rect, span) = {
            let ingest = harness.state_mut().ingest_state_mut();
            (ingest.canvas_rect.expect("the timeline painted"), ingest.span)
        };
        // The ruler's lane area starts after the label column and stops at
        // the vertical scrollbar.
        let left = rect.left() + 150.0;
        let px = (rect.width() - 150.0 - 10.0) / span as f32;
        let at = |tick: f32| egui::pos2(left + (tick + 0.5) * px, rect.top() + 10.0);
        harness.drag_at(at(40.0));
        harness.run();
        harness.drop_at(at(40.0));
        harness.run();
        assert_eq!(harness.state_mut().ingest_state_mut().playhead, 40, "a ruler click scrubs");

        harness.drag_at(at(10.0));
        harness.run();
        harness.hover_at(at(14.0));
        harness.run();
        harness.hover_at(at(20.0));
        harness.run();
        harness.drop_at(at(20.0));
        harness.run();
        let ingest = harness.state_mut().ingest_state_mut();
        assert_eq!(ingest.mark.map(|(a, _)| a), Some(10), "a ruler drag marks from where it started");
        assert!(ingest.mark.is_some_and(|(_, b)| b >= 18), "…to where it ended: {:?}", ingest.mark);

        harness.get_by_label("import").scroll_to_me();
        harness.run();
        harness.get_by_label("import").click();
        harness.run();
        harness.get_by_label("fx layers").click();
        harness.run();
        harness.get_by_label("fx").click();
        harness.run();
        let ingest = harness.state_mut().ingest_state_mut();
        assert_eq!(ingest.track_count(), 2);
        assert_eq!(ingest.active, 1, "the new track is the one being edited");
        assert_eq!(ingest.track_views()[0].keyframes.get(&0), Some(&vec![0.5, 0.25]), "the first keeps its keypoints");
    }

    /// `import → fx` from the toolbar focuses the lanes region as well as
    /// stacking the layer — the click-to-focus catcher never sees a click.
    #[test]
    fn layer_button_focuses_the_lanes_region_and_its_new_layer() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/state", json!({ "models": [] }));
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 1400.0);
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        assert_eq!(harness.state().focus, None, "nothing focused yet");
        {
            let ingest = harness.state_mut().ingest_state_mut();
            ingest.model = "a".into();
            ingest.keyframes.insert(0, vec![0.5]);
        }
        harness.run();
        harness.get_by_label("import").scroll_to_me();
        harness.run();
        harness.get_by_label("import").click();
        harness.run();
        harness.get_by_label("fx layers").click();
        harness.run();
        harness.get_by_label("fx").click();
        harness.run();
        assert_eq!(
            harness.state().focus,
            Some(crate::shell::panels::Region::Lanes),
            "import → fx focuses the lanes region"
        );
        let ingest = harness.state_mut().ingest_state_mut();
        assert_eq!(ingest.track_count(), 2, "and stacks the new layer");
        assert_eq!(ingest.active, 1, "focused on the new layer");
    }

    /// The `importar` gallery with real animations listed still offers the
    /// `fx` layer card: the animation section is on screen first, and the
    /// fx section is one nav row away.
    #[test]
    fn importar_with_animations_still_offers_the_fx_layer() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond(
            "GET",
            "/author/models/state",
            json!({ "models": [{ "id": "a", "label": "A", "width": 4 }] }),
        );
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 1400.0);
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        {
            let ingest = harness.state_mut().ingest_state_mut();
            ingest.model = "a".into();
            ingest.keyframes.insert(0, vec![0.5]);
        }
        harness.run();
        harness.get_by_label("import").scroll_to_me();
        harness.run();
        harness.get_by_label("import").click();
        harness.run();
        harness.get_by_label("fx layers").click();
        harness.run();
        harness.get_by_label("fx").click();
        harness.run();
        assert_eq!(harness.state_mut().ingest_state_mut().track_count(), 2, "fx is reachable with animations listed");
    }

    /// The fx catalogue is in the `importar` gallery: one card per `fx.*`
    /// kind, and picking one adds a layer carrying that effect at the playhead.
    #[test]
    fn importar_lists_the_fx_catalogue_and_a_pick_adds_that_effect() {
        let door = FakeDoor::new();
        door.respond(
            "GET",
            "/catalog",
            json!({ "catalog": [
                { "kind": "fx.shake", "label": "Shake", "family": "effect" },
                { "kind": "gen.a", "label": "A", "family": "generator" },
            ] }),
        );
        door.respond("GET", "/author/models/state", json!({ "models": [] }));
        let sections = import_sections(&door, &super::super::nodes::PanelState::default());
        let fx = sections.iter().find(|s| s.label == tr("fx layers")).expect("the fx section");
        let kinds: Vec<&str> = fx.items.iter().map(|i| i.kind.as_str()).collect();
        assert_eq!(kinds, ["fx", "fx.shake"], "the layer card, then the catalogue");

        let mut lane = crate::shell::lanes::IngestLane::default();
        lane.span = 100;
        lane.add_effect_at_playhead("fx.shake");
        assert_eq!(lane.track_count(), 1);
        let views = lane.track_views();
        assert_eq!(views[0].effect, Some("fx.shake"));
        assert_eq!(views[0].bounds, Some((0, 24)), "a quarter of the sequence from the playhead");
    }

    /// A folder picked for the sequence adds its importable files in order
    /// from the playhead; an audio file becomes the sequence's song.
    #[test]
    fn importing_a_folder_adds_its_images_videos_and_audio() {
        let door = FakeDoor::new();
        door.respond(
            "GET",
            "/fs",
            json!({ "files": [
                { "file": "a.png", "kind": "image" },
                { "file": "notes.txt", "kind": "text" },
                { "file": "b.mp4", "kind": "video" },
                { "file": "mix.wav", "kind": "audio" },
            ] }),
        );
        let mut lane = crate::shell::lanes::IngestLane::default();
        lane.span = 100;
        import_file(&door, &mut lane, "folder", "/media/show");
        assert_eq!(lane.track_count(), 2, "the image and the video; the text file is skipped");
        assert_eq!(lane.song, "mix", "the audio file names the sequence's song");
        assert_eq!(song_name_of("/x/mix.wav"), "mix");
    }

    /// Keyboard Delete/Backspace is the 🗑 button's twin: with the lanes
    /// region focused (and no text field typing) it clears the marked range
    /// and removes the active track as one undoable op.
    #[test]
    fn delete_key_removes_the_active_track_when_lanes_is_focused() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond(
            "GET",
            "/author/models/state",
            json!({ "models": [{ "id": "a", "label": "A", "width": 1 }] }),
        );
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        {
            let ingest = harness.state_mut().ingest_state_mut();
            ingest.model = "a".into();
            ingest.keyframes.insert(0, vec![0.5]);
            ingest.keyframes.insert(4, vec![0.9]);
            ingest.add_track();
            ingest.model = "b".into();
            ingest.keyframes.insert(2, vec![0.25]);
            ingest.select_track(0);
            ingest.goto(0);
            ingest.mark = Some((0, 0));
        }
        harness.run();
        assert_eq!(harness.state().focus, None, "nothing focused yet");

        harness.get_by_label("lanes / overlays").click();
        harness.run();
        assert_eq!(
            harness.state().focus,
            Some(crate::shell::panels::Region::Lanes),
            "clicking the panel head focuses the lanes region"
        );

        harness.key_press(egui::Key::Delete);
        harness.run();
        let ingest = harness.state_mut().ingest_state_mut();
        assert_eq!(ingest.track_count(), 1, "Delete removed the active track");
        assert_eq!(ingest.model, "b", "the remaining track becomes live");

        // …and it undoes as the one gesture it was.
        harness.state_mut().ingest_state_mut().undo();
        harness.run();
        let ingest = harness.state_mut().ingest_state_mut();
        assert_eq!(ingest.track_count(), 2, "undo restored the removed track");
        assert_eq!(ingest.model, "a");
    }

    /// The layer header's eye is a real widget: clicking it writes the
    /// active layer's `muted`.
    #[test]
    fn the_layer_header_eye_toggles_mute() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond(
            "GET",
            "/author/models/state",
            json!({ "models": [{ "id": "a", "label": "A", "width": 1 }] }),
        );
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        {
            let ingest = harness.state_mut().ingest_state_mut();
            ingest.model = "a".into();
            ingest.keyframes.insert(0, vec![0.5]);
        }
        harness.run();
        harness.run();
        assert!(
            !harness.state_mut().ingest_state_mut().layer_meta(&crate::shell::lanes::Page::default(), 0).muted,
            "a fresh layer is unmuted"
        );
        harness.get_by_label(&glyph(Icon::Eye).to_string()).click();
        harness.run();
        let ingest = harness.state_mut().ingest_state_mut();
        assert!(ingest.layer_meta(&ingest.current_page(), 0).muted, "the eye toggles mute");
    }

    /// G2: the sequence's `+ lane ` menu offers every venue fixture and
    /// every `Fixture.group`, and picking one opens its page and adds a lane
    /// aimed at it — the fixture family's own dropdown, beside `+ layer`.
    #[test]
    fn the_fixture_lane_menu_offers_fixtures_and_rigs_and_adds_a_lane() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/state", json!({ "models": [] }));
        recorder.respond(
            "GET",
            "/venue",
            json!({ "venue": {
                "venue": { "id": "v1", "name": "v1", "setting": "indoor" },
                "fixture": [
                    { "id": "wash-1", "name": "wash 1", "kind": "wash", "group": "front-wash" },
                    { "id": "spot-1", "name": "spot 1", "kind": "spot", "group": "front-wash" }
                ]
            } }),
        );
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        harness.get_by_label("+ lane ").scroll_to_me();
        harness.run();
        harness.get_by_label("+ lane ").click();
        harness.run();
        harness.get_by_label("◉ wash 1").click();
        harness.run();
        {
            let ingest = harness.state_mut().ingest_state_mut();
            assert_eq!(ingest.current_page().fixture.as_deref(), Some("wash-1"));
            assert!(
                ingest.track_views().iter().any(|t| t.fixture == Some("wash-1")),
                "the new lane targets the fixture"
            );
        }
        // The rig half: the same menu's `▣ <group>` opens the group's page.
        harness.get_by_label("+ lane ").click();
        harness.run();
        harness.get_by_label("▣ front-wash").click();
        harness.run();
        let ingest = harness.state_mut().ingest_state_mut();
        assert_eq!(ingest.current_page().group.as_deref(), Some("front-wash"));
        assert!(ingest.track_views().iter().any(|t| t.group == Some("front-wash")));
    }

    /// V1: the layer editor paints the selected page's device stack — one row
    /// per layer, read off `GET /state`'s `outputs[].layers` (rule 4). The
    /// front layer (`z` 1) is listed before the back one (`z` 0).
    #[test]
    fn the_layer_editor_renders_a_devices_layers() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/state",
            json!({
                "outputs": [{
                    "id": "screen-1",
                    "masks": [],
                    "layers": [
                        { "id": "l0", "name": "fondo", "z": 0, "opacity": 1.0,
                          "blend": "normal", "active": true, "solo": false, "mask": null,
                          "sector": { "x": 0.0, "and": 0.0, "w": 1.0, "h": 1.0 } },
                        { "id": "l1", "name": "texto", "z": 1, "opacity": 0.5,
                          "blend": "add", "active": true, "solo": false, "mask": null,
                          "sector": { "x": 0.0, "and": 0.0, "w": 1.0, "h": 1.0 } }
                    ]
                }]
            }),
        );
        recorder.respond("GET", "/author/models/state", json!({ "models": [] }));
        let mut harness =
            shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 3200.0);
        harness
            .state_mut()
            .lanes_state_mut()
            .force_mode(crate::shell::lanes::Mode::Lanes);
        harness.state_mut().ingest_state_mut().device = Some("screen-1".into());
        harness.run();
        harness.run();

        harness.get_by_label("layer stack");
        let background = harness.get_by_label("fondo");
        let text = harness.get_by_label("texto");
        assert!(
            text.rect().top() < background.rect().top(),
            "the front layer (z 1) is listed above the back one (z 0)"
        );
    }

    /// Opens the bottom bar's `save ▾`.
    fn open_save_menu(harness: &mut egui_kittest::Harness<'static, Shell>) {
        harness.get_by_label(&glyph(Icon::Save).to_string()).scroll_to_me();
        harness.run();
        harness.get_by_label(&glyph(Icon::Save).to_string()).click();
        harness.run();
    }

    /// `S.AP` correction (2026-09-22): "the ingestion of the songs should
    /// happen after load the lanes" — no more "load" button; setting a
    /// model and a song is enough, and the very next frame's `needs_load`
    /// fetches the song's digest, the model's authored keyframes for that
    /// song, and the song's own beat hits — all three, through
    /// [`crate::shell::lanes::IngestLane::load`] — and lands the playhead
    /// back at 0.
    #[test]
    fn loading_the_dataset_populates_matrix_keyframes_and_song_hits_from_the_door() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond(
            "GET",
            "/author/models/state",
            json!({ "models": [{ "id": "wake", "label": "Estela", "width": 2 }] }),
        );
        let chroma = vec![[0.0f32; 12], [0.0f32; 12]];
        recorder.respond(
            "GET",
            "/author/grade/matrix",
            json!({
                "song": "click-100", "rate": 8.0, "ticks": 2,
                "rms": [0.1, 0.2], "bandwidth": [0.0, 0.0], "onset": [0.0, 0.0], "centroid": [0.0, 0.0],
                "chroma": chroma,
            }),
        );
        recorder.respond(
            "GET",
            "/author/models/example",
            json!({
                "model": "wake", "song": "click-100", "rate": 8.0,
                "ticks": [{ "tick": 0, "floats": [0.25, 0.75] }]
            }),
        );
        recorder.respond("GET", "/author/models/song-hits", json!({ "ticks": [4, 12] }));

        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 3400.0);
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();
        {
            let ingest = harness.state_mut().ingest_state_mut();
            ingest.model = "wake".into();
            ingest.song = "click-100".into();
        }
        // No button: `needs_load` fires on its own now that a model and a
        // song are both set.
        harness.run();
        harness.run();

        let ingest = harness.state_mut().ingest_state_mut();
        let matrix = ingest
            .matrix
            .clone()
            .expect("needs_load should have fetched the matrix automatically");
        assert_eq!(matrix.ticks, 2);
        assert_eq!(matrix.rms, vec![0.1, 0.2]);
        assert_eq!(
            ingest.keyframes.get(&0),
            Some(&vec![0.25, 0.75]),
            "the model's authored ticks for this song loaded too"
        );
        assert_eq!(
            ingest.song_hits,
            [4usize, 12].into_iter().collect(),
            "the song's own beat hits loaded too"
        );
        assert_eq!(ingest.playhead, 0, "loading lands the playhead back at 0");
    }

    /// `S.AP`: "save dataset" posts the ingest lane's keyframes as
    /// `{model, song, rate, ticks: [{tick, floats}]}` through `POST
    /// /author/models/example` — the dataset shape, distinct from
    /// `ticks_json`'s dense array under `{name, model}` the sequence save
    /// above posts.
    #[test]
    fn saving_the_dataset_posts_the_models_example_body() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond(
            "GET",
            "/author/models/state",
            json!({ "models": [{ "id": "wake", "label": "Estela", "width": 2 }] }),
        );
        // Scripted so the auto-load (`needs_load`, fired the instant model
        // and song are both set) lands exactly the keyframe this test
        // saves back out — the same trip a real "save dataset" makes.
        recorder.respond(
            "GET",
            "/author/models/example",
            json!({
                "model": "wake", "song": "click-100", "rate": 8.0,
                "ticks": [{ "tick": 0, "floats": [0.5, 0.0] }]
            }),
        );
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 3400.0);
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();
        {
            let ingest = harness.state_mut().ingest_state_mut();
            ingest.model = "wake".into();
            ingest.song = "click-100".into();
        }
        harness.run();
        harness.run();

        open_save_menu(&mut harness);
        harness.get_by_label("save dataset").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/author/models/example")
            .map(|(_, _, params)| body_of(params))
            .next()
            .expect("save dataset should POST /author/models/example");
        assert_eq!(
            posted,
            json!({
                "model": "wake", "song": "click-100", "rate": 8.0,
                "ticks": [{ "tick": 0, "floats": [0.5, 0.0] }]
            })
        );
        assert!(
            posted.get("device").is_none(),
            "no node selected, no device target — the shared corpus"
        );
    }

    /// 2026-10-01: "save sequence" posts every page's clips — a model
    /// and a video, on their layers with their marks — as one project file
    /// for the song through `POST /author/project`.
    #[test]
    fn saving_the_sequence_posts_the_project_with_every_clip() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/state", json!({ "models": [] }));
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 3400.0);
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();
        {
            let ingest = harness.state_mut().ingest_state_mut();
            ingest.song = "click-100".into();
            ingest.model = "turing".into();
            ingest.keyframes.insert(4, vec![0.5]);
            ingest.add_video_clip("/media/a.mp4", 10, 16, None);
        }
        harness.run();

        open_save_menu(&mut harness);
        harness.get_by_label("save sequence").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/author/project")
            .map(|(_, _, params)| body_of(params))
            .next()
            .expect("save sequence should POST /author/project");
        assert_eq!(posted["song"], json!("click-100"));
        let clips = posted["clips"].as_array().unwrap();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0]["model"], json!("turing"));
        assert_eq!(clips[0]["keyframes"], json!([{ "tick": 4, "floats": [0.5] }]));
        assert_eq!(clips[1]["video"], json!("/media/a.mp4"));
        assert_eq!(clips[1]["bounds"], json!([10, 25]));
        assert_eq!(clips[1]["layer"], json!(1), "a dropped video opens a layer over the model's");
    }

    /// A click on an idle sequence button posts the `play` body
    /// `call::overlay` has answered since §16.7 — movement, device, layer,
    /// loop — against the first output's first layer, and focuses the button
    /// it hit.
    #[test]
    fn clicking_an_idle_overlay_button_posts_play() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/state",
            json!({ "outputs": [{ "id": "screen-1", "layers": [{ "id": "l0" }] }], "overlays": [] }),
        );
        recorder.respond(
            "GET",
            "/author/models/library",
            json!({ "movements": [{ "name": "drop", "kind": "model", "ticks": 3, "favourite": false }] }),
        );
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();


        let button = harness.get_by_role_and_label(egui::accesskit::Role::Button, "drop");
        button.scroll_to_me();
        harness.run();
        harness.get_by_role_and_label(egui::accesskit::Role::Button, "drop").click();
        harness.run();

        assert_eq!(
            harness.state_mut().lanes_state_mut().focused_overlay(),
            Some("drop"),
            "clicking a sequence button focuses it"
        );
        let calls = recorder.calls.borrow();
        let posted = calls
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/overlay")
            .map(|(_, _, params)| body_of(params))
            .next()
            .expect("clicking an idle sequence button should POST /overlay");
        assert_eq!(
            posted,
            json!({
                "op": "play", "device": "screen-1", "layer": "l0",
                "movement": "drop", "loop": false
            })
        );
    }

    /// A click on a sequence button whose movement is already in the air
    /// posts the `stop` body — the same toggle, the other half.
    #[test]
    fn clicking_a_running_overlay_button_posts_stop() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/state",
            json!({
                "outputs": [{ "id": "screen-1", "layers": [{ "id": "l0" }] }],
                "overlays": [{ "device": "screen-1", "layer": "l0", "movement": "drop", "tick": 5 }]
            }),
        );
        recorder.respond(
            "GET",
            "/author/models/library",
            json!({ "movements": [{ "name": "drop", "kind": "model", "ticks": 10, "favourite": false }] }),
        );
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();


        let button = harness.get_by_role_and_label(egui::accesskit::Role::Button, "drop");
        button.scroll_to_me();
        harness.run();
        harness.get_by_role_and_label(egui::accesskit::Role::Button, "drop").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/overlay")
            .map(|(_, _, params)| body_of(params))
            .next()
            .expect("clicking a running sequence button should POST /overlay");
        assert_eq!(
            posted,
            json!({ "op": "stop", "device": "screen-1", "layer": "l0" })
        );
    }

    /// `+` → `create sequence` switches to the authoring state with a clean
    /// scratch — the old phrase and cells are gone, and the note says where
    /// the operator landed. 2026-09-22 (afternoon): the option lives behind the
    /// grid's one `+` now.
    #[test]
    fn create_sequence_switches_to_lanes_with_a_clean_scratch() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();

        {
            let mv = &mut harness.state_mut().lanes_state_mut().mv;
            mv.name = "old".into();
            mv.write(0, 0, 1.0);
        }
        harness.run();

        the_overlays_plus(&harness).scroll_to_me();
        harness.run();
        the_overlays_plus(&harness).click();
        harness.run();
        harness.get_by_label("create sequence").click();
        harness.run();
        let state = harness.state_mut().lanes_state_mut();
        assert_eq!(state.mode_override, Some(crate::shell::lanes::Mode::Lanes));
        assert!(state.mv.name.is_empty(), "the scratch name is clean");
        assert!(state.mv.cells.is_empty(), "the scratch cells are clean");
        assert_eq!(state.mv.note.as_deref(), Some("new sequence"));
    }

    /// `+` → `add loop` opens a sub-dropdown of 8, 16 and 64 ticks; picking
    /// 16 replaces the view with an empty loop of 16 ticks and no song, so
    /// no digest is requested for it.
    #[test]
    fn add_loop_offers_8_16_64_ticks_and_opens_a_tick_limited_view() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();

        the_overlays_plus(&harness).scroll_to_me();
        harness.run();
        the_overlays_plus(&harness).click();
        harness.run();
        // `menu_button` draws its `⏵` affordance into the label.
        harness.get_by_label_contains("add loop").click();
        harness.run();
        for label in ["8 ticks", "16 ticks", "64 ticks"] {
            harness.get_by_label(label);
        }
        harness.get_by_label("16 ticks").click();
        harness.run();

        let ingest = harness.state_mut().ingest_state_mut();
        assert_eq!(ingest.loop_ticks, Some(16));
        assert!(ingest.song.is_empty(), "a loop carries no ingestion");
        assert_eq!(ingest.span, 16);
        assert!(
            !recorder.calls.borrow().iter().any(|(_, path, _)| path.contains("/author/grade/matrix")),
            "a loop never asks for the song's digest"
        );
    }

    /// The grid paints one square button per sequence, plus the `+` that
    /// opens the two-way menu — so the library is the whole listing, not
    /// a row per movement. 2026-09-22 (afternoon): the empty grid itself is
    /// painted as built-but-empty phantom squares (asserted separately).
    #[test]
    fn the_grid_paints_one_button_per_sequence_plus_create() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond(
            "GET",
            "/author/models/library",
            json!({ "movements": [
                { "name": "drop", "kind": "model", "ticks": 3, "favourite": false },
                { "name": "pad", "kind": "model", "ticks": 4, "favourite": true }
            ] }),
        );
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();


        harness.get_by_role_and_label(egui::accesskit::Role::Button, "drop");
        harness.get_by_label("pad ★");
        the_overlays_plus(&harness);
    }

    /// The `+` cell in the overlays grid. `menu_button` draws its `⏵`
    /// affordance as a child accesskit node that inherits the button's own
    /// label, so an exact-label query finds it twice (the cell and the
    /// affordance); the real button is the bigger of the two.
    fn the_overlays_plus<'a>(
        harness: &'a egui_kittest::Harness<'static, Shell>,
    ) -> egui_kittest::Node<'a> {
        harness
            .query_all_by_label("+")
            .max_by(|a, b| a.rect().area().total_cmp(&b.rect().area()))
            .expect("the overlays grid should paint its + cell")
    }

    /// 2026-09-22 (afternoon): with nothing saved the grid is not a lone row —
    /// the `+` and the two master controls sit in a built-but-empty grid,
    /// and the old "— no sequences —" line (which used to be the only
    /// acknowledgement of an empty library) is gone. The `+` itself is the
    /// proof the grid exists.
    #[test]
    fn an_empty_overlays_grid_still_paints_its_plus_and_drops_the_empty_line() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        recorder.respond("GET", "/switch", json!({ "switches": [] }));
        recorder.respond("GET", "/overlay/blackout", json!({ "on": false }));
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();

        the_overlays_plus(&harness);
        assert!(
            harness
                .query_by_label("— no sequences: create one with + create sequence —")
                .is_none(),
            "the empty-library line is gone: the grid shows it is empty"
        );
    }

    /// `S.AK`/`O.28`: the grid paints one button per `GET /switch` entry,
    /// beside the sequence buttons — "the grid does not tell them apart... except by
    /// the icon: it is a button grid" (`O.28` point 4).
    #[test]
    fn the_grid_paints_one_button_per_switch() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        recorder.respond(
            "GET",
            "/switch",
            json!({ "switches": [
                { "id": "mood-1", "name": "mood", "kind": "toggle", "on": false },
                { "id": "strobo-1", "name": "strobo", "kind": "hold", "on": false },
            ] }),
        );
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();

        harness.get_by_label("mood");
        harness.get_by_label("strobo");
    }

    /// A click on an off `toggle` switch posts `{op: "on"}`; the same
    /// button, now on, posts `{op: "off"}` — the latch `O.28` describes for
    /// a mood.
    #[test]
    fn clicking_a_toggle_switch_posts_on_then_off() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        recorder.respond(
            "GET",
            "/switch",
            json!({ "switches": [{ "id": "mood-1", "name": "mood", "kind": "toggle", "on": false }] }),
        );
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();

        let button = harness.get_by_label("mood");
        button.scroll_to_me();
        harness.run();
        harness.get_by_label("mood").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/switch")
            .map(|(_, _, params)| body_of(params))
            .next()
            .expect("clicking an off toggle switch should POST /switch");
        assert_eq!(posted, json!({ "op": "on", "id": "mood-1" }));
    }

    /// G1: a `fixture` switch paints as its own cell — its glyph and the
    /// fixture name — and a click fires the same `POST /switch {op:"on"}` a
    /// `toggle` switch does, against the same switch id.
    #[test]
    fn a_fixture_cell_paints_and_fires() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        recorder.respond(
            "GET",
            "/switch",
            json!({ "switches": [
                { "id": "sw-1", "name": "wash 1", "kind": "fixture", "fixture": "wash-1",
                  "graph": "g1", "set": [{ "node": "venue-1", "on": true }], "on": false }
            ] }),
        );
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();

        let button = harness.get_by_label("◉ wash 1");
        button.scroll_to_me();
        harness.run();
        harness.get_by_label("◉ wash 1").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/switch")
            .map(|(_, _, params)| body_of(params))
            .next()
            .expect("clicking an off fixture cell should POST /switch");
        assert_eq!(posted, json!({ "op": "on", "id": "sw-1" }));
    }

    /// G1: a `rig` cell fires against the saved switch's own id (not the
    /// group it drives) — the same `POST /switch {op:"on"}` a `toggle` posts.
    #[test]
    fn a_rig_cell_fires_its_switch_id_not_its_group() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        recorder.respond(
            "GET",
            "/switch",
            json!({ "switches": [
                { "id": "sw-2", "name": "front wash", "kind": "rig", "group": "front-wash",
                  "graph": "g1", "set": [{ "node": "venue-1", "on": true }], "on": false }
            ] }),
        );
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();

        harness.get_by_label("▣ front wash").scroll_to_me();
        harness.run();
        harness.get_by_label("▣ front wash").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/switch")
            .map(|(_, _, params)| body_of(params))
            .next()
            .expect("clicking an off rig cell should POST /switch");
        assert_eq!(posted, json!({ "op": "on", "id": "sw-2" }));
    }

    /// `S.O.4a`: the grid shows `GET /sequences` entries as buttons too,
    /// distinct from the movement library; clicking an idle one posts
    /// `run` on the panel's current device.
    #[test]
    fn a_sequence_button_runs_it() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [{ "id": "probe" }] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        recorder.respond(
            "GET",
            "/sequences",
            json!({ "sequences": [{ "id": "opening", "name": "opening" }], "running": [] }),
        );
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();

        let button = harness.get_by_label("opening");
        button.scroll_to_me();
        harness.run();
        harness.get_by_label("opening").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/sequences")
            .map(|(_, _, params)| body_of(params))
            .next()
            .expect("clicking an idle sequence button should POST /sequences");
        assert_eq!(
            posted,
            json!({ "op": "run", "id": "opening", "device": "probe" })
        );
    }

    /// P5: the grid reads a sequence's own trigger for `device`/`layer`/`loop`
    /// off `/state`, falling back to the caller's defaults when there is no
    /// trigger yet.
    #[test]
    fn the_grid_reads_device_and_layer_from_the_trigger() {
        let state_val = json!({
            "outputs": [
                { "id": "a", "layers": [{ "id": "l1" }], "triggers": [] },
                { "id": "b", "layers": [{ "id": "l2" }], "triggers": [
                    { "id": "seq-x", "target": "overlay-x",
                      "params": { "device": "b", "layer": "l2", "loop": true } }
                ] },
            ]
        });
        assert_eq!(
            trigger_target(&state_val, "x"),
            Some(("b".to_string(), "l2".to_string(), true)),
            "the trigger's own params win over the first output"
        );
        assert_eq!(
            trigger_target(&state_val, "nope"),
            None,
            "no trigger means the caller's defaults"
        );
    }

    /// The running sequence's button is accented and carries its playhead
    /// tick as a bar at the foot — read straight off `/state.overlays[].tick`
    /// (rule 5). Inspects the frame's rect shapes: the button's accent
    /// stroke is `theme.phosphor` on a cell-sized rect (the grid's fixed
    /// columns share the width, so a cell is narrower than the panel), and
    /// the bar is a phosphor fill one hairline tall.
    #[test]
    fn a_running_sequence_button_is_accented_and_shows_its_tick() {
        fn collect_rects(shape: &egui::Shape, out: &mut Vec<egui::epaint::RectShape>) {
            match shape {
                egui::Shape::Rect(r) => out.push(r.clone()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| collect_rects(s, out)),
                _ => {}
            }
        }

        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/state",
            json!({
                "outputs": [{ "id": "screen-1", "layers": [{ "id": "l0" }] }],
                "overlays": [{ "device": "screen-1", "layer": "l0", "movement": "drop", "tick": 5 }]
            }),
        );
        recorder.respond(
            "GET",
            "/author/models/library",
            json!({ "movements": [{ "name": "drop", "kind": "model", "ticks": 10, "favourite": false }] }),
        );
        let theme = Theme::phosphor();
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();


        let mut rects = Vec::new();
        for cs in &harness.output().shapes {
            collect_rects(&cs.shape, &mut rects);
        }
        let accented = rects
            .iter()
            .filter(|r| {
                r.rect.width() > theme.gear_size
                    && r.rect.width() * 4.0 < harness.ctx.content_rect().width()
                    && r.stroke.color == theme.phosphor
            })
            .count();
        assert!(
            accented >= 1,
            "the running sequence's button should be painted with the accent stroke"
        );
        let tick_bars = rects
            .iter()
            .filter(|r| r.fill == theme.phosphor && r.rect.height() <= theme.hairline + 0.5)
            .count();
        assert!(
            tick_bars >= 1,
            "the running sequence should show its playhead tick as a bar"
        );
    }

    /// The `+` that grows the grid lives in the page row now, one column left
    /// of the next-page (`n/m ›`) cell.
    #[test]
    fn the_plus_sits_beside_the_next_page_cell() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        recorder.respond("GET", "/switch", json!({ "switches": [] }));
        recorder.respond("GET", "/overlay/blackout", json!({ "on": false }));
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();

        let add = the_overlays_plus(&harness).rect();
        let next = harness.get_by_label_contains("›").rect();
        assert!(add.left() < next.left(), "the + is left of the next-page cell: {add:?} {next:?}");
        assert!(
            (add.top() - next.top()).abs() < 1.0,
            "the + shares the page row with the next-page cell: {add:?} {next:?}"
        );
    }

    /// The left edge of each `s{i}` and `w{i}` button, paging the grid until
    /// all of them have been seen. The fixed row and the colour built-ins
    /// take the first slots, so a kind's cells need not share page one.
    fn xs_across_pages(harness: &mut egui_kittest::Harness<'static, Shell>, n: usize) -> (Vec<f32>, Vec<f32>) {
        let mut s: Vec<Option<f32>> = vec![None; n];
        let mut w: Vec<Option<f32>> = vec![None; n];
        for _ in 0..24 {
            for i in 0..n {
                if s[i].is_none() {
                    s[i] = harness.query_by_label(&format!("s{i}")).map(|x| x.rect().left());
                }
                if w[i].is_none() {
                    w[i] = harness.query_by_label(&format!("w{i}")).map(|x| x.rect().left());
                }
            }
            if s.iter().chain(&w).all(Option::is_some) {
                break;
            }
            if harness.query_by_label("›").is_none() {
                break;
            }
            harness.get_by_label("›").click();
            harness.run();
        }
        (s.into_iter().flatten().collect(), w.into_iter().flatten().collect())
    }

    /// `R22.3` decision 2: every button — fixed controls, movements,
    /// sequences and switches — is one cell of the same placed grid.
    /// Enough of each saved kind is scripted that a sequence and a switch
    /// each cover every column, so the two must land on the same column
    /// `x`s; a per-row `horizontal_wrapped` could not promise that.
    #[test]
    fn every_button_shares_the_columns_of_one_grid() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/state",
            json!({ "outputs": [{ "id": "screen-1" }] }),
        );
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        let sequences: Vec<Value> = (0..12)
            .map(|i| json!({ "id": format!("s{i}"), "name": format!("s{i}") }))
            .collect();
        recorder.respond(
            "GET",
            "/sequences",
            json!({ "sequences": sequences, "running": [] }),
        );
        let switches: Vec<Value> = (0..12)
            .map(|i| {
                json!({ "id": format!("w{i}"), "name": format!("w{i}"),
                        "kind": "toggle", "on": false })
            })
            .collect();
        recorder.respond("GET", "/switch", json!({ "switches": switches }));

        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();

        let (sequence_x, switch_x) = xs_across_pages(&mut harness, 12);
        // Twelve consecutive cells cover every column of the grid, so each
        // kind sees the whole column set — and a sequence and a switch in
        // the same column must land on the same `x`.
        let columns_of = |xs: &[f32]| {
            let mut v: Vec<f32> = xs.to_vec();
            v.sort_by(f32::total_cmp);
            v.dedup_by(|a, b| (*a - *b).abs() < 0.5);
            v
        };
        let sequence_columns = columns_of(&sequence_x);
        let switch_columns = columns_of(&switch_x);
        assert!(
            sequence_columns.len() == switch_columns.len()
                && sequence_columns
                    .iter()
                    .zip(&switch_columns)
                    .all(|(a, b)| (a - b).abs() < 0.5),
            "a sequence and a switch in the same column must share x: \
             sequences {sequence_columns:?}, switches {switch_columns:?}"
        );
    }
    // -- grid layout: the packer, the pages, the sizes ----------------------

    /// The saved map is read leniently: a `[w,h]` off `surface::SPANS` or a
    /// shape that is not a pair is dropped, the good entries kept — the
    /// halves and thirds among them.
    #[test]
    fn parse_spans_keeps_only_the_sizes_the_grid_can_paint() {
        let spans = parse_spans(&json!({
            "mv:drop": [2, 1],
            "sw:bad": [3, 1],
            "seq:short": [1],
            "sw:mood-1": [1, 2],
            "sw:thin": [1, 0.3],
            "sw:half": [0.5, 1]
        }));
        assert_eq!(spans.len(), 4);
        assert_eq!(spans["mv:drop"], (2.0, 1.0));
        assert_eq!(spans["sw:mood-1"], (1.0, 2.0));
        assert_eq!(spans["sw:thin"], (1.0, 0.3));
        assert_eq!(spans_json(&spans)["sw:thin"], json!([1, 0.3]));
        assert_eq!(spans_json(&spans)["mv:drop"], json!([2, 1]));
        assert!(parse_spans(&json!({})).is_empty());
    }

    /// A MIDI key bound to a switch fires it while the sequence editor is
    /// open: the grid is off screen, so the shell's off-screen drive is what
    /// hears the key.
    #[test]
    fn a_bound_midi_key_fires_its_switch_while_the_sequence_is_open() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        recorder.respond("GET", "/switch", json!({ "switches": [
            { "id": "strobe", "name": "strobe", "kind": "toggle", "on": false }
        ] }));
        recorder.respond(
            "GET",
            "/settings",
            json!({ "grid_page": 0, "grid_surface": {
                "binds": [{ "input": "note:60", "page": 0, "cell": "sw:strobe" }]
            } }),
        );
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 900.0);
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        assert!(harness.query_by_label_contains("›").is_none(), "the grid is off screen");

        crate::input::drain_surface();
        crate::input::push_surface(crate::input::SurfaceEvent {
            port: "FLkey Mini".into(),
            input: "note:60".into(),
            edge: metal_midi::Edge::Press { velocity: 1.0 },
        });
        harness.run();
        let fired = recorder.calls.borrow().iter().any(|(m, p, params)| {
            m == "POST" && p == "/switch" && body_of(params) == json!({ "op": "on", "id": "strobe" })
        });
        assert!(fired, "the key turns the switch on through the off-screen drive");
    }

    /// Picking the FLkey Mini in the bar's controller menu gives the grid its
    /// face, every control blank: the first pad then fires the first switch
    /// (the key of the same note, another channel, does not); a job given to
    /// the record button, a fader to a pot and a button to a piano key each
    /// answer their control.
    #[test]
    fn the_flkey_face_is_blank_controls_each_given_its_cell() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        recorder.respond("GET", "/switch", json!({ "switches": [
            { "id": "strobe", "name": "strobe", "kind": "toggle", "on": false }
        ] }));
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();
        harness.get_by_label("Custom Grid 1 ▾").click();
        harness.run();
        harness.get_by_label("Create a new layout from hardware").click();
        harness.run();
        harness.get_by_label("Novation FLkey Mini").click();
        harness.run();
        assert!(harness.query_by_label("Name the layout").is_some(), "a new layout asks for its name");
        harness.get_by_label("cancel").click();
        harness.run();
        let posted = recorder.calls.borrow().iter().any(|(m, p, params)| {
            m == "POST" && p == "/settings" && body_of(params)["grid_surface"]["device"] == json!("flkey-mini")
        });
        assert!(posted, "the face is stored");
        assert!(harness.query_by_label("piano").is_some());
        for printed in ["shift", "shift\nfn", "speed", "rec", "octave +"] {
            assert!(harness.query_by_label(printed).is_none(), "{printed:?} is not painted: the controls are blank");
        }

        let send = |input: &str, edge: metal_midi::Edge| {
            crate::input::push_surface(crate::input::SurfaceEvent { port: "FLkey Mini".into(), input: input.into(), edge })
        };
        let press = |input: &str| send(input, metal_midi::Edge::Press { velocity: 1.0 });
        let posted_with = |r: &FakeDoor, path: &str, body: Value| {
            r.calls.borrow().iter().any(|(m, p, params)| m == "POST" && p == path && body_of(params) == body)
        };
        let switch_posts = |r: &FakeDoor| r.calls.borrow().iter().filter(|(m, p, _)| m == "POST" && p == "/switch").count();
        assert!(harness.query_by_label("strobe").is_none(), "the grid's own buttons wait in the controls' menus");
        crate::input::drain_surface();
        press("note:40@10");
        harness.run();
        assert_eq!(switch_posts(&recorder), 0, "an empty pad fires nothing");

        {
            let state = harness.state_mut().lanes_state_mut();
            let slot = |state: &PanelState, id: &str| state.layout.slots.iter().position(|s| s.id == id).unwrap();
            let pad = slot(state, "pad-1");
            assert!(state.face_pool.iter().any(|(k, _)| k == "sw:strobe"), "the switch is offered to the pad");
            state.layout.place("sw:strobe", pad, Some(0));
            let record = slot(state, "record");
            state.layout.place("hw:record", record, None);
            state.layout.roles.insert("hw:record".into(), crate::surface::Role::Rec);
            let pot = slot(state, "pot-1");
            // The node's float is there to be the fader (a deleted one is not).
            recorder.respond("GET", "/graph/io", json!({ "params": [{ "name": "gain", "kind": "float", "min": 0.0, "max": 1.0 }] }));
            state.controllers.insert(
                "hw:pot-1:0".into(),
                json!({ "graph": "g", "name": "", "mode": "switch", "targets": [{ "node": "glow", "param": "gain", "value": 0.0 }] }),
            );
            state.layout.place("ctrl:hw:pot-1:0", pot, Some(0));
            assert_eq!(slot_of_controller(state, "hw:pot-1:0"), Some(pot), "its editor offers the jobs for that pot");
            state.controllers.insert(
                "key:48:0".into(),
                json!({ "graph": "g", "name": "", "mode": "switch", "targets": [{ "node": "strobe-node", "on": true }] }),
            );
        }
        harness.run();
        press("note:40@1");
        harness.run();
        assert_eq!(switch_posts(&recorder), 0, "a key is not the pad of the same note");
        press("note:40@10");
        harness.run();
        assert!(posted_with(&recorder, "/switch", json!({ "op": "on", "id": "strobe" })), "the pad fires the switch given it");
        // A CC button sends 127 down, 0 up; `rec` records on its release.
        for raw in [127u8, 0] {
            send("cc:117@1", metal_midi::Edge::Value { value: raw as f32 / 127.0, raw });
            harness.run();
        }
        assert!(harness.state_mut().lanes_state_mut().surface.is_recording(), "the record button was given rec");
        send("cc:21@1", metal_midi::Edge::Value { value: 0.5, raw: 64 });
        harness.run();
        assert!(
            posted_with(&recorder, "/graph/node", json!({ "op": "param", "graph": "g", "node": "glow", "name": "gain", "value": 0.5 })),
            "the pot is the fader it holds"
        );
        press("note:48@1");
        harness.run();
        assert!(
            posted_with(&recorder, "/graph/node", json!({ "op": "flag", "graph": "g", "node": "strobe-node", "field": "bypass", "on": true })),
            "the key fires its button"
        );
    }

    /// The layouts menu: a box's face is made a new layout from its
    /// template and named in the popup; going back to the first parks the
    /// face's controllers with it, and they come back with it; `Rename`
    /// renames the one up; `Delete` brings up the one left.
    #[test]
    fn layouts_are_made_from_hardware_named_switched_and_deleted() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();
        harness.get_by_label("Custom Grid 1 ▾").click();
        harness.run();
        assert!(harness.query_by_label("Novation FLkey Mini").is_none(), "the boxes wait in their submenu");
        harness.get_by_label("Create a new layout from hardware").click();
        harness.run();
        harness.get_by_label("Novation FLkey Mini").click();
        harness.run();
        harness.state_mut().lanes_state_mut().layout_name.as_mut().expect("the name popup").text = "Club rig".into();
        harness.get_by_label("save").click();
        harness.run();
        let ctrl = json!({ "graph": "g", "name": "", "mode": "switch", "targets": [{ "node": "n", "on": true }] });
        {
            let state = harness.state_mut().lanes_state_mut();
            assert_eq!(state.layouts.list.len(), 2);
            assert_eq!(state.layouts.current().unwrap().name, "Club rig");
            assert_eq!(state.layout.device.as_deref(), Some("flkey-mini"));
            state.controllers.insert("hw:pad-1:0".into(), ctrl.clone());
        }
        harness.get_by_label("Club rig ▾").click();
        harness.run();
        harness.get_by_label("Custom Grid 1").click();
        harness.run();
        {
            let state = harness.state_mut().lanes_state_mut();
            assert!(state.layout.device.is_none(), "the free grid is back");
            assert!(!state.controllers.contains_key("hw:pad-1:0"), "the face's buttons are parked");
        }
        let stored = recorder.calls.borrow().iter().rev().find_map(|(m, p, params)| {
            (m == "POST" && p == "/settings").then(|| body_of(params)).filter(|b| b["grid_layouts"].is_object())
        });
        let stored = stored.expect("the layouts are posted");
        assert_eq!(stored["grid_layouts"]["active"], json!("grid-1"));
        assert_eq!(stored["grid_layouts"]["list"][1]["controllers"]["hw:pad-1:0"], ctrl);

        harness.get_by_label("Custom Grid 1 ▾").click();
        harness.run();
        harness.get_by_label("Club rig").click();
        harness.run();
        assert_eq!(harness.state_mut().lanes_state_mut().controllers.get("hw:pad-1:0"), Some(&ctrl), "and come back");

        harness.get_by_label("Club rig ▾").click();
        harness.run();
        harness.get_by_label("Rename").click();
        harness.run();
        harness.state_mut().lanes_state_mut().layout_name.as_mut().expect("the rename popup").text = "Hotel".into();
        harness.get_by_label("save").click();
        harness.run();
        assert_eq!(harness.state_mut().lanes_state_mut().layouts.current().unwrap().name, "Hotel");

        harness.get_by_label("Hotel ▾").click();
        harness.run();
        harness.get_by_label("Delete").click();
        harness.run();
        harness.get_by_label("delete").click();
        harness.run();
        let state = harness.state_mut().lanes_state_mut();
        assert_eq!(state.layouts.list.len(), 1);
        assert_eq!(state.layouts.current().unwrap().name, "Custom Grid 1");
        assert!(state.layout.device.is_none() && !state.controllers.contains_key("hw:pad-1:0"));
    }

    /// `learn the face` takes each control as the operator touches it: a
    /// knob's further turns and a button's release do not move the walk on,
    /// and the last step sets the piano from its lowest key.
    #[test]
    fn the_piano_follows_the_box_octave_by_octave() {
        use crate::surface::Role;
        let mut l = crate::surface::Layout::default();
        l.apply_device(&metal_midi::devices::device("flkey-mini").unwrap());
        let base = |l: &crate::surface::Layout| match l.roles.get("hw:keys") {
            Some(Role::Piano { base, .. }) => *base,
            _ => 0,
        };
        assert!(!follow_octave(&mut l, 72), "the top C is on the piano");
        assert!(follow_octave(&mut l, 73));
        assert_eq!(base(&l), 60);
        assert!(follow_octave(&mut l, 30));
        assert_eq!(base(&l), 24);
    }

    #[test]
    fn learning_the_face_takes_each_control_as_it_is_touched() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();
        {
            let state = harness.state_mut().lanes_state_mut();
            state.layout.apply_device(&metal_midi::devices::device("generic-25-keys").unwrap());
        }
        harness.run();
        harness.get_by_label("Custom Grid 1 ▾").click();
        harness.run();
        // The walk starts from a control's menu (`learn the face` there).
        harness.state_mut().lanes_state_mut().face_learn = Some(0);
        harness.run();
        assert_eq!(harness.state_mut().lanes_state_mut().face_learn, Some(0), "the walk starts");
        let send = |input: &str, edge: metal_midi::Edge| {
            crate::input::push_surface(crate::input::SurfaceEvent { port: "box".into(), input: input.into(), edge });
        };
        crate::input::drain_surface();
        // The wheels first: the pitch wheel turned a lot, then the mod wheel.
        for v in [0.5, 0.6, 0.7] {
            send("bend@1", metal_midi::Edge::Value { value: v, raw: 64 });
            harness.run();
        }
        send("cc:9@2", metal_midi::Edge::Value { value: 1.0, raw: 127 });
        harness.run();
        let (first, second, at) = {
            let state = harness.state_mut().lanes_state_mut();
            (state.layout.slots[0].input.clone(), state.layout.slots[1].input.clone(), state.face_learn)
        };
        assert_eq!((first.as_deref(), second.as_deref(), at), (Some("bend@1"), Some("cc:9@2"), Some(2)));
        // Skip to the end: the piano's lowest key.
        let n = harness.state_mut().lanes_state_mut().layout.slots.len();
        harness.state_mut().lanes_state_mut().face_learn = Some(n);
        send("note:36@1", metal_midi::Edge::Press { velocity: 1.0 });
        harness.run();
        let state = harness.state_mut().lanes_state_mut();
        assert!(state.face_learn.is_none(), "the walk ends");
        assert!(state.layout.roles.values().any(|r| *r == crate::surface::Role::Piano { base: 36, keys: 25 }));
    }

    /// `GET /settings` is read once, and a 2×1 picked in the cell's menu is
    /// posted as the whole map (`POST /settings {grid_spans}`) — the one place
    /// the size is written.
    #[test]
    fn picking_a_size_in_the_menu_posts_the_whole_span_map() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        recorder.respond("GET", "/switch", json!({ "switches": [
            { "id": "mood-1", "name": "mood", "kind": "toggle", "on": false }
        ] }));
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();

        harness.get_by_label("mood").click_secondary();
        harness.run();
        harness.get_by_label("2×1").click();
        harness.run();

        let calls = recorder.calls.borrow();
        let posted = calls
            .iter()
            .find(|(m, p, _)| m == "POST" && p == "/settings")
            .map(|(_, _, params)| body_of(params))
            .expect("choosing a size posts /settings");
        assert_eq!(posted["grid_spans"], json!({ "sw:mood-1": [2, 1] }));
    }

    /// Forty switches cannot fit the fixed 8 × 4 grid: it pages, and
    /// the `‹`/`›` cells in its last row turn the page. The stored page is
    /// the one thing a click changes, so each page is scripted through
    /// `GET /settings`: a page holds switches, no two pages share one, a
    /// click on `›` stores the next page, and a stored page past the last
    /// one is clamped back to it (and written back).
    #[test]
    fn forty_switches_page_the_fixed_grid_and_the_page_buttons_turn_it() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        recorder.respond("GET", "/settings", json!({ "grid_page": 0 }));
        let switches: Vec<Value> = (0..40)
            .map(|i| json!({ "id": format!("w{i}"), "name": format!("s{i:02}"), "kind": "toggle", "on": false }))
            .collect();
        recorder.respond("GET", "/switch", json!({ "switches": switches }));
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 800.0);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();

        // The switches a page paints, by number.
        let painted = |h: &egui_kittest::Harness<'static, Shell>| -> Vec<usize> {
            (0..40).filter(|i| h.query_by_label(&format!("s{i:02}")).is_some()).collect()
        };

        // Store each page in turn and collect what it paints, until the walk
        // runs past the last page.
        let mut seen: Vec<usize> = Vec::new();
        let mut last_page_with_switches = 0;
        for page in 0..30 {
            recorder.respond("GET", "/settings", json!({ "grid_page": page }));
            harness.run();
            let here = painted(&harness);
            if here.is_empty() {
                continue;
            }
            // A page past the last one is clamped to it: it repeats what an
            // earlier page showed, which ends the walk.
            if here.iter().all(|i| seen.contains(i)) {
                break;
            }
            assert!(
                here.iter().all(|i| !seen.contains(i)),
                "no switch is painted on two pages: seen {seen:?}, page {here:?}"
            );
            seen.extend(&here);
            last_page_with_switches = page;
        }
        assert_eq!(seen.len(), 40, "every switch lands on some page: {seen:?}");

        // `›` on the first page stores page 1.
        recorder.respond("GET", "/settings", json!({ "grid_page": 0 }));
        harness.run();
        harness.get_by_label_contains("›").click();
        harness.run();
        let stored = |page: i64| {
            recorder.calls.borrow().iter().any(|(m, p, body)| {
                m == "POST" && p == "/settings" && body.json().is_some_and(|b| b["grid_page"] == json!(page))
            })
        };
        assert!(stored(1), "the next button stores page 1");

        // A stored page past the last one is clamped back to it and written back.
        recorder.respond("GET", "/settings", json!({ "grid_page": 999 }));
        harness.run();
        assert!(
            stored(last_page_with_switches as i64),
            "a page past the end clamps to the last page, {last_page_with_switches}"
        );
    }

    // -- R22.8: the ingest preview, its mask, and the saved mask -----------

    /// A door whose `/state` carries one output with one mask, and a model to
    /// author — the setup every `R22.8` kittest shares.
    fn preview_door() -> std::rc::Rc<FakeDoor> {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/state",
            json!({ "outputs": [{
                "id": "device-preview",
                "masks": [{ "id": "mask-1", "name": "tallo" }]
            }] }),
        );
        recorder.respond(
            "GET",
            "/author/models/state",
            json!({ "models": [{ "id": "geoterrain", "label": "GeoTerrain", "width": 2 }] }),
        );
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        // The auto-load (`needs_load`) fetches the example and would wipe the
        // keyframes a test seeded; script one tick so the load keeps them.
        recorder.respond(
            "GET",
            "/author/models/example",
            json!({ "ticks": [{ "tick": 0, "floats": [0.5, 0.25] }] }),
        );
        recorder.respond("GET", "/author/grade/matrix", json!({ "ticks": 8 }));
        recorder
    }

    /// The dataset picker's door: one `rings` part of `magalenha` on its
    /// span, the song's matrix and its hits. Shared by the picker test and
    /// the column-fit test.
    fn dataset_picker_door() -> std::rc::Rc<FakeDoor> {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond(
            "GET",
            "/author/models/state",
            json!({ "models": [{ "id": "rings", "label": "Rings", "width": 2 }] }),
        );
        recorder.respond("GET", "/author/models/library", json!({ "movements": [] }));
        recorder.respond(
            "GET",
            "/author/dataset",
            json!({ "parts": [{
                "part": "67-p2", "model": "rings", "song": "magalenha", "span": [865, 1911],
                "kind": "build-drop", "keywords": ["samba"], "title": "samba build", "keys": 2
            }] }),
        );
        recorder.respond(
            "GET",
            "/author/dataset/example",
            json!({
                "model": "rings", "song": "magalenha", "rate": 32.0, "part": "67-p2",
                "span": [865, 1911], "kind": "build-drop",
                "ticks": [{ "tick": 865, "floats": [0.2, 0.4] }, { "tick": 1454, "floats": [0.9, 0.1] }]
            }),
        );
        let n = 2000usize;
        recorder.respond(
            "GET",
            "/author/grade/matrix",
            json!({
                "song": "magalenha", "rate": 32.0, "ticks": n,
                "rms": vec![0.1f32; n], "bandwidth": vec![0.0f32; n], "onset": vec![0.0f32; n],
                "centroid": vec![0.0f32; n], "chroma": vec![[0.0f32; 12]; n],
            }),
        );
        recorder.respond("GET", "/author/models/song-hits", json!({ "ticks": [] }));
        recorder
    }

    /// 2026-10-06: the **dataset** picker opens a Colourist dataset part as
    /// a clip of its song — song, model, keypoints, marks = span — lands the
    /// playhead on the part once the song's matrix is in, and pushes the
    /// part's row to the preview.
    #[test]
    fn picking_a_dataset_part_opens_it_as_a_clip_on_its_span() {
        let recorder = dataset_picker_door();
        let n = 2000usize;
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 3400.0);
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();
        // A closed `ComboBox` exposes its selected text as its `value`
        // (`shell/kittest.rs`'s note), so the closed picker is found by value.
        harness.get_by_value("— part").click();
        harness.run();
        harness.get_by_label(" 67-p2 · rings · build-drop · samba build").click();
        harness.run();
        harness.run();
        harness.run();

        let ingest = harness.state_mut().ingest_state_mut();
        assert_eq!(ingest.song, "magalenha");
        assert_eq!(ingest.model, "rings");
        assert_eq!(ingest.bounds, Some((865, 1911)), "the marks are the part's span");
        assert_eq!(ingest.keyframes.len(), 2);
        assert_eq!(ingest.matrix.as_ref().map(|m| m.ticks), Some(n), "the song's matrix loaded");
        assert_eq!(ingest.playhead, 865, "the playhead waits for the matrix, then lands on the part");
        assert_eq!(ingest.row, vec![0.2, 0.4]);
        let pushed: Vec<Vec<f32>> = recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/author/render/floats")
            .map(|(_, _, params)| serde_json::from_slice(&params.body).unwrap())
            .collect();
        assert_eq!(pushed.last(), Some(&vec![0.2, 0.4]), "the preview renders the part's row");
        assert!(
            !recorder.calls.borrow().iter().any(|(m, p, _)| m == "GET" && p == "/author/project"),
            "the song's saved project is not loaded over the part"
        );
    }

    /// The lanes panel keeps its content inside its column at the 1600×950
    /// desktop: the two compact bars and the timeline are sized to the
    /// column's width, so no scrollbar or the timeline itself reaches past
    /// the column's right edge.
    #[test]
    fn the_lanes_scroll_area_fits_its_column() {
        let recorder = dataset_picker_door();
        let mut harness = shell_harness_of_height(
            Box::new(std::rc::Rc::clone(&recorder)),
            crate::shell::DESKTOP_VIEWPORT.1 as f32,
        );
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        harness.get_by_label("lanes / overlays").click();
        harness.run();
        harness.run();

        let column = *harness.state().rects.get(&crate::shell::panels::Region::Lanes).expect("lanes region rect");
        let bars: Vec<egui::Rect> = harness
            .query_all(
                egui_kittest::kittest::by()
                    .predicate(|n| {
                        n.data().role() == egui::accesskit::Role::ScrollBar && n.data().bounds().is_some()
                    }),
            )
            .map(|n| n.rect())
            .collect();
        for bar in bars {
            assert!(
                bar.min.x >= column.min.x && bar.max.x <= column.max.x,
                "the scrollbar {bar:?} sits outside the lanes column {column:?}"
            );
        }
        // The compaction's point: the timeline keeps the panel rather than
        // pushing the column to scroll, and it stays inside the column.
        let canvas = harness
            .state_mut()
            .ingest_state_mut()
            .canvas_rect
            .expect("the timeline should have painted");
        assert!(
            canvas.min.x >= column.min.x && canvas.max.x <= column.max.x,
            "the timeline {canvas:?} sits outside the lanes column {column:?}"
        );
    }

    fn solo_calls(recorder: &FakeDoor) -> Vec<crate::call::Params> {
        recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "GET" && p == "/author/render/solo.jpg")
            .map(|(_, _, params)| params.clone())
            .collect()
    }

    /// Point 3: the preview appears once a model is chosen and stays hidden
    /// without one.
    #[test]
    fn lanes_preview_appears_with_a_model_and_hides_without() {
        let recorder = preview_door();
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        assert!(solo_calls(&recorder).is_empty(), "no model, no preview");

        harness.state_mut().ingest_state_mut().model = "geoterrain".into();
        harness.state_mut().ingest_state_mut().row = vec![0.5, 0.25];
        harness.run();
        assert!(
            !solo_calls(&recorder).is_empty(),
            "a chosen model should pump /author/render/solo.jpg"
        );
    }

    /// Point 2: the chosen mask (and device) ride the solo query.
    #[test]
    fn picking_a_mask_adds_it_to_the_solo_query() {
        let recorder = preview_door();
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        {
            harness.state_mut().ingest_state_mut().model = "geoterrain".into();
            harness.state_mut().ingest_state_mut().row = vec![0.5, 0.25];
            harness.state_mut().ingest_state_mut().mask = Some("mask-1".into());
            harness.state_mut().ingest_state_mut().device = Some("device-preview".into());
        }
        harness.run();

        let last = solo_calls(&recorder).pop().expect("a solo call");
        assert_eq!(last.query.get("mask").map(String::as_str), Some("mask-1"));
        assert_eq!(
            last.query.get("device").map(String::as_str),
            Some("device-preview")
        );
        assert_eq!(last.query.get("model").map(String::as_str), Some("geoterrain"));
    }

    /// Point 4: `save also as movement` writes the chosen mask into
    /// the posted `Movement`.
    #[test]
    fn saving_a_sequence_records_its_mask() {
        let recorder = preview_door();
        let mut harness = shell_harness_of_height(Box::new(std::rc::Rc::clone(&recorder)), 2600.0);
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        {
            harness.state_mut().ingest_state_mut().model = "geoterrain".into();
            harness.state_mut().ingest_state_mut().song = "click-100".into();
            harness.state_mut().ingest_state_mut().keyframes.insert(0, vec![0.5, 0.25]);
            harness.state_mut().ingest_state_mut().mask = Some("mask-1".into());
        }
        harness.run();

        open_save_menu(&mut harness);
        harness.get_by_label("save as sequence").click();
        harness.run();

        let posted = recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/author/models/library")
            .map(|(_, _, params)| body_of(params))
            .next()
            .expect("the save should POST /author/models/library");
        assert_eq!(posted["mask"], "mask-1");
    }

    // -- L22.11: drag of `files` to the overlays grid ----------------

    fn file_drag(name: &str, kind: &str) -> crate::shell::panels::FileDrag {
        crate::shell::panels::FileDrag {
            name: name.to_string(),
            path: format!("/media/{name}"),
            kind: kind.to_string(),
            seconds: None,
            scene: None,
        }
    }

    /// A `sequence` dropped on the grid loads into the scratch and opens the
    /// lanes — the `load` path, without the button.
    #[test]
    fn dropping_a_sequence_on_the_grid_loads_it_into_the_scratch() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/author/models/movement",
            json!({ "name": "drop", "model": "geoterrain", "ticks": [[0.5, 0.0]] }),
        );
        let mut state = PanelState::default();
        apply_overlay_drop(&recorder, &mut state, &file_drag("drop.json", "sequence"));

        assert_eq!(state.mode_override, Some(crate::shell::lanes::Mode::Lanes));
        assert_eq!(state.mv.name, "drop");
        assert_eq!(state.mv.note.as_deref(), Some("opened: drop.json"));
    }

    /// An `audio` dropped on the grid raises the intent `play` does — the
    /// lanes panel holds the name; `shell/mod.rs` drains it into ingest.
    #[test]
    fn dropping_audio_on_the_grid_raises_the_ingest_intent() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        let mut state = PanelState::default();
        apply_overlay_drop(&recorder, &mut state, &file_drag("a.mp3", "audio"));

        assert_eq!(state.pending_song.as_deref(), Some("a.mp3"));
        assert_eq!(state.mode_override, Some(crate::shell::lanes::Mode::Lanes));
    }

    /// A `video` dropped on the grid opens the destination popover, not a
    /// load: the draft carries the card, waiting for an output/mask pick.
    #[test]
    fn dropping_video_on_the_grid_opens_the_destination_popover() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        let mut state = PanelState::default();
        apply_overlay_drop(&recorder, &mut state, &file_drag("clip.mp4", "video"));

        let draft = state.new_video.as_ref().expect("video opens the popover");
        assert_eq!(draft.name, "clip.mp4");
        assert_eq!(draft.path, "/media/clip.mp4");
        assert!(draft.target.is_empty(), "no destination picked yet");
    }

    /// An image on the grid is refused — the overlays grid is not a graph
    /// canvas; nothing loads, nothing opens.
    #[test]
    fn dropping_an_image_on_the_grid_does_nothing() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        let mut state = PanelState::default();
        apply_overlay_drop(&recorder, &mut state, &file_drag("photo.png", "image"));

        assert!(state.pending_song.is_none());
        assert!(state.new_video.is_none());
        assert!(state.mode_override.is_none());
        assert!(state.mv.name.is_empty());
    }

    /// The `video` popover's write half: a `reel` node with the clip's path,
    /// then a `toggle` switch over it whose name says the destination — the
    /// button the ask wanted ("converted to a button that triggers the
    /// video, only requiring to select on which output device (or mask)").
    #[test]
    fn creating_a_video_button_adds_a_reel_and_a_switch_over_it() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond(
            "POST",
            "/graph/node",
            json!({ "graph": { "id": "g1", "nodes": [] }, "primitives": [], "issues": [], "node": "reel-0" }),
        );
        let draft = NewVideo {
            name: "clip.mp4".into(),
            path: "/media/clip.mp4".into(),
            target: "screen-1".into(),
            mask: false,
        };
        let name = create_video_button(&recorder, Some("g1"), &draft).expect("should create");

        assert_eq!(name, "video: clip.mp4 → screen-1");
        let calls = recorder.calls.borrow();
        let added = calls
            .iter()
            .find(|(m, p, params)| {
                m == "POST"
                    && p == "/graph/node"
                    && serde_json::from_slice::<Value>(&params.body)
                        .map(|b| b["op"] == "add" && b["kind"] == "reel.clip")
                        .unwrap_or(false)
            })
            .expect("a video button should add a reel node");
        let body: Value = serde_json::from_slice(&added.2.body).expect("json");
        assert_eq!(body["kind"], "reel.clip");
        assert!(calls.iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/graph/node"
                && serde_json::from_slice::<Value>(&params.body)
                    .map(|b| b["op"] == "config" && b["config"]["file"] == "/media/clip.mp4")
                    .unwrap_or(false)
        }));
        let switched = calls
            .iter()
            .find(|(m, p, _)| m == "POST" && p == "/switch")
            .map(|(_, _, params)| serde_json::from_slice::<Value>(&params.body).expect("json"))
            .expect("the video button should add a switch");
        assert_eq!(switched["kind"], "toggle");
        assert_eq!(switched["set"], json!([{ "node": "reel-0", "on": true }]));
    }

    /// Without an open graph the video button refuses, named — the popover
    /// says so and offers no `create`.
    #[test]
    fn a_video_button_without_a_graph_refuses_by_name() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        let draft = NewVideo {
            name: "clip.mp4".into(),
            path: "/media/clip.mp4".into(),
            target: "screen-1".into(),
            mask: false,
        };
        let err = create_video_button(&recorder, None, &draft).expect_err("no graph");
        assert!(err.contains("open a graph"), "named refusal: {err}");
    }

    // -- §2: the lanes toolbars wrap over fixed rows, never scroll ----------

    /// §2: at a narrow panel the editor keeps its controls dense — the
    /// playhead/range verbs stack in a left icon column, the transport stays
    /// one row, and no control is pushed outside the viewport by a
    /// horizontal scroll.
    #[test]
    fn lanes_toolbars_wrap_at_a_narrow_width() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [] }));
        recorder.respond("GET", "/author/models/state", json!({ "models": [] }));
        recorder.respond("GET", "/author/songs", json!({ "songs": [] }));
        recorder.respond("GET", "/author/dataset", json!({ "parts": [] }));

        let width = 620.0;
        let mut harness = shell_harness_of_size(
            Box::new(std::rc::Rc::clone(&recorder)),
            width,
            900.0,
        );
        harness.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
        harness.run();
        let head = harness.get_by_label("lanes / overlays").rect();
        harness.drag_at(head.left_top() + head.size() * 0.02);
        harness.run();
        harness.drop_at(head.left_top() + head.size() * 0.02);
        harness.run();
        harness.run();

        // Top toolbar: the import menu (its `animation` second level carries
        // the models; there is no standalone combo any more).
        let import = harness.get_by_label("import").rect();

        // The left icon column, stacked top to bottom.
        let keypoint = harness.get_by_label(&glyph(Icon::Keypoint).to_string()).rect();
        let copiar = harness.get_by_label(&glyph(Icon::Copy).to_string()).rect();
        let pegar = harness.get_by_label(&glyph(Icon::Paste).to_string()).rect();
        let eliminar = harness.get_by_label(&glyph(Icon::Trash).to_string()).rect();
        let undo = harness.get_by_label(&glyph(Icon::Undo).to_string()).rect();
        let save = harness.get_by_label(&glyph(Icon::Save).to_string()).rect();

        // The transport: the four playback controllers and the clock.
        let first = harness.get_by_label(&glyph(Icon::SkipBack).to_string()).rect();

        // The icon column stacks in order…
        assert!(
            keypoint.top() < copiar.top()
                && copiar.top() < pegar.top()
                && pegar.top() < eliminar.top()
                && eliminar.top() < undo.top()
                && undo.top() < save.top(),
            "the icon column must stack: {keypoint:?} {copiar:?} {pegar:?} {eliminar:?} {undo:?} {save:?}"
        );
        // …and leads the body, to the left of the main content.
        assert!(
            save.right() <= import.left() && keypoint.right() <= import.left(),
            "the icon column must lead the body: {save:?} {import:?} {keypoint:?}"
        );
        // No horizontal scroll: every control sits inside the viewport.
        for (name, r) in [
            ("import ▾".to_string(), import),
            (glyph(Icon::Keypoint).to_string(), keypoint),
            (glyph(Icon::Copy).to_string(), copiar),
            (glyph(Icon::Paste).to_string(), pegar),
            (glyph(Icon::Trash).to_string(), eliminar),
            (glyph(Icon::Undo).to_string(), undo),
            (glyph(Icon::Save).to_string(), save),
            (glyph(Icon::SkipBack).to_string(), first),
        ] {
            assert!(
                r.left() >= 0.0 && r.right() <= width,
                "{name} must stay inside the viewport, got {r:?}"
            );
        }
    }
}

/// Native audio transport (Android, no digest): starting playback posts the
/// song to `/author/play` from the playhead, the audio clock drives the
/// playhead, and the playback ends with the audio.
#[cfg(test)]
mod audio_transport_tests {
    use super::*;
    use crate::shell::FakeDoor;
    use serde_json::json;

    #[test]
    fn an_undigested_song_starts_native_audio_and_follows_its_clock() {
        let door = FakeDoor::new();
        door.respond("GET", "/author/play", json!({ "playing": true, "at": 2.0, "native": true }));
        door.respond("POST", "/author/play", json!({ "ok": true, "playing": true, "from": 0.0, "native": true }));
        door.respond("POST", "/loops/song", json!({ "ok": true }));
        let mut state = PanelState::default();
        let mut ingest = crate::shell::lanes::IngestLane::default();
        ingest.song = "tone".into();
        assert_eq!(ingest.end_tick(), 0, "undigested: no ticks yet");

        ingest.toggle_play(0.0);
        assert!(ingest.playing.is_some(), "a song with audio plays without a digest");
        sync_audio(&door, &mut state, &mut ingest);
        let posted = door
            .calls
            .borrow()
            .iter()
            .find(|(m, p, _)| m == "POST" && p == "/author/play")
            .map(|(_, _, params)| params.json().expect("json body"))
            .expect("the transport posts /author/play");
        assert_eq!(posted["song"], "tone");
        assert_eq!(posted["from"], 0.0);

        // The next frame: the audio clock is at 2 s, and the playhead is there.
        let audible = audible_clock(&door, &ingest);
        assert_eq!(audible, Some((2.0, true)));
        assert!(ingest.advance_to(0.1, audible.map(|(at, _)| at)));
        assert_eq!(ingest.playhead, 64, "2 s of song is 64 canonical ticks");
        assert!(ingest.playing.is_some(), "still sounding, still playing");
    }

    #[test]
    fn a_song_without_a_native_clock_keeps_the_wall_clock_path() {
        let door = FakeDoor::new();
        door.respond("GET", "/author/play", json!({ "playing": false, "at": null, "native": false }));
        let mut ingest = crate::shell::lanes::IngestLane::default();
        ingest.song = "tone".into();
        ingest.toggle_play(0.0);
        assert_eq!(audible_clock(&door, &ingest), None, "ffplay has no clock to read");
    }
}
