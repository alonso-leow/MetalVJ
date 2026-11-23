//! The **Views** folder of a `gen.*` node (`config.views`, see
//! `metal_graph::views`): the list of perspectives, each with a camera source
//! and its own edits, the shared `amount`, the `vary` spreads, and the
//! `parallel` params. A parallel param (a float, a colour slot or `camera`)
//! leaves the regular params section and gets one row per view, under that
//! view's name; a view's palette fills every parallel colour slot at once.
//!
//! Every write posts the whole `views` object through `op: "config"` (which
//! replaces the top-level key), so a partial edit can never drop a sibling.
//! `add` maps are shown read-only: v1 edits them in the file only.

use egui::{Id, Ui};
use serde_json::{json, Map, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{dropdown_fill, kv_wrap, number_row, payload_button, section_heading, small_text, subtitle, toggle_row};
use crate::shell::i18n::{tr, trf};

use super::io::colour_stops;
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

/// The names a view may hold parallel: every float and colour param, then
/// `camera`.
pub(crate) fn parallel_choices(params: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = params
        .iter()
        .filter(|p| p["kind"] == json!("float") || p["kind"] == json!("colour"))
        .filter_map(|p| p["name"].as_str().map(str::to_string))
        .collect();
    if !out.iter().any(|n| n == "camera") {
        out.push("camera".to_string());
    }
    out
}

/// `parallel` with `name` added when it is absent, removed when it is present.
pub(crate) fn toggle_parallel(parallel: &[String], name: &str) -> Vec<String> {
    let mut out: Vec<String> = parallel.iter().filter(|n| n.as_str() != name).cloned().collect();
    if parallel.iter().all(|n| n != name) {
        out.push(name.to_string());
    }
    out
}

/// The palettes of a `GET /author/palettes` reply, as `(name, 12 floats)`. A
/// palette whose colours are not twelve floats (four RGB stops) is skipped.
pub(crate) fn palettes_of(reply: &Value) -> Vec<(String, Vec<f32>)> {
    reply["palettes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let name = p["name"].as_str()?.to_string();
            let colours: Vec<f32> = p["colors"].as_array()?.iter().filter_map(Value::as_f64).map(|f| f as f32).collect();
            (colours.len() == 12).then_some((name, colours))
        })
        .collect()
}

/// Fills `view` from palette `id`: `palette` is set to `id`, and the palette's
/// twelve floats are written to every colour slot that is both parallel and a
/// colour param (`colour_slots`). Other slots are left alone.
pub(crate) fn fill_palette(view: &mut Value, id: &str, colours: &[f32], parallel: &[String], colour_slots: &[String]) {
    view["palette"] = json!(id);
    for slot in colour_slots.iter().filter(|s| parallel.contains(s)) {
        view["colours"][slot.as_str()] = json!(colours);
    }
}

/// The node feeding this gen node's `viewer` port, by node id, if an edge
/// does.
pub(crate) fn viewer_source(graph: &Value, node_id: &str) -> Option<String> {
    graph["graph"]["edges"]
        .as_array()?
        .iter()
        .find(|e| e["to"]["node"].as_str() == Some(node_id) && e["to"]["port"].as_str() == Some(metal_graph::views::VIEWER_PORT))
        .and_then(|e| e["from"]["node"].as_str().map(str::to_string))
}

/// The "Rooms" preset: `parallel` becomes `colour_slots`. A non-empty `list`
/// is kept; an empty one becomes one view per venue projector, or `room-a` and
/// `room-b` when there is no venue with projectors.
pub(crate) fn rooms_preset(list: &[Value], colour_slots: &[String], venue: Option<&Value>) -> Value {
    let list = if !list.is_empty() {
        Value::Array(list.to_vec())
    } else {
        let from_venue = venue.map(views_for_projectors).unwrap_or(json!([]));
        if from_venue.as_array().is_some_and(|a| !a.is_empty()) {
            from_venue
        } else {
            json!([{ "name": "room-a" }, { "name": "room-b" }])
        }
    };
    json!({ "parallel": colour_slots, "list": list })
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

/// A JSON array of numbers as floats; `None` when it is not one.
fn floats(v: &Value) -> Option<Vec<f32>> {
    v.as_array().map(|a| a.iter().map(|x| num(x, 0.0)).collect())
}

/// The camera source dropdown of view `view` (`k` keys its widget) and, for a
/// manual camera, its eye, target and fov. True when the view changed.
#[allow(clippy::too_many_arguments)]
fn camera_rows(ui: &mut Ui, theme: &Theme, gid: &str, node_id: &str, k: usize, venue: Option<&Value>, view: &mut Value) -> bool {
    let mut changed = false;
    let cam = view["camera"].clone();
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
        cam_changed = dropdown_fill(ui, theme, ("view-camera", gid, node_id, k), &mut key, &options).changed();
    });
    if cam_changed {
        if let Some(o) = view.as_object_mut() {
            match camera_for(&key, &cam) {
                Some(c) => o.insert("camera".into(), c),
                None => o.remove("camera"),
            };
        }
        changed = true;
    }

    // Manual eye, target and fov.
    if camera_key(&view["camera"]) == "manual" {
        let c = view["camera"].clone();
        let mut eye = xyz(&c["eye"], [0.0, 1.6, 4.0]);
        let mut target = xyz(&c["target"], [0.0, 0.0, 0.0]);
        let mut fov = num(&c["fov"], 50.0);
        let mut edited = false;
        let axes = ["x", "y", "z"];
        for (what, v) in [(tr("camera · position"), &mut eye), (tr("camera · target"), &mut target)] {
            for (j, axis) in axes.iter().enumerate() {
                edited |= number_row(ui, theme, &format!("{what} {axis}"), &mut v[j], -1000.0..=1000.0, "", 3, true).changed();
            }
        }
        edited |= number_row(ui, theme, tr("camera · fov"), &mut fov, 1.0..=179.0, "", 1, true).changed();
        if edited {
            view["camera"] = json!({ "eye": eye, "target": target, "fov": fov });
            changed = true;
        }
    }
    changed
}

/// The palettes the Palette dropdowns list, read once per session of the
/// panel and kept in egui's memory.
fn palette_choices(ui: &mut Ui, door: &dyn Call) -> Vec<(String, Vec<f32>)> {
    let id = Id::new("view-palettes");
    if let Some(cached) = ui.data_mut(|d| d.get_temp::<Vec<(String, Vec<f32>)>>(id)) {
        return cached;
    }
    let found = door.json("GET", "/author/palettes", None).map(|r| palettes_of(&r)).unwrap_or_default();
    ui.data_mut(|d| d.insert_temp(id, found.clone()));
    found
}

/// The node's current 12-float value of colour param `param`: the held value,
/// else its `values`, else its `defaults`.
fn colour_value(held: Option<Vec<f32>>, param: &Value) -> Vec<f32> {
    held.or_else(|| floats(&param["values"])).or_else(|| floats(&param["defaults"])).unwrap_or_default()
}

/// Paints the Views folder of a `gen.*` node. `venue` is the node's venue
/// (`GET /venue`'s `venue` object) when it has one; `params` are the node's
/// io params (their names, kinds and ranges); `graph` is the graph for the
/// viewer status line.
#[allow(clippy::too_many_arguments)]
pub(crate) fn views_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
    venue: Option<&Value>,
    params: &[Value],
) {
    let mut next: Map<String, Value> = node["config"]["views"].as_object().cloned().unwrap_or_default();
    let mut list: Vec<Value> = next.get("list").and_then(Value::as_array).cloned().unwrap_or_default();
    let vary: Vec<Value> = next.get("vary").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut parallel: Vec<String> = metal_graph::views::views_of(&node["config"]).map(|v| v.parallel).unwrap_or_default();
    let colour_slots: Vec<String> = params
        .iter()
        .filter(|p| p["kind"] == json!("colour"))
        .filter_map(|p| p["name"].as_str().map(str::to_string))
        .collect();
    let camera_parallel = parallel.iter().any(|n| n == "camera");
    let mut changed = false;

    // Component 1 — create a variation: every param with a switch for "this
    // one varies" (floats, colours and the camera), then the button that adds
    // the variation as the last one, seeded from the main values.
    subtitle(ui, theme, tr("Create variation"));
    for name in parallel_choices(params) {
        let mut on = parallel.iter().any(|n| *n == name);
        if toggle_row(ui, theme, &name, &mut on).changed() {
            parallel = toggle_parallel(&parallel, &name);
            changed = true;
        }
    }
    kv_wrap(ui, theme, "", |ui| {
        if payload_button(ui, theme, small_text(theme, tr("Add variation"))).clicked() {
            let mut n = list.len() + 1;
            while list.iter().any(|v| v["name"].as_str() == Some(format!("v{n}").as_str())) {
                n += 1;
            }
            let mut view = json!({ "name": format!("v{n}") });
            for name in parallel.iter().filter(|n| n.as_str() != "camera") {
                let Some(param) = params.iter().find(|p| p["name"].as_str() == Some(name.as_str())) else {
                    continue;
                };
                match param["kind"].as_str() {
                    Some("float") => view["set"][name.as_str()] = json!(num(&param["value"], num(&param["min"], 0.0))),
                    Some("colour") => view["colours"][name.as_str()] = json!(colour_value(None, param)),
                    _ => {}
                }
            }
            list.push(view);
            changed = true;
        }
    });

    let mut amount = num(&next.get("amount").cloned().unwrap_or(json!(1.0)), 1.0);
    if number_row(ui, theme, tr("Amount"), &mut amount, 0.0..=1.0, "", 2, true).changed() {
        next.insert("amount".into(), json!(amount));
        changed = true;
    }

    let ports = view_ports(&node["config"]);
    if !ports.is_empty() {
        ui.label(small_text(theme, trf("outputs: {ports}", &[("ports", &ports.join(", "))])));
    }
    let viewer = match viewer_source(graph, node_id) {
        Some(from) => trf("Viewer: tracked by {node}", &[("node", &from)]),
        None => tr("Viewer: fixed").to_string(),
    };
    ui.label(small_text(theme, viewer));

    // Component 2 — the variations: after the main params above, one column
    // per variation, each a copy of the whole params list. The params that
    // vary are editable here; the others show the main value, greyed.
    let mut remove_view: Option<usize> = None;
    for k in 0..list.len() {
        let current = list[k]["name"].as_str().unwrap_or("").to_string();
        subtitle(ui, theme, &current);

        let draft_id = Id::new(("view-name", gid, node_id, k));
        let mut draft = ui.data_mut(|d| d.get_temp::<String>(draft_id)).unwrap_or_else(|| current.clone());
        let mut lost_focus = false;
        kv_wrap(ui, theme, tr("name"), |ui| {
            lost_focus = ui.add(egui::TextEdit::singleline(&mut draft).desired_width(f32::INFINITY)).lost_focus();
        });
        let taken = |name: &str| {
            let name = name.trim();
            name.is_empty() || list.iter().enumerate().any(|(j, v)| j != k && v["name"].as_str() == Some(name))
        };
        if lost_focus {
            ui.data_mut(|d| d.remove::<String>(draft_id));
            if !taken(&draft) && draft.trim() != current {
                list[k]["name"] = json!(draft.trim());
                changed = true;
            }
        } else {
            ui.data_mut(|d| d.insert_temp(draft_id, draft.clone()));
            if taken(&draft) {
                ui.label(small_text(theme, tr("a view needs a unique name")));
            }
        }

        let palettes = palette_choices(ui, door);
        let mut palette = list[k]["palette"].as_str().unwrap_or("").to_string();
        let mut options: Vec<(String, &str)> = vec![(String::new(), tr("None"))];
        options.extend(palettes.iter().map(|(n, _)| (n.clone(), n.as_str())));
        let mut palette_changed = false;
        kv_wrap(ui, theme, tr("Palette"), |ui| {
            palette_changed = dropdown_fill(ui, theme, ("view-palette", gid, node_id, k), &mut palette, &options).changed();
        });
        if palette_changed {
            match palettes.iter().find(|(n, _)| *n == palette) {
                Some((id, colours)) => fill_palette(&mut list[k], id, colours, &parallel, &colour_slots),
                None => {
                    if let Some(o) = list[k].as_object_mut() {
                        o.remove("palette");
                    }
                }
            }
            changed = true;
        }

        // The camera row: editable when it varies, else the shared camera.
        if camera_parallel {
            changed |= camera_rows(ui, theme, gid, node_id, k, venue, &mut list[k]);
        }

        for param in params.iter().filter(|p| p["kind"] == json!("float") || p["kind"] == json!("colour")) {
            let Some(name) = param["name"].as_str() else { continue };
            let varies = parallel.iter().any(|n| n == name);
            if param["kind"] == json!("float") {
                let min = num(&param["min"], 0.0);
                let max = num(&param["max"], 1.0);
                let main = num(&param["value"], min);
                let mut value = if varies { list[k]["set"][name].as_f64().map(|f| f as f32).unwrap_or(main) } else { main };
                let edited = ui
                    .add_enabled_ui(varies, |ui| number_row(ui, theme, name, &mut value, min..=max, "", 3, varies).changed())
                    .inner;
                if varies && edited {
                    list[k]["set"][name] = json!(value);
                    changed = true;
                }
            } else {
                let mut vector = if varies {
                    colour_value(floats(&list[k]["colours"][name]), param)
                } else {
                    colour_value(None, param)
                };
                section_heading(ui, theme, name);
                let edited = ui.add_enabled_ui(varies, |ui| colour_stops(ui, theme, &mut vector)).inner;
                if varies && edited {
                    list[k]["colours"][name] = json!(vector);
                    changed = true;
                }
            }
        }

        // Per-view edits: read-only in v1.
        for field in ["add", "set"] {
            if field == "set" {
                continue;
            }
            if let Some(m) = list[k][field].as_object().filter(|m| !m.is_empty()) {
                ui.label(small_text(theme, format!("{field} {}", Value::Object(m.clone()))));
            }
        }

        kv_wrap(ui, theme, "", |ui| {
            if payload_button(ui, theme, small_text(theme, tr("remove view"))).clicked() {
                remove_view = Some(k);
            }
        });
    }
    if let Some(i) = remove_view {
        list.remove(i);
        changed = true;
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

    // The Rooms preset: every colour slot parallel, one view per room.
    kv_wrap(ui, theme, "", |ui| {
        let rooms = payload_button(ui, theme, small_text(theme, tr("Rooms")));
        if rooms.on_hover_text(tr("Same music, different rooms")).clicked() {
            let preset = rooms_preset(&list, &colour_slots, venue);
            parallel = preset["parallel"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            list = preset["list"].as_array().cloned().unwrap_or_default();
            changed = true;
        }
    });

    if changed {
        next.insert("list".into(), Value::Array(list));
        next.insert("vary".into(), Value::Array(vary));
        next.insert("parallel".into(), json!(parallel));
        post_text(door, gid, node_id, json!({ "views": Value::Object(next) }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::FakeDoor;
    use egui_kittest::kittest::Queryable;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

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

    fn params() -> Vec<Value> {
        vec![
            json!({ "name": "hue", "kind": "float", "min": 0.0, "max": 1.0, "value": 0.2 }),
            json!({ "name": "speed", "kind": "float", "min": 0.0, "max": 2.0 }),
            json!({ "name": "tint", "kind": "colour", "defaults": vec![0.5f32; 12] }),
            json!({ "name": "camera", "kind": "camera", "defaults": [0, 0, 4, 0, 0, 0, 45] }),
        ]
    }

    /// The toggle grid offers every float, every colour and `camera`, and a
    /// toggle adds the name to `parallel` or takes it out again.
    #[test]
    fn the_toggle_builds_the_parallel_list() {
        assert_eq!(parallel_choices(&params()), names(&["hue", "speed", "tint", "camera"]));
        let on = toggle_parallel(&[], "hue");
        assert_eq!(on, names(&["hue"]));
        assert_eq!(toggle_parallel(&on, "camera"), names(&["hue", "camera"]));
        assert_eq!(toggle_parallel(&on, "hue"), Vec::<String>::new());
    }

    /// A palette writes its colours to parallel colour slots only, and sets
    /// `palette`.
    #[test]
    fn the_palette_fill_writes_only_parallel_colour_slots() {
        let colours: Vec<f32> = (0..12).map(|i| i as f32 / 12.0).collect();
        let mut view = json!({ "name": "left" });
        fill_palette(&mut view, "warm", &colours, &names(&["tint", "hue"]), &names(&["tint", "bg"]));
        assert_eq!(view["palette"], json!("warm"));
        assert_eq!(view["colours"], json!({ "tint": colours }));
        let mut other = json!({ "name": "right" });
        fill_palette(&mut other, "cool", &colours, &names(&["hue"]), &names(&["tint", "bg"]));
        assert!(other.get("colours").is_none());
    }

    /// The preset: every colour slot is parallel; the list is the venue's
    /// projectors, else `room-a` and `room-b`; existing views are kept.
    #[test]
    fn the_rooms_preset_json() {
        let slots = names(&["tint", "bg"]);
        assert_eq!(
            rooms_preset(&[], &slots, None),
            json!({ "parallel": ["tint", "bg"], "list": [{ "name": "room-a" }, { "name": "room-b" }] })
        );
        let venue = json!({ "projectors": [{ "id": "p1" }, { "id": "p2" }] });
        assert_eq!(
            rooms_preset(&[], &slots, Some(&venue))["list"],
            json!([
                { "name": "p1", "camera": { "projector": "p1" } },
                { "name": "p2", "camera": { "projector": "p2" } }
            ])
        );
        let kept = vec![json!({ "name": "front" })];
        assert_eq!(rooms_preset(&kept, &slots, Some(&venue)), json!({ "parallel": ["tint", "bg"], "list": [{ "name": "front" }] }));
    }

    #[test]
    fn palettes_of_keeps_twelve_float_palettes() {
        let twelve: Vec<f32> = vec![0.5; 12];
        let reply = json!({ "palettes": [{ "name": "warm", "colors": twelve }, { "name": "bad", "colors": [1.0] }] });
        assert_eq!(palettes_of(&reply), vec![("warm".to_string(), twelve)]);
    }

    #[test]
    fn the_viewer_line_names_the_node_wired_to_the_viewer_port() {
        let graph = json!({ "graph": { "edges": [
            { "from": { "node": "cap", "port": "out" }, "to": { "node": "gen-0", "port": "image" } },
            { "from": { "node": "sensor.yolo-1", "port": "control" }, "to": { "node": "gen-0", "port": "viewer" } }
        ] } });
        assert_eq!(viewer_source(&graph, "gen-0").as_deref(), Some("sensor.yolo-1"));
        assert_eq!(viewer_source(&graph, "gen-9"), None);
        assert_eq!(viewer_source(&json!({ "graph": { "edges": [] } }), "gen-0"), None);
    }

    fn draw(door: std::rc::Rc<FakeDoor>, node: Value) -> egui_kittest::Harness<'static, ()> {
        let theme = Theme::phosphor();
        let t = theme.clone();
        let h = egui_kittest::Harness::builder().with_size(egui::Vec2 { x: 600.0, y: 900.0 }).build_ui(move |ui| {
            views_section(ui, &t, door.as_ref(), "g1", "gen-0", &node, &json!({ "graph": { "edges": [] } }), None, &params());
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
        h.get_by_label("Add variation").click();
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
