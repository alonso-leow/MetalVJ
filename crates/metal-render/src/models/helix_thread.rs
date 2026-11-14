//! `helix-thread`: a double-helix filament with two counter-phased strands,
//! periodic rungs, twist, weave, drift and glow, on the procedural scaffold
//! (`shared/scaffold.rs`). Behaviour follows the "DNA Filament" sketch tool; the
//! WGSL is original (`shaders/helix_thread.wgsl`).
//!
//! Own floats (16): `mode lines length spread bundle field curl twist weave
//! width drift speed rungs glow taper seed`. Slots: `strand`, `rung`, `glow`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("mode", 0.0, 3.0, 2.0, 0.2),
    spec_held("lines", 4.0, 180.0, 56.0, 0.2),
    spec("length", 10.0, 240.0, 122.0),
    spec("spread", 0.0, 200.0, 74.0),
    spec("bundle", 0.0, 200.0, 128.0),
    spec("field", 0.0, 200.0, 96.0),
    spec("curl", -200.0, 200.0, 38.0),
    spec("twist", -200.0, 200.0, 26.0),
    spec("weave", -200.0, 200.0, 0.0),
    spec("width", 0.5, 20.0, 2.4),
    spec("drift_v", 0.0, 200.0, 42.0),
    spec("speed", 0.0, 260.0, 72.0),
    spec_held("rungs", 4.0, 64.0, 18.0, 0.2),
    spec("glow", 0.0, 1.0, 0.55),
    spec("taper", 0.0, 1.0, 0.35),
    spec_held("seed", 0.0, 50.0, 3.0, 0.2),
];

/// Strands: deep indigo, teal, cream, warm white (linear).
const STRAND: [f32; 12] = [0.015, 0.025, 0.075, 0.02, 0.30, 0.42, 0.80, 0.88, 0.72, 1.0, 0.98, 0.86];
/// Rungs: dark rust, orange, amber, pale gold.
const RUNG: [f32; 12] = [0.10, 0.01, 0.0, 0.95, 0.25, 0.02, 1.0, 0.55, 0.08, 1.0, 0.85, 0.30];
/// Glow: near-black violet, blue, magenta, warm peach.
const GLOW: [f32; 12] = [0.01, 0.01, 0.03, 0.10, 0.35, 0.55, 0.55, 0.20, 0.65, 1.0, 0.72, 0.42];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "strand" => STRAND, "rung" => RUNG, "glow" => GLOW);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "helix-thread",
    "Helix Thread",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/helix_thread.wgsl"),
);

crate::scaffold_model!(HelixThread, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(HelixThread::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("warp_amount", 0.4),
        ("pulse", 0.0),
        ("mode", 2.0),
        ("lines", 56.0),
        ("length", 122.0),
        ("spread", 90.0),
        ("bundle", 128.0),
        ("field", 110.0),
        ("curl", 80.0),
        ("twist", 60.0),
        ("weave", 80.0),
        ("width", 3.5),
        ("drift_v", 70.0),
        ("speed", 90.0),
        ("rungs", 20.0),
        ("glow", 0.6),
        ("taper", 0.4),
        ("seed", 7.0),
    ];

    #[test]
    fn standard_checks() {
        testing::check_all(build, BASE);
    }

    #[test]
    fn ms_per_frame() {
        if std::env::var("SCAFFOLD_MEASURE").is_ok() {
            eprintln!("helix-thread: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
