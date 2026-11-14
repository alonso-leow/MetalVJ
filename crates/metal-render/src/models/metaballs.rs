//! `metaballs`: Soft blobs orbiting and merging like liquid, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the metaball field maths.
//!
//! Own floats (24): the brief's `cuenta tam viscosidad umbral`, plus
//! `suavidad velocidad orbita_r orbita_var excentricidad tam_var pulso_tam caos
//! atraccion centro_x centro_y color_bola color_deriva brillo_nucleo borde halo
//! sombreado luz_dir gelatina semilla`. Slots: `bolas` (one colour per ball
//! along its gradient), `nucleo` (inner glow / rim), `halo` (outer glow).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("count", 1.0, 16.0, 8.0, 0.2),
    spec("size", 0.0, 1.0, 0.5),
    spec("viscosity", 0.0, 1.0, 0.5),
    spec("threshold", 0.0, 1.0, 0.5),
    spec("softness", 0.0, 1.0, 0.2),
    spec("speed", -3.0, 3.0, 1.0),
    spec("orbit_r", 0.0, 1.0, 0.6),
    spec("orbit_variation", 0.0, 1.0, 0.5),
    spec("eccentricity", 0.0, 1.0, 0.3),
    spec("size_variation", 0.0, 1.0, 0.4),
    spec("pulse_size", 0.0, 1.0, 0.0),
    spec("chaos", 0.0, 1.0, 0.0),
    spec("attraction", 0.0, 1.0, 0.0),
    spec("center_x", -0.9, 0.9, 0.0),
    spec("center_y", -0.9, 0.9, 0.0),
    spec("color_ball", 0.0, 1.0, 0.7),
    spec("color_drift", 0.0, 1.0, 0.0),
    spec("shine_core", 0.0, 1.0, 0.4),
    spec("edge", 0.0, 1.0, 0.3),
    spec("halo", 0.0, 1.0, 0.3),
    spec("shading", 0.0, 1.0, 0.0),
    spec("light_direction", 0.0, 1.0, 0.125),
    spec("gelatin", 0.0, 1.0, 0.0),
    spec_held("seed", 0.0, 50.0, 0.0, 0.2),
];

/// Linear values: magenta, orange, cyan, yellow.
const BOLAS: [f32; 12] = [0.90, 0.05, 0.45, 1.00, 0.25, 0.02, 0.02, 0.60, 0.90, 1.00, 0.80, 0.05];
/// Core / rim: pink-white, cream, pale blue, white.
const NUCLEO: [f32; 12] = [1.00, 0.55, 0.75, 1.00, 0.90, 0.70, 0.60, 0.80, 1.00, 1.00, 1.00, 1.00];
/// Outer glow: violet, blue, magenta, deep blue.
const HALO: [f32; 12] = [0.35, 0.05, 0.80, 0.05, 0.10, 0.60, 0.60, 0.05, 0.40, 0.02, 0.05, 0.30];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "bolas" => BOLAS, "core" => NUCLEO, "halo" => HALO);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "metaballs",
    "Metaballs",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/metaballs.wgsl"),
);

crate::scaffold_model!(Metaballs, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Metaballs::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("pulse_size", 0.4),
        ("chaos", 0.4),
        ("attraction", 0.4),
        ("color_drift", 0.4),
        ("shading", 0.5),
        ("gelatin", 0.4),
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
            eprintln!("metaballs: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
