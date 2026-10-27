//! Properties for `output.device`'s venue controls (`project venue`, `look at
//! venue`) and the calibration list of every surface this projector covers
//! (plan P4.3), each with `calibrar` — the same shared [`ContourEditor`] and
//! the same `POST /venue { op: "pins" }` the `venue.surface` panel uses.
//!
//! The properties panel is a browser: a tree of folders for the selected
//! node on the left, the chosen folder's content on the right. This module
//! adds its own folders to that tree: [`folders`] names them for a node kind
//! (English source text, painted through `i18n::tr`; empty for kinds this
//! module does not own), and [`section`] paints one of them when the operator
//! opens it.
//!
//! **H5.3: the venue sensor.** A `sensor.yolo` node gets a `venue sensor`
//! folder here: pick the venue this camera watches, then place the capture
//! camera into it (`Venue.sensors`) — `POST /venue { op: "sensor" }`, the
//! route the runtime reads back. The folder is the venue editor's capture
//! half; the projector/fixture placement lives with the venue generator.
//!
//! **P4.3: the device's venue folder.** An `output.device` gets a `venue`
//! folder ([`device_section`]) — `project venue`, `look at venue`, and the
//! calibration list of every surface this projector covers.

use egui::{Id, RichText, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{kv_row, note, num_value};
use crate::shell::i18n::{tr, tr_key, trf};

use super::properties::surface;
use super::properties::PanelState;

/// What this module keeps between frames (drafts, fetched lists, notes).
#[derive(Default)]
pub struct State {}

/// The folders a `sensor.yolo` node adds from this module: the venue sensor
/// placement. A `const` so [`folders`] can return it `'static`.
const SENSOR_FOLDERS: &[&str] = &[tr_key("venue sensor")];

/// F11: the one folder a per-fixture `venue.fixture` node adds — its colour
/// source, the model it follows and how it writes the bus.
const FIXTURE_NODE_FOLDERS: &[&str] = &[tr_key("fixture control")];

/// The folders this module adds for a node of `kind`, in tree order. Each
/// name is an English literal marked with `i18n::tr_key` (its Spanish in
/// this module's locale file).
pub fn folders(kind: &str) -> &'static [&'static str] {
    if kind == "sensor.yolo" {
        return SENSOR_FOLDERS;
    }
    if kind == metal_graph::VENUE_FIXTURE_KIND {
        return FIXTURE_NODE_FOLDERS;
    }
    // The room is a node's own (2026-10-21): its `map` folder names the room
    // and where each output and fixture lands in it, so an `output.device`
    // never picks a venue from a list any more. `draw` stays: it is the
    // surface editor for the screen this node already is.
    const VENUE_FOLDERS: [&str; 3] = [
        crate::shell::i18n::tr_key("map"),
        crate::shell::i18n::tr_key("venue"),
        crate::shell::i18n::tr_key("draw"),
    ];
    if kind == "output.device" {
        return &VENUE_FOLDERS;
    }
    &[]
}

/// One labelled combo of `(id, label)` choices; returns the picked id.
fn pick_row(ui: &mut Ui, theme: &Theme, salt: &str, label: &str, current: &str, choices: &[(String, String)]) -> Option<String> {
    let shown = choices
        .iter()
        .find(|(id, _)| id == current)
        .map(|(_, l)| l.clone())
        .unwrap_or_else(|| if current.is_empty() { tr("— none —").to_string() } else { current.to_string() });
    let mut picked = None;
    kv_row(ui, theme, label, |ui| {
        crate::shell::components::combo_box(theme, Id::new(("shell/venue/sensor", salt)))
            .width(ui.available_width())
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for (id, l) in choices {
                    if ui.selectable_label(id == current, l).clicked() && id != current {
                        picked = Some(id.clone());
                    }
                }
            })
            .response
    });
    picked
}

/// `POST /graph/node {op:"config"}` on `node_id` — the one way a panel writes
/// a node's config (F7/F11 both go through it).
pub(crate) fn post_config(door: &dyn Call, gid: &str, node_id: &str, fields: Value) {
    super::properties::shared::post_node_config(door, gid, node_id, fields);
}

/// The `capture` node feeding `node_id`'s `image` port, and its `source`.
fn upstream_source(graph: &Value, node_id: &str) -> Option<String> {
    let capture = graph["graph"]["edges"]
        .as_array()?
        .iter()
        .find(|e| e["to"]["node"] == json!(node_id) && e["to"]["port"] == json!("image"))
        .and_then(|e| e["from"]["node"].as_str())?;
    graph["graph"]["nodes"]
        .as_array()?
        .iter()
        .find(|n| n["id"] == json!(capture))
        .and_then(|n| n["config"]["source"].as_str())
        .map(str::to_string)
}

/// Paint `folder` (one of [`folders`]) for `node` — the `GET /graph` node
/// object, `graph` the whole reply — and answer whether it painted anything.
#[allow(clippy::too_many_arguments)]
pub fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
    folder: &str,
    _state: &mut State,
) -> bool {
    if folder == "fixture control" && node["kind"].as_str() == Some(metal_graph::VENUE_FIXTURE_KIND) {
        fixture_node_folder(ui, theme, door, gid, node_id, node, graph);
        return true;
    }
    if folder != "venue sensor" || node["kind"].as_str() != Some("sensor.yolo") {
        return false;
    }

    let venues = door
        .json("GET", "/venue", None)
        .unwrap_or_else(|_| json!({}));
    let choices: Vec<(String, String)> = venues["venues"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            let id = v["id"].as_str()?.to_string();
            let name = v["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(&id).to_string();
            Some((id, name))
        })
        .collect();
    let current = node["config"]["venue"].as_str().unwrap_or_default().to_string();

    if let Some(pick) = pick_row(ui, theme, "venue", tr("venue"), &current, &choices) {
        post_config(door, gid, node_id, json!({ "venue": pick }));
    }
    if choices.is_empty() {
        note(ui, theme, tr("— no venues yet —"));
        return true;
    }
    if current.is_empty() {
        note(ui, theme, tr("pick the venue this camera watches."));
        return true;
    }

    // The capture camera this sensor reads, if one is wired.
    let source = upstream_source(graph, node_id);
    kv_row(ui, theme, tr("capture"), |ui| {
        ui.add(
            egui::Label::new(
                RichText::new(source.clone().unwrap_or_else(|| tr("— none —").to_string()))
                    .font(theme.font_small.clone())
                    .color(theme.ink),
            )
            .truncate(),
        )
    });
    let can_place = source.is_some();
    if ui
        .add_enabled(can_place, egui::Button::new(RichText::new(tr("place capture camera")).font(theme.font_small.clone())))
        .clicked()
    {
        let body = json!({
            "op": "sensor",
            "id": current,
            "sensor": { "id": node_id, "name": node["label"].as_str().unwrap_or(""), "capture": source },
        });
        let _ = door.json("POST", "/venue", Some(body));
    }
    if !can_place {
        note(ui, theme, tr("wire a `capture` node first."));
    }

    // What the venue already has.
    let detail = {
        let mut query = std::collections::HashMap::new();
        query.insert("id".to_string(), current.clone());
        let params = crate::call::Params { query, body: Vec::new(), tail: None };
        door.call("GET", "/venue", params)
            .filter(|reply| reply.status == 200)
            .and_then(|reply| crate::json_memo::parse(&reply.body).ok())
            .unwrap_or_default()
    };
    let sensors = detail["venue"]["sensors"].as_array().cloned().unwrap_or_default();
    ui.add_space(theme.section_gap);
    if sensors.is_empty() {
        note(ui, theme, tr("no capture camera placed in this venue yet."));
    } else {
        for s in &sensors {
            let id = s["id"].as_str().unwrap_or("?");
            let at = s["at"].as_array().map(|a| {
                format!(
                    "{:.2}, {:.2}, {:.2}",
                    a.first().and_then(Value::as_f64).unwrap_or(0.0),
                    a.get(1).and_then(Value::as_f64).unwrap_or(0.0),
                    a.get(2).and_then(Value::as_f64).unwrap_or(0.0),
                )
            }).unwrap_or_default();
            let capture = s["capture"].as_str().unwrap_or("—");
            note(ui, theme, &format!("{id} · {at} · {capture}"));
        }
    }
    true
}

/// The node's config as an object, or an empty one.
fn config_of(node: &Value) -> serde_json::Map<String, Value> {
    node.get("config").and_then(Value::as_object).cloned().unwrap_or_default()
}

/// F11: the config-selector labels a `venue.fixture` node's colour source
/// offers, as `(config value, translatable label)`.
fn colour_from_options() -> [(String, String); 3] {
    [
        ("frame".to_string(), tr("frame from picture").to_string()),
        ("model".to_string(), tr("model").to_string()),
        ("signal".to_string(), tr("signal").to_string()),
    ]
}

/// F11: the node ids in `graph` that offer a colour the fixture can follow — a
/// `color`/`palette` node, or a model whose catalog entry declares a colour
/// slot (`GET /catalog`, the same catalog the engine reads — rule 7). The
/// list is computed from the catalog, never hand-written.
pub(crate) fn colour_model_nodes(door: &dyn Call, graph: &Value) -> Vec<(String, String)> {
    let mut kinds = vec!["color".to_string(), "palette".to_string()];
    if let Ok(reply) = door.json("GET", "/catalog", None) {
        for k in reply["catalog"].as_array().into_iter().flatten() {
            let has_colour = k["params"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|p| p["kind"].as_str() == Some("Colour"));
            if has_colour {
                if let Some(kind) = k["kind"].as_str() {
                    kinds.push(kind.to_string());
                }
            }
        }
    }
    kinds.sort();
    kinds.dedup();
    graph["graph"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let id = n["id"].as_str()?;
            let kind = n["kind"].as_str()?;
            kinds.contains(&kind.to_string()).then(|| (id.to_string(), kind.to_string()))
        })
        .collect()
}

/// The node currently wired into `node_id`'s `colour` input, if any.
pub(crate) fn colour_edge_source(graph: &Value, node_id: &str) -> Option<String> {
    graph["graph"]["edges"]
        .as_array()?
        .iter()
        .find(|e| e["to"]["node"] == json!(node_id) && e["to"]["port"] == json!("colour"))
        .and_then(|e| e["from"]["node"].as_str())
        .map(str::to_string)
}

/// F11: replace whatever feeds `node_id`'s `colour` with a new edge from
/// `model` — `POST /graph/edge {op:"remove"}` then `{op:"add"}`, the same
/// routes the canvas wires by hand (rule 6). `color`/`palette` expose their
/// slots as `control.base`; a model as `control.colour0`.
pub(crate) fn wire_colour_model(door: &dyn Call, gid: &str, node_id: &str, graph: &Value, model: &str, kind: &str) {
    if let Some(edge) = graph["graph"]["edges"].as_array().into_iter().flatten().find(|e| {
        e["to"]["node"] == json!(node_id) && e["to"]["port"] == json!("colour")
    }) {
        if let Some(id) = edge["id"].as_str() {
            let _ = door.json("POST", "/graph/edge", Some(json!({ "op": "remove", "graph": gid, "edge": id })));
        }
    }
    let from_port = match kind {
        "color" => "control.rgb",
        "palette" => "control.base",
        _ => "control.colour0",
    };
    let _ = door.json(
        "POST",
        "/graph/edge",
        Some(json!({
            "op": "add", "graph": gid,
            "from": { "node": model, "port": from_port },
            "to": { "node": node_id, "port": "colour" },
        })),
    );
    post_config(door, gid, node_id, json!({ "colour_from": "model" }));
}

/// F11: the `fixture control` folder for a `venue.fixture` node — where its
/// colour comes from (`frame`/`model`/`signal`), the model node it follows
/// (wired into its `colour` input through `POST /graph/edge`, never written
/// locally), how a rig spreads a model's four slots and whether it holds its
/// own hue, and where it sits on the universe bus (through the
/// `POST /outputs {op:"dmx-bus"}` helper, rule 4 — the config is read back
/// off the node).
fn fixture_node_folder(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
) {
    use crate::shell::components;

    let config = config_of(node);
    let text = |k: &str, d: &str| config.get(k).and_then(Value::as_str).unwrap_or(d).to_string();
    let number = |k: &str, d: f32| {
        config.get(k).and_then(Value::as_f64).map(|v| v as f32).unwrap_or(d)
    };

    components::section_heading(ui, theme, tr("colour from"));
    let mut from = text("colour_from", "frame");
    let options = colour_from_options();
    let labels: Vec<(String, &str)> = options.iter().map(|(v, l)| (v.clone(), l.as_str())).collect();
    let mut picked = from.clone();
    if components::segmented(ui, theme, &mut picked, &labels).changed() && picked != from {
        post_config(door, gid, node_id, json!({ "colour_from": picked }));
        from = picked;
    }

    if from == "model" {
        let models = colour_model_nodes(door, graph);
        let current = colour_edge_source(graph, node_id);
        if models.is_empty() {
            note(ui, theme, tr("— no colour node in this graph —"));
        } else {
            kv_row(ui, theme, tr("model"), |ui| {
                let shown = current.clone().unwrap_or_else(|| tr("— none —").to_string());
                crate::shell::components::combo_box(theme, Id::new(("shell/venue/fixture-node-model", node_id)))
                    .width(ui.available_width())
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for (id, kind) in &models {
                            let label = format!("{id} · {kind}");
                            if ui.selectable_label(current.as_deref() == Some(id.as_str()), label).clicked() {
                                wire_colour_model(door, gid, node_id, graph, id, kind);
                            }
                        }
                    })
                    .response
            });
        }
    }

    components::section_heading(ui, theme, tr("spread"));
    let spread = text("spread", "slots");
    let spread_options = [("slots".to_string(), tr("one slot per fixture").to_string()), ("same".to_string(), tr("the same slot").to_string())];
    let spread_labels: Vec<(String, &str)> = spread_options.iter().map(|(v, l)| (v.clone(), l.as_str())).collect();
    let mut picked_spread = spread.clone();
    if components::segmented(ui, theme, &mut picked_spread, &spread_labels).changed() && picked_spread != spread {
        post_config(door, gid, node_id, json!({ "spread": picked_spread }));
    }

    for (key, default_max, name) in [(tr("pick"), 3.0, "pick"), (tr("offset"), 3.0, "offset")] {
        let mut v = number(name, 0.0);
        let changed = kv_row(ui, theme, key, |ui| {
            let w = ui.available_width();
            num_value(ui, theme, w, &mut v, 0.0..=default_max, "", 0, 1.0, true)
        })
        .changed();
        if changed {
            post_config(door, gid, node_id, json!({ name: v.round().clamp(0.0, 3.0) as u32 }));
        }
    }

    components::section_heading(ui, theme, tr("hue"));
    let mut lock = text("hue_lock", "free");
    let lock_options = [("free".to_string(), tr("free").to_string()), ("hold".to_string(), tr("keep hue").to_string())];
    let lock_labels: Vec<(String, &str)> = lock_options.iter().map(|(v, l)| (v.clone(), l.as_str())).collect();
    if components::segmented(ui, theme, &mut lock, &lock_labels).changed() {
        post_config(door, gid, node_id, json!({ "hue_lock": lock }));
    }

    components::section_heading(ui, theme, tr("bus"));
    // The screen the node draws on: a collection's output it feeds, or a
    // legacy `device-<id>` graph's own screen (`shell::collection::device_of`).
    let device = crate::shell::collection::device_of(gid, graph, Some(node_id));
    let mut pr = number("priority", 100.0);
    let changed = kv_row(ui, theme, tr("priority"), |ui| {
        let w = ui.available_width();
        num_value(ui, theme, w, &mut pr, 0.0..=255.0, "", 0, 1.0, true)
    })
    .changed();
    if changed {
        let mut body = json!({
            "op": "dmx-bus", "node": node_id,
            "priority": pr.round().clamp(0.0, 255.0) as u32,
        });
        if let Some(d) = &device {
            body["device"] = json!(d);
        }
        let _ = door.json("POST", "/outputs", Some(body));
    }
    let mut mode = text("mode", "htp");
    let mode_options = [
        ("htp".to_string(), tr("HTP").to_string()),
        ("input".to_string(), tr("input").to_string()),
        ("software".to_string(), tr("software").to_string()),
    ];
    let mode_labels: Vec<(String, &str)> = mode_options.iter().map(|(v, l)| (v.clone(), l.as_str())).collect();
    if components::segmented(ui, theme, &mut mode, &mode_labels).changed() {
        let mut body = json!({ "op": "dmx-bus", "node": node_id, "mode": mode });
        if let Some(d) = &device {
            body["device"] = json!(d);
        }
        let _ = door.json("POST", "/outputs", Some(body));
    }
}

/// The `output.device`'s venue folder (plan P4.3): the venue this device's
/// projector belongs to, `project venue` (posts `/venue` then `/overlay`),
/// `look at venue`, and a calibration row per surface the projector covers.
#[allow(clippy::too_many_arguments)]
pub(crate) fn device_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    _node: &Value,
    graph: &Value,
    state: &mut PanelState,
) {
    crate::shell::components::section_heading(ui, theme, tr("venue"));
    let Some(device) = crate::shell::collection::device_of(gid, graph, Some(node_id)) else {
        note(ui, theme, tr("— this node is not on a device graph —"));
        return;
    };

    // The room is the node's own (`venue_map`): no dropdown over every venue
    // on the machine. The Map folder in the same node names it, and this
    // section reads the same answer, so the two cannot disagree. A node with
    // no room of its own yet only *reads* it here (`venue_of_graph`, no
    // write): the owner node is the one that adopts a room, so opening a
    // screen's page never invents one.
    let venue_id = super::venue_map::venue_of_graph(gid, graph);

    let outputs = surface::get_outputs(door);
    let (out_w, out_h) = outputs
        .iter()
        .find(|o| o["id"] == json!(device))
        .map(|o| {
            (
                o["width"].as_u64().unwrap_or(1920) as f32,
                o["height"].as_u64().unwrap_or(1080) as f32,
            )
        })
        .unwrap_or((1920.0, 1080.0));

    // The two verbs first: they are the section's reason for existing, and a
    // calibration table under them must not push them off a short panel.
    if ui
        .button(RichText::new(tr("project venue")).font(theme.font_small.clone()))
        .clicked()
    {
        project_venue(door, state, &venue_id, &device);
    }
    if ui
        .button(RichText::new(tr("look at venue")).font(theme.font_small.clone()))
        .clicked()
    {
        look_at_venue(door, state, &venue_id);
    }

    let detail = surface::get_venue(door, &venue_id);
    let projector = detail
        .as_ref()
        .and_then(|v| v["venue"]["projectors"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .find(|p| p["output"] == json!(device));
    let Some(projector_id) = projector.as_ref().and_then(|p| p["id"].as_str()).map(str::to_string) else {
        note(ui, theme, &trf("— no projector drives {device} in this venue —", &[("device", &device)]));
        return;
    };
    ui.add_space(theme.section_gap);
    crate::shell::components::section_heading(ui, theme, tr("calibration"));
    let surfaces: Vec<(String, String)> = detail
        .as_ref()
        .and_then(|v| v["venue"]["surfaces"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| {
            Some((
                s["id"].as_str()?.to_string(),
                s["name"].as_str().unwrap_or_default().to_string(),
            ))
        })
        .collect();
    if surfaces.is_empty() {
        note(ui, theme, tr("— this venue has no surface —"));
    }
    for (surface_id, name) in &surfaces {
        let pins = surface::read_pins(projector.as_ref(), surface_id);
        let key = format!("{projector_id}:{surface_id}");
        let mut calibrating = state.wall_calibrate.as_deref() == Some(key.as_str());
        let title = if name.is_empty() { surface_id.as_str() } else { name.as_str() };
        let clicked = kv_row(ui, theme, title, |ui| {
            let label = if calibrating { tr("stop calibrating") } else { tr("calibrar") };
            ui.button(RichText::new(label).font(theme.font_small.clone()))
        })
        .clicked();
        if clicked {
            calibrating = !calibrating;
            state.wall_calibrate = calibrating.then(|| key.clone());
        }
        if calibrating {
            surface::calibrate_pins(
                ui, theme, door, state, &venue_id, &projector_id, surface_id, &device, out_w, out_h, &pins,
            );
        }
    }
    if let Some(note) = state.mask_edit_note.clone() {
        crate::shell::components::note(ui, theme, &note);
    }
}

/// `project venue`: `POST /venue { op: "project" }` writes the venue's masks
/// and fixtures onto this device, then `POST /overlay { op: "solid" }` fills
/// the frame so the projection is visible, exactly as the `project_venue`
/// tool's two routes.
fn project_venue(door: &dyn Call, state: &mut PanelState, venue: &str, device: &str) {
    let body = json!({ "op": "project", "id": venue, "device": device });
    let reply = match door.json("POST", "/venue", Some(body)) {
        Ok(reply) => reply,
        Err((_, msg)) => {
            state.mask_edit_note = Some(msg);
            return;
        }
    };
    let solid = reply["devices"]
        .as_array()
        .and_then(|ds| ds.iter().find(|d| d["device"] == json!(device)))
        .and_then(|d| d["solid"].as_object().cloned())
        .filter(|m| !m.is_empty());
    let mut body = json!({ "op": "solid", "device": device, "duration_seconds": 30.0 });
    match solid {
        Some(masks) => body["masks"] = json!(masks),
        None => body["rgb"] = json!([255, 255, 255]),
    }
    state.mask_edit_note = match door.json("POST", "/overlay", Some(body)) {
        Ok(_) => Some(tr("venue projected").to_string()),
        Err((_, msg)) => Some(msg),
    };
}

/// `look at venue`: `GET /venue?id=` and the check report's counts.
fn look_at_venue(door: &dyn Call, state: &mut PanelState, venue: &str) {
    let reply = surface::get_venue(door, venue);
    state.mask_edit_note = match reply {
        Some(v) => {
            let errors = v["report"]["problems"]
                .as_array()
                .map(|ps| ps.iter().filter(|p| p["level"] == "error").count())
                .unwrap_or(0);
            let warnings = v["report"]["problems"]
                .as_array()
                .map(|ps| ps.iter().filter(|p| p["level"] == "warning").count())
                .unwrap_or(0);
            let (es, ws) = (errors.to_string(), warnings.to_string());
            Some(trf("{errors} error(s), {warnings} warning(s)", &[("errors", &es), ("warnings", &ws)]))
        }
        None => Some(tr("— could not read the venue —").to_string()),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    use crate::shell::FakeDoor;

    /// F11: the model picker lists the graph's colour-producing nodes — a
    /// model whose catalog entry declares a `Colour` param, plus `color` and
    /// `palette` — and nothing else.
    #[test]
    fn the_model_picker_lists_the_graphs_colour_nodes() {
        let door = Rc::new(FakeDoor::new());
        door.respond(
            "GET",
            "/catalog",
            json!({ "catalog": [
                { "kind": "gen.aurora", "params": [{ "kind": "Colour" }] },
                { "kind": "fx.blur", "params": [{ "kind": "Float" }] },
                { "kind": "color", "params": [] },
            ] }),
        );
        let graph = json!({ "graph": { "id": "g", "nodes": [
            { "id": "a", "kind": "gen.aurora" },
            { "id": "b", "kind": "fx.blur" },
            { "id": "c", "kind": "color" },
        ], "edges": [] } });
        assert_eq!(
            colour_model_nodes(door.as_ref(), &graph),
            vec![
                ("a".to_string(), "gen.aurora".to_string()),
                ("c".to_string(), "color".to_string()),
            ]
        );
    }

    /// F11: the picker reads back which node feeds the node's `colour` input.
    #[test]
    fn the_colour_edge_source_is_the_wired_model() {
        let graph = json!({ "graph": { "nodes": [], "edges": [
            { "id": "e1", "from": { "node": "aurora", "port": "control.colour0" },
              "to": { "node": "f-0", "port": "colour" } },
            { "id": "e2", "from": { "node": "other", "port": "control" },
              "to": { "node": "f-0", "port": "image" } },
        ] } });
        assert_eq!(colour_edge_source(&graph, "f-0"), Some("aurora".to_string()));
        assert_eq!(colour_edge_source(&graph, "f-9"), None);
    }
}
