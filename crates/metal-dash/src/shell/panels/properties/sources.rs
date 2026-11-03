//! 2026-10-01: the three source pages the properties browser lacked.
//!
//! - `audio.live` → **input device**: moved to `listen.rs` (plan "Audio
//!   input is a node": one listener per node, `source` in its config).
//! - `capture` → **capture source**: a camera (the a6400's loopback or
//!   `gphoto2` direct included), a screen, or one program's window, written to
//!   the node's config (`source`) — `metal_perform::capture` opens it.
//!
//! The lists come off `GET /io`, cached for [`LIST_TTL`] in egui memory: the
//! route spawns `pactl`/`niri`, which is not a per-frame cost.

use crate::shell::components::small_text;
use egui::{Id, RichText, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{float_row, kv_row, kv_wrap, lucide, note, Float, Icon};
use crate::shell::i18n::{tr, tr_dyn};

use super::listen;
use super::shared::get;
use super::text::post_text;

/// How long one `GET /io` answer is reused, in seconds.
const LIST_TTL: f64 = 2.0;

/// `GET /io`, cached in egui memory; `refresh` drops the cache first.
pub(crate) fn io_listing(ui: &Ui, door: &dyn Call, refresh: bool) -> Value {
    let id = Id::new("shell/properties/io-listing");
    let now = ui.input(|i| i.time);
    let cached: Option<(Value, f64)> = ui.ctx().data(|d| d.get_temp(id));
    if let Some((v, _)) = cached.filter(|(_, at)| !refresh && now - at < LIST_TTL) {
        return v;
    }
    let v = get(door, "GET", "/io", &[]).unwrap_or_else(|| json!({}));
    ui.ctx().data_mut(|d| d.insert_temp(id, (v.clone(), now)));
    v
}

/// One labelled combo of `(id, label)` choices; returns the picked id.
pub(crate) fn pick_row(
    ui: &mut Ui,
    theme: &Theme,
    salt: &str,
    label: &str,
    current: &str,
    choices: &[(String, String)],
) -> Option<String> {
    let shown = choices
        .iter()
        .find(|(id, _)| id == current)
        .map(|(_, l)| l.clone())
        .unwrap_or_else(|| {
            if current.is_empty() {
                tr("— none —").to_string()
            } else {
                current.to_string()
            }
        });
    let mut picked = None;
    kv_row(ui, theme, label, |ui| {
        crate::shell::components::combo_box(theme, Id::new(("shell/region/properties", salt)))
            .width(ui.available_width())
            .selected_text((shown).to_string())
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

fn refresh_button(ui: &mut Ui, theme: &Theme) -> bool {
    ui.button(small_text(theme, tr("refresh list")))
        .clicked()
}

/// `capture`'s page: which camera, screen or window it shows.
pub(crate) fn capture_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
) {
    let refresh = refresh_button(ui, theme);
    let io = io_listing(ui, door, refresh);
    let choices: Vec<(String, String)> = io["video"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some((
                s["id"].as_str()?.to_string(),
                s["label"].as_str()?.to_string(),
            ))
        })
        .collect();
    let current = node["config"]["source"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if let Some(pick) = pick_row(
        ui,
        theme,
        "capture-source",
        tr("source"),
        &current,
        &choices,
    ) {
        post_text(door, gid, node_id, json!({ "source": pick }));
    }
    if current.starts_with("window:") {
        note(ui, theme, tr("the first window capture asks the screen-share picker once: choose \"Dynamic Cast Target\"."));
    }
    if current == "gphoto:" {
        note(ui, theme, tr("camera on, USB mode PC Remote; stop launch-camera first, only one program can hold it."));
    }
}

/// 2026-10-01: `reel.batch`'s page — its list (videos, stills, saved or
/// favourite scenes, whole favourites groups) and how it steps. Every edit
/// writes the whole `items` array back through the node's config; the floats
/// (`interval`, `unit`, `order`, `seed`) are the same keys the parameters page
/// holds. Scenes and groups come off `GET /reel/scenes`.
pub(crate) fn reel_batch_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
) {
    let config = &node["config"];
    let items: Vec<Value> = config["items"].as_array().cloned().unwrap_or_default();

    // How it steps.
    let unit = if config["unit"].as_f64().unwrap_or(0.0) >= 0.5 {
        "1"
    } else {
        "0"
    };
    let units = vec![
        ("0".to_string(), tr("seconds").to_string()),
        ("1".to_string(), tr("beats").to_string()),
    ];
    if let Some(pick) = pick_row(ui, theme, "reel-batch-unit", tr("unit"), unit, &units) {
        post_text(
            door,
            gid,
            node_id,
            json!({ "unit": pick.parse::<f32>().unwrap_or(0.0) }),
        );
    }
    let order = match config["order"].as_f64().unwrap_or(0.5) {
        o if o < 0.25 => "0",
        o if o < 0.75 => "0.5",
        _ => "1",
    };
    let orders = vec![
        ("0".to_string(), tr("sequential").to_string()),
        ("0.5".to_string(), tr("loop").to_string()),
        ("1".to_string(), tr("random").to_string()),
    ];
    if let Some(pick) = pick_row(ui, theme, "reel-batch-order", tr("order"), order, &orders) {
        post_text(
            door,
            gid,
            node_id,
            json!({ "order": pick.parse::<f32>().unwrap_or(0.5) }),
        );
    }
    let mut interval = config["interval"].as_f64().unwrap_or(8.0) as f32;
    let r = kv_row(ui, theme, tr("interval"), |ui| {
        float_row(
            ui,
            &mut interval,
            0.05..=600.0,
            Float::Drag {
                speed: 0.1,
                decimals: None,
            },
        )
    });
    if r.changed() {
        post_text(door, gid, node_id, json!({ "interval": interval }));
    }

    // The list.
    ui.separator();
    if items.is_empty() {
        note(
            ui,
            theme,
            tr("the list is empty: add videos, stills, scenes or a group below."),
        );
    }
    let mut edited: Option<Vec<Value>> = None;
    for (i, item) in items.iter().enumerate() {
        let kind = item["kind"].as_str().unwrap_or("?");
        let what = item["path"]
            .as_str()
            .or(item["id"].as_str())
            .or(item["name"].as_str())
            .unwrap_or("");
        let key = format!("{}. {}", i + 1, tr_dyn(kind));
        kv_wrap(ui, theme, &key, |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(what)
                        .font(theme.font_small.clone())
                        .color(theme.ink),
                )
                .wrap(),
            );
            if i > 0 && ui.small_button(lucide(Icon::ArrowUp, theme.font_small.size)).clicked() {
                let mut v = items.clone();
                v.swap(i, i - 1);
                edited = Some(v);
            }
            if i + 1 < items.len() && ui.small_button(lucide(Icon::ArrowDown, theme.font_small.size)).clicked() {
                let mut v = items.clone();
                v.swap(i, i + 1);
                edited = Some(v);
            }
            if ui.small_button(tr("remove")).clicked() {
                let mut v = items.clone();
                v.remove(i);
                edited = Some(v);
            }
        });
    }

    // Adding: a path, a scene, a group.
    ui.separator();
    let path_id = Id::new(("shell/properties/reel-batch-path", node_id));
    let mut path: String = ui.ctx().data(|d| d.get_temp(path_id)).unwrap_or_default();
    let mut still = false;
    kv_row(ui, theme, tr("path"), |ui| {
        ui.add(egui::TextEdit::singleline(&mut path).desired_width(ui.available_width()))
    });
    kv_wrap(ui, theme, "", |ui| {
        let trimmed = path.trim().to_string();
        still = ["jpg", "jpeg", "png", "webp"]
            .iter()
            .any(|e| trimmed.to_lowercase().ends_with(&format!(".{e}")));
        if ui.small_button(tr("add video")).clicked() && !trimmed.is_empty() {
            let mut v = edited.clone().unwrap_or_else(|| items.clone());
            v.push(json!({ "kind": "video", "path": trimmed }));
            edited = Some(v);
            path.clear();
        } else if ui.small_button(tr("add still")).clicked() && !trimmed.is_empty() {
            let mut v = edited.clone().unwrap_or_else(|| items.clone());
            v.push(json!({ "kind": "image", "path": trimmed }));
            edited = Some(v);
            path.clear();
        }
    });
    if still {
        note(ui, theme, tr("stills hold for the interval."));
    }
    ui.ctx().data_mut(|d| d.insert_temp(path_id, path));

    let refresh = refresh_button(ui, theme);
    let listing = scenes_listing(ui, door, refresh);
    let scenes: Vec<(String, String)> = listing["scenes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let id = s["id"].as_str()?.to_string();
            let name = s["name"]
                .as_str()
                .filter(|n| !n.is_empty())
                .unwrap_or(&id)
                .to_string();
            Some((id, name))
        })
        .collect();
    let groups: Vec<(String, String)> = listing["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| {
            let name = g["name"].as_str()?.to_string();
            Some((
                name.clone(),
                format!("{name} ({})", g["count"].as_u64().unwrap_or(0)),
            ))
        })
        .collect();
    if let Some(id) = pick_row(ui, theme, "reel-batch-scene", tr("add scene"), "", &scenes) {
        let mut v = edited.clone().unwrap_or_else(|| items.clone());
        v.push(json!({ "kind": "scene", "id": id }));
        edited = Some(v);
    }
    if let Some(name) = pick_row(ui, theme, "reel-batch-group", tr("add group"), "", &groups) {
        let mut v = edited.clone().unwrap_or_else(|| items.clone());
        v.push(json!({ "kind": "group", "name": name }));
        edited = Some(v);
    }
    if scenes.is_empty() && groups.is_empty() {
        note(ui, theme, tr("no saved scenes or favourite groups yet."));
    } else {
        note(
            ui,
            theme,
            tr("a group is followed live: edit it and the batch plays the new list."),
        );
    }

    if let Some(v) = edited {
        post_text(door, gid, node_id, json!({ "items": v }));
    }
}

/// `GET /reel/scenes`, cached like [`io_listing`].
fn scenes_listing(ui: &Ui, door: &dyn Call, refresh: bool) -> Value {
    let id = Id::new("shell/properties/reel-scenes");
    let now = ui.input(|i| i.time);
    let cached: Option<(Value, f64)> = ui.ctx().data(|d| d.get_temp(id));
    if let Some((v, _)) = cached.filter(|(_, at)| !refresh && now - at < LIST_TTL) {
        return v;
    }
    let v = get(door, "GET", "/reel/scenes", &[]).unwrap_or_else(|| json!({}));
    ui.ctx().data_mut(|d| d.insert_temp(id, (v.clone(), now)));
    v
}

/// A source's **reaction** page (plan A3.1): how it reacts to the audio. What
/// it paints is the drive dials [`listen::reaction_section`]; the trained
/// half (the embedded `ai.*` toggle) is retired with the `ai.colourist` node.
pub(crate) fn reaction_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
) {
    let kind = node["kind"].as_str().unwrap_or_default();
    // The `modo` (AI / AI FX / manual) now lives in the properties node card
    // (2026-10-08); this section keeps only the drive dials and the embedded
    // colourist controls it folds into Parameters.
    // The deterministic half stays the source's own drive dials, which only a
    // `gen.*`/`text.source` reads; a `reel.*`/`router` source gets only the
    // trained half below.
    if kind.starts_with("gen.") || kind == metal_graph::TEXT_SOURCE_KIND {
        listen::reaction_section(ui, theme, door, gid, node_id, node, graph);
    }
}
