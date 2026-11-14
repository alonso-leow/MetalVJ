//! `simplexnoise`: A palette stepped onto true 3D simplex gradient noise, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the noise maths.
//!
//! 27 own floats: `pasos_color`(H) `suavidad`, the field (`escala rotacion off_x
//! off_y velocidad fase_z estiramiento`), octaves (`octavas`(H) `lacunaridad
//! persistencia rot_octava crestas`), curl-ish flow (`flujo flujo_escala
//! flujo_vel`), shaping (`dureza sesgo vacio`), outlines (`bordes bordes_ancho`)
//! and colour (`paleta_mez paleta_var deriva_color grano grano_tam`). Slots:
//! `colores`, `colores2`, `lineas`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("steps_color", 1.0, 8.0, 2.0, 0.2),
    spec("softness", 0.0, 1.0, 0.0),
    spec("scale", 0.2, 6.0, 1.4),
    spec("rotation", 0.0, 1.0, 0.0),
    spec("off_x", -2.0, 2.0, 0.0),
    spec("off_y", -2.0, 2.0, 0.0),
    spec("speed", -3.0, 3.0, 0.5),
    spec("phase_z", 0.0, 1.0, 0.0),
    spec("stretch", 0.25, 4.0, 1.0),
    spec_held("octaves", 1.0, 5.0, 2.0, 0.2),
    spec("lacunarity", 1.2, 3.2, 2.0),
    spec("persistence", 0.15, 0.85, 0.5),
    spec("rot_octave", 0.0, 1.0, 0.35),
    spec("crests", 0.0, 1.0, 0.0),
    spec("flow", 0.0, 1.0, 0.3),
    spec("flow_scale", 0.3, 4.0, 1.0),
    spec("flow_speed", -2.0, 2.0, 0.4),
    spec("hardness", 0.0, 1.0, 0.4),
    spec("bias", -0.4, 0.4, 0.0),
    spec("empty", 0.0, 1.0, 0.3),
    spec("edges", 0.0, 1.0, 0.0),
    spec("edges_width", 0.02, 0.4, 0.12),
    spec("palette_mix", 0.0, 1.0, 0.2),
    spec("palette_variation", 0.0, 1.0, 0.3),
    spec("drift_color", -2.0, 2.0, 0.0),
    spec("grain", 0.0, 1.0, 0.0),
    spec("grain_size", 0.2, 4.0, 1.0),
];

/// Linear values: deep blue, cyan, warm white, orange.
const COLORES: [f32; 12] = [0.01, 0.03, 0.22, 0.00, 0.45, 0.70, 0.90, 0.85, 0.80, 0.90, 0.60, 0.04];
/// Second palette: plum, magenta, coral, gold.
const COLORES2: [f32; 12] = [0.10, 0.01, 0.15, 0.55, 0.04, 0.35, 0.90, 0.22, 0.20, 0.95, 0.60, 0.05];
/// Outline / ridge tint: near white, mint, pale yellow, ink.
const LINEAS: [f32; 12] = [1.00, 1.00, 1.00, 0.35, 1.00, 0.70, 1.00, 0.95, 0.40, 0.02, 0.02, 0.05];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "colores" => COLORES, "colores2" => COLORES2, "lines" => LINEAS);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "simplexnoise",
    "Simplex Noise",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/simplexnoise.wgsl"),
);

crate::scaffold_model!(Simplexnoise, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Simplexnoise::new())
    }

    /// Optional stages on so every float has something to act on.
    const BASE: &[(&str, f32)] = &[
        ("octaves", 3.0),
        ("flow", 0.5),
        ("crests", 0.3),
        ("edges", 0.6),
        ("palette_variation", 0.5),
        ("palette_mix", 0.4),
        ("drift_color", 0.5),
        ("grain", 0.5),
        ("empty", 0.5),
        ("rot_octave", 0.4),
        ("steps_color", 3.0),
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
            eprintln!("simplexnoise: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
