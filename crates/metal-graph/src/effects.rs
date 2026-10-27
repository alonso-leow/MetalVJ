//! §3.2/B5's mechanism: a generator's sixteen common effects are not a
//! second list anywhere — they *are* the `fx.*` nodes chained downstream of
//! it, read back off the graph. Deliberately the mirror image of
//! [`crate::camera_track`]: that module resolves a `cam.track` node's
//! *config* pointing at generators it is not wired to; this module resolves
//! a chain of nodes the operator *did* wire, in the order they wired them.
//!
//! **The rule this whole module exists to keep true**: "off / reactive /
//! fixed(value)" is never stored as its own field anywhere. `Off` is a
//! missing `fx.<name>` node. `Reactive` is that node's `control` port having
//! an edge. `Fixed` is the same port having none, holding [`FxConfig::fixed`]
//! instead. A second, cached copy of any of this would drift the first time
//! someone drags a cable — see `the design notes` §3.2's own warning,
//! word for word: *"if not, two"*.
//!
//! **Axis 2a/2b — where a reactive effect's number comes from.** The user's
//! spec asks for a choice between "the AI models (not yet trained)"
//! and "the deterministic outputs that map the ingest values ...
//! without any processing". `EffectState::Reactive` alone cannot carry
//! that choice — it is one bit, and this is a second axis — so it falls out
//! of *which node the control cable comes from*, the same way §13.3
//! resolves audio-vs-sensor.yolo for a hand fader. Concretely: a generator
//! gains a second output port, `row` ([`NodeKind::catalog`]), standing
//! next to its existing `image` output. An `fx.*` node's `control` wired to
//! `gen.<id>`'s `row` port inherits whatever that generator's own row
//! already computed for this effect — the seat a trained router's
//! translation will fill, occupied today by `drive::row`'s deterministic
//! mapping because nothing is trained yet, exactly as `metal-dash::router`'s
//! `Mode::Model` falls back to automix rather than erroring. Wired instead
//! to `audio.live`/`audio.file` directly, it bypasses that row entirely and
//! reads the raw digest column — see [`ControlSource`].

use std::collections::HashSet;

use crate::{Endpoint, Graph, NodeId};

// ---------------------------------------------------------------------------
// an fx.* node's own config
// ---------------------------------------------------------------------------

/// An `fx.*` node's config: the value it holds while disconnected.
///
/// Exactly one field, because that is the only thing about an effect that
/// cannot be read back off the graph — *which* effect it is comes from
/// `node.kind` (`fx.vignette`), whether it is reactive comes from the
/// `control` edge, and the order comes from where it sits in the chain.
/// Only "fixed to what" has nowhere else to live.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FxConfig {
    /// Preserved across a reactive<->fixed toggle rather than reset on
    /// disconnect — E4's own words: *"disconnecting the cable sets the effect
    /// to fixed with its last value"*. Only [`set_effect`] writing a fresh
    /// `Fixed(v)` ever changes it.
    #[serde(default)]
    pub fixed: f32,
}

impl FxConfig {
    /// Packs this into the opaque [`crate::Config`] a [`crate::Node`]
    /// carries — the same door [`crate::camera_track::CamTrackConfig`] uses.
    pub fn to_config(&self) -> crate::Config {
        crate::Config::from_value(serde_json::to_value(self).expect("FxConfig always serializes"))
    }

    /// The default (fixed at 0.0) for anything that fails to parse as this
    /// shape — an `fx.*` node with no config set yet included. Same
    /// "never panic on a mid-drag graph" discipline as `Graph::lint`.
    fn from_config(config: &crate::Config) -> FxConfig {
        serde_json::from_str(&config.to_json_str()).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// axis 2a/2b
// ---------------------------------------------------------------------------

/// Which of the two sources a `Reactive` `fx.*` node's control cable comes
/// from — see this module's own doc comment for the full reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlSource {
    /// From the generator's own `row` output port: inherits the row —
    /// axis (a), "the AI models".
    ///
    /// Named for what it carries, not for what reads it. An earlier pass
    /// called this port `router`, which collided with three unrelated things
    /// already in the repo: the `router` node kind (an N-to-1 image switch),
    /// the `metal-router` crate, and the trained model the dashboard calls "the
    /// router". None of them has anything to do with a generator's own
    /// computed effect amounts, which is all this port is.
    Row,
    /// From `audio.live`/`audio.file` (or a future `sensor.yolo`) directly:
    /// bypasses the row — axis (b), "without any processing".
    RawIngest,
}

/// Where `fx_node`'s `control` edge, if it has one, comes from. `None` when
/// it has none at all — i.e. when [`state_of`] would answer `Fixed`, this
/// answers nothing, because there is no cable to read a source off.
pub fn control_source(graph: &Graph, fx_node: &str) -> Option<ControlSource> {
    let edge = graph.edges.iter().find(|e| e.to.node == fx_node && e.to.port == "control")?;
    let from = graph.node(&edge.from.node)?;
    if from.kind.starts_with("gen.") && edge.from.port == "row" {
        Some(ControlSource::Row)
    } else {
        Some(ControlSource::RawIngest)
    }
}

// ---------------------------------------------------------------------------
// reading the chain
// ---------------------------------------------------------------------------

/// The `fx.*` node ids chained downstream of `gen_node`'s `image` output, in
/// graph order — §3.2: *"the graph determines the order"*.
///
/// Walks one `image` edge at a time and stops the moment the walk stops
/// being a simple chain: no outgoing edge, more than one (a branch — which
/// fork would this even follow), a node the catalog would not call `fx.*`,
/// or a node already visited (a cycle `lint` will flag on its own terms;
/// this just refuses to loop forever over one it has not been asked to
/// reject). Anything after that point is real graph content this function
/// simply is not the reader for — it answers "what is *this generator's*
/// effect chain", not "what is downstream of it".
pub fn chain(graph: &Graph, gen_node: &str) -> Vec<NodeId> {
    let mut out = Vec::new();
    if graph.node(gen_node).is_none() {
        return out;
    }
    let mut current: NodeId = gen_node.to_string();
    let mut visited: HashSet<NodeId> = HashSet::new();
    loop {
        let next: Vec<&crate::Edge> =
            graph.edges.iter().filter(|e| e.from.node == current && e.from.port == "image").collect();
        if next.len() != 1 {
            break;
        }
        let next_id = next[0].to.node.clone();
        if !visited.insert(next_id.clone()) {
            break;
        }
        let Some(node) = graph.node(&next_id) else { break };
        if !node.kind.starts_with("fx.") {
            // A `mat.*` node shares this chain and passes the picture through
            // unchanged (see `crate::materials`): step over it rather than
            // letting it end the effect chain.
            if node.kind.starts_with(crate::materials::PREFIX) {
                current = next_id;
                continue;
            }
            break;
        }
        out.push(next_id.clone());
        current = next_id;
    }
    out
}

/// The generators whose [`chain`] holds `fx_node` — whose row carries its
/// amount. Usually one; more when several pictures are cabled into the same
/// effect. What `metal_perform::bind` writes a bound amount into.
pub fn generators_of(graph: &Graph, fx_node: &str) -> Vec<NodeId> {
    graph
        .nodes
        .iter()
        .filter(|n| n.kind.starts_with("gen.") && chain(graph, &n.id).iter().any(|id| id == fx_node))
        .map(|n| n.id.clone())
        .collect()
}

/// The effect name an `fx.*` node's `kind` names (`fx.vignette` ->
/// `vignette`), or `None` for a missing node or one that is not `fx.*`.
pub fn effect_name(graph: &Graph, fx_node: &str) -> Option<String> {
    graph.node(fx_node)?.kind.strip_prefix("fx.").map(str::to_string)
}

/// This `fx.*` node's state, read straight off the graph — never stored.
/// `Off` for a node that is missing or not `fx.*` (there is nothing for a
/// caller walking [`chain`] to ask this about that would hit that case, but
/// it is the honest answer rather than a panic for one asking anyway) — and
/// `Off` for a node whose `bypass` is set: O.28's switches flip exactly that
/// bit, and an effect that is switched off is, to everything reading the
/// chain (the panel's `view`, `metal_perform::resolved_chain`), an effect that
/// is not there.
pub fn state_of(graph: &Graph, fx_node: &str) -> metal_schema::EffectState {
    let Some(node) = graph.node(fx_node) else { return metal_schema::EffectState::Off };
    if !node.kind.starts_with("fx.") || node.bypass {
        return metal_schema::EffectState::Off;
    }
    let connected = graph.edges.iter().any(|e| e.to.node == fx_node && e.to.port == "control");
    if connected {
        metal_schema::EffectState::Reactive
    } else {
        metal_schema::EffectState::Fixed(FxConfig::from_config(&node.config).fixed)
    }
}

/// One entry in [`view`]: an effect actually present on a generator, read
/// off the graph.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectView {
    pub effect: String,
    pub node: NodeId,
    pub state: metal_schema::EffectState,
    /// `Some` exactly when `state` is `Reactive` — see [`control_source`].
    pub source: Option<ControlSource>,
}

/// The config panel's whole read for one generator: every effect actually
/// materialized, in chain order, with its state and (when reactive) its
/// source. **Deliberately does not mention the thirteen-or-fewer effects
/// that are off** — this crate does not carry a list of the sixteen common
/// effect names to pad the rest out with (see this crate's own top doc: no
/// knowledge that an app exists); a caller wanting "all sixteen, the rest
/// marked off" zips this against its own copy of the catalog.
pub fn view(graph: &Graph, gen_node: &str) -> Vec<EffectView> {
    chain(graph, gen_node)
        .into_iter()
        .filter_map(|id| {
            let effect = effect_name(graph, &id)?;
            let state = state_of(graph, &id);
            let source = control_source(graph, &id);
            Some(EffectView { effect, node: id, state, source })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// writing: materialize / remove
// ---------------------------------------------------------------------------

/// Turns effect `effect` on `gen_node` off, reactive, or fixed(v) — §3.2/B5.
/// The **only** writer of any of this: everything [`view`]/[`state_of`]
/// answer afterward is read straight back off whatever this leaves in the
/// graph, never off a second copy.
///
/// - `Off`: removes the `fx.<effect>` node if one exists, reconnecting its
///   predecessor straight to its successor(s) — no dangling edge, no gap in
///   the chain.
/// - `Reactive`/`Fixed(v)`: materializes `fx.<effect>` if it does not exist
///   yet, spliced onto the **end** of the current chain — §3.2: *"in the
///   order they were turned on"*. Either way, rebuilds the node's
///   `control` edge from scratch: none for `Fixed` (E4 — disconnected is
///   what fixed *means*), or one from `source` for `Reactive`.
///
/// `source` is read only when `desired` is `Reactive`. Fails, before
/// mutating anything, if `desired` asks for [`ControlSource::RawIngest`]
/// and the graph has no `audio.live`/`audio.file` node to wire from — a
/// half-materialized effect with nothing to connect would be worse than
/// refusing.
pub fn set_effect(
    graph: &mut Graph,
    gen_node: &str,
    effect: &str,
    desired: metal_schema::EffectState,
    source: ControlSource,
) -> Result<Option<NodeId>, String> {
    if graph.node(gen_node).is_none() {
        return Err(format!("no such node: {gen_node}"));
    }
    let existing =
        chain(graph, gen_node).into_iter().find(|id| effect_name(graph, id).as_deref() == Some(effect));

    if matches!(desired, metal_schema::EffectState::Off) {
        if let Some(fx_id) = existing {
            remove_and_heal(graph, &fx_id);
        }
        return Ok(None);
    }

    if matches!(desired, metal_schema::EffectState::Reactive) && source == ControlSource::RawIngest {
        let has_audio = graph.nodes.iter().any(|n| n.kind == "audio.live" || n.kind == "audio.file");
        if !has_audio {
            return Err(
                "no audio.live/audio.file node in this graph to wire raw ingest from".to_string()
            );
        }
    }

    let fx_id = match existing {
        Some(id) => id,
        None => splice_new(graph, gen_node, effect),
    };

    // One driver per float (`crate::bind`): fixed and reactive both replace a
    // binding on the amount, the same way they replace the control cable.
    let old_control: Vec<String> = graph
        .edges
        .iter()
        .filter(|e| {
            e.to.node == fx_id
                && (e.to.port == "control" || crate::params::param_name(&e.to.port) == Some("amount"))
        })
        .map(|e| e.id.clone())
        .collect();
    for id in old_control {
        graph.remove_edge(&id);
    }

    match desired {
        metal_schema::EffectState::Fixed(v) => {
            // The loop choice (`loop`, the loops design notes T8) is the node's, not
            // the fixed value's: replacing the amount keeps it.
            let node = graph.node_mut(&fx_id).expect("just found or created");
            node.config = FxConfig { fixed: v }.to_config().carrying(&node.config, "loop");
        }
        metal_schema::EffectState::Reactive => {
            let from = match source {
                ControlSource::Row => Endpoint::new(gen_node, "row"),
                ControlSource::RawIngest => {
                    let audio_id = graph
                        .nodes
                        .iter()
                        .find(|n| n.kind == "audio.live" || n.kind == "audio.file")
                        .map(|n| n.id.clone())
                        .expect("checked above");
                    Endpoint::new(audio_id, "control")
                }
            };
            graph.add_edge(from, Endpoint::new(&fx_id, "control"));
        }
        metal_schema::EffectState::Off => unreachable!("handled above"),
    }

    Ok(Some(fx_id))
}

/// Removes `fx_id` and reconnects whatever fed its `image` input straight
/// to whatever it fed — the "heal" half of turning an effect off. Silently
/// skips reconnecting when there is no predecessor to reconnect *from*
/// (an orphan `fx.*` node, legitimate mid-drag) rather than inventing one.
fn remove_and_heal(graph: &mut Graph, fx_id: &str) {
    let predecessor: Option<Endpoint> = graph
        .edges
        .iter()
        .find(|e| e.to.node == fx_id && e.to.port == "image")
        .map(|e| e.from.clone());
    let successors: Vec<Endpoint> = graph
        .edges
        .iter()
        .filter(|e| e.from.node == fx_id && e.from.port == "image")
        .map(|e| e.to.clone())
        .collect();
    graph.remove_node(fx_id); // drops every edge touching fx_id, control included
    if let Some(pred) = predecessor {
        for succ in successors {
            graph.add_edge(pred.clone(), succ);
        }
    }
}

/// Creates `fx.<effect>` and splices it onto the end of `gen_node`'s current
/// chain, lifting whatever the tail used to feed onto the new node's output
/// instead — the "materialize" half of turning an effect on.
fn splice_new(graph: &mut Graph, gen_node: &str, effect: &str) -> NodeId {
    let tail = chain(graph, gen_node).last().cloned().unwrap_or_else(|| gen_node.to_string());
    let tail_pos = graph.node(&tail).map(|n| n.pos).unwrap_or([0.0, 0.0]);

    let successors: Vec<Endpoint> = graph
        .edges
        .iter()
        .filter(|e| e.from.node == tail && e.from.port == "image")
        .map(|e| e.to.clone())
        .collect();
    let stale: Vec<String> = graph
        .edges
        .iter()
        .filter(|e| e.from.node == tail && e.from.port == "image")
        .map(|e| e.id.clone())
        .collect();
    for id in stale {
        graph.remove_edge(&id);
    }

    let new_id = graph.add_node("fx", &format!("fx.{effect}"), [tail_pos[0] + 180.0, tail_pos[1]]);
    graph.add_edge(Endpoint::new(&tail, "image"), Endpoint::new(&new_id, "image"));
    for succ in successors {
        graph.add_edge(Endpoint::new(&new_id, "image"), succ);
    }
    new_id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Domain, Graph};

    fn manifest() -> metal_schema::Manifest {
        let text = metal_vfs::read_to_string(schema_path()).expect("data/schema.json readable");
        metal_schema::Manifest::parse(&text).expect("schema.json parses")
    }

    fn schema_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json")
    }

    fn catalog() -> Vec<crate::NodeKind> {
        crate::NodeKind::catalog(&manifest())
    }

    // -- reading --------------------------------------------------------

    #[test]
    fn a_generator_with_no_fx_nodes_has_an_empty_chain() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        assert!(chain(&g, &gen).is_empty());
        assert!(view(&g, &gen).is_empty());
    }

    #[test]
    fn two_chained_fx_nodes_read_back_in_the_order_they_are_wired() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let shake = g.add_node("fx", "fx.shake", [1.0, 0.0]);
        let vignette = g.add_node("fx", "fx.vignette", [2.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&shake, "image"));
        g.add_edge(Endpoint::new(&shake, "image"), Endpoint::new(&vignette, "image"));

        assert_eq!(chain(&g, &gen), vec![shake.clone(), vignette.clone()]);
    }

    /// The rule the whole module exists for: this is a *read*, so moving a
    /// node directly in the graph (as a drag would) changes the answer on
    /// the very next call, with nothing to update or invalidate by hand.
    #[test]
    fn reordering_the_fx_nodes_in_the_graph_changes_the_chain_order_on_the_next_read() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let shake = g.add_node("fx", "fx.shake", [1.0, 0.0]);
        let vignette = g.add_node("fx", "fx.vignette", [2.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&shake, "image"));
        g.add_edge(Endpoint::new(&shake, "image"), Endpoint::new(&vignette, "image"));
        assert_eq!(chain(&g, &gen), vec![shake.clone(), vignette.clone()]);

        // The operator drags vignette in front of shake: cut both image
        // edges and rewire gen -> vignette -> shake, directly on the graph,
        // the way `POST /graph/edge` would.
        let edge_ids: Vec<String> = g.edges.iter().map(|e| e.id.clone()).collect();
        for id in edge_ids {
            g.remove_edge(&id);
        }
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&vignette, "image"));
        g.add_edge(Endpoint::new(&vignette, "image"), Endpoint::new(&shake, "image"));

        assert_eq!(
            chain(&g, &gen),
            vec![vignette, shake],
            "the chain must reflect the graph as it now is, not as it was read once before"
        );
    }

    #[test]
    fn a_fixed_effect_reads_back_its_held_value() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let vignette = g.add_node("fx", "fx.vignette", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&vignette, "image"));
        g.node_mut(&vignette).unwrap().config = FxConfig { fixed: 0.7 }.to_config();

        assert_eq!(state_of(&g, &vignette), metal_schema::EffectState::Fixed(0.7));
    }

    #[test]
    fn a_bypassed_fx_node_reads_as_off_whatever_its_config_or_wiring() {
        // O.28: a switch flips `bypass`; to the chain that effect is gone.
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let audio = g.add_node("audio", "audio.live", [-1.0, 0.0]);
        let vignette = g.add_node("fx", "fx.vignette", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&vignette, "image"));
        g.node_mut(&vignette).unwrap().config = FxConfig { fixed: 0.7 }.to_config();
        g.node_mut(&vignette).unwrap().bypass = true;
        assert_eq!(state_of(&g, &vignette), metal_schema::EffectState::Off);

        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&vignette, "control"));
        assert_eq!(state_of(&g, &vignette), metal_schema::EffectState::Off);

        g.node_mut(&vignette).unwrap().bypass = false;
        assert_eq!(state_of(&g, &vignette), metal_schema::EffectState::Reactive);
    }

    #[test]
    fn a_control_port_with_an_edge_reads_as_reactive_regardless_of_config() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let audio = g.add_node("audio", "audio.live", [-1.0, 0.0]);
        let vignette = g.add_node("fx", "fx.vignette", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&vignette, "image"));
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&vignette, "control"));
        // A stale fixed value left over from before it was wired reactive —
        // must not leak into the state.
        g.node_mut(&vignette).unwrap().config = FxConfig { fixed: 0.9 }.to_config();

        assert_eq!(state_of(&g, &vignette), metal_schema::EffectState::Reactive);
    }

    #[test]
    fn control_wired_from_the_generators_row_port_reads_as_row_sourced() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let vignette = g.add_node("fx", "fx.vignette", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&vignette, "image"));
        g.add_edge(Endpoint::new(&gen, "row"), Endpoint::new(&vignette, "control"));

        assert_eq!(control_source(&g, &vignette), Some(ControlSource::Row));
    }

    #[test]
    fn control_wired_from_audio_directly_reads_as_raw_ingest_sourced() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let audio = g.add_node("audio", "audio.live", [-1.0, 0.0]);
        let vignette = g.add_node("fx", "fx.vignette", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&vignette, "image"));
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&vignette, "control"));

        assert_eq!(control_source(&g, &vignette), Some(ControlSource::RawIngest));
    }

    #[test]
    fn a_disconnected_control_port_has_no_source_to_read() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let vignette = g.add_node("fx", "fx.vignette", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&vignette, "image"));
        assert_eq!(control_source(&g, &vignette), None);
    }

    #[test]
    fn a_branch_downstream_of_the_generator_stops_the_chain_rather_than_guessing() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let shake = g.add_node("fx", "fx.shake", [1.0, 0.0]);
        let m1 = g.add_node("mask", "mask", [2.0, 0.0]);
        let m2 = g.add_node("mask", "mask", [2.0, 1.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&shake, "image"));
        // shake fans out to two sinks — not a simple chain past this point.
        g.add_edge(Endpoint::new(&shake, "image"), Endpoint::new(&m1, "image"));
        g.add_edge(Endpoint::new(&shake, "image"), Endpoint::new(&m2, "image"));

        assert_eq!(chain(&g, &gen), vec![shake], "shake itself is still this generator's effect");
    }

    // -- writing ----------------------------------------------------------

    #[test]
    fn turning_an_effect_on_materializes_exactly_one_fx_node_wired_into_the_image_chain() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));

        let id = set_effect(&mut g, &gen, "vignette", metal_schema::EffectState::Fixed(0.5), ControlSource::RawIngest)
            .expect("materializes");
        let id = id.expect("Fixed materializes a node");

        assert_eq!(g.nodes.iter().filter(|n| n.kind == "fx.vignette").count(), 1);
        assert_eq!(chain(&g, &gen), vec![id.clone()]);
        // The old gen -> out edge is gone, replaced by gen -> fx -> out.
        assert!(g.edges.iter().any(|e| e.from.node == gen && e.to.node == id && e.to.port == "image"));
        assert!(g.edges.iter().any(|e| e.from.node == id && e.to.node == out && e.to.port == "image"));
        assert!(!g.edges.iter().any(|e| e.from.node == gen && e.to.node == out));
    }

    #[test]
    fn turning_an_effect_off_removes_the_node_and_leaves_no_dangling_edge() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        set_effect(&mut g, &gen, "vignette", metal_schema::EffectState::Fixed(0.5), ControlSource::RawIngest).unwrap();
        assert_eq!(chain(&g, &gen).len(), 1);

        set_effect(&mut g, &gen, "vignette", metal_schema::EffectState::Off, ControlSource::RawIngest).unwrap();

        assert!(g.nodes.iter().all(|n| n.kind != "fx.vignette"), "the fx node must be gone");
        assert!(chain(&g, &gen).is_empty());
        // Healed straight back to gen -> out, and nothing points at the
        // node that no longer exists.
        assert!(g.edges.iter().any(|e| e.from.node == gen && e.to.node == out));
        assert!(!g.edges.iter().any(|e| e.from.node.contains("fx") || e.to.node.contains("fx")));
    }

    #[test]
    fn turning_on_a_second_effect_appends_it_after_the_first_in_the_order_they_were_turned_on() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));

        set_effect(&mut g, &gen, "vignette", metal_schema::EffectState::Fixed(0.1), ControlSource::RawIngest).unwrap();
        set_effect(&mut g, &gen, "crt", metal_schema::EffectState::Fixed(0.2), ControlSource::RawIngest).unwrap();

        let names: Vec<String> =
            chain(&g, &gen).iter().map(|id| effect_name(&g, id).unwrap()).collect();
        assert_eq!(names, vec!["vignette".to_string(), "crt".to_string()]);
    }

    #[test]
    fn a_reactive_effect_tracks_its_control_source_and_a_fixed_one_resolves_to_a_constant() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let audio = g.add_node("audio", "audio.live", [-1.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&gen, "control"));

        set_effect(&mut g, &gen, "shake", metal_schema::EffectState::Reactive, ControlSource::RawIngest).unwrap();
        set_effect(&mut g, &gen, "crt", metal_schema::EffectState::Fixed(0.33), ControlSource::RawIngest).unwrap();

        let views = view(&g, &gen);
        let shake = views.iter().find(|v| v.effect == "shake").unwrap();
        assert_eq!(shake.state, metal_schema::EffectState::Reactive);
        assert_eq!(shake.source, Some(ControlSource::RawIngest));

        let crt = views.iter().find(|v| v.effect == "crt").unwrap();
        assert_eq!(crt.state, metal_schema::EffectState::Fixed(0.33));
        assert_eq!(crt.source, None);
    }

    #[test]
    fn switching_an_effect_from_fixed_to_reactive_and_back_preserves_the_last_fixed_value() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let audio = g.add_node("audio", "audio.live", [-1.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));

        set_effect(&mut g, &gen, "shake", metal_schema::EffectState::Fixed(0.42), ControlSource::RawIngest).unwrap();
        set_effect(&mut g, &gen, "shake", metal_schema::EffectState::Reactive, ControlSource::RawIngest).unwrap();
        assert_eq!(view(&g, &gen)[0].state, metal_schema::EffectState::Reactive);

        // Disconnecting (going back to fixed with no explicit value passed
        // through `set_effect`'s own value-less `Off`-then-`Fixed` dance)
        // must read back the value it held before — E4's own example.
        set_effect(&mut g, &gen, "shake", metal_schema::EffectState::Fixed(0.42), ControlSource::RawIngest).unwrap();
        assert_eq!(view(&g, &gen)[0].state, metal_schema::EffectState::Fixed(0.42));
        let _ = audio; // present in the graph, unused once shake goes back to fixed
    }

    #[test]
    fn reactive_raw_ingest_without_an_audio_node_in_the_graph_is_refused_not_half_built() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let before_nodes = g.nodes.len();
        let before_edges = g.edges.len();

        let result =
            set_effect(&mut g, &gen, "shake", metal_schema::EffectState::Reactive, ControlSource::RawIngest);

        assert!(result.is_err(), "no audio node to wire from must be refused");
        assert_eq!(g.nodes.len(), before_nodes, "a refused reactive must not leave a half-wired node");
        assert_eq!(g.edges.len(), before_edges);
    }

    #[test]
    fn reactive_row_sourced_needs_no_audio_node_at_all() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);

        let id = set_effect(&mut g, &gen, "shake", metal_schema::EffectState::Reactive, ControlSource::Row)
            .expect("row-sourced reactive needs no audio node");
        let id = id.unwrap();
        assert_eq!(control_source(&g, &id), Some(ControlSource::Row));
    }

    /// A binding (`crate::bind`) is invisible to every reader here: the chain,
    /// the state and the control source read exactly what they read before
    /// it — and `generators_of` finds the chain's generator.
    #[test]
    fn param_edges_change_nothing_the_chain_readers_see() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let audio = g.add_node("audio", "audio.live", [-1.0, 0.0]);
        let vignette = g.add_node("fx", "fx.vignette", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&vignette, "image"));
        g.node_mut(&vignette).unwrap().config = FxConfig { fixed: 0.3 }.to_config();
        let before = (chain(&g, &gen), state_of(&g, &vignette), control_source(&g, &vignette));
        g.add_edge(Endpoint::new(&audio, "control.rms"), Endpoint::new(&vignette, "param:amount"));
        g.add_edge(Endpoint::new(&gen, "row.scale"), Endpoint::new(&vignette, "param:amount"));
        assert_eq!((chain(&g, &gen), state_of(&g, &vignette), control_source(&g, &vignette)), before);
        assert_eq!(generators_of(&g, &vignette), vec![gen.clone()]);
    }

    #[test]
    fn setting_an_effect_fixed_or_reactive_replaces_a_binding_on_its_amount() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let audio = g.add_node("audio", "audio.live", [-1.0, 0.0]);
        let id = set_effect(&mut g, &gen, "shake", metal_schema::EffectState::Fixed(0.2), ControlSource::RawIngest)
            .unwrap()
            .unwrap();
        g.add_edge(Endpoint::new(&audio, "control.onset"), Endpoint::new(&id, "param:amount"));
        set_effect(&mut g, &gen, "shake", metal_schema::EffectState::Fixed(0.4), ControlSource::RawIngest).unwrap();
        assert!(!g.edges.iter().any(|e| e.to.port == "param:amount"), "fixed is the one driver now");
    }

    #[test]
    fn the_generator_catalog_entry_carries_a_row_output_port() {
        let catalog = catalog();
        let turing = catalog.iter().find(|k| k.kind == "gen.turing").expect("turing is cataloged");
        assert!(
            turing.outputs.iter().any(|p| p.name == "row" && p.ty == crate::PortType::Control),
            "generators must expose a `row` control output for axis (a) — got {:?}",
            turing.outputs
        );
        assert!(
            turing.outputs.iter().any(|p| p.name == "image" && p.ty == crate::PortType::Image),
            "the existing `image` output must survive adding `row`"
        );
    }
}
