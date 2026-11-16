//! `slumped-grid`: a grid of randomly occupied squares that sag, drip and
//! fuse into one another on the procedural scaffold (`shared/scaffold.rs`). Behaviour
//! and parameter list follow the "Melted Squares" app tool: a square lattice,
//! a random fill probability, a corner-radius percentage, and a single ink
//! colour, with the diagonal "melt" bridge at touching corners. The WGSL is
//! original (`shaders/slumped_grid.wgsl`).
//!
//! Own floats (28): `rows cols` (held grid size), `corner_radius fill_prob
//! gap melt sag drip wobble noise_scale noise_speed speed drift_x drift_y
//! jitter rot_jitter stretch breath edge_soft contour shadow glow highlight
//! sparkle hue_speed color_mix`, plus held `mode` (smooth union / diagonal
//! melts / heavy gravity) and held `seed`. Slots: `slump`, `void`, `glint`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("rows", 2.0, 10.0, 5.0, 0.1),
    spec_held("cols", 2.0, 10.0, 5.0, 0.1),
    spec("corner_radius", 0.0, 1.0, 0.35),
    spec("fill_prob", 0.05, 0.95, 0.5),
    spec("gap", 0.0, 0.45, 0.0),
    spec("melt", 0.0, 1.0, 0.55),
    spec("sag", 0.0, 1.0, 0.35),
    spec("drip", 0.0, 2.0, 0.6),
    spec("wobble", 0.0, 1.0, 0.4),
    spec("noise_scale", 0.2, 5.0, 1.2),
    spec("noise_speed", -2.0, 2.0, 0.6),
    spec("speed", -2.0, 2.0, 0.35),
    spec("drift_x", -1.0, 1.0, 0.0),
    spec("drift_y", -1.0, 1.0, 0.0),
    spec("jitter", 0.0, 1.0, 0.35),
    spec("rot_jitter", 0.0, 1.0, 0.0),
    spec("stretch", 0.0, 1.0, 0.3),
    spec("breath", 0.0, 1.0, 0.2),
    spec("edge_soft", 0.0, 1.0, 0.08),
    spec("contour", 0.0, 1.0, 0.5),
    spec("shadow", 0.0, 1.0, 0.4),
    spec("glow", 0.0, 1.0, 0.2),
    spec("highlight", 0.0, 1.0, 0.35),
    spec("sparkle", 0.0, 1.0, 0.0),
    spec("hue_speed", 0.0, 1.0, 0.15),
    spec("color_mix", 0.0, 1.0, 0.6),
    spec_held("mode", 0.0, 2.0, 0.0, 0.2),
    spec_held("seed", 0.0, 50.0, 3.0, 0.2),
];

/// Slump ink: cream, orange, rust, warm white.
const SLUMP: [f32; 12] = [0.95, 0.78, 0.50, 1.00, 0.35, 0.08, 0.62, 0.10, 0.05, 1.00, 0.88, 0.72];
/// Void: near-black indigo, maroon, brown, deep teal.
const VOID: [f32; 12] = [0.010, 0.010, 0.030, 0.050, 0.015, 0.055, 0.090, 0.030, 0.020, 0.020, 0.090, 0.100];
/// Glint: white, pale gold, mint, pink.
const GLINT: [f32; 12] = [1.00, 1.00, 1.00, 1.00, 0.90, 0.50, 0.50, 1.00, 0.80, 1.00, 0.40, 0.60];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "slump" => SLUMP, "void" => VOID, "glint" => GLINT);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "slumped-grid",
    "Slumped Grid",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/slumped_grid.wgsl"),
);

crate::scaffold_model!(SlumpedGrid, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(SlumpedGrid::new())
    }

    /// Optional stages on so every float has something to act on.
    const BASE: &[(&str, f32)] = &[
        ("mode", 1.0),
        ("corner_radius", 0.45),
        ("gap", 0.04),
        ("melt", 0.7),
        ("sag", 0.55),
        ("drip", 0.8),
        ("wobble", 0.5),
        ("jitter", 0.35),
        ("rot_jitter", 0.2),
        ("stretch", 0.3),
        ("breath", 0.3),
        ("edge_soft", 0.2),
        ("contour", 0.5),
        ("shadow", 0.5),
        ("glow", 0.3),
        ("highlight", 0.4),
        ("sparkle", 0.3),
        ("hue_speed", 0.2),
        ("color_mix", 0.5),
        ("speed", 0.5),
        ("drift_x", 0.3),
        ("drift_y", 0.2),
        ("noise_scale", 1.5),
        ("noise_speed", 0.8),
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
            eprintln!("slumped-grid: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
