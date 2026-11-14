//! O.4's control: a `stack.mode` node's mode, read straight off
//! `node["config"]` (`{ kind, id? }` — `DeviceMode::describe`'s own JSON, the
//! shape `main.rs::graph_device_mode` reads before the layer list exists) and
//! written through `POST /graph/node { op: "mode", kind, collection? }` — the
//! write-side twin of that reader, and the only route that writes it.

use egui::Ui;
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{dropdown_fill, label_row, subtitle};
use crate::shell::i18n::tr;

use super::chain::{cell, cell_grid};
use super::shared::get;

/// The four device modes `crate::layers::DeviceMode::parse` reads, in the
/// spelling `DeviceMode::describe` writes (`kind`) — a closed Rust enum, not a
/// count that could drift, and `parse` is what turns the string this panel
/// posts back into the enum on the other side of the door.
pub(crate) const DEVICE_MODES: [&str; 4] = ["automix", "disabled", "own", "collection"];

pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
) {
    let cfg = &node["config"];
    let kind = cfg["kind"].as_str().unwrap_or_default().to_string();
    let collection = cfg["id"].as_str().unwrap_or_default().to_string();

    subtitle(ui, theme, tr("mode"));
    let mut picked: Option<String> = None;
    cell_grid(ui, theme, |ui| {
        if kind.is_empty() {
            cell(ui, theme, tr("— no mode (the dropdown decides) —"), true);
        }
        for m in DEVICE_MODES {
            if cell(ui, theme, m, m == kind).clicked() && m != kind {
                picked = Some(m.to_string());
            }
        }
    });
    if let Some(kind_sel) = picked {
        // `collection` needs an id to be a mode at all; with none picked yet
        // the route would refuse, so the first collection `/state` lists is the
        // pick until the operator changes it below.
        if kind_sel == "collection" {
            if let Some(cid) = first_collection(door) {
                post_mode(door, gid, node_id, "collection", Some(&cid));
            }
        } else {
            post_mode(door, gid, node_id, &kind_sel, None);
        }
    }

    if kind == "collection" {
        let state_val = get(door, "GET", "/state", &[]).unwrap_or_else(|| json!({}));
        let collections = state_val["collections"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let options: Vec<(String, &str)> = collections
            .iter()
            .filter_map(|c| {
                let cid = c["id"].as_str()?;
                let name = c["name"].as_str().unwrap_or(cid);
                Some((cid.to_string(), name))
            })
            .collect();
        subtitle(ui, theme, tr("collection"));
        let mut collection_sel = collection.clone();
        cell_grid(ui, theme, |ui| {
            for (cid, name) in &options {
                if cell(ui, theme, name.clone(), *cid == collection).clicked() && *cid != collection {
                    collection_sel = cid.clone();
                }
            }
        });
        if collection_sel != collection {
            post_mode(door, gid, node_id, "collection", Some(&collection_sel));
        }
    }
}

/// The first collection `GET /state` lists, if any — what a fresh `collection`
/// mode starts on (see [`section`]).
pub(crate) fn first_collection(door: &dyn Call) -> Option<String> {
    let state_val = get(door, "GET", "/state", &[])?;
    state_val["collections"].as_array()?.first()?["id"]
        .as_str()
        .map(str::to_string)
}

pub(crate) fn post_mode(
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    kind: &str,
    collection: Option<&str>,
) {
    let mut body = json!({ "op": "mode", "graph": gid, "node": node_id, "kind": kind });
    if let Some(cid) = collection {
        body["collection"] = json!(cid);
    }
    door.post_async("/graph/node", body);
}
