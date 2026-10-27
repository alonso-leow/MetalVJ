//! `ripples`: Rows of undulating lines on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the wave maths.
//!
//! Own floats (29): the brief's `forma frecuencia amplitud espaciado
//! proporcion suavidad`, plus `velocidad angulo fase_y desfase_par armonicos
//! modulacion ruido ruido_frec grosor_var curva inclina radial cruce respiro
//! parpadeo guion guion_frec contorno sombra relleno degrade desvanece`.
//! Slots: `lineas` (rows, graded by `degrade`), `banda` (tint between rows).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("shape", 0.0, 3.0, 1.0, 0.2),
    spec("frequency", 0.0, 2.0, 0.5),
    spec("amplitude", 0.0, 3.0, 0.5),
    spec("spacing", 0.2, 4.0, 1.2),
    spec("proportion", 0.0, 1.0, 0.35),
    spec("softness", 0.0, 1.0, 0.1),
    spec("speed", -3.0, 3.0, 0.6),
    spec_held("angle", 0.0, 1.0, 0.0, 0.2),
    spec("phase_y", -3.0, 3.0, 0.4),
    spec("shift_par", 0.0, 1.0, 0.0),
    spec("harmonics", 0.0, 1.0, 0.0),
    spec("modulation", 0.0, 1.0, 0.0),
    spec("noise", 0.0, 1.0, 0.0),
    spec("noise_freq", 0.0, 1.0, 0.3),
    spec("thickness_variation", 0.0, 1.0, 0.0),
    spec("curve", -1.5, 1.5, 0.0),
    spec("tilt", 0.0, 1.0, 0.0),
    spec_held("radial", 0.0, 1.0, 0.0, 0.2),
    spec("crossing", 0.0, 1.0, 0.0),
    spec("breath", 0.0, 1.0, 0.0),
    spec("blink", 0.0, 1.0, 0.0),
    spec("dash", 0.0, 1.0, 0.0),
    spec("dash_freq", 0.0, 1.0, 0.3),
    spec("outline", 0.0, 1.0, 0.0),
    spec("shadow", 0.0, 1.0, 0.0),
    spec("fill", 0.0, 1.0, 0.0),
    spec("erode", 0.0, 1.0, 0.4),
    spec("fade", 0.0, 1.0, 0.0),
];

/// Linear values: white, cyan, pink, yellow.
const LINEAS: [f32; 12] = [1.00, 1.00, 1.00, 0.05, 0.80, 0.90, 0.90, 0.10, 0.45, 0.95, 0.75, 0.05];
/// Band tint: deep navy to violet.
const BANDA: [f32; 12] = [0.01, 0.02, 0.10, 0.04, 0.02, 0.20, 0.10, 0.03, 0.30, 0.02, 0.05, 0.15];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "lines" => LINEAS, "banda" => BANDA);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "ripples",
    "Ripples",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/ripples.wgsl"),
);

crate::scaffold_model!(Ripples, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Ripples::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("shift_par", 0.5),
        ("harmonics", 0.5),
        ("modulation", 0.5),
        ("noise", 0.5),
        ("thickness_variation", 0.5),
        ("curve", 0.4),
        ("tilt", 0.4),
        ("radial", 0.3),
        ("crossing", 0.4),
        ("breath", 0.4),
        ("blink", 0.3),
        ("dash", 0.4),
        ("outline", 0.3),
        ("shadow", 0.5),
        ("fill", 0.3),
        ("fade", 0.4),
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
            eprintln!("ripples: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
