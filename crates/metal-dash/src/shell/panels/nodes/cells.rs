//! Param cells on the node canvas: the painting, the press/scrub/typed-edit/
//! pick gestures and their write-back. Geometry and hit-testing stay in
//! `metal-graph` (`cell` primitives, rule 5); every number's scrub math and
//! every cell's look is `components::cell_*`, shared with Properties.
//!
//! Writes go through the door's one routes: `op:"param"` for numbers,
//! `op:"config"` for bools, enums and strings; each gesture is one write and
//! one [`Undo`](super::Undo) entry.

use std::time::Duration;

use egui::{Id, Pos2, Rect as EguiRect, RichText, Vec2};
use serde_json::{json, Value};

use super::{
    num, push_undo, shifted_rect, to_graph, to_screen, DragOffset, LiveRef, ParamDrag, PanelState, Undo,
    PARAM_DRAG_SLOP,
};
use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{self, CellState};
use crate::shell::i18n::tr_dyn;
use crate::shell::panels::properties::shared::{note_write, post_node_config};
use crate::shell::panels::Selection;

/// Two presses on one cell this close are a double-click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// The last press on a cell, for the double-click that opens typed entry.
pub(super) struct CellClick {
    node: String,
    name: String,
    at: web_time::Instant,
}

/// Typed entry open on a cell (a number or a string).
pub(super) struct CellEdit {
    node: String,
    name: String,
    kind: String,
    buffer: String,
    min: f32,
    max: f32,
    was: Value,
    at: EguiRect,
    view: (Vec2, f32),
    first: bool,
    /// A vec cell's axis being typed (`was` then holds the whole vector).
    axis: Option<(usize, String, usize)>,
}

/// A vec scrub: which float of which vector param moves.
pub(super) struct VecDrag {
    pub(super) of: String,
    pub(super) offset: usize,
    pub(super) axis: usize,
    pub(super) base: Vec<f32>,
}

/// An enum cell's option list, open under the cell.
pub(super) struct CellPick {
    node: String,
    key: String,
    options: Vec<String>,
    current: String,
    at: EguiRect,
    view: (Vec2, f32),
    armed: bool,
}

/// The cell primitive of `(node, name)` in the cached graph.
pub(super) fn cell_prim<'a>(state: &'a PanelState, node: &str, name: &str) -> Option<&'a Value> {
    state.graph.as_ref()?["primitives"].as_array()?.iter().find(|p| {
        p["tag"] == "cell" && p["node"].as_str() == Some(node) && p["name"].as_str() == Some(name)
    })
}

/// A cell's screen rect, from its cached primitive.
fn screen_rect(state: &PanelState, origin: Pos2, p: &Value) -> EguiRect {
    let min = to_screen(origin, state.pan, state.zoom, [num(p, "x"), num(p, "y")]);
    EguiRect::from_min_size(min, Vec2::new(num(p, "w"), num(p, "h")) * state.zoom)
}

/// Writes `value` into the cached primitive so the cell never flashes the old
/// value before the reply lands.
fn set_prim(state: &mut PanelState, node: &str, name: &str, field: &str, value: Value) {
    let Some(prims) = state.graph.as_mut().and_then(|g| g["primitives"].as_array_mut()) else { return };
    if let Some(p) = prims.iter_mut().find(|p| {
        p["tag"] == "cell" && p["node"].as_str() == Some(node) && p["name"].as_str() == Some(name)
    }) {
        p[field] = value;
    }
}

fn prim_f32(p: &Value, key: &str) -> Option<f32> {
    p[key].as_f64().map(|v| v as f32)
}

/// One `op:"param"` write: the held value, one undo entry, the cache updated.
pub(super) fn post_param_value(
    door: &dyn Call,
    state: &mut PanelState,
    theme: &Theme,
    node: &str,
    name: &str,
    was: f32,
    value: f32,
) {
    let Some(gid) = state.selected_graph.clone() else { return };
    door.post_async(
        "/graph/node",
        json!({ "op": "param", "graph": gid, "node": node, "name": name, "value": value }),
    );
    note_write();
    push_undo(state, Undo::Param { node: node.to_string(), name: name.to_string(), was }, theme);
    set_prim(state, node, name, "value", json!(value));
    for peer in peers(state, node, name) {
        door.post_async(
            "/graph/node",
            json!({ "op": "param", "graph": gid, "node": peer, "name": name, "value": value }),
        );
        set_prim(state, &peer, name, "value", json!(value));
    }
}

/// The other nodes of a Ctrl+click multi-selection that carry a cell of the
/// same name and kind: an edit on one applies to all of them.
fn peers(state: &PanelState, node: &str, name: &str) -> Vec<String> {
    if state.picked.len() < 2 || !state.picked.iter().any(|p| p == node) {
        return Vec::new();
    }
    let kind = cell_prim(state, node, name).map(|p| p["kind"].clone());
    state
        .picked
        .iter()
        .filter(|p| p.as_str() != node)
        .filter(|p| cell_prim(state, p, name).is_some_and(|c| Some(&c["kind"]) == kind.as_ref() && c["bound"] != json!(true)))
        .cloned()
        .collect()
}

/// One `op:"config"` write for a bool / enum / string cell.
pub(super) fn post_config_value(
    door: &dyn Call,
    state: &mut PanelState,
    theme: &Theme,
    node: &str,
    key: &str,
    was: Value,
    value: Value,
) {
    let Some(gid) = state.selected_graph.clone() else { return };
    post_node_config(door, &gid, node, json!({ key: value }));
    push_undo(state, Undo::Config { node: node.to_string(), key: key.to_string(), was }, theme);
    apply_config_to_prim(state, node, key, &value);
    for peer in peers(state, node, key) {
        post_node_config(door, &gid, &peer, json!({ key: value }));
        apply_config_to_prim(state, &peer, key, &value);
    }
}

fn apply_config_to_prim(state: &mut PanelState, node: &str, key: &str, value: &Value) {
    match value {
        Value::Bool(_) => set_prim(state, node, key, "on", value.clone()),
        Value::String(_) => set_prim(state, node, key, "text", value.clone()),
        _ => {}
    }
}

/// Replays an undo entry through the same routes.
pub(super) fn replay_param(door: &dyn Call, state: &mut PanelState, node: &str, name: &str, was: f32) {
    let Some(gid) = state.selected_graph.clone() else { return };
    door.post_async(
        "/graph/node",
        json!({ "op": "param", "graph": gid, "node": node, "name": name, "value": was }),
    );
    note_write();
    set_prim(state, node, name, "value", json!(was));
}

/// The whole vector (`camera`: 7 floats) from its open vec cells.
fn vec_all(state: &PanelState, node: &str, of: &str) -> Option<Vec<f32>> {
    let mut all = Vec::new();
    for part in ["pos", "target", "fov"] {
        let p = cell_prim(state, node, &format!("{of}.{part}"))?;
        all.extend(p["values"].as_array()?.iter().filter_map(|v| v.as_f64().map(|f| f as f32)));
    }
    Some(all)
}

fn set_vec_prims(state: &mut PanelState, node: &str, of: &str, v: &[f32]) {
    if v.len() != 7 {
        return;
    }
    for (part, a, b) in [("pos", 0, 3), ("target", 3, 6), ("fov", 6, 7)] {
        set_prim(state, node, &format!("{of}.{part}"), "values", json!(&v[a..b]));
    }
}

fn post_vector(door: &dyn Call, state: &mut PanelState, node: &str, of: &str, v: &[f32]) {
    let Some(gid) = state.selected_graph.clone() else { return };
    door.post_async("/graph/node", json!({ "op": "param", "graph": gid, "node": node, "name": of, "value": v }));
    note_write();
    set_vec_prims(state, node, of, v);
}

pub(super) fn replay_vector(door: &dyn Call, state: &mut PanelState, node: &str, of: &str, was: &[f32]) {
    post_vector(door, state, node, of, was);
}

pub(super) fn replay_config(door: &dyn Call, state: &mut PanelState, node: &str, key: &str, was: &Value) {
    let Some(gid) = state.selected_graph.clone() else { return };
    post_node_config(door, &gid, node, json!({ key: was }));
    apply_config_to_prim(state, node, key, was);
}

/// A press that landed on a cell (`GET /graph/hit` answered `param` or
/// `cell`). Selects the node; a number starts a scrub (a second press inside
/// [`DOUBLE_CLICK`] opens typed entry), a bool flips, an enum opens its list,
/// a string opens typed entry on a double-click. A bound cell only selects.
#[allow(clippy::too_many_arguments)]
pub(super) fn press(
    door: &dyn Call,
    state: &mut PanelState,
    selection: &mut Selection,
    theme: &Theme,
    hit: &Value,
    origin: Pos2,
    press_pos: Pos2,
    now: web_time::Instant,
) {
    let (Some(node), Some(name)) = (hit["node"].as_str(), hit["name"].as_str()) else { return };
    *selection = Selection::Node(node.to_string());
    let Some(gid) = state.selected_graph.clone() else { return };
    let double = state
        .last_cell_click
        .as_ref()
        .is_some_and(|c| c.node == node && c.name == name && now.duration_since(c.at) < DOUBLE_CLICK);
    state.last_cell_click =
        if double { None } else { Some(CellClick { node: node.to_string(), name: name.to_string(), at: now }) };
    state.cell_edit = None;
    state.cell_pick = None;
    let prim = cell_prim(state, node, name)
        .cloned()
        .unwrap_or_else(|| json!({ "kind": hit["kind"].as_str().unwrap_or("float") }));
    if prim["bound"] == json!(true) {
        return;
    }
    let kind = prim["kind"].as_str().unwrap_or("float").to_string();
    let at = screen_rect(state, origin, &prim);
    let view = (state.pan, state.zoom);
    match kind.as_str() {
        "float" | "int" => {
            if state.zoom >= theme.node_cell_edit_zoom && prim["w"].is_number() {
                let zone = at.height();
                let dir = if press_pos.x < at.left() + zone { -1.0 } else if press_pos.x > at.right() - zone { 1.0 } else { 0.0 };
                if dir != 0.0 {
                    step(door, state, theme, hit, dir);
                    return;
                }
            }
            let (pmin, pmax) = (prim_f32(&prim, "min").unwrap_or(0.0), prim_f32(&prim, "max").unwrap_or(1.0));
            let shown = prim_f32(&prim, "value").or_else(|| prim_f32(&prim, "default")).unwrap_or(pmin);
            let (min, max, value) = super::param_bounds(door, &gid, node, name).unwrap_or((pmin, pmax, shown));
            if double {
                state.cell_edit = Some(CellEdit {
                    node: node.to_string(),
                    name: name.to_string(),
                    kind,
                    buffer: format_edit(value, kind_decimals(&prim, min, max)),
                    min,
                    max,
                    was: json!(value),
                    at,
                    view,
                    first: true,
                    axis: None,
                });
                return;
            }
            let [gx, gy] = to_graph(origin, state.pan, state.zoom, press_pos);
            state.param_drag = Some(ParamDrag {
                node: node.to_string(),
                name: name.to_string(),
                start_value: value,
                start_pointer: [gx, gy],
                cur_pointer: [gx, gy],
                min,
                max,
                value,
                int: kind == "int",
                start_screen_x: press_pos.x,
                vec: None,
            });
        }
        "colour" => {
            if let Some(key) = prim["key"].as_str() {
                let c = |i: usize| prim["rgb"][i].as_f64().unwrap_or(1.0) as f32;
                state.colour_menu = Some(super::ColourMenu {
                    node: node.to_string(),
                    key: key.to_string(),
                    rgb: [c(0), c(1), c(2)],
                    at: press_pos,
                });
            }
        }
        "button" => {
            post_node_config(door, &gid, node, json!({ name: true }));
        }
        "vec" => {
            let of = prim["vec_of"].as_str().unwrap_or("camera").to_string();
            let offset = prim["offset"].as_u64().unwrap_or(0) as usize;
            let n = prim["values"].as_array().map_or(1, Vec::len).max(1);
            let axis = (((press_pos.x - at.left()) / at.width().max(1.0) * n as f32) as usize).min(n - 1);
            let Some(base) = vec_all(state, node, &of) else { return };
            let value = base.get(offset + axis).copied().unwrap_or(0.0);
            let (min, max) = (prim_f32(&prim, "min").unwrap_or(-20.0), prim_f32(&prim, "max").unwrap_or(20.0));
            if double {
                state.cell_edit = Some(CellEdit {
                    node: node.to_string(),
                    name: of.clone(),
                    kind,
                    buffer: format_edit(value, 2),
                    min,
                    max,
                    was: json!(base),
                    at,
                    view,
                    first: true,
                    axis: Some((offset + axis, of, axis)),
                });
                return;
            }
            let [gx, gy] = to_graph(origin, state.pan, state.zoom, press_pos);
            state.param_drag = Some(ParamDrag {
                node: node.to_string(),
                name: of.clone(),
                start_value: value,
                start_pointer: [gx, gy],
                cur_pointer: [gx, gy],
                min,
                max,
                value,
                int: false,
                start_screen_x: press_pos.x,
                vec: Some(VecDrag { of, offset, axis, base }),
            });
        }
        "bool" => {
            let was = prim["on"].as_bool().unwrap_or(false);
            post_config_value(door, state, theme, node, name, json!(was), json!(!was));
        }
        "enum" => {
            state.cell_pick = Some(CellPick {
                node: node.to_string(),
                key: name.to_string(),
                options: prim["options"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|o| o.as_str().map(str::to_string)).collect())
                    .unwrap_or_default(),
                current: prim["text"].as_str().unwrap_or_default().to_string(),
                at,
                view,
                armed: false,
            });
        }
        "text" if double => {
            let was = prim["text"].as_str().unwrap_or_default().to_string();
            state.cell_edit = Some(CellEdit {
                node: node.to_string(),
                name: name.to_string(),
                kind,
                buffer: was.clone(),
                min: 0.0,
                max: 0.0,
                was: json!(was),
                at,
                view,
                first: true,
                axis: None,
            });
        }
        _ => {}
    }
}

fn kind_decimals(prim: &Value, min: f32, max: f32) -> usize {
    if prim["kind"] == "int" {
        0
    } else {
        components::cell_decimals(min, max)
    }
}

fn format_edit(value: f32, decimals: usize) -> String {
    components::cell_format(value, decimals)
}

/// The scrub follows the pointer's screen travel (zoom independent): Shift
/// divides by ten, Ctrl multiplies by ten. Nothing is sent until release.
pub(super) fn drag_to(state: &mut PanelState, origin: Pos2, pos: Pos2, shift: bool, ctrl: bool) {
    let (pan, zoom) = (state.pan, state.zoom);
    let Some(d) = state.param_drag.as_mut() else { return };
    d.cur_pointer = to_graph(origin, pan, zoom, pos);
    d.value = components::cell_value_from_drag(
        d.start_value,
        d.min,
        d.max,
        pos.x - d.start_screen_x,
        shift,
        ctrl,
        if d.int { 0 } else { 2 },
    );
}

/// Esc during a scrub: nothing is posted.
pub(super) fn cancel_drag(state: &mut PanelState) {
    state.param_drag = None;
}

/// Release: one write per gesture. A drag that never crossed the slop was a
/// click, already handled by the press.
pub(super) fn finish_drag(door: &dyn Call, state: &mut PanelState, theme: &Theme) {
    let Some(d) = state.param_drag.take() else { return };
    let dx = d.cur_pointer[0] - d.start_pointer[0];
    if dx.abs() <= PARAM_DRAG_SLOP || (d.value - d.start_value).abs() <= f32::EPSILON {
        return;
    }
    if let Some(v) = d.vec {
        let mut next = v.base.clone();
        if let Some(slot) = next.get_mut(v.offset + v.axis) {
            *slot = d.value;
        }
        push_undo(state, Undo::Vector { node: d.node.clone(), name: v.of.clone(), was: v.base }, theme);
        post_vector(door, state, &d.node, &v.of, &next);
        return;
    }
    post_param_value(door, state, theme, &d.node, &d.name, d.start_value, d.value);
}

/// Enter (or focus loss) on typed entry: a number is clamped to its range
/// unless `ctrl`; a string is written as is.
pub(super) fn commit_edit(door: &dyn Call, state: &mut PanelState, theme: &Theme, ctrl: bool) {
    let Some(e) = state.cell_edit.take() else { return };
    match e.kind.as_str() {
        "float" | "int" => {
            let Ok(mut v) = e.buffer.trim().parse::<f32>() else { return };
            if !v.is_finite() {
                return;
            }
            if !ctrl {
                v = v.clamp(e.min, e.max);
            }
            if e.kind == "int" {
                v = v.round();
            }
            let was = e.was.as_f64().unwrap_or(0.0) as f32;
            post_param_value(door, state, theme, &e.node, &e.name, was, v);
        }
        "vec" => {
            let Ok(mut v) = e.buffer.trim().parse::<f32>() else { return };
            if !v.is_finite() {
                return;
            }
            if !ctrl {
                v = v.clamp(e.min, e.max);
            }
            let Some((idx, of, _)) = e.axis.clone() else { return };
            let was: Vec<f32> = e.was.as_array().map(|a| a.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect()).unwrap_or_default();
            let mut next = was.clone();
            if let Some(slot) = next.get_mut(idx) {
                *slot = v;
            }
            push_undo(state, Undo::Vector { node: e.node.clone(), name: of.clone(), was }, theme);
            post_vector(door, state, &e.node, &of, &next);
        }
        "text" => {
            if e.was.as_str() != Some(e.buffer.as_str()) {
                post_config_value(door, state, theme, &e.node, &e.name, e.was.clone(), json!(e.buffer));
            }
        }
        _ => {}
    }
}

/// An option picked from an enum cell's list.
pub(super) fn pick_option(door: &dyn Call, state: &mut PanelState, theme: &Theme, option: &str) {
    let Some(p) = state.cell_pick.take() else { return };
    if p.current != option {
        post_config_value(door, state, theme, &p.node, &p.key, json!(p.current), json!(option));
    }
}

/// Right-click on a cell: a bound cell is unbound, a number resets to its
/// default.
pub(super) fn secondary(door: &dyn Call, state: &mut PanelState, theme: &Theme, hit: &Value) {
    let (Some(node), Some(name)) = (hit["node"].as_str(), hit["name"].as_str()) else { return };
    let Some(gid) = state.selected_graph.clone() else { return };
    let Some(prim) = cell_prim(state, node, name).cloned() else { return };
    if prim["bound"] == json!(true) {
        door.post_async(
            "/graph/node",
            json!({ "op": "bind", "graph": gid, "node": node, "param": name, "from": Value::Null }),
        );
        note_write();
        return;
    }
    if matches!(prim["kind"].as_str(), Some("float" | "int")) {
        let was = prim_f32(&prim, "value").or_else(|| prim_f32(&prim, "default")).unwrap_or(0.0);
        let default = prim_f32(&prim, "default").unwrap_or(0.0);
        if (was - default).abs() > f32::EPSILON {
            post_param_value(door, state, theme, node, name, was, default);
        }
    }
}

/// A panel header or the node chevron was pressed: the open panels and the
/// collapsed flag live on the node as `config.ui` (layout needs them).
pub(super) fn toggle(door: &dyn Call, state: &mut PanelState, hit: &Value) {
    let Some(node) = hit["node"].as_str() else { return };
    let Some(gid) = state.selected_graph.clone() else { return };
    let ui = super::node_record(state, node).map(|n| n["config"]["ui"].clone()).unwrap_or(Value::Null);
    let mut open: Vec<String> = ui["open"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let mut collapsed = ui["collapsed"].as_bool().unwrap_or(false);
    match hit["panel"].as_str() {
        Some(id) => {
            if let Some(i) = open.iter().position(|o| o == id) {
                open.remove(i);
            } else {
                open.push(id.to_string());
            }
        }
        None => collapsed = !collapsed,
    }
    post_node_config(door, &gid, node, json!({ "ui": { "open": open, "collapsed": collapsed } }));
}

/// Ctrl+C over a cell: the text the clipboard gets.
pub(super) fn copy_text(state: &PanelState, hit: &Value) -> Option<String> {
    let (node, name) = (hit["node"].as_str()?, hit["name"].as_str()?);
    let p = cell_prim(state, node, name)?;
    match p["kind"].as_str()? {
        "float" | "int" => {
            let v = prim_f32(p, "value").or_else(|| prim_f32(p, "default"))?;
            let d = if p["kind"] == "int" { 0 } else { components::cell_decimals(num(p, "min"), num(p, "max")) };
            Some(components::cell_format(v, d))
        }
        "enum" | "text" => p["text"].as_str().map(str::to_string),
        "bool" => p["on"].as_bool().map(|b| b.to_string()),
        _ => None,
    }
}

/// Ctrl+V over a cell: the clipboard text, if it fits the cell.
pub(super) fn paste(door: &dyn Call, state: &mut PanelState, theme: &Theme, hit: &Value, text: &str) {
    let (Some(node), Some(name)) = (hit["node"].as_str(), hit["name"].as_str()) else { return };
    let Some(p) = cell_prim(state, node, name).cloned() else { return };
    if p["bound"] == json!(true) {
        return;
    }
    let text = text.trim();
    match p["kind"].as_str() {
        Some("float" | "int") => {
            let Ok(mut v) = text.parse::<f32>() else { return };
            if !v.is_finite() {
                return;
            }
            v = v.clamp(num(&p, "min"), num(&p, "max"));
            if p["kind"] == "int" {
                v = v.round();
            }
            let was = prim_f32(&p, "value").or_else(|| prim_f32(&p, "default")).unwrap_or(0.0);
            post_param_value(door, state, theme, node, name, was, v);
        }
        Some("enum") => {
            let ok = p["options"].as_array().is_some_and(|o| o.iter().any(|x| x.as_str() == Some(text)));
            if ok {
                let was = p["text"].clone();
                post_config_value(door, state, theme, node, name, was, json!(text));
            }
        }
        Some("text") => {
            let was = p["text"].clone();
            post_config_value(door, state, theme, node, name, was, json!(text));
        }
        Some("bool") => {
            if let Ok(b) = text.parse::<bool>() {
                let was = p["on"].clone();
                post_config_value(door, state, theme, node, name, was, json!(b));
            }
        }
        _ => {}
    }
}

/// Ctrl+wheel over a number cell steps it; anywhere else the wheel zooms.
/// Returns whether the wheel was consumed.
pub(super) fn wheel_step(
    ui: &egui::Ui,
    door: &dyn Call,
    state: &mut PanelState,
    theme: &Theme,
    rect: EguiRect,
    response: &egui::Response,
) -> bool {
    let Some(pos) = response.hover_pos() else { return false };
    if state.zoom < theme.node_cell_edit_zoom {
        return false;
    }
    let dy: f32 = ui.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::MouseWheel { delta, modifiers, .. } if modifiers.command || modifiers.ctrl => Some(delta.y),
                _ => None,
            })
            .sum()
    });
    if dy == 0.0 {
        return false;
    }
    let Some(gid) = state.selected_graph.clone() else { return false };
    let [gx, gy] = to_graph(rect.min, state.pan, state.zoom, pos);
    let Some(h) = super::hit_at(door, &gid, gx, gy).filter(|h| matches!(h["hit"].as_str(), Some("param"))) else {
        return false;
    };
    step(door, state, theme, &h, dy.signum());
    true
}

/// Ctrl+wheel: one step of a hundredth of the range, up for `dir > 0`.
pub(super) fn step(door: &dyn Call, state: &mut PanelState, theme: &Theme, hit: &Value, dir: f32) {
    let (Some(node), Some(name)) = (hit["node"].as_str(), hit["name"].as_str()) else { return };
    let Some(prim) = cell_prim(state, node, name).cloned() else { return };
    if prim["bound"] == json!(true) || !matches!(prim["kind"].as_str(), Some("float" | "int")) {
        return;
    }
    let (min, max) = (prim_f32(&prim, "min").unwrap_or(0.0), prim_f32(&prim, "max").unwrap_or(1.0));
    let was = prim_f32(&prim, "value").or_else(|| prim_f32(&prim, "default")).unwrap_or(min);
    let mut step = (max - min) / 100.0;
    if prim["kind"] == "int" {
        step = step.max(1.0);
    }
    let v = (was + dir * step).clamp(min, max);
    if (v - was).abs() > f32::EPSILON {
        post_param_value(door, state, theme, node, name, was, v);
    }
}

// ---------------------------------------------------------------------------
// painting
// ---------------------------------------------------------------------------

/// Paints one `cell` primitive with the shared cell kit: text drops below
/// [`Theme::node_cell_text_zoom`], a bound cell reads disabled with its live
/// value, a scrubbing cell shows the in-flight value.
pub(super) fn paint_cell(
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
    let zoom = state.zoom;
    let text = zoom >= theme.node_cell_text_zoom;
    let mut font = theme.font_small.clone();
    font.size *= 1.4 * zoom.clamp(0.5, theme.node_text_zoom_max);
    let editing = state.param_drag.as_ref().filter(|d| d.node == node && d.name == name);
    let hover = zoom >= theme.node_cell_edit_zoom
        && painter.ctx().input(|i| i.pointer.hover_pos()).is_some_and(|h| r.contains(h));
    let bound = p["bound"] == json!(true);
    let live_value = live.param(node, name);
    let st = CellState { hover, active: editing.is_some(), disabled: bound, live: live_value.is_some() && !bound };
    let label = tr_dyn(p["label"].as_str().unwrap_or(name));
    let (min, max) = (num(p, "min"), num(p, "max"));
    match p["kind"].as_str() {
        Some(k @ ("float" | "int")) => {
            let held = editing
                .map(|d| d.value)
                .or(live_value)
                .or_else(|| p["value"].as_f64().map(|v| v as f32))
                .unwrap_or(num(p, "default"));
            let decimals = if k == "int" { 0 } else { components::cell_decimals(min, max) };
            let mut shown = components::cell_format(held, decimals);
            if bound {
                shown = format!("\u{27f5} {shown}");
            }
            let centered = min < 0.0 && max > 0.0 && (max + min).abs() < 1e-4;
            let bar = components::cell_bar(held, min, max, centered);
            components::cell_paint_float(painter, theme, r, &label, &shown, Some(bar), &font, text, st);
            if hover && !bound {
                painter.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                components::cell_paint_arrows(painter, theme, r, &font);
            }
        }
        Some("colour") => {
            let rgb = p["rgb"].as_array().map(|a| {
                let c = |i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32;
                theme.node_rgb(c(0), c(1), c(2))
            });
            match rgb {
                Some(c) => components::cell_paint_colour(painter, theme, r, &label, c, &font, text, st),
                None => components::cell_paint_text(painter, theme, r, &label, "", &font, text, st),
            }
        }
        Some("button") => components::cell_paint_text(painter, theme, r, &label, "\u{25b6}", &font, text, st),
        Some("vec") => {
            let vals: Vec<f32> = p["values"].as_array().map(|a| a.iter().filter_map(|v| v.as_f64().map(|f| f as f32)).collect()).unwrap_or_default();
            let n = vals.len().max(1);
            let gap = metal_graph::layout::CELL_GAP_H * zoom;
            let w = (r.width() - gap * (n as f32 - 1.0)) / n as f32;
            let offset = p["offset"].as_u64().unwrap_or(0) as usize;
            let of = p["vec_of"].as_str().unwrap_or_default();
            let hover_pos = painter.ctx().input(|i| i.pointer.hover_pos());
            for (i, v) in vals.iter().enumerate() {
                let cell = EguiRect::from_min_size(Pos2::new(r.left() + (w + gap) * i as f32, r.top()), Vec2::new(w, r.height()));
                let live_axis = editing.filter(|d| d.vec.as_ref().is_some_and(|vd| vd.of == of && vd.offset == offset && vd.axis == i));
                let shown = live_axis.map_or(*v, |d| d.value);
                let tag = if n == 1 { label.to_string() } else { ["X", "Y", "Z"].get(i).copied().unwrap_or("").to_string() };
                let ast = CellState { hover: hover && hover_pos.is_some_and(|h| cell.contains(h)), active: live_axis.is_some(), ..CellState::default() };
                components::cell_paint_float(painter, theme, cell, &tag, &components::cell_format(shown, 2), None, &font, text, ast);
            }
        }
        Some("bool") => {
            components::cell_paint_toggle(painter, theme, r, &label, p["on"] == json!(true), &font, text, st)
        }
        Some("enum") => {
            components::cell_paint_enum(painter, theme, r, &label, p["text"].as_str().unwrap_or_default(), &font, text, st)
        }
        Some("text") => {
            components::cell_paint_text(painter, theme, r, &label, p["text"].as_str().unwrap_or_default(), &font, text, st)
        }
        _ => components::cell_ground(painter, theme, r, st),
    }
}

/// The typed-entry box and the enum list, as real egui areas over the canvas
/// (screen space, never zoomed). Any pan or zoom closes them.
pub(super) fn paint_overlays(ui: &egui::Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    let view = (state.pan, state.zoom);
    if state.cell_edit.as_ref().is_some_and(|e| e.view != view) {
        state.cell_edit = None;
    }
    if state.cell_pick.as_ref().is_some_and(|p| p.view != view) {
        state.cell_pick = None;
    }
    if let Some(edit) = state.cell_edit.as_mut() {
        let id = Id::new("shell/region/nodes/cell-edit");
        let mut buffer = std::mem::take(&mut edit.buffer);
        let first = edit.first;
        let at = edit.at;
        let mut lost = false;
        egui::Area::new(id).fixed_pos(at.min).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
            let edit_id = id.with("text");
            let r = ui.add(
                egui::TextEdit::singleline(&mut buffer)
                    .id(edit_id)
                    .font(theme.font_small.clone())
                    .desired_width(at.width()),
            );
            if first {
                ui.memory_mut(|m| m.request_focus(edit_id));
                let ctx = ui.ctx().clone();
                if let Some(mut st) = egui::TextEdit::load_state(&ctx, edit_id) {
                    let len = buffer.chars().count();
                    st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(len),
                    )));
                    st.store(&ctx, edit_id);
                }
            }
            lost = r.lost_focus() && !first;
        });
        edit.buffer = buffer;
        edit.first = false;
        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Tab));
        let ctrl = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
        if esc {
            state.cell_edit = None;
        } else if enter || lost {
            commit_edit(door, state, theme, ctrl);
        }
    }
    if let Some(pick) = state.cell_pick.as_mut() {
        let id = Id::new("shell/region/nodes/cell-pick");
        let pos = Pos2::new(pick.at.left(), pick.at.bottom());
        let mut chosen: Option<String> = None;
        let area = egui::Area::new(id).fixed_pos(pos).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(pick.at.width());
                for o in &pick.options {
                    let label = RichText::new(tr_dyn(o)).font(theme.font_small.clone());
                    if ui.selectable_label(*o == pick.current, label).clicked() {
                        chosen = Some(o.clone());
                    }
                }
            });
        });
        let down = ui.input(|i| i.pointer.primary_down());
        let outside = ui.input(|i| i.pointer.any_click())
            && ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| !area.response.rect.contains(p));
        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
        let armed = pick.armed;
        if !down {
            pick.armed = true;
        }
        if let Some(o) = chosen {
            pick_option(door, state, theme, &o);
        } else if esc || (armed && outside) {
            state.cell_pick = None;
        }
    }
}

#[cfg(test)]
impl PanelState {
    /// Test hook: types into the open cell edit.
    pub(super) fn type_into_cell_edit(&mut self, text: &str) {
        if let Some(e) = self.cell_edit.as_mut() {
            e.buffer = text.to_string();
        }
    }
}

/// A collapsible panel's header row.
pub(super) fn paint_panel(painter: &egui::Painter, theme: &Theme, origin: EguiRect, state: &PanelState, p: &Value, drag: Option<DragOffset>) {
    if state.zoom < theme.node_cell_text_zoom {
        return;
    }
    let r = shifted_rect(origin, state, p, [num(p, "x"), num(p, "y")], drag);
    let mut font = theme.font_small.clone();
    font.size *= 1.4 * state.zoom.clamp(0.5, theme.node_text_zoom_max);
    components::cell_paint_panel_header(painter, theme, r, &tr_dyn(p["title"].as_str().unwrap_or_default()), p["open"] == json!(true), &font);
}

/// The chevron on a node's header that collapses it.
pub(super) fn paint_chevron(painter: &egui::Painter, theme: &Theme, origin: EguiRect, state: &PanelState, p: &Value, drag: Option<DragOffset>) {
    let r = shifted_rect(origin, state, p, [num(p, "x"), num(p, "y")], drag);
    let mut font = theme.font_small.clone();
    font.size *= state.zoom.clamp(0.5, theme.node_text_zoom_max);
    let glyph = if p["collapsed"] == json!(true) { "\u{25b8}" } else { "\u{25be}" };
    painter.text(r.center(), egui::Align2::CENTER_CENTER, glyph, font, theme.node_more_ink);
}
