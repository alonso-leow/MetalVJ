//! A binding is an edge (rule 4: the graph *is* the binding — no copy of it in
//! any config): one variable of an upstream output port, cabled to one
//! parameter of the consumer.
//!
//! ```text
//! Edge { from: { node: "script-1", port: "control.x" }, to: { node: "fx-3", port: "param:amount" } }
//! ```
//!
//! The port syntax is [`crate::params`]'s. What this module adds:
//!
//! - **reading** a node's bindings ([`param_edges`], [`param_edge`],
//!   [`bindings`]);
//! - **the one validated writer** ([`bind`]): replace or remove a parameter's
//!   single edge — the body of `POST /graph/node {op: "bind"}` and of the AI's
//!   `bind_param`;
//! - **the check lint runs on every param edge** ([`check_param_edge`]): the
//!   parameter exists on the consumer, the variable on the producer, both are
//!   floats;
//! - **the node's whole I/O as one read** ([`io`]) — its cables in and out,
//!   its parameters with their bindings, and every upstream node's variables
//!   with whether this node uses them: `GET /graph/io` and the AI's `get_io`
//!   answer with exactly this.
//!
//! # The rules a param edge follows
//!
//! - **One driver per parameter.** At most one edge lands on a `param:` port
//!   (lint error otherwise). An effect's `amount` has a second possible driver,
//!   its legacy `control` cable (`effects::state_of`'s `Reactive`): both at
//!   once is a lint error too, and [`bind`] replaces the control cable, since
//!   in the one-cable-per-input model they are the same input.
//! - **Exempt from the stage order**, not from cycles: a script (Modulate) may
//!   drive a generator's float (Source) — the value arrives before the picture
//!   is drawn — but a binding that closes a loop is refused like any cable.
//! - **Never a picture.** `compile` gives a param edge no buffer: it makes no
//!   node an image source or sink. The runtime (`metal_perform::bind`) writes the
//!   mapped value into the consumer's row or config for the frame.
//! - Every reader that walks cables by port name (`effects::chain`,
//!   `effects::control_source`, `effects::state_of`, the script runtime's
//!   `control`/`floats` lookups, the mask contour reader, the sACN wire) reads
//!   exact port names, which a param edge never carries on the side it looks
//!   at — so none of them sees one.

use std::collections::HashMap;

use serde::Serialize;

use crate::params::{self, Param, ParamKind, Var, VarKind};
use crate::{Edge, Endpoint, Graph, Node, NodeId, NodeKind};

/// One binding into a node, read off its param edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Binding {
    pub edge: String,
    /// The parameter it drives, on the consumer.
    pub param: String,
    /// The producer.
    pub node: NodeId,
    /// The producer's output port (`control`, `row`).
    pub port: String,
    /// The variable on that port (`rms`, `x`, `out0`).
    pub var: String,
}

/// Every param edge landing on `node`, in edge order.
pub fn param_edges<'a>(graph: &'a Graph, node: &str) -> Vec<&'a Edge> {
    graph.edges.iter().filter(|e| e.to.node == node && params::is_param_edge(e)).collect()
}

/// The param edge driving `node`'s parameter `param`, if one does.
pub fn param_edge<'a>(graph: &'a Graph, node: &str, param: &str) -> Option<&'a Edge> {
    param_edges(graph, node).into_iter().find(|e| params::param_name(&e.to.port) == Some(param))
}

/// [`param_edges`], parsed. An edge whose `from.port` is not `<port>.<var>`
/// (lint reports it) is skipped rather than guessed at.
pub fn bindings(graph: &Graph, node: &str) -> Vec<Binding> {
    param_edges(graph, node)
        .into_iter()
        .filter_map(|e| {
            let param = params::param_name(&e.to.port)?;
            let (port, var) = params::split_var_port(&e.from.port)?;
            Some(Binding {
                edge: e.id.clone(),
                param: param.to_string(),
                node: e.from.node.clone(),
                port: port.to_string(),
                var: var.to_string(),
            })
        })
        .collect()
}

/// Whether an effect's `control` cable drives the same float its `amount`
/// parameter names: every `fx.*` whose amount `effects::state_of` reads —
/// not a text effect (its amount is `config.amount`; nothing reads its
/// `control` port) and not `fx.forge` (it has no amount).
pub fn control_drives_amount(kind: &str) -> bool {
    kind.starts_with("fx.") && !kind.starts_with("fx.text.") && kind != crate::FORGE_EFFECT_KIND
}

/// The check `lint` runs on every param edge: the parameter exists on the
/// consumer and is a float (a colour binding is the extension point), and
/// `from.port` names a float variable the producer really offers. `by_kind`
/// is the catalog by kind string, the map `lint` already builds.
pub(crate) fn check_param_edge(g: &Graph, by_kind: &HashMap<&str, &NodeKind>, e: &Edge) -> Result<(), String> {
    let name = params::param_name(&e.to.port).ok_or_else(|| format!("{} is not a parameter port", e.to.port))?;
    let consumer = g.node(&e.to.node).ok_or_else(|| format!("target node {} does not exist", e.to.node))?;
    let producer = g.node(&e.from.node).ok_or_else(|| format!("source node {} does not exist", e.from.node))?;
    let param = params::find_param(consumer, by_kind.get(consumer.kind.as_str()).copied(), name)
        .ok_or_else(|| format!("{} ({}) has no parameter {name:?}", consumer.id, consumer.kind))?;
    let var = params::resolve_var(producer, by_kind.get(producer.kind.as_str()).copied(), &e.from.port)?;
    compatible(&consumer.id, &param, producer, &var)
}

/// Whether a variable may drive a parameter: a float drives a float, a colour
/// drives a colour, and the two never cross — a single number cannot fill four
/// RGB stops, and a colour is not a number a `min..max` float can follow.
fn compatible(consumer: &str, param: &Param, producer: &Node, var: &Var) -> Result<(), String> {
    match (param.kind, var.kind) {
        (ParamKind::Float, VarKind::Float) => Ok(()),
        (ParamKind::Colour, VarKind::Colour) => Ok(()),
        (ParamKind::Colour, _) => Err(format!(
            "{consumer}.{} is a colour slot — bind a colour variable to it (a `color` or `palette` node's `control.<slot>`)",
            param.name
        )),
        (ParamKind::Float, VarKind::Colour | VarKind::Image) => Err(format!(
            "{}'s {} is {} — a float parameter cannot follow it",
            producer.id,
            var.port_ref(),
            if var.kind == VarKind::Image { "a picture" } else { "a colour" }
        )),
    }
}

/// Binds `consumer`'s parameter `param` to `from = (producer, port, var)` —
/// or, with `None`, unbinds it. The one validated writer of a binding.
///
/// Replaces whatever drove the parameter: its own param edge and, for an
/// effect's `amount`, the legacy `control` cable (one driver per float, see
/// the module doc). The new edge then goes through
/// [`Graph::add_edge_checked`], so a cycle, an unknown variable or a second
/// driver is refused by name and **nothing** changes — the replaced edges are
/// put back. Returns the new edge's id, or `None` after an unbind.
pub fn bind(
    graph: &mut Graph,
    consumer: &str,
    param: &str,
    from: Option<(&str, &str, &str)>,
    catalog: &[NodeKind],
) -> Result<Option<String>, String> {
    let by_kind: HashMap<&str, &NodeKind> = catalog.iter().map(|k| (k.kind.as_str(), k)).collect();
    let node = graph.node(consumer).ok_or_else(|| format!("no such node: {consumer}"))?;
    // The parameter exists, whatever its kind; whether a *particular* variable
    // may drive it is [`compatible`]'s job below, once the producer is known.
    let consumer_param = params::find_param(node, by_kind.get(node.kind.as_str()).copied(), param)
        .ok_or_else(|| format!("{} ({}) has no parameter {param:?}", node.id, node.kind))?;
    let replaces_control = param == "amount" && control_drives_amount(&node.kind);

    let from = match from {
        None => None,
        Some((producer, port, var)) => {
            let p = graph.node(producer).ok_or_else(|| format!("no such node: {producer}"))?;
            let from_port = params::var_port(port, var);
            let v = params::resolve_var(p, by_kind.get(p.kind.as_str()).copied(), &from_port)?;
            compatible(consumer, &consumer_param, p, &v)?;
            Some(Endpoint::new(producer, from_port))
        }
    };

    let to_port = params::param_port(param);
    let saved = graph.edges.clone();
    graph.edges.retain(|e| !(e.to.node == consumer && e.to.port == to_port));
    let Some(from) = from else { return Ok(None) };
    if replaces_control {
        graph.edges.retain(|e| !(e.to.node == consumer && e.to.port == "control"));
    }
    match graph.add_edge_checked(from, Endpoint::new(consumer, to_port), catalog) {
        Ok(id) => Ok(Some(id)),
        Err(errors) => {
            graph.edges = saved;
            Err(errors.join("; "))
        }
    }
}

// ---------------------------------------------------------------------------
// the node's whole I/O, as one read
// ---------------------------------------------------------------------------

/// What the running engine knows about a graph this frame — implemented by
/// `metal_perform::bind::LiveGraph`; `None` everywhere a caller has no engine
/// (the AI harness reading a file).
pub trait LiveValues {
    /// How many values `node`'s live-sized `control` (a script, a sensor)
    /// answered last frame.
    fn width(&self, node: &str) -> Option<usize>;
    /// `node`'s effective value of parameter `param` this frame — bound,
    /// held or driven.
    fn input(&self, node: &str, param: &str) -> Option<f32>;
    /// `node`'s current value of variable `var`.
    fn output(&self, node: &str, var: &str) -> Option<f32>;
}

/// One end of a cable, as the I/O read shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IoEnd {
    pub node: NodeId,
    pub label: String,
    pub kind: String,
    /// The port on *that* node: the producer's output port for an input, the
    /// consumer's input port (`image`, `param:amount`) for an output.
    pub port: String,
    /// The variable, when the cable is a binding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub var: Option<String>,
}

/// A cable landing on the node.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IoInput {
    pub edge: String,
    pub from: IoEnd,
    /// The node's own input port it lands on (`image`, `control`, `param:x`).
    pub to_port: String,
}

/// A cable leaving the node.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IoOutput {
    pub edge: String,
    pub to: IoEnd,
    /// The node's own output port it leaves (`image`, `control`, `row`).
    pub from_port: String,
    /// The variable, when the cable is a binding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub var: Option<String>,
}

/// The binding on a parameter.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IoBound {
    pub node: NodeId,
    pub port: String,
    pub var: String,
    pub edge: String,
}

/// A parameter, with its binding and this frame's value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IoParam {
    #[serde(flatten)]
    pub param: Param,
    pub bound: Option<IoBound>,
    pub live: Option<f32>,
}

/// A variable of an upstream node, and whether this node uses it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IoVar {
    #[serde(flatten)]
    pub var: Var,
    /// The `from.port` a cable from this variable carries ([`Var::port_ref`]).
    pub port_ref: String,
    /// Its value this frame, when the graph is being rendered.
    pub value: Option<f32>,
    /// `true` when `used_by` or `via` is non-empty: this node consumes it.
    pub used: bool,
    /// This node's parameters bound to it.
    pub used_by: Vec<String>,
    /// This node's own input ports that receive it whole through an ordinary
    /// cable — `image` for "the render" arriving, `control` for a digest row.
    pub via: Vec<String>,
}

/// One upstream node and what it offers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IoSource {
    pub node: NodeId,
    pub label: String,
    pub kind: String,
    /// Distance in cables: 1 for a node wired straight in.
    pub hops: usize,
    pub vars: Vec<IoVar>,
}

/// One node's inputs, outputs, parameters and sources — the read
/// `GET /graph/io` and the AI's `get_io` answer with.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeIo {
    pub node: NodeId,
    pub kind: String,
    pub label: String,
    pub inputs: Vec<IoInput>,
    pub outputs: Vec<IoOutput>,
    pub params: Vec<IoParam>,
    /// Every node upstream (transitive), nearest first — see
    /// [`params::upstream`].
    pub sources: Vec<IoSource>,
}

fn label_of(node: &Node, kind: Option<&NodeKind>) -> String {
    if !node.label.is_empty() {
        node.label.clone()
    } else {
        kind.map(|k| k.label.clone()).unwrap_or_else(|| node.kind.clone())
    }
}

/// `(port, var)` of a cable's `from.port`: split for a binding, whole for an
/// ordinary cable.
fn from_side(e: &Edge) -> (String, Option<String>) {
    match (params::is_param_edge(e), params::split_var_port(&e.from.port)) {
        (true, Some((port, var))) => (port.to_string(), Some(var.to_string())),
        _ => (e.from.port.clone(), None),
    }
}

/// `node`'s whole I/O — see [`NodeIo`]. `None` when there is no such node.
/// `live` fills the values (`params[].live`, `sources[].vars[].value`) and
/// sizes a running script's `out<k>` variables; without it those are `None`
/// and a script shows what it declares plus whatever a cable already names.
pub fn io(graph: &Graph, node: &str, catalog: &[NodeKind], live: Option<&dyn LiveValues>) -> Option<NodeIo> {
    let by_kind: HashMap<&str, &NodeKind> = catalog.iter().map(|k| (k.kind.as_str(), k)).collect();
    let kind_of = |n: &Node| by_kind.get(n.kind.as_str()).copied();
    let me = graph.node(node)?;
    let end = |id: &str, port: String, var: Option<String>| -> IoEnd {
        let n = graph.node(id);
        IoEnd {
            node: id.to_string(),
            label: n.map(|n| label_of(n, kind_of(n))).unwrap_or_else(|| id.to_string()),
            kind: n.map(|n| n.kind.clone()).unwrap_or_default(),
            port,
            var,
        }
    };

    let inputs = graph
        .edges
        .iter()
        .filter(|e| e.to.node == node)
        .map(|e| {
            let (port, var) = from_side(e);
            IoInput { edge: e.id.clone(), from: end(&e.from.node, port, var), to_port: e.to.port.clone() }
        })
        .collect();
    let outputs = graph
        .edges
        .iter()
        .filter(|e| e.from.node == node)
        .map(|e| {
            let (from_port, var) = from_side(e);
            IoOutput { edge: e.id.clone(), to: end(&e.to.node, e.to.port.clone(), None), from_port, var }
        })
        .collect();

    let bound = bindings(graph, node);
    let params = params::node_params(me, kind_of(me))
        .into_iter()
        .map(|param| {
            let bound = (param.kind == ParamKind::Float)
                .then(|| bound.iter().find(|b| b.param == param.name))
                .flatten()
                .map(|b| IoBound { node: b.node.clone(), port: b.port.clone(), var: b.var.clone(), edge: b.edge.clone() });
            let live = live.and_then(|l| l.input(node, &param.name));
            IoParam { param, bound, live }
        })
        .collect();

    let sources = params::upstream(graph, node)
        .into_iter()
        .filter_map(|(id, hops)| {
            let n = graph.node(&id)?;
            let mut vars = params::node_vars(n, kind_of(n), live.and_then(|l| l.width(&id)));
            // A variable a binding already names always shows, even before the
            // script that produces it has run (`out<k>` is sized live).
            for e in graph.edges.iter().filter(|e| e.from.node == id && params::is_param_edge(e)) {
                if vars.iter().any(|v| v.port_ref() == e.from.port) {
                    continue;
                }
                if let Ok(v) = params::resolve_var(n, kind_of(n), &e.from.port) {
                    vars.push(v);
                }
            }
            let vars = vars
                .into_iter()
                .map(|var| {
                    let port_ref = var.port_ref();
                    let used_by: Vec<String> = bound
                        .iter()
                        .filter(|b| b.node == id && params::var_port(&b.port, &b.var) == port_ref)
                        .map(|b| b.param.clone())
                        .collect();
                    let mut via: Vec<String> = graph
                        .edges
                        .iter()
                        .filter(|e| {
                            e.from.node == id && e.to.node == node && !params::is_param_edge(e) && e.from.port == var.port
                        })
                        .map(|e| e.to.port.clone())
                        .collect();
                    via.dedup();
                    let value = live.and_then(|l| l.output(&id, &var.name));
                    IoVar { used: !used_by.is_empty() || !via.is_empty(), port_ref, value, used_by, via, var }
                })
                .collect();
            Some(IoSource { label: label_of(n, kind_of(n)), kind: n.kind.clone(), node: id, hops, vars })
        })
        .collect();

    Some(NodeIo {
        node: me.id.clone(),
        kind: me.kind.clone(),
        label: label_of(me, kind_of(me)),
        inputs,
        outputs,
        params,
        sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, Domain, Severity};

    fn catalog() -> Vec<NodeKind> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json");
        let manifest = metal_schema::Manifest::parse(&metal_vfs::read_to_string(path).unwrap()).unwrap();
        NodeKind::catalog(&manifest)
    }

    fn script(g: &mut Graph, outputs: serde_json::Value) -> NodeId {
        let id = g.add_node("script", "script.py", [0.0, 0.0]);
        g.node_mut(&id).unwrap().config = Config::from_value(serde_json::json!({
            "code": "def process(floats, row):\n    return [0.5]\n",
            "floats": [{ "name": "gain", "min": 0.0, "max": 1.0, "default": 0.5 }],
            "outputs": outputs,
        }));
        id
    }

    /// audio → gen → fx.vignette → out, and a script beside it: the chain every
    /// test below binds into.
    fn chain(g: &mut Graph) -> (NodeId, NodeId, NodeId, NodeId) {
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        let fx = g.add_node("fx", "fx.vignette", [2.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&gen, "control"));
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&fx, "image"));
        g.add_edge(Endpoint::new(&fx, "image"), Endpoint::new(&out, "image"));
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&out, "control"));
        (audio, gen, fx, out)
    }

    fn errors(g: &Graph, catalog: &[NodeKind]) -> Vec<String> {
        g.lint(catalog).into_iter().filter(|i| i.severity == Severity::Error).map(|i| i.message).collect()
    }

    #[test]
    fn a_script_output_binds_to_an_effects_amount_and_lints_clean() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let (audio, _gen, fx, _out) = chain(&mut g);
        let s = script(&mut g, serde_json::json!(["x"]));
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&s, "control"));
        let edge = bind(&mut g, &fx, "amount", Some((&s, "control", "x")), &catalog).unwrap().unwrap();
        let e = g.edges.iter().find(|e| e.id == edge).unwrap();
        assert_eq!((e.from.port.as_str(), e.to.port.as_str()), ("control.x", "param:amount"));
        assert!(errors(&g, &catalog).is_empty(), "{:?}", errors(&g, &catalog));
        assert_eq!(
            bindings(&g, &fx),
            vec![Binding { edge, param: "amount".into(), node: s.clone(), port: "control".into(), var: "x".into() }]
        );
    }

    #[test]
    fn bind_refuses_an_unknown_param_or_var_and_changes_nothing() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let (audio, gen, fx, _out) = chain(&mut g);
        let before = g.edges.clone();
        assert!(bind(&mut g, &fx, "nope", Some((&audio, "control", "rms")), &catalog).unwrap_err().contains("no parameter"));
        assert!(bind(&mut g, &fx, "amount", Some((&audio, "control", "nope")), &catalog).unwrap_err().contains("no variable"));
        assert!(bind(&mut g, &fx, "amount", Some((&gen, "image", "image")), &catalog).is_err(), "a picture drives nothing");
        assert_eq!(g.edges, before);
    }

    /// A raw second edge on the same parameter (the canvas dropping a cable on
    /// an occupied row through `POST /graph/edge`) is refused; `bind` itself
    /// replaces.
    #[test]
    fn a_param_takes_one_edge_and_bind_replaces_it() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let (audio, gen, _fx, _out) = chain(&mut g);
        bind(&mut g, &gen, "scale", Some((&audio, "control", "rms")), &catalog).unwrap();
        let second = g.add_edge_checked(Endpoint::new(&audio, "control.onset"), Endpoint::new(&gen, "param:scale"), &catalog);
        assert!(second.unwrap_err().iter().any(|m| m.contains("driven by 2")), "a second driver is refused");
        bind(&mut g, &gen, "scale", Some((&audio, "control", "onset")), &catalog).unwrap();
        let b = bindings(&g, &gen);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].var, "onset");
        assert_eq!(bind(&mut g, &gen, "scale", None, &catalog).unwrap(), None);
        assert!(bindings(&g, &gen).is_empty(), "unbinding removes the edge");
    }

    #[test]
    fn binding_an_effects_amount_replaces_its_control_cable() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let (audio, _gen, fx, _out) = chain(&mut g);
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&fx, "control"));
        assert_eq!(crate::effects::state_of(&g, &fx), metal_schema::EffectState::Reactive);
        // Wired raw, both at once, the amount has two drivers.
        let raw = g.add_edge_checked(Endpoint::new(&audio, "control.onset"), Endpoint::new(&fx, "param:amount"), &catalog);
        assert!(raw.unwrap_err().iter().any(|m| m.contains("driven twice")));
        bind(&mut g, &fx, "amount", Some((&audio, "control", "onset")), &catalog).unwrap();
        assert!(!g.edges.iter().any(|e| e.to.node == fx && e.to.port == "control"), "one driver per float");
        assert!(errors(&g, &catalog).is_empty(), "{:?}", errors(&g, &catalog));
    }

    /// A script (Modulate) may drive a generator (Source) through a binding —
    /// the one backward cable a param edge is allowed — but the same pair as
    /// an ordinary control cable is still a backward edge.
    #[test]
    fn a_backward_stage_is_allowed_for_a_param_edge_only() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let (_audio, gen, _fx, _out) = chain(&mut g);
        let s = script(&mut g, serde_json::json!([]));
        assert!(bind(&mut g, &gen, "scale", Some((&s, "control", "out0")), &catalog).is_ok());
        let backward = g.add_edge_checked(Endpoint::new(&s, "control"), Endpoint::new(&gen, "control"), &catalog);
        assert!(backward.unwrap_err().iter().any(|m| m.contains("backward edge")));
    }

    #[test]
    fn a_binding_that_closes_a_loop_is_refused() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let (_audio, gen, _fx, _out) = chain(&mut g);
        let s = script(&mut g, serde_json::json!([]));
        g.add_edge(Endpoint::new(&gen, "row"), Endpoint::new(&s, "control"));
        let err = bind(&mut g, &gen, "scale", Some((&s, "control", "out0")), &catalog).unwrap_err();
        assert!(err.contains("cycle"), "{err}");
        assert!(bindings(&g, &gen).is_empty());
    }

    #[test]
    fn a_colour_slot_cannot_be_bound_yet() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let (audio, gen, _fx, _out) = chain(&mut g);
        let colour = catalog
            .iter()
            .find(|k| k.kind == "gen.turing")
            .unwrap()
            .params
            .iter()
            .find(|p| p.kind == ParamKind::Colour)
            .unwrap()
            .name
            .clone();
        let err = bind(&mut g, &gen, &colour, Some((&audio, "control", "rms")), &catalog).unwrap_err();
        assert!(err.contains("colour"), "{err}");
    }

    /// K: a colour node's `rgb` binds to a generator's colour slot — the pair
    /// `compatible` accepts and `check_param_edge` lets through lint clean.
    #[test]
    fn a_colour_variable_binds_to_a_colour_slot() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let (_audio, gen, _fx, _out) = chain(&mut g);
        let colour = g.add_node("color", crate::COLOR_KIND, [0.0, 0.0]);
        let slot = catalog
            .iter()
            .find(|k| k.kind == "gen.turing")
            .unwrap()
            .params
            .iter()
            .find(|p| p.kind == ParamKind::Colour)
            .unwrap()
            .name
            .clone();
        let edge = bind(&mut g, &gen, &slot, Some((&colour, "control", "rgb")), &catalog).unwrap().unwrap();
        let e = g.edges.iter().find(|e| e.id == edge).unwrap();
        assert_eq!((e.from.port.as_str(), e.to.port.as_str()), ("control.rgb", format!("param:{slot}").as_str()));
        assert!(errors(&g, &catalog).is_empty(), "{:?}", errors(&g, &catalog));
    }

    /// The two directions a colour must not cross: a float variable on a colour
    /// slot, and a colour variable on a float parameter.
    #[test]
    fn a_colour_and_a_float_never_cross() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let (audio, gen, _fx, _out) = chain(&mut g);
        let colour = g.add_node("color", crate::COLOR_KIND, [0.0, 0.0]);
        let slot = catalog
            .iter()
            .find(|k| k.kind == "gen.turing")
            .unwrap()
            .params
            .iter()
            .find(|p| p.kind == ParamKind::Colour)
            .unwrap()
            .name
            .clone();
        let err = bind(&mut g, &gen, &slot, Some((&audio, "control", "rms")), &catalog).unwrap_err();
        assert!(err.contains("colour slot"), "{err}");
        let err = bind(&mut g, &gen, "scale", Some((&colour, "control", "rgb")), &catalog).unwrap_err();
        assert!(err.contains("float parameter cannot follow"), "{err}");
    }

    /// `GET /graph/io`'s read: a transitive upstream's variables, nearest
    /// first, with which of them this node uses — a binding (`used_by`) or a
    /// whole cable (`via`, "the render" arriving).
    #[test]
    fn io_lists_a_transitive_upstreams_vars_with_what_this_node_uses() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let (audio, gen, fx, _out) = chain(&mut g);
        let s = script(&mut g, serde_json::json!(["x"]));
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&s, "control"));
        bind(&mut g, &fx, "amount", Some((&s, "control", "x")), &catalog).unwrap();

        let io = io(&g, &fx, &catalog, None).unwrap();
        assert_eq!(io.kind, "fx.vignette");
        let order: Vec<(&str, usize)> = io.sources.iter().map(|s| (s.node.as_str(), s.hops)).collect();
        assert_eq!(order, vec![(gen.as_str(), 1), (s.as_str(), 1), (audio.as_str(), 2)]);

        let from_script = &io.sources[1];
        let x = from_script.vars.iter().find(|v| v.var.name == "x").unwrap();
        assert_eq!((x.used, x.used_by.clone(), x.port_ref.as_str()), (true, vec!["amount".to_string()], "control.x"));

        let from_gen = &io.sources[0];
        let image = from_gen.vars.iter().find(|v| v.var.kind == VarKind::Image).unwrap();
        assert_eq!((image.used, image.via.clone()), (true, vec!["image".to_string()]), "the render arrives");
        assert!(from_gen.vars.iter().filter(|v| v.var.port == "row").all(|v| !v.used), "received, unused");

        let audio_row = &io.sources[2];
        assert_eq!(audio_row.vars.len(), metal_schema::DIGEST_COLUMNS.len() + params::REPEAT_VARS.len());
        assert!(audio_row.vars.iter().all(|v| !v.used), "two hops away: nothing wired straight in");

        let amount = &io.params[0];
        assert_eq!(amount.param.name, "amount");
        assert_eq!(amount.bound.as_ref().map(|b| (b.node.as_str(), b.var.as_str())), Some((s.as_str(), "x")));
        let input = io.inputs.iter().find(|i| i.to_port == "param:amount").unwrap();
        assert_eq!((input.from.port.as_str(), input.from.var.as_deref()), ("control", Some("x")));
        assert_eq!(io.outputs.len(), 1);
        assert_eq!(io.outputs[0].to.port, "image");
    }

    struct FakeLive;
    impl LiveValues for FakeLive {
        fn width(&self, _node: &str) -> Option<usize> {
            Some(3)
        }
        fn input(&self, _node: &str, param: &str) -> Option<f32> {
            (param == "amount").then_some(0.25)
        }
        fn output(&self, _node: &str, var: &str) -> Option<f32> {
            (var == "out2").then_some(0.75)
        }
    }

    #[test]
    fn io_takes_its_values_and_live_widths_from_the_engine() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let (_audio, _gen, fx, _out) = chain(&mut g);
        let s = script(&mut g, serde_json::json!([]));
        bind(&mut g, &fx, "amount", Some((&s, "control", "out1")), &catalog).unwrap();
        let io = io(&g, &fx, &catalog, Some(&FakeLive)).unwrap();
        assert_eq!(io.params[0].live, Some(0.25));
        let src = io.sources.iter().find(|x| x.node == s).unwrap();
        assert_eq!(src.vars.iter().map(|v| v.var.name.as_str()).collect::<Vec<_>>(), ["out0", "out1", "out2"]);
        assert_eq!(src.vars[2].value, Some(0.75));
        assert!(src.vars[1].used);
    }
}
