//! `dotgrid`: A lattice of small shapes on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the lattice maths.
//!
//! Own floats (22) on top of the scaffold kits: the brief's `forma` (stepped:
//! circle, diamond, square, triangle, cross) `tam gap_x gap_y trazo tam_rango
//! op_rango`, plus variability floats: `desfase` (brick stagger), `dispersion`
//! (scatter), `redondez`, `suavidad`, `giro_punto`, travelling size wave
//! (`ola ola_freq ola_vel ola_dir`), `destello destello_vel` (twinkle), `lente`
//! (centre bulge), `vineta` (edge fall-off), `vel_x vel_y` (scroll) and
//! `mezcla_color` (per-dot palette pick). Slots: `relleno`, `trazo`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("shape", 0.0, 4.0, 0.0, 0.2),
    spec("size", 0.004, 0.25, 0.045),
    spec("gap_x", 0.03, 0.5, 0.1),
    spec("gap_y", 0.03, 0.5, 0.1),
    spec("stroke", 0.0, 0.04, 0.0),
    spec("size_range", 0.0, 1.0, 0.0),
    spec("opacity_range", 0.0, 1.0, 0.0),
    spec("shift", 0.0, 1.0, 0.0),
    spec("dispersion", 0.0, 0.8, 0.0),
    spec("roundness", 0.0, 1.0, 0.0),
    spec("softness", 0.0, 1.0, 0.0),
    spec("turn_dot", 0.0, 1.0, 0.0),
    spec("wave", 0.0, 1.0, 0.0),
    spec("wave_freq", 0.1, 4.0, 1.0),
    spec("wave_speed", -3.0, 3.0, 0.6),
    spec("wave_direction", 0.0, 1.0, 0.1),
    spec("flash", 0.0, 1.0, 0.0),
    spec("flash_speed", 0.0, 4.0, 1.0),
    spec("lens", 0.0, 2.0, 0.0),
    spec("vignette", 0.0, 1.0, 0.0),
    spec("speed_x", -2.0, 2.0, 0.0),
    spec("speed_y", -2.0, 2.0, 0.0),
    spec("mix_color", 0.0, 1.0, 0.0),
];

/// Linear values (targets encode sRGB on write): white, cyan, pink, amber.
const RELLENO: [f32; 12] = [0.90, 0.90, 0.90, 0.10, 0.70, 0.90, 0.90, 0.15, 0.50, 0.95, 0.60, 0.10];
/// Outline: amber, magenta, violet, white.
const TRAZO: [f32; 12] = [1.00, 0.40, 0.00, 0.85, 0.10, 0.55, 0.40, 0.10, 0.85, 0.90, 0.90, 0.90];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "fill" => RELLENO, "stroke" => TRAZO);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "dotgrid",
    "Dot Grid",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/dotgrid.wgsl"),
);

crate::scaffold_model!(Dotgrid, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Dotgrid::new())
    }

    /// Optional stages on so every float has something to act on.
    const BASE: &[(&str, f32)] = &[
        ("stroke", 0.006),
        ("size_range", 0.3),
        ("opacity_range", 0.3),
        ("shift", 0.3),
        ("dispersion", 0.2),
        ("roundness", 0.2),
        ("turn_dot", 0.2),
        ("wave", 0.4),
        ("flash", 0.4),
        ("lens", 0.4),
        ("vignette", 0.3),
        ("speed_x", 0.5),
        ("mix_color", 0.5),
        ("shape", 2.0),
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
            eprintln!("dotgrid: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
