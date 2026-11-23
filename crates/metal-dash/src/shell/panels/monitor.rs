//! W2, the Monitor view: a multiview grid of frame sources, and the draggable
//! split that puts it beside the Show grid (`ShellView::ShowMonitor`).
//!
//! Three kinds of cell, grouped under their own headings:
//! - **Cameras**: the `capture` nodes of the open graph (camera, screen, window).
//! - **Nodes**: every other node of the open graph, except its `output.*` sinks.
//! - **Outputs**: the non-preview screens of `GET /state`.
//!
//! Every picture is one the shell already fetches: a node's stage through
//! `GET /node.jpg` and a screen's composed frame through `GET /output.jpg`,
//! both through the same [`Frame`] seam the preview uses, so no new render is
//! added. A click takes the cell to program, the main output being the first
//! screen of `GET /state`:
//! - a screen: `POST /outputs {op:"composed_on"}` (the main output composes it);
//! - a camera or node of the main output's own graph (`device-<main>`): the
//!   graph's `output.*` sink is rewired so its `image` input reads the node
//!   (`POST /graph/edge`, the route the graph editor uses for every wire).
//!
//! The cell on program carries the red tally; the hovered cell (the preview
//! candidate) the green one.
//!
//! The split's ratio and orientation are `data/settings.json`'s
//! `monitor_split` and `monitor_orientation` (`call::settings`), read once
//! through `GET /settings` and written back when a drag or a flip ends.

use std::collections::HashMap;
use std::time::Duration;

use egui::{Align2, Id, Rect, Sense, Ui, UiBuilder, WidgetInfo, WidgetType};
use serde_json::{json, Value};
use web_time::Instant;

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::frames::{live_gap, Frame};
use crate::shell::i18n::tr;
use metal_perform::engine::LEGACY_PREVIEW as PREVIEW;

/// The split's orientation. `Horizontal` sets the two panes side by side (the
/// divider is vertical); `Vertical` stacks them (the divider is horizontal).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    #[default]
    Horizontal,
    Vertical,
}

impl Orientation {
    pub fn code(self) -> &'static str {
        match self {
            Orientation::Horizontal => "horizontal",
            Orientation::Vertical => "vertical",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "horizontal" => Some(Orientation::Horizontal),
            "vertical" => Some(Orientation::Vertical),
            _ => None,
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Orientation::Horizontal => Orientation::Vertical,
            Orientation::Vertical => Orientation::Horizontal,
        }
    }
}

/// The three headings the grid is grouped under, in paint order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Cameras,
    Nodes,
    Outputs,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Cameras => tr("Cameras"),
            Group::Nodes => tr("Nodes"),
            Group::Outputs => tr("Outputs"),
        }
    }
}

/// What a cell takes to program when it is clicked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A screen of `GET /state`: the main output composes its frame.
    Output { device: String },
    /// A node of the open graph: wired into the main output's sink.
    Node { graph: String, node: String },
}

/// One cell: its caption, its group, what a click takes, and the picture
/// route it fetches (`/output.jpg` or `/node.jpg`, with its query).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub key: String,
    pub group: Group,
    pub label: String,
    pub target: Target,
    pub route: &'static str,
    pub query: Vec<(&'static str, String)>,
}

/// How often the source list is re-read (a device or a node appearing shows
/// within this).
const LIST_EVERY: Duration = Duration::from_millis(750);

/// The panel's state: the source list, what is on program, the split (loaded
/// once from settings), and one [`Frame`] per cell.
pub struct PanelState {
    frames: HashMap<String, Frame>,
    /// The cells, grouped by [`Group`] in paint order.
    pub sources: Vec<Source>,
    /// The main output's device id (the program output), when there is one.
    pub main: Option<String>,
    /// The [`Source::key`] on program right now.
    pub live: Option<String>,
    /// The Show grid's share of the split, in [`SPLIT_BOUNDS`]'s bounds (the server validates the same range).
    pub split: f32,
    pub orientation: Orientation,
    loaded: bool,
    listed_at: Option<Instant>,
}

impl Default for PanelState {
    fn default() -> Self {
        PanelState {
            frames: HashMap::new(),
            sources: Vec::new(),
            main: None,
            live: None,
            split: DEFAULT_SPLIT,
            orientation: Orientation::default(),
            loaded: false,
            listed_at: None,
        }
    }
}

/// The graph id a device's own graph is stored under (`device-<id>`, the
/// convention `call::graph` and `shell::collection::device_of` already read).
pub fn device_graph(device: &str) -> String {
    format!("device-{device}")
}

impl PanelState {
    /// Reads the split once (the first frame with a door) and re-reads the
    /// sources when they are older than [`LIST_EVERY`]. `graph` is the open
    /// graph whose nodes become cells.
    pub fn refresh(&mut self, door: &dyn Call, graph: Option<&str>) {
        if !self.loaded {
            self.loaded = true;
            if let Ok(reply) = door.json("GET", "/settings", None) {
                if let Some(n) = reply["monitor_split"].as_f64() {
                    self.split = clamp_split(n as f32);
                }
                if let Some(o) = reply["monitor_orientation"].as_str().and_then(Orientation::parse) {
                    self.orientation = o;
                }
            }
        }
        if !self.listed_at.is_none_or(|t| t.elapsed() >= LIST_EVERY) {
            return;
        }
        self.listed_at = Some(Instant::now());
        let Ok(state) = door.json("GET", "/state", None) else {
            return;
        };
        let (screens, main, composed_on) = read_screens(&state);
        let nodes = graph
            .and_then(|g| get_graph(door, g).map(|reply| read_nodes(g, &reply)))
            .unwrap_or_default();
        self.sources = screens.into_iter().chain(nodes).collect();
        self.sources.sort_by_key(|s| s.group);
        self.live = match (&main, &composed_on) {
            (Some(_), Some(dev)) => Some(output_key(dev)),
            (Some(m), None) => {
                get_graph(door, &device_graph(m))
                    .as_ref()
                    .and_then(sink_feed)
                    .map(|node| node_key(&device_graph(m), &node))
                    .or_else(|| Some(output_key(m)))
            }
            _ => None,
        };
        self.main = main;
    }

    /// Writes the split and the orientation back (`POST /settings`). A refused
    /// write leaves the in-memory values, which the next frame still shows.
    pub fn save(&self, door: &dyn Call) {
        let _ = door.json(
            "POST",
            "/settings",
            Some(json!({ "monitor_split": self.split, "monitor_orientation": self.orientation.code() })),
        );
    }
}

/// `GET /graph?id=<gid>`, or `None` when the door refuses it.
fn get_graph(door: &dyn Call, gid: &str) -> Option<Value> {
    let params = crate::call::Params {
        query: std::collections::HashMap::from([("id".to_string(), gid.to_string())]),
        body: Vec::new(),
        tail: None,
    };
    match door.call("GET", "/graph", params) {
        Some(reply) if reply.status == 200 => serde_json::from_slice(&reply.body).ok(),
        _ => None,
    }
}

/// The split's bounds and default, mirrored from `call::settings::MONITOR_SPLIT`.
/// The shell may not name `call::settings` (gate 1), so the range lives here
/// too; the server still validates what is written.
pub const SPLIT_BOUNDS: (f32, f32) = (0.2, 0.8);
pub const DEFAULT_SPLIT: f32 = 0.5;

/// Clamps a split ratio into [`SPLIT_BOUNDS`].
pub fn clamp_split(n: f32) -> f32 {
    n.clamp(SPLIT_BOUNDS.0, SPLIT_BOUNDS.1)
}

/// The grid's `(columns, rows)` for `n` cells: as square as the count allows,
/// the extra row at the bottom. `(0, 0)` for none.
pub fn grid_dims(n: usize) -> (usize, usize) {
    if n == 0 {
        return (0, 0);
    }
    let cols = (n as f64).sqrt().ceil() as usize;
    (cols, n.div_ceil(cols))
}

fn output_key(device: &str) -> String {
    format!("output:{device}")
}

fn node_key(graph: &str, node: &str) -> String {
    format!("node:{graph}:{node}")
}

/// The screens in `GET /state` minus the preview, as cells; the first is the
/// main output, and its `composed_on` (if any) is returned with it.
pub fn read_screens(state: &Value) -> (Vec<Source>, Option<String>, Option<String>) {
    let screens: Vec<&Value> = state["outputs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|o| o["id"].as_str().is_some_and(|id| id != PREVIEW))
        .collect();
    let cells = screens
        .iter()
        .filter_map(|o| {
            let device = o["id"].as_str()?.to_string();
            let label = o["label"].as_str().unwrap_or(&device).to_string();
            Some(Source {
                key: output_key(&device),
                group: Group::Outputs,
                label,
                target: Target::Output { device: device.clone() },
                route: "/output.jpg",
                query: vec![("device", device)],
            })
        })
        .collect();
    let main = screens.first().and_then(|o| o["id"].as_str()).map(str::to_string);
    let composed_on = screens.first().and_then(|o| o["composed_on"].as_str()).map(str::to_string);
    (cells, main, composed_on)
}

/// The cells of the open graph `gid` (its `reply` is `GET /graph`): `capture`
/// nodes are cameras, the other non-sink nodes are nodes. Each draws its own
/// stage through `GET /node.jpg` on the device it runs on.
pub fn read_nodes(gid: &str, reply: &Value) -> Vec<Source> {
    let graph = &reply["graph"];
    graph["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let id = n["id"].as_str()?;
            let kind = n["kind"].as_str().unwrap_or_default();
            if kind.starts_with("output.") {
                return None;
            }
            let group = if kind == "capture" { Group::Cameras } else { Group::Nodes };
            let label = n["name"].as_str().or_else(|| n["label"].as_str()).unwrap_or(id).to_string();
            let device = crate::shell::collection::device_of(gid, reply, Some(id)).unwrap_or_else(|| gid.to_string());
            Some(Source {
                key: node_key(gid, id),
                group,
                label,
                target: Target::Node { graph: gid.to_string(), node: id.to_string() },
                route: "/node.jpg",
                query: vec![("device", device), ("graph", gid.to_string()), ("node", id.to_string())],
            })
        })
        .collect()
}

/// The node wired into the `image` input of the graph's `output.*` sink, if any.
pub fn sink_feed(reply: &Value) -> Option<String> {
    let graph = &reply["graph"];
    let sink = graph["nodes"]
        .as_array()?
        .iter()
        .find(|n| n["kind"].as_str().is_some_and(|k| k.starts_with("output.")))?
        .get("id")?
        .as_str()?;
    graph["edges"]
        .as_array()?
        .iter()
        .find(|e| e["to"]["node"].as_str() == Some(sink) && e["to"]["port"].as_str() == Some("image"))
        .and_then(|e| e["from"]["node"].as_str())
        .map(str::to_string)
}

/// The body that takes the screen `device` to program: the main output composes
/// `device`. Taking the main output's own frame clears the composition.
pub fn take_to_program_body(main: &str, device: &str) -> Value {
    let composed_on = if device == main { Value::Null } else { json!(device) };
    json!({ "op": "composed_on", "id": main, "composed_on": composed_on })
}

/// The `POST /graph/edge` bodies that wire `node` into the sink `out` of the
/// graph `gid` (`reply` is its `GET /graph`): first the edges already feeding
/// the sink's `image` input are removed, then the new one is added. Empty when
/// the node already feeds it alone.
pub fn wire_sink_bodies(gid: &str, reply: &Value, out: &str, node: &str) -> Vec<Value> {
    let edges = reply["graph"]["edges"].as_array().cloned().unwrap_or_default();
    let feeding: Vec<&Value> = edges
        .iter()
        .filter(|e| e["to"]["node"].as_str() == Some(out) && e["to"]["port"].as_str() == Some("image"))
        .collect();
    if feeding.len() == 1 && feeding[0]["from"]["node"].as_str() == Some(node) {
        return Vec::new();
    }
    let mut bodies: Vec<Value> = feeding
        .iter()
        .filter_map(|e| e["id"].as_str())
        .map(|id| json!({ "graph": gid, "op": "remove", "edge": id }))
        .collect();
    bodies.push(json!({
        "graph": gid,
        "op": "add",
        "from": { "node": node, "port": "image" },
        "to": { "node": out, "port": "image" },
    }));
    bodies
}

/// Takes `source` to program. A screen goes through `composed_on`; a node must
/// live in the main output's graph, and is wired into that graph's sink. A
/// refusal is answered as its text.
pub fn take_to_program(door: &dyn Call, main: &str, source: &Source) -> Result<(), String> {
    match &source.target {
        Target::Output { device } => door
            .json("POST", "/outputs", Some(take_to_program_body(main, device)))
            .map(|_| ())
            .map_err(|(_, e)| e),
        Target::Node { graph, node } => {
            let gid = device_graph(main);
            if *graph != gid {
                return Err(format!("{node} is not in the graph of the main output"));
            }
            let reply = get_graph(door, &gid).ok_or_else(|| "the main output's graph is not readable".to_string())?;
            let sink = reply["graph"]["nodes"]
                .as_array()
                .and_then(|ns| ns.iter().find(|n| n["kind"].as_str().is_some_and(|k| k.starts_with("output."))))
                .and_then(|n| n["id"].as_str())
                .ok_or_else(|| "the main output has no sink node".to_string())?
                .to_string();
            for body in wire_sink_bodies(&gid, &reply, &sink, node) {
                door.json("POST", "/graph/edge", Some(body)).map_err(|(_, e)| e)?;
            }
            Ok(())
        }
    }
}

/// Paints the multiview into the whole of `ui`'s available rect. `graph` is the
/// open graph whose nodes are cells.
pub fn paint_grid(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState, graph: Option<&str>) {
    state.refresh(door, graph);
    let rect = ui.available_rect_before_wrap();
    ui.painter().rect_filled(rect, 0.0, theme.inset);
    let sources = state.sources.clone();
    if sources.is_empty() {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            tr("no outputs"),
            theme.font_body.clone(),
            theme.ink_weak,
        );
        return;
    }
    let gap = ui.spacing().item_spacing.x;
    let heading = theme.panel_head_height;
    let groups: Vec<Group> = [Group::Cameras, Group::Nodes, Group::Outputs]
        .into_iter()
        .filter(|g| sources.iter().any(|s| s.group == *g))
        .collect();
    let rows_of = |g: Group| {
        let n = sources.iter().filter(|s| s.group == g).count();
        grid_dims(n).1
    };
    let total_rows: usize = groups.iter().map(|g| rows_of(*g)).sum();
    let headings = groups.len() as f32 * heading;
    let row_h = ((rect.height() - headings) / total_rows as f32).max(1.0);
    let mut top = rect.min.y;
    let mut clicked = None;
    for group in groups {
        ui.painter().text(
            egui::pos2(rect.min.x + gap, top),
            Align2::LEFT_TOP,
            group.title(),
            theme.font_small.clone(),
            theme.ink_weak,
        );
        top += heading;
        let cells: Vec<&Source> = sources.iter().filter(|s| s.group == group).collect();
        let (cols, rows) = grid_dims(cells.len());
        let cell_w = ((rect.width() - gap * (cols as f32 - 1.0)) / cols as f32).max(1.0);
        for (i, src) in cells.iter().enumerate() {
            let (c, r) = ((i % cols) as f32, (i / cols) as f32);
            let cell = Rect::from_min_size(
                egui::pos2(rect.min.x + c * (cell_w + gap), top + r * row_h),
                egui::vec2(cell_w, (row_h - gap).max(1.0)),
            );
            let response = ui.interact(cell, Id::new(("monitor/cell", &src.key)), Sense::click());
            let label = src.label.clone();
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label.clone()));
            let frame = state.frames.entry(src.key.clone()).or_default();
            let mut query: Vec<(&str, String)> = src.query.clone();
            query.push(("w", (cell_w.round() as u32).max(1).to_string()));
            let picture = frame.picture(ui.ctx(), door, src.route, &query, live_gap());
            let painter = ui.painter();
            painter.rect_filled(cell, 0.0, theme.panel);
            if let Some((id, _)) = picture {
                let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                painter.image(id, cell, uv, theme.image_tint);
            }
            painter.text(
                cell.left_bottom() + theme.caption_inset(),
                Align2::LEFT_BOTTOM,
                &src.label,
                theme.font_small.clone(),
                theme.ink,
            );
            if state.live.as_deref() == Some(src.key.as_str()) {
                painter.rect_stroke(cell, 0.0, theme.tally_stroke(theme.tally), egui::StrokeKind::Inside);
            }
            if response.hovered() {
                painter.rect_stroke(cell, 0.0, theme.tally_stroke(theme.tally_preview), egui::StrokeKind::Inside);
            }
            if response.clicked() {
                clicked = Some((*src).clone());
            }
        }
        top += rows as f32 * row_h;
    }
    ui.allocate_rect(rect, Sense::hover());
    if let (Some(source), Some(main)) = (clicked, state.main.clone()) {
        let _ = take_to_program(door, &main, &source);
    }
}

/// Splits the available rect for the Show grid and the multiview: returns the
/// two sub-rects `(show, monitor)` and paints the drag bar between them. The
/// bar carries the orientation toggle; a drag moves the ratio, and the end of
/// a drag or a flip is written to settings.
pub fn split_rects(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState, graph: Option<&str>) -> (Rect, Rect) {
    state.refresh(door, graph);
    let rect = ui.available_rect_before_wrap();
    let side_by_side = state.orientation == Orientation::Horizontal;
    let handle = theme.split_handle;
    let total = (if side_by_side { rect.width() } else { rect.height() } - handle).max(0.0);
    let first = total * state.split;
    let (a, bar, b) = if side_by_side {
        let left = Rect::from_min_max(rect.min, egui::pos2(rect.min.x + first, rect.max.y));
        let bar = Rect::from_min_max(egui::pos2(left.max.x, rect.min.y), egui::pos2(left.max.x + handle, rect.max.y));
        let right = Rect::from_min_max(egui::pos2(bar.max.x, rect.min.y), rect.max);
        (left, bar, right)
    } else {
        let top = Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.min.y + first));
        let bar = Rect::from_min_max(egui::pos2(rect.min.x, top.max.y), egui::pos2(rect.max.x, top.max.y + handle));
        let bottom = Rect::from_min_max(egui::pos2(rect.min.x, bar.max.y), rect.max);
        (top, bar, bottom)
    };

    let drag = ui.interact(bar, Id::new("monitor/split"), Sense::drag());
    if drag.dragged() && total > 0.0 {
        let delta = if side_by_side { drag.drag_delta().x } else { drag.drag_delta().y };
        state.split = clamp_split((first + delta) / total);
    }
    if drag.drag_stopped() {
        state.save(door);
    }
    ui.painter().rect_filled(bar, 0.0, theme.rule);

    // The orientation toggle sits in the bar, its own small box so it never
    // reads as part of the drag.
    let toggle = Rect::from_center_size(bar.center(), egui::vec2(bar.width().min(bar.height()), bar.width().min(bar.height())));
    let clicked = ui
        .scope_builder(UiBuilder::new().max_rect(toggle), |ui| ui.small_button(tr("flip")).clicked())
        .inner;
    if clicked {
        state.orientation = state.orientation.toggled();
        state.save(door);
    }
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::FakeDoor;
    use egui_kittest::{kittest::Queryable, Harness};
    use std::rc::Rc;

    fn posts(door: &FakeDoor, path: &str) -> Vec<Value> {
        door.calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == path)
            .map(|(_, _, q)| serde_json::from_slice(&q.body).expect("json body"))
            .collect()
    }

    /// The grid is as square as the count allows, the extra row at the bottom.
    #[test]
    fn the_grid_is_as_square_as_the_count_allows() {
        assert_eq!(grid_dims(0), (0, 0));
        assert_eq!(grid_dims(1), (1, 1));
        assert_eq!(grid_dims(2), (2, 1));
        assert_eq!(grid_dims(5), (3, 2));
        assert_eq!(grid_dims(10), (4, 3));
    }

    /// Screens become output cells, the preview is not one, the first is the main
    /// output, and its `composed_on` is handed back.
    #[test]
    fn the_screens_minus_the_preview_are_the_output_cells() {
        let state = json!({ "outputs": [
            { "id": PREVIEW, "label": "preview" },
            { "id": "wall", "label": "Wall" },
            { "id": "pi", "label": "Pi", "composed_on": "wall" },
        ]});
        let (cells, main, composed_on) = read_screens(&state);
        let labels: Vec<&str> = cells.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Wall", "Pi"]);
        assert!(cells.iter().all(|c| c.group == Group::Outputs && c.route == "/output.jpg"));
        assert_eq!(main.as_deref(), Some("wall"));
        assert_eq!(composed_on, None, "the main output composes its own frame");
        assert!(read_screens(&json!({})).0.is_empty());
    }

    /// `capture` nodes are cameras, other nodes are nodes, sinks are not cells.
    #[test]
    fn capture_nodes_are_cameras_and_sinks_are_skipped() {
        let reply = json!({ "graph": { "nodes": [
            { "id": "cam", "kind": "capture", "name": "Front cam" },
            { "id": "fx", "kind": "fx.blur" },
            { "id": "out", "kind": "output.wall" },
        ]}});
        let cells = read_nodes("device-wall", &reply);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].group, Group::Cameras);
        assert_eq!(cells[0].label, "Front cam");
        assert_eq!(cells[1].group, Group::Nodes);
        assert_eq!(cells[1].label, "fx", "an unnamed node is captioned by its id");
        assert_eq!(cells[1].route, "/node.jpg");
    }

    /// Wiring a node into the sink removes the edge that fed it before adding the
    /// new one, and does nothing when the node already feeds the sink alone.
    #[test]
    fn wiring_the_sink_replaces_its_feed() {
        let reply = json!({ "graph": {
            "nodes": [{ "id": "out", "kind": "output.wall" }],
            "edges": [{ "id": "e1", "from": { "node": "gen", "port": "image" }, "to": { "node": "out", "port": "image" } }],
        }});
        assert_eq!(
            wire_sink_bodies("device-wall", &reply, "out", "cam"),
            vec![
                json!({ "graph": "device-wall", "op": "remove", "edge": "e1" }),
                json!({ "graph": "device-wall", "op": "add", "from": { "node": "cam", "port": "image" }, "to": { "node": "out", "port": "image" } }),
            ]
        );
        assert!(wire_sink_bodies("device-wall", &reply, "out", "gen").is_empty());
        assert_eq!(sink_feed(&reply).as_deref(), Some("gen"));
    }

    /// Taking a screen composes the main output on it; the main output's own frame
    /// clears the composition.
    #[test]
    fn taking_a_screen_composes_the_main_output_on_it() {
        assert_eq!(
            take_to_program_body("wall", "pi"),
            json!({ "op": "composed_on", "id": "wall", "composed_on": "pi" })
        );
        assert_eq!(
            take_to_program_body("wall", "wall"),
            json!({ "op": "composed_on", "id": "wall", "composed_on": null })
        );
    }

    /// The orientation vocabulary round-trips and toggles; a stored ratio outside
    /// the bounds clamps back into them.
    #[test]
    fn the_orientation_and_ratio_are_bounded_and_round_trip() {
        assert_eq!(Orientation::parse("vertical"), Some(Orientation::Vertical));
        assert_eq!(Orientation::parse("diagonal"), None);
        assert_eq!(Orientation::Horizontal.toggled(), Orientation::Vertical);
        assert_eq!(Orientation::Vertical.toggled().code(), "horizontal");
        assert_eq!(clamp_split(0.05), 0.2);
        assert_eq!(clamp_split(0.95), 0.8);
        assert!((PanelState::default().split - 0.5).abs() < 1e-6, "the split defaults to the middle");
    }

    /// The Monitor view paints one cell per screen and node from the door's state
    /// and graph, and a click on a screen posts its take.
    #[test]
    fn a_click_on_a_monitor_cell_posts_the_take() {
        let door = Rc::new(FakeDoor::new());
        door.respond("GET", "/settings", json!({}));
        door.respond("GET", "/state", json!({ "outputs": [
            { "id": "wall", "label": "Wall" },
            { "id": "pi", "label": "Pi" },
        ]}));
        door.respond("GET", "/graph", json!({ "graph": { "nodes": [
            { "id": "cam", "kind": "capture", "name": "Front cam" },
        ], "edges": [] }}));
        let door_for_ui = Rc::clone(&door);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(crate::shell::classes::SMALL_HARNESS.0, crate::shell::classes::SMALL_HARNESS.1))
            .build_ui_state(
                move |ui, state: &mut PanelState| {
                    let theme = Theme::phosphor();
                    let door: &dyn Call = door_for_ui.as_ref();
                    paint_grid(ui, &theme, door, state, Some("device-wall"));
                },
                PanelState::default(),
            );
        harness.run();
        harness.get_by_label("Front cam");
        harness.get_by_label("Pi").click();
        harness.run();
        assert_eq!(posts(&door, "/outputs"), vec![json!({ "op": "composed_on", "id": "wall", "composed_on": "pi" })]);
    }
}
