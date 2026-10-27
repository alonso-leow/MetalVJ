//! `phosphor-trace`: a CRT oscilloscope face — graticule, axis and a glowing
//! persistence trace — on the procedural scaffold (`shared/scaffold.rs`). Behaviour and
//! parameter list follow the scraped built-in oscilloscope tool; the WGSL is
//! original (`shaders/phosphor_trace.wgsl`).
//!
//! Own floats (32): figure, mode, blend, decimate, phase, rotation,
//! beam_width, trail, persistence, glow, displacement, ratio_a, ratio_b, spin,
//! dwell, grid, axis, gain, speed, frequency, noise, jitter, warp, size, seed,
//! margin, divisions, bloom, softness, waveform, scan, drift. Slots: `beam`,
//! `graticule`, `afterglow`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("figure", 0.0, 7.0, 1.0, 0.2),
    spec_held("mode", 0.0, 2.0, 2.0, 0.2),
    spec("blend", 0.0, 1.0, 0.6),
    spec("decimate", 1.0, 24.0, 3.0),
    spec("phase", 0.0, 1.0, 0.0),
    spec("rotation", -1.0, 1.0, 0.0),
    spec("beam_width", 0.002, 0.08, 0.018),
    spec("trail", 0.0, 1.0, 0.45),
    spec("persistence", 0.0, 0.96, 0.55),
    spec("glow", 0.0, 1.0, 0.5),
    spec("displacement", 0.0, 1.0, 0.15),
    spec("ratio_a", 1.0, 9.0, 3.0),
    spec("ratio_b", 1.0, 9.0, 2.0),
    spec("spin", -1.0, 1.0, 0.0),
    spec("dwell", 0.0, 1.0, 0.3),
    spec("grid", 0.0, 1.0, 0.6),
    spec("axis", 0.0, 1.0, 0.6),
    spec("gain", 0.0, 2.0, 0.8),
    spec("speed", -3.0, 3.0, 0.5),
    spec("frequency", 0.5, 12.0, 3.0),
    spec("noise", 0.0, 1.0, 0.25),
    spec("jitter", 0.0, 1.0, 0.3),
    spec("warp", 0.0, 1.0, 0.25),
    spec("size", 0.0, 1.0, 0.5),
    spec_held("seed", 0.0, 50.0, 7.0, 0.2),
    spec("margin", 0.02, 0.22, 0.06),
    spec_held("divisions", 2.0, 12.0, 8.0, 0.3),
    spec("bloom", 0.0, 1.0, 0.4),
    spec("softness", 0.0, 1.0, 0.3),
    spec("waveform", 0.0, 1.0, 0.5),
    spec("scan", 0.0, 1.0, 0.25),
    spec("trace_drift", 0.0, 1.0, 0.15),
];

/// Beam: ember green, phosphor green, mint, warm white (linear).
const BEAM: [f32; 12] = [
    0.02, 0.30, 0.08, 0.04, 0.62, 0.30, 0.45, 1.00, 0.62, 1.00, 0.96, 0.78,
];
/// Graticule: deep green, dim sage, pale green, warm highlight.
const GRID: [f32; 12] = [
    0.01, 0.10, 0.04, 0.04, 0.25, 0.12, 0.10, 0.48, 0.22, 0.42, 0.72, 0.44,
];
/// Afterglow: dim green, teal, amber, violet.
const AFTERGLOW: [f32; 12] = [
    0.01, 0.30, 0.10, 0.03, 0.45, 0.46, 0.72, 0.48, 0.12, 0.55, 0.24, 0.70,
];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "beam" => BEAM, "graticule" => GRID, "afterglow" => AFTERGLOW);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "phosphor-trace",
    "Phosphor Trace",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/phosphor_trace.wgsl"),
);

crate::scaffold_model!(PhosphorTrace, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(PhosphorTrace::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("figure", 1.0),
        ("mode", 2.0),
        ("blend", 0.6),
        ("trail", 0.5),
        ("persistence", 0.55),
        ("glow", 0.5),
        ("grid", 0.6),
        ("axis", 0.6),
        ("noise", 0.5),
        ("jitter", 0.4),
        ("warp", 0.3),
        ("bloom", 0.4),
        ("scan", 0.25),
        ("trace_drift", 0.15),
        ("waveform", 0.5),
        ("warp_amount", 0.4),
        ("pulse", 0.0),
    ];

    #[test]
    fn standard_checks() {
        testing::check_all(build, BASE);
    }

    #[test]
    fn ms_per_frame() {
        if std::env::var("SCAFFOLD_MEASURE").is_ok() {
            eprintln!("phosphor-trace: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
