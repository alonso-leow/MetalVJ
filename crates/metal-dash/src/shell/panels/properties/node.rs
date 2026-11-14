//! The selected node's identity folder: id, kind, family, stage, label,
//! bypass, solo and its ports — the `node` folder of the browser tree, plus the
//! small `ports` block the old flat panel painted inline. Everything is read
//! straight off `GET /graph` and `GET /catalog` (rule 5), never recomputed.

use egui::Ui;

use crate::shell::classes::Theme;
use crate::shell::components::field_row as kit_field_row;
#[cfg(test)]
use crate::call::Call;
#[cfg(test)]
use crate::shell::components::section as kit_section;
#[cfg(test)]
use crate::shell::i18n::tr;
#[cfg(test)]
use egui::RichText;
#[cfg(test)]
use serde_json::Value;
#[cfg(test)]
use super::shared::get;


/// One `label: value` row in the weak-ink / ink pair every identity row uses —
/// the shared kit's own [`crate::shell::components::field_row`], kept as a thin
/// name so `panels::properties::mod`'s file page keeps calling `node::field_row`.
pub(crate) fn field_row(ui: &mut Ui, theme: &Theme, label: &str, value: &str) {
    kit_field_row(ui, theme, label, value);
}

#[cfg(test)]
/// The `node` folder: identity fields off the node object, then the catalog's
/// family and the two port lists.
pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    _gid: &str,
    node_id: &str,
    node: &Value,
    _graph: &Value,
) {
    let kind = node["kind"].as_str().unwrap_or_default().to_string();
    let label = node["label"].as_str().unwrap_or_default().to_string();
    field_row(ui, theme, tr("id"), node_id);
    field_row(ui, theme, tr("kind"), &kind);
    if !label.is_empty() {
        field_row(ui, theme, tr("label"), &label);
    }
    field_row(
        ui,
        theme,
        tr("stage"),
        node["stage"].as_str().unwrap_or("—"),
    );
    let bypass = node["bypass"].as_bool().unwrap_or(false);
    let solo = node["solo"].as_bool().unwrap_or(false);
    field_row(
        ui,
        theme,
        tr("bypass"),
        if bypass { tr("on") } else { tr("off") },
    );
    field_row(
        ui,
        theme,
        tr("solo"),
        if solo { tr("on") } else { tr("off") },
    );
    ports_block(ui, theme, door, &kind);
}

#[cfg(test)]
/// `GET /catalog`'s family/ports badge — the old page's `familyOfLayer`
/// resolved a node's family the same way, off the catalog rather than by
/// re-deriving it from the kind string.
pub(crate) fn ports_block(ui: &mut Ui, theme: &Theme, door: &dyn Call, kind: &str) {
    let Some(catalog) = get(door, "GET", "/catalog", &[]) else {
        return;
    };
    let Some(entry) = catalog["catalog"]
        .as_array()
        .and_then(|ks| ks.iter().find(|k| k["kind"] == serde_json::json!(kind)))
    else {
        return;
    };
    let family = entry["family"].as_str().unwrap_or_default();
    field_row(ui, theme, tr("family"), family);

    let inputs: Vec<String> = entry["inputs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|p| p["name"].as_str().map(str::to_string))
        .collect();
    let outputs: Vec<String> = entry["outputs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|p| p["name"].as_str().map(str::to_string))
        .collect();
    let mut inputs_open = true;
    kit_section(ui, theme, tr("inputs"), &mut inputs_open);
    if inputs_open {
        port_list(ui, theme, &inputs);
    }
    let mut outputs_open = true;
    kit_section(ui, theme, tr("outputs"), &mut outputs_open);
    if outputs_open {
        port_list(ui, theme, &outputs);
    }
}

#[cfg(test)]
/// The joined names of one port list under its section header, or the empty
/// dash — the read-only body a [`ports_block`] section folds.
fn port_list(ui: &mut Ui, theme: &Theme, ports: &[String]) {
    let text = if ports.is_empty() {
        "—".to_string()
    } else {
        ports.join(", ")
    };
    ui.label(
        RichText::new(text)
            .font(theme.font_small.clone())
            .color(theme.ink),
    );
}
