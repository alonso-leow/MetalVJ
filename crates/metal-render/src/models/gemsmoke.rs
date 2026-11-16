//! `gemsmoke`: A glassy gem with smoke inside and around it, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the field maths.
//!
//! Own floats (26): the brief's `forma` (stepped: diamond, circle, hexagon,
//! square, star) `distorsion_in distorsion_out brillo_out brillo_in tam`, plus
//! variability floats: gem `angulo alargue esquinas latido`; smoke `humo_escala
//! humo_vel llama remolino contraste_humo humo_color_vel`; facets `facetas
//! faceta_fuerza refraccion destello`; light `borde borde_brillo cuerpo
//! halo_radio aberracion`. Slots: `smoke`, `cuerpo`, `brillo`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("shape", 0.0, 4.0, 0.0, 0.2),
    spec("distortion_in", 0.0, 2.0, 0.8),
    spec("distortion_out", 0.0, 2.0, 0.6),
    spec("shine_out", 0.0, 2.0, 0.55),
    spec("shine_in", 0.0, 2.0, 1.0),
    spec("size", 0.15, 1.4, 0.8),
    spec("angle", 0.0, 1.0, 0.0),
    spec("stretch", 0.4, 2.5, 1.0),
    spec("corners", 0.0, 1.0, 0.0),
    spec("beat", 0.0, 1.0, 0.0),
    spec("smoke_scale", 0.2, 3.0, 1.0),
    spec("smoke_speed", -2.0, 2.0, 0.5),
    spec("flame", -1.5, 1.5, 0.3),
    spec("swirl", -2.0, 2.0, 0.3),
    spec("contrast_smoke", 0.0, 3.0, 1.0),
    spec("smoke_color_speed", 0.0, 2.0, 0.2),
    spec_held("facets", 3.0, 16.0, 8.0, 0.1),
    spec("facet_force", 0.0, 1.0, 0.5),
    spec("refraction", 0.0, 1.0, 0.3),
    spec("flash", 0.0, 1.0, 0.0),
    spec("edge", 0.0, 0.3, 0.04),
    spec("edge_shine", 0.0, 2.0, 0.5),
    spec("body", 0.0, 1.0, 0.4),
    spec("halo_radius", 0.02, 1.0, 0.25),
    spec("aberration", 0.0, 1.0, 0.0),
];

/// Linear values: smoke colours (deep violet, teal, magenta, pale blue).
const HUMO: [f32; 12] = [0.03, 0.01, 0.12, 0.02, 0.35, 0.40, 0.55, 0.05, 0.30, 0.55, 0.65, 0.95];
/// Gem body tint: ink, blue, rose, cream.
const CUERPO: [f32; 12] = [0.02, 0.02, 0.06, 0.05, 0.10, 0.45, 0.60, 0.15, 0.40, 0.95, 0.92, 0.80];
/// Glow / sparkle: warm white, cyan, pink, gold.
const BRILLO: [f32; 12] = [0.90, 0.85, 0.80, 0.20, 0.80, 0.95, 0.90, 0.30, 0.60, 0.95, 0.60, 0.10];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "smoke" => HUMO, "body" => CUERPO, "shine" => BRILLO);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "gemsmoke",
    "Gem Smoke",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/gemsmoke.wgsl"),
);

crate::scaffold_model!(Gemsmoke, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Gemsmoke::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("shape", 0.0),
        ("corners", 0.2),
        ("beat", 0.3),
        ("flash", 0.6),
        ("aberration", 0.4),
        ("angle", 0.1),
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
            eprintln!("gemsmoke: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
