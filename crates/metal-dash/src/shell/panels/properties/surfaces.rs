//! **Surfaces** on a `gen.*` node (plan "Surface sources and less-friction
//! authoring", step 2): one row per material part the model declares
//! (`tex.<part>` inputs of its catalog entry, in catalog order). Each row has
//! a source dropdown — dynamic colours (detach), the nodes of this graph
//! that make a picture, the generators by relevance for that part, and the
//! forge shaders — and, once something is wired, the part's `mix`, `mapping`
//! and `scale` (`config.surfaces.<part>` on this node).
//!
//! Every write is one door call: `attach` for a source (reusing `from`, or a
//! new `kind` with its `config`, or nothing for detach); `config` for the
//! three knobs, posting the whole `surfaces` object since a config merge is
//! shallow. A pick reveals the node it made on the canvas. The parts and the
//! choices are the door's: nothing here lists a model by name (rule 6).

use crate::shell::components::small_text;
use egui::{Id, Ui};
use serde_json::{json, Map, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{kv_row, note, segmented, slider_row};
use crate::shell::i18n::{tr, tr_dyn};

use super::chain::name_of;
use super::shared::{get, post, post_reply};

/// The mapping a part starts with, and the one `paint.rs` reads when the
/// config says nothing.
const DEFAULT_MAPPING: &str = "object";

/// What a source dropdown pick asks the door for.
#[derive(Debug, PartialEq)]
enum Pick {
    /// `dynamic colours`: no source, the model's own colours.
    Detach,
    /// An existing node of the graph.
    From(String),
    /// A new node of `kind`, with its `config` (the forge shader's `shader`).
    Kind { kind: String, config: Option<Value> },
}

/// The material parts of `kind` from the door's catalog entry, `tex.`
/// prefix removed. Empty for a kind with no surface input, or when the
/// catalog cannot be read: the section then does not show.
pub(crate) fn parts(door: &dyn Call, kind: &str) -> Vec<String> {
    let Some(catalog) = get(door, "GET", "/catalog", &[]) else {
        return Vec::new();
    };
    catalog["catalog"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|k| k["kind"].as_str() == Some(kind))
        .map(|k| {
            k["inputs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["name"].as_str()?.strip_prefix("tex.").map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The section: one row per part of the node's kind.
pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
    pending_reveal: &mut Option<String>,
) {
    let kind = node["kind"].as_str().unwrap_or_default();
    for part in parts(door, kind) {
        part_row(
            ui,
            theme,
            door,
            gid,
            node_id,
            node,
            graph,
            &part,
            pending_reveal,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn part_row(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
    part: &str,
    pending_reveal: &mut Option<String>,
) {
    let port = format!("tex.{part}");
    let catalog = get(
        door,
        "GET",
        "/catalog",
        &[
            ("graph", gid.to_string()),
            ("anchor", node_id.to_string()),
            ("port", port.clone()),
        ],
    );
    let entries: Vec<Value> = catalog
        .as_ref()
        .and_then(|c| c["catalog"].as_array().cloned())
        .unwrap_or_default();
    let shaders: Vec<String> = catalog
        .as_ref()
        .and_then(|c| c["shaders"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect();

    let source = wired_source(graph, node_id, &port);
    let shown = match &source {
        Some(id) => name_of(graph, id),
        None => tr("dynamic colours").to_string(),
    };
    let mut picked = None;
    kv_row(ui, theme, &tr_dyn(part), |ui| {
        let (response, pick) = source_menu(
            ui,
            theme,
            part,
            &shown,
            source.as_deref(),
            graph,
            node_id,
            &entries,
            &shaders,
            door,
        );
        picked = pick;
        response
    });
    if let Some(pick) = picked {
        let body = attach_body(gid, node_id, &port, &pick);
        if let Ok(reply) = post_reply(door, "/graph/node", body) {
            if let Some(id) = reply["node"].as_str() {
                *pending_reveal = Some(id.to_string());
            }
        }
    }

    if source.is_none() {
        return;
    }
    let current = node["config"]["surfaces"][part].clone();
    let mut mix = current["mix"].as_f64().unwrap_or(1.0) as f32;
    let mut mapping = current["mapping"]
        .as_str()
        .unwrap_or(DEFAULT_MAPPING)
        .to_string();
    let mut scale = current["scale"].as_f64().unwrap_or(1.0) as f32;

    let options = [
        ("object".to_string(), tr("object")),
        ("projector".to_string(), tr("projector")),
    ];
    let mut changed = slider_row(ui, theme, tr("mix"), &mut mix, 0.0..=1.0, true).changed();
    if kv_row(ui, theme, tr("mapping"), |ui| {
        segmented(ui, theme, &mut mapping, &options)
    })
    .changed()
    {
        changed = true;
    }
    changed |= slider_row(ui, theme, tr("scale"), &mut scale, 0.1..=8.0, true).changed();
    if changed {
        post_surface(door, gid, node_id, node, part, mix, &mapping, scale);
    }
}

/// The dropdown: `dynamic colours`, then the graph's picture-making nodes,
/// the generators in relevance order, and the forge shaders. Returns the pick.
#[allow(clippy::too_many_arguments)]
fn source_menu(
    ui: &mut Ui,
    theme: &Theme,
    part: &str,
    shown: &str,
    current: Option<&str>,
    graph: &Value,
    node_id: &str,
    entries: &[Value],
    shaders: &[String],
    door: &dyn Call,
) -> (egui::Response, Option<Pick>) {
    let image_kinds: Vec<&str> = entries
        .iter()
        .filter(|k| {
            k["outputs"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|p| p["name"] == "image")
        })
        .filter_map(|k| k["kind"].as_str())
        .collect();
    let in_graph: Vec<(String, String)> = graph["graph"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let id = n["id"].as_str()?;
            let kind = n["kind"].as_str()?;
            (id != node_id && image_kinds.contains(&kind))
                .then(|| (id.to_string(), name_of(graph, id)))
        })
        .collect();
    let models: Vec<(String, String)> = entries
        .iter()
        .filter(|k| {
            k["family"] == "generator" && k["kind"].as_str().is_some_and(|k| k.starts_with("gen."))
        })
        .map(|k| {
            let kind = k["kind"].as_str().unwrap_or_default().to_string();
            let label = k["label"]
                .as_str()
                .filter(|l| !l.is_empty())
                .unwrap_or(&kind)
                .to_string();
            (kind, label)
        })
        .collect();

    let mut pick = None;
    let response =
        crate::shell::components::combo_box(theme, Id::new(("shell/region/properties/surface-source", part)))
            .width(ui.available_width())
            .selected_text((tr_dyn(shown)).to_string())
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(
                        current.is_none(),
                        small_text(theme, tr("dynamic colours")),
                    )
                    .clicked()
                {
                    pick = Some(Pick::Detach);
                    ui.close();
                }
                if !in_graph.is_empty() {
                    heading(ui, theme, tr("in the graph"));
                    for (id, label) in &in_graph {
                        if ui
                            .selectable_label(
                                current == Some(id.as_str()),
                                small_text(theme, tr_dyn(label)),
                            )
                            .clicked()
                        {
                            pick = Some(Pick::From(id.clone()));
                            ui.close();
                        }
                    }
                }
                if !models.is_empty() {
                    heading(ui, theme, tr("models"));
                    for (kind, label) in &models {
                        let button = ui.button(small_text(theme, tr_dyn(label)));
                        if let Some(id) = kind.strip_prefix("gen.") {
                            button.clone().on_hover_ui(|ui| super::poster::show(ui, theme, door, id));
                        }
                        if button.clicked() {
                            pick = Some(Pick::Kind {
                                kind: kind.clone(),
                                config: None,
                            });
                            ui.close();
                        }
                    }
                }
                if !shaders.is_empty() {
                    heading(ui, theme, tr("shaders"));
                    for id in shaders {
                        if ui
                            .button(small_text(theme, id.as_str()))
                            .clicked()
                        {
                            pick = Some(Pick::Kind {
                                kind: "gen.forge".to_string(),
                                config: Some(json!({ "shader": id })),
                            });
                            ui.close();
                        }
                    }
                }
            })
            .response;
    (response, pick)
}

fn heading(ui: &mut Ui, theme: &Theme, text: &str) {
    note(ui, theme, text);
}

/// The node wired into `node.port`, if any: the surface's current source.
pub(crate) fn wired_source(graph: &Value, node_id: &str, port: &str) -> Option<String> {
    graph["graph"]["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|e| {
            e["to"]["node"].as_str() == Some(node_id) && e["to"]["port"].as_str() == Some(port)
        })
        .and_then(|e| e["from"]["node"].as_str().map(str::to_string))
}

/// The `attach` body for a pick (rule 6: the same op the AI sends).
fn attach_body(gid: &str, node_id: &str, port: &str, pick: &Pick) -> Value {
    let mut body = json!({ "op": "attach", "graph": gid, "node": node_id, "port": port });
    match pick {
        Pick::Detach => {}
        Pick::From(id) => body["from"] = json!(id),
        Pick::Kind { kind, config } => {
            body["kind"] = json!(kind);
            if let Some(config) = config {
                body["config"] = config.clone();
            }
        }
    }
    body
}

/// Posts `config.surfaces` with `part` set to the given knobs. The config
/// merge is shallow, so the whole object goes, the other parts as they are.
#[allow(clippy::too_many_arguments)]
fn post_surface(
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    part: &str,
    mix: f32,
    mapping: &str,
    scale: f32,
) {
    let mut surfaces: Map<String, Value> = node["config"]["surfaces"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    surfaces.insert(
        part.to_string(),
        json!({ "mix": mix, "mapping": mapping, "scale": scale }),
    );
    let body = json!({
        "op": "config", "graph": gid, "node": node_id,
        "config": { "surfaces": Value::Object(surfaces) },
    });
    post(door, "/graph/node", body);
}
