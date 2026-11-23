//! M1 of the show-control lote (plan "Value sources"): the runtime of the
//! `source.*` value nodes — pure CPU arithmetic on the clock, no state, no
//! `rand`.
//!
//! The catalog, the float tables and the reading of a held value live in
//! `metal_graph::source`; this module only computes each node's output row.
//! The rows are produced by [`crate::script::evaluate_clocked`] alongside the
//! native script presets and read by a `param:<name>` binding the same way a
//! `script.fader`'s output is — one row per node, `out0..`.
//!
//! **Deterministic.** Every kind is a function of `(clock, config)` alone, so
//! the same tick gives the same value on a repeated call, and the noise
//! (`source.random`) is the pure `mix32`/`unit` pair on
//! `(seed, floor(t·rate), channel)` — never a source of real entropy.

use std::collections::HashMap;

use metal_graph::Node;

use crate::bind::Rows;

/// Whether `kind` is a `source.*` value node.
pub fn is_source_kind(kind: &str) -> bool {
    metal_graph::source::is_source_kind(kind)
}

/// The waveform `name` at phase `p` (cycles, any real): `sine` (the default),
/// `square`, `triangle` or `saw`, each returning `-1..1`.
pub fn wave(name: &str, p: f32) -> f32 {
    let p = if p.is_finite() { p - p.floor() } else { 0.0 };
    match name {
        "square" => {
            if p < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        "triangle" => 1.0 - 4.0 * (p - 0.5).abs(),
        "saw" => 2.0 * p - 1.0,
        _ => (std::f32::consts::TAU * p).sin(),
    }
}

/// A 32-bit integer hash — the one `mix32` every `source.random` value is
/// built from. A deterministic bijection-ish scramble, no state.
pub fn mix32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// The hash `x` as a float in `[0, 1)`.
pub fn unit(x: u32) -> f32 {
    (x >> 8) as f32 / 16_777_216.0
}

/// One noise sample in `[0, 1)` for `(seed, step, channel)`, uniform or
/// triangular (`dist` rounds to 1).
fn sample01(seed: u32, step: i64, channel: i64, distance: f32) -> f32 {
    let mixed = seed ^ mix32(step as u32) ^ mix32((channel as u32).wrapping_mul(0x9e37_79b9));
    let h = mix32(mixed);
    let u = unit(h);
    if distance.round() >= 1.0 {
        (u + unit(mix32(h))) * 0.5
    } else {
        u
    }
}

/// The oscillator's row: one float, `wave(cycles + phase + jitter) * amp + offset`.
/// `sync` 0 follows the show clock at `freq` Hz; `1..=8` locks the phase to a
/// note value of the beat clock ([`metal_graph::SCRIPT_BEAT_DIVISIONS`]).
fn osc_row(node: &Node, beats: f64, time: f32) -> Vec<f32> {
    let name = node
        .config
        .get("wave")
        .and_then(|v| v.as_str())
        .unwrap_or("sine");
    let freq = metal_graph::source::held(node, "freq");
    let phase = metal_graph::source::held(node, "phase");
    let amp = metal_graph::source::held(node, "amp");
    let offset = metal_graph::source::held(node, "offset");
    let sync = metal_graph::source::held(node, "sync");
    let seed = metal_graph::source::held(node, "seed");
    let cycles = if sync.round() >= 1.0 {
        let divisions = metal_graph::SCRIPT_BEAT_DIVISIONS;
        let i = (sync.round() as usize - 1).min(divisions.len().saturating_sub(1));
        divisions
            .get(i)
            .map(|d| beats as f32 / d.2 as f32)
            .unwrap_or(0.0)
    } else {
        time * freq
    };
    let jitter = unit(mix32(seed.to_bits()));
    vec![wave(name, cycles + phase + jitter) * amp + offset]
}

/// The constant's row: one float, a named constant when the config names one,
/// else its own held `value`.
fn const_row(node: &Node) -> Vec<f32> {
    let named = node
        .config
        .get("constant")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let v = metal_graph::source::named_constant(named)
        .unwrap_or_else(|| metal_graph::source::held(node, "value"));
    vec![v]
}

/// The noise row: `per` floats, each a deterministic sample of
/// `(seed, floor(t·rate), channel)` mapped into `min..max`. `rate` 0 holds;
/// `smooth` crossfades across the step; `distribution` picks uniform (0) or
/// triangular (1).
fn random_row(node: &Node, time: f32) -> Vec<f32> {
    let seed = metal_graph::source::held(node, "seed");
    let rate = metal_graph::source::held(node, "rate");
    let smooth = metal_graph::source::held(node, "smooth").clamp(0.0, 0.99);
    let lo = metal_graph::source::held(node, "min");
    let hi = metal_graph::source::held(node, "max");
    let distance = metal_graph::source::held(node, "distribution");
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    let held = rate <= 0.0;
    let scaled = time * rate;
    let step = if held || !scaled.is_finite() {
        0
    } else {
        scaled.floor() as i64
    };
    let frac = if held || !scaled.is_finite() {
        0.0
    } else {
        scaled.fract().abs()
    };
    let seed_bits = seed.to_bits();
    (0..metal_graph::source::output_count(node))
        .map(|c| {
            let cur = sample01(seed_bits, step, c as i64, distance);
            let v = if smooth > 0.0 && !held {
                let prev = sample01(seed_bits, step - 1, c as i64, distance);
                prev + (cur - prev) * (smooth * frac)
            } else {
                cur
            };
            lo + v * (hi - lo)
        })
        .collect()
}

/// The lattice hash `shaders/shared.wgsl`'s `lattice3` uses: a biased integer
/// mix with no `sin` in it, so it never goes flat at large coordinates. The
/// one hash every `source.noise` value is built from — deliberately the same
/// constants as the shader, so a CPU value and a GPU field agree in character.
pub fn lattice(p: [i32; 3]) -> f32 {
    let x = (p[0] + 65_536) as u32;
    let y = (p[1] + 65_536) as u32;
    let z = (p[2] + 65_536) as u32;
    let mut n = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263))
        .wrapping_add(z.wrapping_mul(2_246_822_519));
    n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
    n ^= n >> 16;
    (n & 0x00FF_FFFF) as f32 / 16_777_216.0
}

/// The quintic fade `6t⁵ - 15t⁴ + 10t³` — C2 at the cell boundary, the same
/// one `quintic_noise` interpolates with.
fn quintic(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Value noise in `[0, 1)`: the eight lattice corners around `p` hashed with
/// [`lattice`] and interpolated quintically, the 3-D twin of the shader's
/// `quintic_noise`. Continuous and bounded; no state, no `rand`.
pub fn noise3(p: [f32; 3]) -> f32 {
    let i = [p[0].floor(), p[1].floor(), p[2].floor()];
    let f = [p[0] - i[0], p[1] - i[1], p[2] - i[2]];
    let w = [quintic(f[0]), quintic(f[1]), quintic(f[2])];
    let ix = i[0] as i32;
    let iy = i[1] as i32;
    let iz = i[2] as i32;
    let c = |dx, dy, dz| lattice([ix + dx, iy + dy, iz + dz]);
    let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = mix(c(0, 0, 0), c(1, 0, 0), w[0]);
    let x10 = mix(c(0, 1, 0), c(1, 1, 0), w[0]);
    let x01 = mix(c(0, 0, 1), c(1, 0, 1), w[0]);
    let x11 = mix(c(0, 1, 1), c(1, 1, 1), w[0]);
    let y0 = mix(x00, x10, w[1]);
    let y1 = mix(x01, x11, w[1]);
    mix(y0, y1, w[2])
}

/// [`noise3`] summed over `octaves` octaves, the twin of the shader's
/// `lattice_fbm`. `lacunarity` steps the frequency and `gain` the amplitude
/// each octave; the total is **normalised by the weights**, so the result is
/// in `[0, 1)` for every `gain` (a fixed `0.5` gain would let a larger one run
/// past the bound).
pub fn lattice_fbm(p: [f32; 3], octaves: usize, lacunarity: f32, gain: f32) -> f32 {
    let mut total = 0.0;
    let mut norm = 0.0;
    let mut amp = 1.0;
    let mut freq = 1.0;
    for _ in 0..octaves.max(1) {
        total += noise3([p[0] * freq, p[1] * freq, p[2] * freq]) * amp;
        norm += amp;
        amp *= gain;
        freq *= lacunarity;
    }
    if norm > 0.0 {
        total / norm
    } else {
        0.0
    }
}

/// The value-noise row: one float in `offset ± amp`, a quintic fbm sampled
/// along `axis` (`t`/`x`/`y`/`z`) at `rate` cycles/s and seeded by `seed`.
/// `rate` 0 holds the field: the coordinate stops, the value stays.
fn noise_row(node: &Node, time: f32) -> Vec<f32> {
    let seed = metal_graph::source::held(node, "seed");
    let rate = metal_graph::source::held(node, "rate");
    let amp = metal_graph::source::held(node, "amp");
    let octaves = metal_graph::source::held(node, "octaves")
        .round()
        .clamp(1.0, 8.0) as usize;
    let lacunarity = metal_graph::source::held(node, "lacunarity").clamp(1.0, 8.0);
    let gain = metal_graph::source::held(node, "gain").clamp(0.0, 1.0);
    let axis = metal_graph::source::held(node, "axis")
        .round()
        .clamp(0.0, 3.0) as i64;
    let offset = metal_graph::source::held(node, "offset");
    let bits = seed.to_bits();
    // The seed fixes the two lattice coordinates the clock does not walk and a
    // phase (so two seeds at the same tick read different fields, and the same
    // seed at the same tick reads the same one).
    let b0 = unit(mix32(bits ^ 0x9e37_79b9)) * 8.0;
    let b1 = unit(mix32(bits ^ 0x85eb_ca6b)) * 8.0;
    let c = if rate == 0.0 || !rate.is_finite() {
        unit(mix32(bits ^ 0xc2b2_ae35)) * 8.0
    } else {
        time * rate + unit(mix32(bits ^ 0xc2b2_ae35)) * 8.0
    };
    let p = match axis {
        1 => [b0, c, b1],
        2 => [b0, b1, c],
        3 => [b1, c, b0],
        _ => [c, b0, b1],
    };
    let v = lattice_fbm(p, octaves, lacunarity, gain) * 2.0 - 1.0;
    vec![offset + v * amp]
}

/// One `source.*` node's output row, or `None` for any other kind.
pub fn evaluate_node(node: &Node, beats: f64, time: f32) -> Option<Vec<f32>> {
    Some(match node.kind.as_str() {
        metal_graph::source::OSC_KIND => osc_row(node, beats, time),
        metal_graph::source::CONST_KIND => const_row(node),
        metal_graph::source::RANDOM_KIND => random_row(node, time),
        metal_graph::source::NOISE_KIND => noise_row(node, time),
        _ => return None,
    })
}

/// Every `source.*` node's row this frame, by node id — the map the binding
/// pass reads (`crate::bind::var_value`'s live-row path).
pub fn evaluate(graph: &metal_graph::Graph, beats: f64, time: f32) -> Rows {
    let mut out = HashMap::new();
    for node in &graph.nodes {
        if node.bypass || !is_source_kind(&node.kind) {
            continue;
        }
        if let Some(row) = evaluate_node(node, beats, time) {
            out.insert(node.id.clone(), row);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_graph::Config;

    fn node(kind: &str, config: &str) -> Node {
        let mut n = Node::new("n", kind, [0.0, 0.0]);
        n.config = Config::from_json_str(config).unwrap();
        n
    }

    /// The four waveforms are the shapes their names promise, in `-1..1`.
    #[test]
    fn source_osc_waves_are_the_shapes_their_names_promise() {
        assert!((wave("sine", 0.0)).abs() < 1e-6);
        assert!((wave("sine", 0.25) - 1.0).abs() < 1e-6);
        assert_eq!(wave("square", 0.25), 1.0);
        assert_eq!(wave("square", 0.75), -1.0);
        assert!((wave("triangle", 0.5) - 1.0).abs() < 1e-6);
        assert!((wave("triangle", 0.0) + 1.0).abs() < 1e-6);
        assert!((wave("saw", 0.0) + 1.0).abs() < 1e-6);
        assert!((wave("saw", 0.5)).abs() < 1e-6);
        for p in 0..100 {
            for name in ["sine", "square", "triangle", "saw"] {
                let v = wave(name, p as f32 / 100.0);
                assert!((-1.0..=1.0).contains(&v), "{name} out of range at {p}");
            }
        }
    }

    /// The oscillator's range is `offset ± amp`; at phase 0 a sine rests at
    /// `offset`, and it swings by `amp`.
    #[test]
    fn source_osc_range_and_phase_follow_its_config() {
        let n = node(
            metal_graph::source::OSC_KIND,
            "{\"freq\": 3.0, \"phase\": 0.0, \"amp\": 2.0, \"offset\": 5.0}",
        );
        let at0 = evaluate_node(&n, 0.0, 0.0).unwrap();
        assert_eq!(at0.len(), 1);
        assert!(
            (at0[0] - 5.0).abs() < 1e-6,
            "a sine at phase 0 rests at its offset"
        );
        let (lo, hi) = metal_graph::source::output_range(&n);
        assert_eq!((lo, hi), (3.0, 7.0));
        // A quarter cycle later the sine peaks at offset + amp.
        let at_quarter = evaluate_node(&n, 0.0, 0.25 / 3.0).unwrap();
        assert!((at_quarter[0] - 7.0).abs() < 1e-6, "peaks at offset + amp");
    }

    /// Same tick, same value: the reader is pure.
    #[test]
    fn source_reads_the_same_value_at_the_same_tick() {
        let n = node(
            metal_graph::source::OSC_KIND,
            "{\"freq\": 1.7, \"phase\": 0.3, \"seed\": 4.0}",
        );
        assert_eq!(evaluate_node(&n, 12.5, 3.25), evaluate_node(&n, 12.5, 3.25));
        let r = node(
            metal_graph::source::RANDOM_KIND,
            "{\"seed\": 7.0, \"rate\": 6.0, \"per\": 3}",
        );
        assert_eq!(evaluate_node(&r, 0.0, 2.4), evaluate_node(&r, 0.0, 2.4));
    }

    /// `rate` 0 holds the noise steady through time.
    #[test]
    fn source_random_holds_when_rate_is_zero() {
        let n = node(
            metal_graph::source::RANDOM_KIND,
            "{\"seed\": 3.0, \"rate\": 0.0, \"per\": 2}",
        );
        let a = evaluate_node(&n, 0.0, 0.0).unwrap();
        let b = evaluate_node(&n, 0.0, 100.0).unwrap();
        assert_eq!(a, b, "rate 0 is held");
        assert_eq!(a.len(), 2, "one value per channel");
    }

    /// Every noise value stays within `min..max`, for uniform and triangular.
    #[test]
    fn source_random_stays_within_its_bounds() {
        for distance in ["0.0", "1.0"] {
            let n = node(
                metal_graph::source::RANDOM_KIND,
                &format!("{{\"seed\": 11.0, \"rate\": 3.0, \"min\": -2.0, \"max\": 5.0, \"distribution\": {distance}, \"per\": 4}}"),
            );
            let (lo, hi) = metal_graph::source::output_range(&n);
            assert_eq!((lo, hi), (-2.0, 5.0));
            for k in 0..200 {
                let row = evaluate_node(&n, 0.0, k as f32 * 0.13).unwrap();
                assert_eq!(row.len(), 4);
                for v in row {
                    assert!(
                        (lo..=hi).contains(&v),
                        "sample {v} outside {lo}..{hi} (dist {distance})"
                    );
                }
            }
        }
    }

    /// A named constant replaces the node's own `value`.
    #[test]
    fn source_const_selects_a_named_constant() {
        let pi = node(
            metal_graph::source::CONST_KIND,
            "{\"constant\": \"pi\", \"value\": 0.5}",
        );
        assert!((evaluate_node(&pi, 0.0, 0.0).unwrap()[0] - std::f32::consts::PI).abs() < 1e-6);
        let own = node(metal_graph::source::CONST_KIND, "{\"value\": 0.25}");
        assert_eq!(evaluate_node(&own, 0.0, 0.0).unwrap(), vec![0.25]);
    }

    /// The graph reader collects one row per `source.*` node and ignores the
    /// rest.
    #[test]
    fn source_evaluate_reads_every_source_node() {
        let mut g = metal_graph::Graph::new("g", "t", metal_graph::Domain::Live);
        let osc = g.add_node("osc", metal_graph::source::OSC_KIND, [0.0, 0.0]);
        g.add_node("gen", "gen.turing", [1.0, 0.0]);
        let rows = evaluate(&g, 1.0, 0.5);
        assert_eq!(rows.len(), 1);
        assert!(rows.contains_key(&osc));
    }

    /// M2: the lattice hash is the shader's — biased, bounded in `[0, 1)` and
    /// stable at large coordinates (the reason it exists).
    #[test]
    fn source_noise_lattice_matches_the_shader_contract() {
        for p in [[0, 0, 0], [1, 2, 3], [-40, 7, 9_000], [-65_536, 65_535, 12]] {
            let v = lattice(p);
            assert!((0.0..1.0).contains(&v), "lattice({p:?}) = {v} out of [0,1)");
            assert_eq!(v, lattice(p), "the hash is a function");
        }
        // The quintic fade meets the lattice corners exactly, so the field is
        // continuous across a cell boundary.
        assert_eq!(noise3([0.0, 0.0, 0.0]), lattice([0, 0, 0]));
        let left = noise3([0.999_9, 0.0, 0.0]);
        let right = lattice([1, 0, 0]);
        assert!((left - right).abs() < 1e-3, "no seam at the corner");
    }

    /// M2: same tick, same value; `rate` 0 holds the field steady.
    #[test]
    fn source_noise_is_deterministic_and_held_when_rate_is_zero() {
        let n = node(
            metal_graph::source::NOISE_KIND,
            "{\"seed\": 9.0, \"rate\": 3.0, \"octaves\": 4}",
        );
        assert_eq!(evaluate_node(&n, 0.0, 2.5), evaluate_node(&n, 0.0, 2.5));
        let held = node(
            metal_graph::source::NOISE_KIND,
            "{\"seed\": 9.0, \"rate\": 0.0, \"octaves\": 5}",
        );
        let a = evaluate_node(&held, 0.0, 0.0).unwrap();
        let b = evaluate_node(&held, 0.0, 500.0).unwrap();
        assert_eq!(a, b, "rate 0 is held");
        assert_eq!(a.len(), 1, "one value, like the oscillator");
    }

    /// M2: every noise value stays within `offset ± amp`, for every axis, and
    /// the field moves smoothly (bounded derivative) — no jump between ticks.
    #[test]
    fn source_noise_stays_within_its_bounds_and_moves_smoothly() {
        for axis in 0..4 {
            let n = node(
                metal_graph::source::NOISE_KIND,
                &format!("{{\"seed\": 12.0, \"rate\": 4.0, \"amp\": 2.0, \"offset\": 1.0, \"octaves\": 6, \"axis\": {axis}}}"),
            );
            let (lo, hi) = metal_graph::source::output_range(&n);
            assert_eq!((lo, hi), (-1.0, 3.0), "offset ± amp");
            let mut prev = evaluate_node(&n, 0.0, 0.0).unwrap()[0];
            for k in 1..800 {
                let t = k as f32 * 0.01;
                let v = evaluate_node(&n, 0.0, t).unwrap()[0];
                assert!(
                    (lo..=hi).contains(&v),
                    "axis {axis}: {v} outside {lo}..{hi}"
                );
                // A C2 field: a 10 ms step moves by a bounded amount, never a
                // cell-boundary jump. The bound scales with `rate·dt`.
                assert!((v - prev).abs() < 0.4, "axis {axis}: jump {prev} -> {v}");
                prev = v;
            }
        }
    }

    /// M2: `octaves` is the fbm depth — `gain` 0 collapses it to the first
    /// octave, and more octaves add detail that changes the value.
    #[test]
    fn source_noise_octave_count_is_the_fbm_depth() {
        let p = [0.3, 1.7, 4.1];
        assert!(
            (lattice_fbm(p, 5, 2.03, 0.0) - noise3(p)).abs() < 1e-6,
            "gain 0 = one octave"
        );
        let one = lattice_fbm(p, 1, 2.03, 0.5);
        let eight = lattice_fbm(p, 8, 2.03, 0.5);
        assert!((one - eight).abs() > 1e-4, "more octaves add detail");
        for octaves in 1..=8 {
            let v = lattice_fbm(p, octaves, 2.03, 0.5);
            assert!((0.0..1.0).contains(&v), "{octaves} octaves stay in [0,1)");
        }
    }
}
