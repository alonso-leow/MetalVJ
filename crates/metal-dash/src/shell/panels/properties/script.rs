//! A `script.*` node's own page: the source editor (`§9.4 V3`) above the
//! floats `S.AL.3` already had.
//!
//! **Rule 6 / the one writer.** The source is read from `GET /script/<node>`
//! and written by `POST /script {graph, node, source}` through [`Call`] — the
//! same two routes the AI's `read_script`/`save_script` tools walk. The panel
//! never touches `data/graphs/`: the route runs the source through
//! `metal_perform::script` before it writes, so a source that raises, hangs or
//! fails to compile comes back with its runtime error and the node is left as
//! it was. That error is shown inline, not swallowed.
//!
//! **A draft, not a live write.** The field is bound to a local draft loaded
//! once per selected node; keystrokes never reach the graph until Save, so
//! typing a half-finished line does not spawn (or kill) a worker each frame.
//!
//! The floats half is unchanged: a float starts at its live `value` when it has
//! one, its `default` otherwise, exactly `ScriptConfig::values`'s own read. A
//! `floats` port with an incoming edge renders disabled with a "reactive"
//! label, and its one write goes through `POST /graph/node {op:"config"}` — the
//! same validated writer the AI's `set_params` uses.

use crate::shell::components::small_text;
use egui::{Id, RichText, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{kv_row, note, num_value};
use crate::shell::i18n::tr;

/// What this editor keeps between frames: the loaded source and the last
/// runtime error, both keyed to the node they belong to.
#[derive(Default)]
pub struct State {
    /// The node `draft`/`lang` were loaded from, so selecting another script
    /// reloads rather than showing the previous one's text.
    loaded_for: Option<String>,
    /// The source under the cursor, seeded from `GET /script/<node>`.
    draft: String,
    /// The language wire string of `loaded_for` (`py`/`js`/`rs`).
    lang: String,
    /// The last Save's runtime error, cleared by a Save that runs.
    error: Option<String>,
}

/// The English label for a language wire string, painted through `tr`.
fn lang_label(lang: &str) -> &'static str {
    match lang {
        "rs" => tr("Rust"),
        "js" => tr("JavaScript"),
        _ => tr("Python"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
    state: &mut State,
) {
    source_section(ui, theme, door, gid, node_id, state);
    ui.add_space(theme.section_gap);
    floats_section(ui, theme, door, gid, node_id, node, graph);
}

/// The code editor: the language badge, the source field and the Save button.
fn source_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    state: &mut State,
) {
    if state.loaded_for.as_deref() != Some(node_id) {
        if let Some((lang, source)) = fetch_source(door, gid, node_id) {
            state.draft = source;
            state.lang = lang;
        } else {
            state.draft.clear();
            state.lang.clear();
        }
        state.error = None;
        state.loaded_for = Some(node_id.to_string());
    }

    ui.label(
        RichText::new(tr("Source"))
            .font(theme.font_small.clone())
            .color(theme.ink_weak)
            .strong(),
    );
    if !state.lang.is_empty() {
        kv_row(ui, theme, tr("language"), |ui| {
            ui.label(small_text(theme, lang_label(&state.lang)).color(theme.ink))
        });
    }
    ui.add(
        egui::TextEdit::multiline(&mut state.draft)
            .id(Id::new("shell/panel/properties/script"))
            .desired_width(ui.available_width())
            .margin(theme.field_pad)
            .desired_rows(theme.code_rows)
            .font(theme.font_small.clone()),
    );
    let save = ui
        .button(small_text(theme, tr("Save")))
        .clicked();
    if let Some(error) = state.error.clone() {
        ui.label(
            RichText::new(tr("Runtime error"))
                .font(theme.font_small.clone())
                .color(theme.warn)
                .strong(),
        );
        ui.add(egui::Label::new(small_text(theme, error).color(theme.soft)).wrap());
    }
    if save {
        save_source(door, gid, node_id, state);
    }
}

/// `GET /script/<node>?graph=<id>` — the `(lang, source)` the field opens on.
fn fetch_source(door: &dyn Call, gid: &str, node_id: &str) -> Option<(String, String)> {
    let reply = door
        .bytes("GET", &format!("/script/{node_id}"), &[("graph", gid.to_string())])
        .ok()?;
    let v: Value = serde_json::from_slice(&reply.body).ok()?;
    let lang = v["lang"].as_str().unwrap_or("py").to_string();
    Some((lang, v["source"].as_str().unwrap_or_default().to_string()))
}

/// `POST /script {graph, node, source}` — the one writer. A reply that is not
/// `ok` carries the runtime's error, shown inline; a save that runs clears it.
fn save_source(door: &dyn Call, gid: &str, node_id: &str, state: &mut State) {
    let body = json!({ "graph": gid, "node": node_id, "source": state.draft });
    match door.json("POST", "/script", Some(body)) {
        Ok(reply) if reply["ok"].as_bool() == Some(true) => {
            state.error = None;
            if let Some(lang) = reply["lang"].as_str() {
                state.lang = lang.to_string();
            }
        }
        Ok(reply) => {
            state.error = Some(
                reply["error"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| tr("the save was refused").to_string()),
            );
        }
        Err((_status, message)) => state.error = Some(message),
    }
}

/// The declared floats, one slider each (`S.AL.3`'s human half).
fn floats_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
) {
    let floats_wired = graph["graph"]["edges"]
        .as_array()
        .map(|edges| {
            edges
                .iter()
                .any(|e| e["to"]["node"] == json!(node_id) && e["to"]["port"] == json!("floats"))
        })
        .unwrap_or(false);

    let declared = node["config"]["floats"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if declared.is_empty() {
        note(ui, theme, tr("— no declared floats —"));
        return;
    }

    for f in &declared {
        let Some(name) = f["name"].as_str() else {
            continue;
        };
        let min = f["min"].as_f64().unwrap_or(0.0) as f32;
        let max = f["max"].as_f64().unwrap_or(1.0) as f32;
        let default = f["default"].as_f64().unwrap_or(0.0) as f32;
        let mut value = f["value"].as_f64().map(|v| v as f32).unwrap_or(default);

        let response = kv_row(ui, theme, name, |ui| {
            let tag = if floats_wired { theme.label_width } else { 0.0 };
            let w = (ui.available_width() - tag).max(0.0);
            let response = num_value(ui, theme, w, &mut value, min..=max, "", 2, default, !floats_wired);
            if floats_wired {
                ui.label(
                    RichText::new(tr("reactive"))
                        .font(theme.font_small.clone())
                        .color(theme.ink_weak),
                );
            }
            response
        });
        if !floats_wired && response.changed() {
            post_script_float(door, gid, node_id, name, value);
        }
    }
}

pub(crate) fn post_script_float(door: &dyn Call, gid: &str, node_id: &str, name: &str, value: f32) {
    let mut floats = serde_json::Map::new();
    floats.insert(name.to_string(), json!(value));
    let body = json!({
        "op": "config",
        "graph": gid,
        "node": node_id,
        "config": { "floats": Value::Object(floats) },
    });
    door.post_async("/graph/node", body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    use crate::shell::FakeDoor;
    use egui_kittest::kittest::Queryable;

    /// The editor opens on the node's `GET /script/<node>` source (with its
    /// language badge), and Save posts the draft to `POST /script` — the same
    /// route the AI's `save_script` walks (regla 6).
    #[test]
    fn the_editor_shows_the_source_and_saves_through_the_route() {
        let source = "def process(floats, row):\n    return [1.0]\n";
        let door = Rc::new(FakeDoor::new());
        door.respond_bytes(
            "GET",
            "/script/py-0",
            "application/json",
            serde_json::to_vec(&json!({ "lang": "py", "source": source })).unwrap(),
        );

        let theme = Theme::phosphor();
        let t = theme.clone();
        let d = Rc::clone(&door);
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::Vec2 { x: 700.0, y: 500.0 })
            .build_ui_state(
                move |ui, state: &mut State| {
                    let node = json!({ "id": "py-0", "kind": "script.py", "config": null });
                    let graph = json!({ "graph": { "edges": [] } });
                    section(ui, &t, d.as_ref(), "g1", "py-0", &node, &graph, state);
                },
                State::default(),
            );
        theme.apply(&h.ctx);
        h.run_steps(3);

        h.get_by_label("Source");
        h.get_by_label("Python");
        h.get_by_label("Save").click();
        h.run_steps(3);

        let calls = door.calls.borrow();
        let posted = calls
            .iter()
            .find(|(m, p, _)| m == "POST" && p == "/script")
            .map(|(_, _, p)| serde_json::from_slice::<Value>(&p.body).unwrap())
            .expect("the save posts /script");
        assert_eq!(posted, json!({ "graph": "g1", "node": "py-0", "source": source }));
    }
}
