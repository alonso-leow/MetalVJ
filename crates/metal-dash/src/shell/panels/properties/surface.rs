//! `venue.surface`'s properties (plan P4.2): which venue and which surface the
//! picture lands on, a thumbnail of the surface with the incoming device frame
//! warped onto it, and `calibrar` — the shared [`ContourEditor`] on the four
//! pins of *this device's* projector, posted as `POST /venue { op: "pins" }`.
//!
//! The device is never in the node's config (regla 4): it is read from the
//! graph id (`device-<id>`) the node belongs to, exactly as the `mask` panel
//! reads its own device.

use crate::shell::components::small_text;
use egui::{Id, RichText, Sense, Ui};
use serde_json::{json, Value};

use std::collections::{BTreeMap, HashMap};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{dropdown, kv_row, label_row, note, table};
use crate::shell::i18n::{tr, trf};

use super::contour::{ContourEditor, ContourEvent, ContourMode};
use super::shared::get;
use super::PanelState;

/// The default pin quad a surface starts with before anyone has dragged one:
/// inset from the device edges so the handles are reachable.
pub(crate) const DEFAULT_PINS: [[f32; 2]; 4] = [[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]];

/// `GET /venue`: every venue on disk, and which was projected last.
pub(crate) fn get_venue_list(door: &dyn Call) -> Option<Value> {
    get(door, "GET", "/venue", &[])
}

/// `GET /venue?id=`: one venue, its check report and its outputs.
pub(crate) fn get_venue(door: &dyn Call, id: &str) -> Option<Value> {
    get(door, "GET", "/venue", &[("id", id.to_string())])
}

/// `GET /state`'s `outputs`, for the device's own size.
pub(crate) fn get_outputs(door: &dyn Call) -> Vec<Value> {
    get(door, "GET", "/state", &[])
        .and_then(|v| v["outputs"].as_array().cloned())
        .unwrap_or_default()
}

/// Paint the `venue.surface` node's folder.
#[allow(clippy::too_many_arguments)]
pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
    state: &mut PanelState,
) {
    crate::shell::components::section_heading(ui, theme, tr("surface"));

    let device = crate::shell::collection::device_of(gid, graph, Some(node_id));
    // The room is the node's own (`venue_map`): the surface this picture is
    // warped onto belongs to the room the graph already reaches, and the
    // group node that owns it is the only one that may point it elsewhere.
    // The old dropdown over every venue on disk is gone with the venue canvas.
    let venue = crate::shell::panels::venue_map::venue_for(door, gid, node_id, node, graph);
    let venue_id = venue.id.clone();
    label_row(ui, theme, tr("room"), |ui| {
        ui.label(
            RichText::new(&venue_id)
                .font(theme.font_small.clone())
                .color(theme.ink),
        )
    });
    if venue.adopted {
        note(ui, theme, tr("this room is the one the graph already used."));
    }

    let detail = get(door, "GET", "/venue", &[("id", venue_id.clone())]);
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
    let mut surface = node["config"]["surface"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| surfaces.first().map(|s| s.0.clone()));
    let mut picked_surface = false;
    kv_row(ui, theme, tr("surface"), |ui| {
        crate::shell::components::combo_box(theme, Id::new("shell/region/properties/surface-node-surface"))
            .width(ui.available_width())
            .selected_text(
                surface.clone().unwrap_or_else(|| "—".into()),
            )
            .show_ui(ui, |ui| {
                for (id, name) in &surfaces {
                    let label = if name.is_empty() {
                        id.as_str()
                    } else {
                        name.as_str()
                    };
                    if ui
                        .selectable_label(surface.as_deref() == Some(id), label)
                        .clicked()
                        && surface.as_deref() != Some(id)
                    {
                        surface = Some(id.clone());
                        picked_surface = true;
                    }
                }
            })
            .response
    });
    if picked_surface {
        super::text::post_text(door, gid, node_id, json!({ "surface": surface }));
    }
    let Some(surface_id) = surface else {
        note(ui, theme, tr("— this venue has no surface —"));
        return;
    };

    mapping_table(ui, theme, door, gid, node_id, node, graph, &surface_id, detail.as_ref());

    let Some(device_id) = device else {
        note(ui, theme, tr("— this node is not on a device graph —"));
        return;
    };
    let outputs = get(door, "GET", "/state", &[])
        .and_then(|v| v["outputs"].as_array().cloned())
        .unwrap_or_default();
    let (out_w, out_h) = outputs
        .iter()
        .find(|o| o["id"] == json!(device_id))
        .map(|o| {
            (
                o["width"].as_u64().unwrap_or(1920) as f32,
                o["height"].as_u64().unwrap_or(1080) as f32,
            )
        })
        .unwrap_or((1920.0, 1080.0));

    let projector = detail
        .as_ref()
        .and_then(|v| v["venue"]["projectors"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .find(|p| p["output"] == json!(device_id));
    let Some(projector_id) = projector
        .as_ref()
        .and_then(|p| p["id"].as_str())
        .map(str::to_string)
    else {
        ui.label(
            RichText::new(trf(
                "— no projector drives {device} in this venue —",
                &[("device", &device_id)],
            ))
            .font(theme.font_small.clone())
            .color(theme.ink_weak),
        );
        return;
    };
    let pins = read_pins(projector.as_ref(), &surface_id);

    let key = format!("{projector_id}:{surface_id}");
    let mut calibrating = state.wall_calibrate.as_deref() == Some(key.as_str());
    let label = if calibrating {
        tr("stop calibrating")
    } else {
        tr("calibrar")
    };
    if ui
        .button(small_text(theme, label))
        .clicked()
    {
        calibrating = !calibrating;
        state.wall_calibrate = calibrating.then(|| key.clone());
    }
    if calibrating {
        calibrate_pins(
            ui,
            theme,
            door,
            state,
            &venue_id,
            &projector_id,
            &surface_id,
            &device_id,
            out_w,
            out_h,
            &pins,
        );
    } else {
        thumbnail(ui, theme, door, state, &device_id, out_w, out_h, &pins);
    }
    if let Some(msg) = state.mask_edit_note.clone() {
        note(ui, theme, &msg);
    }
}

/// The longest a signal path may run before the walk gives up on it: the
/// node count, so a cycle in a file an older build wrote cannot spin.
const PATH_STEPS: usize = 4096;
/// How many paths the walk keeps: a graph this wide is past what one table can
/// show, and the cap keeps the frame bounded.
const MAX_PATHS: usize = 64;

/// One signal path that reaches a `venue.surface`: the generator it starts at,
/// the nodes between that generator and the surface (its effects), the nodes
/// after the surface, and the output device it ends at when it reaches one.
///
/// Two paths through the same surface that differ in their nodes are two
/// rows: a generator wired straight into the surface and the same generator
/// routed through an effect first are different signals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SignalPath {
    pub gen: String,
    /// The effects between the generator and the surface, in order.
    pub fx: Vec<String>,
    /// Every node on the path, in order: the key a section is stored under.
    pub nodes: Vec<String>,
    /// The output device the path ends at, when it reaches one.
    pub device: Option<String>,
}

impl SignalPath {
    /// The key this path's section assignment is stored under in the node's
    /// `config.sections`: its node ids joined in order.
    pub(crate) fn key(&self) -> String {
        self.nodes.join(">")
    }
}

/// Every signal path from a `gen.*` node to `surface` in `graph`, each one
/// running from its generator, through `surface`, to the output device it
/// ends at (or to the end of the graph when it ends at no device).
///
/// The walk runs forward from each generator and keeps only the maximal paths
/// — ones that reach a device or stop — that pass `surface`. A generator wired
/// straight to an output device without going through the surface is not a
/// row of this surface. Bounded by [`PATH_STEPS`] and [`MAX_PATHS`].
pub(crate) fn signal_paths(graph: &Value, surface: &str) -> Vec<SignalPath> {
    let nodes = graph["graph"]["nodes"].as_array().cloned().unwrap_or_default();
    let kinds: HashMap<String, String> = nodes
        .iter()
        .filter_map(|n| Some((n["id"].as_str()?.to_string(), n["kind"].as_str()?.to_string())))
        .collect();
    let mut adj: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in graph["graph"]["edges"].as_array().into_iter().flatten() {
        if let (Some(from), Some(to)) = (e["from"]["node"].as_str(), e["to"]["node"].as_str()) {
            adj.entry(from.to_string()).or_default().push(to.to_string());
        }
    }
    let mut walks: Vec<Vec<String>> = Vec::new();
    let mut steps = 0usize;
    for n in &nodes {
        let Some(id) = n["id"].as_str() else { continue };
        if !kind_of(&kinds, id).starts_with("gen.") {
            continue;
        }
        let mut path = Vec::new();
        walk(id, &adj, &kinds, &mut path, &mut walks, &mut steps);
    }
    let mut out: Vec<SignalPath> = Vec::new();
    for path in walks {
        let Some(at) = path.iter().position(|p| p.as_str() == surface) else {
            continue;
        };
        let device = path
            .last()
            .filter(|last| kind_of(&kinds, last) == "output.device")
            .cloned();
        let fx = path[1..at].to_vec();
        let found = SignalPath {
            gen: path[0].clone(),
            fx,
            nodes: path,
            device,
        };
        if !out.iter().any(|p| p.nodes == found.nodes) {
            out.push(found);
        }
    }
    out
}

/// The depth-first walk behind [`signal_paths`]: extend `path` along each
/// edge not already on it; a node with nowhere left to go closes the path.
/// An output device is terminal.
fn kind_of<'a>(kinds: &'a HashMap<String, String>, id: &str) -> &'a str {
    kinds.get(id).map(String::as_str).unwrap_or_default()
}

fn walk(
    cur: &str,
    adj: &BTreeMap<String, Vec<String>>,
    kinds: &HashMap<String, String>,
    path: &mut Vec<String>,
    out: &mut Vec<Vec<String>>,
    steps: &mut usize,
) {
    *steps += 1;
    if *steps > PATH_STEPS || out.len() >= MAX_PATHS {
        return;
    }
    path.push(cur.to_string());
    let next: Vec<&String> = match adj.get(cur) {
        Some(to) if kind_of(kinds, cur) != "output.device" => {
            to.iter().filter(|n| !path.contains(n)).collect()
        }
        _ => Vec::new(),
    };
    if next.is_empty() {
        out.push(path.clone());
    } else {
        for n in next {
            walk(n, adj, kinds, path, out, steps);
        }
    }
    path.pop();
}

/// The sections of the room a surface's signal can be sent to: each display
/// and each surface of the venue, by id and name. A display's glass is also
/// one of the venue's surfaces, so both lists are read and de-duplicated.
fn sections_of(detail: Option<&Value>) -> Vec<(String, String)> {
    let venue = detail.and_then(|d| d.get("venue"));
    let mut out: Vec<(String, String)> = Vec::new();
    for key in ["displays", "surface"] {
        for s in venue.and_then(|v| v[key].as_array()).into_iter().flatten() {
            let Some(id) = s["id"].as_str() else { continue };
            if out.iter().any(|(o, _)| o == id) {
                continue;
            }
            let name = s["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(id);
            out.push((id.to_string(), name.to_string()));
        }
    }
    out
}

/// The section each path is sent to, read from the node's `config.sections`
/// (`path key → section id`). A path with no entry is unassigned.
fn assignments(node: &Value) -> BTreeMap<String, String> {
    node["config"]["sections"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
        .collect()
}

/// The mapping table on the surface page: one row per signal path that
/// reaches the surface, with the generator, the effects, and the output
/// device the signal lands on as read-only labels, and a dropdown naming the
/// screen section it is sent to. Changing a dropdown writes the whole
/// assignment map to the node's config (`POST /graph/node`, the route the
/// rest of this page uses).
#[allow(clippy::too_many_arguments)]
fn mapping_table(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
    surface: &str,
    detail: Option<&Value>,
) {
    let paths = signal_paths(graph, surface);
    if paths.is_empty() {
        note(ui, theme, tr("no signal reaches this surface yet."));
        return;
    }
    let sections = sections_of(detail);
    let mut chosen: Vec<String> = paths
        .iter()
        .map(|p| assignments(node).remove(&p.key()).unwrap_or_default())
        .collect();
    let before = chosen.clone();
    let options: Vec<(String, &str)> = std::iter::once((String::new(), tr("unassigned")))
        .chain(sections.iter().map(|(id, name)| (id.clone(), name.as_str())))
        .collect();
    table(
        ui,
        theme,
        ("surface-sections", node_id),
        &[tr("generator"), tr("effect"), tr("output"), tr("section")],
        paths.len(),
        |ui, row, col| {
            let p = &paths[row];
            let text = match col {
                0 => p.gen.clone(),
                1 => p.fx.join(" · "),
                2 => p.device.clone().unwrap_or_default(),
                _ => {
                    dropdown(
                        ui,
                        theme,
                        ("surface-section", node_id, row),
                        &mut chosen[row],
                        &options,
                    );
                    return;
                }
            };
            if !text.is_empty() {
                ui.label(small_text(theme, text).color(theme.ink));
            } else {
                ui.label("");
            }
        },
    );
    if chosen != before {
        let map: serde_json::Map<String, Value> = paths
            .iter()
            .zip(&chosen)
            .filter(|(_, s)| !s.is_empty())
            .map(|(p, s)| (p.key(), json!(s)))
            .collect();
        super::text::post_text(door, gid, node_id, json!({ "sections": map }));
    }
}

/// The four pins of `surface` on `projector`, or [`DEFAULT_PINS`].
pub(crate) fn read_pins(projector: Option<&Value>, surface: &str) -> [[f32; 2]; 4] {
    let from = projector
        .and_then(|p| p["pins"].get(surface))
        .and_then(|v| serde_json::from_value::<[[f32; 2]; 4]>(v.clone()).ok());
    from.unwrap_or(DEFAULT_PINS)
}

/// The device frame with the surface's pins outlined — the read-only view.
#[allow(clippy::too_many_arguments)]
fn thumbnail(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    device: &str,
    out_w: f32,
    out_h: f32,
    pins: &[[f32; 2]; 4],
) {
    let aspect = (out_w.max(1.0) / out_h.max(1.0)).clamp(0.2, 5.0);
    let width = theme.mask_canvas_width.min(ui.available_width());
    let size = egui::Vec2::new(width, width / aspect);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let query = vec![
        ("device", device.to_string()),
        ("w", (width.round() as u32).to_string()),
    ];
    let pic = state.mask_frame.picture(
        ui.ctx(),
        door,
        "/output.jpg",
        &query,
        crate::shell::frames::live_gap(),
    );
    match pic {
        Some((id, _)) => draw_warped(ui, rect, id, theme.image_tint, pins),
        None => {
            ui.painter()
                .rect_filled(rect, theme.corner_radius(), theme.tube);
        }
    }
    let pts: Vec<egui::Pos2> = pins
        .iter()
        .map(|p| rect.min + egui::vec2(p[0] * rect.width(), p[1] * rect.height()))
        .collect();
    ui.painter()
        .add(egui::Shape::closed_line(pts, theme.accent()));
}

/// `calibrar`: the shared [`ContourEditor`] on the four pins over the device
/// frame. Every drag posts the whole quad as `POST /venue { op: "pins" }`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn calibrate_pins(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    venue: &str,
    projector: &str,
    surface: &str,
    device: &str,
    out_w: f32,
    out_h: f32,
    pins: &[[f32; 2]; 4],
) {
    let aspect = (out_w.max(1.0) / out_h.max(1.0)).clamp(0.2, 5.0);
    let width = theme.mask_canvas_width.min(ui.available_width());
    let size = egui::Vec2::new(width, width / aspect);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let key = format!("{projector}:{surface}");
    state.set_mask_canvas_rect(&key, rect);

    let query = vec![
        ("device", device.to_string()),
        ("w", (width.round() as u32).to_string()),
    ];
    let pic = state.mask_frame.picture(
        ui.ctx(),
        door,
        "/output.jpg",
        &query,
        crate::shell::frames::live_gap(),
    );
    match pic {
        Some((id, _)) => draw_warped(ui, rect, id, theme.image_tint, pins),
        None => {
            ui.painter()
                .rect_filled(rect, theme.corner_radius(), theme.tube);
        }
    }

    let id = ui.id().with(("pins-contour", &key));
    let corners = state.wall_fit.entry(key).or_insert_with(|| pins.to_vec());
    let mut draft = Vec::new();
    let event = ContourEditor {
        points: corners,
        rect,
        mode: ContourMode::Edit,
        min_points: 4,
        draft: &mut draft,
        lock_count: true,
        id,
    }
    .ui(ui, theme);
    if let ContourEvent::Changed(pts) = event {
        if pts.len() == 4 {
            let quad = [
                [pts[0][0], pts[0][1]],
                [pts[1][0], pts[1][1]],
                [pts[2][0], pts[2][1]],
                [pts[3][0], pts[3][1]],
            ];
            post_pins(door, venue, projector, surface, &quad);
            *corners = pts;
            state.mask_edit_note = Some(tr("pins calibrated").to_string());
        }
    }
}

/// `POST /venue { op: "pins" }` — the one route the calibration writes.
pub(crate) fn post_pins(
    door: &dyn Call,
    venue: &str,
    projector: &str,
    surface: &str,
    pins: &[[f32; 2]; 4],
) {
    let body = json!({
        "op": "pins", "id": venue, "projector": projector, "surface": surface, "pins": pins,
    });
    let _ = door.json("POST", "/venue", Some(body));
}

/// The device frame mapped onto the pins quad: a four-vertex textured mesh
/// with the frame's `0..1` corners, so the picture reads as it will land on
/// the surface.
fn draw_warped(
    ui: &mut Ui,
    rect: egui::Rect,
    id: egui::TextureId,
    tint: egui::Color32,
    quad: &[[f32; 2]; 4],
) {
    let mut mesh = egui::Mesh::with_texture(id);
    let uvs = [(0.0f32, 0.0f32), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
    for (i, (u, v)) in uvs.iter().enumerate() {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: rect.min + egui::vec2(quad[i][0] * rect.width(), quad[i][1] * rect.height()),
            uv: egui::pos2(*u, *v),
            color: tint,
        });
    }
    mesh.indices = vec![0, 1, 2, 0, 2, 3];
    ui.painter().add(egui::Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(nodes: &[(&str, &str)], edges: &[(&str, &str)]) -> Value {
        let nodes: Vec<Value> = nodes
            .iter()
            .map(|(id, kind)| json!({ "id": id, "kind": kind }))
            .collect();
        let edges: Vec<Value> = edges
            .iter()
            .map(|(from, to)| json!({ "from": { "node": from }, "to": { "node": to } }))
            .collect();
        json!({ "graph": { "nodes": nodes, "edges": edges } })
    }

    /// A generator wired straight into the surface and the same generator
    /// routed through an effect first are two rows, with different effect
    /// columns and the same device.
    #[test]
    fn direct_and_via_fx_are_two_rows() {
        let g = graph(
            &[
                ("gen-a", "gen.solid"),
                ("fx-1", "fx.blur"),
                ("s", "venue.surface"),
                ("dev", "output.device"),
            ],
            &[("gen-a", "s"), ("gen-a", "fx-1"), ("fx-1", "s"), ("s", "dev")],
        );
        let mut paths = signal_paths(&g, "s");
        paths.sort_by_key(|p| p.fx.len());
        assert_eq!(paths.len(), 2);
        assert!(paths[0].fx.is_empty());
        assert_eq!(paths[1].fx, vec!["fx-1".to_string()]);
        assert!(paths.iter().all(|p| p.gen == "gen-a" && p.device.as_deref() == Some("dev")));
        assert_ne!(paths[0].key(), paths[1].key());
    }

    /// One generator into a surface that feeds two output devices: two rows,
    /// one per device.
    #[test]
    fn two_outputs_from_one_generator_are_two_rows() {
        let g = graph(
            &[
                ("gen-a", "gen.solid"),
                ("s", "venue.surface"),
                ("d1", "output.device"),
                ("d2", "output.device"),
            ],
            &[("gen-a", "s"), ("s", "d1"), ("s", "d2")],
        );
        let mut devices: Vec<String> = signal_paths(&g, "s")
            .into_iter()
            .filter_map(|p| p.device)
            .collect();
        devices.sort();
        assert_eq!(devices, vec!["d1".to_string(), "d2".to_string()]);
    }

    /// A generator straight onto an output device, never through the surface,
    /// is not a row of the surface; a surface with no device downstream still
    /// has its rows, with an empty device column.
    #[test]
    fn only_paths_through_the_surface_count() {
        let g = graph(
            &[
                ("gen-a", "gen.solid"),
                ("gen-b", "gen.solid"),
                ("s", "venue.surface"),
                ("dev", "output.device"),
            ],
            &[("gen-a", "dev"), ("gen-b", "s")],
        );
        let paths = signal_paths(&g, "s");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].gen, "gen-b");
        assert_eq!(paths[0].device, None);
    }

    /// A cycle in the wiring terminates and yields no endless path.
    #[test]
    fn a_cycle_terminates() {
        let g = graph(
            &[("gen-a", "gen.solid"), ("fx", "fx.blur"), ("s", "venue.surface")],
            &[("gen-a", "fx"), ("fx", "fx"), ("fx", "s"), ("s", "fx")],
        );
        let paths = signal_paths(&g, "s");
        assert!(paths.iter().all(|p| p.nodes.len() <= 3));
    }

    /// Assignments survive a round trip through the node's config map.
    #[test]
    fn assignments_read_the_config_map() {
        let node = json!({ "config": { "sections": { "gen-a>s>dev": "left" } } });
        let map = assignments(&node);
        assert_eq!(map.get("gen-a>s>dev").map(String::as_str), Some("left"));
        assert!(assignments(&json!({})).is_empty());
    }
}
