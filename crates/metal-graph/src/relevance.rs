//! Relevance: which catalog kinds to offer first for an anchor node.
//!
//! One scorer for every "pick a kind" menu: the properties panel's effects
//! and source dropdowns, and every `+ … ▾` menu of the canvas bar (anchor =
//! the selected node). `GET /catalog?graph=&anchor=` answers it; nothing
//! else re-derives an order.
//!
//! The score of a kind `k` for an anchor node `a`:
//!
//! - `affinity(kind(a), k)`: a small static table keyed by the anchor's
//!   kind ([`AFFINITY`]). Footage-like sources want colour correction
//!   first; a 3D scene wants motion; a mask wants a generator for its stroke.
//!   Every 3D generator (a model with a camera track or surfaces, see
//!   [`scene_kinds`]) reads the 3D rows, as `gen.escena3d` does.
//! - `port_affinity(port, k)` when a material port is named ([`PORT_AFFINITY`]):
//!   a skin wants organic textures, water wants flowing ones. The source
//!   dropdown of a surface part asks with its port.
//! - `0.6 · ln(1 + usage[k])`: how often the kind is saved across the
//!   operator's graphs (counted by the caller, so this crate never reads the
//!   disk).
//! - `+0.4` when `k` is already in the open graph.
//! - `-2.0` when `k` is an `fx.*` already downstream of the anchor: no
//!   duplicates on top of the chain.
//!
//! Ties keep catalog order (the sort is stable).

use std::collections::{HashMap, HashSet};

use crate::{Graph, NodeKind};

/// `(anchor kind, candidate kind, weight)`. Either side may end in `*` to
/// match a prefix (`reel*` is `reel` and `reel.clip`, `gen.*` is every
/// generator). Anchor kinds are the real catalog names: `reel.clip`,
/// `still`, `capture`, `cam.track`, `gen.escena3d`, `mask`.
///
/// `gen.*` footage: a footage model is promoted to `fx.estela` and is not a
/// `gen.*` kind, so there is no footage generator row here.
pub const AFFINITY: &[(&str, &str, f32)] = &[
    // Footage-like sources: colour correction first.
    ("reel*", "fx.grade", 3.0),
    ("reel*", "fx.dehaze", 1.5),
    ("reel*", "fx.clarity", 1.0),
    ("reel*", "fx.texture", 0.8),
    ("reel*", "fx.denoise", 0.6),
    ("still", "fx.grade", 3.0),
    ("still", "fx.dehaze", 1.5),
    ("still", "fx.clarity", 1.0),
    ("still", "fx.texture", 0.8),
    ("still", "fx.denoise", 0.6),
    ("capture", "fx.grade", 3.0),
    ("capture", "fx.dehaze", 1.5),
    ("capture", "fx.clarity", 1.0),
    ("capture", "fx.texture", 0.8),
    ("capture", "fx.denoise", 0.6),
    // 3D scenes and the camera track: motion and shake.
    ("cam.track", "fx.shake", 2.0),
    ("cam.track", "fx.zoom", 1.0),
    ("cam.track", "fx.move", 1.2),
    ("cam.track", "fx.speed", 0.8),
    ("cam.track", "fx.warp", 0.8),
    ("gen.scene3d", "fx.shake", 2.0),
    ("gen.scene3d", "fx.zoom", 1.0),
    ("gen.scene3d", "fx.move", 1.2),
    ("gen.scene3d", "fx.speed", 0.8),
    ("gen.scene3d", "fx.warp", 0.8),
    // A mask's stroke is fed by a generator.
    ("mask", "gen.*", 1.5),
];

/// `(material port, candidate kind, weight)` for a surface part's source
/// dropdown (`GET /catalog?port=`). Exact port names; kinds are real catalog
/// names (the test below checks them against `data/schema.json`).
pub const PORT_AFFINITY: &[(&str, &str, f32)] = &[
    // Skin: organic, procedural surfaces.
    ("tex.skin", "gen.turing", 2.0),
    ("tex.skin", "gen.membrane", 2.0),
    ("tex.skin", "gen.fractals", 1.6),
    ("tex.skin", "gen.dla", 1.6),
    ("tex.skin", "gen.solvent", 1.4),
    ("tex.skin", "gen.stained-glass", 1.2),
    // Water: flowing, drifting pictures.
    ("tex.water", "gen.smoke", 2.0),
    ("tex.water", "gen.aurora", 2.0),
    ("tex.water", "gen.moire", 1.4),
    ("tex.water", "gen.river", 1.6),
];

fn pattern_matches(pattern: &str, kind: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => kind.starts_with(prefix),
        None => pattern == kind,
    }
}

/// The affinity weight of `candidate` for an anchor of kind `anchor_kind`:
/// the largest matching row of [`AFFINITY`], or `0.0` when none matches.
pub fn affinity(anchor_kind: &str, candidate: &str) -> f32 {
    AFFINITY
        .iter()
        .filter(|(a, c, _)| pattern_matches(a, anchor_kind) && pattern_matches(c, candidate))
        .map(|(_, _, w)| *w)
        .fold(0.0, f32::max)
}

/// The affinity weight of `candidate` for a material `port`: the largest
/// matching row of [`PORT_AFFINITY`], `0.0` when none matches.
pub fn port_affinity(port: &str, candidate: &str) -> f32 {
    PORT_AFFINITY
        .iter()
        .filter(|(p, c, _)| *p == port && pattern_matches(c, candidate))
        .map(|(_, _, w)| *w)
        .fold(0.0, f32::max)
}

/// The 3D generators of the manifest: every model with a camera track or
/// declared surfaces, as catalog kinds (`gen.<id>`). They take the motion
/// rows of [`AFFINITY`] as `gen.escena3d` does.
pub fn scene_kinds(manifest: &metal_schema::Manifest) -> HashSet<String> {
    manifest
        .models
        .iter()
        .filter(|m| m.camera_track || !m.surfaces.is_empty())
        .map(|m| format!("gen.{}", m.id))
        .collect()
}

/// Every `kind` reachable downstream of `anchor` along any edge, the anchor
/// itself excluded.
fn downstream_kinds(graph: &Graph, anchor: &str) -> HashSet<String> {
    let mut seen_nodes: HashSet<&str> = HashSet::new();
    let mut stack = vec![anchor];
    let mut kinds = HashSet::new();
    while let Some(current) = stack.pop() {
        for e in graph.edges.iter().filter(|e| e.from.node == current) {
            let to = e.to.node.as_str();
            if seen_nodes.insert(to) {
                if let Some(n) = graph.node(to) {
                    kinds.insert(n.kind.clone());
                }
                stack.push(to);
            }
        }
    }
    kinds
}

/// Ranks every kind in `catalog` for the optional anchor. Sorted by score,
/// descending; ties keep catalog order. `anchor` is a node id of `graph`;
/// without a graph or an anchor the score is the usage and presence terms
/// only. Without `scenes` (the 3D generators, see [`scene_kinds`]) only
/// `gen.escena3d` reads the motion rows.
pub fn rank(
    catalog: &[NodeKind],
    graph: Option<&Graph>,
    anchor: Option<&str>,
    usage: &HashMap<String, u32>,
) -> Vec<(String, f32)> {
    rank_for_port(catalog, graph, anchor, None, usage, &HashSet::new())
}

/// [`rank`] with the two extra terms the surface widgets need: the material
/// `port` a source is wanted for ([`port_affinity`]), and the manifest's 3D
/// generators `scenes` ([`scene_kinds`]).
pub fn rank_for_port(
    catalog: &[NodeKind],
    graph: Option<&Graph>,
    anchor: Option<&str>,
    port: Option<&str>,
    usage: &HashMap<String, u32>,
    scenes: &HashSet<String>,
) -> Vec<(String, f32)> {
    let anchor_kind = graph
        .zip(anchor)
        .and_then(|(g, a)| g.node(a))
        .map(|n| n.kind.clone());
    // A 3D generator answers to the scene rows, as `gen.escena3d` does.
    let anchor_key = anchor_kind.as_deref().map(|ak| if scenes.contains(ak) { "gen.scene3d" } else { ak });
    let downstream = match (graph, anchor) {
        (Some(g), Some(a)) => downstream_kinds(g, a),
        _ => HashSet::new(),
    };
    let mut scored: Vec<(String, f32)> = catalog
        .iter()
        .map(|k| {
            let mut score = 0.0f32;
            if let Some(ak) = anchor_key {
                score += affinity(ak, &k.kind);
            }
            if let Some(port) = port {
                score += port_affinity(port, &k.kind);
            }
            let uses = usage.get(&k.kind).copied().unwrap_or(0);
            score += 0.6 * (1.0 + uses as f32).ln();
            if graph.is_some_and(|g| g.nodes.iter().any(|n| n.kind == k.kind)) {
                score += 0.4;
            }
            if k.kind.starts_with("fx.") && downstream.contains(&k.kind) {
                score -= 2.0;
            }
            (k.kind.clone(), score)
        })
        .collect();
    // `sort_by` is stable: equal scores keep catalog order.
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Domain, Endpoint, NodeFamily, Port, PortType};

    fn kind(name: &str) -> NodeKind {
        let family = match name.split('.').next().unwrap_or("") {
            "fx" => NodeFamily::Effect,
            "gen" => NodeFamily::Generator,
            _ => NodeFamily::Mask,
        };
        NodeKind {
            kind: name.to_string(),
            family,
            label: name.to_string(),
            inputs: vec![Port::new("image", PortType::Image)],
            outputs: vec![Port::new("image", PortType::Image)],
            menu: true,
            domains: &[],
            params: Vec::new(),
        }
    }

    fn catalog() -> Vec<NodeKind> {
        [
            "fx.shake", "fx.grade", "fx.zoom", "fx.dehaze", "gen.turing", "gen.scene3d", "reel.clip",
            "mask",
        ]
        .iter()
        .map(|k| kind(k))
        .collect()
    }

    fn position(ranked: &[(String, f32)], name: &str) -> usize {
        ranked.iter().position(|(k, _)| k == name).expect("kind is ranked")
    }

    #[test]
    fn an_effect_already_in_the_anchors_chain_sinks() {
        let mut g = Graph::new("g", "g", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let fx = g.add_node("fx", "fx.shake", [200.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&fx, "image"));
        let cat = catalog();
        let usage = HashMap::new();

        let ranked = rank(&cat, Some(&g), Some(&gen), &usage);
        let shake = ranked.iter().find(|(k, _)| k == "fx.shake").unwrap().1;
        let zoom = ranked.iter().find(|(k, _)| k == "fx.zoom").unwrap().1;
        assert!(shake < zoom, "fx.shake is downstream of the anchor, so it sinks below fx.zoom");
        // Without the anchor the chain is not in view and shake is not sunk.
        let free = rank(&cat, Some(&g), None, &usage);
        assert!(free.iter().find(|(k, _)| k == "fx.shake").unwrap().1 > shake);
    }

    #[test]
    fn a_footage_anchor_ranks_colour_correction_first() {
        let mut g = Graph::new("g", "g", Domain::Live);
        let reel = g.add_node("reel", "reel.clip", [0.0, 0.0]);
        let ranked = rank(&catalog(), Some(&g), Some(&reel), &HashMap::new());
        let effects: Vec<&str> = ranked
            .iter()
            .map(|(k, _)| k.as_str())
            .filter(|k| k.starts_with("fx."))
            .collect();
        assert_eq!(effects.first().copied(), Some("fx.grade"));
        assert!(position(&ranked, "fx.grade") < position(&ranked, "fx.dehaze"));
        assert!(position(&ranked, "fx.dehaze") < position(&ranked, "fx.shake"));
    }

    #[test]
    fn a_3d_scene_ranks_motion_before_colour() {
        let mut g = Graph::new("g", "g", Domain::Live);
        let cam = g.add_node("gen", "gen.scene3d", [0.0, 0.0]);
        let ranked = rank(&catalog(), Some(&g), Some(&cam), &HashMap::new());
        assert!(position(&ranked, "fx.shake") < position(&ranked, "fx.grade"));
    }

    #[test]
    fn usage_breaks_ties_and_catalog_order_holds_otherwise() {
        let cat = catalog();
        // No anchor, no graph: nothing scores, so catalog order is the answer.
        let plain = rank(&cat, None, None, &HashMap::new());
        let names: Vec<&str> = plain.iter().map(|(k, _)| k.as_str()).collect();
        let catalog_names: Vec<&str> = cat.iter().map(|k| k.kind.as_str()).collect();
        assert_eq!(names, catalog_names, "a tie keeps catalog order");

        // Usage lifts a kind above its neighbours; more usage lifts it more.
        let mut usage = HashMap::new();
        usage.insert("gen.turing".to_string(), 1);
        usage.insert("reel.clip".to_string(), 9);
        let used = rank(&cat, None, None, &usage);
        assert_eq!(used[0].0, "reel.clip");
        assert!(position(&used, "gen.turing") < position(&used, "fx.shake"));
        assert!(used[0].1 > used[1].1);
    }

    fn models() -> Vec<NodeKind> {
        ["gen.rings", "gen.turing", "gen.membrane", "gen.smoke", "gen.aurora", "gen.matrix", "gen.monster"]
            .iter()
            .map(|k| kind(k))
            .collect()
    }

    /// A skin wants organic textures first, water wants flowing ones; the
    /// same catalog answers differently per port, and without a port the
    /// order is catalog order.
    #[test]
    fn a_surface_port_favours_the_textures_that_suit_it() {
        let cat = models();
        let usage = HashMap::new();
        let skin = rank_for_port(&cat, None, None, Some("tex.skin"), &usage, &HashSet::new());
        assert_eq!(skin[0].0, "gen.turing");
        assert!(position(&skin, "gen.membrane") < position(&skin, "gen.rings"));
        assert!(position(&skin, "gen.turing") < position(&skin, "gen.smoke"));

        let water = rank_for_port(&cat, None, None, Some("tex.water"), &usage, &HashSet::new());
        assert_eq!(water[0].0, "gen.smoke");
        assert!(position(&water, "gen.aurora") < position(&water, "gen.turing"));

        let plain = rank_for_port(&cat, None, None, None, &usage, &HashSet::new());
        let names: Vec<&str> = plain.iter().map(|(k, _)| k.as_str()).collect();
        let catalog_names: Vec<&str> = cat.iter().map(|k| k.kind.as_str()).collect();
        assert_eq!(names, catalog_names);
    }

    /// Every 3D generator of the manifest reads the motion rows, not only
    /// `gen.escena3d`; a flat model does not.
    #[test]
    fn every_3d_generator_ranks_motion_like_escena3d() {
        let mut g = Graph::new("g", "g", Domain::Live);
        let matrix = g.add_node("gen", "gen.matrix", [0.0, 0.0]);
        let cat = catalog();
        let usage = HashMap::new();
        let scenes: HashSet<String> = ["gen.matrix".to_string()].into_iter().collect();

        let ranked = rank_for_port(&cat, Some(&g), Some(&matrix), None, &usage, &scenes);
        assert!(position(&ranked, "fx.shake") < position(&ranked, "fx.grade"));

        // Without the scene set the anchor is a plain generator: no motion row.
        let flat = rank_for_port(&cat, Some(&g), Some(&matrix), None, &usage, &HashSet::new());
        let shake = |r: &[(String, f32)]| r.iter().find(|(k, _)| k == "fx.shake").unwrap().1;
        assert!(shake(&ranked) > shake(&flat));
    }

    /// The 3D set is the manifest's: a camera track or surfaces make a model
    /// 3D; a plain model is not.
    #[test]
    fn scene_kinds_are_the_camera_track_and_surfaced_models() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json"),
        )
        .expect("data/schema.json readable");
        let manifest = metal_schema::Manifest::parse(&text).expect("schema.json parses");
        let scenes = scene_kinds(&manifest);
        assert!(scenes.contains("gen.scene3d"));
        assert!(scenes.contains("gen.monster"), "monstruo declares surfaces");
        assert!(!scenes.contains("gen.rings"));
        let expected = manifest.models.iter().filter(|m| m.camera_track || !m.surfaces.is_empty()).count();
        assert_eq!(scenes.len(), expected);
    }

    /// The port table names real catalog kinds, like [`AFFINITY`].
    #[test]
    fn the_port_table_names_real_catalog_kinds() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json"),
        )
        .expect("data/schema.json readable");
        let manifest = metal_schema::Manifest::parse(&text).expect("schema.json parses");
        let real: HashSet<String> =
            NodeKind::catalog(&manifest).into_iter().map(|k| k.kind).collect();
        for (_, candidate, _) in PORT_AFFINITY {
            assert!(real.contains(*candidate), "{candidate} is not a catalog kind");
        }
    }

    #[test]
    fn the_table_names_real_catalog_kinds() {
        // Every exact name in the affinity table is a kind the repo's own
        // catalog builds (a footage source has no gen row, so `*` rows are
        // patterns and are skipped).
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json"),
        )
        .expect("data/schema.json readable");
        let manifest = metal_schema::Manifest::parse(&text).expect("schema.json parses");
        let real: HashSet<String> =
            NodeKind::catalog(&manifest).into_iter().map(|k| k.kind).collect();
        for (anchor, candidate, _) in AFFINITY {
            for name in [anchor, candidate] {
                if !name.ends_with('*') {
                    assert!(real.contains(*name), "{name} is not a catalog kind");
                }
            }
        }
    }
}
