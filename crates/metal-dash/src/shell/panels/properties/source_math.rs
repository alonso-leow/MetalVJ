//! M1/M2 of the show-control batch (plan "Value sources"): the settings of the
//! `source.*` value nodes that are **not** plain floats — an oscillator's
//! `wave`, a constant's named `constant` and the value noise's `axis`. The
//! floats themselves (freq, amp, seed, …) are on Parameters like every node's,
//! and the names come from `metal_graph::source`, never a second table here.
//!
//! **One writer.** Every control posts `POST /graph/node {op:"config"}` — the
//! same merge the AI's graph tools reach (rule 6) — and reads back off
//! `GET /graph`'s own `node.config` (rule 4).

use egui::Ui;
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{label_row, segmented};
use crate::shell::i18n::{tr, tr_key};

use super::text::post_text;

/// The section key, for `own_sections`.
pub(crate) const SECTION: &str = tr_key("value source");

/// The translation keys of [`metal_graph::source::OSC_WAVES`], index for index.
const WAVE_LABELS: [&str; 4] = [
    tr_key("sine"),
    tr_key("square"),
    tr_key("triangle"),
    tr_key("saw"),
];

/// The labels of [`metal_graph::source::NOISE_AXES`] (M2), index for index.
const AXIS_LABELS: [&str; 4] = [tr_key("t"), tr_key("x"), tr_key("y"), tr_key("z")];

/// Whether `kind` has this page.
pub(crate) fn has_page(kind: &str) -> bool {
    metal_graph::source::is_source_kind(kind)
}

/// The section body for one `source.*` node.
pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
) {
    let cfg = &node["config"];
    let kind = node["kind"].as_str().unwrap_or_default();

    if kind == metal_graph::source::OSC_KIND {
        let mut w = metal_graph::source::OSC_WAVES
            .iter()
            .position(|name| Some(*name) == cfg["wave"].as_str())
            .unwrap_or(0);
        let options: Vec<(usize, String)> = WAVE_LABELS
            .iter()
            .enumerate()
            .map(|(i, l)| (i, tr(l).to_string()))
            .collect();
        let options: Vec<(usize, &str)> = options.iter().map(|(i, s)| (*i, s.as_str())).collect();
        if label_row(ui, theme, tr("wave"), |ui| {
            segmented(ui, theme, &mut w, &options)
        })
        .changed()
        {
            let wave = metal_graph::source::OSC_WAVES
                .get(w)
                .copied()
                .unwrap_or("sine");
            post_text(door, gid, node_id, json!({ "wave": wave }));
        }
    } else if kind == metal_graph::source::CONST_KIND {
        // 0 = the node's own `value`; 1.. names a constant from the module's
        // list, never a second spelling here.
        let mut sel = metal_graph::source::CONST_NAMES
            .iter()
            .position(|name| Some(*name) == cfg["constant"].as_str())
            .map(|i| i + 1)
            .unwrap_or(0);
        let mut labels: Vec<String> = vec![tr("value").to_string()];
        labels.extend(
            metal_graph::source::CONST_NAMES
                .iter()
                .map(|s| (*s).to_string()),
        );
        let options: Vec<(usize, &str)> = labels
            .iter()
            .enumerate()
            .map(|(i, s)| (i, s.as_str()))
            .collect();
        if label_row(ui, theme, tr("constant"), |ui| {
            segmented(ui, theme, &mut sel, &options)
        })
        .changed()
        {
            let v = match sel
                .checked_sub(1)
                .and_then(|i| metal_graph::source::CONST_NAMES.get(i))
            {
                Some(name) => json!(name),
                None => Value::Null,
            };
            post_text(door, gid, node_id, json!({ "constant": v }));
        }
    } else if kind == metal_graph::source::NOISE_KIND {
        // M2: which lattice dimension the clock walks — `t`, `x`, `y` or `z`.
        let mut a = cfg["axis"].as_f64().unwrap_or(0.0).round().clamp(0.0, 3.0) as usize;
        let options: Vec<(usize, String)> = AXIS_LABELS
            .iter()
            .enumerate()
            .map(|(i, l)| (i, tr(l).to_string()))
            .collect();
        let options: Vec<(usize, &str)> = options.iter().map(|(i, s)| (*i, s.as_str())).collect();
        if label_row(ui, theme, tr("axis"), |ui| {
            segmented(ui, theme, &mut a, &options)
        })
        .changed()
        {
            post_text(door, gid, node_id, json!({ "axis": a as f64 }));
        }
    }
}
