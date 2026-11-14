//! `perlinnoise`: A field cut from fractal gradient noise, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the noise maths.
//!
//! Own floats: the brief's `proporcion suavidad octavas(H) persistencia
//! lacunaridad`, plus `escala velocidad rotacion off_x off_y warp_dom
//! warp_escala flujo morfeo cresta onda anisotropia contraste_campo bandas
//! n_bandas(H) contorno contorno_ancho relieve luz_ang rampa deriva_color
//! grano hueco`. Slots: `base` (low side), `frente` (high side), `acento` (outline
//! and rim).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("proportion", 0.0, 1.0, 0.45),
    spec("softness", 0.0, 1.0, 0.1),
    spec_held("octaves", 1.0, 8.0, 4.0, 0.2),
    spec("persistence", 0.1, 1.2, 0.55),
    spec("lacunarity", 1.1, 4.0, 2.0),
    spec("scale", 0.2, 8.0, 1.6),
    spec("speed", -3.0, 3.0, 0.5),
    spec("rotation", 0.0, 1.0, 0.0),
    spec("off_x", -2.0, 2.0, 0.0),
    spec("off_y", -2.0, 2.0, 0.0),
    spec("warp_domain", 0.0, 1.0, 0.0),
    spec("warp_scale", 0.2, 4.0, 1.0),
    spec("flow", 0.0, 1.0, 0.0),
    spec("morph", 0.0, 1.0, 0.2),
    spec("crest", 0.0, 1.0, 0.0),
    spec("wave", 0.0, 1.0, 0.0),
    spec("anisotropy", -1.0, 1.0, 0.0),
    spec("contrast_field", 0.5, 4.0, 1.8),
    spec("bands", 0.0, 1.0, 0.0),
    spec_held("n_bands", 2.0, 16.0, 6.0, 0.2),
    spec("outline", 0.0, 1.0, 0.0),
    spec("outline_width", 0.0, 1.0, 0.3),
    spec("relief", 0.0, 1.0, 0.0),
    spec("light_angle", 0.0, 1.0, 0.125),
    spec("ramp", 0.0, 1.0, 0.0),
    spec("drift_color", -1.0, 1.0, 0.0),
    spec("grain", 0.0, 1.0, 0.0),
    spec("hole", 0.0, 1.0, 0.0),
];

/// Linear values: deep violet, indigo, blue, night.
const BASE: [f32; 12] = [0.13, 0.02, 0.37, 0.07, 0.01, 0.25, 0.03, 0.05, 0.40, 0.01, 0.01, 0.08];
/// Pale pink, lilac, cream, white.
const FRENTE: [f32; 12] = [0.97, 0.60, 0.92, 0.70, 0.45, 0.95, 1.00, 0.85, 0.80, 1.00, 1.00, 1.00];
/// Outline: amber, orange, hot pink, white.
const ACENTO: [f32; 12] = [1.00, 0.70, 0.10, 1.00, 0.30, 0.03, 1.00, 0.10, 0.30, 1.00, 1.00, 1.00];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "base" => BASE, "frente" => FRENTE, "accent" => ACENTO);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "perlinnoise",
    "Perlin Noise",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/perlinnoise.wgsl"),
);

crate::scaffold_model!(Perlinnoise, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Perlinnoise::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("warp_domain", 0.5),
        ("flow", 0.5),
        ("crest", 0.4),
        ("wave", 0.4),
        ("anisotropy", 0.4),
        ("bands", 0.6),
        ("outline", 0.6),
        ("relief", 0.6),
        ("ramp", 0.5),
        ("grain", 0.5),
        ("hole", 0.5),
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
            eprintln!("perlinnoise: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
