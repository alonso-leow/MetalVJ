//! Converts the real `data/collections.json` shape into [`Graph`]s.
//!
//! `the design notes` A7: each `Layer` (`metal-perform/src/layers.rs`)
//! already carries everything a generator node needs — `model`, `feed`,
//! `picked`, `library`, `base`, `sector`, `mask`, `z`, `blend`, `opacity`,
//! `active`, `solo`, `name` — it is a node written as a table row, not a
//! table row that needs interpreting. This module is the field-for-field
//! mapping the plan calls for.
//!
//! **This crate does not depend on `metal-dash`.** Doing so would pull `wgpu`
//! transitively through `metal-render` and violate the one rule this crate
//! exists to keep — see the crate doc comment. So this module does not reuse
//! `metal-dash::layers::{Layer, Collection}`; it declares its own minimal
//! private structs that mirror the JSON shape those types serialize to, and
//! only the fields this migration reads. If `Layer` grows a field, this
//! module does not need to know until the day it needs the field.
//!
//! **This step does not wire into the render pipeline.** It only has to
//! produce a structurally correct `Graph` from real `collections.json` data
//! — see A7's acceptance gate.

use serde::Deserialize;

use crate::{Domain, Endpoint, Graph, NodeId};

#[derive(Debug, Deserialize)]
struct RawCollection {
    id: String,
    name: String,
    #[serde(default)]
    layers: Vec<RawLayer>,
}

#[derive(Debug, Deserialize)]
struct RawSector {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

#[derive(Debug, Deserialize)]
struct RawLayer {
    id: String,
    name: String,
    model: usize,
    opacity: f32,
    z: i32,
    blend: String,
    #[serde(default)]
    sector: Option<RawSector>,
    active: bool,
    #[serde(default)]
    solo: bool,
    #[serde(default)]
    base: Option<Vec<f32>>,
    #[serde(default)]
    library: Option<String>,
    #[serde(default)]
    feed: String,
    #[serde(default)]
    picked: Vec<usize>,
    #[serde(default)]
    mask: Option<String>,
}

/// Errors turning `collections.json` text into graphs.
#[derive(Debug)]
pub struct MigrateError(pub String);

impl std::fmt::Display for MigrateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "metal-graph migrate: {}", self.0)
    }
}
impl std::error::Error for MigrateError {}

/// Converts every collection in `collections_json` (the real
/// `data/collections.json` shape — a JSON array of collections, each with
/// `id`, `name`, `layers[]`) into one [`Graph`] per collection.
///
/// `manifest` resolves `Layer::model` (an index into the append-only model
/// registry — see `metal-render::registry`'s "order is append-only") to the
/// `gen.<id>` kind this crate's catalog uses.
///
/// **What this does not attempt**: `data/outputs.json` in this repo is an
/// empty array (no devices configured yet), so there is no real device to
/// address. Each collection gets one synthesized `output.device` node rather
/// than one per real device — the correct per-device fan-out is Set B's
/// job, once the compositor actually consumes a plan. Every layer's `image`
/// edge here targets that one synthesized device.
pub fn from_layers(
    collections_json: &str,
    manifest: &metal_schema::Manifest,
) -> Result<Vec<Graph>, MigrateError> {
    let raw: Vec<RawCollection> =
        serde_json::from_str(collections_json).map_err(|e| MigrateError(e.to_string()))?;

    let model_ids: Vec<&str> = manifest.models.iter().map(|m| m.id.as_str()).collect();

    let mut graphs = Vec::with_capacity(raw.len());
    for c in raw {
        graphs.push(collection_to_graph(&c, &model_ids));
    }
    Ok(graphs)
}

fn collection_to_graph(c: &RawCollection, model_ids: &[&str]) -> Graph {
    let mut g = Graph::new(c.id.clone(), c.name.clone(), Domain::Live);
    // A collection predates audio nodes: its layers reacted to the one
    // process input. Version 0 so the first load wires them ([`upgrade`]).
    g.version = 0;

    let device: NodeId = g.add_node("out", "output.device", [800.0, 0.0]);

    for (i, layer) in c.layers.iter().enumerate() {
        let model_id = model_ids.get(layer.model).copied().unwrap_or("unknown");
        let gen_kind = format!("gen.{model_id}");
        let gen_id = g.add_node("gen", &gen_kind, [0.0, i as f32 * (crate::layout::NODE_H + 20.0)]);

        {
            let node = g.node_mut(&gen_id).expect("just added");
            node.label = layer.name.clone();
            node.bypass = !layer.active;
            node.solo = layer.solo;
            node.config = layer_config(layer);
        }

        // §3.2: a fixed effect in `layer.base`'s trailing floats materializes
        // as an `fx.*` node chained after the generator, in declared order —
        // one node per effect whose value differs from `Post::default()`.
        let mut chain_tail = gen_id.clone();
        for (effect_id, value) in effects_differing_from_default(layer.base.as_deref()) {
            let fx_kind = format!("fx.{effect_id}");
            let fx_id = g.add_node(
                "fx",
                &fx_kind,
                [200.0, i as f32 * (crate::layout::NODE_H + 20.0)],
            );
            {
                let node = g.node_mut(&fx_id).expect("just added");
                node.config = crate::Config::from_value(
                    serde_json::json!({ "value": value, "mode": "fixed" }),
                );
            }
            g.add_edge(Endpoint::new(&chain_tail, "image"), Endpoint::new(&fx_id, "image"));
            chain_tail = fx_id;
        }

        g.add_edge(Endpoint::new(&chain_tail, "image"), Endpoint::new(&device, "image"));
    }

    g
}

/// The layer fields A7 asks to preserve that have no dedicated typed slot on
/// a generator node yet — carried as config, exactly what [`crate::Node::config`]
/// is for.
fn layer_config(layer: &RawLayer) -> crate::Config {
    crate::Config::from_value(serde_json::json!({
        "source_id": layer.id,
        "z": layer.z,
        "blend": layer.blend,
        "sector": layer.sector.as_ref().map(|s| serde_json::json!({
            "x": s.x, "y": s.y, "w": s.w, "h": s.h,
        })),
        "opacity": layer.opacity,
        "feed": if layer.feed.is_empty() { "model" } else { layer.feed.as_str() },
        "picked": layer.picked,
        "mask": layer.mask,
        "library": layer.library,
    }))
}

/// Reads the trailing `Post` floats out of `base`, if present and wide
/// enough, and returns every field that differs from
/// `metal_schema::Post::default()` — in the fixed order A2's `COMMON_EFFECT_IDS`
/// declares, which is the order `fx.*` nodes materialize in per §3.2.
///
/// Only the ten scalar common effects are considered here (not
/// `leak_color`, and not `vignette`/`blur`/`sharpen`/`grain`, which are not
/// promoted to their own node kind in this pass — see [`crate::COMMON_EFFECT_IDS`]).
fn effects_differing_from_default(base: Option<&[f32]>) -> Vec<(&'static str, f32)> {
    let Some(floats) = base else { return Vec::new() };
    if floats.len() < metal_schema::EFFECTS_WIDTH {
        return Vec::new();
    }
    let post = metal_schema::read_post(floats);
    let default = metal_schema::Post::default();
    let mut out = Vec::new();
    let fields: [(&str, f32, f32); 10] = [
        ("shake", post.shake, default.shake),
        ("light_leak", post.light_leak, default.light_leak),
        ("ghost", post.ghost, default.ghost),
        ("kaleido", post.kaleido, default.kaleido),
        ("crt", post.crt, default.crt),
        ("glitch", post.glitch, default.glitch),
        ("posterize", post.posterize, default.posterize),
        ("zoom", post.zoom, default.zoom),
        ("fire", post.fire, default.fire),
        ("lorenz", post.lorenz, default.lorenz),
    ];
    for (id, value, default_value) in fields {
        if (value - default_value).abs() > f32::EPSILON {
            out.push((id, value));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// format versions
// ---------------------------------------------------------------------------

/// Version 1 (plan "Audio input is a node", Design 5): audio input is a node.
/// Before it, every source reacted to the one process input with no cable;
/// after it, a source reads the audio node wired to its `control`, and an
/// unwired one is static.
pub const AUDIO_NODE_VERSION: u32 = 1;

/// Version 2 (plan "Projection: mask → surface → device", P1): a `mask` node
/// that carried a wall (`config.wall`, the `pared` mode) loses it and gains a
/// `venue.surface` on its downstream `image` edge. The mask keeps its
/// `contour` — that is source space; the wall is where the warp belongs.
pub const PARED_SURFACE_VERSION: u32 = 2;

/// Version 3 (materials M6): the model `matrix` is renamed `room`. The name
/// `matrix` goes to a material, so a saved `gen.matrix` is rewritten to the
/// live `gen.room`; the old id stays a tombstone (rule 1) and draws nothing.
pub const MODEL_ROOM_VERSION: u32 = 3;

/// The version [`Graph::new`] writes and [`upgrade`] brings a graph to.
pub const CURRENT: u32 = MODEL_ROOM_VERSION;

/// Whether `kind` is a source the audio drives (`gen.*`, `text.source`).
pub fn is_audio_driven_source(kind: &str) -> bool {
    kind.starts_with("gen.") || kind == crate::TEXT_SOURCE_KIND
}

/// Brings `g` to [`CURRENT`]. Returns whether any node or edge was added.
///
/// To version 1: every `gen.*`/`text.source` whose `control` is unwired gets
/// an edge from the graph's audio node — its first `audio.live` (or, in an
/// offline graph, its `audio.file`), else a new `audio.live {source: null}`
/// — so no graph saved before this version stops reacting. Nothing else in
/// the graph changes.
pub fn upgrade(g: &mut Graph) -> bool {
    if g.version >= CURRENT {
        return false;
    }
    let mut changed = false;
    if g.version < AUDIO_NODE_VERSION {
        let unwired: Vec<NodeId> = g
            .nodes
            .iter()
            .filter(|n| is_audio_driven_source(&n.kind))
            .filter(|n| !g.edges.iter().any(|e| e.to.node == n.id && e.to.port == "control"))
            .map(|n| n.id.clone())
            .collect();
        for id in unwired {
            wire_to_audio(g, &id);
            changed = true;
        }
    }
    if g.version < PARED_SURFACE_VERSION {
        // Every `mask` that carried a wall becomes `mask` (wall gone) plus a
        // `venue.surface { surface: <wall> }` inserted on the mask's
        // downstream `image` edge, so the wall's warp now happens where the
        // plan puts it — after the cut, before the device.
        let wall: Vec<(NodeId, String)> = g
            .nodes
            .iter()
            .filter(|n| n.kind == "mask")
            .filter_map(|n| {
                n.config
                    .get("wall")
                    .and_then(serde_json::Value::as_str)
                    .filter(|w| !w.is_empty())
                    .map(|w| (n.id.clone(), w.to_string()))
            })
            .collect();
        for (mask_id, wall) in wall {
            if let Some(n) = g.node_mut(&mask_id) {
                let mut value: serde_json::Value =
                    serde_json::from_str(&n.config.to_json_str()).unwrap_or(serde_json::Value::Null);
                if let Some(obj) = value.as_object_mut() {
                    obj.remove("wall");
                }
                n.config = crate::Config::from_value(value);
            }
            let pos = g.node(&mask_id).map(|n| n.pos).unwrap_or([0.0, 0.0]);
            let surface = g.add_node(
                "surface",
                crate::VENUE_SURFACE_KIND,
                [pos[0] + crate::layout::NODE_W + 40.0, pos[1]],
            );
            if let Some(n) = g.node_mut(&surface) {
                n.config = crate::Config::from_value(serde_json::json!({ "surface": wall }));
            }
            let downstream: Vec<(String, Endpoint)> = g
                .edges
                .iter()
                .filter(|e| e.from.node == mask_id && e.from.port == "image")
                .map(|e| (e.id.clone(), e.to.clone()))
                .collect();
            g.add_edge(Endpoint::new(&mask_id, "image"), Endpoint::new(&surface, "image"));
            for (edge_id, to) in downstream {
                g.remove_edge(&edge_id);
                g.add_edge(Endpoint::new(&surface, "image"), to);
            }
            changed = true;
        }
    }
    if g.version < MODEL_ROOM_VERSION {
        // Materials M6: the model `matrix` became `room`. A saved graph that
        // wired a `gen.matrix` source now draws the live `gen.room`; the old
        // id remains a tombstone so a positional reference still resolves.
        for n in &mut g.nodes {
            if n.kind == "gen.matrix" {
                n.kind = "gen.room".to_string();
                changed = true;
            }
        }
    }
    g.version = CURRENT;
    changed
}

/// Wires the graph's audio node (see [`upgrade`]; added when there is none)
/// into `node`'s `control`. Returns the audio node's id. Also what a writer
/// that adds a source on the operator's behalf (the layer table's sync)
/// calls, so a source it creates reacts the way the table's layer did.
pub fn wire_to_audio(g: &mut Graph, node: &str) -> NodeId {
    let audio = g
        .nodes
        .iter()
        .find(|n| n.kind == "audio.live")
        .or_else(|| g.nodes.iter().find(|n| n.kind == "audio.file"))
        .map(|n| n.id.clone());
    let audio = match audio {
        Some(id) => id,
        None => {
            let x = g.nodes.iter().map(|n| n.pos[0]).fold(f32::INFINITY, f32::min);
            let x = if x.is_finite() { x - 240.0 } else { 0.0 };
            let id = g.add_node("audio", "audio.live", [x, 0.0]);
            if let Some(n) = g.node_mut(&id) {
                n.config = crate::Config::from_value(serde_json::json!({ "source": null }));
            }
            id
        }
    };
    g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(node, "control"));
    audio
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Plan Design 9: a graph saved before audio nodes gets one explicit
    /// `audio.live {source: null}` wired to every unwired source, and only that.
    #[test]
    fn an_old_graph_migrates_to_an_explicit_audio_node() {
        let mut old = Graph::new("old", "old", Domain::Live);
        old.version = 0;
        let a = old.add_node("gen", "gen.turing", [0.0, 0.0]);
        let b = old.add_node("gen", "gen.dla", [0.0, 100.0]);
        let t = old.add_node("text", crate::TEXT_SOURCE_KIND, [0.0, 200.0]);
        let s = old.add_node("script", "script.py", [-100.0, 300.0]);
        let c = old.add_node("gen", "gen.terrarium", [0.0, 300.0]);
        // `c`'s control is already wired (to a script): left alone.
        old.add_edge(Endpoint::new(&s, "control"), Endpoint::new(&c, "control"));
        let text = old.to_json().replace("\"version\": 1", "\"version\": 0");

        let g = Graph::from_json(&text).expect("parses");
        assert_eq!(g.version, CURRENT);
        let audio: Vec<_> = g.nodes.iter().filter(|n| n.kind == "audio.live").collect();
        assert_eq!(audio.len(), 1, "one explicit audio node");
        assert_eq!(audio[0].config.get("source"), Some(&serde_json::Value::Null));
        for id in [&a, &b, &t] {
            assert!(
                g.edges.iter().any(|e| e.from.node == audio[0].id && e.from.port == "control"
                    && e.to.node == *id && e.to.port == "control"),
                "{id} is wired to the audio node"
            );
        }
        assert!(!g.edges.iter().any(|e| e.from.node == audio[0].id && e.to.node == c));
        assert_eq!(g.nodes.len(), old.nodes.len() + 1, "nothing else added");
        assert_eq!(g.edges.len(), old.edges.len() + 3);

        // Idempotent: a current graph is never upgraded again, so a source
        // the operator unwires afterwards stays static.
        let mut again = g.clone();
        assert!(!upgrade(&mut again));
        assert_eq!(again, g);
    }

    #[test]
    fn an_old_offline_graph_wires_its_audio_file_instead_of_adding_a_live_node() {
        let mut old = Graph::new("old", "old", Domain::Offline);
        old.version = 0;
        let f = old.add_node("audio", "audio.file", [0.0, 0.0]);
        let a = old.add_node("gen", "gen.turing", [100.0, 0.0]);
        assert!(upgrade(&mut old));
        assert!(!old.nodes.iter().any(|n| n.kind == "audio.live"), "lint forbids mixing");
        assert!(old.edges.iter().any(|e| e.from.node == f && e.to.node == a && e.to.port == "control"));
    }

    #[test]
    fn a_new_graph_is_current_and_its_unwired_source_stays_unwired() {
        let mut g = Graph::new("new", "new", Domain::Live);
        g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let back = Graph::from_json(&g.to_json()).unwrap();
        assert_eq!(back, g);
    }

    /// P1: a `mask` that carried a wall (the `pared` mode) becomes `mask`
    /// (wall gone, contour kept) plus a `venue.surface { surface: <wall> }`
    /// inserted on the mask's downstream `image` edge, before the device.
    #[test]
    fn a_pared_mask_migrates_to_mask_plus_surface() {
        let mut g = Graph::new("g", "pared", Domain::Live);
        g.version = AUDIO_NODE_VERSION;
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let mask = g.add_node("mask", "mask", [1.0, 0.0]);
        let out = g.add_node("out", "output.device", [2.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&mask, "image"));
        g.add_edge(Endpoint::new(&mask, "image"), Endpoint::new(&out, "image"));
        {
            let n = g.node_mut(&mask).expect("mask");
            n.config = crate::Config::from_json_str(
                r#"{"wall":"front","contour":[[0,0],[1,0],[1,1],[0,1]]}"#,
            )
            .expect("config");
        }

        assert!(upgrade(&mut g));
        assert_eq!(g.version, CURRENT);

        let m = g.node(&mask).expect("mask still there");
        assert!(m.config.get("wall").is_none(), "the wall key is gone");
        assert!(m.config.get("contour").is_some(), "the contour stays: it is source space");

        let surface = g
            .nodes
            .iter()
            .find(|n| n.kind == crate::VENUE_SURFACE_KIND)
            .expect("a venue.surface was inserted");
        assert_eq!(
            surface.config.get("surface").and_then(serde_json::Value::as_str),
            Some("front"),
            "the surface names the wall the mask carried"
        );
        // The chain is now mask → surface → device, and the old direct edge
        // is gone.
        assert!(g.edges.iter().any(|e| e.from.node == mask && e.to.node == surface.id));
        assert!(g.edges.iter().any(|e| e.from.node == surface.id && e.to.node == out));
        assert!(!g.edges.iter().any(|e| e.from.node == mask && e.to.node == out));

        // Idempotent: a current graph is never upgraded again.
        let mut again = g.clone();
        assert!(!upgrade(&mut again));
        assert_eq!(again, g);
    }

    /// M6: a graph saved before the `matrix` → `room` rename draws the live
    /// `room` model; the migration rewrites the generator kind, and is
    /// idempotent (a current graph is never touched again).
    #[test]
    fn a_saved_matrix_graph_migrates_to_room() {
        let mut g = Graph::new("g", "room", Domain::Live);
        g.version = PARED_SURFACE_VERSION;
        let a = g.add_node("gen", "gen.matrix", [0.0, 0.0]);
        let b = g.add_node("gen", "gen.turing", [0.0, 100.0]);
        let out = g.add_node("out", "output.device", [200.0, 0.0]);
        g.add_edge(Endpoint::new(&a, "image"), Endpoint::new(&out, "image"));
        g.add_edge(Endpoint::new(&b, "image"), Endpoint::new(&out, "image"));

        assert!(upgrade(&mut g));
        assert_eq!(g.version, CURRENT);
        assert_eq!(g.node(&a).expect("the generator stays").kind, "gen.room");
        assert_eq!(g.node(&b).expect("an unrelated generator is untouched").kind, "gen.turing");
        assert!(!g.nodes.iter().any(|n| n.kind == "gen.matrix"), "no gen.matrix is left to draw nothing");

        let mut again = g.clone();
        assert!(!upgrade(&mut again));
        assert_eq!(again, g);
    }

    fn manifest() -> metal_schema::Manifest {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json");
        let text = metal_vfs::read_to_string(path).expect("data/schema.json readable");
        metal_schema::Manifest::parse(&text).expect("schema.json parses")
    }

    fn collections_path() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/collections.json")
    }

    #[test]
    fn a_minimal_synthetic_collection_converts() {
        let json = r#"[{
            "id": "c1", "name": "test",
            "layers": [{
                "id": "l0", "name": "one", "model": 0, "opacity": 1.0, "z": 0,
                "blend": "normal", "sector": {"x":0.0,"y":0.0,"w":1.0,"h":1.0},
                "active": true, "solo": false, "base": null, "feed": "model",
                "picked": [], "mask": null
            }]
        }]"#;
        let m = manifest();
        let graphs = from_layers(json, &m).expect("converts");
        assert_eq!(graphs.len(), 1);
        let g = &graphs[0];
        // one output.device + one generator, no effect nodes (base is null).
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 1);
        assert!(g.nodes.iter().any(|n| n.kind == "output.device"));
        let gen = g.nodes.iter().find(|n| n.kind != "output.device").unwrap();
        assert_eq!(gen.kind, format!("gen.{}", m.models[0].id));
    }

    /// The test A7 actually asks for: the real `data/collections.json` in
    /// this repo, if there is one, converts to a structurally correct graph.
    #[test]
    fn the_real_collections_json_converts_with_the_right_counts() {
        let Ok(json) = metal_vfs::read_to_string(collections_path()) else {
            eprintln!("no data/collections.json in this checkout; skipping");
            return;
        };
        let m = manifest();
        let graphs = from_layers(&json, &m).expect("real collections.json converts");

        let raw: Vec<RawCollection> = serde_json::from_str(&json).expect("same file parses raw");
        assert_eq!(graphs.len(), raw.len());

        for (g, c) in graphs.iter().zip(raw.iter()) {
            // One output.device, one generator per layer, plus one fx node
            // per differing effect. With today's data every `base` is null,
            // so it's exactly 1 + layers.len() nodes and layers.len() edges.
            let expected_generators = c.layers.len();
            let generators = g.nodes.iter().filter(|n| n.kind.starts_with("gen.")).count();
            assert_eq!(generators, expected_generators);
            assert_eq!(g.nodes.iter().filter(|n| n.kind == "output.device").count(), 1);
            // Every layer contributes exactly one edge into the device
            // (chained through any fx nodes it materialized).
            let into_device =
                g.edges.iter().filter(|e| g.nodes.iter().any(|n| n.id == e.to.node && n.kind == "output.device")).count();
            assert_eq!(into_device, c.layers.len());
        }
    }
}
