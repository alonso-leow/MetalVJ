//! The `mat.*` family: what a 3D model's surface is made of, as a node.
//!
//! `mat.<kind>` is a **new node family beside `fx.*`**, not an effect (the
//! materials brief, Decision 1). It has exactly the fx chain's shape — one
//! `image` input, one `image` output, one `control` input — so a graph wires it
//! the same way and the resolver reads the chain back off the graph (rule 4),
//! never off a second state:
//!
//! - the node **kind** names the material (`mat.wood` -> `wood`), the enum the
//!   row's held `mat_kind` float selects;
//! - its **config** carries `part` (`"*"`, every surface, or a model's own part
//!   id) and the per-part numeric overrides, each with a declared default
//!   ([`CONFIG_DEFAULTS`], [`default_of`]);
//! - the picture passes through unchanged: a material acts on the *3D model's*
//!   surfaces (the `gen.*` upstream), not on the frame. `chain` walks the
//!   `image` edges downstream of a generator, skipping the `fx.*` nodes it
//!   shares the chain with, and collects the `mat.*` nodes in graph order.
//!
//! **One slot per row, many nodes per graph.** The row's `material` group is
//! the slot automix drives; a `mat.*` node overlays its own fixed overrides on
//! it ([`floats_for`]). Several nodes on different parts are the per-part
//! overrides the brief keeps in node config, fixed and not driven.
//!
//! **The kinds live here, once**: [`KINDS`] is the catalog's source, the order
//! is the `mat_kind` enum the row float indexes, and a test ties the two
//! together so neither can drift (rule 7).

use std::collections::HashSet;

use crate::{Graph, Node, NodeFamily, NodeId, NodeKind, Port, PortType};

/// The node-kind prefix, spelled once.
pub const PREFIX: &str = "mat.";

/// Every `mat.*` kind, in `mat_kind` enum order (the row float's value is the
/// index here). Derived into the catalog by [`kinds`]; the count is never
/// written down in a test (rule 7).
pub const KINDS: &[&str] = &[
    "matte",
    "concrete",
    "wood",
    "leather",
    "metal",
    "glass",
    "matrix",
];

/// The `part` value meaning "every surface of the model".
pub const PART_ANY: &str = "*";

/// The numeric config keys a `mat.*` node may hold, with their defaults. Each
/// names a float of the row's `material` group by dropping the `mat_` prefix
/// (`mix` -> `mat_mix`), so the mapping is derived, not a second index list.
///
/// **`mix` defaults to 1**: a material *node* means the material is on, so a
/// fresh `mat.wood` on a row whose group is all-zero still shows. Every other
/// key defaults to 0, which the shader reads as "the kind decides".
pub const CONFIG_DEFAULTS: &[(&str, f32)] = &[
    ("mix", 1.0),
    ("scale", 0.0),
    ("rough", 0.0),
    ("metal", 0.0),
    ("bump", 0.0),
    ("env", 0.0),
    ("tint", 0.0),
    ("tint_hue", 0.0),
    ("glitch", 0.0),
    ("glitch_rate", 0.0),
    ("glitch_seed", 0.0),
];

/// The `mat.<kind>` string's bare kind name (`mat.wood` -> `wood`), or `None`
/// for anything that is not one of [`KINDS`].
pub fn kind_of(kind: &str) -> Option<&'static str> {
    let name = kind.strip_prefix(PREFIX)?;
    KINDS.iter().copied().find(|k| *k == name)
}

/// Whether `kind` is a `mat.*` node of a declared kind — the one test every
/// reader of the family shares.
pub fn is_material_kind(kind: &str) -> bool {
    kind_of(kind).is_some()
}

/// The `mat_kind` float value of a `mat.<kind>` node — its position in
/// [`KINDS`], the same number the row's held float carries.
pub fn kind_index(kind: &str) -> Option<usize> {
    let name = kind_of(kind)?;
    KINDS.iter().position(|k| *k == name)
}

/// The kind name at a `mat_kind` index, or `None` past the end.
pub fn kind_name_at(index: usize) -> Option<&'static str> {
    KINDS.get(index).copied()
}

/// The declared default of a numeric config key, or `None` when the kind does
/// not carry it.
pub fn default_of(key: &str) -> Option<f32> {
    CONFIG_DEFAULTS.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// The `MATERIAL_FLOATS` position a config key names (`mix` -> `mat_mix`), or
/// `None` for a key the row group does not carry.
fn float_index_of(key: &str) -> Option<usize> {
    let name = format!("mat_{key}");
    metal_schema::MATERIAL_FLOATS.iter().position(|s| s.name == name.as_str())
}

/// The `(min, max)` of a config key's row float, for a slider — the row's
/// own declared range, never a second one.
pub fn range_of(key: &str) -> Option<(f32, f32)> {
    let spec = &metal_schema::MATERIAL_FLOATS[float_index_of(key)?];
    Some((spec.min, spec.max))
}

/// The `part` a `mat.*` node overrides: its config's `part`, or [`PART_ANY`]
/// when absent or empty.
pub fn part_of(node: &Node) -> String {
    node.config
        .get("part")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(PART_ANY)
        .to_string()
}

/// The value `node` holds for `key`, when its config names it — the override,
/// distinct from the declared default a missing key falls back to.
pub fn override_of(node: &Node, key: &str) -> Option<f32> {
    node.config
        .get(key)
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .filter(|v| v.is_finite())
}

/// The `material` group `node` resolves to, overlaid on the row's own group
/// `base`: the node's kind at the `mat_kind` position, then each override its
/// config names. A missing row value falls back to the declared default, and
/// an all-zero `mix` is lifted to 1 so a material node is never silently off.
pub fn floats_for(node: &Node, base: &[f32]) -> Vec<f32> {
    let mut out: Vec<f32> = metal_schema::MATERIAL_FLOATS.iter().map(|s| s.default).collect();
    for (i, v) in base.iter().take(metal_schema::N_MATERIAL).enumerate() {
        out[i] = *v;
    }
    if let Some(idx) = kind_index(&node.kind) {
        out[0] = idx as f32;
    }
    for (key, _) in CONFIG_DEFAULTS {
        if let (Some(v), Some(i)) = (override_of(node, key), float_index_of(key)) {
            out[i] = v;
        }
    }
    if out[1] == 0.0 {
        out[1] = default_of("mix").unwrap_or(1.0);
    }
    out
}

/// One `mat.*` node's resolved material: which part it names, its kind and the
/// `material` group it asks for — see [`resolve`].
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMaterial {
    pub node: NodeId,
    pub part: String,
    pub kind: &'static str,
    pub floats: Vec<f32>,
}

/// The `mat.*` node ids chained downstream of `gen_node`'s `image` output, in
/// graph order. Walks one `image` edge at a time, collecting `mat.*` nodes and
/// stepping through the `fx.*` nodes that share the chain (they act on the
/// frame, the material on the surface); stops the moment the walk stops being
/// a simple chain — a fork, a node of neither family, or a cycle.
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
        if node.kind.starts_with(PREFIX) {
            out.push(next_id.clone());
            current = next_id;
        } else if node.kind.starts_with("fx.") {
            current = next_id;
        } else {
            break;
        }
    }
    out
}

/// The material a generator's `mat.*` chain resolves to, read back off the
/// graph (rule 4), with the row's own `material` group `base` as the starting
/// point — see [`floats_for`].
pub fn resolve(graph: &Graph, gen_node: &str, base: &[f32]) -> Vec<ResolvedMaterial> {
    chain(graph, gen_node)
        .into_iter()
        .filter_map(|id| {
            let node = graph.node(&id)?;
            let kind = kind_of(&node.kind)?;
            Some(ResolvedMaterial {
                node: id,
                part: part_of(node),
                kind,
                floats: floats_for(node, base),
            })
        })
        .collect()
}

/// The `mat.*` kinds the catalog appends — the fx chain's shape exactly: one
/// `image` in, one `image` out, one `control` in.
pub(crate) fn kinds() -> Vec<NodeKind> {
    KINDS
        .iter()
        .map(|name| {
            NodeKind::new(
                &format!("{PREFIX}{name}"),
                NodeFamily::Material,
                name,
                vec![
                    Port::new("image", PortType::Image),
                    Port::new("control", PortType::Control),
                ],
                vec![Port::new("image", PortType::Image)],
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Endpoint, Graph};

    fn node(kind: &str, config: &str) -> Node {
        let mut n = Node::new("m", kind, [0.0, 0.0]);
        n.config = crate::Config::from_json_str(config).unwrap();
        n
    }

    fn chain_graph(after: &[&str]) -> (Graph, String) {
        let mut g = Graph::new("g", "t", crate::Domain::Live);
        let gen = g.add_node("gen", "gen.monster", [0.0, 0.0]);
        let mut prev = gen.clone();
        for (i, kind) in after.iter().enumerate() {
            let id = g.add_node("n", kind, [i as f32 + 1.0, 0.0]);
            g.add_edge(Endpoint::new(&prev, "image"), Endpoint::new(&id, "image"));
            prev = id;
        }
        (g, gen)
    }

    /// Every declared kind is in the catalog once, and the catalog has no
    /// `mat.*` kind the table does not name — counted from [`KINDS`] (rule 7).
    #[test]
    fn every_material_kind_is_in_the_catalog() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json");
        let text = metal_vfs::read_to_string(path).expect("data/schema.json readable");
        let cat = NodeKind::catalog(&metal_schema::Manifest::parse(&text).expect("schema parses"));
        for name in KINDS {
            let kind = format!("{PREFIX}{name}");
            let found: Vec<&NodeKind> = cat.iter().filter(|k| k.kind == kind).collect();
            assert_eq!(found.len(), 1, "{kind} exactly once in the catalog");
            assert_eq!(found[0].family, NodeFamily::Material);
        }
        let mat_kinds: Vec<&str> =
            cat.iter().filter(|k| k.kind.starts_with(PREFIX)).map(|k| k.kind.as_str()).collect();
        assert_eq!(mat_kinds.len(), KINDS.len(), "no undeclared mat.* kind in the catalog");
    }

    /// The kinds are in `mat_kind` enum order: the index in [`KINDS`] is the
    /// value the row float carries, and [`kind_name_at`] is its inverse.
    #[test]
    fn the_kind_order_is_the_mat_kind_enum() {
        assert_eq!(KINDS.len(), metal_schema::MATERIAL_FLOATS[0].max as usize + 1);
        for (i, name) in KINDS.iter().enumerate() {
            assert_eq!(kind_index(&format!("{PREFIX}{name}")), Some(i));
            assert_eq!(kind_name_at(i), Some(*name));
        }
        assert!(kind_index("mat.nope").is_none());
        assert!(kind_name_at(KINDS.len()).is_none());
    }

    /// Every config key has a declared default, and each one names a float the
    /// material group carries (`mix` -> `mat_mix`).
    #[test]
    fn every_config_key_has_a_default_that_names_a_row_float() {
        for (key, default) in CONFIG_DEFAULTS {
            assert_eq!(default_of(key), Some(*default), "{key} has a default");
            let i = float_index_of(key).unwrap_or_else(|| panic!("{key} names no material float"));
            assert!(
                metal_schema::MATERIAL_FLOATS[i].name == format!("mat_{key}"),
                "{key} maps to {}",
                metal_schema::MATERIAL_FLOATS[i].name
            );
            assert!(*default >= metal_schema::MATERIAL_FLOATS[i].min);
            assert!(*default <= metal_schema::MATERIAL_FLOATS[i].max);
        }
        assert_eq!(default_of("nope"), None);
    }

    /// A fresh node turns the material on: its kind at the `mat_kind` position
    /// and `mix` at 1 even over an all-zero row.
    #[test]
    fn a_fresh_node_resolves_to_its_kind_and_turns_the_mix_on() {
        let n = node("mat.wood", "{}");
        let f = floats_for(&n, &[0.0; metal_schema::N_MATERIAL]);
        assert_eq!(f[0], kind_index("mat.wood").unwrap() as f32);
        assert_eq!(f[1], 1.0, "a material node is on unless told otherwise");
        assert_eq!(part_of(&n), "*");
    }

    /// A config override beats the row's own value; an absent key leaves the
    /// row's value in place.
    #[test]
    fn config_overrides_the_rows_group_and_absent_keys_keep_it() {
        let mut base = vec![0.0f32; metal_schema::N_MATERIAL];
        base[1] = 0.4; // mat_mix
        base[3] = 0.7; // mat_rough
        let n = node("mat.metal", r#"{"part":"skin","rough":0.1}"#);
        let f = floats_for(&n, &base);
        assert_eq!(part_of(&n), "skin");
        assert_eq!(f[3], 0.1, "the override wins");
        assert_eq!(f[1], 0.4, "an absent key keeps the row's value");
    }

    /// The chain walks downstream through an interleaved `fx.*` and never past
    /// a node of another family.
    #[test]
    fn the_chain_skips_fx_and_stops_at_another_family() {
        let (g, gen) = chain_graph(&["mat.wood", "fx.vignette", "mat.leather"]);
        let ids = chain(&g, &gen);
        assert_eq!(ids.len(), 2, "both materials, the fx passed through");
        let names: Vec<&str> = ids.iter().map(|id| kind_of(&g.node(id).unwrap().kind).unwrap()).collect();
        assert_eq!(names, vec!["wood", "leather"]);
    }

    /// `resolve` reads the whole material chain back off the graph.
    #[test]
    fn resolve_reads_the_chain_back_off_the_graph() {
        let (g, gen) = chain_graph(&["mat.concrete", "mat.glass"]);
        let mats = resolve(&g, &gen, &[0.0; metal_schema::N_MATERIAL]);
        assert_eq!(mats.len(), 2);
        assert_eq!(mats[0].kind, "concrete");
        assert_eq!(mats[1].kind, "glass");
        assert!(mats.iter().all(|m| m.part == PART_ANY));
    }
}
