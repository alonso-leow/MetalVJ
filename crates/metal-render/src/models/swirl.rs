//! `swirl`: A vortex of colour bands on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the vortex maths.
//!
//! Own floats (24): the brief's `bandas torsion centro_x centro_y proporcion
//! suavidad ruido ruido_frec`, plus `velocidad torsion_pulso radio_nucleo
//! ondulacion ondul_frec excentricidad excent_ang facetas borde degradado
//! color_giro gemelo gemelo_dist desvanece turbulencia sombra`. Slots: `tono_a`
//! (first tone), `tono_b` (second tone), `linea` (outlines).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("bands", 1.0, 16.0, 4.0, 1.0),
    spec("torsion", 0.0, 1.0, 0.35),
    spec("center_x", -0.9, 0.9, 0.0),
    spec("center_y", -0.9, 0.9, 0.0),
    spec("proportion", 0.0, 1.0, 0.5),
    spec("softness", 0.0, 1.0, 0.1),
    spec("noise", 0.0, 1.0, 0.2),
    spec("noise_freq", 0.0, 1.0, 0.4),
    spec("speed", -3.0, 3.0, 0.32),
    spec("torsion_pulse", 0.0, 1.0, 0.0),
    spec("radius_core", 0.0, 0.6, 0.0),
    spec("ripple", 0.0, 1.0, 0.0),
    spec("ripple_freq", 0.0, 1.0, 0.3),
    spec("eccentricity", 0.0, 1.5, 0.0),
    spec("eccentricity_angle", 0.0, 1.0, 0.0),
    spec("facets", 0.0, 1.0, 0.0),
    spec("edge", 0.0, 1.0, 0.0),
    spec("gradient", 0.0, 1.0, 0.0),
    spec("color_turn", -3.0, 3.0, 0.0),
    spec("twin", 0.0, 1.0, 0.0),
    spec("twin_distance", 0.1, 1.0, 0.5),
    spec("fade", 0.0, 1.0, 0.4),
    spec("turbulence", 0.0, 1.0, 0.0),
    spec("shadow", 0.0, 1.0, 0.0),
];

/// Linear values: hot pink, orange, yellow, green.
const TONO_A: [f32; 12] = [1.00, 0.00, 0.16, 0.97, 0.09, 0.00, 1.00, 0.51, 0.00, 0.00, 0.67, 0.35];
/// Blue, violet, teal, rose.
const TONO_B: [f32; 12] = [0.04, 0.24, 1.00, 0.24, 0.02, 0.84, 0.01, 0.26, 0.38, 0.86, 0.06, 0.15];
/// Outlines: near-black, cream, deep red, white.
const LINEA: [f32; 12] = [0.01, 0.00, 0.02, 1.00, 0.85, 0.60, 0.40, 0.00, 0.02, 1.00, 1.00, 1.00];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "tono_a" => TONO_A, "tono_b" => TONO_B, "linea" => LINEA);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "swirl",
    "Swirl",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/swirl.wgsl"),
);

crate::scaffold_model!(Swirl, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Swirl::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("torsion_pulse", 0.4),
        ("radius_core", 0.1),
        ("ripple", 0.4),
        ("eccentricity", 0.3),
        ("facets", 0.3),
        ("edge", 0.4),
        ("gradient", 0.3),
        ("color_turn", 0.5),
        ("twin", 0.5),
        ("turbulence", 0.3),
        ("shadow", 0.3),
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
            eprintln!("swirl: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
