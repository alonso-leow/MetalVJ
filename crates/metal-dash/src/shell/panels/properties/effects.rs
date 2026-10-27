//! The effect node's own controls: state (off / reactive / fixed) and the fixed
//! numeric value, for the **selected `fx.*` node** — never a list of the
//! catalog effects keyed off a generator. The backend `POST /graph/effects`
//! keys off the *generator* the chain belongs to, so [`upstream_gen`] walks the
//! selected node's input cable back to the gen feeding it — a read off a `GET
//! /graph` reply the panel already holds, never a re-derived layout (rule 5).
//!
//! With no upstream cable — an `fx.*` node sitting in the canvas unbuilt — the
//! section says "not wired" and posts nothing: per B5, `off` *is* the cable
//! removed.

use egui::{RichText, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{field_row, segmented, slider_row};
use crate::shell::i18n::{tr, trf};

use super::shared::get;

#[allow(clippy::too_many_arguments)]
pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    kind: &str,
    gid: &str,
    fx_id: &str,
    graph: &Value,
) {
    let Some(effect) = kind.strip_prefix("fx.").map(str::to_string) else {
        return;
    };
    let Some(gen_node) = upstream_gen(graph, fx_id) else {
        ui.label(
            RichText::new(tr("— not wired: connect this fx node downstream of a gen —"))
            .font(theme.font_small.clone())
            .color(theme.ink_weak),
        );
        return;
    };
    let Some(effects) = get(
        door,
        "GET",
        "/graph/effects",
        &[("id", gid.to_string()), ("node", gen_node.clone())],
    ) else {
        return;
    };
    let Some(list) = effects["effects"].as_array() else {
        return;
    };
    let Some(entry) = list.iter().find(|e| e["node"] == json!(fx_id)) else {
        ui.label(
            RichText::new(trf("not listed in {gen}'s chain", &[("gen", &gen_node)]))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        return;
    };

    let state = entry["state"].as_str().unwrap_or("off").to_string();
    field_row(ui, theme, tr("effect"), &effect);

    let mut pick = state.clone();
    let options = [
        ("off".to_string(), tr("off")),
        ("reactive".to_string(), tr("reactivo")),
        ("fixed".to_string(), tr("fijo")),
    ];
    if segmented(ui, theme, &mut pick, &options).changed() {
        let value = if pick == "fixed" {
            entry["value"].as_f64().unwrap_or(0.0)
        } else {
            0.0
        };
        post_effect(door, gid, &gen_node, &effect, &pick, value);
    }
    if state == "fixed" {
        let mut value = entry["value"].as_f64().unwrap_or(0.0) as f32;
        if slider_row(ui, theme, tr("value"), &mut value, 0.0..=1.0, true).changed() {
            post_effect(door, gid, &gen_node, &effect, "fixed", value as f64);
        }
    }
}

/// Walk `fx_id`'s input cable upstream through the graph edges until the
/// feeding node is no longer an `fx.*` — the gen whose `image` chain this
/// effect sits on. `None` means the effect node is not wired into any generator
/// yet.
pub(crate) fn upstream_gen(graph: &Value, fx_id: &str) -> Option<String> {
    let nodes = graph["graph"]["nodes"].as_array()?;
    let edges = graph["graph"]["edges"].as_array()?;
    let kind_of = |id: &str| {
        nodes
            .iter()
            .find(|n| n["id"] == json!(id))
            .and_then(|n| n["kind"].as_str())
            .map(str::to_string)
    };
    let mut current = fx_id.to_string();
    for _ in 0..nodes.len() {
        let next = edges
            .iter()
            .find(|e| e["to"]["node"].as_str() == Some(current.as_str()))
            .and_then(|e| e["from"]["node"].as_str())?
            .to_string();
        match kind_of(&next) {
            Some(k) if k.starts_with("fx.") => current = next,
            Some(_) => return Some(next),
            None => return None,
        }
    }
    None
}

pub(crate) fn post_effect(
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    effect: &str,
    state: &str,
    value: f64,
) {
    let mut body = json!({ "graph": gid, "node": node_id, "effect": effect, "state": state });
    if state == "fixed" {
        body["value"] = json!(value);
    }
    let _ = door.json("POST", "/graph/effects", Some(body));
}
