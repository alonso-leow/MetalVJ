//! `deep-tunnel`: a receding polygonal tunnel with wireframe, gradient,
//! spiral, star-streak, grid and alien-corridor treatments, built on the paper
//! scaffold (`shared/scaffold.rs`). The behaviour and control list follow the
//! built-in "tunnel" tool; the WGSL is an original 2D projection
//! (`shaders/deep_tunnel.wgsl`), not the upstream 3D mesh shader.
//!
//! Own floats (40): `style shape seed rings rays ring_opacity ray_opacity
//! line_width glow speed bend_x bend_y amplitude diffuse shading spiral_arms
//! spiral_twist spiral_band spiral_blend spiral_spin spiral_soft spiral_turb
//! stars streak_len streak_thick spread color_split grid_gap grid_fill
//! grid_density grid_twinkle grid_random strips strip_width walls windows orbs
//! grad_steps grad_repeat grad_stripes`. Slots: `palette`, `line`, `accent`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("style", 0.0, 5.0, 0.0, 0.0),
    spec_held("shape", 0.0, 5.0, 0.0, 0.0),
    spec_held("seed", 0.0, 50.0, 3.0, 0.0),
    spec("rings", 0.0, 100.0, 100.0),
    spec("rays", 0.0, 100.0, 66.0),
    spec("ring_opacity", 0.0, 100.0, 100.0),
    spec("ray_opacity", 0.0, 100.0, 100.0),
    spec("line_width", 0.5, 12.0, 3.0),
    spec("glow", 0.0, 100.0, 0.0),
    spec("speed", 0.0, 100.0, 16.0),
    spec("bend_x", -100.0, 100.0, 96.0),
    spec("bend_y", -100.0, 100.0, 6.0),
    spec("amplitude", 0.0, 100.0, 0.0),
    spec("diffuse", 0.0, 100.0, 72.0),
    spec("shading", 0.0, 100.0, 25.0),
    spec("spiral_arms", 1.0, 12.0, 6.0),
    spec("spiral_twist", -100.0, 100.0, 40.0),
    spec("spiral_band", 5.0, 100.0, 60.0),
    spec("spiral_blend", 0.0, 100.0, 0.0),
    spec("spiral_spin", -100.0, 100.0, 20.0),
    spec("spiral_soft", 0.0, 100.0, 10.0),
    spec("spiral_turb", 0.0, 100.0, 0.0),
    spec("stars", 0.0, 100.0, 50.0),
    spec("streak_len", 0.0, 100.0, 50.0),
    spec("streak_thick", 0.0, 100.0, 30.0),
    spec("spread", 0.0, 100.0, 40.0),
    spec("color_split", 0.0, 100.0, 0.0),
    spec("grid_gap", 0.0, 90.0, 30.0),
    spec("grid_fill", 0.0, 100.0, 0.0),
    spec("grid_density", 0.0, 100.0, 100.0),
    spec("grid_twinkle", 0.0, 100.0, 0.0),
    spec("grid_random", 0.0, 100.0, 0.0),
    spec("strips", 1.0, 4.0, 3.0),
    spec("strip_width", 0.5, 12.0, 3.0),
    spec("walls", 0.0, 100.0, 35.0),
    spec("windows", 0.0, 100.0, 40.0),
    spec("orbs", 0.0, 100.0, 30.0),
    spec("grad_steps", 0.0, 100.0, 0.0),
    spec("grad_repeat", 0.0, 10.0, 1.0),
    spec("grad_stripes", 0.0, 100.0, 0.0),
];

/// Wall palette: night violet, magenta, teal, pale sand.
const PALETA: [f32; 12] = [
    0.035, 0.015, 0.090, 0.420, 0.035, 0.330, 0.040, 0.330, 0.520, 0.920, 0.840, 0.700,
];
/// Wireframe, streaks and grid: ice blue, cobalt, hot pink, white.
const LINEA: [f32; 12] = [
    0.040, 0.520, 0.820, 0.070, 0.180, 0.900, 0.820, 0.080, 0.680, 1.000, 1.000, 1.000,
];
/// Neon frames, windows, light blocks and orbs.
const ACENTO: [f32; 12] = [
    1.000, 0.320, 0.040, 0.980, 0.060, 0.140, 0.080, 0.760, 0.900, 0.580, 1.000, 0.380,
];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "palette" => PALETA, "line" => LINEA, "accent" => ACENTO);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "deep-tunnel",
    "Deep Tunnel",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/deep_tunnel.wgsl"),
);

crate::scaffold_model!(DeepTunnel, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(DeepTunnel::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("style", 2.0),
        ("shape", 2.0),
        ("rings", 70.0),
        ("rays", 60.0),
        ("ring_opacity", 80.0),
        ("ray_opacity", 70.0),
        ("line_width", 4.0),
        ("glow", 40.0),
        ("speed", 50.0),
        ("bend_x", 30.0),
        ("bend_y", -20.0),
        ("amplitude", 60.0),
        ("diffuse", 70.0),
        ("shading", 60.0),
        ("spiral_arms", 8.0),
        ("spiral_twist", 60.0),
        ("spiral_band", 70.0),
        ("spiral_blend", 50.0),
        ("spiral_spin", 40.0),
        ("spiral_soft", 30.0),
        ("spiral_turb", 50.0),
        ("stars", 60.0),
        ("streak_len", 60.0),
        ("streak_thick", 50.0),
        ("spread", 50.0),
        ("color_split", 40.0),
        ("grid_gap", 30.0),
        ("grid_fill", 40.0),
        ("grid_density", 80.0),
        ("grid_twinkle", 50.0),
        ("grid_random", 40.0),
        ("strips", 4.0),
        ("strip_width", 6.0),
        ("walls", 60.0),
        ("windows", 60.0),
        ("orbs", 60.0),
        ("grad_steps", 40.0),
        ("grad_repeat", 3.0),
        ("grad_stripes", 50.0),
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
            eprintln!("deep-tunnel: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
