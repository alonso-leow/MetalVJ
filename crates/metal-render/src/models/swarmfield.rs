//! `swarmfield`: a dense cloud of small glowing particles with shaped
//! morphing, drifting attractors, trails and shallow depth of field, on the
//! procedural scaffold (`shared/scaffold.rs`). Behaviour and controls follow the
//! built-in "Particles 2.0" tool (density, particle size, focus, depth of
//! field, aberration, motion, four shapes, zoom, rotation speed and direction);
//! the WGSL is original procedural scaffold code, so the stateful 150k-particle
//! simulation is replaced by a time-scrolled hashed site field.
//!
//! Own floats (27): `density size focus dof zoom motion rotation_speed
//! aberration shape direction speed trail swirl scatter attractors
//! attractor_radius gravity turbulence twist spikes morph jitter
//! size_variation glow fill flicker seed`. Slots: `particles`, `warm`
//! (chromatic fringe), `cool` (chromatic fringe and halo).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("density", 0.0, 100.0, 55.0),
    spec("size", 0.0, 100.0, 30.0),
    spec("focus", 0.0, 100.0, 50.0),
    spec("dof", 0.0, 100.0, 45.0),
    spec("swarm_zoom", 0.2, 3.0, 1.0),
    spec("motion", 0.0, 100.0, 45.0),
    spec("rotation_speed", 0.0, 100.0, 15.0),
    spec("aberration", 0.0, 100.0, 25.0),
    spec_held("shape", 0.0, 3.0, 0.0, 0.2),
    spec_held("direction", 0.0, 3.0, 0.0, 0.2),
    spec("speed", 0.0, 100.0, 45.0),
    spec("trail", 0.0, 100.0, 35.0),
    spec("swirl", 0.0, 100.0, 20.0),
    spec("scatter", 0.0, 100.0, 30.0),
    spec_held("attractors", 0.0, 8.0, 5.0, 0.2),
    spec("attractor_radius", 0.02, 0.6, 0.2),
    spec("gravity", 0.0, 100.0, 50.0),
    spec("turbulence", 0.0, 100.0, 40.0),
    spec("twist", 0.0, 100.0, 30.0),
    spec("spikes", 0.0, 100.0, 30.0),
    spec("morph", 0.0, 100.0, 60.0),
    spec("jitter", 0.0, 100.0, 60.0),
    spec("size_variation", 0.0, 100.0, 50.0),
    spec("glow", 0.0, 200.0, 40.0),
    spec("fill", 0.0, 100.0, 30.0),
    spec("flicker", 0.0, 100.0, 15.0),
    spec_held("seed", 0.0, 50.0, 3.0, 0.2),
];

/// Particles: white, pale blue, cream, warm white (linear).
const PARTICLES: [f32; 12] = [1.00, 1.00, 1.00, 0.72, 0.86, 1.00, 1.00, 0.93, 0.78, 1.00, 0.70, 0.42];
/// Warm chromatic fringe: red, orange, amber, pink.
const WARM: [f32; 12] = [1.00, 0.08, 0.04, 1.00, 0.28, 0.04, 1.00, 0.55, 0.10, 1.00, 0.20, 0.35];
/// Cool fringe and halo: deep blue, cyan, teal, pale cyan.
const COOL: [f32; 12] = [0.04, 0.10, 0.60, 0.05, 0.55, 1.00, 0.10, 0.80, 0.85, 0.55, 0.90, 1.00];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "particles" => PARTICLES, "warm" => WARM, "cool" => COOL);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "swarmfield",
    "Swarmfield",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/swarmfield.wgsl"),
);

crate::scaffold_model!(Swarmfield, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Swarmfield::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("shape", 2.0),
        ("direction", 2.0),
        ("morph", 60.0),
        ("rotation_speed", 40.0),
        ("aberration", 40.0),
        ("trail", 40.0),
        ("fill", 40.0),
        ("glow", 60.0),
        ("flicker", 30.0),
        ("attractors", 5.0),
        ("gravity", 60.0),
        ("attractor_radius", 0.25),
        ("twist", 40.0),
        ("spikes", 40.0),
        ("turbulence", 45.0),
        ("swirl", 30.0),
        ("scatter", 40.0),
        ("jitter", 55.0),
        ("size_variation", 55.0),
        ("speed", 55.0),
        ("swarm_zoom", 1.2),
        ("focus", 55.0),
        ("dof", 50.0),
        ("density", 55.0),
        ("size", 40.0),
        ("motion", 50.0),
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
            eprintln!("swarmfield: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
