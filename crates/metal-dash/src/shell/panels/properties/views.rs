//! The **Views** folder of a `gen.*` node (`config.views`, see
//! `metal_graph::views`): the list of perspectives, each with a camera source
//! and its own edits, the shared `amount`, and the `vary` spreads.
//!
//! Every write posts the whole `views` object through `op: "config"` (which
//! replaces the top-level key), so a partial edit can never drop a sibling.
//! `add` and `set` maps are shown read-only: v1 edits them in the file only.

use egui::{Id, Ui};
use serde_json::{json, Map, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{dropdown_fill, kv_wrap, number_row, payload_button, small_text, subtitle};
use crate::shell::i18n::{tr, trf};

use super::text::post_text;

/// One view per projector of `venue` (a `GET /venue` reply's `venue` object):
/// `[{name: <projector id>, camera: {projector: <id>}}]`. Returns the new
/// `list` array; the caller replaces the node's list with it.
pub(crate) fn views_for_projectors(venue: &Value) -> Value {
    let list = venue["projectors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["id"].as_str())
        .map(|id| json!({ "name": id, "camera": { "projector": id } }))
        .collect();
    Value::Array(list)
}

/// The output ports a gen node's views add on the canvas: `view.<name>` per
/// view, in list order.
pub(crate) fn view_ports(config: &Value) -> Vec<String> {
    metal_graph::views::views_of(config)
        .map(|v| v.list.into_iter().map(|g| format!("{}{}", metal_graph::views::VIEW_PORT_PREFIX, g.name)).collect())
        .unwrap_or_default()
}

/// The dropdown key of a view's camera: `none`, `manual`, `p:<projector>` or
/// `s:<surface>`.
fn camera_key(cam: &Value) -> String {
    if let Some(id) = cam["projector"].as_str() {
        format!("p:{id}")
    } else if let Some(id) = cam["surface"].as_str() {
        format!("s:{id}")
    } else if cam.get("eye").is_some() {
        "manual".to_string()
    } else {
        "none".to_string()
    }
}

/// The camera a dropdown key stands for, keeping a surface's viewer. `None`
/// removes the camera.
fn camera_for(key: &str, old: &Value) -> Option<Value> {
    if key == "manual" {
        return Some(json!({ "eye": [0.0, 1.6, 4.0], "target": [0.0, 0.0, 0.0], "fov": 50.0 }));
    }
    if let Some(id) = key.strip_prefix("p:") {
        return Some(json!({ "projector": id }));
    }
    if let Some(id) = key.strip_prefix("s:") {
        let mut cam = json!({ "surface": id });
        if let Some(viewer) = old.get("viewer") {
            cam["viewer"] = viewer.clone();
        }
        return Some(cam);
    }
    None
}

/// A JSON number as `f32`, or `fallback` when it is not one.
fn num(v: &Value, fallback: f32) -> f32 {
    v.as_f64().map(|f| f as f32).unwrap_or(fallback)
}

/// A three-number JSON array as `[f32; 3]`, or `fallback` when short.
fn xyz(v: &Value, fallback: [f32; 3]) -> [f32; 3] {
    let mut out = fallback;
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = num(&v[i], fallback[i]);
    }
    out
}

/// Paints the Views folder of a `gen.*` node. `venue` is the node's venue
/// (`GET /venue`'s `venue` object) when it has one; `float_names` are the
/// node's float params, the choices of a `vary` row.
#[allow(clippy::too_many_arguments)]
pub(crate) fn views_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    venue: Option<&Value>,
    float_names: &[String],
) {
    let mut next: Map<String, Value> = node["config"]["views"].as_object().cloned().unwrap_or_default();
    let mut list: Vec<Value> = next.get("list").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut vary: Vec<Value> = next.get("vary").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut changed = false;

    subtitle(ui, theme, tr("Views"));
    let ports = view_ports(&node["config"]);
    if !ports.is_empty() {
        ui.label(small_text(theme, trf("outputs: {ports}", &[("ports", &ports.join(", "))])));
    }

    let mut remove_view: Option<usize> = None;
    for i in 0..list.len() {
        let current = list[i]["name"].as_str().unwrap_or("").to_string();
        let draft_id = Id::new(("view-name", gid, node_id, i));
        let mut draft = ui.data_mut(|d| d.get_temp::<String>(draft_id)).unwrap_or_else(|| current.clone());
        let mut lost_focus = false;
        kv_wrap(ui, theme, tr("name"), |ui| {
            lost_focus = ui.add(egui::TextEdit::singleline(&mut draft).desired_width(f32::INFINITY)).lost_focus();
        });
        let taken = |name: &str| {
            let name = name.trim();
            name.is_empty() || list.iter().enumerate().any(|(j, v)| j != i && v["name"].as_str() == Some(name))
        };
        if lost_focus {
            ui.data_mut(|d| d.remove::<String>(draft_id));
            if !taken(&draft) && draft.trim() != current {
                list[i]["name"] = json!(draft.trim());
                changed = true;
            }
        } else {
            ui.data_mut(|d| d.insert_temp(draft_id, draft.clone()));
            if taken(&draft) {
                ui.label(small_text(theme, tr("a view needs a unique name")));
            }
        }

        // Camera source.
        let cam = list[i]["camera"].clone();
        let mut key = camera_key(&cam);
        let mut labels: Vec<(String, String)> = vec![
            ("none".into(), tr("None").into()),
            ("manual".into(), tr("Manual").into()),
        ];
        for p in venue.and_then(|v| v["projectors"].as_array()).into_iter().flatten() {
            if let Some(id) = p["id"].as_str() {
                labels.push((format!("p:{id}"), format!("projector {id}")));
            }
        }
        for s in venue.and_then(|v| v["surfaces"].as_array()).into_iter().flatten() {
            if let Some(id) = s["id"].as_str() {
                labels.push((format!("s:{id}"), format!("surface {id}")));
            }
        }
        let options: Vec<(String, &str)> = labels.iter().map(|(k, l)| (k.clone(), l.as_str())).collect();
        let mut cam_changed = false;
        kv_wrap(ui, theme, tr("camera"), |ui| {
            cam_changed = dropdown_fill(ui, theme, ("view-camera", gid, node_id, i), &mut key, &options).changed();
        });
        if cam_changed {
            if let Some(o) = list[i].as_object_mut() {
                match camera_for(&key, &cam) {
                    Some(c) => o.insert("camera".into(), c),
                    None => o.remove("camera"),
                };
            }
            changed = true;
        }

        // Manual eye, target and fov.
        if camera_key(&list[i]["camera"]) == "manual" {
            let c = list[i]["camera"].clone();
            let mut eye = xyz(&c["eye"], [0.0, 1.6, 4.0]);
            let mut target = xyz(&c["target"], [0.0, 0.0, 0.0]);
            let mut fov = num(&c["fov"], 50.0);
            let mut edited = false;
            let axes = ["x", "y", "z"];
            for (what, v) in [(tr("camera · position"), &mut eye), (tr("camera · target"), &mut target)] {
                for (k, axis) in axes.iter().enumerate() {
                    edited |= number_row(ui, theme, &format!("{what} {axis}"), &mut v[k], -1000.0..=1000.0, "", 3, true).changed();
                }
            }
            edited |= number_row(ui, theme, tr("camera · fov"), &mut fov, 1.0..=179.0, "", 1, true).changed();
            if edited {
                list[i]["camera"] = json!({ "eye": eye, "target": target, "fov": fov });
                changed = true;
            }
        }

        // Per-view edits: read-only in v1.
        for field in ["add", "set"] {
            if let Some(m) = list[i][field].as_object().filter(|m| !m.is_empty()) {
                ui.label(small_text(theme, format!("{field} {}", Value::Object(m.clone()))));
            }
        }

        kv_wrap(ui, theme, "", |ui| {
            if payload_button(ui, theme, small_text(theme, tr("remove view"))).clicked() {
                remove_view = Some(i);
            }
        });
    }
    if let Some(i) = remove_view {
        list.remove(i);
        changed = true;
    }

    // Add a view: the next free `v<n>` name.
    kv_wrap(ui, theme, "", |ui| {
        if payload_button(ui, theme, small_text(theme, tr("Add view"))).clicked() {
            let mut n = list.len() + 1;
            while list.iter().any(|v| v["name"].as_str() == Some(format!("v{n}").as_str())) {
                n += 1;
            }
            list.push(json!({ "name": format!("v{n}") }));
            changed = true;
        }
    });

    // Amount: one knob for every variation.
    let mut amount = num(&next.get("amount").cloned().unwrap_or(json!(1.0)), 1.0);
    if number_row(ui, theme, tr("Amount"), &mut amount, 0.0..=1.0, "", 2, true).changed() {
        next.insert("amount".into(), json!(amount));
        changed = true;
    }

    // Vary: one spread per float param.
    subtitle(ui, theme, tr("Vary"));
    let mut remove_vary: Option<usize> = None;
    for (j, v) in vary.iter_mut().enumerate() {
        let mut param = v["param"].as_str().unwrap_or("").to_string();
        let options: Vec<(String, &str)> = float_names.iter().map(|n| (n.clone(), n.as_str())).collect();
        let mut param_changed = false;
        kv_wrap(ui, theme, tr("param"), |ui| {
            param_changed = dropdown_fill(ui, theme, ("view-vary", gid, node_id, j), &mut param, &options).changed();
        });
        if param_changed {
            v["param"] = json!(param);
            changed = true;
        }
        let mut spread = num(&v["spread"], 0.0);
        if number_row(ui, theme, tr("spread"), &mut spread, -1.0..=1.0, "", 2, true).changed() {
            v["spread"] = json!(spread);
            changed = true;
        }
        kv_wrap(ui, theme, "", |ui| {
            if payload_button(ui, theme, small_text(theme, tr("remove vary"))).clicked() {
                remove_vary = Some(j);
            }
        });
    }
    if let Some(j) = remove_vary {
        vary.remove(j);
        changed = true;
    }
    if let Some(first) = float_names.first() {
        kv_wrap(ui, theme, "", |ui| {
            if payload_button(ui, theme, small_text(theme, tr("Add vary"))).clicked() {
                vary.push(json!({ "param": first, "spread": 0.0 }));
                changed = true;
            }
        });
    }

    // One view per projector, when the venue has projectors.
    if let Some(v) = venue.filter(|v| v["projectors"].as_array().is_some_and(|a| !a.is_empty())) {
        kv_wrap(ui, theme, "", |ui| {
            if payload_button(ui, theme, small_text(theme, tr("One view per projector"))).clicked() {
                if let Value::Array(a) = views_for_projectors(v) {
                    list = a;
                    changed = true;
                }
            }
        });
    }

    if changed {
        next.insert("list".into(), Value::Array(list));
        next.insert("vary".into(), Value::Array(vary));
        post_text(door, gid, node_id, json!({ "views": Value::Object(next) }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::FakeDoor;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn one_view_per_projector_names_each_view_after_its_projector() {
        let venue = json!({
            "projectors": [{ "id": "p1" }, { "id": "p2" }],
            "surfaces": [{ "id": "wall" }]
        });
        assert_eq!(
            views_for_projectors(&venue),
            json!([
                { "name": "p1", "camera": { "projector": "p1" } },
                { "name": "p2", "camera": { "projector": "p2" } }
            ])
        );
        assert_eq!(views_for_projectors(&json!({ "surfaces": [] })), json!([]));
    }

    #[test]
    fn the_canvas_lists_one_view_port_per_view() {
        let config = json!({ "views": { "list": [{ "name": "left" }, { "name": "right" }] } });
        assert_eq!(view_ports(&config), ["view.left", "view.right"]);
        assert!(view_ports(&json!({ "floats": {} })).is_empty());
    }

    fn draw(door: std::rc::Rc<FakeDoor>, node: Value) -> egui_kittest::Harness<'static, ()> {
        let theme = Theme::phosphor();
        let t = theme.clone();
        let h = egui_kittest::Harness::builder().with_size(egui::Vec2 { x: 600.0, y: 900.0 }).build_ui(move |ui| {
            views_section(ui, &t, door.as_ref(), "g1", "gen-0", &node, None, &["hue".to_string()]);
        });
        theme.apply(&h.ctx);
        h
    }

    /// The empty folder offers "Add view", and a click writes the first view
    /// as the whole `views` object.
    #[test]
    fn add_view_posts_the_whole_views_object() {
        let door = std::rc::Rc::new(FakeDoor::new());
        let mut h = draw(std::rc::Rc::clone(&door), json!({ "config": {} }));
        h.run();
        h.get_by_label("Add view").click();
        h.run();
        let posts: Vec<Value> = door
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
            .map(|(_, _, params)| serde_json::from_slice(&params.body).unwrap())
            .collect();
        assert_eq!(posts.len(), 1, "{posts:?}");
        assert_eq!(posts[0]["op"], json!("config"));
        assert_eq!(posts[0]["config"]["views"]["list"], json!([{ "name": "v1" }]));
    }
}
