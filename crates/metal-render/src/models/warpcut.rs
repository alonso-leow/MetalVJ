//! `warpcut`: A pattern (checks, stripes, edge, rings, triangles) bent by an iterated swirl and a noise distortion, cut into four colours on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the warp maths.
//!
//! 27 own floats: `forma` (stepped) `proporcion suavidad forma_escala
//! distorsion remolino iter_remolino` (stepped), plus own variability: `escala
//! velocidad rotacion off_x off_y dist_freq vel_remolino remolino_fase asimetria
//! pliegue cruce angulo_forma contorno bandas grano relieve paleta_mez
//! deriva_color vacio`. Slots: `colores`, `colores2`, `acento`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("shape", 0.0, 4.0, 0.0, 0.2),
    spec("proportion", 0.0, 1.0, 0.45),
    spec("softness", 0.0, 1.0, 0.8),
    spec("shape_scale", 0.02, 1.0, 0.1),
    spec("distortion", 0.0, 1.0, 0.25),
    spec("swirl", 0.0, 1.5, 0.8),
    spec_held("iter_swirl", 0.0, 20.0, 10.0, 0.2),
    spec("scale", 0.2, 4.0, 1.0),
    spec("speed", -3.0, 3.0, 0.5),
    spec("rotation", 0.0, 1.0, 0.0),
    spec("off_x", -1.0, 1.0, 0.0),
    spec("off_y", -1.0, 1.0, 0.0),
    spec("distance_freq", 1.0, 20.0, 6.0),
    spec("speed_swirl", -3.0, 3.0, 0.6),
    spec("swirl_phase", 0.0, 1.0, 0.0),
    spec("asymmetry", -1.0, 1.0, 0.0),
    spec("fold", 0.0, 1.0, 0.0),
    spec("crossing", 0.0, 1.0, 0.5),
    spec("angle_shape", 0.0, 1.0, 0.0),
    spec("outline", 0.0, 1.0, 0.0),
    spec("bands", 0.0, 1.0, 0.0),
    spec("grain", 0.0, 1.0, 0.0),
    spec("relief", 0.0, 1.0, 0.0),
    spec("palette_mix", 0.0, 1.0, 0.0),
    spec("drift_color", -2.0, 2.0, 0.0),
    spec("empty", 0.0, 1.0, 0.2),
];

/// Linear values: blue, green, yellow, pink.
const COLORES: [f32; 12] = [0.04, 0.24, 1.00, 0.00, 0.64, 0.35, 1.00, 0.60, 0.04, 0.85, 0.15, 0.30];
/// Second set: violet, deep teal, orange, cream.
const COLORES2: [f32; 12] = [0.24, 0.04, 0.80, 0.00, 0.25, 0.30, 0.95, 0.20, 0.00, 0.95, 0.80, 0.55];
/// Highlight tint for outlines and relief: white, gold, cyan, rose.
const ACENTO: [f32; 12] = [1.00, 1.00, 1.00, 1.00, 0.75, 0.10, 0.10, 0.90, 0.95, 1.00, 0.40, 0.60];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "colores" => COLORES, "colores2" => COLORES2, "accent" => ACENTO);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "warpcut",
    "Warp Cut",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/warpcut.wgsl"),
);

crate::scaffold_model!(Warpcut, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Warpcut::new())
    }

    /// Optional stages on so every float has something to act on.
    const BASE: &[(&str, f32)] = &[
        ("fold", 0.3),
        ("outline", 0.5),
        ("bands", 0.4),
        ("grain", 0.4),
        ("relief", 0.5),
        ("palette_mix", 0.5),
        ("drift_color", 0.5),
        ("asymmetry", 0.2),
        ("shape", 0.0),
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
            eprintln!("warpcut: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
