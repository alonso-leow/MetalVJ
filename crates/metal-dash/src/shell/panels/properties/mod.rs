//! `S.U12`: the properties of the selected node — a compact node card followed
//! by collapsible sections (2026-10-08, no tabs), in the
//! [`super::Region::Properties`] region.
//!
//! **Rule 5.** This panel never re-derives what the door already resolved:
//! a node's kind/label/config comes straight off `GET /graph`'s own `graph`
//! field, its ports off `GET /catalog`, its effects off `GET /graph/effects`,
//! its I/O off `GET /graph/io` and its live values off `GET /graph/live` —
//! every value shown here is copied out of a reply, never recomputed.
//!
//! **Rule 6 / the transversal rule.** Every call goes through
//! [`crate::call::Call`] — `door.call`/`door.json` — the same `get` helper
//! `panels::nodes` already wrote for a `GET` with a query string.

mod browser;
mod cam_track;
mod chain;
mod describe;
pub(crate) mod contour;
mod effects;
pub(crate) mod ingest;
mod io;
mod layer;
mod listen;
mod loops;
mod mask;
mod material;
mod mode;
mod node;
mod output_device;
mod overlay;
mod patch;
mod poster;
pub(crate) mod render_on;
mod fixture;
mod grade;
mod image;
mod sacn;
mod artnet;
mod artnet_in;
mod timecode;
mod script;
mod sectors;
mod sensor;
pub(crate) mod shared;
mod sources;
mod source_math;
mod source_runtime;
pub(crate) mod surface;
mod surfaces;
mod text;
mod transition;

use crate::shell::components::small_text;
use std::collections::HashMap;

use egui::{Id, Response, RichText, ScrollArea, Ui};
use serde_json::json;

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{
    field_row, label_row, num, panel_frame, panel_head, section as kit_section, section_heading,
    segmented, toggle_row,
};
use crate::shell::i18n::{tr, trf};
use crate::shell::panels::Selection;

/// The `section_states` key of the lanes layer card's Layer section. The
/// header reuses `tr("layer")`; the key is distinct so opening the card's
/// section never toggles the node browser's own `layer` section.
const LAYER_CARD_SECTION: &str = "lanes/layer-card";

/// The one kind of the `Stack` family — spelled once in `metal-graph` and read
/// here as the same constant, not a second literal that could drift.
pub(crate) const STACK_MODE_KIND: &str = metal_graph::STACK_MODE_KIND;
/// The one `Text`-family kind, spelled once in `metal-graph`.
pub(crate) const TEXT_SOURCE_KIND: &str = metal_graph::TEXT_SOURCE_KIND;

/// The cached header the properties panel paints above the selected node's
/// card (task 4a): the node id it belongs to, a short title, a one-sentence
/// description and the kind's three short keypoints — all already translated.
/// `PanelState::header_info` refreshes it from the body each frame, and the
/// header paints it at the top of the panel next frame.
#[derive(Clone)]
struct HeaderInfo {
    /// The node id the descriptor belongs to, so the cache is keyed by node
    /// and the header never shows a stale selection.
    node: String,
    /// The node's kind: what its favourite star stores.
    kind: String,
    title: String,
    description: String,
    keypoints: [String; 3],
    /// The file an image or audio node carries, shown under the keypoints.
    payload: Option<Payload>,
}

/// What kind of file a node carries.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PayloadKind {
    Image,
    Audio,
}

/// A node's file: an image is shown as a thumbnail, an audio file by name.
#[derive(Clone)]
struct Payload {
    kind: PayloadKind,
    /// The path the node's config holds (empty when none is set yet).
    path: String,
}

/// The file `node` carries, for the kinds that carry one: a still or an image
/// node (`file`, else `path`) and an audio-file node (`file`).
fn payload_of(kind: &str, node: &serde_json::Value) -> Option<Payload> {
    let kind = match kind {
        "still" | "image.image" => PayloadKind::Image,
        "audio.file" => PayloadKind::Audio,
        _ => return None,
    };
    let config = &node["config"];
    let path = config["file"]
        .as_str()
        .or_else(|| config["path"].as_str())
        .unwrap_or_default()
        .to_string();
    Some(Payload { kind, path })
}

/// Almost nothing survives here between frames — every value this panel shows
/// is re-read fresh (`GET /graph`, `GET /catalog`, `GET /graph/effects`) on
/// every frame it paints, the same trade `panels::lanes`/`panels::library`
/// make. The exceptions are the drafts an operator is mid-typing and the two
/// caches the browser adds: `/graph/io` (briefly, invalidated after this
/// panel's own writes) and `/graph/live` (polled at ~4–5 Hz, never a GET per
/// frame).
pub struct PanelState {
    // --- output creation (moved from lanes, 15-sept-2026) -----------------
    output_new_id: String,
    output_note: Option<String>,

    // --- a focused overlay sequence (P5) ----------------------------------
    overlay_for: Option<String>,
    overlay_trigger_id: String,
    overlay_note: Option<String>,

    // --- masks (S.F/O.1/O.29) ---------------------------------------------
    mask_rows: i64,
    mask_cols: i64,
    mask_note: Option<String>,
    pub(crate) mask_edit_note: Option<String>,
    mask_tab: Option<String>,
    mask_canvas_rects: HashMap<String, egui::Rect>,
    /// The free-polygon draft, and which mask it belongs to (`S.F`/`O.1`):
    /// a mask with no points opens straight into draw mode, so a new mask
    /// is never invisible.
    mask_draft: Vec<[f32; 2]>,
    mask_draft_for: Option<String>,
    /// The mask whose `+ new` put the editor into draw mode (cleared when
    /// the polygon closes or the operator cancels).
    mask_draw: Option<String>,
    /// The mask currently sent to the projector (`POST /mask/project`),
    /// mirrored so the toggle can clear it with a no-`mask_id` post.
    mask_project: Option<String>,
    /// The device preview pumped under the editor (`shell::frames::Frame`).
    mask_frame: crate::shell::frames::Frame,
    /// The thumbnail of the image the selected node carries (header card).
    payload_frame: crate::shell::frames::Frame,
    /// `calibrar pared`: the wall being dragged, and its four corners in the
    /// output's `0..1` while they are dragged, keyed `"{wall}:{output}"`. The
    /// shared contour editor owns the gesture; this only remembers it.
    pub(crate) wall_calibrate: Option<String>,
    pub(crate) wall_fit: HashMap<String, Vec<[f32; 2]>>,
    /// The sector editor's canvas, per `(venue, surface)` (`sectors.rs`).
    pub(crate) sector_rects: HashMap<(String, String), egui::Rect>,

    // --- the browser -------------------------------------------------------
    /// Which collapsible sections are open, keyed by their English label
    /// (2026-10-08): the first two sections of a kind default open, the rest
    /// shut, and the operator's toggle is remembered per label.
    /// Folder fold state, keyed by `(kind, node, folder)`: two nodes of the
    /// same kind must not share a fold, and a generator's remembered layout
    /// must never decide what a screen's page shows.
    section_states: HashMap<(String, String, String), bool>,
    /// The `detail` toggle: when on, the node card paints the raw I/O tables
    /// (`io::input_table`/`output_table`) for debugging.
    io_detail: bool,
    /// The `customise sources` toggle: when on, Parameters reveals the
    /// binding editor (`io::source_section`) inline. Off by default.
    source_advanced: bool,
    /// The node card's label as painted — a read-only mirror today (there is
    /// no node-label route), kept so the card has one place it reads.
    card_label_draft: Option<String>,
    /// The selected node's header descriptor (task 4a), refreshed by
    /// [`PanelState::node_browser`] and read — one frame later — by the
    /// header block painted under the panel head. `None` for no node
    /// selection or an uncatalogued kind.
    header_info: Option<HeaderInfo>,
    /// The search text of the table folders.
    io_search: String,
    /// The last note or refusal of any write in the panel — shown in the
    /// status line until the next one.
    browser_note: Option<String>,
    /// A node a clicked table row asked to select — drained by `Shell::ui`
    /// after the panel painted (the smallest selection plumbing that keeps
    /// `regions.rs` untouched).
    pending_selection: Option<String>,
    /// A node another panel should bring into view on the canvas — set by the
    /// efectos pick (`chain`), drained by the shell into the nodes panel.
    pending_reveal: Option<String>,
    /// `GET /graph/io` replies, filed by `gid\u{1}node`, timestamped so a stale
    /// entry is re-read (and cleared on this panel's own writes).
    io_cache: HashMap<String, (serde_json::Value, f64)>,
    /// 2026-10-09: the last `GET /graph` — `(gid, read at, write
    /// generation, reply)`. It was re-read every frame (a whole graph
    /// serialised under the shared lock, sixty times a second); now at most
    /// every [`GRAPH_TTL`] or right after this panel writes.
    graph_cache: Option<(String, f64, u64, serde_json::Value)>,
    /// The last `GET /graph/live` reply for its graph, timestamped and polled.
    live_cache: Option<(serde_json::Value, f64, String)>,
    /// The three hook modules' own state (colour/venue/shader) — other agents
    /// fill those files right now; the panel calls them, never edits them.
    runtime: source_runtime::State,
    colour: crate::shell::panels::props_colour::State,
    venue: crate::shell::panels::props_venue::State,
    shader: crate::shell::panels::props_shader::State,
    /// The script code editor's loaded source and last runtime error (`§9.4 V3`).
    script: script::State,
    /// The loop page's draft under edit (the loops design notes T9).
    loop_edit: loops::Edit,
    /// H5: the `sensor.yolo` page's live preview pump.
    sensor: sensor::State,
    /// The room popup was asked for from the Map card (`venue_map`): the shell
    /// opens `panels::venue_popup` for the selected node's room next frame.
    pub(crate) venue_popup_open: bool,
    /// The `output.device` page's "abrir la escena" row asked for the room
    /// this device's `venue` output feeds (2026-10-22): `(graph, node)`,
    /// drained by the shell into the **canvas's own** `venue_requested`
    /// channel (`nodes::PanelState`) — the same one the `paint the room`
    /// gear sets — so one room popup serves both doors and no panel keeps a
    /// second "which room is open" state (rule 4). S3's `venue_scene` reads
    /// that channel.
    pub(crate) venue_request: Option<(String, String)>,
    /// The venue editor (M4/P4.4): the palette/surface choice, the selected
    /// fixture and the two view rects, shared with the fixture/patch folders
    /// so the pages are one workflow.
    pub(crate) venue_editor: crate::shell::panels::venue_editor::State,
}

impl Default for PanelState {
    fn default() -> Self {
        PanelState {
            output_new_id: String::new(),
            output_note: None,
            overlay_for: None,
            overlay_trigger_id: String::new(),
            overlay_note: None,
            mask_rows: 1,
            mask_cols: 1,
            mask_note: None,
            mask_edit_note: None,
            mask_tab: None,
            mask_canvas_rects: HashMap::new(),
            mask_draft: Vec::new(),
            mask_draft_for: None,
            mask_draw: None,
            mask_project: None,
            mask_frame: crate::shell::frames::Frame::default(),
            payload_frame: crate::shell::frames::Frame::default(),
            wall_calibrate: None,
            wall_fit: HashMap::new(),
            sector_rects: HashMap::new(),
            section_states: HashMap::new(),
            io_detail: false,
            source_advanced: false,
            card_label_draft: None,
            header_info: None,
            io_search: String::new(),
            browser_note: None,
            pending_selection: None,
            pending_reveal: None,
            io_cache: HashMap::new(),
            graph_cache: None,
            live_cache: None,
            runtime: Default::default(),
            colour: Default::default(),
            venue: Default::default(),
            shader: Default::default(),
            script: Default::default(),
            loop_edit: Default::default(),
            sensor: Default::default(),
            venue_popup_open: false,
            venue_request: None,
            venue_editor: Default::default(),
        }
    }
}

impl PanelState {
    #[cfg(test)]
    pub(crate) fn set_overlay_trigger_id(&mut self, id: &str) {
        self.overlay_trigger_id = id.to_string();
    }

    #[cfg(test)]
    pub(crate) fn overlay_for(&self) -> Option<&str> {
        self.overlay_for.as_deref()
    }

    /// A failed graph write, shown in the status line until the next note.
    pub(crate) fn set_write_note(&mut self, note: String) {
        self.browser_note = Some(note);
    }

    /// Drains the selection a clicked table row asked for — `Shell::ui` calls
    /// this once per frame after the panel painted.
    pub(crate) fn take_pending_selection(&mut self) -> Option<String> {
        self.pending_selection.take()
    }

    pub(crate) fn take_pending_reveal(&mut self) -> Option<String> {
        self.pending_reveal.take()
    }

    fn set_mask_canvas_rect(&mut self, mask_id: &str, rect: egui::Rect) {
        self.mask_canvas_rects.insert(mask_id.to_string(), rect);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn mask_canvas_rect(&self, mask_id: &str) -> Option<egui::Rect> {
        self.mask_canvas_rects.get(mask_id).copied()
    }

    /// The sector editor's canvas rect, filed by `sectors::section` under
    /// `(venue, surface)` — the same trade [`Self::mask_canvas_rect`] makes,
    /// so the kittest that clicks the grid has a point inside the canvas.
    pub(crate) fn sector_canvas_rect(&self, venue: &str, surface: &str) -> Option<egui::Rect> {
        self.sector_rects.get(&(venue.to_string(), surface.to_string())).copied()
    }

    /// The venue editor's canvas rect as of the last frame — P4.4's kittest
    /// needs a point inside that canvas, the same trade `mask_canvas_rect`
    /// makes.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn venue_canvas_rect(&self) -> Option<egui::Rect> {
        self.venue_editor.canvas_rect()
    }

    /// Reads and clears the Map card's "paint the room" press — the shell's
    /// one call per frame, which opens the room popup for the selected node.
    pub(crate) fn take_venue_popup_request(&mut self) -> bool {
        std::mem::take(&mut self.venue_popup_open)
    }

    /// Reads and clears the `output.device` page's "abrir la escena" press —
    /// `(graph, node)` for the room the canvas should open. See
    /// [`PanelState::venue_request`].
    pub(crate) fn take_venue_request(&mut self) -> Option<(String, String)> {
        self.venue_request.take()
    }

    /// The panel's whole content, with no outer frame — [`paint`] wraps this in
    /// the region's own frame, and the narrow-viewport overlay reuses it as-is.
    #[allow(clippy::too_many_arguments)]
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        door: &dyn Call,
        ingest: &mut crate::shell::lanes::IngestLane,
        selection: &Selection,
        picked: &[String],
        graph_id: Option<&str>,
        overlay_focus: Option<&str>,
        lanes_focused: bool,
        full_rect: egui::Rect,
    ) -> Response {
        let narrow = ui.available_width() < theme.properties_narrow;
        let bg = ui.interact(full_rect, ui.id().with("click-bg"), egui::Sense::click());
        // The header block for the selected node (task 4a). Resolved *this*
        // frame, before the header paints, so the block appears the same frame
        // the selection changes — a one-frame-late block would shift every row
        // under it and make a pointer click queued against the previous frame's
        // rect miss (`kittest`). Not while the lanes/overlay take the panel —
        // the node browser is not its body then.
        if !lanes_focused {
            if let (Selection::Node(id), Some(gid)) = (selection, graph_id) {
                self.refresh_header(door, gid, id);
            }
        }
        let header = self
            .header_info
            .as_ref()
            .filter(|_| !lanes_focused)
            .filter(|info| matches!(selection, Selection::Node(id) if id == &info.node))
            .cloned();
        // The selected node's own card *is* the panel head — there is no
        // separate `properties` strip above it. Only with no node descriptor
        // does the bare head paint, so the region keeps its AccessKit name.
        if let Some(info) = &header {
            header_block(ui, theme, door, &mut self.payload_frame, info);
        } else {
            panel_head(ui, theme, super::Region::Properties.title(), |_| {});
        }
        ScrollArea::vertical()
            .id_salt("shell/region/properties/scroll")
            .show(ui, |ui| {
                if lanes_focused {
                    if let Some(name) = overlay_focus {
                        overlay::section(ui, theme, door, self, name);
                        return;
                    }
                    // Authoring on the timeline: the active track's layer
                    // card and its Layer (blend/opacity/mute/solo), then its
                    // floats at the playhead (2026-10-08, plan §1/§6).
                    self.layer_card(ui, theme, ingest);
                    if !ingest.model.is_empty() {
                        ingest::section(ui, theme, door, ingest);
                        return;
                    }
                }
                // A Ctrl+click multi-selection: every picked node, listed.
                if picked.len() >= 2 {
                    self.multi_list(ui, theme, door, graph_id, picked);
                    return;
                }
                let mut show_ingest = false;
                match selection.clone() {
                    Selection::None => {
                        show_ingest = true;
                    }
                    Selection::Edge(id) => self.edge_state(ui, theme, id),
                    Selection::File(file) => file_page(ui, theme, door, &file),
                    Selection::Node(id) => match graph_id {
                        Some(gid) => {
                            self.node_browser(ui, theme, door, gid, &id, narrow, ingest);
                        }
                        None => {
                            ui.label(
                                RichText::new(tr("— no graph open —"))
                                    .font(theme.font_small.clone())
                                    .color(theme.ink_weak),
                            );
                        }
                    },
                };
                // No node selected: drop the cached header so it cannot
                // linger into a later node's selection (task 4a).
                if !matches!(selection, Selection::Node(_)) {
                    self.header_info = None;
                }
                if show_ingest {
                    ingest::section(ui, theme, door, ingest);
                }
            });
        bg
    }

    /// The Ctrl+click multi-selection (`picked`, two or more): a count and one
    /// row per node — its label (or id) and its kind. Read-only; the nodes
    /// panel owns the selection.
    fn multi_list(&self, ui: &mut Ui, theme: &Theme, door: &dyn Call, graph_id: Option<&str>, picked: &[String]) {
        ui.label(
            RichText::new(trf("{n} nodes selected", &[("n", &picked.len().to_string())]))
                .font(theme.font_body.clone())
                .color(theme.ink),
        );
        let graph = graph_id.and_then(|gid| shared::get(door, "GET", "/graph", &[("id", gid.to_string())]));
        for id in picked {
            let node = graph
                .as_ref()
                .and_then(|g| g["graph"]["nodes"].as_array())
                .and_then(|ns| ns.iter().find(|n| n["id"] == json!(id)));
            let label = node
                .and_then(|n| n["label"].as_str())
                .filter(|l| !l.is_empty())
                .unwrap_or(id.as_str())
                .to_string();
            let kind = node.and_then(|n| n["kind"].as_str()).unwrap_or_default().to_string();
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(label).font(theme.font_body.clone()).color(theme.ink));
                ui.label(small_text(theme, kind).color(theme.ink_faint));
            });
        }
    }

    /// Resolves the selected node's header descriptor (task 4a) the same
    /// frame the selection is painted. Reads the graph through the shared
    /// cache (`shared::get`, the same read `node_browser` makes, so this is a
    /// cache hit, not a second fetch), finds the node's kind and caches the
    /// descriptor keyed by node id.
    fn refresh_header(&mut self, door: &dyn Call, gid: &str, node_id: &str) {
        if self.header_info.as_ref().is_some_and(|info| info.node == node_id) {
            return;
        }
        let node = shared::get(door, "GET", "/graph", &[("id", gid.to_string())]).and_then(|graph| {
            graph["graph"]["nodes"]
                .as_array()
                .and_then(|ns| ns.iter().find(|n| n["id"] == json!(node_id)))
                .cloned()
        });
        let Some(node) = node else {
            self.header_info = None;
            return;
        };
        let kind = node["kind"].as_str().unwrap_or_default();
        let label = node["label"].as_str().unwrap_or_default();
        let model = catalog_label(door, kind);
        self.header_info = describe_kind(kind, label, model.as_deref()).map(|(title, description, keypoints)| HeaderInfo {
            node: node_id.to_string(),
            kind: kind.to_string(),
            title,
            description,
            keypoints,
            payload: payload_of(kind, &node),
        });
    }

    fn edge_state(&self, ui: &mut Ui, theme: &Theme, id: String) {
        section_heading(ui, theme, tr("cable"));
        crate::shell::components::note(ui, theme, &trf("cable: {id} — no editable properties", &[("id", &id)]));
    }

    /// The active track's card while the sequence editor (lanes) is focused
    /// (2026-10-08, plan §1/§6): its identity — `layer N · model`, its page and
    /// its clip count — always visible, then a **Layer** section (open by
    /// default) with the active `(page, layer)`'s blend/opacity/mute/solo,
    /// read and written through [`IngestLane::layer_meta`]/
    /// [`IngestLane::set_layer_meta`]. The floats follow (`ingest::section`).
    fn layer_card(&mut self, ui: &mut Ui, theme: &Theme, ingest: &mut crate::shell::lanes::IngestLane) {
        let layer = ingest.layer;
        let page = ingest.current_page();
        let model = if ingest.model.is_empty() {
            tr("— no model —").to_string()
        } else {
            ingest.model.clone()
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(trf("layer {n} · {model}", &[("n", &(layer + 1)), ("model", &model)]))
                    .font(theme.font_body.clone())
                    .color(theme.ink),
            );
            ui.label(
                RichText::new(trf("clip {i}/{n}", &[("i", &(ingest.active + 1)), ("n", &ingest.track_count())]))
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
            );
        });
        field_row(ui, theme, tr("page"), &page.label());

        let mut open = *self
            .section_states
            .entry((String::new(), String::new(), LAYER_CARD_SECTION.to_string()))
            .or_insert(true);
        kit_section(ui, theme, tr("layer"), &mut open);
        self.section_states
            .insert((String::new(), String::new(), LAYER_CARD_SECTION.to_string()), open);
        if !open {
            return;
        }
        let mut meta = ingest.layer_meta(&page, layer);
        let mut changed = false;
        label_row(ui, theme, tr("blend"), |ui| {
            let response = ui.button(small_text(theme, meta.blend.clone()));
            if response.clicked() {
                meta.cycle_blend();
                changed = true;
            }
            response
        });
        if num(ui, theme, tr("opacity"), &mut meta.opacity, 0.0..=1.0, "", 2, 1.0, true).changed() {
            changed = true;
        }
        if toggle_row(ui, theme, tr("muted"), &mut meta.muted).changed() {
            changed = true;
        }
        if toggle_row(ui, theme, tr("solo"), &mut meta.solo).changed() {
            changed = true;
        }
        if changed {
            ingest.set_layer_meta(&page, layer, meta);
        }
    }

    /// The selected node's card plus its collapsible sections (2026-10-08):
    /// the card (label, kind, mode, I/O summary) always visible, then the
    /// kind's sections — own editor, Parameters, loop, hook folders — each a
    /// kit [`kit_section`] the operator opens. Returns the node's kind.
    fn node_browser(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        door: &dyn Call,
        gid: &str,
        node_id: &str,
        narrow: bool,
        ingest: &mut crate::shell::lanes::IngestLane,
    ) -> Option<String> {
        let now = ui.input(|i| i.time);
        let generation = shared::writes();
        let fresh = self.graph_cache.as_ref().filter(|(g, at, wrote, _)| {
            g == gid && *wrote == generation && now - at < GRAPH_TTL && now >= *at
        });
        let graph = match fresh {
            Some((_, _, _, v)) => Some(v.clone()),
            None => {
                let v = shared::get(door, "GET", "/graph", &[("id", gid.to_string())]);
                if let Some(v) = &v {
                    self.graph_cache = Some((gid.to_string(), now, generation, v.clone()));
                }
                v
            }
        };
        let Some(graph) = graph else {
            ui.label(
                RichText::new(tr("— could not read the graph —"))
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
            );
            return None;
        };
        let Some(node) = graph["graph"]["nodes"]
            .as_array()
            .and_then(|ns| ns.iter().find(|n| n["id"] == json!(node_id)))
            .cloned()
        else {
            ui.label(
                RichText::new(tr("— node not found —"))
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
            );
            return None;
        };
        let kind = node["kind"].as_str().unwrap_or_default().to_string();
        // Refresh the header cache for this node (task 4a): the header paints
        // it at the top of the panel next frame, keyed by the node id.
        let label = node["label"].as_str().unwrap_or_default();
        let model = catalog_label(door, &kind);
        self.header_info = describe_kind(&kind, label, model.as_deref()).map(|(title, description, keypoints)| HeaderInfo {
            node: node_id.to_string(),
            kind: kind.to_string(),
            title,
            description,
            keypoints,
            payload: payload_of(&kind, &node),
        });

        let io = io::fetch_io(self, door, now, gid, node_id);
        let params = io.as_ref().map(io::counts).unwrap_or((0, 0, 0, 0)).3;

        let _ = narrow;
        let mut clicked = self.node_card(ui, theme, door, gid, node_id, &node, &kind, io.as_ref());

        let authored = kind
            .strip_prefix("gen.")
            .is_some_and(|m| !m.is_empty() && m == ingest.model.as_str());
        let mut keys = section_keys(&kind, params > 0);
        // Superficies (plan "Surface sources", step 2): a generator whose
        // catalog entry has material `tex.*` inputs, after its effects chain.
        if kind.starts_with("gen.") && !surfaces::parts(door, &kind).is_empty() {
            let at = keys
                .iter()
                .position(|k| *k == crate::shell::i18n::tr_key("effects chain"))
                .map_or(keys.len(), |i| i + 1);
            keys.insert(at, crate::shell::i18n::tr_key("surfaces"));
        }
        if source_runtime::shows(&kind, &graph, node_id) {
            keys.insert(0, crate::shell::i18n::tr_key("runtime"));
        }
        // A room node's folders are all folded but the mapping: the Map card
        // is one screen of rows (the room, the popup button, the output each
        // screen lands on), and an open `venue editor` under it would push the
        // calibration and the views out of the panel. The operator opens what
        // they came for; the map is what the node is for.
        // A node wired to a screen keeps the output device, the venue and the
        // draw folders open: those three *are* the screen's page (which output
        // it feeds, the room it lands in, its glass). The room node's own
        // folders stay folded but the map.
        let room = matches!(
            kind.as_str(),
            metal_graph::VENUE_FIXTURES_KIND | metal_graph::VENUE_SURFACE_KIND
        );
        for (i, key) in keys.iter().enumerate() {
            // A grade's own page is long; its generic Parameters list (the same
            // floats, unstructured) starts shut so the page reads as one surface.
            // The generator's chain opens with its layer; its parameters stay
            // open as they always were, pushed down by the chain's own row.
            let default_open = if kind == "fx.grade" {
                i < 1
            } else if room {
                *key == crate::shell::i18n::tr_key("map")
                    // The screen's grid opens too when the node names no
                    // surface yet: an empty page would hide the one step
                    // that gives it its split.
                    || (kind == metal_graph::VENUE_SURFACE_KIND
                        && *key == crate::shell::i18n::tr_key("grid")
                        && node["config"]["surface"].as_str().is_none_or(str::is_empty))
            } else if kind == "output.device" {
                matches!(
                    *key,
                    k if k == crate::shell::i18n::tr_key("output device")
                        || k == crate::shell::i18n::tr_key("map")
                        || k == crate::shell::i18n::tr_key("venue")
                        || k == crate::shell::i18n::tr_key("draw")
                )
            } else {
                i < 2
                    || *key == crate::shell::i18n::tr_key("effects chain")
                    || *key == crate::shell::i18n::tr_key("surfaces")
                    || *key == crate::shell::i18n::tr_key("parameters")
            };
            // The fold state is remembered per folder name **per node**: a
            // generator's remembered `map`/`venue`/`draw` must not decide what
            // a screen's page shows, and two screens must not share one fold.
            let fold_key = (kind.to_string(), node_id.to_string(), (*key).to_string());
            let mut open = *self.section_states.entry(fold_key).or_insert(default_open);
            let body = crate::shell::components::card_section(ui, theme, tr(*key), &mut open, |ui| {
                let _span = crate::perf::Span::new(*key);
                self.paint_section(ui, theme, door, gid, node_id, &node, &graph, io.as_ref(), ingest, authored, key)
            });
            self.section_states.insert(
                (kind.to_string(), node_id.to_string(), (*key).to_string()),
                open,
            );
            if let Some(Some(row)) = body {
                clicked = Some(row);
            }
        }
        if let Some(browser::RowKey::Text(node)) = clicked {
            self.browser_note = Some(trf("selected {node}", &[("node", &node)]));
            self.pending_selection = Some(node);
        }
        browser::status_line(ui, theme, self.browser_note.as_deref());
        Some(kind)
    }

    /// The node card (2026-10-08): the always-visible identity line, the
    /// `gen.*` mode segment (the same `config.mode` merge `sources.rs` posts),
    /// the one-line I/O summary, and the `detail` toggle that reveals the raw
    /// input/output tables for debugging.
    #[allow(clippy::too_many_arguments)]
    fn node_card(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        door: &dyn Call,
        gid: &str,
        node_id: &str,
        node: &serde_json::Value,
        kind: &str,
        io: Option<&serde_json::Value>,
    ) -> Option<browser::RowKey> {
        let label = node["label"].as_str().unwrap_or_default().to_string();
        self.card_label_draft = Some(if label.is_empty() { node_id.to_string() } else { label });
        let shown = self.card_label_draft.clone().unwrap_or_else(|| node_id.to_string());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(shown).font(theme.font_body.clone()).color(theme.ink));
            ui.label(small_text(theme, kind.to_string()).color(theme.ink_faint));
        });

        if kind.starts_with("gen.") {
            let mut mode = match node["config"]["mode"].as_str() {
                Some("ai") => 0usize,
                Some("manual") => 2,
                _ => 1,
            };
            let options = [(0usize, tr("AI")), (1, tr("AI FX")), (2, tr("manual"))];
            if label_row(ui, theme, tr("mode"), |ui| segmented(ui, theme, &mut mode, &options)).changed() {
                let value = match mode {
                    0 => "ai",
                    2 => "manual",
                    _ => "ai_fx",
                };
                text::post_text(door, gid, node_id, json!({ "mode": value }));
            }
        }

        if let Some(io) = io {
            let (ins, outs) = io::io_summary(io);
            ui.label(
                RichText::new(format!("{}: {ins}", tr("inputs")))
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
            )
            .on_hover_text(ins);
            ui.label(
                RichText::new(format!("{}: {outs}", tr("outputs")))
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
            )
            .on_hover_text(outs);
        }

        toggle_row(ui, theme, tr("detail"), &mut self.io_detail);
        let mut clicked = None;
        if self.io_detail {
            if let Some(io) = io {
                let count = io::counts(io).0 + io::counts(io).1;
                browser::search_bar(ui, theme, &mut self.io_search, count);
                clicked = io::input_table(ui, theme, self, io);
                if let Some(row) = io::output_table(ui, theme, self, io) {
                    clicked = Some(row);
                }
            }
        }
        clicked
    }

    /// Paints one section's content. `parameters` routes to [`io`]; the kind's
    /// own sections to their files; the hooks to their modules.
    #[allow(clippy::too_many_arguments)]
    fn paint_section(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        door: &dyn Call,
        gid: &str,
        node_id: &str,
        node: &serde_json::Value,
        graph: &serde_json::Value,
        io: Option<&serde_json::Value>,
        ingest: &mut crate::shell::lanes::IngestLane,
        authored: bool,
        open: &str,
    ) -> Option<browser::RowKey> {
        let mut clicked = None;
        match open {
            "input" => {
                if let Some(io) = io {
                    clicked = io::input_table(ui, theme, self, io);
                }
            }
            "output" => {
                if let Some(io) = io {
                    clicked = io::output_table(ui, theme, self, io);
                }
            }
            "source" => {
                if let Some(io) = io {
                    clicked = io::source_section(ui, theme, door, self, gid, node_id, io);
                }
            }
            "parameters" => {
                let empty = json!({});
                let io = io.unwrap_or(&empty);
                clicked = io::parameters_section(
                    ui, theme, door, self, gid, node_id, node, graph, io, ingest, authored,
                );
            }
            "layer" => {
                layer::section(ui, theme, door, gid, node_id, node);
                // "Renderiza en" rides inside the source's own layer section.
                render_on::source_section(ui, theme, door, node);
            }
            "surfaces" => surfaces::section(
                ui, theme, door, gid, node_id, node, graph, &mut self.pending_reveal,
            ),
            "effects chain" => {
                clicked = chain::section(
                    ui, theme, door, gid, node_id, graph, &mut self.pending_reveal,
                );
            }
            "mode" => mode::section(ui, theme, door, gid, node_id, node),
            "output device" => output_device::section(ui, theme, door, gid, node_id, graph, self),
            "mask" => mask::section(ui, theme, door, gid, node_id, node, graph, self),
            "material" => material::section(ui, theme, door, gid, node_id, node, graph),
            "surface" => surface::section(ui, theme, door, gid, node_id, node, graph, self),
            "grid" => {
                let venue =
                    crate::shell::panels::venue_map::venue_for(door, gid, node_id, node, graph);
                let named = node["config"]["surface"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                let surface = crate::shell::panels::properties::surface::get_venue(door, &venue.id)
                    .and_then(|detail| {
                        let rows = detail["venue"]["surfaces"].as_array()?.clone();
                        named
                            .as_deref()
                            .and_then(|n| rows.iter().find(|s| s["id"] == json!(n)).cloned())
                            .or_else(|| rows.first().cloned())
                    })
                    .unwrap_or_else(|| json!({ "id": named.unwrap_or_default() }));
                sectors::section(ui, theme, door, gid, node_id, node, &venue.id, &surface, self);
            }
            "venue" => crate::shell::panels::props_venue::device_section(
                ui, theme, door, gid, node_id, node, graph, self,
            ),
            "draw" => crate::shell::panels::venue_editor::draw_section(
                ui, theme, door, gid, node_id, node, graph, self,
            ),
            "text" => text::section(ui, theme, door, gid, node_id, node),
            transition::IN_SECTION => transition::section(ui, theme, door, gid, node_id, node, true),
            transition::OUT_SECTION => transition::section(ui, theme, door, gid, node_id, node, false),
            "image settings" => image::section(ui, theme, door, gid, node_id, node),
            source_math::SECTION => source_math::section(ui, theme, door, gid, node_id, node),
            "text effect" => text::effect_section(ui, theme, door, gid, node_id, node),            "script" => script::section(ui, theme, door, gid, node_id, node, graph, &mut self.script),
            "sACN (E1.31)" => sacn::section(ui, theme, door, gid, node_id, node),
            "Art-Net" => artnet::section(ui, theme, door, gid, node_id, node),
            "Art-Net in" => artnet_in::section(ui, theme, door, gid, node_id, node),
            "timecode" => timecode::section(ui, theme, door, gid, node_id, node),
            "sensor" => sensor::section(ui, theme, door, gid, node_id, node, graph, &mut self.sensor),
            "camera track" => cam_track::section(ui, theme, door, gid, node_id, node, graph),
            "input device" => listen::audio_live_section(ui, theme, door, gid, node_id, node),
            "reaction" => sources::reaction_section(ui, theme, door, gid, node_id, node, graph),
            "capture source" => sources::capture_section(ui, theme, door, gid, node_id, node),
            "runtime" => source_runtime::section(
                ui, theme, door, gid, node_id, node, graph, &mut self.runtime,
            ),
            "reel list" => sources::reel_batch_section(ui, theme, door, gid, node_id, node),
            "colourist" => sources::colourist_section(ui, theme, door, gid, node_id, node),
            "loop" => loops::section(ui, theme, door, gid, node_id, &mut self.loop_edit),
            "map" => {
                crate::shell::panels::venue_map::map_card(
                    ui,
                    theme,
                    door,
                    gid,
                    node_id,
                    node,
                    graph,
                    &mut self.venue_popup_open,
                );
                // Which of this graph's nodes drives each row of the room:
                // the mapping between the scene the popup adds to and the
                // nodes the operator already wired.
                ui.add_space(theme.item_gap.y);
                crate::shell::panels::venue_map::mapping_card(ui, theme, door, gid, graph);
            }
            "venue editor" => {
                let vid = node["config"]["venue"].as_str().unwrap_or_default().to_string();
                crate::shell::panels::venue_editor::section(ui, theme, door, &vid, gid, graph, &mut self.venue_editor);
            }
            "fixture" => {
                // A `venue.fixture` node picks its own row (and any others it
                // drives) here; the `venue.fixtures` group node's page keeps
                // the browser's selected row.
                if node["kind"].as_str() == Some(metal_graph::VENUE_FIXTURE_KIND) {
                    fixture::node_section(
                        ui,
                        theme,
                        door,
                        gid,
                        node_id,
                        node,
                        graph,
                        &mut self.venue_editor,
                    );
                } else {
                    let vid = crate::shell::panels::venue_map::venue_for(door, gid, node_id, node, graph)
                        .id;
                    fixture::section(ui, theme, door, &vid, &mut self.venue_editor);
                }
            }
            "patch" => {
                let vid = node["config"]["venue"].as_str().unwrap_or_default().to_string();
                patch::section(ui, theme, door, &vid, &mut self.venue_editor);
            }
            "grade" => {
                let kind = node["kind"].as_str().unwrap_or_default();
                let _ = grade::section(ui, theme, door, kind, gid, node_id, graph, io);
            }
            "effect" => {
                let kind = node["kind"].as_str().unwrap_or_default();
                // The advanced effects (`fx.grade`, `fx.texture`, …) hold each
                // parameter under its own config key, not a single `fixed`
                // amount, so they get their own grouped slider page; everything
                // else keeps the state/value section.
                if !grade::advanced_section(ui, theme, door, kind, gid, node_id, graph, io) {
                    effects::section(ui, theme, door, kind, gid, node_id, graph);
                }
            }
            other => {
                // The three hook modules, called in turn — the first that
                // reports it painted the section wins.
                let _ = crate::shell::panels::props_colour::section(
                    ui, theme, door, gid, node_id, node, graph, other, &mut self.colour,
                ) || crate::shell::panels::props_venue::section(
                    ui, theme, door, gid, node_id, node, graph, other, &mut self.venue,
                ) || crate::shell::panels::props_shader::section(
                    ui, theme, door, gid, node_id, node, graph, other, &mut self.shader,
                );
            }
        }
        clicked
    }
}

/// The selected node's header block, painted under the panel head (task 4a):
/// the title emphasised, the one-sentence description, then the kind's three
/// short keypoints, each on its own `· `-prefixed line. Every size and colour
/// is a `Theme` token.
fn header_block(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    frame: &mut crate::shell::frames::Frame,
    info: &HeaderInfo,
) {
    ui.add_space(theme.section_gap);
    let card = theme.card_frame().show(ui, |ui| {
        ui.spacing_mut().item_spacing = theme.item_gap;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(info.title.as_str())
                    .font(theme.font_body.clone())
                    .color(theme.accent_vivid)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(info.node.as_str())
                        .font(theme.font_small.clone())
                        .color(theme.ink_weak),
                );
                // A file node's own star: the file itself goes to the global
                // favourites (its gallery entry imports the file again).
                if let Some(p) = info.payload.as_ref().filter(|p| !p.path.is_empty()) {
                    let file_kind = if p.kind == PayloadKind::Image { "image" } else { "audio" };
                    let name = std::path::Path::new(&p.path).file_name().and_then(|n| n.to_str()).unwrap_or(&p.path);
                    let fav = crate::shell::favorites::is_favourite(ui.ctx(), &crate::shell::favorites::file_key(file_kind, &p.path));
                    let star = if fav { "★" } else { "☆" };
                    if ui.add(egui::Button::new(RichText::new(star).font(theme.font_body.clone()))).on_hover_text(tr("favourite")).clicked() {
                        crate::shell::favorites::request_toggle(ui.ctx(), crate::shell::favorites::file_entry(file_kind, name, &p.path));
                    }
                }
                // The global favourite star (`shell/favorites.rs`): queued on
                // the context, the shell stores it at the frame's end.
                let fav = crate::shell::favorites::is_favourite(ui.ctx(), &info.kind);
                let star = if fav { "★" } else { "☆" };
                if ui.add(egui::Button::new(RichText::new(star).font(theme.font_body.clone()))).on_hover_text(tr("favourite")).clicked() {
                    crate::shell::favorites::request_toggle(
                        ui.ctx(),
                        crate::shell::favorites::Entry {
                            key: info.kind.clone(),
                            label: info.title.clone(),
                            family: info.kind.split('.').next().unwrap_or_default().to_string(),
                            inputs: Vec::new(),
                            outputs: Vec::new(),
                        },
                    );
                }
            });
        });
        ui.add_space(theme.item_gap.y);
        crate::shell::components::note(ui, theme, info.description.as_str());
        for keypoint in &info.keypoints {
            crate::shell::components::note(ui, theme, &format!("· {keypoint}"));
        }
        if let Some(payload) = &info.payload {
            payload_block(ui, theme, door, frame, payload);
        }
    });
    // The card carries the region's AccessKit name (the `properties` head is
    // gone when a node is selected), so a reader/test still reaches the panel.
    card.response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, super::Region::Properties.title())
    });
}

/// The file a still, image or audio node carries, under the card's keypoints:
/// an image as a thumbnail (the route the library previews it through), an
/// audio file as its icon; both with the file's name and folder. A node with
/// no file yet says so.
fn payload_block(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    frame: &mut crate::shell::frames::Frame,
    payload: &Payload,
) {
    ui.add_space(theme.section_gap);
    if payload.path.is_empty() {
        crate::shell::components::note(ui, theme, tr("— no file —"));
        return;
    }
    if payload.kind == PayloadKind::Image {
        let gap = std::time::Duration::from_secs(30);
        let query = [("path", payload.path.clone())];
        if let Some((tex, size)) = frame.picture(ui.ctx(), door, "/author/image.jpg", &query, gap) {
            let width = ui.available_width();
            let height = (width * size[1] as f32 / size[0].max(1) as f32).min(theme.payload_max_height);
            ui.add(
                egui::Image::from_texture(egui::load::SizedTexture::new(tex, [size[0] as f32, size[1] as f32]))
                    .fit_to_exact_size(egui::Vec2::new(width, height))
                    .corner_radius(theme.corner_radius()),
            );
        }
    }
    let path = std::path::Path::new(&payload.path);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or(&payload.path);
    let folder = path.parent().and_then(|p| p.to_str()).unwrap_or_default();
    ui.horizontal(|ui| {
        if payload.kind == PayloadKind::Audio {
            let (rect, _) = ui.allocate_exact_size(theme.library_badge_size, egui::Sense::hover());
            ui.painter().rect_filled(rect, theme.library_card_corner(), theme.library_badge);
            crate::shell::components::paint_icon(ui.painter(), rect, crate::shell::components::Icon::Audio, theme);
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = theme.library_detail_gap;
            ui.add(egui::Label::new(RichText::new(name).font(theme.font_body.clone()).color(theme.ink)).truncate())
                .on_hover_text(&payload.path);
            ui.add(egui::Label::new(small_text(theme, folder).color(theme.ink_weak)).truncate());
        });
    });
}

/// A node `kind`'s short title, one-sentence description and three keypoints
/// naming its important floats or node-relative facts (task 4a), translated
/// and ready to paint. The literals ride through `tr_key` so the i18n gates
/// see them, then through `tr`. `None` for an uncatalogued kind, so the
/// header block only shows for the kinds the catalogue covers.
fn describe_kind(kind: &str, _label: &str, model: Option<&str>) -> Option<(String, String, [String; 3])> {
    let (title, description, keypoints): (&'static str, &'static str, [&'static str; 3]) = if kind == "fx.grade" {
        (
            crate::shell::i18n::tr_key("Effect · Grade"),
            crate::shell::i18n::tr_key("Colour correction with exposure, contrast and curves."),
            [
                crate::shell::i18n::tr_key("exposure"),
                crate::shell::i18n::tr_key("contrast"),
                crate::shell::i18n::tr_key("saturation"),
            ],
        )
    } else if kind.starts_with("fx.text.") {
        (
            crate::shell::i18n::tr_key("text effect"),
            crate::shell::i18n::tr_key("Warps or styles the text drawn by a source."),
            [
                crate::shell::i18n::tr_key("intensity 0-1"),
                crate::shell::i18n::tr_key("speed"),
                crate::shell::i18n::tr_key("reactive"),
            ],
        )
    } else if kind.starts_with("fx.") {
        (
            crate::shell::i18n::tr_key("effect"),
            crate::shell::i18n::tr_key("An effect applied over a generator's image."),
            [
                crate::shell::i18n::tr_key("intensity 0-1"),
                crate::shell::i18n::tr_key("speed"),
                crate::shell::i18n::tr_key("reactive"),
            ],
        )
    } else if kind.starts_with("gen.") {
        (
            crate::shell::i18n::tr_key("generator"),
            crate::shell::i18n::tr_key("A generative model that draws the image in real time."),
            [
                crate::shell::i18n::tr_key("speed"),
                crate::shell::i18n::tr_key("seed"),
                crate::shell::i18n::tr_key("own parameters"),
            ],
        )
    } else if kind == "reel.clip" {
        (
            crate::shell::i18n::tr_key("Video clip"),
            crate::shell::i18n::tr_key("A video from the reel played on the output."),
            [
                crate::shell::i18n::tr_key("duration"),
                crate::shell::i18n::tr_key("inputs"),
                crate::shell::i18n::tr_key("outputs"),
            ],
        )
    } else if kind == "reel.batch" {
        (
            crate::shell::i18n::tr_key("reel list"),
            crate::shell::i18n::tr_key("Plays videos, stills and scenes in sequence."),
            [
                crate::shell::i18n::tr_key("order"),
                crate::shell::i18n::tr_key("interval"),
                crate::shell::i18n::tr_key("unit"),
            ],
        )
    } else if kind == "router" {
        (
            crate::shell::i18n::tr_key("Router"),
            crate::shell::i18n::tr_key("Routes one signal to several outputs."),
            [
                crate::shell::i18n::tr_key("inputs"),
                crate::shell::i18n::tr_key("outputs"),
                crate::shell::i18n::tr_key("source"),
            ],
        )
    } else if kind == "audio.live" {
        (
            crate::shell::i18n::tr_key("Live audio"),
            crate::shell::i18n::tr_key("Reads the audio input and exposes its features."),
            [
                crate::shell::i18n::tr_key("audio input"),
                crate::shell::i18n::tr_key("reactive"),
                crate::shell::i18n::tr_key("live"),
            ],
        )
    } else if kind == "audio.file" {
        (
            crate::shell::i18n::tr_key("Audio file"),
            crate::shell::i18n::tr_key("Plays an audio file as a source."),
            [
                crate::shell::i18n::tr_key("path"),
                crate::shell::i18n::tr_key("bpm"),
                crate::shell::i18n::tr_key("reactive"),
            ],
        )
    } else if kind == "output.device" {
        (
            crate::shell::i18n::tr_key("output"),
            crate::shell::i18n::tr_key("Sends the composed image to a screen or sink."),
            [
                crate::shell::i18n::tr_key("resolution"),
                crate::shell::i18n::tr_key("destination"),
                crate::shell::i18n::tr_key("mode"),
            ],
        )
    } else if kind == "out.sacn" {
        (
            crate::shell::i18n::tr_key("sACN (E1.31)"),
            crate::shell::i18n::tr_key("Sends the graph's values over sACN."),
            [
                crate::shell::i18n::tr_key("universe"),
                crate::shell::i18n::tr_key("unicast"),
                crate::shell::i18n::tr_key("channels"),
            ],
        )
    } else if kind.starts_with("out.") {
        (
            crate::shell::i18n::tr_key("Control output"),
            crate::shell::i18n::tr_key("Sends the graph's values over an external protocol."),
            [
                crate::shell::i18n::tr_key("protocol"),
                crate::shell::i18n::tr_key("destination"),
                crate::shell::i18n::tr_key("channels"),
            ],
        )
    } else if kind == "in.artnet" {
        (
            crate::shell::i18n::tr_key("Art-Net in"),
            crate::shell::i18n::tr_key("Receives control values from an Art-Net node."),
            [
                crate::shell::i18n::tr_key("universe"),
                crate::shell::i18n::tr_key("first channel"),
                crate::shell::i18n::tr_key("channels"),
            ],
        )
    } else if kind == TEXT_SOURCE_KIND {
        (
            crate::shell::i18n::tr_key("text"),
            crate::shell::i18n::tr_key("Draws text on the image with font, size and colour."),
            [
                crate::shell::i18n::tr_key("font"),
                crate::shell::i18n::tr_key("size"),
                crate::shell::i18n::tr_key("align"),
            ],
        )
    } else if kind == "mask" {
        (
            crate::shell::i18n::tr_key("mask"),
            crate::shell::i18n::tr_key("Cuts the image with shapes or a free polygon."),
            [
                crate::shell::i18n::tr_key("shape"),
                crate::shell::i18n::tr_key("stroke"),
                crate::shell::i18n::tr_key("alpha"),
            ],
        )
    } else if kind == "color" {
        (
            crate::shell::i18n::tr_key("colour"),
            crate::shell::i18n::tr_key("A fixed colour edited by hue, saturation and brightness."),
            [
                crate::shell::i18n::tr_key("colour"),
                crate::shell::i18n::tr_key("saturation"),
                crate::shell::i18n::tr_key("brightness"),
            ],
        )
    } else if kind == "palette" {
        (
            crate::shell::i18n::tr_key("Palette"),
            crate::shell::i18n::tr_key("Twelve colours that tint the graph's surfaces."),
            [
                crate::shell::i18n::tr_key("12 slots"),
                crate::shell::i18n::tr_key("colour per slot"),
                crate::shell::i18n::tr_key("background"),
            ],
        )
    } else if metal_graph::is_script_kind(kind) {
        (
            crate::shell::i18n::tr_key("script"),
            crate::shell::i18n::tr_key("Code that produces the node's outputs."),
            [
                crate::shell::i18n::tr_key("script language"),
                crate::shell::i18n::tr_key("declared floats"),
                crate::shell::i18n::tr_key("reactive"),
            ],
        )
    } else if kind == "ctl.math" {
        (
            crate::shell::i18n::tr_key("Control maths"),
            crate::shell::i18n::tr_key("Combines two control values with a chosen operation."),
            [
                crate::shell::i18n::tr_key("operation"),
                crate::shell::i18n::tr_key("inputs"),
                crate::shell::i18n::tr_key("outputs"),
            ],
        )
    } else if kind == "clock.timecode" {
        (
            crate::shell::i18n::tr_key("timecode"),
            crate::shell::i18n::tr_key("Follows or generates an LTC or MTC signal."),
            [
                crate::shell::i18n::tr_key("source"),
                crate::shell::i18n::tr_key("frame rate"),
                crate::shell::i18n::tr_key("offset"),
            ],
        )
    } else if kind == "sensor.yolo" {
        (
            crate::shell::i18n::tr_key("sensor"),
            crate::shell::i18n::tr_key("Detects a head in the image and publishes its position."),
            [
                crate::shell::i18n::tr_key("min cutoff"),
                crate::shell::i18n::tr_key("beta"),
                crate::shell::i18n::tr_key("track"),
            ],
        )
    } else if kind == "cam.track" {
        (
            crate::shell::i18n::tr_key("camera track"),
            crate::shell::i18n::tr_key("Moves the 3D camera towards the detected position."),
            [
                crate::shell::i18n::tr_key("strength"),
                crate::shell::i18n::tr_key("invert"),
                crate::shell::i18n::tr_key("all 3D generators"),
            ],
        )
    } else if kind == "image.image" {
        (
            crate::shell::i18n::tr_key("Image"),
            crate::shell::i18n::tr_key("Loads and shows an image file."),
            [
                crate::shell::i18n::tr_key("path"),
                crate::shell::i18n::tr_key("blend"),
                crate::shell::i18n::tr_key("swatch"),
            ],
        )
    } else if kind == "still" {
        (
            crate::shell::i18n::tr_key("Still image"),
            crate::shell::i18n::tr_key("An image held for the interval."),
            [
                crate::shell::i18n::tr_key("path"),
                crate::shell::i18n::tr_key("interval"),
                crate::shell::i18n::tr_key("transition in"),
            ],
        )
    } else if kind == metal_graph::AI_COLOURIST_KIND {
        (
            crate::shell::i18n::tr_key("colourist"),
            crate::shell::i18n::tr_key("A trained automatic grade read from the song."),
            [
                crate::shell::i18n::tr_key("trained"),
                crate::shell::i18n::tr_key("colour"),
                crate::shell::i18n::tr_key("source"),
            ],
        )
    } else if kind == STACK_MODE_KIND {
        (
            crate::shell::i18n::tr_key("mode"),
            crate::shell::i18n::tr_key("How a stack combines its layers."),
            [
                crate::shell::i18n::tr_key("mode"),
                crate::shell::i18n::tr_key("collection"),
                crate::shell::i18n::tr_key("blend"),
            ],
        )
    } else if kind == metal_graph::VENUE_SURFACE_KIND {
        (
            crate::shell::i18n::tr_key("surface"),
            crate::shell::i18n::tr_key("An Avenue-style mappable surface."),
            [
                crate::shell::i18n::tr_key("mapping"),
                crate::shell::i18n::tr_key("object"),
                crate::shell::i18n::tr_key("projector"),
            ],
        )
    } else if kind == metal_graph::VENUE_FIXTURES_KIND {
        (
            crate::shell::i18n::tr_key("venue"),
            crate::shell::i18n::tr_key("Fixtures, patch and mapping for the venue."),
            [
                crate::shell::i18n::tr_key("fixture"),
                crate::shell::i18n::tr_key("patch"),
                crate::shell::i18n::tr_key("venue editor"),
            ],
        )
    } else if metal_graph::materials::is_material_kind(kind) {
        (
            crate::shell::i18n::tr_key("material"),
            crate::shell::i18n::tr_key("What a 3D model's surface is made of."),
            [
                crate::shell::i18n::tr_key("part"),
                crate::shell::i18n::tr_key("rough"),
                crate::shell::i18n::tr_key("glitch"),
            ],
        )
    } else if describe::lookup(kind).is_some() {
        (crate::shell::i18n::tr_key("node"), "", ["", "", ""])
    } else {
        return None;
    };
    // A kind with text of its own (`describe.rs`) says what *it* does; the
    // family text above is the fallback for the rest.
    let (description, keypoints) = match describe::lookup(kind) {
        Some(entry) => (entry.description, entry.keypoints),
        None => (description, keypoints),
    };
    // The card names the *specific* model, not only its kind's class: the
    // catalogue's own label for the kind (`Turing`, `Anillos`, `Vignette`…)
    // stands in for the generic `generator`/`effect` when one exists.
    let title = match model {
        Some(m) if !m.is_empty() => crate::shell::i18n::tr_dyn(m),
        _ => tr(title).to_string(),
    };
    Some((
        title,
        tr(description).to_string(),
        [
            tr(keypoints[0]).to_string(),
            tr(keypoints[1]).to_string(),
            tr(keypoints[2]).to_string(),
        ],
    ))
}

/// The catalogue's own label for a `kind` (`GET /catalog`), the specific
/// model's name (`Turing`, `Anillos`, `Vignette`…) — `None` when the kind is
/// uncatalogued or the catalogue carries no label for it.
fn catalog_label(door: &dyn Call, kind: &str) -> Option<String> {
    let catalog = shared::get(door, "GET", "/catalog", &[])?;
    catalog["catalog"]
        .as_array()?
        .iter()
        .find(|k| k["kind"] == json!(kind))
        .and_then(|k| k["label"].as_str())
        .filter(|l| !l.is_empty())
        .map(str::to_string)
}

/// The sections a kind's own editor adds, in section order — the existing
/// editors under their English names. Empty for a kind with no editor of its
/// own. `gen.*`'s reaction controls fold into Parameters (2026-10-08), so it
/// carries only its `layer`; `text source` only its `text`.
fn own_sections(kind: &str) -> Vec<&'static str> {
    // M4 of the materials lote: a `mat.*` node's part and fixed knobs.
    if metal_graph::materials::is_material_kind(kind) {
        return vec![crate::shell::i18n::tr_key("material")];
    }
    // Parity P-A: the image nodes' non-float settings (`gen.ramp` too, which
    // has no layer of its own — it is not a model).
    if image::has_page(kind) {
        return vec![image::SECTION];
    }
    // M1 (plan "Value sources"): a `source.*` node's wave/constant pick. Its
    // floats ride on Parameters like every node's.
    if source_math::has_page(kind) {
        return vec![source_math::SECTION];
    }
    if kind.starts_with("gen.") {
        return vec![crate::shell::i18n::tr_key("layer"), crate::shell::i18n::tr_key("effects chain")];
    }
    if kind == "output.device" {
        return vec![crate::shell::i18n::tr_key("output device")];
    }
    if kind == "mask" {
        return vec![crate::shell::i18n::tr_key("mask")];
    }
    if kind == metal_graph::VENUE_SURFACE_KIND {
        // The room this picture is warped onto is the node's own (2026-10-21):
        // its map card opens the room's popup, so the surface step and the
        // screens/glass it lands on are one page.
        return vec![
            crate::shell::i18n::tr_key("map"),
            crate::shell::i18n::tr_key("surface"),
            crate::shell::i18n::tr_key("grid"),
        ];
    }
    if kind == TEXT_SOURCE_KIND {
        return vec![crate::shell::i18n::tr_key("text")];
    }
    if kind.starts_with("fx.text.") {
        return vec![crate::shell::i18n::tr_key("text effect")];
    }
    if kind == "fx.grade" {
        return vec![crate::shell::i18n::tr_key("grade"), crate::shell::i18n::tr_key("effect")];
    }
    if kind.starts_with("fx.") {
        return vec![crate::shell::i18n::tr_key("effect")];
    }
    if metal_graph::is_script_kind(kind) {
        return vec![crate::shell::i18n::tr_key("script")];
    }
    if kind == "audio.live" {
        return vec![crate::shell::i18n::tr_key("input device")];
    }
    if kind == "capture" {
        return vec![crate::shell::i18n::tr_key("capture source")];
    }
    if kind == metal_graph::REEL_BATCH_KIND {
        return vec![crate::shell::i18n::tr_key("reel list"), crate::shell::i18n::tr_key("reaction")];
    }
    if kind.starts_with("reel.") || kind == "router" {
        return vec![crate::shell::i18n::tr_key("reaction")];
    }
    if kind == metal_graph::AI_COLOURIST_KIND {
        return vec![crate::shell::i18n::tr_key("colourist")];
    }
    if kind == "out.sacn" {
        return vec![crate::shell::i18n::tr_key("sACN (E1.31)")];
    }
    if kind == metal_perform::artnet::ARTNET_KIND {
        return vec![crate::shell::i18n::tr_key("Art-Net")];
    }
    if kind == metal_graph::CLOCK_TIMECODE_KIND {
        return vec![crate::shell::i18n::tr_key("timecode")];
    }
    if kind == metal_graph::ARTNET_IN_KIND {
        return vec![crate::shell::i18n::tr_key("Art-Net in")];
    }
    if kind == "sensor.yolo" {
        return vec![crate::shell::i18n::tr_key("sensor")];
    }
    if kind == "cam.track" {
        return vec![crate::shell::i18n::tr_key("camera track")];
    }
    if kind == STACK_MODE_KIND {
        return vec![crate::shell::i18n::tr_key("mode")];
    }
    if kind == metal_graph::VENUE_FIXTURES_KIND {
        // The room is this node's own (2026-10-21): the mapping first
        // (which room, which output and fixture lands where), then the room
        // itself, its fixtures and its patch. The venue dropdown the old
        // `venue editor` page carried is gone: `map` names the room.
        return vec![
            crate::shell::i18n::tr_key("map"),
            crate::shell::i18n::tr_key("venue editor"),
            crate::shell::i18n::tr_key("fixture"),
            crate::shell::i18n::tr_key("patch"),
        ];
    }
    Vec::new()
}

/// The ordered section list for a node of `kind` (2026-10-08): its own
/// editor's sections, then Parameters when the node has floats (or always for
/// the kinds whose floats are the point), then Loop for generators/effects,
/// then the hook modules' sections. Pure, so the per-kind order is testable.
/// How long the browser trusts its `GET /graph` read with no write of its
/// own in between — other writers (the canvas, the AI) show within this.
const GRAPH_TTL: f64 = 1.5;

fn section_keys(kind: &str, has_params: bool) -> Vec<&'static str> {
    let mut keys = own_sections(kind);
    let always_params = kind.starts_with("gen.") || kind.starts_with("fx.") || kind == TEXT_SOURCE_KIND;
    if has_params || always_params {
        keys.push(crate::shell::i18n::tr_key("parameters"));
    }
    if kind.starts_with("gen.") || kind.starts_with("fx.") {
        keys.push(crate::shell::i18n::tr_key("loop"));
    }
    // Parity P-E: every source's in/out transition slots.
    if transition::has_slots(kind) {
        keys.push(transition::IN_SECTION);
        keys.push(transition::OUT_SECTION);
    }
    keys.extend_from_slice(crate::shell::panels::props_colour::folders(kind));
    keys.extend_from_slice(crate::shell::panels::props_venue::folders(kind));
    keys.extend_from_slice(crate::shell::panels::props_shader::folders(kind));
    keys
}

/// A library file's page (2026-09-30): what it is, where it is, and for an
/// audio the song library's own digest status — off `GET /author/songs`
/// (rule 5), never re-derived — with `digest` when it has none yet.
fn file_page(ui: &mut Ui, theme: &Theme, door: &dyn Call, file: &crate::shell::panels::FileRef) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(file.name.clone()).font(theme.font_body.clone()).color(theme.ink));
        ui.label(small_text(theme, file.kind.clone()).color(theme.ink_faint));
    });
    ui.add_space(theme.section_gap);
    node::field_row(ui, theme, tr("path"), &file.path);
    if file.kind == "video" {
        crate::shell::panels::scenes::gallery(ui, theme, door, &file.path);
        return;
    }
    if file.kind != "audio" {
        return;
    }
    let stem = std::path::Path::new(&file.name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&file.name)
        .to_string();
    let songs = shared::get(door, "GET", "/author/songs", &[]).unwrap_or_default();
    let song = songs["songs"]
        .as_array()
        .and_then(|a| a.iter().find(|s| s["name"].as_str() == Some(stem.as_str())).cloned());
    match song {
        None => node::field_row(ui, theme, tr("song"), tr("not in the song library yet")),
        Some(song) => {
            let status = song["status"].as_str().unwrap_or("?").to_string();
            node::field_row(ui, theme, tr("song"), &status);
            if let Some(err) = song["error"].as_str() {
                node::field_row(ui, theme, tr("error"), err);
            }
            if matches!(status.as_str(), "undigested" | "failed")
                && ui.button(small_text(theme, tr("digest"))).clicked()
            {
                let _ = shared::get(door, "POST", "/author/digest", &[("name", file.name.clone())]);
            }
        }
    }
}

/// Paint the whole `properties` region — [`PanelState::ui`]'s content inside
/// this region's own frame — and report the click-to-focus catcher as the
/// region's own [`Response`].
#[allow(clippy::too_many_arguments)]
pub fn paint(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    ingest: &mut crate::shell::lanes::IngestLane,
    selection: &Selection,
    picked: &[String],
    graph_id: Option<&str>,
    overlay_focus: Option<&str>,
    lanes_focused: bool,
    focused: bool,
) -> Response {
    let _span = crate::perf::Span::new("properties");
    ui.push_id(Id::new(super::Region::Properties.id()), |ui| {
        let mut frame = panel_frame(theme);
        if focused {
            frame = frame.stroke(theme.accent());
        }
        let inner = frame.show(ui, |ui| {
            ui.expand_to_include_rect(ui.max_rect());
            let full_rect = ui.max_rect();
            state.ui(ui, theme, door, ingest, selection, picked, graph_id, overlay_focus, lanes_focused, full_rect)
        });
        inner.inner
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    use serde_json::{json, Value};

    use crate::call::Call;
    use crate::shell::panels::Selection;
    use crate::shell::{FakeDoor, Shell};
    use egui_kittest::kittest::Queryable;

    // --- pure helpers -------------------------------------------------------

    /// The kind's own sections and the ordered section list: a gen carries
    /// only `layer` (its reaction folds into Parameters), a `text source` only
    /// `text`; Parameters always follows for the kinds whose floats are the
    /// point, Loop follows for generators/effects, and the hook folders come
    /// last.
    #[test]
    fn own_sections_and_section_keys_follow_the_kind() {
        assert_eq!(own_sections("mask"), vec!["mask"]);
        assert_eq!(own_sections("output.device"), vec!["output device"]);
        assert_eq!(own_sections("script.py"), vec!["script"]);
        assert_eq!(own_sections("gen.turing"), vec!["layer", "effects chain"]);
        assert_eq!(own_sections("fx.shake"), vec!["effect"]);
        assert_eq!(own_sections("fx.grade"), vec!["grade", "effect"]);
        assert_eq!(own_sections("fx.texture"), vec!["effect"]);
        assert_eq!(own_sections("fx.geometry"), vec!["effect"]);
        assert_eq!(own_sections("fx.text.wave"), vec!["text effect"]);
        assert_eq!(own_sections("out.sacn"), vec!["sACN (E1.31)"]);
        assert_eq!(own_sections("out.artnet"), vec!["Art-Net"]);
        assert_eq!(own_sections("clock.timecode"), vec!["timecode"]);
        assert_eq!(own_sections("in.artnet"), vec!["Art-Net in"]);
        assert_eq!(own_sections("reel.batch"), vec!["reel list", "reaction"]);
        assert_eq!(own_sections("audio.live"), vec!["input device"]);
        assert_eq!(own_sections("sensor.yolo"), vec!["sensor"]);
        assert_eq!(own_sections("cam.track"), vec!["camera track"]);
        assert_eq!(section_keys("gen.turing", false), vec!["layer", "effects chain", "parameters", "loop", "transition in", "transition out"]);
        assert_eq!(section_keys("fx.shake", false), vec!["effect", "parameters", "loop"]);
        assert_eq!(section_keys("fx.grade", false), vec!["grade", "effect", "parameters", "loop"]);
        assert_eq!(section_keys(TEXT_SOURCE_KIND, false), vec!["text", "parameters", "transition in", "transition out"]);
        assert_eq!(section_keys("reel.batch", false), vec!["reel list", "reaction", "transition in", "transition out"]);
        assert_eq!(section_keys("mask", false), vec!["mask"]);
        assert_eq!(section_keys("still", false), vec!["transition in", "transition out"]);
        assert_eq!(section_keys("script.py", true), vec!["script", "parameters"]);
    }

    /// M1 (plan "Value sources"): a `source.*` node opens on its `value source`
    /// page (the wave or the named constant), then Parameters — and has no
    /// transition slots, because it draws no picture.
    #[test]
    fn a_source_value_node_opens_on_its_own_source_page() {
        for kind in metal_graph::source::KINDS {
            assert_eq!(own_sections(kind), vec!["value source"], "{kind}'s own section");
            assert_eq!(
                section_keys(kind, true),
                vec!["value source", "parameters"],
                "{kind}'s ordered sections"
            );
        }
    }

    /// A transitive upstream's variables carry their used state and the
    /// producer triple a binding of the row would post.
    #[test]
    fn source_rows_mark_used_and_carry_the_bind_triple() {
        let io = json!({
            "sources": [{
                "node": "script-1", "label": "s", "kind": "script.py", "hops": 1,
                "vars": [
                    { "name": "x", "port": "control", "kind": "float", "port_ref": "control.x",
                      "used": true, "used_by": ["amount"], "via": [] },
                    { "name": "y", "port": "control", "kind": "float", "port_ref": "control.y",
                      "used": false, "used_by": [], "via": [] }
                ]
            }]
        });
        let live = json!({ "nodes": { "script-1": { "out": { "x": 0.5 } } } });
        let rows = io::source_rows(&io, &live);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].used);
        assert_eq!(rows[0].value, Some(0.5));
        assert_eq!(rows[0].bind, ("script-1".to_string(), "control".to_string(), "x".to_string()));
        assert!(!rows[1].used);
        assert_eq!(rows[1].value, None);
    }

    /// A table sorts by the picked column, numerically when both cells parse,
    /// and reverses on a second click.
    #[test]
    fn table_sort_reorders_numerically_and_reverses() {
        let rows = vec![
            browser::Row { cells: vec!["b".into(), "10".into()], used: false, key: browser::RowKey::None },
            browser::Row { cells: vec!["a".into(), "2".into()], used: false, key: browser::RowKey::None },
        ];
        let mut t = browser::Table::new(vec![browser::Column::new("N", 40.0), browser::Column::new("V", 40.0)], rows);
        t.sort = 1;
        t.ascending = true;
        t.sort_rows();
        assert_eq!(t.rows[0].cells[1], "2", "numeric ascending");
        t.ascending = false;
        t.sort_rows();
        assert_eq!(t.rows[0].cells[1], "10", "numeric descending");
    }

    /// The bind and param writers carry the exact bodies the routes read.
    #[test]
    fn post_bind_and_post_param_carry_the_exact_bodies() {
        let door = FakeDoor::new();
        io::post_bind(&door, "g1", "fx-0", "amount", Some(("script-1", "control", "x")));
        io::post_bind(&door, "g1", "fx-0", "amount", None);
        io::post_param(&door, "g1", "fx-0", "amount", 0.5);
        let calls = door.calls.borrow();
        let bodies: Vec<Value> = calls.iter().map(|(_, _, p)| serde_json::from_slice(&p.body).unwrap()).collect();
        assert_eq!(
            bodies[0],
            json!({ "op": "bind", "graph": "g1", "node": "fx-0", "param": "amount",
                     "from": { "node": "script-1", "port": "control", "var": "x" } })
        );
        assert_eq!(
            bodies[1],
            json!({ "op": "bind", "graph": "g1", "node": "fx-0", "param": "amount", "from": Value::Null })
        );
        assert_eq!(
            bodies[2],
            json!({ "op": "param", "graph": "g1", "node": "fx-0", "name": "amount", "value": 0.5_f32 })
        );
    }

    // --- kittest ------------------------------------------------------------

    fn shell_harness_of_size(door: Box<dyn Call>, width: f32, height: f32) -> egui_kittest::Harness<'static, Shell> {
        let theme = Theme::phosphor();
        let harness = egui_kittest::Harness::builder()
            .with_size(egui::Vec2::new(width, height))
            .build_ui_state(|ui, shell: &mut Shell| shell.ui(ui), Shell::new(theme.clone(), door));
        theme.apply(&harness.ctx);
        harness
    }

    fn shell_harness(door: Box<dyn Call>) -> egui_kittest::Harness<'static, Shell> {
        shell_harness_of_size(door, crate::shell::DESKTOP_VIEWPORT.0 as f32, crate::shell::DESKTOP_VIEWPORT.1 as f32)
    }

    /// A graph with `audio → gen → fx`, a script beside it, and the two routes
    /// the browser reads (`GET /graph/io`, `GET /graph/live`) scripted.
    fn browser_door() -> Rc<FakeDoor> {
        let recorder = Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/graphs",
            json!({ "graphs": [{ "id": "g1", "name": "g1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            json!({ "graph": { "id": "g1", "nodes": [
                { "id": "audio-0", "kind": "audio.live", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false, "config": {} },
                { "id": "gen-0", "kind": "gen.turing", "pos": [1.0, 0.0], "label": "", "bypass": false, "solo": false, "config": {} }
            ], "edges": [
                { "id": "e1", "from": { "node": "audio-0", "port": "control" }, "to": { "node": "gen-0", "port": "control" } }
            ] }, "primitives": [], "issues": [] }),
        );
        recorder.respond("GET", "/catalog", json!({ "catalog": [] }));
        recorder.respond(
            "GET",
            "/graph/io",
            json!({
                "node": "gen-0", "kind": "gen.turing", "label": "",
                "inputs": [{ "edge": "e1", "from": { "node": "audio-0", "label": "Audio", "kind": "audio.live", "port": "control" }, "to_port": "control" }],
                "outputs": [],
                "params": [{ "name": "scale", "min": 1.0, "max": 12.0, "default": 1.0, "kind": "float",
                             "bound": null, "live": 3.0 }],
                "sources": [{ "node": "audio-0", "label": "Audio", "kind": "audio.live", "hops": 1,
                    "vars": [{ "name": "onset", "port": "control", "kind": "float", "port_ref": "control.onset",
                               "used": true, "used_by": ["scale"], "via": [] }] }]
            }),
        );
        recorder.respond("GET", "/graph/live", json!({ "graph": "g1", "t": 1.0, "age_ms": 10, "nodes": {} }));
        recorder.respond(
            "GET",
            "/author/pagedata",
            json!({ "models": [{ "id": "turing", "bindings": [{ "param": "scale", "var": "onset" }] }] }),
        );
        recorder
    }

    /// H5: a graph with `capture → sensor.yolo → cam.track`, a `gen.turing`,
    /// and the routes both new pages read (`GET /io`'s video list,
    /// `GET /sensor`'s reading).
    fn sensor_door() -> Rc<FakeDoor> {
        let recorder = Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/graphs",
            json!({ "graphs": [{ "id": "g1", "name": "g1", "domain": "live" }] }),
        );
        recorder.respond(
            "GET",
            "/graph",
            json!({ "graph": { "id": "g1", "nodes": [
                { "id": "cap-0", "kind": "capture", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false,
                  "config": { "source": "v4l2:/dev/video0" } },
                { "id": "sensor-0", "kind": "sensor.yolo", "pos": [1.0, 0.0], "label": "", "bypass": false, "solo": false,
                  "config": { "min_cutoff": 1.0, "beta": 0.5 } },
                { "id": "cam-0", "kind": "cam.track", "pos": [2.0, 0.0], "label": "", "bypass": false, "solo": false,
                  "config": { "targets": [], "strength": 1.0, "invert": [false, false, false] } },
                { "id": "gen-0", "kind": "gen.turing", "pos": [3.0, 0.0], "label": "Turing", "bypass": false, "solo": false, "config": {} }
            ], "edges": [
                { "id": "e1", "from": { "node": "cap-0", "port": "image" }, "to": { "node": "sensor-0", "port": "image" } },
                { "id": "e2", "from": { "node": "sensor-0", "port": "control" }, "to": { "node": "cam-0", "port": "control" } }
            ] }, "primitives": [], "issues": [] }),
        );
        recorder.respond("GET", "/catalog", json!({ "catalog": [] }));
        recorder.respond("GET", "/graph/io", json!({ "inputs": [], "outputs": [], "params": [], "sources": [] }));
        recorder.respond("GET", "/graph/live", json!({ "graph": "g1", "t": 1.0, "age_ms": 10, "nodes": {} }));
        recorder.respond(
            "GET",
            "/io",
            json!({ "video": [{ "id": "v4l2:/dev/video0", "label": "cam · /dev/video0", "kind": "camera" }] }),
        );
        recorder.respond(
            "GET",
            "/sensor",
            json!({ "node": "sensor-0", "source": "v4l2:/dev/video0", "reading": true, "stale": false,
                    "eye_x": 0.6, "eye_y": 0.5, "eye_z_est": 0.5 }),
        );
        recorder
    }

    /// H5: the `sensor.yolo` page opens on the camera picker, the smoothing
    /// dials and the track indicator — all reachable without a camera.
    #[test]
    fn the_sensor_page_shows_camera_preview_and_smoothing() {
        let recorder = sensor_door();
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("sensor-0".to_string()));
        harness.run();

        harness.get_by_label("▾ sensor");
        harness.get_by_label("camera");
        harness.get_by_label("track");
        harness.get_by_label("following head");
        harness.get_by_label("min cutoff");
        harness.get_by_label("beta");
    }

    /// H5: the `cam.track` page opens on targets, strength and the per-axis
    /// invert boxes.
    #[test]
    fn the_cam_track_page_shows_targets_strength_and_invert() {
        let recorder = sensor_door();
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("cam-0".to_string()));
        harness.run();

        harness.get_by_label("▾ camera track");
        harness.get_by_label("all 3D generators");
        harness.get_by_label("strength");
        harness.get_by_label("invert");
    }

    /// H5: toggling "all 3D generators" off seeds the explicit target list
    /// with every generator, and on posts the empty list back.
    #[test]
    fn unchecking_all_generators_seeds_the_explicit_target_list() {
        let recorder = sensor_door();
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("cam-0".to_string()));
        harness.run();

        harness.get_by_label("all 3D generators").click();
        harness.run();
        let posted = recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
            .map(|(_, _, params)| serde_json::from_slice::<Value>(&params.body).unwrap())
            .find(|b| b["op"] == "config")
            .expect("unchecking all should post a config");
        assert_eq!(posted["config"]["targets"], json!(["gen-0"]));
    }

    /// The node card shows the `entradas:`/`salidas:` summary and the `gen.*`
    /// mode segments; the `detail` toggle reveals the raw input table, and a
    /// clicked input row selects that node through `pending_selection`.
    #[test]
    fn the_node_card_shows_the_summary_and_detail_reveals_the_input_row() {
        let recorder = browser_door();
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();

        // The card: the I/O summary and the mode segments.
        harness.get_by_label_contains("inputs:");
        harness.get_by_label_contains("outputs:");
        harness.get_by_label("AI");
        harness.get_by_label("AI FX");
        harness.get_by_label("manual");

        // The raw table is behind the `detail` toggle.
        harness.get_by_label("detail").click();
        harness.run();
        harness.get_by_label("Audio").scroll_to_me();
        harness.run();
        harness.get_by_label("Audio").click();
        harness.run();

        assert_eq!(
            harness.state().selection(),
            &Selection::Node("audio-0".to_string()),
            "clicking an input row selects the upstream node"
        );
    }

    /// A section's open state is remembered by its label across a focus change
    /// (2026-10-08): opening `loop` on one gen keeps it open on the next.
    #[test]
    fn a_section_state_survives_a_focus_change() {
        let recorder = browser_door();
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();

        // `loop` is the third section, closed by default; open it.
        harness.get_by_label("▸ loop").scroll_to_me();
        harness.run();
        harness.get_by_label("▸ loop").click();
        harness.run();
        harness.get_by_label("▾ loop");

        harness.state_mut().set_selection_for_test(Selection::Node("audio-0".to_string()));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();
        // The `loop` section is on screen open again without a click.
        harness.get_by_label("▾ loop");
    }

    /// No `node` and no `effects` section any more; the `layer` section is.
    #[test]
    fn there_is_no_node_or_effects_page() {
        let recorder = browser_door();
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();
        assert!(harness.query_by_label("node").is_none());
        assert!(harness.query_by_label("effects").is_none());
        harness.get_by_label("▾ layer");
    }

    /// A file selected in the library shows its own page here.
    #[test]
    fn a_selected_file_shows_its_page() {
        let recorder = browser_door();
        recorder.respond("GET", "/author/songs", json!({ "songs": [{ "name": "a", "status": "failed", "error": "no audio stream" }] }));
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::File(crate::shell::panels::FileRef {
            name: "a.mp3".into(), path: "/m/a.mp3".into(), kind: "audio".into(),
        }));
        harness.run();
        harness.get_by_label("a.mp3");
        harness.get_by_label("/m/a.mp3");
        harness.get_by_label("no audio stream");
    }

    /// The `customise sources` toggle reveals the source editor, which
    /// lists a transitive upstream's variables with their used state, and the
    /// row's `→ param` combo posts the bind op with the exact body.
    #[test]
    fn the_source_section_lists_used_vars_and_binding_posts_the_bind_op() {
        let recorder = browser_door();
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();

        // Parameters is open by default; the source editor is behind its toggle.
        harness.get_by_label("customise sources").scroll_to_me();
        harness.run();
        harness.get_by_label("customise sources").click();
        harness.run();
        harness.get_by_label("onset").scroll_to_me();
        harness.run();

        // The row's combo posts the bind op for the selected param. The popup
        // option is painted after the table, so it is the last `escala`.
        harness.get_all_by_value("→ param").next().unwrap().click();
        harness.run();
        harness.get_all_by_label("scale").last().unwrap().click();
        harness.run();

        let posted = recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
            .map(|(_, _, params)| serde_json::from_slice::<Value>(&params.body).unwrap())
            .find(|b| b["op"] == "bind")
            .expect("picking a param should POST the bind op");
        assert_eq!(
            posted,
            json!({ "op": "bind", "graph": "g1", "node": "gen-0", "param": "scale",
                     "from": { "node": "audio-0", "port": "control", "var": "onset" } })
        );
    }

    /// `restore sources` applies the model's default `(param, var)` table:
    /// each entry whose var an upstream source offers posts the bind op with
    /// the exact body, and the rest are skipped.
    #[test]
    fn restaurar_fuentes_posts_the_model_default_binds() {
        let recorder = browser_door();
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();

        harness.get_by_label("customise sources").scroll_to_me();
        harness.run();
        harness.get_by_label("customise sources").click();
        harness.run();
        harness.get_by_label("restore sources").scroll_to_me();
        harness.run();
        harness.get_by_label("restore sources").click();
        harness.run();

        let posted = recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
            .map(|(_, _, params)| serde_json::from_slice::<Value>(&params.body).unwrap())
            .find(|b| b["op"] == "bind")
            .expect("restore sources should POST the bind op");
        assert_eq!(
            posted,
            json!({ "op": "bind", "graph": "g1", "node": "gen-0", "param": "scale",
                     "from": { "node": "audio-0", "port": "control", "var": "onset" } })
        );
    }

    /// Parameters opens by default for a gen and lists the node's floats; its
    /// drag field is reachable (the post-once-on-release is proven by
    /// `post_bind_and_post_param_carry_the_exact_bodies`).
    #[test]
    fn the_parameters_section_lists_the_floats() {
        let recorder = browser_door();
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();

        harness.get_by_label("▾ parameters");
        harness.get_all_by_label("scale").next().unwrap().scroll_to_me();
    }

    /// A focused overlay button (lanes focused) takes over the panel: the
    /// `secuencia · <name>` section shows and resets the trigger id draft.
    #[test]
    fn a_focused_overlay_shows_its_section_and_arms_the_trigger_id() {
        let recorder = Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [{ "id": "screen-1", "layers": [{ "id": "l1" }] }] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [{ "name": "drop", "kind": "model", "ticks": 4, "favourite": false }] }));
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.state_mut().lanes_state_mut().set_focused_overlay_for_test("drop");
        harness.get_by_label(crate::shell::panels::Region::Lanes.title()).click();
        harness.run();
        harness.get_by_label("sequence · drop");
        assert_eq!(harness.state_mut().properties_state_mut().overlay_for(), Some("drop"));
    }

    /// `test` posts `POST /trigger/<id>` with the armed trigger id draft.
    #[test]
    fn test_posts_the_trigger_id() {
        let recorder = Rc::new(FakeDoor::new());
        recorder.respond("GET", "/state", json!({ "outputs": [{ "id": "screen-1", "layers": [{ "id": "l1" }] }] }));
        recorder.respond("GET", "/author/models/library", json!({ "movements": [{ "name": "drop", "kind": "model", "ticks": 4, "favourite": false }] }));
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.state_mut().lanes_state_mut().set_focused_overlay_for_test("drop");
        harness.get_by_label(crate::shell::panels::Region::Lanes.title()).click();
        harness.run();
        harness.state_mut().properties_state_mut().set_overlay_trigger_id("pedal");
        harness.run();
        harness.get_by_label("test").scroll_to_me();
        harness.run();
        harness.get_by_label("test").click();
        harness.run();
        assert!(
            recorder.calls.borrow().iter().any(|(m, p, _)| m == "POST" && p == "/trigger/pedal"),
            "test should POST /trigger/pedal"
        );
    }

    /// On a narrow width the sections are still reachable — the layout no
    /// longer switches to a chips strip, so `layer` and `parameters` paint the
    /// same collapsible headers.
    #[test]
    fn the_narrow_layout_shows_the_sections() {
        let recorder = browser_door();
        let mut harness = shell_harness_of_size(Box::new(Rc::clone(&recorder)), 620.0, 900.0);
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();

        harness.get_by_label("▾ layer");
        harness.get_by_label("▾ parameters");
    }

    /// Focusing lanes shows the active track's layer card — its
    /// `layer N · model` identity, its page and its clip count — and its Layer
    /// controls; toggling `muted`/`solo`, cycling `blend` and stepping
    /// `opacity` write the active `(page, layer)` meta (2026-10-08, §1/§6).
    #[test]
    fn the_lanes_layer_card_shows_and_edits_the_active_layer_meta() {
        let recorder = browser_door();
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run();
        {
            let ingest = harness.state_mut().ingest_state_mut();
            ingest.model = "gen.turing".into();
            ingest.set_layer_meta(
                &ingest.current_page(),
                0,
                crate::shell::lanes::LayerMeta { opacity: 0.0, ..Default::default() },
            );
        }
        let head = harness.get_by_label(crate::shell::panels::Region::Lanes.title()).rect();
        harness.drag_at(head.left_top() + head.size() * 0.02);
        harness.run();
        harness.drop_at(head.left_top() + head.size() * 0.02);
        harness.run();

        // The card: identity, page, clip count and the Layer section.
        assert!(harness.get_all_by_label_contains("layer 1 · gen.turing").next().is_some());
        assert!(harness.get_all_by_label_contains("clip 1/1").next().is_some());
        harness.get_by_label("page");
        harness.get_by_label("▾ layer");
        harness.get_by_label("blend");
        harness.get_by_label("opacity");
        harness.get_by_label("muted");
        harness.get_all_by_label("solo").next().unwrap();

        // `muted` and `solo` write the active layer's meta.
        harness.get_by_label("muted").click();
        harness.run();
        harness.get_all_by_label("solo").next().unwrap().click();
        harness.run();
        {
            let ingest = harness.state_mut().ingest_state_mut();
            let meta = ingest.layer_meta(&ingest.current_page(), 0);
            assert!(meta.muted, "the muted toggle writes the active layer");
            assert!(meta.solo, "the solo toggle writes the active layer");
        }

        // The blend chip cycles normal → add.
        harness.get_by_label("normal").click();
        harness.run();
        {
            let ingest = harness.state_mut().ingest_state_mut();
            assert_eq!(ingest.layer_meta(&ingest.current_page(), 0).blend, "add");
        }

        // The opacity track's right arrow steps it above the seeded 0.0.
        let rect = harness
            .get_all_by_role(egui::accesskit::Role::Slider)
            .next()
            .expect("the opacity track registers as a slider")
            .rect();
        let right = egui::pos2(rect.right() - rect.height() * 0.5, rect.center().y);
        harness.drag_at(right);
        harness.run_steps(1);
        harness.drop_at(right);
        harness.run_steps(1);
        let ingest = harness.state_mut().ingest_state_mut();
        assert!(
            ingest.layer_meta(&ingest.current_page(), 0).opacity > 0.0,
            "the opacity track writes the active layer"
        );
    }

    // -- A3: the reaction section (`ai.embed`) ------------------------------

    /// One `gen.turing` source, optionally with an `ai.colourist` embedded in
    /// it — the two states the reaction page shows (`directa`/`entrenada`).
    fn reaction_door(embedded: bool) -> Rc<FakeDoor> {
        let recorder = Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/graphs",
            json!({ "graphs": [{ "id": "g1", "name": "g1", "domain": "live" }] }),
        );
        let mut nodes = vec![json!({
            "id": "gen-0", "kind": "gen.turing", "pos": [0.0, 0.0], "label": "",
            "bypass": false, "solo": false, "config": {}
        })];
        if embedded {
            nodes.push(json!({
                "id": "ai-0", "kind": metal_graph::AI_COLOURIST_KIND, "pos": [0.0, 0.0], "label": "",
                "bypass": false, "solo": false, "config": { "weights": null }, "embedded_in": "gen-0"
            }));
        }
        recorder.respond(
            "GET",
            "/graph",
            json!({ "graph": { "id": "g1", "nodes": nodes, "edges": [] }, "primitives": [], "issues": [] }),
        );
        recorder.respond("GET", "/catalog", json!({ "catalog": [] }));
        recorder
    }

    fn posted_embed(recorder: &FakeDoor) -> Option<Value> {
        recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
            .map(|(_, _, params)| serde_json::from_slice::<Value>(&params.body).unwrap())
            .find(|b| b["op"] == "ai.embed")
    }

    /// The reaction toggle posts `ai.embed on: true` — the op that mints an
    /// owned chip on the source, never a free `op: "add"` node. The graph
    /// gesture and the UI control are one route (rule 6).
    #[test]
    fn the_reaction_toggle_creates_a_chip_not_a_free_node() {
        let recorder = reaction_door(false);
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();
        // The reaction controls fold into Parameters, open by default.
        // The checkbox sits below the properties column's fold at the default
        // size; scroll it into view before the click.
        harness.get_by_label("colourist").scroll_to_me();
        harness.run();
        harness.get_by_label("colourist").click();
        harness.run();

        assert_eq!(
            posted_embed(&recorder),
            Some(json!({ "op": "ai.embed", "graph": "g1", "node": "gen-0",
                          "ai": metal_graph::AI_COLOURIST_KIND, "on": true })),
            "the toggle posts the ai.embed op"
        );
        let added = recorder.calls.borrow().iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/graph/node"
                && serde_json::from_slice::<Value>(&params.body)
                    .map(|b| b["op"] == "add")
                    .unwrap_or(false)
        });
        assert!(!added, "a reaction is a chip, not a free node");
    }

    /// The source's own `mode` (2026-10-08): the three-way segment on a
    /// `gen.*`'s reaction page posts one `config` merge with `mode` — the
    /// single field the engine reads to pick the drive.
    #[test]
    fn the_source_modo_posts_the_config_mode() {
        let recorder = reaction_door(false);
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();
        // The mode now lives in the node card, always visible.
        harness.get_by_label("AI").click();
        harness.run();

        let posted = recorder.calls.borrow().iter().any(|(m, p, params)| {
            m == "POST"
                && p == "/graph/node"
                && serde_json::from_slice::<Value>(&params.body)
                    .map(|b| b["op"] == "config" && b["config"]["mode"] == "ai")
                    .unwrap_or(false)
        });
        assert!(posted, "the mode segment posts config mode: ai");
    }

    /// `separar` posts the same op with `on: false`, detaching the owned node
    /// so it becomes a normal free node — one gesture, one route.
    #[test]
    fn separar_turns_the_chip_into_a_node() {
        let recorder = reaction_door(true);
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run_steps(3);
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run_steps(3);
        harness.get_by_label("separate").scroll_to_me();
        harness.run_steps(3);
        harness.get_by_label("separate").click();
        harness.run_steps(3);

        assert_eq!(
            posted_embed(&recorder),
            Some(json!({ "op": "ai.embed", "graph": "g1", "node": "gen-0",
                          "ai": metal_graph::AI_COLOURIST_KIND, "on": false })),
            "separar posts the same ai.embed op with on: false"
        );
    }

    // -- D8: the colour-source selector (the tone design notes step 11) ------------------

    /// One `gen.turing` whose `control` is wired from an `audio.live`, so the
    /// reaction page paints its drive dials (the `colour source` selector) —
    /// without an audio edge [`super::listen::reaction_section`] early-returns
    /// "sin audio" and the dials never exist to test. `initial` is the node's
    /// saved `drive.colour_source`, so a test can start from a value other than
    /// the default and prove a click posts a change.
    fn colour_source_door(initial: &str) -> Rc<FakeDoor> {
        let recorder = Rc::new(FakeDoor::new());
        recorder.respond(
            "GET",
            "/graphs",
            json!({ "graphs": [{ "id": "g1", "name": "g1", "domain": "live" }] }),
        );
        let nodes = vec![
            json!({
                "id": "gen-0", "kind": "gen.turing", "pos": [0.0, 0.0], "label": "",
                "bypass": false, "solo": false,
                "config": { "drive": { "colour_source": initial } }
            }),
            json!({
                "id": "audio-0", "kind": "audio.live", "pos": [0.0, 0.0], "label": "",
                "bypass": false, "solo": false, "config": {}
            }),
        ];
        let edges = vec![json!({
            "from": { "node": "audio-0", "port": "out" },
            "to": { "node": "gen-0", "port": "control" }
        })];
        recorder.respond(
            "GET",
            "/graph",
            json!({ "graph": { "id": "g1", "nodes": nodes, "edges": edges }, "primitives": [], "issues": [] }),
        );
        recorder.respond("GET", "/catalog", json!({ "catalog": [] }));
        recorder
    }

    /// The `drive.colour_source` a `config` POST carried, if any.
    fn posted_colour_source(recorder: &FakeDoor) -> Option<String> {
        recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
            .map(|(_, _, params)| serde_json::from_slice::<Value>(&params.body).unwrap())
            .find(|b| {
                b["op"] == "config" && b["config"][metal_perform::drive::CONFIG_KEY]["colour_source"].is_string()
            })
            .and_then(|b| {
                b["config"][metal_perform::drive::CONFIG_KEY]["colour_source"]
                    .as_str()
                    .map(str::to_string)
            })
    }

    /// Clicking each option posts one `config` merge carrying its
    /// `colour_source` wire value — the same route (rule 6) `set_params` reaches.
    /// Each case starts from a *different* saved value so a click is a real
    /// change (`auto` from `tone`, `tone`/`chroma` from `auto`), which is what
    /// makes the heartbeat post at all.
    #[test]
    fn the_colour_source_selector_writes_each_option() {
        for (initial, label, value) in [
            ("tone", "auto", "auto"),
            ("auto", "tone", "tone"),
            ("auto", "chroma", "chroma"),
        ] {
            let recorder = colour_source_door(initial);
            let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
            harness.run();
            harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
            harness.run();
            harness.get_by_label(label).scroll_to_me();
            harness.run();
            harness.get_by_label(label).click();
            harness.run();

            assert_eq!(
                posted_colour_source(&recorder).as_deref(),
                Some(value),
                "clicking {label:?} posts colour_source = {value:?}"
            );
        }
    }

    /// A generator graph for the efectos section: `gen-0`, then the chain
    /// `fx-0` when `chained` (else the generator feeds the device directly),
    /// and an `output.device` at the end. The catalog is the door's relevance
    /// answer for `gen-0`, effects first.
    fn efectos_door(chained: bool) -> Rc<FakeDoor> {
        let recorder = Rc::new(FakeDoor::new());
        recorder.respond("GET", "/graphs", json!({ "graphs": [{ "id": "g1", "name": "g1", "domain": "live" }] }));
        let mut nodes = vec![
            json!({ "id": "gen-0", "kind": "gen.turing", "pos": [0.0, 0.0], "label": "",
                    "bypass": false, "solo": false, "config": {} }),
            json!({ "id": "out-0", "kind": "output.device", "pos": [400.0, 0.0], "label": "",
                    "bypass": false, "solo": false, "config": {} }),
        ];
        let mut edges = vec![json!({ "from": { "node": "gen-0", "port": "image" },
                                    "to": { "node": "out-0", "port": "image" } })];
        if chained {
            nodes.push(json!({ "id": "fx-0", "kind": "fx.shake", "pos": [200.0, 0.0], "label": "",
                               "bypass": false, "solo": false, "config": {} }));
            edges = vec![
                json!({ "from": { "node": "gen-0", "port": "image" }, "to": { "node": "fx-0", "port": "image" } }),
                json!({ "from": { "node": "fx-0", "port": "image" }, "to": { "node": "out-0", "port": "image" } }),
            ];
        }
        recorder.respond(
            "GET",
            "/graph",
            json!({ "graph": { "id": "g1", "domain": "live", "nodes": nodes, "edges": edges },
                    "primitives": [], "issues": [] }),
        );
        recorder.respond(
            "GET",
            "/catalog",
            json!({ "catalog": [
                { "kind": "fx.grade", "family": "effect", "label": "grade", "inputs": [], "outputs": [], "menu": true, "domains": ["live", "offline"] },
                { "kind": "fx.shake", "family": "effect", "label": "shake", "inputs": [], "outputs": [], "menu": true, "domains": ["live", "offline"] },
                { "kind": "gen.rings", "family": "generator", "label": "Rings", "inputs": [], "outputs": [], "menu": true, "domains": ["live", "offline"] }
            ] }),
        );
        recorder
    }

    /// The bodies of the `POST /graph/node` calls, in order.
    fn posted_node_ops(recorder: &FakeDoor) -> Vec<Value> {
        recorder
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
            .map(|(_, _, params)| serde_json::from_slice::<Value>(&params.body).unwrap())
            .collect()
    }

    /// Plan "Surface sources" step 2: `+ efecto ▾` on a generator offers the
    /// effect kinds only, and a pick posts `insert` after the chain's last
    /// node, or after the generator when the chain is empty. The pick is
    /// then revealed on the canvas, and the selection stays on the generator.
    #[test]
    fn the_efectos_dropdown_inserts_after_the_chains_last_node() {
        for (chained, after) in [(true, "fx-0"), (false, "gen-0")] {
            let recorder = efectos_door(chained);
            let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
            harness.run();
            harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
            harness.run();

            // The canvas bar has its own `+ effect `; the properties one is
            // the first in the tree (the canvas's is the last).
            let efectos = harness.get_all_by_label("+ effect ").next().expect("the efectos menu");
            efectos.scroll_to_me();
            harness.run();
            harness.get_all_by_label("+ effect ").next().expect("the efectos menu").click();
            harness.run();
            assert!(harness.query_by_label("Rings").is_none(), "a generator is not an effect");
            harness.get_by_label("grade").click();
            harness.run();

            let ops = posted_node_ops(&recorder);
            assert_eq!(
                ops.last(),
                Some(&json!({ "op": "insert", "graph": "g1", "after": after, "kind": "fx.grade" })),
                "chained = {chained}"
            );
            assert_eq!(
                harness.state().selection(),
                &Selection::Node("gen-0".to_string()),
                "the selection stays on the node being edited"
            );
        }
    }

    /// The efectos list shows the chain in order, and the catalog is asked
    /// for the open graph with the generator as its anchor.
    #[test]
    fn the_efectos_list_asks_for_relevance_and_names_the_chain() {
        let recorder = efectos_door(true);
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();

        harness.get_by_label("fx.shake");
        let asked = recorder.calls.borrow().iter().any(|(m, p, params)| {
            m == "GET"
                && p == "/catalog"
                && params.query.get("graph").map(String::as_str) == Some("g1")
                && params.query.get("anchor").map(String::as_str) == Some("gen-0")
        });
        assert!(asked, "the dropdown asks /catalog for this graph, anchored on the generator");
    }


    /// A `gen.monstruo`-like generator graph for the Superficies section: `gen-0`
    /// feeding `out-0`, and (when `wired`) a membrane node `gen-m`, labelled
    /// `tierra`, feeding `gen-0`'s `tex.skin`. The catalog answers every
    /// `GET /catalog` the same way: the generator with `parts`, a membrane
    /// model, an effect, and one forge shader.
    fn surfaces_door(kind: &str, parts: &[&str], wired: bool, surfaces: Value) -> Rc<FakeDoor> {
        let recorder = Rc::new(FakeDoor::new());
        recorder.respond("GET", "/graphs", json!({ "graphs": [{ "id": "g1", "name": "g1", "domain": "live" }] }));
        let mut nodes = vec![
            json!({ "id": "gen-0", "kind": kind, "pos": [0.0, 0.0], "label": "",
                    "bypass": false, "solo": false, "config": { "surfaces": surfaces } }),
            json!({ "id": "out-0", "kind": "output.device", "pos": [400.0, 0.0], "label": "",
                    "bypass": false, "solo": false, "config": {} }),
        ];
        let mut edges = vec![json!({ "from": { "node": "gen-0", "port": "image" },
                                    "to": { "node": "out-0", "port": "image" } })];
        if wired {
            nodes.push(json!({ "id": "gen-m", "kind": "gen.membrane", "pos": [-200.0, 0.0], "label": "tierra",
                               "bypass": false, "solo": false, "config": {} }));
            edges.push(json!({ "from": { "node": "gen-m", "port": "image" },
                               "to": { "node": "gen-0", "port": "tex.skin" } }));
        }
        recorder.respond(
            "GET",
            "/graph",
            json!({ "graph": { "id": "g1", "domain": "live", "nodes": nodes, "edges": edges },
                    "primitives": [], "issues": [] }),
        );
        let mut inputs = vec![json!({ "name": "control", "ty": "control", "material": false })];
        inputs.extend(parts.iter().map(|p| json!({ "name": format!("tex.{p}"), "ty": "image", "material": true })));
        let image = json!([{ "name": "image", "ty": "image", "material": false }]);
        recorder.respond(
            "GET",
            "/catalog",
            json!({
                "catalog": [
                    { "kind": kind, "family": "generator", "label": "Monstruo", "inputs": inputs,
                      "outputs": [{ "name": "image", "ty": "image", "material": false }], "menu": true, "domains": ["live"] },
                    { "kind": "gen.membrane", "family": "generator", "label": "Membrane", "inputs": [],
                      "outputs": image, "menu": true, "domains": ["live"] },
                    { "kind": "fx.grade", "family": "effect", "label": "grade", "inputs": [], "outputs": image,
                      "menu": true, "domains": ["live"] }
                ],
                "shaders": ["wave"],
            }),
        );
        recorder.respond("POST", "/graph/node", json!({ "graph": { "id": "g1" }, "node": "n-new" }));
        recorder
    }

    /// Picking a model in the skin row posts `attach` with `port: "tex.skin"`
    /// and that model's kind.
    #[test]
    fn the_skin_row_attaches_a_model_and_reveals_it() {
        let recorder = surfaces_door("gen.monster", &["skin", "eyes"], false, json!({}));
        let mut harness = shell_harness_of_size(Box::new(Rc::clone(&recorder)), crate::shell::DESKTOP_VIEWPORT.0 as f32, 1600.0);
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();

        // The source dropdown is a combo box: its selected text is its value.
        let source = harness.get_all_by_value("dynamic colours").next().expect("the skin row's source");
        source.scroll_to_me();
        source.click();
        harness.run();
        harness.run();
        harness.get_by_label("Membrane").click();
        harness.run();

        let ops = posted_node_ops(&recorder);
        assert_eq!(
            ops.last(),
            Some(&json!({ "op": "attach", "graph": "g1", "node": "gen-0", "port": "tex.skin", "kind": "gen.membrane" })),
        );
    }

    /// `dynamic colours` detaches: `attach` with the node and port only.
    #[test]
    fn dynamic_colours_detaches_the_wired_source() {
        let recorder = surfaces_door("gen.monster", &["skin"], true, json!({}));
        let mut harness = shell_harness_of_size(Box::new(Rc::clone(&recorder)), crate::shell::DESKTOP_VIEWPORT.0 as f32, 1600.0);
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();

        let source = harness.get_by_value("tierra");
        source.scroll_to_me();
        source.click();
        harness.run();
        harness.get_by_label("dynamic colours").click();
        harness.run();

        assert_eq!(
            posted_node_ops(&recorder).last(),
            Some(&json!({ "op": "attach", "graph": "g1", "node": "gen-0", "port": "tex.skin" })),
        );
    }

    /// The mapping segment posts the whole `surfaces` object: the skin part
    /// set to `projector`, the eyes part carried over untouched.
    #[test]
    fn the_mapping_posts_the_surfaces_object_keeping_the_other_parts() {
        let eyes = json!({ "mix": 0.5, "mapping": "object", "scale": 2.0 });
        let recorder = surfaces_door("gen.monster", &["skin", "eyes"], true, json!({ "eyes": eyes }));
        let mut harness = shell_harness_of_size(Box::new(Rc::clone(&recorder)), crate::shell::DESKTOP_VIEWPORT.0 as f32, 1600.0);
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();

        let projector = harness.get_all_by_label("projector").next().expect("the mapping segment");
        projector.scroll_to_me();
        projector.click();
        harness.run();
        harness.run();

        assert_eq!(
            posted_node_ops(&recorder).last(),
            Some(&json!({
                "op": "config", "graph": "g1", "node": "gen-0",
                "config": { "surfaces": {
                    "eyes": eyes,
                    "skin": { "mix": 1.0, "mapping": "projector", "scale": 1.0 },
                } },
            })),
        );
    }

    /// A generator whose catalog entry has no `tex.*` input shows no
    /// Superficies section at all.
    #[test]
    fn a_generator_without_surface_ports_has_no_superficies_section() {
        let recorder = surfaces_door("gen.turing", &[], false, json!({}));
        let mut harness = shell_harness_of_size(Box::new(Rc::clone(&recorder)), crate::shell::DESKTOP_VIEWPORT.0 as f32, 1600.0);
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("gen-0".to_string()));
        harness.run();
        assert!(harness.query_by_label("surfaces").is_none());
    }

    /// A `mat.*` node's graph: a gen with two surface parts feeding a
    /// `mat.wood`, which feeds the output.
    fn material_door() -> Rc<FakeDoor> {
        let recorder = Rc::new(FakeDoor::new());
        recorder.respond("GET", "/graphs", json!({ "graphs": [{ "id": "g1", "name": "g1", "domain": "live" }] }));
        let nodes = vec![
            json!({ "id": "gen-0", "kind": "gen.monster", "pos": [0.0, 0.0], "label": "",
                    "bypass": false, "solo": false, "config": {} }),
            json!({ "id": "mat-0", "kind": "mat.wood", "pos": [200.0, 0.0], "label": "",
                    "bypass": false, "solo": false, "config": { "part": "*" } }),
            json!({ "id": "out-0", "kind": "output.device", "pos": [400.0, 0.0], "label": "",
                    "bypass": false, "solo": false, "config": {} }),
        ];
        let edges = vec![
            json!({ "from": { "node": "gen-0", "port": "image" }, "to": { "node": "mat-0", "port": "image" } }),
            json!({ "from": { "node": "mat-0", "port": "image" }, "to": { "node": "out-0", "port": "image" } }),
        ];
        recorder.respond(
            "GET",
            "/graph",
            json!({ "graph": { "id": "g1", "domain": "live", "nodes": nodes, "edges": edges },
                    "primitives": [], "issues": [] }),
        );
        recorder.respond(
            "GET",
            "/catalog",
            json!({ "catalog": [
                { "kind": "gen.monster", "family": "generator", "label": "Monstruo",
                  "inputs": [ { "name": "control", "ty": "control", "material": false },
                              { "name": "tex.skin", "ty": "image", "material": true },
                              { "name": "tex.eyes", "ty": "image", "material": true } ],
                  "outputs": [{ "name": "image", "ty": "image", "material": false }],
                  "menu": true, "domains": ["live"] },
            ], "shaders": [] }),
        );
        recorder.respond("POST", "/graph/node", json!({ "graph": { "id": "g1" }, "node": "mat-0" }));
        recorder
    }

    /// M4: the material page lists the model's parts and the `part` pick posts
    /// the same `POST /graph/node {op:"config"}` merge the AI's `set_material`
    /// uses (rule 6).
    #[test]
    fn the_material_page_picks_a_part_and_posts_the_config_merge() {
        let recorder = material_door();
        let mut harness = shell_harness_of_size(Box::new(Rc::clone(&recorder)), crate::shell::DESKTOP_VIEWPORT.0 as f32, 1600.0);
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("mat-0".to_string()));
        harness.run();

        let combo = harness.get_by_value("every surface");
        combo.scroll_to_me();
        combo.click();
        harness.run();
        harness.run();
        harness.get_by_label("eyes").click();
        harness.run();

        assert_eq!(
            posted_node_ops(&recorder).last(),
            Some(&json!({ "op": "config", "graph": "g1", "node": "mat-0", "config": { "part": "eyes" } })),
        );
    }

    /// A Ctrl+click multi-selection: the properties panel lists every picked
    /// node (its label and kind), not just the primary one.
    #[test]
    fn a_multi_selection_lists_every_picked_node() {
        let recorder = Rc::new(FakeDoor::new());
        recorder.respond("GET", "/graphs", json!({ "graphs": [{ "id": "g1", "name": "g1", "domain": "live" }] }));
        recorder.respond(
            "GET",
            "/graph",
            json!({
                "graph": { "id": "g1", "domain": "live", "nodes": [
                    { "id": "gen-0", "kind": "gen.turing", "pos": [0.0, 0.0], "label": "Pulso", "bypass": false, "solo": false, "config": null },
                    { "id": "fx-0", "kind": "fx.shake", "pos": [1.0, 0.0], "label": "", "bypass": false, "solo": false, "config": null }
                ] },
                "primitives": [], "issues": []
            }),
        );
        let mut harness = shell_harness(Box::new(Rc::clone(&recorder)));
        harness.run();
        harness.state_mut().set_selection_for_test(Selection::Node("fx-0".to_string()));
        harness.state_mut().nodes.picked = vec!["gen-0".to_string(), "fx-0".to_string()];
        harness.run();
        harness.get_by_label("2 nodes selected");
        harness.get_by_label("Pulso");
        harness.get_by_label("fx.shake");
    }
}
