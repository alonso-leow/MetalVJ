//! `graingradient`: Grainy gradient on the procedural scaffold (`shared/scaffold.rs`), absorbing a static radial variant. WGSL written from the gradient maths.
//!
//! 24 own floats: `forma` (stepped: wave, dots, truchet, corners, ripple, blob,
//! sphere) `suavidad intensidad ruido`, the folded-in radial `radio foco_dist
//! foco_ang caida mezcla dist_freq` plus `radial` (blend), and own variability:
//! `escala velocidad rotacion off_x off_y distorsion dist_fase bandas contorno
//! paleta_mez deriva_color grano_tam brillo_grano`. Slots: `colores`,
//! `colores2`, `brillo`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("shape", 0.0, 6.0, 3.0, 0.2),
    spec("softness", 0.0, 1.0, 0.5),
    spec("intensity", 0.0, 1.0, 0.5),
    spec("noise", 0.0, 1.0, 0.25),
    spec("scale", 0.2, 4.0, 1.0),
    spec("speed", -3.0, 3.0, 0.6),
    spec("rotation", 0.0, 1.0, 0.0),
    spec("off_x", -1.0, 1.0, 0.0),
    spec("off_y", -1.0, 1.0, 0.0),
    spec("radius", 0.1, 2.5, 0.8),
    spec("focus_distance", 0.0, 1.0, 0.0),
    spec("focus_angle", 0.0, 1.0, 0.0),
    spec("fall", 0.05, 2.0, 0.5),
    spec("mix", 0.0, 1.0, 0.5),
    spec("distance_freq", 1.0, 30.0, 12.0),
    spec("radial", 0.0, 1.0, 0.0),
    spec("distortion", 0.0, 1.0, 0.2),
    spec("distance_phase", 0.0, 1.0, 0.0),
    spec("bands", 0.0, 1.0, 0.0),
    spec("outline", 0.0, 1.0, 0.0),
    spec("palette_mix", 0.0, 1.0, 0.3),
    spec("drift_color", -2.0, 2.0, 0.0),
    spec("grain_size", 0.2, 4.0, 1.0),
    spec("shine_grain", 0.0, 1.0, 0.0),
];

/// Linear values: blue, green, yellow, pink.
const COLORES: [f32; 12] = [0.04, 0.24, 1.00, 0.00, 0.64, 0.35, 1.00, 0.60, 0.04, 0.85, 0.15, 0.30];
/// Second gradient: violet, teal, orange, cream.
const COLORES2: [f32; 12] = [0.24, 0.04, 0.80, 0.00, 0.25, 0.30, 0.95, 0.20, 0.00, 0.95, 0.80, 0.55];
/// Sparkle tint: white-ish, gold, cyan, rose.
const BRILLO: [f32; 12] = [1.00, 1.00, 1.00, 1.00, 0.75, 0.10, 0.10, 0.90, 0.95, 1.00, 0.40, 0.60];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "colores" => COLORES, "colores2" => COLORES2, "shine" => BRILLO);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "graingradient",
    "Grain Gradient",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/graingradient.wgsl"),
);

crate::scaffold_model!(Graingradient, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Graingradient::new())
    }

    /// Optional stages on so every float has something to act on.
    const BASE: &[(&str, f32)] = &[
        ("radial", 0.5),
        ("focus_distance", 0.4),
        ("noise", 0.3),
        ("distortion", 0.3),
        ("bands", 0.4),
        ("outline", 0.5),
        ("palette_mix", 0.5),
        ("drift_color", 0.5),
        ("shine_grain", 0.5),
        ("shape", 3.0),
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
            eprintln!("graingradient: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
