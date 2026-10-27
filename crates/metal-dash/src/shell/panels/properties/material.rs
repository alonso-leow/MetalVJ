//! The `mat.*` node's own controls (materials brief, M4): which part it
//! overrides and its fixed knobs, over the `POST /graph/node {op:"config"}`
//! merge the AI's `set_material` shares (rule 6).
//!
//! The picture is the node's upstream 3D model; the section names it, lists
//! its surface parts from the door's catalog (rule 5, never re-derived) and
//! warns when the model declares none — the same no-op the lint reports. The
//! knobs and their ranges come from `metal_graph::materials`, so the panel,
//! the row floats and the tool's schema cannot drift.

use crate::shell::components::small_text;
use egui::{Id, RichText, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::slider_row;
use crate::shell::i18n::tr;

use super::effects::upstream_gen;
use super::node::field_row;
use super::shared::post;
use super::surfaces::parts;

pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
) {
    let kind = node["kind"].as_str().unwrap_or_default();
    let Some(name) = metal_graph::materials::kind_of(kind) else { return };
    let Some(gen) = upstream_gen(graph, node_id) else {
        ui.label(
            RichText::new(tr("— not wired: connect this material downstream of a 3D model —"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        return;
    };
    let gen_kind = model_kind(graph, &gen).unwrap_or_default();
    let surface_parts = parts(door, &gen_kind);

    field_row(ui, theme, tr("material"), name);
    field_row(
        ui,
        theme,
        tr("model"),
        &crate::shell::panels::properties::chain::name_of(graph, &gen),
    );

    let current = node["config"]["part"].as_str().unwrap_or(metal_graph::materials::PART_ANY);
    let mut pick = current.to_string();
    let mut options: Vec<(String, String)> =
        vec![(metal_graph::materials::PART_ANY.to_string(), tr("every surface").to_string())];
    for part in &surface_parts {
        options.push((part.clone(), crate::shell::i18n::tr_dyn(part)));
    }
    let selected = options
        .iter()
        .find(|(id, _)| id == &pick)
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| tr("every surface").to_string());
    crate::shell::components::combo_box(theme, Id::new(("shell/region/properties/material-part", node_id)))
        .selected_text((selected).to_string())
        .show_ui(ui, |ui| {
            for (id, label) in &options {
                if ui
                    .selectable_label(*id == pick, small_text(theme, label.as_str()))
                    .clicked()
                {
                    pick = id.clone();
                }
            }
        });
    if pick != current {
        post_config(door, gid, node_id, "part", json!(pick));
    }

    if surface_parts.is_empty() {
        ui.label(
            RichText::new(tr("this model declares no surfaces — the material is a no-op"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    }

    for (key, default) in metal_graph::materials::CONFIG_DEFAULTS {
        let mut value =
            node["config"][key].as_f64().map(|v| v as f32).unwrap_or(*default);
        let (min, max) = metal_graph::materials::range_of(key).unwrap_or((0.0, 1.0));
        if slider_row(ui, theme, label(key), &mut value, min..=max, true).changed() {
            post_config(door, gid, node_id, key, json!(value));
        }
    }
}

/// The label a config key shows — explicit `tr` calls so the locale gate sees
/// every literal (a `tr(key)` over the table would read as an orphan).
fn label(key: &str) -> &'static str {
    match key {
        "mix" => tr("mix"),
        "scale" => tr("scale"),
        "rough" => tr("rough"),
        "metal" => tr("metal"),
        "bump" => tr("bump"),
        "env" => tr("environment"),
        "tint" => tr("tint"),
        "tint_hue" => tr("tint hue"),
        "glitch" => tr("glitch"),
        "glitch_rate" => tr("glitch rate"),
        "glitch_seed" => tr("glitch seed"),
        _ => tr("value"),
    }
}

fn model_kind(graph: &Value, node: &str) -> Option<String> {
    graph["graph"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|n| n["id"].as_str() == Some(node))
        .and_then(|n| n["kind"].as_str())
        .map(str::to_string)
}

/// The `config` merge one knob posts — the same `POST /graph/node` op the AI's
/// `set_material` sends (rule 6).
fn post_config(door: &dyn Call, gid: &str, node_id: &str, key: &str, value: Value) {
    let mut cfg = serde_json::Map::new();
    cfg.insert(key.to_string(), value);
    let body = json!({
        "op": "config", "graph": gid, "node": node_id,
        "config": Value::Object(cfg),
    });
    post(door, "/graph/node", body);
}
