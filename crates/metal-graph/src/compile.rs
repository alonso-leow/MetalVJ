//! `Graph::compile` — turn a graph into a `Plan`: topological order,
//! intermediate-buffer reuse, and dedup.
//!
//! **The graph is compiled, not interpreted** (`the design notes` §6):
//! every time the graph changes, this produces a plan once, and the frame
//! loop just walks the plan — it never re-walks nodes or re-resolves edges
//! per frame. This module is the part of that decision that lives in
//! `metal-graph`; nothing here touches a pixel, it only decides *order* and
//! *which buffer slot* each node's output lands in.
//!
//! Dedup falls out of the topological walk for free: each node is visited
//! exactly once regardless of its fan-out, so a generator feeding three masks
//! appears once in [`Plan::ops`], not three times — see §6 invariant 4.

use std::collections::{HashMap, VecDeque};

use crate::{Graph, NodeId};

/// One step of the plan: render `node`, reading `inputs` (buffer indices, in
/// the order its incoming edges were declared) and — if anything downstream
/// consumes its output — writing to `buffer`.
///
/// `buffer` is `None` for a pure sink (`output.device`, `export.video`,
/// `cam.track`): nothing reads a further "output" from them, so no slot is
/// reserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Op {
    pub node: NodeId,
    pub buffer: Option<usize>,
    pub inputs: Vec<usize>,
}

/// The compiled graph: a topologically-ordered op list and how many
/// intermediate buffer slots the plan needs at once (the high-water mark
/// after reuse, not one per node).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub ops: Vec<Op>,
    pub buffer_count: usize,
}

impl Graph {
    /// Compiles a topological order and a buffer assignment.
    ///
    /// **Does not itself check for cycles** — that is `lint`'s job, and
    /// running lint before compiling is the caller's responsibility (the
    /// editor always lints before it ever compiles). If a cycle slips
    /// through anyway, the nodes on it are simply never ready (their
    /// in-degree never reaches zero) and are appended at the end in id
    /// order — deterministic, not a panic, not an infinite loop.
    pub fn compile(&self) -> Plan {
        let mut indeg: HashMap<&str, usize> = HashMap::new();
        let mut outdeg: HashMap<&str, usize> = HashMap::new();
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        // Incoming edges per node, sorted for determinism, so `inputs` is a
        // pure function of the graph's content.
        let mut incoming: HashMap<&str, Vec<&crate::Edge>> = HashMap::new();

        for n in &self.nodes {
            indeg.entry(n.id.as_str()).or_insert(0);
            outdeg.entry(n.id.as_str()).or_insert(0);
        }
        for e in &self.edges {
            // Skip edges that reference a node no longer in the graph —
            // `lint` reports these; `compile` just must not panic on them.
            if self.node(&e.from.node).is_none() || self.node(&e.to.node).is_none() {
                continue;
            }
            // Order: every cable, a binding included — what drives a float is
            // resolved before what the float belongs to.
            *indeg.entry(e.to.node.as_str()).or_insert(0) += 1;
            adj.entry(e.from.node.as_str()).or_default().push(e.to.node.as_str());
            // Buffers: never a binding (`crate::bind`). A param edge carries
            // one float the runtime writes into the consumer's row or config;
            // it makes its producer no image source and its consumer no sink.
            if crate::params::is_param_edge(e) {
                continue;
            }
            *outdeg.entry(e.from.node.as_str()).or_insert(0) += 1;
            incoming.entry(e.to.node.as_str()).or_default().push(e);
        }
        for v in incoming.values_mut() {
            v.sort_by(|a, b| (a.to.port.as_str(), a.from.node.as_str())
                .cmp(&(b.to.port.as_str(), b.from.node.as_str())));
        }

        // Kahn's algorithm, always picking the smallest ready id so the
        // order — and therefore the whole plan — is deterministic for the
        // same graph.
        let mut ready: Vec<&str> =
            indeg.iter().filter(|(_, &d)| d == 0).map(|(&id, _)| id).collect();
        ready.sort_unstable();
        let mut queue: VecDeque<&str> = ready.into();
        let mut order: Vec<&str> = Vec::with_capacity(self.nodes.len());
        let mut remaining_indeg = indeg.clone();
        let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();

        while let Some(id) = queue.pop_front() {
            if !visited.insert(id) {
                continue;
            }
            order.push(id);
            let mut newly_ready = Vec::new();
            if let Some(next) = adj.get(id) {
                for &n in next {
                    if let Some(d) = remaining_indeg.get_mut(n) {
                        if *d > 0 {
                            *d -= 1;
                            if *d == 0 {
                                newly_ready.push(n);
                            }
                        }
                    }
                }
            }
            newly_ready.sort_unstable();
            for n in newly_ready {
                queue.push_back(n);
            }
        }
        // Anything left (a cycle lint should have caught) — append in id
        // order rather than dropping it, so a still-invalid graph still
        // compiles to *something* instead of silently losing nodes.
        let mut leftover: Vec<&str> =
            self.nodes.iter().map(|n| n.id.as_str()).filter(|id| !visited.contains(id)).collect();
        leftover.sort_unstable();
        order.extend(leftover);

        // Buffer assignment with refcount-based reuse.
        let mut buffer_of: HashMap<&str, usize> = HashMap::new();
        let mut remaining_consumers: HashMap<&str, usize> = outdeg.clone();
        let mut free: Vec<usize> = Vec::new();
        let mut next_buf = 0usize;
        let mut ops = Vec::with_capacity(order.len());

        for id in &order {
            let inputs: Vec<usize> = incoming
                .get(id)
                .into_iter()
                .flatten()
                .filter_map(|e| buffer_of.get(e.from.node.as_str()).copied())
                .collect();

            // This node has now consumed each of its predecessors once;
            // release any predecessor buffer whose last consumer just ran.
            if let Some(edges) = incoming.get(id) {
                for e in edges {
                    let src = e.from.node.as_str();
                    if let Some(cnt) = remaining_consumers.get_mut(src) {
                        if *cnt > 0 {
                            *cnt -= 1;
                        }
                        if *cnt == 0 {
                            if let Some(b) = buffer_of.remove(src) {
                                free.push(b);
                                free.sort_unstable();
                            }
                        }
                    }
                }
            }

            let produces = outdeg.get(id).copied().unwrap_or(0) > 0;
            let buffer = if produces {
                let b = free.pop().unwrap_or_else(|| {
                    let b = next_buf;
                    next_buf += 1;
                    b
                });
                buffer_of.insert(id, b);
                Some(b)
            } else {
                None
            };

            ops.push(Op { node: id.to_string(), buffer, inputs });
        }

        Plan { ops, buffer_count: next_buf }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Domain, Endpoint, Graph};

    #[test]
    fn a_shared_generator_appears_once_in_the_plan() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let m1 = g.add_node("mask", "mask", [1.0, 0.0]);
        let m2 = g.add_node("mask", "mask", [1.0, 1.0]);
        let m3 = g.add_node("mask", "mask", [1.0, 2.0]);
        for m in [&m1, &m2, &m3] {
            g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(m, "image"));
        }
        let plan = g.compile();
        let gen_ops = plan.ops.iter().filter(|op| op.node == gen).count();
        assert_eq!(gen_ops, 1, "a node feeding three sinks must appear once in the plan");
        assert_eq!(plan.ops.len(), 4);
    }

    #[test]
    fn the_generators_buffer_is_reused_after_its_last_consumer() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let m1 = g.add_node("mask", "mask", [1.0, 0.0]);
        let out = g.add_node("out", "output.device", [2.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&m1, "image"));
        g.add_edge(Endpoint::new(&m1, "image"), Endpoint::new(&out, "image"));
        let plan = g.compile();
        // Two producing nodes (gen, mask), never alive at the same time once
        // gen is consumed by mask before mask itself needs a slot -> reuse.
        assert_eq!(plan.buffer_count, 1, "the generator's buffer should be recycled for the mask");
    }

    #[test]
    fn the_plan_is_deterministic_for_the_same_graph() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let a = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let b = g.add_node("gen", "gen.turing", [0.0, 1.0]);
        let out = g.add_node("out", "output.device", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&a, "image"), Endpoint::new(&out, "image"));
        g.add_edge(Endpoint::new(&b, "image"), Endpoint::new(&out, "control"));
        let p1 = g.compile();
        let p2 = g.compile();
        assert_eq!(p1, p2);
    }

    #[test]
    fn compile_does_not_panic_on_a_dangling_edge() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let a = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        g.add_edge(Endpoint::new(&a, "image"), Endpoint::new("missing", "image"));
        let _plan = g.compile();
    }

    /// A binding orders the plan (the script before the generator it drives)
    /// but is no picture: the script gets no buffer and is nobody's input.
    #[test]
    fn a_param_edge_orders_the_plan_but_holds_no_buffer() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [1.0, 0.0]);
        let script = g.add_node("script", "script.py", [0.0, 1.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        g.add_edge(Endpoint::new(&script, "control.out0"), Endpoint::new(&gen, "param:scale"));
        let plan = g.compile();
        let pos = |id: &str| plan.ops.iter().position(|op| op.node == id).unwrap();
        assert!(pos(&script) < pos(&gen), "the driver resolves first");
        let op = |id: &str| plan.ops.iter().find(|op| op.node == id).unwrap().clone();
        assert_eq!(op(&script).buffer, None, "a binding is no image source");
        assert!(op(&gen).inputs.is_empty(), "nor an image input");
        assert_eq!(plan.buffer_count, 1, "only the generator's picture holds a slot");
    }

    #[test]
    fn a_lone_node_with_no_edges_still_gets_an_op() {
        let mut g = Graph::new("g", "t", Domain::Live);
        g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let plan = g.compile();
        assert_eq!(plan.ops.len(), 1);
        assert_eq!(plan.ops[0].buffer, None, "a node nothing consumes needs no buffer slot");
    }
}
