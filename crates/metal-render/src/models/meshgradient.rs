//! `meshgradient`: A mesh of colour points on the procedural scaffold (`shared/scaffold.rs`), absorbing a static mesh variant. WGSL written from the interpolation maths.
//!
//! 33 own floats: upstream `puntos` (stepped) `distorsion giro_remolino
//! grano_mez grano_sup`, `escala velocidad rotacion off_x off_y`, the folded-in
//! static `ondas_x ondas_y fase_x fase_y mezcla` plus `estatico`, and own
//! variability: `estatico_vel orbita radio_punto nitidez turbulencia vena
//! remolino_caida pliegue bordes bandas contorno paleta_mez deriva_color
//! grano_tam brillo_grano luz vineta`. Slots: `colores`, `colores2`, `tinte`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("dots", 2.0, 10.0, 5.0, 0.2),
    spec("distortion", 0.0, 1.0, 0.8),
    spec("turn_swirl", 0.0, 1.0, 0.1),
    spec("grain_mix", 0.0, 1.0, 0.0),
    spec("grain_surface", 0.0, 1.0, 0.0),
    spec("scale", 0.2, 4.0, 1.0),
    spec("speed", -3.0, 3.0, 1.0),
    spec("rotation", 0.0, 1.0, 0.0),
    spec("off_x", -1.0, 1.0, 0.0),
    spec("off_y", -1.0, 1.0, 0.0),
    spec("waves_x", 0.0, 4.0, 1.0),
    spec("waves_y", 0.0, 4.0, 1.0),
    spec("phase_x", 0.0, 1.0, 0.6),
    spec("phase_y", 0.0, 1.0, 0.21),
    spec("mix", 0.0, 1.0, 0.5),
    spec("static", 0.0, 1.0, 0.0),
    spec("static_speed", -2.0, 2.0, 0.0),
    spec("orbit", 0.0, 2.0, 1.0),
    spec("radius_dot", 0.2, 1.6, 1.0),
    spec("sharpness", 0.0, 1.0, 0.3),
    spec("turbulence", 0.0, 1.0, 0.3),
    spec("vein", 0.0, 1.0, 0.0),
    spec("swirl_fall", 0.2, 4.0, 1.5),
    spec("fold", 0.0, 1.0, 0.0),
    spec("edges", 0.0, 1.0, 0.0),
    spec("bands", 0.0, 1.0, 0.0),
    spec("outline", 0.0, 1.0, 0.0),
    spec("palette_mix", 0.0, 1.0, 0.3),
    spec("drift_color", -2.0, 2.0, 0.0),
    spec("grain_size", 0.2, 4.0, 1.0),
    spec("shine_grain", 0.0, 1.0, 0.0),
    spec("light", 0.0, 1.0, 0.0),
    spec("vignette", 0.0, 1.0, 0.0),
];

/// Linear values: pink, orange, yellow, mint.
const COLORES: [f32; 12] = [1.00, 0.15, 0.34, 1.00, 0.32, 0.13, 1.00, 0.63, 0.15, 0.39, 0.79, 0.60];
/// Second mesh: blue, violet, rose, lime.
const COLORES2: [f32; 12] = [0.21, 0.47, 1.00, 0.38, 0.22, 0.95, 0.90, 0.20, 0.50, 0.60, 0.90, 0.20];
/// Seam / sparkle tint: white, gold, cyan, rose.
const TINTE: [f32; 12] = [1.00, 1.00, 1.00, 1.00, 0.75, 0.10, 0.10, 0.90, 0.95, 1.00, 0.40, 0.60];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "colores" => COLORES, "colores2" => COLORES2, "tint" => TINTE);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "meshgradient",
    "Mesh Gradient",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/meshgradient.wgsl"),
);

crate::scaffold_model!(Meshgradient, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Meshgradient::new())
    }

    /// Optional stages on so every float has something to act on.
    const BASE: &[(&str, f32)] = &[
        ("static", 0.5),
        ("static_speed", 0.5),
        ("grain_mix", 0.5),
        ("grain_surface", 0.5),
        ("vein", 0.3),
        ("fold", 0.3),
        ("edges", 0.5),
        ("bands", 0.4),
        ("outline", 0.5),
        ("palette_mix", 0.5),
        ("drift_color", 0.5),
        ("shine_grain", 0.5),
        ("light", 0.5),
        ("vignette", 0.5),
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
            eprintln!("meshgradient: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
