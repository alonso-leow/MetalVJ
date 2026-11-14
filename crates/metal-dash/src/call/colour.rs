//! The overlays grid's colour built-ins: one producer node (`overlay-colour`,
//! a `palette` or a `color`) bound to every colour slot of the chosen nodes.
//! The shell sends the colour in the body; this module owns the graph
//! surgery, so a palette or an HSB fader lands in the same file the editor
//! reads. A slot the engine cannot fill is reported as `skipped`, never
//! silently dropped.

use metal_graph::params::{self, ParamKind};
use serde_json::{json, Value};

use crate::call::{CallError, Ctx, Params};

/// The one producer every built-in writes. Fixed id, so a second press
/// rewrites it rather than adding a node per click.
pub const PRODUCER: &str = "overlay-colour";

/// What the built-in brings: four stops, or one colour.
enum Source {
    Palette(Vec<[f32; 3]>),
    Hsb([f32; 3]),
    Rgb([f32; 3]),
}

/// Which nodes a press reaches: a fixed group, or the ids named.
enum Targets {
    Group(&'static str),
    Ids(Vec<String>),
}

/// The group a node belongs to, or `None` when it is no colour target.
/// `fx.text.*` is checked before `fx.` so text is never an effect.
fn group_of(kind: &str) -> Option<&'static str> {
    if kind.starts_with("gen.") {
        Some("models")
    } else if kind.starts_with("fx.text.") || kind == "text.source" {
        Some("text")
    } else if kind.starts_with("fx.") {
        Some("effects")
    } else {
        None
    }
}

fn unit(v: &Value) -> Option<f32> {
    let f = v.as_f64()? as f32;
    (f.is_finite() && (0.0..=1.0).contains(&f)).then_some(f)
}

fn triple(v: &Value) -> Option<[f32; 3]> {
    let a = v.as_array().filter(|a| a.len() == 3)?;
    Some([unit(&a[0])?, unit(&a[1])?, unit(&a[2])?])
}

fn source_of(body: &Value) -> Result<Source, CallError> {
    let bad = || CallError::Bad("colour: one of `palette` [[r,g,b]×4], `hsb` [h,s,b] or `rgb` [r,g,b], 0..1".into());
    let given = ["palette", "hsb", "rgb"].iter().filter(|k| !body[**k].is_null()).count();
    if given != 1 {
        return Err(bad());
    }
    if !body["palette"].is_null() {
        let stops = body["palette"].as_array().filter(|a| a.len() == 4).ok_or_else(bad)?;
        let colours = stops.iter().map(triple).collect::<Option<Vec<_>>>().ok_or_else(bad)?;
        return Ok(Source::Palette(colours));
    }
    if !body["hsb"].is_null() {
        return triple(&body["hsb"]).map(Source::Hsb).ok_or_else(bad);
    }
    triple(&body["rgb"]).map(Source::Rgb).ok_or_else(bad)
}

fn targets_of(body: &Value) -> Result<Targets, CallError> {
    let bad = || CallError::Bad("targets: \"all\", \"models\", \"effects\", \"text\" or [node ids]".into());
    match &body["targets"] {
        Value::String(s) => match s.as_str() {
            "all" => Ok(Targets::Group("all")),
            "models" => Ok(Targets::Group("models")),
            "effects" => Ok(Targets::Group("effects")),
            "text" => Ok(Targets::Group("text")),
            _ => Err(bad()),
        },
        Value::Array(ids) => ids
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .map(Targets::Ids)
            .ok_or_else(bad),
        _ => Err(bad()),
    }
}

/// The colour slots of one node, by name, in declaration order.
fn colour_params(node: &metal_graph::Node, catalog: &[metal_graph::NodeKind]) -> Vec<String> {
    let kind = catalog.iter().find(|k| k.kind == node.kind);
    params::node_params(node, kind)
        .into_iter()
        .filter(|p| p.kind == ParamKind::Colour)
        .map(|p| p.name)
        .collect()
}

/// Writes the producer (kind and config) at its fixed id, keeping its place
/// on the canvas. A kind change is remove + add, since the kind decides the
/// node's outputs.
fn write_producer(g: &mut metal_graph::Graph, source: &Source) -> Result<(), String> {
    let (kind, config) = match source {
        Source::Palette(colours) => (metal_graph::PALETTE_KIND, json!({ "colors": colours })),
        Source::Hsb([h, s, b]) => (metal_graph::COLOR_KIND, json!({ "h": h, "s": s, "b": b })),
        Source::Rgb(rgb) => (metal_graph::COLOR_KIND, json!({ "rgb": rgb })),
    };
    let pos = match g.node(PRODUCER) {
        Some(n) if n.kind == kind => n.pos,
        Some(n) => {
            let pos = n.pos;
            g.remove_node(PRODUCER);
            pos
        }
        None => {
            let left = g.nodes.iter().map(|n| n.pos[0]).fold(0.0_f32, f32::min);
            let top = g.nodes.iter().map(|n| n.pos[1]).fold(0.0_f32, f32::min);
            [left - 240.0, top]
        }
    };
    if g.node(PRODUCER).is_none() {
        g.nodes.push(metal_graph::Node::new(PRODUCER, kind, pos));
    }
    let config = metal_graph::Config::from_json_str(&config.to_string()).map_err(|e| e.0)?;
    g.node_mut(PRODUCER).expect("producer was just written").config = config;
    Ok(())
}

/// Binds every colour slot of `id` to the producer: a palette's stops in
/// order (slot `i` takes stop `i % 4`), or the colour variable of a `color`.
fn bind_node(
    g: &mut metal_graph::Graph,
    catalog: &[metal_graph::NodeKind],
    id: &str,
    source: &Source,
) -> Result<bool, String> {
    let Some(node) = g.node(id) else { return Ok(false) };
    let slots = colour_params(node, catalog);
    if slots.is_empty() {
        return Ok(false);
    }
    for (i, param) in slots.iter().enumerate() {
        let var = match source {
            Source::Palette(_) => metal_graph::palette_slot_var(i % metal_graph::PALETTE_SLOTS.len()),
            _ => "rgb".to_string(),
        };
        metal_graph::bind::bind(g, id, param, Some((PRODUCER, "control", &var)), catalog)
            .map_err(|e| format!("{id}.{param}: {e}"))?;
    }
    Ok(true)
}

/// The whole write, separated from the request so a test can drive it: the
/// producer, then each target, then the reply's `applied` and `skipped`.
fn apply_to(
    g: &mut metal_graph::Graph,
    catalog: &[metal_graph::NodeKind],
    source: &Source,
    targets: &Targets,
) -> Result<(Vec<String>, Vec<String>), String> {
    write_producer(g, source)?;
    let candidates: Vec<String> = match targets {
        Targets::Group(group) => g
            .nodes
            .iter()
            .filter(|n| n.id != PRODUCER)
            .filter(|n| *group == "all" || group_of(&n.kind) == Some(*group))
            .map(|n| n.id.clone())
            .collect(),
        Targets::Ids(ids) => ids.iter().filter(|i| i.as_str() != PRODUCER).cloned().collect(),
    };
    let (mut applied, mut skipped) = (Vec::new(), Vec::new());
    for id in candidates {
        if bind_node(g, catalog, &id, source)? {
            applied.push(id);
        } else {
            skipped.push(id);
        }
    }
    Ok((applied, skipped))
}

/// `POST /colour/apply`: `{graph, targets, palette? | hsb? | rgb?}`.
pub fn apply(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    apply_body(&body)
}

fn apply_body(body: &Value) -> Result<Value, CallError> {
    let Some(gid) = body["graph"].as_str() else {
        return Err(CallError::Bad("which graph".into()));
    };
    let source = source_of(body)?;
    let targets = targets_of(body)?;
    let mut g = crate::load_graph(gid).map_err(CallError::Bad)?;
    let catalog = crate::node_catalog().map_err(CallError::Bad)?;
    let (applied, skipped) = apply_to(&mut g, &catalog, &source, &targets).map_err(CallError::Bad)?;
    crate::save_graph(&g).map_err(CallError::Bad)?;
    let mut reply = crate::graph_reply(&g, &catalog);
    reply["applied"] = json!(applied);
    reply["skipped"] = json!(skipped);
    Ok(reply)
}

/// `GET /colour/targets?graph=`: the nodes that carry at least one colour
/// slot, grouped as the rules page lists them.
pub fn targets(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(gid) = params.param("graph") else {
        return Err(CallError::Bad("which graph".into()));
    };
    targets_of_graph(gid)
}

fn targets_of_graph(gid: &str) -> Result<Value, CallError> {
    let g = crate::load_graph(gid).map_err(CallError::Bad)?;
    let catalog = crate::node_catalog().map_err(CallError::Bad)?;
    let nodes: Vec<Value> = g
        .nodes
        .iter()
        .filter(|n| n.id != PRODUCER)
        .filter_map(|n| {
            let group = group_of(&n.kind)?;
            if colour_params(n, &catalog).is_empty() {
                return None;
            }
            let label = if n.label.is_empty() { n.kind.as_str() } else { n.label.as_str() };
            Some(json!({ "id": n.id, "kind": n.kind, "label": label, "group": group }))
        })
        .collect();
    Ok(json!({ "nodes": nodes }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// gen.turing and fx.particles both carry colour slots; fx.vignette and
    /// output.device do not. Saved to the scratch graphs dir.
    fn saved(tag: &str) -> (String, String, String, String, String) {
        let id = format!("colour-{tag}-{}", std::process::id());
        let mut g = metal_graph::Graph::new(&id, "colour", metal_graph::Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [200.0, 0.0]);
        let part = g.add_node("fx", "fx.particles", [400.0, 0.0]);
        let vig = g.add_node("fx", "fx.vignette", [400.0, 200.0]);
        let out = g.add_node("out", "output.device", [600.0, 0.0]);
        crate::save_graph(&g).expect("scratch graphs dir");
        (id, gen, part, vig, out)
    }

    fn graph_of(id: &str) -> metal_graph::Graph {
        crate::load_graph(id).expect("saved graph loads")
    }

    fn bound_vars(g: &metal_graph::Graph, node: &str) -> Vec<String> {
        g.edges
            .iter()
            .filter(|e| e.to.node == node && metal_graph::params::is_param_edge(e))
            .map(|e| e.from.port.clone())
            .collect()
    }

    #[test]
    fn a_palette_binds_each_colour_slot_to_a_stop_in_order() {
        let (id, gen, part, _, _) = saved("palette");
        let palette = json!([[0.0, 0.0, 1.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0], [0.5, 0.0, 1.0]]);
        let reply = apply_body(&json!({ "graph": id, "targets": "all", "palette": palette })).unwrap();
        let g = graph_of(&id);
        assert_eq!(g.node(PRODUCER).unwrap().kind, metal_graph::PALETTE_KIND);
        assert!(reply["applied"].as_array().unwrap().iter().any(|v| v == gen.as_str()));
        assert!(reply["applied"].as_array().unwrap().iter().any(|v| v == part.as_str()));
        assert_eq!(bound_vars(&g, &part), ["control.base"], "one slot takes the first stop");
        let gen_vars = bound_vars(&g, &gen);
        assert!(gen_vars.len() >= 2, "a generator holds several colour slots: {gen_vars:?}");
        assert!(gen_vars.contains(&"control.base".to_string()) && gen_vars.contains(&"control.accent".to_string()));
    }

    #[test]
    fn an_hsb_fader_writes_one_colour_node_and_binds_rgb() {
        let (id, _, part, _, _) = saved("hsb");
        apply_body(&json!({ "graph": id, "targets": [part.clone()], "hsb": [0.5, 1.0, 1.0] })).unwrap();
        let g = graph_of(&id);
        let producer = g.node(PRODUCER).unwrap();
        assert_eq!(producer.kind, metal_graph::COLOR_KIND);
        assert_eq!(producer.config.get("h"), Some(&json!(0.5)));
        assert!(bound_vars(&g, &part).iter().all(|v| v.ends_with("rgb")));
    }

    #[test]
    fn changing_the_kind_replaces_the_producer_in_place() {
        let (id, _, part, _, _) = saved("kind");
        apply_body(&json!({ "graph": id, "targets": [part.clone()], "palette": [[0,0,0],[0,0,0],[0,0,0],[0,0,0]] })).unwrap();
        let pos = graph_of(&id).node(PRODUCER).unwrap().pos;
        apply_body(&json!({ "graph": id, "targets": [part.clone()], "rgb": [1.0, 0.0, 0.0] })).unwrap();
        let g = graph_of(&id);
        assert_eq!(g.node(PRODUCER).unwrap().kind, metal_graph::COLOR_KIND);
        assert_eq!(g.node(PRODUCER).unwrap().pos, pos);
        assert_eq!(g.nodes.iter().filter(|n| n.id == PRODUCER).count(), 1);
    }

    #[test]
    fn a_node_with_no_colour_slot_is_skipped_not_bound() {
        let (id, _, _, vig, _) = saved("skip");
        let reply = apply_body(&json!({ "graph": id, "targets": [vig.clone(), "missing"], "rgb": [1.0, 1.0, 1.0] })).unwrap();
        assert_eq!(reply["applied"], json!([]));
        assert_eq!(reply["skipped"], json!([vig, "missing"]));
    }

    #[test]
    fn a_press_names_exactly_one_colour_and_valid_targets() {
        let (id, _, _, _, _) = saved("bad");
        for bad in [
            json!({ "graph": id, "targets": "all" }),
            json!({ "graph": id, "targets": "all", "rgb": [1.0, 0.0, 0.0], "hsb": [0.0, 0.0, 0.0] }),
            json!({ "graph": id, "targets": "all", "rgb": [2.0, 0.0, 0.0] }),
            json!({ "graph": id, "targets": "everything", "rgb": [1.0, 0.0, 0.0] }),
            json!({ "graph": id, "targets": "all", "palette": [[0, 0, 0]] }),
        ] {
            assert!(matches!(apply_body(&bad), Err(CallError::Bad(_))), "refused: {bad}");
        }
    }

    #[test]
    fn targets_lists_only_nodes_with_a_colour_slot_grouped() {
        let (id, gen, part, vig, _) = saved("targets");
        let reply = targets_of_graph(&id).unwrap();
        let nodes = reply["nodes"].as_array().unwrap();
        assert!(nodes.iter().any(|n| n["id"] == gen.as_str() && n["group"] == "models"));
        assert!(nodes.iter().any(|n| n["id"] == part.as_str() && n["group"] == "effects"));
        assert!(!nodes.iter().any(|n| n["id"] == vig.as_str()));
    }
}
