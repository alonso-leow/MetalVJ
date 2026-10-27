//! Parity P-E: a source's **Transition in** / **Transition out** —
//! `config.transition_in/out = { preset, duration, unit, ease }`.
//! The engine plays them when an overlays-grid switch turns the node on/off
//! (`metal_perform::transition`).
//!
//! **One writer.** Every control posts `POST /graph/node {op:"config"}` — the
//! merge the AI's `set_params` reaches (rule 6) — and reads back off
//! `GET /graph`'s `node.config` (rule 4). The preset list is
//! `GET /transitions`, the route the AI's `list_transitions` reads.

use egui::Ui;
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{dropdown, label_row, number_row, segmented};
use crate::shell::i18n::{tr, tr_key};

use super::text::post_text;

pub(crate) const IN_SECTION: &str = tr_key("transition in");
pub(crate) const OUT_SECTION: &str = tr_key("transition out");

/// Whether `kind` is a source that gets the two slots.
pub(crate) fn has_slots(kind: &str) -> bool {
    kind.starts_with("gen.")
        || kind.starts_with("reel.")
        || kind == metal_graph::TEXT_SOURCE_KIND
        || kind == "still"
}

const EASES: [(&str, &str); 4] = [
    ("linear", tr_key("linear ease")),
    ("in", tr_key("ease in")),
    ("out", tr_key("ease out")),
    ("in_out", tr_key("ease in-out")),
];

/// How long the cached preset list is trusted, in seconds.
const PRESETS_TTL: f64 = 2.0;

/// `(id, name, duration, unit)` per preset, off `GET /transitions`, cached in
/// egui's temp memory.
fn presets(ui: &mut Ui, door: &dyn Call) -> Vec<(String, String, f64, String)> {
    let id = egui::Id::new("transition-presets");
    let now = ui.input(|i| i.time);
    let cached: Option<(f64, Value)> = ui.data(|d| d.get_temp(id));
    let list = match cached {
        Some((at, v)) if now - at < PRESETS_TTL => v,
        _ => {
            let v = door.json("GET", "/transitions", None).unwrap_or_else(|_| json!({}));
            ui.data_mut(|d| d.insert_temp(id, (now, v.clone())));
            v
        }
    };
    list["presets"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| {
                    (
                        p["id"].as_str().unwrap_or_default().to_string(),
                        p["name"].as_str().unwrap_or_default().to_string(),
                        p["duration"].as_f64().unwrap_or(1.0),
                        p["unit"].as_str().unwrap_or("beats").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One slot's page. `on` = the in-transition.
pub(crate) fn section(ui: &mut Ui, theme: &Theme, door: &dyn Call, gid: &str, node_id: &str, node: &Value, on: bool) {
    let key = if on { metal_perform::transition::IN_KEY } else { metal_perform::transition::OUT_KEY };
    let slot = &node["config"][key];
    let list = presets(ui, door);

    let mut chosen = slot["preset"].as_str().unwrap_or_default().to_string();
    let mut options: Vec<(String, &str)> = vec![(String::new(), tr("cut (none)"))];
    options.extend(list.iter().map(|(id, name, _, _)| (id.clone(), name.as_str())));
    let before = chosen.clone();
    label_row(ui, theme, tr("preset"), |ui| dropdown(ui, theme, ("transition-preset", node_id, on), &mut chosen, &options));
    if chosen != before {
        // A new preset starts from its own duration/unit/ease; none is a cut.
        let v = if chosen.is_empty() { Value::Null } else { json!({ "preset": chosen }) };
        post_text(door, gid, node_id, json!({ key: v }));
        return;
    }
    if !slot.is_object() {
        return;
    }
    let preset = list.iter().find(|(id, ..)| *id == chosen);
    let post = |field: &str, v: Value| {
        let mut s = slot.clone();
        s[field] = v;
        post_text(door, gid, node_id, json!({ key: s }));
    };

    let mut unit = slot["unit"].as_str().map(str::to_string).or(preset.map(|p| p.3.clone())).unwrap_or_else(|| "beats".into());
    let units = [("beats".to_string(), tr("beats")), ("seconds".to_string(), tr("seconds"))];
    if label_row(ui, theme, tr("unit"), |ui| segmented(ui, theme, &mut unit, &units)).changed() {
        post("unit", json!(unit));
    }

    let mut dur = slot["duration"].as_f64().or(preset.map(|p| p.2)).unwrap_or(1.0) as f32;
    let suffix = if unit == "seconds" { tr("s") } else { tr("beats") };
    if number_row(ui, theme, tr("duration"), &mut dur, 0.0..=32.0, suffix, 2, true).changed() {
        post("duration", json!(dur));
    }

    let mut ease = slot["ease"].as_str().unwrap_or("in_out").to_string();
    let eases: Vec<(String, &str)> = EASES.iter().map(|(id, l)| (id.to_string(), tr(l))).collect();
    let before = ease.clone();
    label_row(ui, theme, tr("ease"), |ui| dropdown(ui, theme, ("transition-ease", node_id, on), &mut ease, &eases));
    if ease != before {
        post("ease", json!(ease));
    }
}
