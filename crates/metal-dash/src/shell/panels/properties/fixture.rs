//! Per-fixture properties (M4.2 of the pixel map): the profile, the address
//! or Bluetooth pairing, the group, the live state (commanded vs estimated
//! pan/tilt, `GET /venue/state`), `aim`, `test` (flash/identify) and
//! `highlight in frame`.
//!
//! It is a page of the `venue.fixtures` node's browser. The fixture under
//! edit is the one the venue editor last selected
//! ([`crate::shell::panels::venue_editor::State::selected`]), so the two
//! panels are one workflow: place and click in the editor, tune here.
//!
//! Every write goes through `POST /venue`'s `config`/`aim`/`identify` ops —
//! the same routes the AI's tools use (rule 6).

use crate::shell::components::small_text;
use egui::Ui;
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{field_row, kv_row, kv_wrap, note, num_value};
use crate::shell::i18n::{tr, trf};
use crate::shell::panels::venue_editor::State as EditorState;

/// The `venue.fixture` node's page: which row of the room it *is*, then that
/// row's own settings.
///
/// The node is deliberately not one fixture: one node can stand for a group
/// of real fixtures on the bus (the same picture sent to four cans), and the
/// room it points at is the one its cables reach (`venue_map::venue_for`), so
/// the chain reads left to right — fixtures before the surface they land
/// behind. `extra` lists further rows of the room this node also drives
/// (`config.fixtures`), each with the same source.
#[allow(clippy::too_many_arguments)]
pub(crate) fn node_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
    editor: &mut EditorState,
) {
    let room = crate::shell::panels::venue_map::venue_for(door, gid, node_id, node, graph);
    let venue_id = room.id.as_str();
    let read = get(door, "/venue", &[("id", venue_id)]);
    let rows: Vec<(String, String)> = read
        .as_ref()
        .and_then(|v| v["venue"]["fixture"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|f| {
            Some((
                f["id"].as_str()?.to_string(),
                f["name"].as_str().unwrap_or_default().to_string(),
            ))
        })
        .collect();
    let mut wiring: Vec<String> = node["config"]["fixtures"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let primary = node["config"]["fixture"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| rows.first().map(|(id, _)| id.clone()));

    // The room's own row this node is: the one the panels below edit, and the
    // one the card's swatch shows.
    if let Some(pick) = primary.clone() {
        if node["config"]["fixture"] != json!(pick) {
            crate::shell::panels::props_venue::post_config(
                door,
                gid,
                node_id,
                json!({ "venue": venue_id, "fixture": pick }),
            );
        }
        editor.selected = Some(pick);
    }
    let mut pick_now = primary.clone().unwrap_or_default();
    let options: Vec<(String, &str)> = rows.iter().map(|(id, n)| (id.clone(), n.as_str())).collect();
    if options.is_empty() {
        note(ui, theme, tr("— this room has no fixture —"));
        return;
    }
    let before = pick_now.clone();
    kv_row(ui, theme, tr("fixture"), |ui| {
        crate::shell::components::dropdown(
            ui,
            theme,
            ("shell/venue/fixture-node", node_id),
            &mut pick_now,
            &options,
        )
    });
    if pick_now != before {
        editor.selected = Some(pick_now.clone());
        crate::shell::panels::props_venue::post_config(
            door,
            gid,
            node_id,
            json!({ "venue": venue_id, "fixture": pick_now }),
        );
    }

    // A group node: the further rows of the room the same picture also
    // reaches. The room is the list; the node is one or more of its rows.
    let extra: Vec<(String, &str)> = rows
        .iter()
        .filter(|(id, _)| Some(id) != primary.as_ref())
        .map(|(id, n)| (id.clone(), n.as_str()))
        .collect();
    let mut changed = false;
    for (id, name) in extra {
        let mut wired = wiring.iter().any(|w| w == &id);
        let label = if name.is_empty() { id.clone() } else { name.to_string() };
        let r = crate::shell::components::toggle_row(ui, theme, &label, &mut wired);
        if r.changed() {
            if wired {
                wiring.push(id.clone());
            } else {
                wiring.retain(|w| w != &id);
            }
            changed = true;
        }
    }
    if changed {
        let value = if wiring.is_empty() {
            Value::Null
        } else {
            json!(wiring)
        };
        crate::shell::panels::props_venue::post_config(door, gid, node_id, json!({ "fixtures": value }));
    }

    let Some(fixture) = editor.selected.clone() else {
        return;
    };
    row_fields(ui, theme, door, venue_id, &fixture, Some(node_id), editor);
}

/// One row of the room, edited in place: the same fields the group node's
/// browser shows, keyed by the row id rather than by the browser's selection.
pub(crate) fn section(ui: &mut Ui, theme: &Theme, door: &dyn Call, venue_id: &str, editor: &mut EditorState) {
    let Some(fixture) = editor.selected.clone() else {
        note(ui, theme, tr("select a fixture in the venue editor"));
        return;
    };
    row_fields(ui, theme, door, venue_id, &fixture, None, editor);
}

fn row_fields(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    venue_id: &str,
    fixture: &str,
    node_id: Option<&str>,
    editor: &mut EditorState,
) {
    let Some(read) = get(door, "/venue", &[("id", venue_id)]) else {
        note(ui, theme, tr("— could not read the venue —"));
        return;
    };
    let Some(f) = read["venue"]["fixture"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["id"] == json!(fixture)))
        .cloned()
    else {
        note(ui, theme, tr("— no such fixture —"));
        return;
    };

    field_row(ui, theme, fixture, f["kind"].as_str().unwrap_or_default());

    // Profile combo: the venue's own profiles plus "none".
    let current = f["profile"].as_str().unwrap_or_default().to_string();
    let profiles = profiles(door);
    kv_row(ui, theme, tr("profile"), |ui| {
        let mut chosen = current.clone();
        let r = crate::shell::components::combo_box(theme, ("venue-fixture/profile", venue_id, fixture, node_id))
            .width(theme.control_width)
            .selected_text((if chosen.is_empty() { tr("none").to_string() } else { chosen.clone() }).to_string())
            .show_ui(ui, |ui| {
                for p in &profiles {
                    ui.selectable_value(&mut chosen, p.clone(), small_text(theme, p.clone()));
                }
            })
            .response;
        if chosen != current {
            post_config(door, venue_id, fixture, json!({ "profile": chosen }));
        }
        r
    });

    // Address / universe (DMX), or the Bluetooth pairing when the profile is
    // one (M4.2).
    let wire = f["wire"].as_str().unwrap_or_default().to_string();
    if wire == "bluetooth" {
        kv_row(ui, theme, tr("Bluetooth pairing"), |ui| {
            let r = ui.button(small_text(theme, tr("pair")));
            if r.clicked() {
                post_config(door, venue_id, fixture, json!({ "pair": true }));
            }
            r
        });
    } else if wire != "camera" {
        let address = f["address"].as_u64().unwrap_or(1) as u32;
        let universe = f["universe"].as_u64().unwrap_or(1) as u32;
        kv_row(ui, theme, tr("universe"), |ui| {
            let mut u = universe as f32;
            let w = ui.available_width();
            let r = num_value(ui, theme, w, &mut u, 1.0..=63999.0, "", 0, 1.0, true);
            if r.changed() {
                post_config(door, venue_id, fixture, json!({ "universe": u.round().clamp(1.0, 63999.0) as u32 }));
            }
            r
        });
        kv_row(ui, theme, tr("address"), |ui| {
            let mut a = address as f32;
            let w = ui.available_width();
            let r = num_value(ui, theme, w, &mut a, 1.0..=512.0, "", 0, 1.0, true);
            if r.changed() {
                post_config(door, venue_id, fixture, json!({ "address": a.round().clamp(1.0, 512.0) as u32 }));
            }
            r
        });
    }

    // Group.
    let mut group = f["group"].as_str().unwrap_or_default().to_string();
    kv_row(ui, theme, tr("group"), |ui| {
        let w = ui.available_width();
        let r = ui.add(egui::TextEdit::singleline(&mut group).hint_text(tr("front-wash")).desired_width(w));
        if r.changed() {
            let value = if group.trim().is_empty() { Value::Null } else { json!(group) };
            post_config(door, venue_id, fixture, json!({ "group": value }));
        }
        r
    });

    // Aim handles: yaw/pitch/roll of the fixture's own pose (M4.1/M4.2).
    for (key, label, range) in [("yaw", tr("yaw"), -360.0..=360.0), ("pitch", tr("pitch"), -180.0..=180.0), ("roll", tr("roll"), -360.0..=360.0)] {
        let mut v = f[key].as_f64().unwrap_or(0.0) as f32;
        kv_row(ui, theme, label, |ui| {
            let w = ui.available_width();
            let r = num_value(ui, theme, w, &mut v, range, "°", 0, 0.0, true);
            if r.changed() {
                post_config(door, venue_id, fixture, json!({ key: v }));
            }
            r
        });
    }

    // Live state: commanded vs estimated.
    if let Some(s) = get(door, "/venue/state", &[("id", venue_id)])
        .and_then(|s| s["fixtures"].as_array().and_then(|a| a.iter().find(|x| x["fixture"] == json!(fixture))).cloned())
    {
        let c = &s["commanded"];
        let a = &s["actual"];
        note(ui, theme, &trf("pan {cp}°→{ap}°  tilt {ct}°→{at}°", &[
            ("cp", &fmt(&c["pan"])),
            ("ap", &fmt(&a["pan"])),
            ("ct", &fmt(&c["tilt"])),
            ("at", &fmt(&a["tilt"])),
        ]));
    }

    kv_wrap(ui, theme, "", |ui| {
        if ui.button(small_text(theme, tr("aim at origin"))).clicked() {
            if door.json("POST", "/venue", Some(json!({ "op": "aim", "id": venue_id, "fixture": fixture, "target": [4.0, 1.5, -5.0] }))).is_ok() {
                editor.note = Some(format!("{} {}", tr("aimed"), fixture));
            }
        }
        if ui.button(small_text(theme, tr("test"))).clicked() {
            if door.json("POST", "/venue", Some(json!({ "op": "identify", "id": venue_id, "fixture": fixture }))).is_ok() {
                editor.note = Some(format!("{} {}", tr("identifying"), fixture));
            }
        }
        let mut highlight = editor.highlight.as_deref() == Some(fixture);
        if crate::shell::components::toggle_row(ui, theme, tr("highlight in frame"), &mut highlight).changed() {
            editor.highlight = highlight.then(|| fixture.to_string());
        }
    });
}

fn fmt(v: &Value) -> String {
    format!("{:.1}", v.as_f64().unwrap_or(0.0))
}

/// `POST /venue { op: "config", … }` — one field of a fixture.
fn post_config(door: &dyn Call, venue: &str, fixture: &str, fields: Value) {
    let mut body = json!({ "op": "config", "id": venue, "fixture": fixture });
    if let (Some(dst), Some(src)) = (body.as_object_mut(), fields.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    let _ = door.json("POST", "/venue", Some(body));
}

/// The profile names under `data/fixtures/` the route offers for the combo.
fn profiles(door: &dyn Call) -> Vec<String> {
    get(door, "/fixtures", &[])
        .and_then(|v| v["fixtures"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()))
        .unwrap_or_default()
}

fn get(door: &dyn Call, path: &str, query: &[(&str, &str)]) -> Option<Value> {
    let params = crate::call::Params {
        query: query.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect(),
        body: Vec::new(),
        tail: None,
    };
    match door.call("GET", path, params) {
        Some(reply) if reply.status == 200 => crate::json_memo::parse(&reply.body).ok(),
        _ => None,
    }
}
