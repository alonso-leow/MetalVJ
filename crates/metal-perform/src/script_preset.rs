//! Native Rust script presets: `script.beat.<division>` and `script.fader`.
//!
//! Same node family and ports as the interpreted scripts, but evaluated
//! in-process every frame from the musical clock — no subprocess, no code.
//! The kinds and their declared floats live in `metal_graph`
//! (`script_preset_floats`); this module only computes the output row.
//!
//! **Beat presets.** One cycle lasts a note value (1/16 … 8 bars of 4/4).
//! `fall = 1` jumps to `high` on the cycle start and eases out to `low`
//! (a flash on the beat); `fall = 0` rises from `low` to `high` and snaps back.
//! `ease` is the exponent of the curve. `bpm = 0` follows the live tempo
//! (`State::music_pos`); `bpm > 0` is a set point, run off the wall clock.
//!
//! **Fader.** `low + (high - low) * value`; `value` is the live knob, so a
//! slider, a bound float or a `floats` cable moves it.

use crate::script::ScriptConfig;

/// What the engine knows about time this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeatClock {
    /// Musical position in beats (quarter notes), from `State::music_pos`.
    pub beats: f64,
    /// Seconds on the show clock, for a fixed `bpm` set point.
    pub time: f32,
    /// The clock's current tempo (follows the tracker / a changed BPM).
    pub bpm: f32,
}

impl Default for BeatClock {
    fn default() -> Self {
        BeatClock { beats: 0.0, time: 0.0, bpm: crate::clock::FALLBACK_BPM }
    }
}

pub fn is_preset(kind: &str) -> bool {
    metal_graph::script_preset_floats(kind).is_some()
}

/// The seed `Node.config` for a preset kind (`{code: "", floats: [...]}`).
pub fn seed_config(kind: &str) -> Option<ScriptConfig> {
    let floats = metal_graph::script_preset_floats(kind)?;
    Some(ScriptConfig {
        code: String::new(),
        floats: floats
            .iter()
            .map(|&(name, min, max, default)| crate::script::ScriptFloat {
                name: name.into(),
                min,
                max,
                default,
                value: None,
            })
            .collect(),
    })
}

/// Output row for `kind` given its resolved float `values` (declaration order).
pub fn evaluate(kind: &str, values: &[f32], clock: BeatClock) -> Option<Vec<f32>> {
    if kind == metal_graph::SCRIPT_FADER_KIND {
        let [value, low, high] = values else { return None };
        return Some(vec![low + (high - low) * value]);
    }
    let id = kind.strip_prefix(metal_graph::SCRIPT_BEAT_PREFIX)?;
    let bars = metal_graph::SCRIPT_BEAT_DIVISIONS.iter().find(|d| d.0 == id)?.2;
    let [low, high, ease, fall, phase, bpm] = values else { return None };
    let beats = if *bpm > 0.0 { clock.time as f64 * *bpm as f64 / 60.0 } else { clock.beats };
    let p = (beats / (bars * 4.0) + *phase as f64).rem_euclid(1.0) as f32;
    let left = (1.0 - p).powf(*ease);
    let env = if *fall >= 0.5 { left } else { 1.0 - left };
    Some(vec![low + (high - low) * env])
}

/// `ctl.math` (parity P-A, CHOP Math): every value of the `control` row in,
/// remapped, out — `in_low..in_high` → `0..1`, invert, curve (`v^curve`),
/// quantize to `steps` (0 = off), smooth (a one-pole lag, `smooth` the share
/// of last frame's value kept per 1/60 s), then to `out_low..out_high`. An
/// unwired `control` is one value, 0. The lag's memory is per node, so two
/// `ctl.math` nodes never share a history.
pub fn ctl_math(node_id: &str, values: &[f32], row: &[f32], time: f32) -> Vec<f32> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static LAG: Mutex<Option<HashMap<String, (f32, Vec<f32>)>>> = Mutex::new(None);
    let get = |i: usize, d: f32| values.get(i).copied().unwrap_or(d);
    let (in_lo, in_hi, out_lo, out_hi) = (get(0, 0.0), get(1, 1.0), get(2, 0.0), get(3, 1.0));
    let (curve, smooth, steps, invert) = (get(4, 1.0).max(1e-3), get(5, 0.0).clamp(0.0, 0.99), get(6, 0.0), get(7, 0.0));
    let input: Vec<f32> = if row.is_empty() { vec![0.0] } else { row.to_vec() };
    let shaped: Vec<f32> = input
        .iter()
        .map(|&x| {
            let span = in_hi - in_lo;
            let mut v = if span.abs() < 1e-6 { 0.0 } else { ((x - in_lo) / span).clamp(0.0, 1.0) };
            if invert >= 0.5 {
                v = 1.0 - v;
            }
            v = v.powf(curve);
            let n = steps.round();
            if n >= 1.0 {
                v = (v * n).round() / n;
            }
            v
        })
        .collect();
    let mut lag = LAG.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let lag = lag.get_or_insert_with(HashMap::new);
    let smoothed: Vec<f32> = match lag.get(node_id) {
        Some((t0, prev)) if smooth > 0.0 && prev.len() == shaped.len() && time >= *t0 => {
            let keep = smooth.powf((time - t0) * 60.0);
            shaped.iter().zip(prev).map(|(v, p)| p + (v - p) * (1.0 - keep)).collect()
        }
        _ => shaped,
    };
    lag.insert(node_id.to_string(), (time, smoothed.clone()));
    smoothed.iter().map(|v| out_lo + (out_hi - out_lo) * v).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctl_math_remaps_inverts_curves_quantizes_and_lags() {
        let seed = seed_config(metal_graph::CTL_MATH_KIND).expect("ctl.math is seeded like a preset").values();
        assert_eq!(ctl_math("a", &seed, &[0.0, 0.25, 1.0], 0.0), vec![0.0, 0.25, 1.0], "defaults: identity");
        assert_eq!(ctl_math("b", &seed, &[], 0.0), vec![0.0], "unwired: one zero");
        let mut v = seed.clone();
        v[0] = 0.5; // in_low
        v[3] = 0.5; // out_high
        v[7] = 1.0; // invert
        assert_eq!(ctl_math("c", &v, &[0.25, 1.0], 0.0), vec![0.5, 0.0]);
        let mut q = seed.clone();
        q[6] = 2.0; // steps
        q[4] = 2.0; // curve
        assert_eq!(ctl_math("d", &q, &[0.8], 0.0), vec![0.5], "0.8² = 0.64 → nearest half");
        let mut s = seed.clone();
        s[5] = 0.9;
        assert_eq!(ctl_math("e", &s, &[0.0], 0.0), vec![0.0]);
        let one = ctl_math("e", &s, &[1.0], 1.0 / 60.0)[0];
        assert!((one - 0.1).abs() < 1e-4, "a lag moves a tenth per frame, got {one}");
    }

    fn beat(id: &str, beats: f64, over: &[(usize, f32)]) -> f32 {
        let kind = format!("{}{id}", metal_graph::SCRIPT_BEAT_PREFIX);
        let mut v = seed_config(&kind).unwrap().values();
        for &(i, x) in over {
            v[i] = x;
        }
        evaluate(&kind, &v, BeatClock { beats, ..BeatClock::default() }).unwrap()[0]
    }

    #[test]
    fn every_catalog_preset_has_a_seed_and_an_output() {
        for (id, _, _) in metal_graph::SCRIPT_BEAT_DIVISIONS {
            assert!(beat(id, 0.0, &[]) > 0.99, "{id} starts at high on the cycle start");
        }
        assert!(is_preset(metal_graph::SCRIPT_FADER_KIND));
        assert!(!is_preset("script.py"));
    }

    #[test]
    fn a_quarter_note_cycle_is_one_beat_and_eases_out() {
        let mid = beat("1_4", 0.5, &[]);
        assert!(mid < 0.2 && mid > 0.0, "ease-out 3 at half a beat is 0.125, got {mid}");
        assert!(beat("1_4", 1.0, &[]) > 0.99, "next beat restarts");
        assert!(beat("1_4", 0.25, &[]) < beat("1_4", 0.0, &[]));
    }

    #[test]
    fn division_one_is_a_bar_and_eight_is_eight_bars() {
        assert!(beat("1", 3.99, &[]) < 0.01);
        assert!(beat("1", 4.0, &[]) > 0.99);
        assert!(beat("8", 31.9, &[]) < 0.01);
    }

    #[test]
    fn rise_mode_and_range() {
        let v = beat("1_4", 0.0, &[(3, 0.0), (0, 0.2), (1, 0.8)]);
        assert!((v - 0.2).abs() < 1e-5, "rise starts at low, got {v}");
        let v = beat("1_4", 0.99, &[(3, 0.0), (0, 0.2), (1, 0.8)]);
        assert!(v > 0.79, "rise ends at high, got {v}");
    }

    #[test]
    fn a_set_bpm_runs_off_the_show_clock() {
        let kind = "script.beat.1_4";
        let mut v = seed_config(kind).unwrap().values();
        v[5] = 120.0;
        let a = evaluate(kind, &v, BeatClock { beats: 99.3, time: 0.5, ..BeatClock::default() }).unwrap()[0];
        assert!(a > 0.99, "0.5s at 120 bpm is exactly one beat, got {a}");
    }

    #[test]
    fn the_fader_maps_value_into_its_range() {
        let k = metal_graph::SCRIPT_FADER_KIND;
        assert_eq!(evaluate(k, &[0.5, 0.0, 1.0], BeatClock::default()), Some(vec![0.5]));
        assert_eq!(evaluate(k, &[1.0, 0.25, 0.75], BeatClock::default()), Some(vec![0.75]));
    }

    #[test]
    fn a_seedless_preset_node_in_a_graph_still_answers_a_row() {
        let mut g = metal_graph::Graph::new("g", "t", metal_graph::Domain::Live);
        let id = g.add_node("script", "script.beat.1_4", [0.0, 0.0]);
        let out = crate::script::evaluate_clocked(
            &g,
            &Default::default(),
            &[],
            &crate::bind::GenParams::new(),
            BeatClock { beats: 2.0, ..BeatClock::default() },
        );
        assert!(out.rows[&id][0] > 0.99);
        g.node_mut(&id).unwrap().bypass = true;
        let out = crate::script::evaluate_clocked(&g, &Default::default(), &[], &crate::bind::GenParams::new(), BeatClock::default());
        assert!(out.rows.is_empty(), "a bypassed preset (grid switch off) writes nothing");
    }
}
