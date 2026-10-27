//! **Efectos** on a `gen.*` node (plan "Surface sources and less-friction
//! authoring", step 2): the effects chained downstream of the generator, in
//! graph order, each one a label that selects it and a `×` that takes it out;
//! and `+ efecto ▾`, the effect kinds in relevance order for this generator.
//! A pick is inserted after the chain's last node and revealed on the canvas.
//!
//! Every write goes through the door (rule 6): `insert` for a pick; for the
//! `×`, the chain is closed around the removed node with `attach` (each
//! consumer of the removed effect reads the node before it), then `remove`.
//! The chain is read off the `GET /graph` reply the browser already holds,
//! over the same `image` edges `metal_graph::effects::chain` walks.

use crate::shell::components::small_text;
use egui::Ui;
use serde_json::{json, Value};

use super::browser::RowKey;
use super::shared::{get, post_reply};
use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{kv_wrap, note};
use crate::shell::i18n::{tr, tr_dyn};

/// The `fx.*` node ids downstream of `gen` along `image` edges, in graph
/// order. Stops where the walk stops being a simple chain (a branch, a
/// non-effect, a node already seen), as `metal_graph::effects::chain` does.
pub(crate) fn chain_of(graph: &Value, gen: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = gen.to_string();
    loop {
        let next: Vec<String> = edges(graph)
            .filter(|e| {
                e["from"]["node"].as_str() == Some(current.as_str()) && e["from"]["port"] == "image"
            })
            .filter_map(|e| e["to"]["node"].as_str().map(str::to_string))
            .collect();
        let [id] = next.as_slice() else { break };
        if id == gen || out.contains(id) {
            break;
        }
        let Some(node) = find_node(graph, id) else {
            break;
        };
        if !node["kind"].as_str().unwrap_or_default().starts_with("fx.") {
            break;
        }
        out.push(id.clone());
        current = id.clone();
    }
    out
}

fn edges(graph: &Value) -> impl Iterator<Item = &Value> {
    graph["graph"]["edges"].as_array().into_iter().flatten()
}

fn find_node<'a>(graph: &'a Value, id: &str) -> Option<&'a Value> {
    graph["graph"]["nodes"]
        .as_array()?
        .iter()
        .find(|n| n["id"].as_str() == Some(id))
}

/// A node's name on the chain: its label when it has one, else its kind.
pub(crate) fn name_of(graph: &Value, id: &str) -> String {
    let Some(node) = find_node(graph, id) else {
        return id.to_string();
    };
    match node["label"].as_str().filter(|l| !l.is_empty()) {
        Some(label) => label.to_string(),
        None => node["kind"].as_str().unwrap_or(id).to_string(),
    }
}

/// The section. Returns the chain entry a click selects, for the browser to
/// take as a selection (`pending_selection`).
pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    gen: &str,
    graph: &Value,
    pending_reveal: &mut Option<String>,
) -> Option<RowKey> {
    let mut clicked = None;
    let chain = chain_of(graph, gen);
    if chain.is_empty() {
        note(ui, theme, tr("— no effects downstream yet —"));
    }
    kv_wrap(ui, theme, "", |ui| {
        for (i, id) in chain.iter().enumerate() {
            let name = name_of(graph, id);
            if ui
                .button(small_text(theme, tr_dyn(&name)))
                .clicked()
            {
                clicked = Some(RowKey::Text(id.clone()));
            }
            let remove = ui
                .small_button(small_text(theme, "×"))
                .on_hover_text(tr("remove this effect"));
            if remove.clicked() {
                remove_from_chain(door, gid, graph, gen, &chain, i);
            }
        }
    });

    // The pick is the chain's last node, or the generator itself.
    let after = chain.last().cloned().unwrap_or_else(|| gen.to_string());
    let catalog = get(
        door,
        "GET",
        "/catalog",
        &[("graph", gid.to_string()), ("anchor", gen.to_string())],
    );
    let effects: Vec<(String, String)> = catalog
        .as_ref()
        .and_then(|c| c["catalog"].as_array())
        .into_iter()
        .flatten()
        .filter(|k| k["family"] == json!("effect"))
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
    kv_wrap(ui, theme, "", |ui| {
        ui.menu_button(
            small_text(theme, format!("{} ", tr("+ effect"))),
            |ui| {
                for (kind, label) in &effects {
                    if ui
                        .button(small_text(theme, tr_dyn(label)))
                        .clicked()
                    {
                        let body =
                            json!({ "op": "insert", "graph": gid, "after": after, "kind": kind });
                        if let Ok(reply) = post_reply(door, "/graph/node", body) {
                            if let Some(id) = reply["node"].as_str() {
                                *pending_reveal = Some(id.to_string());
                            }
                        }
                        ui.close();
                    }
                }
            },
        );
    });
    clicked
}

/// Takes `chain[index]` out of the generator's chain: every consumer of its
/// picture is wired to what fed it (`attach`, which replaces the port's
/// cable), then the node is removed. Nothing else in the graph moves.
fn remove_from_chain(
    door: &dyn Call,
    gid: &str,
    graph: &Value,
    gen: &str,
    chain: &[String],
    index: usize,
) {
    let fx = &chain[index];
    let feed = match index {
        0 => gen,
        _ => chain[index - 1].as_str(),
    };
    let consumers: Vec<(String, String)> = edges(graph)
        .filter(|e| e["from"]["node"].as_str() == Some(fx.as_str()) && e["from"]["port"] == "image")
        .filter_map(|e| {
            Some((
                e["to"]["node"].as_str()?.to_string(),
                e["to"]["port"].as_str()?.to_string(),
            ))
        })
        .collect();
    for (node, port) in consumers {
        let body =
            json!({ "op": "attach", "graph": gid, "node": node, "port": port, "from": feed });
        let _ = post_reply(door, "/graph/node", body);
    }
    let _ = post_reply(
        door,
        "/graph/node",
        json!({ "op": "remove", "graph": gid, "node": fx }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(nodes: &[(&str, &str)], edges: &[(&str, &str, &str, &str)]) -> Value {
        json!({
            "graph": {
                "nodes": nodes.iter().map(|(id, kind)| json!({ "id": id, "kind": kind, "label": "" })).collect::<Vec<_>>(),
                "edges": edges.iter().map(|(a, ap, b, bp)| json!({
                    "from": { "node": a, "port": ap }, "to": { "node": b, "port": bp }
                })).collect::<Vec<_>>(),
            }
        })
    }

    /// The chain walks `image` edges from the generator and stops at a branch
    /// or a non-effect; a `control` cable is not part of it.
    #[test]
    fn the_chain_is_the_fx_nodes_downstream_in_order() {
        let g = graph(
            &[
                ("gen", "gen.turing"),
                ("a", "fx.shake"),
                ("b", "fx.grade"),
                ("out", "output.device"),
                ("au", "audio.live"),
            ],
            &[
                ("gen", "image", "a", "image"),
                ("a", "image", "b", "image"),
                ("b", "image", "out", "image"),
                ("au", "control", "gen", "control"),
            ],
        );
        assert_eq!(chain_of(&g, "gen"), vec!["a".to_string(), "b".to_string()]);
        assert!(chain_of(&g, "out").is_empty());

        // A branch after `a` ends the chain there: which fork would it follow?
        let mut branched = g.clone();
        branched["graph"]["edges"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "from": { "node": "a", "port": "image" }, "to": { "node": "out", "port": "image" } }));
        assert_eq!(chain_of(&branched, "gen"), vec!["a".to_string()]);
    }

    /// `×` closes the chain around the removed effect: its consumer reads
    /// what fed it (`attach`), then the effect goes (`remove`).
    #[test]
    fn removing_an_effect_reconnects_its_consumer_then_removes_it() {
        let g = graph(
            &[
                ("gen", "gen.turing"),
                ("a", "fx.shake"),
                ("b", "fx.grade"),
                ("out", "output.device"),
            ],
            &[
                ("gen", "image", "a", "image"),
                ("a", "image", "b", "image"),
                ("b", "image", "out", "image"),
            ],
        );
        let chain = chain_of(&g, "gen");
        let door = crate::shell::FakeDoor::new();
        remove_from_chain(&door, "g1", &g, "gen", &chain, 0);
        let ops: Vec<Value> = door
            .calls
            .borrow()
            .iter()
            .map(|(_, _, p)| serde_json::from_slice(&p.body).unwrap())
            .collect();
        assert_eq!(
            ops,
            vec![
                json!({ "op": "attach", "graph": "g1", "node": "b", "port": "image", "from": "gen" }),
                json!({ "op": "remove", "graph": "g1", "node": "a" }),
            ]
        );
    }
}
