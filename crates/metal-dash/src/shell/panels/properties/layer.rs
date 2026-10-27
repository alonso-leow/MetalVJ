//! `gen.*`'s composite override (`crate::node_layer::NodeLayerConfig`, read
//! straight off `node["config"]` rather than through that type — the panel only
//! ever reads/writes the JSON shape it already serializes to, the same trade
//! `nodes.rs::effects_panel` makes for `state`/`value`). Each control posts only
//! the field it changed — `POST /graph/node { op: "layer" }` starts from the
//! node's *current* config and only overrides what the body names, so a lone
//! opacity drag can never clobber a sibling's blend or sector.

use egui::Ui;
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{dropdown, label_row, num, num_track};
use crate::shell::i18n::tr;

/// The four blend names `crate::layers::Blend::parse` reads.
pub(crate) const BLENDS: [&str; 4] = ["normal", "add", "multiply", "screen"];

pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
) {
    let cfg = &node["config"];
    let mut blend = cfg["blend"].as_str().unwrap_or("normal").to_string();
    let mut opacity = cfg["opacity"].as_f64().unwrap_or(1.0) as f32;
    let sector = &cfg["sector"];
    let mut sx = sector["x"].as_f64().unwrap_or(0.0) as f32;
    let mut sy = sector["y"].as_f64().unwrap_or(0.0) as f32;
    let mut sw = sector["w"].as_f64().unwrap_or(1.0) as f32;
    let mut sh = sector["h"].as_f64().unwrap_or(1.0) as f32;
    let mut mask = cfg["mask"].as_str().unwrap_or_default().to_string();

    let options: Vec<(String, &str)> = BLENDS.iter().map(|b| (b.to_string(), *b)).collect();
    let picked = blend.clone();
    label_row(ui, theme, tr("blend"), |ui| {
        dropdown(
            ui,
            theme,
            "shell/region/properties/blend",
            &mut blend,
            &options,
        )
    });
    if blend != picked {
        post_layer(door, gid, node_id, json!({ "blend": blend }));
    }

    let mut z = cfg["z"].as_i64().unwrap_or(0) as i32;
    let mut zf = z as f32;
    if num(ui, theme, tr("z"), &mut zf, -64.0..=64.0, "", 0, 0.0, true).changed() {
        z = zf.round() as i32;
        post_layer(door, gid, node_id, json!({ "z": z }));
    }

    if num(ui, theme, tr("opacity"), &mut opacity, 0.0..=1.0, "", 2, 1.0, true).changed() {
        post_layer(door, gid, node_id, json!({ "opacity": opacity }));
    }

    let mut sector_changed = false;
    label_row(ui, theme, tr("sector"), |ui| {
        sector_changed |= num_track(ui, theme, &mut sx, 0.0..=1.0, "", 2, 0.0, true).changed();
        sector_changed |= num_track(ui, theme, &mut sy, 0.0..=1.0, "", 2, 0.0, true).changed();
        sector_changed |= num_track(ui, theme, &mut sw, 0.0..=1.0, "", 2, 1.0, true).changed();
        num_track(ui, theme, &mut sh, 0.0..=1.0, "", 2, 1.0, true)
    });
    if sector_changed {
        post_layer(
            door,
            gid,
            node_id,
            json!({ "sector": { "x": sx, "y": sy, "w": sw, "h": sh } }),
        );
    }

    if label_row(ui, theme, tr("mask"), |ui| {
        ui.add(egui::TextEdit::singleline(&mut mask).desired_width(theme.text_input_width))
    })
    .lost_focus()
    {
        let v = if mask.is_empty() {
            Value::Null
        } else {
            json!(mask)
        };
        post_layer(door, gid, node_id, json!({ "mask": v }));
    }
}

/// `POST /graph/node { op: "layer" }` with the changed field merged over the
/// node's current config server-side.
pub(crate) fn post_layer(door: &dyn Call, gid: &str, node_id: &str, mut fields: Value) {
    let body = fields
        .as_object_mut()
        .expect("json!({...}) is always an object");
    body.insert("op".to_string(), json!("layer"));
    body.insert("graph".to_string(), json!(gid));
    body.insert("node".to_string(), json!(node_id));
    door.post_async("/graph/node", Value::Object(body.clone()));
}
