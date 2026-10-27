//! Properties for the WGSL editor of `fx.forge` nodes: pick a shader from
//! `GET /shaders`, edit its source, save through `POST /shaders`, and read
//! naga's errors inline.
//!
//! The properties panel is a browser: a tree of folders for the selected
//! node on the left, the chosen folder's content on the right. This module
//! adds its own folder to that tree: [`folders`] names it for a node kind
//! (English source text, painted through `i18n::tr`; empty for kinds this
//! module does not own), and [`section`] paints it when the operator opens it.
//!
//! # Rule 6 — every write is the route
//!
//! The editor never writes `data/shaders/`. The save button posts `{id, wgsl}`
//! to `POST /shaders` — the one writer — through the door, and the AI's
//! `save_shader` tool posts the same body. A shader that does not pass naga
//! comes back with its errors and is not written.

use egui::{Id, RichText, Ui};
use serde_json::Value;

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{kv_row, note};
use crate::shell::i18n::{tr, tr_key};

/// What this module keeps between frames.
#[derive(Default)]
pub struct State {
    /// `GET /shaders`'s `(id, kind)` rows, fetched once when the folder opens.
    list: Vec<(String, String)>,
    /// Whether the list has been asked for, so a door that answers nothing is
    /// not re-asked every frame.
    fetched: bool,
    /// The id the operator is editing, and the source draft under the cursor.
    selected: Option<String>,
    draft: String,
    /// The id `draft` was loaded from, so a fresh pick replaces the draft.
    loaded: Option<String>,
    /// naga's errors from the last save, until a later save clears them.
    errors: Vec<String>,
}

/// The folder this module owns, as a `tr_key` literal so the i18n scanner
/// sees it and the panel paints it through the table.
const SHADER_FOLDERS: [&str; 1] = [tr_key("Shader")];

/// The folders this module adds for a node of `kind`, in tree order. The one
/// kind is the `fx.forge` effect (`metal_perform::forge::FX_KIND`).
pub fn folders(kind: &str) -> &'static [&'static str] {
    if kind == metal_perform::forge::FX_KIND {
        &SHADER_FOLDERS
    } else {
        &[]
    }
}

/// Paint `folder` (one of [`folders`]) for `node` and answer whether it
/// painted anything.
#[allow(clippy::too_many_arguments)]
pub fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    _gid: &str,
    _node_id: &str,
    node: &Value,
    _graph: &Value,
    folder: &str,
    state: &mut State,
) -> bool {
    if folder != SHADER_FOLDERS[0] {
        return false;
    }
    if !state.fetched {
        state.list = fetch_list(door);
        state.fetched = true;
        // Open on the shader the node names, when it names one.
        if let Some(id) = node["config"]["shader"].as_str().filter(|s| !s.is_empty()) {
            let id = id.to_string();
            if state.list.iter().any(|(listed, _)| *listed == id) {
                state.draft = fetch_source(door, &id).unwrap_or_default();
                state.selected = Some(id.clone());
                state.loaded = Some(id);
            }
        }
    }
    shader_folder(ui, theme, door, state);
    true
}

/// `GET /shaders` — the id/kind rows the picker offers.
fn fetch_list(door: &dyn Call) -> Vec<(String, String)> {
    door.json("GET", "/shaders", None)
        .ok()
        .and_then(|v| v["shaders"].as_array().cloned())
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let id = r["id"].as_str()?.to_string();
                    let kind = r["kind"].as_str().unwrap_or("").to_string();
                    Some((id, kind))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `GET /shaders/<id>` — one shader's source.
fn fetch_source(door: &dyn Call, id: &str) -> Option<String> {
    door.json("GET", &format!("/shaders/{id}"), None)
        .ok()
        .and_then(|v| v["wgsl"].as_str().map(str::to_string))
}

/// The picker, the source field, the save button and the errors.
fn shader_folder(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut State) {
    if state.list.is_empty() {
        note(ui, theme, tr("No shaders yet"));
        return;
    }

    for (id, kind) in state.list.clone() {
        let selected = state.selected.as_deref() == Some(id.as_str());
        let label = if kind == "effect" { tr("effect") } else { tr("generator") };
        let clicked = kv_row(ui, theme, label, |ui| {
            ui.selectable_label(selected, RichText::new(id.clone()).font(theme.font_small.clone()))
        })
        .clicked();
        if clicked {
            state.errors.clear();
            if state.loaded.as_deref() != Some(id.as_str()) {
                state.draft = fetch_source(door, &id).unwrap_or_default();
                state.loaded = Some(id.clone());
            }
            state.selected = Some(id);
        }
    }

    let Some(id) = state.selected.clone() else {
        note(ui, theme, tr("Pick a shader"));
        return;
    };

    ui.add_space(theme.section_gap);
    ui.add(
        egui::TextEdit::multiline(&mut state.draft)
            .id(Id::new("shell/panel/properties/shader"))
            .desired_width(ui.available_width())
            .margin(theme.field_pad)
            .desired_rows(theme.code_rows)
            .font(theme.font_small.clone()),
    );
    if ui.button(RichText::new(tr("Save")).font(theme.font_small.clone())).clicked() {
        let wgsl = state.draft.clone();
        save(door, &id, &wgsl, state);
    }

    if !state.errors.is_empty() {
        ui.label(RichText::new(tr("Validation errors")).font(theme.font_small.clone()).color(theme.ink).strong());
        for error in &state.errors {
            ui.add(egui::Label::new(RichText::new(error.clone()).font(theme.font_small.clone()).color(theme.soft)).wrap());
        }
    }
}

/// `POST /shaders {id, wgsl}` — the one writer. A reply that is not `ok`
/// carries naga's errors, shown inline; a successful save refreshes the list
/// so a shader that changed kind (effect ↔ generator) is reclassified.
fn save(door: &dyn Call, id: &str, wgsl: &str, state: &mut State) {
    match door.json("POST", "/shaders", Some(serde_json::json!({ "id": id, "wgsl": wgsl }))) {
        Ok(reply) if reply["ok"].as_bool() == Some(true) => {
            state.errors.clear();
            state.list = fetch_list(door);
        }
        Ok(reply) => {
            state.errors = reply["errors"]
                .as_array()
                .map(|errors| errors.iter().filter_map(|e| e.as_str().map(str::to_string)).collect())
                .unwrap_or_else(|| vec![tr("the save was refused").to_string()]);
        }
        Err((_status, message)) => state.errors = vec![message],
    }
}
