//! The Colourist ingest lane's floats (relocated from `panels::lanes`,
//! 22-sept-2026). Shows nothing when no model is picked in the ingest lane yet.
//! Once a model is set, fetches that model's own schema entry and hands its
//! `blocks` to [`properties_panel`], editing [`crate::shell::lanes::IngestLane::row`]
//! directly — the row a keyframe commit files, so the sliders and the keyframe
//! are the same object.

use crate::shell::components::small_text;
use egui::{RichText, Ui};
use metal_notation::color::Hsb;
use serde_json::Value;

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{colour_button, float_row, kv_row, kv_wrap, note, num_value, Float};
use crate::shell::i18n::{tr, tr_dyn, trf};


pub(crate) fn section(ui: &mut Ui, theme: &Theme, door: &dyn Call, ingest: &mut crate::shell::lanes::IngestLane) {
    // The song's own voice block at the playhead, before the model's floats:
    // the tick page is where a tick's numbers are read, and the voice columns
    // are read the same way the music ones are — one named value per column.
    if let Some(matrix) = ingest.matrix.as_ref() {
        voice_block(ui, theme, matrix, ingest.playhead);
    }
    if ingest.model.is_empty() {
        return;
    }
    ui.add_space(theme.section_gap);
    crate::shell::components::section_heading(ui, theme, tr("colourist — floats"));
    // Where these floats are: the track, the tick, and whether the tick is
    // a keypoint or eased between two — what `set keypoint` would file.
    let at = ingest.playhead;
    let keyed = ingest.keyframes.contains_key(&at);
    kv_row(ui, theme, tr("track"), |ui| {
        ui.label(
            RichText::new(trf("track {n} · {model} · tick {t}", &[("n", &(ingest.active + 1)), ("model", &ingest.model), ("t", &at)]))
                .font(theme.font_small.clone())
                .color(theme.ink),
        )
    });
    kv_row(ui, theme, tr("keypoint"), |ui| {
        ui.label(
            RichText::new(if keyed { tr("keypoint") } else { tr("eased between keypoints") })
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        )
    });
    kv_wrap(ui, theme, "", |ui| {
        if ui.button(small_text(theme, tr("set keypoint"))).clicked() {
            ingest.write_keyframe();
        }
        if keyed && ui.button(small_text(theme, tr("remove keypoint"))).clicked() {
            ingest.clear_keyframe();
        }
    });
    let entry = crate::shell::reads::get_shared(door, "/author/models/state", &[]).and_then(|v| {
        v["models"]
            .as_array()?
            .iter()
            .find(|m| m["id"].as_str() == Some(ingest.model.as_str()))
            .cloned()
    });
    match entry {
        Some(entry) => {
            let blocks = entry["blocks"].as_array().cloned().unwrap_or_default();
            properties_panel(ui, theme, &blocks, &mut ingest.row);
        }
        None => {
            note(ui, theme, &trf("— {model} is not in the catalog —", &[("model", &ingest.model)]));
        }
    }
}

/// The song's voice block at `tick`: the thirteen `metal_schema::VOICE_COLUMNS`
/// named, with the digest's own value at that tick beside each. A 16-wide
/// digest that never measured a voice says so instead of listing thirteen
/// zeros (the same "not digested for voice" the timeline draws).
pub(crate) fn voice_block(
    ui: &mut Ui,
    theme: &Theme,
    matrix: &crate::shell::lanes::DigestMatrix,
    tick: usize,
) {
    ui.add_space(theme.section_gap);
    crate::shell::components::section_heading(ui, theme, tr("voice block"));
    if matrix.base_only {
        // The one honest loss of the migration: the only digest is an 8 Hz
        // file with no canonical sibling, held as steps at the fine level.
        ui.add(egui::Label::new(small_text(theme, tr("base-only")).color(theme.warn)).wrap());
    }
    if !matrix.has_voice {
        note(ui, theme, tr("not digested for voice"));
        return;
    }
    let row = matrix.voice.get(tick);
    for (i, name) in metal_schema::VOICE_COLUMNS.iter().enumerate() {
        let value = row.map(|r| r[i]).unwrap_or(0.0);
        kv_row(ui, theme, &tr_dyn(name), |ui| {
            ui.label(small_text(theme, format!("{value:.2}")).color(theme.ink))
        });
    }
}

/// One slider per float, four colour pickers per colour slot, six drag fields
/// plus an fov slider per camera — `page/suite.js`'s `panel()` restated: *"a
/// slider per float... no per-model UI: adding a float to a `Schema` is what
/// makes its slider appear"*. `blocks` is `GET /author/models/state`'s own
/// JSON, read as `Value` rather than the typed `schema::Block` — this section
/// only ever needs it for one frame's painting.
pub(crate) fn properties_panel(ui: &mut Ui, theme: &Theme, blocks: &[Value], row: &mut Vec<f32>) {
    let mut offset = 0usize;
    for block in blocks {
        match block["type"].as_str().unwrap_or_default() {
            "floats" => {
                for f in block["fields"].as_array().cloned().unwrap_or_default() {
                    let Some(name) = f["name"].as_str() else {
                        continue;
                    };
                    let min = f["min"].as_f64().unwrap_or(0.0) as f32;
                    let max = f["max"].as_f64().unwrap_or(1.0) as f32;
                    if offset >= row.len() {
                        row.resize(offset + 1, f["default"].as_f64().unwrap_or(0.0) as f32);
                    }
                    let default = f["default"].as_f64().unwrap_or(0.0) as f32;
                    kv_row(ui, theme, name, |ui| {
                        let w = ui.available_width();
                        num_value(ui, theme, w, &mut row[offset], min..=max, "", 2, default, true)
                    });
                    offset += 1;
                }
            }
            "color_slot" => {
                let name = block["name"].as_str().unwrap_or("color");
                if offset + 11 >= row.len() {
                    row.resize(offset + 12, 1.0);
                }
                kv_wrap(ui, theme, name, |ui| {
                    for c in 0..4 {
                        let at = offset + c * 3;
                        let mut hsb = Hsb::from_rgb(row[at], row[at + 1], row[at + 2]);
                        let size = egui::vec2(theme.control_width, ui.spacing().interact_size.y);
                        if ui.allocate_ui(size, |ui| colour_button(ui, theme, &mut hsb)).inner.changed() {
                            let [r, g, b] = hsb.to_rgb();
                            row[at] = r;
                            row[at + 1] = g;
                            row[at + 2] = b;
                        }
                    }
                });
                offset += 12;
            }
            "camera" => {
                if offset + 6 >= row.len() {
                    row.resize(offset + 7, 0.0);
                }
                for (key, base) in [(tr("camera · position"), offset), (tr("camera · target"), offset + 3)] {
                    kv_wrap(ui, theme, key, |ui| {
                        for k in 0..3 {
                            float_row(ui, &mut row[base + k], -10.0..=10.0, Float::Drag { speed: 0.02, decimals: None });
                        }
                    });
                }
                kv_row(ui, theme, tr("camera · fov"), |ui| {
                    let w = ui.available_width();
                    num_value(ui, theme, w, &mut row[offset + 6], 10.0..=120.0, "", 0, 60.0, true)
                });
                offset += 7;
            }
            _ => {}
        }
    }
}
