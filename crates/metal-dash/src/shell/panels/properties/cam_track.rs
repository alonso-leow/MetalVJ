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
use crate::shell::components::{slider_row, toggle_row};
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
    let mut all = targets.is_empty();
    if toggle_row(ui, theme, tr("all 3D generators"), &mut all).changed() {
        if all {
            post_text(door, gid, node_id, json!({ "targets": [] }));
        } else {
            let seed: Vec<String> = gens.iter().map(|(id, _)| id.clone()).collect();
            post_text(door, gid, node_id, json!({ "targets": seed }));
        }
    }
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
        for (id, label) in &gens {
            let mut on = targets.contains(id);
            if toggle_row(ui, theme, label, &mut on).changed() {
                let mut next = targets.clone();
                if on {
                    if !next.contains(id) {
                        next.push(id.clone());
                    }
                } else {
                    next.retain(|x| x != id);
                }
                post_text(door, gid, node_id, json!({ "targets": next }));
            }
        }
    }

    // --- strength --------------------------------------------------------
    ui.separator();
    let mut strength = cfg["strength"].as_f64().unwrap_or(1.0) as f32;
    if slider_row(ui, theme, tr("strength"), &mut strength, 0.0..=1.0, true).changed() {
        post_text(door, gid, node_id, json!({ "strength": strength }));
    }

    // --- invert per axis -------------------------------------------------
    let mut invert = [
        cfg["invert"][0].as_bool().unwrap_or(false),
        cfg["invert"][1].as_bool().unwrap_or(false),
        cfg["invert"][2].as_bool().unwrap_or(false),
    ];
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(tr("invert"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        for (i, axis) in ["x", "y", "z"].iter().enumerate() {
            let label = trf("invert {axis}", &[("axis", axis)]);
            if toggle_row(ui, theme, &label, &mut invert[i]).changed() {
                post_text(door, gid, node_id, json!({ "invert": invert }));
            }
        }
    });
    ui.label(
        RichText::new(tr("flip an axis for a camera mounted the other way round."))
            .font(theme.font_small.clone())
            .color(theme.ink_faint),
    );
}
