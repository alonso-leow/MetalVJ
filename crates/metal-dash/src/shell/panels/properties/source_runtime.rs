//! The runtime of a source (2026-10-10): what a `still`, a `reel` or a
//! `reel.batch` runs on. A source feeds an `fx.forge` whose WGSL is the
//! runtime — a preset from `data/shaders/presets/`, or a `.wgsl` the operator
//! loads. The section shows on the source's own page and on the forge's, so
//! the same control is reached from either end of the cable.
//!
//! Every write is one `POST /graph/node`: `config` for a preset, `shader` for
//! a loaded script. The shell names the file's path and shows the text the
//! engine refuses with; it reads nothing itself.

use crate::shell::components::small_text;
use egui::{TextEdit, Ui};
use serde_json::{json, Value};

use super::sources::pick_row;
use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{kv_row, kv_wrap, note};
use crate::shell::panels::nodes::{add_file_source, free_spot};

/// The presets `data/shaders/presets/<name>.wgsl` ships with.
pub(crate) const PRESETS: &[&str] = &[
    "passthrough",
    "mirror",
    "kaleido",
    "pixelate",
    "invert",
    "ripple",
    "rgb-split",
    "posterize",
    "zoom-pulse",
];

/// The kinds a runtime can sit under: the three file sources.
const SOURCE_KINDS: &[&str] = &["still", "reel", metal_graph::REEL_BATCH_KIND];

/// The runtime's own page state: the script path being typed and the line
/// under the section (the engine's refusal, or the last success).
#[derive(Default)]
pub(crate) struct State {
    script: String,
    note: Option<String>,
}

fn nodes(graph: &Value) -> &[Value] {
    graph["graph"]["nodes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn edges(graph: &Value) -> &[Value] {
    graph["graph"]["edges"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn kind_of<'a>(graph: &'a Value, id: &str) -> &'a str {
    nodes(graph)
        .iter()
        .find(|n| n["id"].as_str() == Some(id))
        .and_then(|n| n["kind"].as_str())
        .unwrap_or_default()
}

/// The `fx.forge` a source feeds on its image port, if one is wired.
pub(crate) fn forge_below(graph: &Value, source: &str) -> Option<String> {
    edges(graph)
        .iter()
        .filter(|e| {
            e["from"]["node"].as_str() == Some(source)
                && e["from"]["port"].as_str() == Some("image")
        })
        .filter_map(|e| e["to"]["node"].as_str())
        .find(|to| kind_of(graph, to) == metal_graph::FORGE_EFFECT_KIND)
        .map(str::to_string)
}

/// The source an `fx.forge` takes its picture from, if it is fed by one.
fn source_above(graph: &Value, forge: &str) -> Option<String> {
    edges(graph)
        .iter()
        .filter(|e| {
            e["to"]["node"].as_str() == Some(forge) && e["to"]["port"].as_str() == Some("image")
        })
        .filter_map(|e| e["from"]["node"].as_str())
        .find(|from| SOURCE_KINDS.contains(&kind_of(graph, from)))
        .map(str::to_string)
}

/// Whether a node's page carries the `runtime` section: every file source,
/// and the `fx.forge` a file source feeds.
pub(crate) fn shows(kind: &str, graph: &Value, node_id: &str) -> bool {
    SOURCE_KINDS.contains(&kind)
        || (kind == metal_graph::FORGE_EFFECT_KIND && source_above(graph, node_id).is_some())
}

/// `añadir runtime`: an `fx.forge` (the passthrough preset) to the source's
/// right, wired image-out → image-in. The same two calls `importar` makes.
fn add_runtime(door: &dyn Call, gid: &str, source: &str, graph: &Value) -> Result<(), String> {
    let pos = free_spot(Some(graph));
    let (forge, _) = add_file_source(
        door,
        gid,
        metal_graph::FORGE_EFFECT_KIND,
        pos,
        json!({ "shader": "presets/passthrough" }),
    )?;
    door.json(
        "POST",
        "/graph/edge",
        Some(json!({
            "op": "add", "graph": gid,
            "from": { "node": source, "port": "image" },
            "to": { "node": forge, "port": "image" },
        })),
    )
    .map(|_| ())
    .map_err(|(_, e)| e)
}

/// The section body. `forge` is the runtime node: the node itself on a forge
/// page, the one below on a source page. Without one, `añadir runtime`.
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
    let kind = node["kind"].as_str().unwrap_or_default();
    let forge = if kind == metal_graph::FORGE_EFFECT_KIND {
        Some(node_id.to_string())
    } else {
        forge_below(graph, node_id)
    };
    let Some(forge) = forge else {
        if ui
            .button(small_text(theme, "añadir runtime"))
            .clicked()
        {
            state.note = add_runtime(door, gid, node_id, graph).err();
        }
        state_note(ui, theme, state);
        return;
    };

    let current = nodes(graph)
        .iter()
        .find(|n| n["id"].as_str() == Some(forge.as_str()))
        .and_then(|n| n["config"]["shader"].as_str())
        .unwrap_or_default()
        .to_string();
    let choices: Vec<(String, String)> = PRESETS
        .iter()
        .map(|p| (format!("presets/{p}"), (*p).to_string()))
        .collect();
    if let Some(pick) = pick_row(ui, theme, "runtime-preset", "preset", &current, &choices) {
        let body = json!({
            "op": "config", "graph": gid, "node": forge,
            "config": { "shader": pick },
        });
        state.note = door
            .json("POST", "/graph/node", Some(body))
            .err()
            .map(|(_, e)| e);
    }

    kv_row(ui, theme, "script", |ui| {
        ui.add(TextEdit::singleline(&mut state.script).desired_width(ui.available_width()))
    });
    kv_wrap(ui, theme, "", |ui| {
        if ui
            .button(small_text(theme, "cargar script…"))
            .clicked()
            && !state.script.trim().is_empty()
        {
            let body = json!({
                "op": "shader", "graph": gid, "node": forge,
                "path": state.script.trim(),
            });
            state.note = match door.json("POST", "/graph/node", Some(body)) {
                Ok(reply) => Some(format!(
                    "script: {}",
                    reply["shader"].as_str().unwrap_or_default()
                )),
                Err((_, e)) => Some(e),
            };
        }
    });
    state_note(ui, theme, state);
}

fn state_note(ui: &mut Ui, theme: &Theme, state: &State) {
    if let Some(text) = &state.note {
        note(ui, theme, text);
    }
}

#[cfg(all(test, feature = "gpu"))]
mod tests {
    use super::*;

    /// Every preset is a shader the engine's own naga gate accepts as an
    /// effect — the same `metal_forge::check` a loaded script passes.
    #[test]
    fn every_preset_checks_as_an_effect() {
        for name in PRESETS {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../data/shaders/presets")
                .join(format!("{name}.wgsl"));
            let wgsl = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let checked = metal_forge::check(&wgsl).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(checked.effect, "{name} must read `src`");
        }
    }

    #[test]
    fn the_runtime_sits_under_a_source_and_on_the_forge_it_feeds() {
        let graph = json!({ "graph": {
            "nodes": [
                { "id": "s", "kind": "still" },
                { "id": "f", "kind": "fx.forge" },
                { "id": "g", "kind": "gen.turing" },
            ],
            "edges": [
                { "id": "e1", "from": { "node": "s", "port": "image" }, "to": { "node": "f", "port": "image" } },
            ],
        }});
        assert_eq!(forge_below(&graph, "s").as_deref(), Some("f"));
        assert!(shows("still", &graph, "s"));
        assert!(shows("fx.forge", &graph, "f"));
        assert!(!shows("fx.forge", &graph, "g"));
        assert!(!shows("gen.turing", &graph, "g"));
    }
}
