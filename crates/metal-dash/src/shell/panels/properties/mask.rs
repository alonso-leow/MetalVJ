//! Masks (`S.F`/`O.1`/`O.29`, relocated from `panels::lanes` 22-sept-2026).
//! R22.6: the real mask lives on the `mask` node between the chain and the
//! device — this is where the editor moved to, so an install with no `outputs[]`
//! at all still has somewhere to draw a contour.

use crate::shell::components::small_text;
use egui::{Id, Ui};
use metal_notation::color::Hsb;
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{colour_row, kv_row, kv_wrap, note, num_value};
use crate::shell::i18n::{tr, trf};

use super::contour::{ContourEditor, ContourEvent, ContourMode};
use super::output_device::create_output_controls;
use super::shared::get;
use super::PanelState;

/// `crate::output::BoolOp`'s three, in its serde spelling, plus the "no
/// compose — this mask is its own polygon" state a mask is born in.
pub(crate) const COMPOSE_OPS: [&str; 4] = [NO_COMPOSE, "union", "intersect", "subtract"];
pub(crate) const NO_COMPOSE: &str = "polygon";
pub(crate) const NO_PARENT: &str = "— none —";
/// Upper bound of the mask editor's "stroke" track, pinned to
/// `metal_perform::output::MASK_STROKE_MAX`.
pub(crate) const MASK_STROKE_MAX: f32 = metal_perform::output::MASK_STROKE_MAX;
/// The fewest vertices a closed mask may keep — `output::Scanline` needs a
/// triangle.
pub(crate) const MIN_MASK_POINTS: usize = 3;


/// R22.6: the line `output_device::section` shows in place of a mask section —
/// where the editor went, and the one button that puts a `mask` node between the
/// chain and the device.
pub(crate) fn masks_note_and_insert(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    gid: &str,
    node_id: &str,
    device_id: &str,
) {
    ui.separator();
    crate::shell::components::section_heading(ui, theme, tr("masks"));
    note(ui, theme, tr("masks are edited on the Mask node"));
    kv_wrap(ui, theme, "", |ui| {
        if ui.button(small_text(theme, tr("+ mask node"))).clicked() {
            insert_mask_node(door, state, gid, node_id, device_id);
        }
    });
    if let Some(msg) = state.mask_note.clone() {
        note(ui, theme, &msg);
    }
}

/// Wires `mask` in front of the device: `image → mask → output.device`, by
/// re-pointing whichever edge already fed the output through the new node.
fn insert_mask_node(door: &dyn Call, state: &mut PanelState, gid: &str, output_node: &str, device_id: &str) {
    let add = json!({ "op": "add", "graph": gid, "kind": "mask", "pos": [200.0, 0.0] });
    let node_id = match door.json("POST", "/graph/node", Some(add)) {
        Ok(reply) => match reply["node"].as_str() {
            Some(id) => id.to_string(),
            None => {
                state.mask_note = Some(tr("the graph did not return the mask node").into());
                return;
            }
        },
        Err((_, msg)) => {
            state.mask_note = Some(msg);
            return;
        }
    };
    super::text::post_text(door, gid, node_id.as_str(), json!({ "device": device_id, "mask": Value::Null }));

    let graph = get(door, "GET", "/graph", &[("id", gid.to_string())]);
    let incoming: Vec<(String, String, String)> = graph
        .as_ref()
        .and_then(|g| g["graph"]["edges"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter(|e| e["to"]["node"] == json!(output_node) && e["to"]["port"] == json!("image"))
        .filter_map(|e| {
            Some((
                e["id"].as_str()?.to_string(),
                e["from"]["node"].as_str()?.to_string(),
                e["from"]["port"].as_str().unwrap_or("image").to_string(),
            ))
        })
        .collect();
    for (edge, from_node, from_port) in &incoming {
        let _ = door.json("POST", "/graph/edge", Some(json!({ "op": "remove", "graph": gid, "edge": edge })));
        let _ = door.json(
            "POST",
            "/graph/edge",
            Some(json!({ "op": "add", "graph": gid,
                         "from": { "node": from_node, "port": from_port },
                         "to": { "node": node_id, "port": "image" } })),
        );
    }
    let _ = door.json(
        "POST",
        "/graph/edge",
        Some(json!({ "op": "add", "graph": gid,
                     "from": { "node": node_id, "port": "image" },
                     "to": { "node": output_node, "port": "image" } })),
    );
    state.mask_note = Some(trf("mask node inserted before {device}", &[("device", &device_id)]));
}

/// R22.6: the `mask` node's own properties — where the contour editor lives
/// now. The node names which output it cuts for (`config.device`) and which
/// contour of it (`config.mask`).
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
    crate::shell::components::section_heading(ui, theme, tr("mask"));

    let outputs = get(door, "GET", "/state", &[])
        .and_then(|v| v["outputs"].as_array().cloned())
        .unwrap_or_default();
    let output_ids: Vec<String> = outputs.iter().filter_map(|o| o["id"].as_str().map(str::to_string)).collect();
    let default_device = crate::shell::collection::device_of(gid, graph, None);
    let mut device = node["config"]["device"]
        .as_str()
        .filter(|d| !d.is_empty())
        .map(str::to_string);
    let mut device_picked = false;

    kv_row(ui, theme, tr("output"), |ui| {
        crate::shell::components::combo_box(theme, Id::new("shell/region/properties/mask-node-device"))
            .width(theme.control_width)
            .selected_text((device.clone().unwrap_or_else(|| "—".into())).to_string())
            .show_ui(ui, |ui| {
                for oid in &output_ids {
                    if ui.selectable_label(device.as_deref() == Some(oid.as_str()), oid).clicked() {
                        device = Some(oid.clone());
                        device_picked = true;
                    }
                }
            })
            .response
    });
    if device_picked {
        super::text::post_text(door, gid, node_id, json!({ "device": device.clone() }));
    } else if device.is_none() {
        if let Some(d) = default_device.filter(|d| output_ids.iter().any(|o| o == d)) {
            device = Some(d.clone());
            super::text::post_text(door, gid, node_id, json!({ "device": d }));
        }
    }

    let Some(device_id) = device.clone() else {
        note(ui, theme, tr("— pick an output: the preview shows its own —"));
        create_output_controls(ui, theme, door, state, None);
        return;
    };

    let Some(output) = outputs.iter().find(|o| o["id"] == json!(device_id)).cloned() else {
        create_output_controls(ui, theme, door, state, Some(&device_id));
        return;
    };
    let masks = output["masks"].as_array().cloned().unwrap_or_default();
    let (out_w, out_h) = (
        output["width"].as_u64().unwrap_or(1920) as f32,
        output["height"].as_u64().unwrap_or(1080) as f32,
    );

    // R22.6 / plan P4.1: `mask` is always flat — it decides *what* part of
    // the source is kept, in source pixels. Where the picture lands is the
    // `venue.surface` node's job now, so the old `plano / pared` combo and its
    // wall calibration are gone from here.
    let configured = node["config"]["mask"].as_str().filter(|m| !m.is_empty()).map(str::to_string);
    let mut chosen = state
        .mask_tab
        .clone()
        .filter(|m| masks.iter().any(|x| x["id"] == json!(m)))
        .or_else(|| configured.clone());
    let mut picked_now = false;
    let picked_text = chosen
        .as_deref()
        .and_then(|mid| {
            masks.iter().find(|m| m["id"] == json!(mid)).map(|m| {
                let name = m["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(mid);
                format!("{mid} · {name}")
            })
        })
        .unwrap_or_else(|| "—".into());
    kv_row(ui, theme, tr("mask"), |ui| {
        let r = crate::shell::components::combo_box(theme, Id::new("shell/region/properties/mask-node-mask"))
            .width(theme.control_width)
            .selected_text((picked_text).to_string())
            .show_ui(ui, |ui| {
                for m in &masks {
                    let Some(mid) = m["id"].as_str() else { continue };
                    let name = m["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(mid);
                    if ui.selectable_label(chosen.as_deref() == Some(mid), name).clicked() {
                        chosen = Some(mid.to_string());
                        picked_now = true;
                    }
                }
            })
            .response;
        if ui.button(small_text(theme, tr("+ new"))).clicked() {
            let id = format!("mask-{}", masks.len() + 1);
            let body = json!({ "op": "mask", "id": device_id, "mask": id, "name": id });
            state.mask_note = match door.json("POST", "/outputs", Some(body)) {
                Ok(_) => Some(trf("mask created: {id}", &[("id", &id)])),
                Err((_, msg)) => Some(msg),
            };
            // A fresh mask has no points, so nothing would ever light it or
            // project it: `+ new` drops straight into draw mode (`O.1`).
            state.mask_tab = Some(id.clone());
            state.mask_draft.clear();
            state.mask_draw = Some(id);
        }
        r
    });
    if picked_now {
        super::text::post_text(door, gid, node_id, json!({ "mask": chosen.clone() }));
        state.mask_tab = chosen.clone();
    }

    kv_row(ui, theme, tr("rows"), |ui| {
        let mut rows = state.mask_rows as f32;
        let w = ui.available_width();
        let r = num_value(ui, theme, w, &mut rows, 1.0..=32.0, "", 0, 1.0, true);
        if r.changed() {
            state.mask_rows = rows.round().clamp(1.0, 32.0) as i64;
        }
        r
    });
    kv_row(ui, theme, tr("columns"), |ui| {
        let mut cols = state.mask_cols as f32;
        let w = ui.available_width();
        let r = num_value(ui, theme, w, &mut cols, 1.0..=32.0, "", 0, 1.0, true);
        if r.changed() {
            state.mask_cols = cols.round().clamp(1.0, 32.0) as i64;
        }
        r
    });
    kv_wrap(ui, theme, "", |ui| {
        if ui.button(small_text(theme, tr("+ grid"))).clicked() {
            let body = json!({ "device_id": device_id, "rows": state.mask_rows, "cols": state.mask_cols });
            state.mask_note = match door.json("POST", "/mask/grid", Some(body)) {
                Ok(reply) => {
                    let n = reply["mask_ids"].as_array().map(Vec::len).unwrap_or(0);
                    Some(trf("{n} cell(s) created", &[("n", &n)]))
                }
                Err((_, msg)) => Some(msg),
            };
        }
    });
    if let Some(msg) = state.mask_note.clone() {
        note(ui, theme, &msg);
    }

    if state
        .mask_tab
        .as_deref()
        .filter(|m| masks.iter().any(|x| x["id"] == json!(m)))
        .is_none()
    {
        state.mask_tab = chosen.clone();
    }
    let _ = graph;
    editor(ui, theme, door, state, &device_id, &masks, out_w, out_h);
}

/// One row group per mask on `device`, each control posting **only its own
/// key** through `POST /outputs { op: "mask", id, mask, <key> }`.
pub(crate) fn editor(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    device: &str,
    masks: &[Value],
    out_w: f32,
    out_h: f32,
) {
    let ids: Vec<String> = masks.iter().filter_map(|m| m["id"].as_str().map(str::to_string)).collect();
    let target = state.mask_tab.clone().filter(|t| ids.contains(t)).or_else(|| ids.first().cloned());
    if let Some(target_id) = target {
        let m = masks.iter().find(|m| m["id"].as_str() == Some(target_id.as_str())).expect("target_id came from masks");
        let mid = target_id;
        let name = m["name"].as_str().unwrap_or_default().to_string();
        let parent = m["parent"].as_str().unwrap_or(NO_PARENT).to_string();
        let mut alpha = m["alpha"].as_f64().unwrap_or(1.0) as f32;
        let op = m["compose"]["op"].as_str().unwrap_or(NO_COMPOSE).to_string();
        let a = m["compose"]["a"].as_str().unwrap_or_default().to_string();
        let b = m["compose"]["b"].as_str().unwrap_or_default().to_string();
        let stroke = m.get("stroke").filter(|s| !s.is_null());
        let mut swidth = stroke.and_then(|s| s["width"].as_f64()).unwrap_or(0.0) as f32;
        let [sred, sgreen, sblue, mut salpha] = stroke
            .and_then(|s| s["color"].as_array())
            .map(|c| {
                let ch = |i: usize| c.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32;
                [ch(0), ch(1), ch(2), ch(3)]
            })
            .unwrap_or([1.0, 1.0, 1.0, 1.0]);
        let mut shsb = Hsb::from_rgb(sred, sgreen, sblue);
        let others: Vec<&String> = ids.iter().filter(|o| **o != mid).collect();

        crate::shell::components::field_row(ui, theme, &mid, &name);
        kv_wrap(ui, theme, "", |ui| {
            if ui.button(small_text(theme, tr("remove mask"))).clicked() {
                post_mask(door, state, device, &mid, json!({}));
            }
        });
        if m["compose"].is_null() {
            let points: Vec<[f32; 2]> = m["points"]
                .as_array()
                .map(|raw| {
                    raw.iter()
                        .filter_map(|p| {
                            let x = p.get(0)?.as_f64()? as f32;
                            let y = p.get(1)?.as_f64()? as f32;
                            Some([x, y])
                        })
                        .collect()
                })
                .unwrap_or_default();
            let changed = mask_canvas(ui, theme, door, state, device, &mid, &points, out_w, out_h);

            // `ver en el proyector` (`O.1` step 4): with the toggle on, the
            // mask goes to the monitor (`POST /mask/project`); turning it off
            // — or the editor closing — posts with no `mask_id`, clearing it.
            let mut projecting = state.mask_project.as_deref() == Some(mid.as_str());
            let toggled = ui
                .checkbox(&mut projecting, small_text(theme, tr("show on projector")))
                .changed();
            if toggled {
                let want = if projecting { Some(mid.clone()) } else { None };
                post_project(door, device, want.as_deref());
                state.mask_project = want;
            } else if projecting && changed {
                post_project(door, device, Some(&mid));
            }
        }
        kv_row(ui, theme, tr("alpha"), |ui| {
            let w = ui.available_width();
            let r = num_value(ui, theme, w, &mut alpha, 0.0..=1.0, "", 2, 1.0, true);
            if r.changed() {
                post_mask(door, state, device, &mid, json!({ "alpha": alpha }));
            }
            r
        });
        kv_row(ui, theme, tr("inside"), |ui| {
            crate::shell::components::combo_box(theme, Id::new(("shell/region/properties/mask-parent", &mid)))
                .width(theme.control_width)
                .selected_text((parent.as_str()).to_string())
                .show_ui(ui, |ui| {
                    if ui.selectable_label(parent == NO_PARENT, NO_PARENT).clicked() && parent != NO_PARENT {
                        post_mask(door, state, device, &mid, json!({ "parent": Value::Null }));
                    }
                    for o in &others {
                        if ui.selectable_label(parent == **o, o.as_str()).clicked() && parent != **o {
                            post_mask(door, state, device, &mid, json!({ "parent": o }));
                        }
                    }
                })
                .response
        });
        let mut width_changed = false;
        let mut color_changed = false;
        let mut alpha_changed = false;
        kv_row(ui, theme, tr("stroke"), |ui| {
            let w = ui.available_width();
            let r = num_value(ui, theme, w, &mut swidth, 0.0..=MASK_STROKE_MAX, "", 0, 0.0, true);
            width_changed = r.changed();
            r
        });
        color_changed = colour_row(ui, theme, tr("colour"), &mut shsb).changed();
        kv_row(ui, theme, tr("alpha"), |ui| {
            let w = ui.available_width();
            let r = num_value(ui, theme, w, &mut salpha, 0.0..=1.0, "", 2, 1.0, true);
            alpha_changed = r.changed();
            r
        });
        if width_changed || color_changed || alpha_changed {
            let [r, g, b] = shsb.to_rgb();
            post_mask(
                door,
                state,
                device,
                &mid,
                json!({ "stroke_width": swidth, "stroke_color": [r, g, b, salpha] }),
            );
        }
        note(ui, theme, tr("— or wire a source to the node's stroke port —"));
        kv_row(ui, theme, tr("shape"), |ui| {
            let can_compose = others.len() >= 2;
            ui.add_enabled_ui(can_compose || op != NO_COMPOSE, |ui| {
                crate::shell::components::combo_box(theme, Id::new(("shell/region/properties/mask-compose", &mid)))
                    .width(theme.control_width)
                    .selected_text((op.as_str()).to_string())
                    .show_ui(ui, |ui| {
                        for candidate in COMPOSE_OPS {
                            if ui.selectable_label(op == candidate, candidate).clicked() && op != candidate {
                                if candidate == NO_COMPOSE {
                                    post_mask(door, state, device, &mid, json!({ "compose": Value::Null }));
                                } else {
                                    let operands = if a.is_empty() || b.is_empty() {
                                        others.get(0).zip(others.get(1)).map(|(x, y)| ((*x).clone(), (*y).clone()))
                                    } else {
                                        Some((a.clone(), b.clone()))
                                    };
                                    if let Some((a0, b0)) = operands {
                                        post_mask(
                                            door,
                                            state,
                                            device,
                                            &mid,
                                            json!({ "compose": { "op": candidate, "a": a0, "b": b0 } }),
                                        );
                                    }
                                }
                            }
                        }
                    })
                    .response
            })
            .inner
        });
        if op != NO_COMPOSE {
            for (which, current) in [("a", &a), ("b", &b)] {
                kv_row(ui, theme, which, |ui| {
                    crate::shell::components::combo_box(theme, Id::new(("shell/region/properties/mask-operand", &mid, which)))
                        .width(theme.control_width)
                        .selected_text((current.as_str()).to_string())
                        .show_ui(ui, |ui| {
                            for o in &others {
                                if ui.selectable_label(*current == **o, o.as_str()).clicked() && *current != **o {
                                    let (a1, b1) = if which == "a" { (o.to_string(), b.clone()) } else { (a.clone(), o.to_string()) };
                                    post_mask(door, state, device, &mid, json!({ "compose": { "op": op, "a": a1, "b": b1 } }));
                                }
                            }
                        })
                        .response
                });
            }
        }
    }
    if let Some(msg) = state.mask_edit_note.clone() {
        note(ui, theme, &msg);
    }
}

/// `POST /outputs { op: "mask", id: device, mask, ..fields }` — the one writer
/// of a mask, with the route's refusal shown under the editor rather than lost.
pub(crate) fn post_mask(door: &dyn Call, state: &mut PanelState, device: &str, mask: &str, mut fields: Value) {
    let body = fields.as_object_mut().expect("json!({...}) is always an object");
    body.insert("op".to_string(), json!("mask"));
    body.insert("id".to_string(), json!(device));
    body.insert("mask".to_string(), json!(mask));
    state.mask_edit_note = match door.json("POST", "/outputs", Some(Value::Object(body.clone()))) {
        Ok(_) => None,
        Err((_, msg)) => Some(msg),
    };
}

/// `POST /mask/project { device_id, mask_id? }` — the projector toggle's one
/// route. `None` clears the projection (the route accepts a missing id).
pub(crate) fn post_project(door: &dyn Call, device: &str, mask: Option<&str>) {
    let body = json!({ "device_id": device, "mask_id": mask });
    let params = crate::call::Params {
        query: Default::default(),
        body: body.to_string().into_bytes(),
        tail: None,
    };
    let _ = door.call("POST", "/mask/project", params);
}

/// The editor over the device preview (`O.1` step 3): the frame from
/// `/output.jpg?device=` is painted in the canvas rect, then the shared
/// [`ContourEditor`] draws the contour on top in the device's `0..1` space.
/// Every vertex change is posted as the *whole* points array in one
/// `POST /outputs { op: "mask" }`. Returns whether the points changed this
/// frame, so the projector toggle can re-project.
#[allow(clippy::too_many_arguments)]
fn mask_canvas(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    device: &str,
    mask_id: &str,
    points: &[[f32; 2]],
    out_w: f32,
    out_h: f32,
) -> bool {
    let aspect = (out_w.max(1.0) / out_h.max(1.0)).clamp(0.2, 5.0);
    let width = theme.mask_canvas_width.min(ui.available_width());
    let size = egui::Vec2::new(width, width / aspect);
    let (rect, _response) = ui.allocate_exact_size(size, egui::Sense::hover());
    state.set_mask_canvas_rect(mask_id, rect);

    // The output's own composed frame under the contour — `Screen`-space
    // `0..1` on both, so a contour can be lined up with the projector.
    let query = vec![
        ("device", device.to_string()),
        ("w", (width.round() as u32).to_string()),
    ];
    let pic = state
        .mask_frame
        .picture(ui.ctx(), door, "/output.jpg", &query, crate::shell::frames::live_gap());
    match pic {
        Some((id, _)) => {
            let full = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0));
            ui.painter().image(id, rect, full, theme.image_tint);
        }
        None => {
            ui.painter().rect_filled(rect, theme.corner_radius(), theme.tube);
        }
    }

    // A draft belongs to one mask; switching clears it.
    if state.mask_draft_for.as_deref() != Some(mask_id) {
        state.mask_draft.clear();
        state.mask_draft_for = Some(mask_id.to_string());
    }
    let drawing = points.is_empty() || state.mask_draw.as_deref() == Some(mask_id);
    let mode = if drawing { ContourMode::Draw } else { ContourMode::Edit };
    let id = ui.id().with(("mask-contour", mask_id));
    let event = ContourEditor {
        points,
        rect,
        mode,
        min_points: MIN_MASK_POINTS,
        draft: &mut state.mask_draft,
        lock_count: false,
        id,
    }
    .ui(ui, theme);

    match &event {
        ContourEvent::Changed(pts) => {
            post_mask(door, state, device, mask_id, json!({ "points": pts }));
        }
        ContourEvent::Closed(pts) => {
            post_mask(door, state, device, mask_id, json!({ "points": pts }));
            state.mask_draw = None;
        }
        ContourEvent::Cancelled => {
            state.mask_draw = None;
        }
        ContourEvent::None => {}
    }
    matches!(event, ContourEvent::Changed(_) | ContourEvent::Closed(_))
}

