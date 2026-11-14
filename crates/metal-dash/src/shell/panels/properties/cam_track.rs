//! H5: `cam.track`'s page — which generators it drives, how hard, and which
//! axes are flipped.
//!
//! **An empty `targets` is "all of them".** The plan's default is every
//! generator in the graph whose model declares a 3D camera; the checkbox
//! spells that as the empty list the engine already reads (`targets: []`),
//! so "all" is one value, not a second list that could go stale. Unchecking
//! it seeds the list with every `gen.*` node so nothing silently drops.
//!
//! Every field posts through `POST /graph/node {op:"config"}`, the same merge
//! op every other page uses.

use egui::{RichText, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{label_row, payload_split, slider_row, subtitle, toggle_row};
use crate::shell::i18n::{tr, trf};

use super::text::post_text;

/// The `(id, label)` of every generator node in the graph reply.
fn generators(graph: &Value) -> Vec<(String, String)> {
    graph["graph"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| n["kind"].as_str().is_some_and(|k| k.starts_with("gen.")))
        .filter_map(|n| {
            let id = n["id"].as_str()?.to_string();
            let label = n["label"]
                .as_str()
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| id.clone());
            Some((id, label))
        })
        .collect()
}

pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
) {
    let cfg = &node["config"];
    let targets: Vec<String> = cfg["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    let gens = generators(graph);

    // --- targets: "all 3D generators" or an explicit list ----------------
    let all = targets.is_empty();
    subtitle(ui, theme, tr("targets"));
    super::chain::cell_grid(ui, theme, |ui| {
        if super::chain::cell(ui, theme, tr("all 3D generators"), all).clicked() {
            if all {
                post_text(door, gid, node_id, json!({ "targets": gens.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>() }));
            } else {
                post_text(door, gid, node_id, json!({ "targets": [] }));
            }
        }
    });
    if all {
        ui.label(
            RichText::new(tr("every generator whose model has a camera follows the head."))
            .font(theme.font_small.clone())
            .color(theme.ink_faint),
        );
    } else if gens.is_empty() {
        ui.label(
            RichText::new(tr("— no generators in this graph —"))
                .font(theme.font_small.clone())
                .color(theme.ink_faint),
        );
    } else {
        super::chain::cell_grid(ui, theme, |ui| {
            for (id, label) in &gens {
                let on = targets.contains(id);
                if super::chain::cell(ui, theme, label.as_str(), on).clicked() {
                    let mut next = targets.clone();
                    if on {
                        next.retain(|x| x != id);
                    } else {
                        next.push(id.clone());
                    }
                    post_text(door, gid, node_id, json!({ "targets": next }));
                }
            }
        });
    }

    // --- strength --------------------------------------------------------
    subtitle(ui, theme, tr("strength"));
    let mut strength = cfg["strength"].as_f64().unwrap_or(1.0) as f32;
    if slider_row(ui, theme, tr("strength"), &mut strength, 0.0..=1.0, true).changed() {
        post_text(door, gid, node_id, json!({ "strength": strength }));
    }

    // --- invert per axis -------------------------------------------------
    subtitle(ui, theme, tr("invert"));
    let invert = [
        cfg["invert"][0].as_bool().unwrap_or(false),
        cfg["invert"][1].as_bool().unwrap_or(false),
        cfg["invert"][2].as_bool().unwrap_or(false),
    ];
    super::chain::cell_grid(ui, theme, |ui| {
        for (i, axis) in ["x", "y", "z"].iter().enumerate() {
            let label = trf("invert {axis}", &[("axis", axis)]);
            if super::chain::cell(ui, theme, label, invert[i]).clicked() {
                let mut next = invert;
                next[i] = !invert[i];
                post_text(door, gid, node_id, json!({ "invert": next }));
            }
        }
    });
    ui.label(
        RichText::new(tr("flip an axis for a camera mounted the other way round."))
            .font(theme.font_small.clone())
            .color(theme.ink_faint),
    );
}
