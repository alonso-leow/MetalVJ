//! Isolating one stage of the picture for debugging (`render_node`,
//! `GET /node.jpg`), and the multi-source grid it is debugged against.
//!
//! One collection, 2026-09-30: effects cut at a node, a mask's slice of the
//! output, an output node answering the device frame, and a `Sectors` grid of
//! several sources in one output composing each tile from its own source.
//! Each test writes a graph under a device id unique to this process and
//! removes it again (same discipline as the LAY.7b/7c tests in `engine.rs`).

use super::*;
use crate::paint;
use metal_graph::{Config, Domain, Endpoint, Graph};

fn cfg(v: serde_json::Value) -> Config {
    Config::from_json_str(&v.to_string()).unwrap()
}

fn setup() -> (Box<dyn Paint>, crate::listeners::Listeners, State) {
    let paint = paint::painter();
    let sim = crate::listeners::Listeners::stand_in();
    let mut state = State::new(paint.as_ref());
    state.paused_at = Some(3.0);
    (paint, sim, state)
}

/// `gen → fx.posterize → fx.vignette → output.device`, with a control edge into
/// the generator from an audio node. Returns the graph and its node ids.
fn chain_graph(device: &str, gen_model: &str) -> (Graph, [String; 5]) {
    let mut g = Graph::new(&device_graph_id(device), device, Domain::Live);
    let out = g.add_node("out", "output.device", [800.0, 0.0]);
    let audio = g.add_node("audio", "audio.live", [-400.0, 0.0]);
    let gen = g.add_node("gen", &format!("gen.{gen_model}"), [0.0, 0.0]);
    let ghost = g.add_node("fx", "fx.posterize", [200.0, 0.0]);
    g.node_mut(&ghost).unwrap().config = cfg(json!({ "fixed": 0.9 }));
    let shake = g.add_node("fx", "fx.vignette", [400.0, 0.0]);
    g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&gen, "control"));
    g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&ghost, "image"));
    g.add_edge(Endpoint::new(&ghost, "image"), Endpoint::new(&shake, "image"));
    g.add_edge(Endpoint::new(&shake, "image"), Endpoint::new(&out, "image"));
    (g, [out, audio, gen, ghost, shake])
}

fn image_into<'a>(g: &'a Graph, node: &str) -> Vec<&'a str> {
    g.edges
        .iter()
        .filter(|e| e.to.node == node && e.to.port == "image")
        .map(|e| e.from.node.as_str())
        .collect()
}

/// Cutting at the first effect keeps the generator and that effect, feeds a
/// stand-in output from it, and leaves nothing downstream fed: the second
/// effect and the real output no longer see the picture. The control edge
/// into the generator survives, so the row is driven exactly as live.
#[test]
fn isolate_graph_cuts_the_chain_right_after_the_node() {
    let (g, [out, audio, gen, ghost, shake]) = chain_graph("iso-cut", "x");
    let sub = isolate_graph(&g, &ghost).expect("the node is in the graph");

    assert_eq!(image_into(&sub, ISOLATED_OUT), vec![ghost.as_str()], "the stand-in output is fed by the cut node");
    assert!(image_into(&sub, &shake).is_empty(), "nothing after the cut receives the picture");
    assert!(sub.node(&out).is_none(), "the real output is gone, so it cannot pull its own chain");
    assert_eq!(image_into(&sub, &ghost), vec![gen.as_str()], "the chain up to the node is intact");
    assert!(
        sub.edges.iter().any(|e| e.from.node == audio && e.to.node == gen),
        "control/binding edges into the kept chain stay"
    );
    assert!(isolate_graph(&g, "nope").is_none(), "an unknown node isolates nothing");
}

/// The chain the renderer resolves for the generator ends at the cut: the
/// full graph resolves both effects, the graph cut at the first effect only
/// that one, and the graph cut at the generator none — which is exactly
/// "the picture after that node".
#[test]
fn the_resolved_effect_chain_stops_at_the_isolated_node() {
    let (g, [_, _, gen, ghost, _]) = chain_graph("iso-chain", "x");
    let steps = |graph: &Graph| {
        let mut post = metal_schema::read_post(&vec![0.0; metal_schema::EFFECTS_WIDTH]);
        crate::resolved_chain(graph, &gen, &mut post, &[]).len()
    };
    let full = steps(&g);
    let at_fx = steps(&isolate_graph(&g, &ghost).unwrap());
    let at_gen = steps(&isolate_graph(&g, &gen).unwrap());
    assert_eq!(full, 2, "the whole chain: posterize then vignette");
    assert_eq!(at_fx, 1, "cut at the posterize: only the posterize");
    assert_eq!(at_gen, 0, "cut at the generator: its raw picture");
}

/// End to end through the one render path: each stage renders, an output
/// node answers the device's own composed frame, and turning the effect on
/// changes that stage's picture (the effect is really applied there, not
/// only drawn in the diagram).
#[test]
fn render_node_draws_each_stage_and_the_output_is_the_device_frame() {
    let device = format!("iso-render-{}", std::process::id());
    let root = output::data_root();
    let (mut paint, mut sim, mut state) = setup();
    let ids = paint.ids();
    let (g, [out, _, gen, ghost, shake]) = chain_graph(&device, ids[0]);
    save_graph_at(&root, &g).unwrap();
    state.sources.insert(device.clone(), layers::Device::default());

    let (w, h) = (48, 27);
    let mut stage = |node: &str| render_node(&mut *paint, &mut sim, &mut state, &device, node, w, h);
    let raw = stage(&gen).expect("the generator stage renders");
    let fx = stage(&ghost).expect("the effect stage renders");
    assert!(stage(&shake).is_some(), "the last effect stage renders");
    assert_eq!((raw.1, raw.2), (w, h));
    assert!(stage("nope").is_none(), "an unknown node renders nothing");

    let as_out = render_node(&mut *paint, &mut sim, &mut state, &device, &out, w, h).unwrap();
    let device_frame = render_device(&mut *paint, &mut sim, &mut state, &device, w, h);
    assert_eq!(as_out, device_frame, "the output node is the device frame /output.jpg draws");

    // With the effect bypassed the stage must differ from it on at 0.9 —
    // the effect is really applied at that stage. Only the wgpu painters
    // run effect chains; the `stand` build draws the generator alone.
    #[cfg(feature = "gpu")]
    {
        let mut off = g.clone();
        off.node_mut(&ghost).unwrap().bypass = true;
        save_graph_at(&root, &off).unwrap();
        let fx_off = render_node(&mut *paint, &mut sim, &mut state, &device, &ghost, w, h).unwrap();
        assert_ne!(fx.0, fx_off.0, "the posterize at 0.9 changes the picture at its own stage");
    }
    #[cfg(not(feature = "gpu"))]
    let _ = (&fx, &g);

    let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));
}

/// Each kind's four variants are four distinct float sets, the same every
/// time, each float inside its own range (the catalogue's for an effect, the
/// row's for a material).
#[test]
fn kind_variants_are_four_distinct_float_sets_in_range() {
    for kind in ["fx.posterize", "mat.wood"] {
        let sets: Vec<Vec<(String, f32)>> = (0..VARIANT_T.len()).map(|k| kind_variant_floats(kind, k)).collect();
        assert!(!sets[0].is_empty(), "{kind} exposes floats to vary");
        for (k, set) in sets.iter().enumerate() {
            assert_eq!(set, &kind_variant_floats(kind, k), "{kind} variant {k} is deterministic");
            for (name, v) in set {
                let (min, max) = if kind.starts_with("mat.") {
                    metal_graph::materials::range_of(name).expect("a material key has a range")
                } else {
                    let p = metal_graph::params::effect_params(kind)
                        .into_iter()
                        .find(|p| &p.name == name)
                        .expect("the effect declares the float");
                    (p.min, p.max)
                };
                assert!(*v > min && *v < max, "{kind}.{name} = {v} inside ({min}, {max})");
            }
        }
        for a in 0..sets.len() {
            for b in a + 1..sets.len() {
                assert_ne!(sets[a][0].1, sets[b][0].1, "{kind}: variants {a} and {b} differ");
            }
        }
    }
    assert!(kind_variant_floats("gen.rings", 0).is_empty(), "a model is not a kind preview");
    assert!(kind_preview_graph("gen.rings", 0, "rings").is_none());
}

/// An fx kind renders its own preview at each variant, over the ramp source,
/// at the size asked for; a kind with no preview renders nothing.
#[test]
fn render_kind_draws_an_fx_preview_at_each_variant() {
    let (mut paint, mut sim, mut state) = setup();
    for k in 0..VARIANT_T.len() {
        let (_, w, h) = render_kind(&mut *paint, &mut sim, &mut state, "fx.posterize", k, 48, 27)
            .expect("the fx kind has a preview");
        assert_eq!((w, h), (48, 27));
    }
    assert!(render_kind(&mut *paint, &mut sim, &mut state, "nope.kind", 0, 48, 27).is_none());
}

/// A mask node's stage is its slice of the output: the device frame
/// cropped to the contour's bounds — the one part of the output it renders.
#[test]
fn render_node_on_a_mask_is_its_slice_of_the_output() {
    let device = format!("iso-mask-{}", std::process::id());
    let root = output::data_root();
    let (mut paint, mut sim, mut state) = setup();
    let ids = paint.ids();

    let mut out = output::Output::new(&device, "iso mask", 64, 64);
    out.masks.push(Mask {
        id: "centre".into(),
        name: "centre".into(),
        points: vec![[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]],
        ..Mask::default()
    });
    state.outputs.list.push(out);
    state.sources.insert(device.clone(), layers::Device::default());

    let mut g = Graph::new(&device_graph_id(&device), &device, Domain::Live);
    let out_node = g.add_node("out", "output.device", [800.0, 0.0]);
    let gen = g.add_node("gen", &format!("gen.{}", ids[0]), [0.0, 0.0]);
    let mask = g.add_node("mask", "mask", [200.0, 0.0]);
    g.node_mut(&mask).unwrap().config = cfg(json!({ "device": device, "mask": "centre" }));
    g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&mask, "image"));
    g.add_edge(Endpoint::new(&mask, "image"), Endpoint::new(&out_node, "image"));
    save_graph_at(&root, &g).unwrap();

    let slice = render_node(&mut *paint, &mut sim, &mut state, &device, &mask, 64, 64).expect("the mask stage renders");
    assert_eq!((slice.1, slice.2), (32, 32), "the contour's bounds: half the device each way");
    let full = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 64);
    let bounds = Sector { x: 0.25, y: 0.25, w: 0.5, h: 0.5 };
    assert_eq!(slice, crop_window(&full.0, full.1, full.2, &bounds).unwrap(), "the same pixels the output shows there");

    let _ = metal_vfs::remove_file(graph_path_at(&root, &device_graph_id(&device)));
}

/// Several sources in one output, as a grid (`Composition::Sectors`): each
/// half of the frame is exactly what that source alone draws in that half —
/// no tile bleeds into the other, and neither is dropped.
#[test]
fn a_sectors_grid_composes_each_tile_from_its_own_source() {
    let device = format!("iso-grid-{}", std::process::id());
    let (mut paint, mut sim, mut state) = setup();
    let (w, h) = (64u32, 32u32);

    let left = Sector { x: 0.0, y: 0.0, w: 0.5, h: 1.0 };
    let right = Sector { x: 0.5, y: 0.0, w: 0.5, h: 1.0 };
    let layer = |id: &str, model: usize, sector: Sector| {
        let mut l = Layer::new(id, id, model);
        l.outputs = vec![device.clone()];
        l.sector = sector;
        l
    };
    let grid = layers::Device { composition: layers::Composition::Sectors, ..layers::Device::default() };
    state.sources.insert(device.clone(), grid);

    let mut frame_with = |state: &mut State, list: Vec<Layer>| {
        state.layers = list;
        render_device(&mut *paint, &mut sim, state, &device, w, h)
    };
    let both = frame_with(&mut state, vec![layer("a", 0, left), layer("b", 1, right)]);
    let only_a = frame_with(&mut state, vec![layer("a", 0, left)]);
    let only_b = frame_with(&mut state, vec![layer("b", 1, right)]);

    let half = |f: &(Vec<u8>, u32, u32), s: &Sector| crop_window(&f.0, f.1, f.2, s).unwrap().0;
    assert_eq!(half(&both, &left), half(&only_a, &left), "the left tile is source a alone");
    assert_eq!(half(&both, &right), half(&only_b, &right), "the right tile is source b alone");
    assert!(half(&both, &left).iter().any(|&p| p != 0), "the left tile is not black");
    assert!(half(&both, &right).iter().any(|&p| p != 0), "the right tile is not black");
}
