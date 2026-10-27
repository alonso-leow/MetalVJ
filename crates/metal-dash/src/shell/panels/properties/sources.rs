//! 2026-10-01: the three source pages the properties browser lacked.
//!
//! - `audio.live` → **input device**: moved to `listen.rs` (plan "Audio
//!   input is a node": one listener per node, `source` in its config).
//! - `capture` → **capture source**: a camera (the a6400's loopback or
//!   `gphoto2` direct included), a screen, or one program's window, written to
//!   the node's config (`source`) — `metal_perform::capture` opens it.
//! - `ai.colourist` → **colourist**: `AI Auto` (the fitted grade) or `Auto`
//!   (the deterministic round robin), the node's config `mode`.
//!
//! The lists come off `GET /io`, cached for [`LIST_TTL`] in egui memory: the
//! route spawns `pactl`/`niri`, which is not a per-frame cost.

use crate::shell::components::small_text;
use egui::{Id, RichText, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{float_row, kv_row, kv_wrap, lucide, note, Float, Icon};
use crate::shell::i18n::{tr, tr_dyn, trf};

use super::listen;
use super::shared::{get, post};
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

/// `ai.colourist`'s page: AI Auto vs Auto.
pub(crate) fn colourist_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
) {
    let current = match node["config"]["mode"].as_str() {
        Some("auto") => "auto",
        _ => "ai",
    };
    let choices = vec![
        ("ai".to_string(), tr("AI Auto — fitted grade").to_string()),
        (
            "auto".to_string(),
            tr("Auto — deterministic math").to_string(),
        ),
    ];
    if let Some(pick) = pick_row(ui, theme, "colourist-mode", tr("mode"), current, &choices) {
        post_text(door, gid, node_id, json!({ "mode": pick }));
    }
    note(
        ui,
        theme,
        if current == "auto" {
            tr("Auto: each float follows one digest column (rms, bandwidth, onset, centroid…) around its base value — plain math, no model.")
        } else {
            tr("AI Auto: each float follows a fitted map over all sixteen digest columns; with no trained grade it falls back to Auto.")
        },
    );
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

/// The `ai.*` kinds a source offers as an embedded reaction (plan A3.1),
/// mirroring `call::graph::offered_ai_for` — the one place the shell answers
/// it. `gen.*` offers the colourist; a `router` will offer `ai.router` and a
/// `reel.*` the curator when those nodes exist, which is the mapping hook, not
/// an invented name.
fn offered_ai(kind: &str) -> &'static [&'static str] {
    if kind.starts_with("gen.") {
        &[metal_graph::AI_COLOURIST_KIND]
    } else {
        &[]
    }
}

/// A source's **reaction** page (plan A3.1): how it reacts to the audio. The
/// deterministic half is the drive dials [`listen::reaction_section`] already
/// paints (`direct`); the trained half is an embedded `ai.*` (`ai.embed`) the
/// source owns (`trained`). Per offered AI: a toggle, the weights in use,
/// `train` (`POST /ai/train { node }`) and `separate` (the same `ai.embed`
/// op with `on: false`).
///
/// Nothing here is a second state (rule 4): what it shows is read off the
/// `GET /graph` reply the browser already fetched, and every write is the
/// `ai.embed` op through the door (rule 6) — the same route the agent's
/// `embed_ai` tool posts.
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

    let offered = offered_ai(kind);
    if offered.is_empty() {
        return;
    }
    let nodes: Vec<Value> = graph["graph"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let owner = |ai: &str| {
        nodes
            .iter()
            .find(|n| n["embedded_in"].as_str() == Some(node_id) && n["kind"].as_str() == Some(ai))
    };
    let state = if nodes
        .iter()
        .any(|n| n["embedded_in"].as_str() == Some(node_id))
    {
        tr("trained")
    } else {
        tr("direct")
    };

    ui.separator();
    crate::shell::components::field_row(ui, theme, tr("reaction"), state);

    for ai in offered {
        let embedded = owner(ai);
        let mut on = embedded.is_some();
        let label = tr_dyn(ai.strip_prefix("ai.").unwrap_or(ai));
        if ui
            .checkbox(&mut on, small_text(theme, label))
            .changed()
        {
            post(
                door,
                "/graph/node",
                json!({ "op": "ai.embed", "graph": gid, "node": node_id, "ai": ai, "on": on }),
            );
            continue;
        }
        let Some(embedded) = embedded else { continue };
        let ai_node = embedded["id"].as_str().unwrap_or_default().to_string();
        let weights = embedded["config"]["weights"]
            .as_str()
            .filter(|w| !w.is_empty());
        let shown = weights
            .map(str::to_string)
            .unwrap_or_else(|| tr("untrained").to_string());
        note(ui, theme, &trf("weights: {name}", &[("name", &shown)]));
        kv_wrap(ui, theme, "", |ui| {
            if ui
                .button(small_text(theme, tr("train")))
                .clicked()
            {
                post(door, "/ai/train", json!({ "node": ai_node }));
            }
            if ui
                .button(small_text(theme, tr("separate")))
                .clicked()
            {
                post(
                    door,
                    "/graph/node",
                    json!({ "op": "ai.embed", "graph": gid, "node": node_id, "ai": ai, "on": false }),
                );
            }
        });
    }
}
