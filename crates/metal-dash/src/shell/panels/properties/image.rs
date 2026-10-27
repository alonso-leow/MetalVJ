//! Parity P-A: the settings of the image nodes that are not floats — a
//! chroma key's colour, a lookup's palette or `.cube`, a math node's
//! swizzle, a projection's layouts, a ramp's shape and palette. The floats
//! themselves are on Parameters like every effect's.
//!
//! **One writer.** Every control posts `POST /graph/node {op:"config"}` —
//! the same merge the AI's `set_node_config`/graph tools reach (rule 6) —
//! and reads back off `GET /graph`'s own `node.config` (rule 4).

use crate::shell::components::small_text;
use egui::Ui;
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{label_row, segmented};
use crate::shell::i18n::{tr, tr_key, trf};
use crate::shell::preview;

use super::text::post_text;

/// The section key, for `own_sections`.
pub(crate) const SECTION: &str = tr_key("image settings");

/// Whether `kind` has this page.
pub(crate) fn has_page(kind: &str) -> bool {
    metal_graph::parity::is_parity_effect(kind) || kind == metal_graph::RAMP_KIND
}

const LAYOUTS: [&str; 3] = [tr_key("equirect"), tr_key("cube cross"), tr_key("fisheye")];
const SHAPES: [&str; 4] = [tr_key("linear"), tr_key("radial"), tr_key("circular"), tr_key("diamond")];
const CHANNELS: [&str; 5] = [tr_key("luma"), "R", "G", "B", "A"];
const LEVEL_CHANNELS: [&str; 4] = [tr_key("all"), "R", "G", "B"];

/// A float config key as a pick among `labels` (index = value).
fn pick(ui: &mut Ui, theme: &Theme, door: &dyn Call, ids: (&str, &str), cfg: &Value, key: &str, label: &str, labels: &[&'static str], translate: bool) {
    let mut v = cfg[key].as_f64().unwrap_or(0.0).round() as usize;
    let options: Vec<(usize, String)> =
        labels.iter().enumerate().map(|(i, l)| (i, if translate && l.len() > 1 { tr(l).to_string() } else { l.to_string() })).collect();
    let options: Vec<(usize, &str)> = options.iter().map(|(i, s)| (*i, s.as_str())).collect();
    if label_row(ui, theme, label, |ui| segmented(ui, theme, &mut v, &options)).changed() {
        post_text(door, ids.0, ids.1, json!({ key: v as f64 }));
    }
}

/// A string config key as a one-line field; empty removes it.
fn text(ui: &mut Ui, theme: &Theme, door: &dyn Call, ids: (&str, &str), cfg: &Value, key: &str, label: &str, hint: &str) {
    let mut s = cfg[key].as_str().unwrap_or_default().to_string();
    let r = label_row(ui, theme, label, |ui| {
        ui.add(egui::TextEdit::singleline(&mut s).hint_text(hint).desired_width(theme.control_width))
    });
    if r.changed() {
        let v = if s.trim().is_empty() { Value::Null } else { json!(s.trim()) };
        post_text(door, ids.0, ids.1, json!({ key: v }));
    }
}

/// The chroma key's eyedropper: the button arms the preview for this node;
/// the next click on a picture writes its colour into `key` through the same
/// `POST /graph/node` config merge the text field and the AI use.
fn eyedropper(ui: &mut Ui, theme: &Theme, door: &dyn Call, ids: (&str, &str)) {
    let ctx = ui.ctx().clone();
    let me = (ids.0.to_string(), ids.1.to_string());
    let armed = preview::pick_target(&ctx).as_ref() == Some(&me);
    if armed {
        if let Some([r, g, b]) = preview::take_picked(&ctx) {
            post_text(door, ids.0, ids.1, json!({ "key": format!("#{r:02x}{g:02x}{b:02x}") }));
            preview::set_pick_target(&ctx, None);
        }
    }
    let armed = preview::pick_target(&ctx).as_ref() == Some(&me);
    let label = if armed { tr("click the preview") } else { tr("pick key colour") };
    if ui.button(small_text(theme, label)).clicked() {
        preview::set_pick_target(&ctx, if armed { None } else { Some(me) });
    }
}

pub(crate) fn section(ui: &mut Ui, theme: &Theme, door: &dyn Call, gid: &str, node_id: &str, node: &Value) {
    let cfg = &node["config"];
    let kind = node["kind"].as_str().unwrap_or_default();
    let ids = (gid, node_id);
    use metal_graph::parity as p;
    match kind {
        p::CHROMA_KEY_KIND => {
            text(ui, theme, door, ids, cfg, "key", tr("key colour"), "#00ff00");
            eyedropper(ui, theme, door, ids);
        }
        p::LEVEL_KIND => pick(ui, theme, door, ids, cfg, "channel", tr("channel"), &LEVEL_CHANNELS, true),
        p::LOOKUP_KIND => {
            pick(ui, theme, door, ids, cfg, "source", tr("source"), &[tr_key("palette"), tr_key("cube file")], true);
            if cfg["source"].as_f64().unwrap_or(0.0) >= 0.5 {
                text(ui, theme, door, ids, cfg, "cube", tr("cube file"), "luts/look.cube");
            } else {
                text(ui, theme, door, ids, cfg, "palette", tr("palette"), "");
                pick(ui, theme, door, ids, cfg, "channel", tr("channel"), &CHANNELS, true);
            }
        }
        p::MATH_KIND => text(ui, theme, door, ids, cfg, "swizzle", tr("swizzle"), "rgba"),
        p::PROJECTION_KIND => {
            pick(ui, theme, door, ids, cfg, "from", tr("from layout"), &LAYOUTS, true);
            pick(ui, theme, door, ids, cfg, "to", tr("to layout"), &LAYOUTS, true);
        }
        p::RAMP_KIND => {
            pick(ui, theme, door, ids, cfg, "shape", tr("shape"), &SHAPES, true);
            text(ui, theme, door, ids, cfg, "palette", tr("palette"), "");
            let n = cfg["stops"].as_array().map(Vec::len).unwrap_or(0);
            ui.label(
                egui::RichText::new(trf("{n} explicit stops", &[("n", &n)]))
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
            );
        }
        _ => {}
    }
}
