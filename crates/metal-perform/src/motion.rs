//! The two motion effects (K): `fx.speed` and `fx.move`.
//!
//! Both are `Effect`-family nodes (`Stage::Modulate`) with the usual effect
//! ports, and both do their work in `Paint::run`'s effect branch rather than in
//! a generator's post pass — like `fx.forge`, they act on the *picture*, and a
//! speed change has to reach the renderer's own clock rather than a float in a
//! row.
//!
//! # `fx.speed` — a per-generator clock that never jumps
//!
//! The engine keeps, per `(device, generator)`, an **accumulated time** that
//! advances by `dt × speed`, where `speed` is the product of every `fx.speed`
//! amount in that generator's chain. The generator is drawn at that time
//! instead of the show clock, so speeding it up or down changes how fast it
//! moves **from now on** — because the accumulated time is carried across
//! frames, a change never makes the picture leap to a new phase the way
//! `time × speed` would. This is [`Clocks`].
//!
//! `speed` is `0..8`, forward only: the brief's own range, and reverse is not
//! something a source of the current generation can honestly play back. `1` is
//! the identity — an `fx.speed` at `1` draws exactly what the show clock would,
//! and a chain with no `fx.speed` at all is never even consulted (the generator
//! keeps the flat time).
//!
//! # `fx.move` — the picture moves, the mask stays
//!
//! A translation of the source's picture relative to its mask, run as an effect
//! shader (`data/shaders/move.wgsl`) over the frame it is handed. Its five
//! parameters are written into the shader's ingest slots by [`move_ingest`] —
//! `x`, `y` (offset, `-1..1` of the frame), `vx`, `vy` (drift **per beat**,
//! advanced by the generator's own accumulated time, so a drift written per
//! beat stalls when the music does) and `wrap`. The CPU stand-in offsets its
//! pixels the same distance with [`translate`], so a host with no adapter still
//! shows the content move.

use std::collections::HashMap;

use metal_graph::{Graph, NodeId};

/// The speed effect's kind.
pub const FX_SPEED_KIND: &str = "fx.speed";
/// The move effect's kind.
pub const FX_MOVE_KIND: &str = "fx.move";

/// The shader id `fx.move` draws with — a built-in file, like `fx.forge`'s
/// examples, so nothing has to be saved before the node works.
pub const MOVE_SHADER: &str = "move";

/// The `fx.speed` amount a node holds — its `config.fixed`, the slot every
/// `fx.*` node's amount lives in (`crate::bind`), clamped to its declared
/// `0..8`. A node with no held value reads the declared default, `1`.
pub fn speed_of_node(node: &metal_graph::Node) -> f32 {
    let config: serde_json::Value =
        serde_json::from_str(&node.config.to_json_str()).unwrap_or(serde_json::Value::Null);
    config.get("fixed").and_then(serde_json::Value::as_f64).map(|v| v as f32).unwrap_or(1.0).clamp(0.0, 8.0)
}

/// The speed multiplier a generator's whole chain applies: the product of
/// every `fx.speed` amount on the path from the generator to the output, `1.0`
/// when there is none. A product because two speed nodes compose — halving and
/// then doubling is the identity, which is what a chain of them means.
pub fn speed_of(graph: &Graph, gen: &str) -> f32 {
    let mut speed = 1.0f32;
    for fx in metal_graph::effects::chain(graph, gen) {
        if graph.node(&fx).map(|n| n.kind == FX_SPEED_KIND).unwrap_or(false) {
            speed *= graph.node(&fx).map(speed_of_node).unwrap_or(1.0);
        }
    }
    speed
}

/// The values `fx.move` writes into the effect shader's ingest, in the order
/// `data/shaders/move.wgsl` reads them: `x`, `y`, `vx`, `vy`, `wrap`. Each is
/// read off the node's config with its declared default when unset.
pub fn move_ingest(node: &metal_graph::Node) -> [f32; 5] {
    let config: serde_json::Value =
        serde_json::from_str(&node.config.to_json_str()).unwrap_or(serde_json::Value::Null);
    let get = |key: &str, default: f32, min: f32, max: f32| {
        config.get(key).and_then(serde_json::Value::as_f64).map(|v| v as f32).unwrap_or(default).clamp(min, max)
    };
    [
        get("x", 0.0, -1.0, 1.0),
        get("y", 0.0, -1.0, 1.0),
        get("vx", 0.0, -1.0, 1.0),
        get("vy", 0.0, -1.0, 1.0),
        if get("wrap", 0.0, 0.0, 1.0) >= 0.5 { 1.0 } else { 0.0 },
    ]
}

/// The one move node in a generator's chain whose shader should run over the
/// frame in `Paint::run`'s effect branch, with the ingest it reads — the last
/// one wins, the same "the tail is the picture" rule the chain's other
/// forwards follow. `None` when the chain has no `fx.move`.
pub fn move_node(graph: &Graph, fx_node: &str) -> Option<(String, [f32; 5])> {
    let node = graph.node(fx_node)?;
    (node.kind == FX_MOVE_KIND).then(|| (MOVE_SHADER.to_string(), move_ingest(node)))
}

/// Per-generator clocks (K): the accumulated time that makes `fx.speed` change
/// the rate of a picture without ever jumping its phase.
///
/// Keyed by `(device, generator)`. The device is part of the key because one
/// graph renders on several devices and a generator sped up on the wall must
/// not also speed the laptop's preview — each device accumulates its own clock,
/// the same way each device composes its own stack.
#[derive(Debug, Default)]
pub struct Clocks {
    /// The last show-clock time each device was rendered at, so the next frame
    /// knows its `dt`. `None` on the first frame of a device (there is no
    /// previous time to subtract), which starts its generators at the show
    /// clock rather than at zero.
    last: HashMap<String, f32>,
    /// Each `(device, generator)`'s accumulated time, in seconds.
    at: HashMap<(String, String), f32>,
}

impl Clocks {
    /// Advances every generator of `graph` by `dt × speed` and returns the
    /// time each should draw at, this frame.
    ///
    /// `time` is the show clock. A device seen for the first time starts every
    /// generator at `time` (so a graph dropped mid-show does not snap back to
    /// zero), and `dt` is clamped to a sane ceiling (`1/4 s`) so a long pause,
    /// a breakpoint or a window dragged offscreen does not fling every clock
    /// forward by the length of the gap — a spike of accumulated time is the
    /// one way this can visibly jump even though the rate is integrated.
    pub fn advance(&mut self, device: &str, graph: &Graph, time: f32) -> HashMap<NodeId, f32> {
        const MAX_DT: f32 = 1.0;
        let dt = match self.last.get(device) {
            Some(&prev) => (time - prev).clamp(0.0, MAX_DT),
            None => 0.0,
        };
        self.last.insert(device.to_string(), time);

        let mut out = HashMap::new();
        for node in graph.nodes.iter().filter(|n| n.kind.starts_with("gen.")) {
            let entry = self.at.entry((device.to_string(), node.id.clone())).or_insert(time);
            let speed = speed_of(graph, &node.id);
            if speed != 1.0 {
                *entry += dt * speed;
            } else {
                // No `fx.speed`: the generator follows the show clock exactly.
                // Tracking it here too keeps the accumulated clock in step, so
                // dropping an `fx.speed` on next frame continues from *now*
                // rather than from wherever the last sped-up frame left it.
                *entry = time;
            }
            out.insert(node.id.clone(), *entry);
        }
        out
    }
}

/// Applies `fx.move`'s offset to an RGBA frame on the CPU — the stand-in's
/// honest version of `data/shaders/move.wgsl`, so a host with no adapter still
/// sees the content move.
///
/// `ingest` is [`move_ingest`]'s five values; `time` is the generator's own
/// accumulated time, which is the beat clock the drift advances on. `wrap`
/// tiles, otherwise the edges hold (clamp), matching the shader.
pub fn translate(frame: &(Vec<u8>, u32, u32), ingest: [f32; 5], time: f32) -> (Vec<u8>, u32, u32) {
    let src = &frame.0;
    let (w, h) = (frame.1, frame.2);
    if w == 0 || h == 0 {
        return frame.clone();
    }
    let dx = (ingest[0] + ingest[2] * time) * w as f32;
    let dy = (ingest[1] + ingest[3] * time) * h as f32;
    if dx.abs() < 0.5 && dy.abs() < 0.5 {
        return frame.clone();
    }
    let wrap = ingest[4] >= 0.5;
    let mut out = vec![0u8; src.len()];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let mut sx = x - dx.round() as i32;
            let mut sy = y - dy.round() as i32;
            if wrap {
                sx = sx.rem_euclid(w as i32);
                sy = sy.rem_euclid(h as i32);
            } else {
                sx = sx.clamp(0, w as i32 - 1);
                sy = sy.clamp(0, h as i32 - 1);
            }
            let d = ((y as usize) * w as usize + x as usize) * 4;
            let s = (sy as usize * w as usize + sx as usize) * 4;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    (out, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_graph::{Domain, Endpoint};

    fn gen_with(graph: &mut Graph, kinds: &[&str]) -> (NodeId, Vec<NodeId>) {
        let gen = graph.add_node("gen", "gen.turing", [0.0, 0.0]);
        let mut prev = gen.clone();
        let mut fx = Vec::new();
        for kind in kinds {
            let id = graph.add_node("fx", kind, [1.0, 0.0]);
            graph.add_edge(Endpoint::new(&prev, "image"), Endpoint::new(&id, "image"));
            prev = id.clone();
            fx.push(id);
        }
        (gen, fx)
    }

    fn set_fixed(g: &mut Graph, id: &str, v: f32) {
        g.node_mut(id).unwrap().config = metal_graph::Config::from_json_str(&format!(r#"{{"fixed":{v}}}"#)).unwrap();
    }

    #[test]
    fn a_chain_with_no_speed_leaves_the_multiplier_at_one() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, _) = gen_with(&mut g, &["fx.shake"]);
        assert_eq!(speed_of(&g, &gen), 1.0);
    }

    #[test]
    fn two_speed_nodes_multiply() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, fx) = gen_with(&mut g, &[FX_SPEED_KIND, "fx.shake", FX_SPEED_KIND]);
        set_fixed(&mut g, &fx[0], 2.0);
        set_fixed(&mut g, &fx[2], 3.0);
        assert_eq!(speed_of(&g, &gen), 6.0);
    }

    #[test]
    fn the_accumulated_clock_never_jumps_across_a_speed_change() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, fx) = gen_with(&mut g, &[FX_SPEED_KIND]);
        set_fixed(&mut g, &fx[0], 2.0);
        let mut clocks = Clocks::default();

        // First frame: the clock starts at the show clock.
        let t0 = clocks.advance("dev", &g, 1.0);
        assert_eq!(t0[&gen], 1.0, "a device seen for the first time starts at now");

        // Half a second later at speed 2 the clock advanced by one, to 2.0 —
        // and the picture's own value is continuous, not `time * speed`.
        let t1 = clocks.advance("dev", &g, 1.5);
        assert!((t1[&gen] - 2.0).abs() < 1e-6, "dt × speed: {}", t1[&gen]);

        // Now change the speed. The very next frame must **not** move the
        // clock to `time * 4`; it continues from 2.0 at the new rate.
        set_fixed(&mut g, &fx[0], 4.0);
        let t2 = clocks.advance("dev", &g, 2.0);
        assert!((t2[&gen] - 4.0).abs() < 1e-6, "half a second at speed 4 from 2.0: {}", t2[&gen]);
        // The jump this replaces would have been 2.0 → 8.0 in one frame.
        assert!(t2[&gen] < 5.0, "a speed change must not jump the phase");
    }

    #[test]
    fn a_generator_with_no_speed_follows_the_show_clock() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, _) = gen_with(&mut g, &["fx.shake"]);
        let mut clocks = Clocks::default();
        clocks.advance("dev", &g, 1.0);
        let t = clocks.advance("dev", &g, 7.25);
        assert_eq!(t[&gen], 7.25, "no speed node: drawn at the show clock");
    }

    #[test]
    fn a_long_gap_is_capped_so_a_clock_cannot_leap() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (gen, fx) = gen_with(&mut g, &[FX_SPEED_KIND]);
        set_fixed(&mut g, &fx[0], 8.0);
        let mut clocks = Clocks::default();
        clocks.advance("dev", &g, 1.0);
        // 100 s later: without the cap this would advance by 800 s.
        let t = clocks.advance("dev", &g, 101.0);
        assert!((t[&gen] - (1.0 + 8.0)).abs() < 1e-6, "dt capped at 1 s: {}", t[&gen]);
    }

    #[test]
    fn move_ingest_reads_the_config_with_defaults_and_clamps() {
        let mut n = metal_graph::Node::new("fx-1", FX_MOVE_KIND, [0.0, 0.0]);
        assert_eq!(move_ingest(&n), [0.0, 0.0, 0.0, 0.0, 0.0]);
        n.config = metal_graph::Config::from_json_str(r#"{"x":0.5,"y":-0.25,"vx":2.0,"vy":0.1,"wrap":1}"#).unwrap();
        let got = move_ingest(&n);
        assert_eq!(got, [0.5, -0.25, 1.0, 0.1, 1.0], "vx clamps to its -1..1 range, wrap reads as 1");
    }

    #[test]
    fn move_node_is_found_by_kind() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let (_, fx) = gen_with(&mut g, &["fx.shake", FX_MOVE_KIND]);
        let node = g.node(&fx[1]).unwrap().clone();
        assert!(move_node(&g, &fx[1]).is_some());
        assert!(move_node(&g, &fx[0]).is_none());
        let _ = node;
    }

    #[test]
    fn the_stand_in_translate_offsets_the_picture() {
        // A 3x1 frame: red, green, blue. Moving by half the width clamps the
        // left column and pulls the rest one pixel.
        let frame = (
            vec![
                255, 0, 0, 255, // 0,0 red
                0, 255, 0, 255, // 1,0 green
                0, 0, 255, 255, // 2,0 blue
            ],
            3,
            1,
        );
        // x = +0.33 of the frame ≈ 1 pixel to the left of the sample point.
        let (out, w, h) = translate(&frame, [0.34, 0.0, 0.0, 0.0, 0.0], 0.0);
        assert_eq!((w, h), (3, 1));
        assert_eq!(&out[0..4], &[255, 0, 0, 255], "the left edge holds (clamp)");
        assert_eq!(&out[4..8], &[255, 0, 0, 255]);
        assert_eq!(&out[8..12], &[0, 255, 0, 255]);
    }

    #[test]
    fn wrap_tiles_instead_of_smearing_the_edge() {
        let frame = (
            vec![
                255, 0, 0, 255, // red
                0, 255, 0, 255, // green
                0, 0, 255, 255, // blue
            ],
            3,
            1,
        );
        let (out, _, _) = translate(&frame, [0.34, 0.0, 0.0, 0.0, 1.0], 0.0);
        assert_eq!(&out[0..4], &[0, 0, 255, 255], "the wrapped pixel comes from the far edge");
    }
}
