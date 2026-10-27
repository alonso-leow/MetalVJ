//! `P5`: a focused overlay sequence — destination and automatic trigger. Read
//! off `GET /state`'s per-screen `triggers` (rule 5 — the config is a read of
//! the graph, never a second state); every write is `POST /outputs
//! {op:"trigger"}` / `POST /trigger/<id>` / `POST /overlay`.

use crate::shell::components::small_text;
use egui::{Id, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{kv_row, kv_wrap, note};
use crate::shell::i18n::{tr, trf};
use crate::shell::media;

use super::shared::get;
use super::PanelState;

fn overlay_target(name: &str) -> String {
    format!("overlay-{name}")
}

/// A span bound, rounded for a scene row. Kept out of the label so the label
/// itself is one translatable literal with only `{a}`/`{b}` replaced.
fn span_seconds(v: f64) -> String {
    format!("{v:.2}")
}

/// The device and `params` of the trigger whose `target` is this overlay.
fn find_overlay_trigger(outputs: &[Value], target: &str) -> Option<(String, Value)> {
    for o in outputs {
        if let Some(t) = o["triggers"]
            .as_array()
            .and_then(|ts| ts.iter().find(|t| t["target"] == json!(target)))
        {
            let dev = o["id"].as_str().unwrap_or_default().to_string();
            return Some((dev, t["params"].clone()));
        }
    }
    None
}

fn write_overlay_trigger(
    door: &dyn Call,
    device: &str,
    layer: &str,
    looping: bool,
    id: &str,
    name: &str,
) {
    let _ = door.json(
        "POST",
        "/outputs",
        Some(json!({
            "op": "trigger",
            "device": device,
            "trigger": {
                "id": id,
                "target": overlay_target(name),
                "params": { "device": device, "layer": layer, "loop": looping },
            },
        })),
    );
}

pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    name: &str,
) {
    crate::shell::components::section_heading(
        ui,
        theme,
        &trf("sequence · {name}", &[("name", &name)]),
    );

    if let Some(listing) = get(door, "GET", "/author/models/library", &[]) {
        if let Some(m) = listing["movements"]
            .as_array()
            .and_then(|ms| ms.iter().find(|m| m["name"] == json!(name)))
        {
            let kind = m["kind"].as_str().unwrap_or("?");
            let ticks = m["ticks"].as_u64().unwrap_or(0);
            let star = if m["favourite"].as_bool().unwrap_or(false) {
                " ★"
            } else {
                ""
            };
            note(ui, theme, &format!("{kind} · {ticks} ticks{star}"));
        }
    }

    let state_val = get(door, "GET", "/state", &[]).unwrap_or_else(|| json!({}));
    let outputs = state_val["outputs"].as_array().cloned().unwrap_or_default();
    let target = overlay_target(name);
    let found = find_overlay_trigger(&outputs, &target);
    let dest_device = found
        .as_ref()
        .map(|(d, _)| d.clone())
        .filter(|d| !d.is_empty())
        .or_else(|| {
            outputs
                .first()
                .and_then(|o| o["id"].as_str().map(str::to_string))
        })
        .unwrap_or_default();
    let params = found
        .as_ref()
        .map(|(_, p)| p.clone())
        .unwrap_or_else(|| json!({}));
    let dest_layers: Vec<String> = outputs
        .iter()
        .find(|o| o["id"] == json!(dest_device))
        .and_then(|o| o["layers"].as_array().cloned())
        .map(|ls| {
            ls.iter()
                .filter_map(|l| l["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let cur_layer = params["layer"]
        .as_str()
        .filter(|l| dest_layers.iter().any(|x| x == l))
        .map(str::to_string)
        .or_else(|| dest_layers.first().cloned())
        .unwrap_or_default();
    let cur_loop = params["loop"].as_bool().unwrap_or(false);

    if state.overlay_for.as_deref() != Some(name) {
        state.overlay_for = Some(name.to_string());
        let existing = outputs.iter().find_map(|o| {
            o["triggers"].as_array().and_then(|ts| {
                ts.iter()
                    .find(|t| t["target"] == json!(target))
                    .and_then(|t| t["id"].as_str().map(str::to_string))
            })
        });
        state.overlay_trigger_id = existing.unwrap_or_else(|| format!("seq-{name}"));
        state.overlay_note = None;
    }

    ui.add_space(theme.section_gap);
    crate::shell::components::section_heading(ui, theme, tr("destination"));
    let mut new_device = dest_device.clone();
    let mut new_layer = cur_layer.clone();
    let mut new_loop = cur_loop;
    kv_row(ui, theme, tr("on"), |ui| {
        crate::shell::components::combo_box(theme, Id::new("shell/region/properties/overlay-device"))
            .width(ui.available_width())
            .selected_text((dest_device.clone()).to_string())
            .show_ui(ui, |ui| {
                for o in &outputs {
                    let Some(id) = o["id"].as_str() else {
                        continue;
                    };
                    if ui.selectable_label(id == dest_device, id).clicked() {
                        new_device = id.to_string();
                        new_layer = o["layers"]
                            .as_array()
                            .and_then(|ls| ls.first())
                            .and_then(|l| l["id"].as_str())
                            .map(str::to_string)
                            .unwrap_or_default();
                    }
                }
            })
            .response
    });
    kv_row(ui, theme, tr("layer"), |ui| {
        crate::shell::components::combo_box(theme, Id::new("shell/region/properties/overlay-layer"))
            .width(ui.available_width())
            .selected_text((cur_layer.clone()).to_string())
            .show_ui(ui, |ui| {
                for l in &dest_layers {
                    if ui.selectable_label(l == &cur_layer, l).clicked() {
                        new_layer = l.clone();
                    }
                }
            })
            .response
    });
    kv_wrap(ui, theme, "", |ui| {
        crate::shell::components::toggle_row(ui, theme, tr("loop"), &mut new_loop);
    });
    if new_device != dest_device || new_layer != cur_layer || new_loop != cur_loop {
        write_overlay_trigger(
            door,
            &new_device,
            &new_layer,
            new_loop,
            &state.overlay_trigger_id,
            name,
        );
    }

    ui.add_space(theme.section_gap);
    crate::shell::components::section_heading(ui, theme, tr("trigger"));
    let before = state.overlay_trigger_id.clone();
    kv_row(ui, theme, tr("id"), |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.overlay_trigger_id)
                .desired_width(ui.available_width()),
        )
    });
    {
        if state.overlay_trigger_id != before && !state.overlay_trigger_id.trim().is_empty() {
            let _ = door.json(
                "POST",
                "/outputs",
                Some(json!({ "op": "trigger-remove", "device": dest_device, "id": before })),
            );
            write_overlay_trigger(
                door,
                &new_device,
                &new_layer,
                new_loop,
                &state.overlay_trigger_id,
                name,
            );
        }
    }
    note(
        ui,
        theme,
        &format!("POST /trigger/{}", state.overlay_trigger_id),
    );
    kv_wrap(ui, theme, "", |ui| {
        if ui
            .button(small_text(theme, tr("test")))
            .clicked()
        {
            let _ = door.json(
                "POST",
                &format!("/trigger/{}", state.overlay_trigger_id),
                None,
            );
        }
        if ui
            .button(small_text(theme, tr("remove trigger")))
            .clicked()
        {
            let _ = door.json(
                "POST",
                "/outputs",
                Some(json!({ "op": "trigger-remove", "device": dest_device, "id": state.overlay_trigger_id })),
            );
        }
    });

    ui.add_space(theme.section_gap);
    kv_wrap(ui, theme, "", |ui| {
        let running = state_val["overlays"]
            .as_array()
            .map(|os| os.iter().any(|o| o["movement"] == json!(name)))
            .unwrap_or(false);
        if running {
            if ui
                .button(small_text(theme, tr("stop")))
                .clicked()
            {
                let _ = door.json(
                    "POST",
                    "/overlay",
                    Some(json!({ "op": "stop", "device": new_device, "layer": new_layer })),
                );
            }
        } else if ui
            .button(small_text(theme, tr("play")))
            .clicked()
        {
            let _ = door.json(
                "POST",
                "/overlay",
                Some(json!({
                    "op": "play",
                    "device": new_device,
                    "layer": new_layer,
                    "movement": name,
                    "loop": new_loop,
                })),
            );
        }
    });

    // Scenes picked on the media board (`crate::shell::media`) become an
    // overlay button: `create button` writes the same trigger the focus editor
    // writes above, `fire` fires it the way the `test` button does.
    ui.add_space(theme.section_gap);
    crate::shell::components::section_heading(ui, theme, tr("overlay from scenes"));

    let board = media::board(ui.ctx());
    if board.clips.is_empty() {
        note(ui, theme, tr("pick scenes in a video's properties first"));
    } else {
        for (i, clip) in board.clips.iter().enumerate() {
            // A scene shows its half-open span; a whole file just its name.
            let label = match &clip.scene {
                Some(s) => trf("{name} · {a}–{b}s",
                    &[
                        ("name", &clip.name),
                        ("a", &span_seconds(s.start)),
                        ("b", &span_seconds(s.end)),
                    ],
                ),
                None => clip.name.clone(),
            };
            if ui
                .selectable_label(
                    i == board.cursor,
                    small_text(theme, label),
                )
                .clicked()
            {
                media::set_cursor(ui.ctx(), i);
            }
        }
    }

    // The button's name, seeded from the sequence and kept only for the frame
    // (a draft an operator is mid-typing, like the other text fields here).
    let button_id = Id::new(("shell/properties/overlay-button-name", name));
    let mut button_name = ui
        .ctx()
        .data(|d| d.get_temp::<String>(button_id))
        .unwrap_or_else(|| name.to_string());
    kv_row(ui, theme, tr("button name"), |ui| {
        ui.add(egui::TextEdit::singleline(&mut button_name).desired_width(ui.available_width()))
    });
    kv_wrap(ui, theme, "", |ui| {
        if ui
            .button(small_text(theme, tr("create button")))
            .clicked()
        {
            write_overlay_trigger(
                door,
                &new_device,
                &new_layer,
                new_loop,
                &state.overlay_trigger_id,
                &button_name,
            );
            state.overlay_note = Some(trf("overlay button created: {name}", &[("name", &button_name)]));
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(button_id, button_name));

    let armed = !state.overlay_trigger_id.trim().is_empty();
    if ui
        .add_enabled(
            armed,
            egui::Button::new(small_text(theme, tr("fire"))),
        )
        .clicked()
    {
        let _ = door.json(
            "POST",
            &format!("/trigger/{}", state.overlay_trigger_id),
            None,
        );
    }

    if let Some(msg) = state.overlay_note.clone() {
        note(ui, theme, &msg);
    }
}
