//! `orbit-swarm`: a swarm of glowing particles orbiting in concentric bands,
//! with Keplerian drift, organic wobble, turbulence, vibration, radial wave
//! kicks, trails, depth sizing and background dust on the procedural scaffold
//! (`shared/scaffold.rs`). Behaviour and control list follow the built-in
//! "particles" tool (sphere/cube orbit + vibration/wave displacement); the WGSL
//! is original (`shaders/orbit_swarm.wgsl`).
//!
//! Own floats (31): `density size speed orbit_radius orbit_ellipse bands
//! spread thickness rotation wobble frequency turbulence swirl vibration
//! vibration_area wave_amp wave_speed trail trail_fade glow core twinkle
//! twinkle_speed depth vignette hue_shift color_mix dust ring_mix shape seed`.
//! Slots: `swarm` (body), `orbit` (trails), `spark` (cores and dust).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("density", 1.0, 100.0, 55.0),
    spec("size", 0.5, 5.0, 1.8),
    spec("speed", -3.0, 3.0, 0.6),
    spec("orbit_radius", 0.08, 0.5, 0.32),
    spec("orbit_ellipse", 0.0, 1.0, 0.3),
    spec_held("bands", 1.0, 12.0, 5.0, 0.2),
    spec("spread", 0.0, 1.0, 0.3),
    spec("thickness", 0.0, 1.0, 0.35),
    spec("rotation", 0.0, 1.0, 0.0),
    spec("wobble", 0.0, 1.0, 0.25),
    spec("frequency", 1.0, 20.0, 6.0),
    spec("turbulence", 0.0, 1.0, 0.3),
    spec("swirl", 0.0, 1.0, 0.3),
    spec("vibration", 0.0, 1.0, 0.3),
    spec("vibration_area", 0.0, 1.0, 0.6),
    spec("wave_amp", 0.0, 1.0, 0.2),
    spec("wave_speed", 0.0, 2.0, 0.5),
    spec("trail", 0.0, 1.0, 0.4),
    spec("trail_fade", 0.0, 1.0, 0.4),
    spec("glow", 0.0, 1.0, 0.5),
    spec("core", 0.0, 1.0, 0.5),
    spec("twinkle", 0.0, 1.0, 0.3),
    spec("twinkle_speed", 0.0, 4.0, 1.5),
    spec("depth", 0.0, 1.0, 0.6),
    spec("vignette", 0.0, 1.0, 0.3),
    spec("hue_shift", 0.0, 1.0, 0.0),
    spec("color_mix", 0.0, 1.0, 0.4),
    spec("dust", 0.0, 1.0, 0.15),
    spec("ring_mix", 0.0, 1.0, 0.75),
    spec_held("shape", 0.0, 2.0, 0.0, 0.2),
    spec_held("seed", 0.0, 50.0, 3.7, 0.2),
];

/// Body: electric blue, cyan, violet, warm white.
const SWARM: [f32; 12] =
    [0.05, 0.55, 1.0, 0.10, 0.90, 0.85, 0.75, 0.25, 1.0, 1.0, 0.95, 0.80];
/// Trail: deep navy to blue.
const ORBIT: [f32; 12] =
    [0.02, 0.08, 0.25, 0.03, 0.22, 0.55, 0.12, 0.06, 0.40, 0.30, 0.18, 0.05];
/// Spark: white, ice, pink, gold.
const SPARK: [f32; 12] =
    [1.0, 1.0, 1.0, 0.65, 0.95, 1.0, 1.0, 0.55, 0.90, 1.0, 0.85, 0.40];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "swarm" => SWARM, "orbit" => ORBIT, "spark" => SPARK);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "orbit-swarm",
    "Orbit Swarm",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/orbit_swarm.wgsl"),
);

crate::scaffold_model!(OrbitSwarm, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(OrbitSwarm::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("density", 70.0),
        ("size", 2.5),
        ("speed", 0.8),
        ("orbit_radius", 0.34),
        ("orbit_ellipse", 0.35),
        ("bands", 4.0),
        ("spread", 0.4),
        ("thickness", 0.5),
        ("rotation", 0.2),
        ("wobble", 0.4),
        ("frequency", 7.0),
        ("turbulence", 0.4),
        ("swirl", 0.4),
        ("vibration", 0.6),
        ("vibration_area", 0.6),
        ("wave_amp", 0.3),
        ("wave_speed", 0.6),
        ("trail", 0.5),
        ("trail_fade", 0.5),
        ("glow", 0.5),
        ("core", 0.5),
        ("twinkle", 0.5),
        ("twinkle_speed", 2.0),
        ("depth", 0.6),
        ("vignette", 0.4),
        ("hue_shift", 0.2),
        ("color_mix", 0.5),
        ("dust", 0.25),
        ("ring_mix", 0.6),
        ("shape", 1.0),
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
            eprintln!("orbit-swarm: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
