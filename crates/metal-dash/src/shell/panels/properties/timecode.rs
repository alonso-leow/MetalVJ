//! P-D: `clock.timecode`'s panel — which wire (LTC / MTC / Art-Net), its
//! input, the rate, the chase mode and the offset, all written through
//! `POST /graph/node {op:"config"}`; and the live reading off `GET
//! /timecode`, the route the lanes playhead and the AI's `timecode_status`
//! read too (rule 6).

use egui::Ui;
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{dropdown, field_row, label_row, number_row};
use crate::shell::i18n::tr;

use super::text::post_text;

pub(crate) fn section(ui: &mut Ui, theme: &Theme, door: &dyn Call, gid: &str, node_id: &str, node: &Value) {
    let cfg = &node["config"];

    let mut mode = cfg["mode"].as_str().unwrap_or("read").to_string();
    let modes = [("read".to_string(), tr("read")), ("generate".to_string(), tr("generate"))];
    if label_row(ui, theme, tr("direction"), |ui| dropdown(ui, theme, ("tc-mode", node_id), &mut mode, &modes)).changed() {
        post_text(door, gid, node_id, json!({ "mode": mode }));
    }
    let generate = mode == "generate";

    let mut source = cfg["source"].as_str().unwrap_or("ltc").to_string();
    let sources = [
        ("ltc".to_string(), tr("LTC (audio)")),
        ("mtc".to_string(), tr("MTC (MIDI)")),
        ("artnet".to_string(), tr("Art-Net")),
    ];
    if label_row(ui, theme, tr("source"), |ui| dropdown(ui, theme, ("tc-source", node_id), &mut source, &sources))
        .changed()
    {
        post_text(door, gid, node_id, json!({ "source": source }));
    }

    let text_field = |ui: &mut Ui, key: &str, label: &str, hint: &str| {
        let mut text = cfg[key].as_str().unwrap_or_default().to_string();
        if label_row(ui, theme, label, |ui| {
            ui.add(egui::TextEdit::singleline(&mut text).hint_text(hint).desired_width(theme.control_width))
        })
        .changed()
        {
            let value = if text.trim().is_empty() { Value::Null } else { json!(text) };
            post_text(door, gid, node_id, json!({ key: value }));
        }
    };
    match source.as_str() {
        "mtc" if generate => text_field(ui, "midi_out", tr("MIDI output"), tr("any port")),
        "mtc" => text_field(ui, "midi_port", tr("MIDI port"), tr("any port")),
        "artnet" => {}
        _ if generate => text_field(ui, "output", tr("audio output"), tr("default output")),
        _ => {
            text_field(ui, "input", tr("audio input"), tr("default input"));
            let mut ch = cfg["channel"].as_u64().unwrap_or(1).clamp(1, 32) as f32;
            if number_row(ui, theme, tr("channel"), &mut ch, 1.0..=32.0, "", 0, true).changed() {
                post_text(door, gid, node_id, json!({ "channel": ch.round() as u32 }));
            }
        }
    }

    let mut fps = cfg["fps"].as_str().unwrap_or("").to_string();
    let rates = [
        (String::new(), tr("detect")),
        ("24".to_string(), "24"),
        ("25".to_string(), "25"),
        ("29.97df".to_string(), "29.97 DF"),
        ("30".to_string(), "30"),
    ];
    if label_row(ui, theme, tr("frame rate"), |ui| dropdown(ui, theme, ("tc-fps", node_id), &mut fps, &rates))
        .changed()
    {
        let value = if fps.is_empty() { Value::Null } else { json!(fps) };
        post_text(door, gid, node_id, json!({ "fps": value }));
    }

    let mut chase = cfg["chase"].as_str().unwrap_or("follow").to_string();
    let chases = [("follow".to_string(), tr("follow")), ("freewheel".to_string(), tr("freewheel"))];
    if label_row(ui, theme, tr("chase"), |ui| dropdown(ui, theme, ("tc-chase", node_id), &mut chase, &chases))
        .changed()
    {
        post_text(door, gid, node_id, json!({ "chase": chase }));
    }
    if chase == "freewheel" {
        let mut n = cfg["freewheel_frames"].as_u64().unwrap_or(10).min(300) as f32;
        if number_row(ui, theme, tr("freewheel frames"), &mut n, 1.0..=300.0, "", 0, true).changed() {
            post_text(door, gid, node_id, json!({ "freewheel_frames": n.round() as u32 }));
        }
    }
    let mut offset = cfg["offset"].as_f64().unwrap_or(0.0) as f32;
    if number_row(ui, theme, tr("offset"), &mut offset, 0.0..=86_400.0, "s", 2, true).changed() {
        post_text(door, gid, node_id, json!({ "offset": offset }));
    }

    // The live reading: this node's row of `GET /timecode`.
    let reading = door
        .json("GET", "/timecode", None)
        .ok()
        .and_then(|v| v["nodes"].as_array().and_then(|a| a.iter().find(|n| n["node"] == json!(node_id) && n["graph"] == json!(gid)).cloned()));
    let Some(r) = reading else {
        field_row(ui, theme, tr("timecode"), tr("— not read yet —"));
        return;
    };
    let tc = r["timecode"].as_str().unwrap_or("--:--:--:--");
    let state = if generate {
        tr("generating")
    } else if r["locked"].as_bool() == Some(true) {
        tr("locked")
    } else {
        tr("no signal")
    };
    field_row(ui, theme, tr("timecode"), &format!("{tc}  {state}"));
    if let Some(e) = r["error"].as_str() {
        field_row(ui, theme, tr("error"), e);
    }
    if r["locked"].as_bool() == Some(true) {
        ui.ctx().request_repaint();
    }
}
