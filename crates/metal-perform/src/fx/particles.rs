//! `fx.particles` — stateless dust, hashed from `(seed, i, t)`.
//!
//! See [`crate::fx`]'s module doc for where this sits in the pipeline and why
//! there is no simulation buffer. The WGSL the GPU runs is
//! `data/shaders/particles.wgsl`; [`dust`] is the CPU stand-in's version of the
//! same arithmetic, so a host with no adapter still shows the dust and the
//! goldens have bytes to freeze.
//!
//! # The config keys, in the order the shader reads them
//!
//! Each float lives under its own key on the node (the `fx.move` convention),
//! so `effect_params` can declare them and `set_held` can write them:
//!
//! | key | meaning | range |
//! |---|---|---|
//! | `count` | fraction of 4096 points | 0..1 |
//! | `size` | point radius | 0..1 |
//! | `drift` | how fast the field travels | 0..1 |
//! | `direction` | the angle it travels on | 0..360° |
//! | `jitter` | per-point wobble | 0..1 |
//! | `twinkle` | how much each point blinks | 0..1 |
//! | `glow` | additive strength | 0..1 |
//! | `depth` | parallax: near points are bigger | 0..1 |
//! | `seed` | the field's identity | 0..1000 |
//!
//! The colour slot `dust` is a separate key (`dust`), a three-number RGB the
//! node's config may carry; [`dust_tint`] reads it with a warm-white default.

use std::f32::consts::TAU;

use super::{soft, unit};

/// The effect's kind.
pub const FX_PARTICLES_KIND: &str = "fx.particles";

/// The shader id `fx.particles` draws with — a built-in file, like
/// `fx.move`'s `move`, so nothing has to be saved before the node works.
pub const PARTICLES_SHADER: &str = "particles";

/// The most points `count = 1` asks for — decision 6's own number.
pub const MAX_POINTS: usize = 4096;

/// The colour slot's default, used when the node carries no `dust`.
pub const DUST_DEFAULT: [f32; 3] = [1.0, 0.92, 0.78];

/// `fx.particles`'s nine floats, read off the node's config with their
/// declared defaults when unset and clamped to their declared ranges — the
/// order `data/shaders/particles.wgsl` reads them (`floats2.x` onward).
pub fn particles_ingest(node: &metal_graph::Node) -> [f32; 9] {
    let config: serde_json::Value =
        serde_json::from_str(&node.config.to_json_str()).unwrap_or(serde_json::Value::Null);
    let get = |key: &str, default: f32, min: f32, max: f32| {
        config.get(key).and_then(serde_json::Value::as_f64).map(|v| v as f32).unwrap_or(default).clamp(min, max)
    };
    [
        get("count", 0.3, 0.0, 1.0),
        get("size", 0.5, 0.0, 1.0),
        get("drift", 0.1, 0.0, 1.0),
        get("direction", 0.0, 0.0, 360.0),
        get("jitter", 0.2, 0.0, 1.0),
        get("twinkle", 0.5, 0.0, 1.0),
        get("glow", 0.3, 0.0, 1.0),
        get("depth", 0.5, 0.0, 1.0),
        get("seed", 0.0, 0.0, 1000.0),
    ]
}

/// The `dust` colour slot: a three-number RGB array in the node's config, or
/// [`DUST_DEFAULT`] when it carries none (or a malformed one).
pub fn dust_tint(node: &metal_graph::Node) -> [f32; 3] {
    let config: serde_json::Value =
        serde_json::from_str(&node.config.to_json_str()).unwrap_or(serde_json::Value::Null);
    let arr = config.get("dust").and_then(serde_json::Value::as_array);
    let Some(arr) = arr else { return DUST_DEFAULT };
    let mut out = DUST_DEFAULT;
    for (i, v) in arr.iter().take(3).enumerate() {
        if let Some(n) = v.as_f64() {
            out[i] = (n as f32).clamp(0.0, 1.0);
        }
    }
    out
}

/// The one `fx.particles` node's shader id and ingest, when `fx_node` is one.
/// `None` for any other node, the same shape [`crate::motion::move_node`] has.
pub fn particles_node(graph: &metal_graph::Graph, fx_node: &str) -> Option<(String, [f32; 9])> {
    let node = graph.node(fx_node)?;
    (node.kind == FX_PARTICLES_KIND).then(|| (PARTICLES_SHADER.to_string(), particles_ingest(node)))
}

/// The number of points an ingest asks for.
fn point_count(ingest: &[f32; 9]) -> usize {
    (ingest[0].clamp(0.0, 1.0) * MAX_POINTS as f32).round() as usize
}

/// Hashes the dust field over `frame` on the CPU — the stand-in's honest
/// version of `data/shaders/particles.wgsl`.
///
/// Pure in `(ingest, time, tint)`: the same inputs are the same pixels, which
/// is what makes the effect survive fan-out and scrubbing and what the goldens
/// freeze. Additive, clamped to 0..255.
pub fn dust(
    frame: &(Vec<u8>, u32, u32),
    ingest: &[f32; 9],
    time: f32,
    tint: [f32; 3],
) -> (Vec<u8>, u32, u32) {
    let (w, h) = (frame.1, frame.2);
    if w == 0 || h == 0 || frame.0.len() < (w as usize * h as usize * 4) {
        return frame.clone();
    }
    let mut out = frame.0.clone();
    let seed = ingest[8].clamp(0.0, 1000.0) as u32;
    let angle = ingest[3].to_radians();
    let (dx, dy) = (angle.cos(), angle.sin());
    let scale = w.min(h) as f32;
    let aspect = w as f32 / h as f32;

    for i in 0..point_count(ingest) {
        let i = i as u32;
        let px = unit(seed, i, 0);
        let py = unit(seed, i, 1);
        let phase = unit(seed, i, 2);
        let size = 0.5 + unit(seed, i, 3);
        // Drift, then wrap; the field is a torus so it never runs out.
        let mut x = px + dx * ingest[2] * time * 0.5;
        let mut y = py + dy * ingest[2] * time * 0.5;
        x = x.fract();
        y = y.fract();
        // Jitter: a small per-point orbit, out of phase with its neighbours.
        x += (time * 3.0 + phase * TAU).sin() * ingest[4] * 0.05;
        y += (time * 2.3 + phase * TAU).cos() * ingest[4] * 0.05;
        if x < 0.0 || x > 1.0 || y < 0.0 || y > 1.0 {
            continue;
        }
        // Depth is parallax: a "near" point (depth high) is bigger.
        let r = ingest[1] * size * (0.4 + 1.2 * ingest[7]) * 0.12 * scale;
        if r < 0.5 {
            continue;
        }
        let twinkle = 0.5 + 0.5 * (time * 6.0 + phase * TAU).sin();
        let alpha = (1.0 - ingest[5] * (1.0 - twinkle)) * (0.4 + ingest[6]);
        let (cx, cy) = (x * w as f32, y * h as f32);
        let x0 = (cx - r).floor().max(0.0) as i32;
        let x1 = (cx + r).ceil().min(w as f32 - 1.0) as i32;
        let y0 = (cy - r).floor().max(0.0) as i32;
        let y1 = (cy + r).ceil().min(h as f32 - 1.0) as i32;
        for py_i in y0..=y1 {
            for px_i in x0..=x1 {
                let ddx = (px_i as f32 - cx) / r;
                let ddy = (py_i as f32 - cy) / r * aspect;
                let fall = soft((ddx * ddx + ddy * ddy).sqrt());
                if fall <= 0.0 {
                    continue;
                }
                let a = (fall * alpha).clamp(0.0, 1.0);
                let p = (py_i as usize * w as usize + px_i as usize) * 4;
                for c in 0..3 {
                    let v = out[p + c] as f32 + tint[c] * a * 255.0;
                    out[p + c] = v.clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    (out, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_graph::{Domain, Graph};

    fn frame(w: u32, h: u32) -> (Vec<u8>, u32, u32) {
        (vec![0u8; (w as usize * h as usize * 4)], w, h)
    }

    fn node_with(json: &str) -> metal_graph::Node {
        let mut n = metal_graph::Node::new("fx-1", FX_PARTICLES_KIND, [0.0, 0.0]);
        n.config = metal_graph::Config::from_json_str(json).unwrap();
        n
    }

    /// FNV-1a over the frame's RGBA bytes — the golden corpus's hash, written
    /// out here for the same reason `metal_perform::engine`'s tests write theirs
    /// out: the point is a value a human can paste.
    fn frame_hash(px: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in px {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    #[test]
    fn particles_ingest_reads_the_config_with_defaults_and_clamps() {
        let n = node_with(r#"{"count":0.5,"direction":90,"seed":7}"#);
        let got = particles_ingest(&n);
        assert_eq!(got[0], 0.5);
        assert_eq!(got[3], 90.0);
        assert_eq!(got[8], 7.0);
        // Out of range clamps, an unset key reads its default.
        let n = node_with(r#"{"count":4.0,"seed":99999.0}"#);
        let got = particles_ingest(&n);
        assert_eq!(got[0], 1.0);
        assert_eq!(got[8], 1000.0);
        assert_eq!(got[1], 0.5, "size default");
    }

    #[test]
    fn particles_are_deterministic_for_a_seed() {
        let ingest = particles_ingest(&node_with(r#"{"count":0.2,"seed":42}"#));
        let f = frame(48, 27);
        let a = dust(&f, &ingest, 1.25, DUST_DEFAULT);
        let b = dust(&f, &ingest, 1.25, DUST_DEFAULT);
        assert_eq!(a.0, b.0, "the same seed and instant must be the same dust");
        assert_ne!(a.0, f.0, "the dust must change the frame at all");

        // A different seed is a different field.
        let other = particles_ingest(&node_with(r#"{"count":0.2,"seed":43}"#));
        assert_ne!(dust(&f, &other, 1.25, DUST_DEFAULT).0, a.0);

        // And time moves the field.
        assert_ne!(dust(&f, &ingest, 1.30, DUST_DEFAULT).0, a.0);
    }

    #[test]
    fn the_particles_shader_is_a_built_in_effect() {
        let wgsl = crate::forge::source(&crate::output::data_root(), PARTICLES_SHADER)
            .expect("data/shaders/particles.wgsl is a built-in shader");
        assert!(wgsl.contains("fs_main"), "the shader must be a forge effect");
        assert!(wgsl.contains("texture_2d"), "an effect samples the picture it is handed");
    }

    #[test]
    fn particles_node_is_found_by_kind() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let p = g.add_node("fx", FX_PARTICLES_KIND, [0.0, 0.0]);
        let m = g.add_node("fx", "fx.move", [1.0, 0.0]);
        assert!(particles_node(&g, &p).is_some());
        assert!(particles_node(&g, &m).is_none());
    }

    /// End to end through the painter: a generator with an `fx.particles`
    /// node after it is not the same frame as the generator alone, and the
    /// effect costs no extra readback (it runs in the one `fx.*` branch).
    #[test]
    fn the_paint_branch_runs_the_dust_over_the_picture() {
        use crate::paint::Paint;
        use metal_graph::Endpoint;
        let plain = {
            let mut g = Graph::new("g", "t", Domain::Live);
            let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
            let out = g.add_node("out", "output.device", [1.0, 0.0]);
            g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
            g
        };
        let dusty = {
            let mut g = Graph::new("g", "t", Domain::Live);
            let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
            let fx = g.add_node("fx", FX_PARTICLES_KIND, [1.0, 0.0]);
            let out = g.add_node("out", "output.device", [2.0, 0.0]);
            g.node_mut(&fx).unwrap().config =
                metal_graph::Config::from_json_str(r#"{"count":0.4,"size":0.6,"seed":5}"#).unwrap();
            g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&fx, "image"));
            g.add_edge(Endpoint::new(&fx, "image"), Endpoint::new(&out, "image"));
            g
        };
        let mut stand = crate::stand::Stand::new();
        let index = stand.ids().iter().position(|id| *id == "turing").unwrap();
        let row = stand.schemas()[index].defaults();
        let gen_of = |g: &metal_graph::Graph| g.nodes.iter().find(|n| n.kind == "gen.turing").unwrap().id.clone();
        let rows_plain: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
            [(gen_of(&plain), row.clone())].into_iter().collect();
        let rows_dusty: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
            [(gen_of(&dusty), row.clone())].into_iter().collect();
        let a = stand.run(&plain, &plain.compile(), &rows_plain, &std::collections::HashMap::new(), &[], 2.0, 64, 36);
        let b = stand.run(&dusty, &dusty.compile(), &rows_dusty, &std::collections::HashMap::new(), &[], 2.0, 64, 36);
        assert_ne!(
            a.frames.values().next().unwrap().0,
            b.frames.values().next().unwrap().0,
            "fx.particles did not draw the dust"
        );
        assert_eq!(b.readbacks, 1, "the dust runs in the same one readback");
    }

    /// The dust's frozen frame at a fixed seed and instant, through the same
    /// `dust` call the live path makes. Re-freeze with `FW_PRINT_GOLDEN=1`.
    #[test]
    fn the_particles_frame_is_frozen_for_a_seed() {
        let ingest = particles_ingest(&node_with(r#"{"count":0.35,"size":0.5,"drift":0.1,"direction":30,"jitter":0.2,"twinkle":0.5,"glow":0.3,"depth":0.5,"seed":11}"#));
        let f = frame(64, 36);
        let px = dust(&f, &ingest, 2.0, DUST_DEFAULT).0;
        let hash = frame_hash(&px);
        if std::env::var("FW_PRINT_GOLDEN").is_ok() {
            eprintln!("GOLDEN particles {hash:#018x}");
        }
        assert_eq!(
            hash, 0x60b25198d139989e,
            "the particles frame changed; if that is deliberate, re-freeze this value"
        );
    }
}
