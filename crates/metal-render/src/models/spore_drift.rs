//! `spore-drift`: a drifting field of coloured spores with an optional web of
//! threads, a bass ring and a soft dark plate. Behaviour and parameter list
//! follow the built-in "Spores" tool (density, dot size and variation, float,
//! float speed, bass/treble layout, ring, connections); the WGSL is original
//! procedural scaffold code, so the particle simulation is replaced by a
//! time-scrolled hashed site field.
//!
//! Own floats (22): `density dot_size size_variation drift speed
//! rotation_speed hit_contrast bass treble ring_intensity ring_shape
//! ring_direction connections connection_mode thread_speed stroke
//! line_opacity aberration layout_mode color_mode looseness seed`. Slots: `spores`, `shade`, `thread`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("density", 0.0, 100.0, 24.0),
    spec("dot_size", 0.0, 100.0, 26.0),
    spec("size_variation", 0.0, 100.0, 100.0),
    spec("mote_drift", 0.0, 100.0, 55.0),
    spec("speed", 0.0, 100.0, 43.0),
    spec("rotation_speed", 0.0, 100.0, 25.0),
    spec("hit_contrast", 0.0, 100.0, 100.0),
    spec("bass", 0.0, 100.0, 100.0),
    spec("treble", 0.0, 100.0, 20.0),
    spec("ring_intensity", 0.0, 100.0, 0.0),
    spec_held("ring_shape", 0.0, 3.0, 0.0, 0.2),
    spec_held("ring_direction", 0.0, 5.0, 0.0, 0.2),
    spec("connections", 0.0, 100.0, 100.0),
    spec_held("connection_mode", 0.0, 1.0, 0.0, 0.1),
    spec("thread_speed", 0.0, 100.0, 35.0),
    spec("stroke", 0.2, 4.0, 1.2),
    spec("line_opacity", 0.0, 100.0, 68.0),
    spec("aberration", 0.0, 100.0, 0.0),
    spec_held("layout_mode", 0.0, 6.0, 1.0, 0.1),
    spec_held("color_mode", 0.0, 1.0, 0.0, 0.1),
    spec("looseness", 0.0, 100.0, 0.0),
    spec_held("seed", 0.0, 50.0, 3.0, 0.2),
];

/// Spores: cream, coral, rust, gold.
const SPORES: [f32; 12] = [1.00, 0.90, 0.72, 0.98, 0.38, 0.24, 0.52, 0.07, 0.05, 1.00, 0.72, 0.30];
/// Plate: deep teal, midnight, pale cyan, near black.
const SHADE: [f32; 12] = [0.02, 0.13, 0.15, 0.01, 0.03, 0.12, 0.45, 0.78, 0.82, 0.01, 0.02, 0.04];
/// Threads and ring: pale blue, white, amber, cyan.
const THREAD: [f32; 12] = [0.55, 0.85, 1.00, 1.00, 1.00, 1.00, 1.00, 0.72, 0.34, 0.18, 0.72, 0.95];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "spores" => SPORES, "shade" => SHADE, "thread" => THREAD);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "spore-drift",
    "Spore Drift",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/spore_drift.wgsl"),
);

crate::scaffold_model!(SporeDrift, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(SporeDrift::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("warp_amount", 0.4),
        ("pulse", 0.0),
        ("ring_intensity", 70.0),
        ("connections", 90.0),
        ("line_opacity", 80.0),
        ("aberration", 60.0),
        ("looseness", 50.0),
        ("color_mode", 0.0),
        ("layout_mode", 1.0),
    ];

    #[test]
    fn standard_checks() {
        testing::check_all(build, BASE);
    }

    #[test]
    fn ms_per_frame() {
        if std::env::var("SCAFFOLD_MEASURE").is_ok() {
            eprintln!("spore-drift: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
