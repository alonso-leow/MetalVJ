//! `text.source`'s render parameters and the `fx.text.*` effect's one knob.
//! Every field posts through the same `POST /graph/node {op:"config"}` merge op,
//! so setting the size never disturbs the string. A field the node has not
//! written yet reads its default, the same one `metal_perform::text::TextConfig`
//! would use.

use egui::{RichText, Ui};
use metal_notation::color::Hsb;
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{colour_row, dropdown, label_row, number_row, slider_row, toggle_row};
use crate::shell::i18n::tr;

/// `POST /graph/node {op:"config"}` with the given config fields merged.
pub(crate) fn post_text(door: &dyn Call, gid: &str, node_id: &str, fields: Value) {
    let body = json!({ "op": "config", "graph": gid, "node": node_id, "config": fields });
    door.post_async("/graph/node", body);
}

/// The `fx.text.*` node's one knob: how much of it, living in the node's
/// config (`metal_perform::text` reads `amount`, defaulting to one).
pub(crate) fn effect_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
) {
    let mut amount = node["config"]["amount"].as_f64().unwrap_or(1.0) as f32;
    if slider_row(ui, theme, tr("amount"), &mut amount, 0.0..=2.0, true).changed() {
        post_text(door, gid, node_id, json!({ "amount": amount }));
    }
}

pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
) {
    const FAMILIES: [&str; 5] = ["sans-serif", "serif", "monospace", "cursive", "fantasy"];
    const ALIGNS: [&str; 3] = ["left", "center", "right"];

    let cfg = &node["config"];

    let mut content = cfg["content"].as_str().unwrap_or("MetalVJ").to_string();
    if label_row(ui, theme, tr("text"), |ui| {
        ui.add(egui::TextEdit::singleline(&mut content).desired_width(theme.control_width))
    })
    .changed()
    {
        post_text(door, gid, node_id, json!({ "content": content }));
    }

    let mut family = cfg["family"].as_str().unwrap_or("sans-serif").to_string();
    let families: Vec<(String, &str)> = FAMILIES.iter().map(|f| (f.to_string(), *f)).collect();
    let before = family.clone();
    label_row(ui, theme, tr("font"), |ui| {
        dropdown(
            ui,
            theme,
            "shell/region/properties/font",
            &mut family,
            &families,
        )
    });
    if family != before {
        post_text(door, gid, node_id, json!({ "family": family }));
    }

    let mut size = cfg["size"].as_f64().unwrap_or(0.18) as f32;
    if slider_row(ui, theme, tr("size"), &mut size, 0.02..=0.9, true).changed() {
        post_text(door, gid, node_id, json!({ "size": size }));
    }

    let mut tracking = cfg["tracking"].as_f64().unwrap_or(0.0) as f32;
    if slider_row(ui, theme, tr("tracking"), &mut tracking, -0.1..=0.5, true).changed() {
        post_text(door, gid, node_id, json!({ "tracking": tracking }));
    }

    let mut leading = cfg["leading"].as_f64().unwrap_or(1.2) as f32;
    if slider_row(ui, theme, tr("leading"), &mut leading, 0.6..=3.0, true).changed() {
        post_text(door, gid, node_id, json!({ "leading": leading }));
    }

    let mut align = cfg["align"].as_str().unwrap_or("left").to_string();
    let aligns: Vec<(String, &str)> = ALIGNS.iter().map(|a| (a.to_string(), *a)).collect();
    let before = align.clone();
    label_row(ui, theme, tr("align"), |ui| {
        dropdown(
            ui,
            theme,
            "shell/region/properties/align",
            &mut align,
            &aligns,
        )
    });
    if align != before {
        post_text(door, gid, node_id, json!({ "align": align }));
    }

    let mut thsb = Hsb::from_rgb(
        cfg["color"][0].as_f64().unwrap_or(1.0) as f32,
        cfg["color"][1].as_f64().unwrap_or(1.0) as f32,
        cfg["color"][2].as_f64().unwrap_or(1.0) as f32,
    );
    let mut talpha = cfg["color"][3].as_f64().unwrap_or(1.0) as f32;
    let color_changed = colour_row(ui, theme, tr("colour"), &mut thsb).changed();
    let alpha_changed =
        number_row(ui, theme, tr("alpha"), &mut talpha, 0.0..=1.0, "", 2, true).changed();
    if color_changed || alpha_changed {
        let [r, g, b] = thsb.to_rgb();
        post_text(door, gid, node_id, json!({ "color": [r, g, b, talpha] }));
    }

    let mut valign = cfg["valign"].as_str().unwrap_or("top").to_string();
    let valigns: Vec<(String, &str)> = vec![
        ("top".into(), tr("top")),
        ("center".into(), tr("center")),
        ("bottom".into(), tr("bottom")),
    ];
    let before = valign.clone();
    label_row(ui, theme, tr("vertical align"), |ui| {
        dropdown(ui, theme, "shell/region/properties/valign", &mut valign, &valigns)
    });
    if valign != before {
        post_text(door, gid, node_id, json!({ "valign": valign }));
    }

    let mut wrap = cfg["line_wrap"].as_bool().unwrap_or(true);
    if toggle_row(ui, theme, tr("wrap lines"), &mut wrap).changed() {
        post_text(door, gid, node_id, json!({ "line_wrap": wrap }));
    }

    // Stroke: the outline ring, width in em.
    let stroke = &cfg["stroke"];
    let mut swidth = stroke["width"].as_f64().unwrap_or(0.0) as f32;
    let width_changed =
        slider_row(ui, theme, tr("outline width"), &mut swidth, 0.0..=0.3, true).changed();
    let (mut shsb, mut salpha) = rgba_of(&stroke["color"], [0.0, 0.0, 0.0, 1.0]);
    let colour_changed = colour_row(ui, theme, tr("outline colour"), &mut shsb).changed();
    let alpha_changed =
        number_row(ui, theme, tr("outline alpha"), &mut salpha, 0.0..=1.0, "", 2, true).changed();
    if width_changed || colour_changed || alpha_changed {
        let [r, g, b] = shsb.to_rgb();
        post_text(
            door,
            gid,
            node_id,
            json!({ "stroke": { "width": swidth, "color": [r, g, b, salpha] } }),
        );
    }

    // Border: the box behind the block (padding, colour, corner radius).
    let border = &cfg["border"];
    let mut padding = border["padding"].as_f64().unwrap_or(0.02) as f32;
    let mut radius = border["radius"].as_f64().unwrap_or(0.0) as f32;
    let pad_changed =
        slider_row(ui, theme, tr("box padding"), &mut padding, 0.0..=0.3, true).changed();
    let radius_changed =
        slider_row(ui, theme, tr("box radius"), &mut radius, 0.0..=0.3, true).changed();
    let (mut bhsb, mut balpha) = rgba_of(&border["color"], [0.0, 0.0, 0.0, 0.0]);
    let bcolour_changed =
        colour_row(ui, theme, tr("box colour"), &mut bhsb).changed();
    let balpha_changed =
        number_row(ui, theme, tr("box alpha"), &mut balpha, 0.0..=1.0, "", 2, true).changed();
    if pad_changed || radius_changed || bcolour_changed || balpha_changed {
        let [r, g, b] = bhsb.to_rgb();
        post_text(
            door,
            gid,
            node_id,
            json!({ "border": { "padding": padding, "radius": radius, "color": [r, g, b, balpha] } }),
        );
    }

    ui.label(
        RichText::new(tr("text effects are fx.text.* nodes downstream"))
            .font(theme.font_small.clone())
            .color(theme.ink_weak),
    );
}

/// A config `[r, g, b, a]` as the picker's HSB plus alpha, `fallback` where
/// the node has not written it.
fn rgba_of(value: &Value, fallback: [f32; 4]) -> (Hsb, f32) {
    let c = |i: usize| value[i].as_f64().map(|v| v as f32).unwrap_or(fallback[i]);
    (Hsb::from_rgb(c(0), c(1), c(2)), c(3))
}
