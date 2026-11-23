//! `neuronoise`: A glowing web of neural filaments on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the noise maths.
//!
//! Own floats (16): `brillo_neuro contraste_neuro iteraciones cant_warp`
//! (brief) plus `velocidad escala grosor nitidez densidad anisotropia
//! giro_capas caida nodos pulso_nodos chispas turbulencia simetria deriva_fib
//! mezcla_medio fase_color`. Slots: `frente`, `medio`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("neural_shine", -1.0, 1.0, 0.1),
    spec("neural_contrast", 0.0, 1.0, 0.3),
    spec_held("iterations", 1.0, 8.0, 4.0, 0.2),
    spec("amount_warp", 0.0, 2.0, 0.6),
    spec("speed", -3.0, 3.0, 0.6),
    spec("scale", 0.0, 1.0, 0.4),
    spec("thickness", 0.0, 1.0, 0.2),
    spec("sharpness", 0.0, 1.0, 0.4),
    spec("density", 0.0, 1.0, 0.4),
    spec("anisotropy", 0.0, 1.0, 0.0),
    spec("layer_turn", 0.0, 3.0, 0.5),
    spec("fall", 0.0, 1.0, 0.4),
    spec("nodes", 0.0, 1.0, 0.3),
    spec("node_pulse", 0.0, 1.0, 0.3),
    spec("sparks", 0.0, 1.0, 0.2),
    spec("turbulence", 0.0, 1.0, 0.3),
    spec("symmetry", 0.0, 1.0, 0.0),
    spec("fiber_drift", -2.0, 2.0, 0.5),
    spec("medium_mix", 0.0, 1.0, 0.5),
    spec("phase_color", 0.0, 1.0, 0.0),
];

/// Linear: white, pale cyan, sky, blue.
const FRENTE: [f32; 12] = [1.00, 1.00, 1.00, 0.60, 0.90, 1.00, 0.30, 0.70, 1.00, 0.10, 0.40, 1.00];
/// Linear: sky blue, indigo, violet, deep blue.
const MEDIO: [f32; 12] = [0.06, 0.38, 1.00, 0.10, 0.15, 0.80, 0.30, 0.05, 0.70, 0.02, 0.05, 0.40];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "front" => FRENTE, "medium" => MEDIO);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "neuronoise",
    "Neuro Noise",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/neuronoise.wgsl"),
);

crate::scaffold_model!(Neuronoise, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Neuronoise::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("anisotropy", 0.4),
        ("symmetry", 0.5),
        ("nodes", 0.5),
        ("node_pulse", 0.5),
        ("sparks", 0.6),
        ("phase_color", 0.4),
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
            eprintln!("neuronoise: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
