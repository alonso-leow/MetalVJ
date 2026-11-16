//! `dithering`: A procedural field quantised to a few ink levels through an ordered or random threshold, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the screening maths.
//!
//! Own floats (22) on top of the kits (CAM2D 5, TWIST 4, LOOK 6): the brief's
//! `forma` (held: simple, warp, dots, wave, ripple, swirl, sphere), `matriz`
//! (held: 2x2, 4x4, 8x8, random) and `tam`, plus `velocidad fase escala
//! centro_x centro_y deformacion ondas detalle niveles umbral curva mezcla
//! ruido giro_malla invierte anim_malla rejilla vineta vacio` (vacio lets the `fondo` show through the paper level).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("shape", 0.0, 6.0, 1.0, 0.15),
    spec_held("matrix", 0.0, 3.0, 1.0, 0.15),
    spec("size", 0.5, 20.0, 3.0),
    spec("speed", -2.0, 2.0, 0.5),
    spec("phase", 0.0, 1.0, 0.0),
    spec("scale", 0.3, 4.0, 1.2),
    spec("center_x", -1.0, 1.0, 0.0),
    spec("center_y", -1.0, 1.0, 0.0),
    spec("deformation", 0.0, 1.0, 0.0),
    spec("waves", 0.0, 1.0, 0.0),
    spec("detail", 0.0, 1.0, 0.0),
    spec_held("levels", 2.0, 8.0, 2.0, 0.15),
    spec("threshold", -0.5, 0.5, 0.0),
    spec("curve", 0.3, 3.0, 1.0),
    spec("mix", 0.0, 1.0, 0.0),
    spec("noise", 0.0, 1.0, 0.0),
    spec("turn_mesh", -1.57, 1.57, 0.0),
    spec_held("invert", 0.0, 1.0, 0.0, 0.1),
    spec("animation_mesh", 0.0, 4.0, 0.0),
    spec("grid", 0.0, 1.0, 0.0),
    spec("vignette", 0.0, 1.0, 0.0),
    spec("empty", 0.0, 1.0, 0.0),
];

/// Linear values: lime-green ink ramp (dark moss to bright lime).
const TINTA: [f32; 12] = [0.02, 0.18, 0.08, 0.10, 0.45, 0.16, 0.30, 0.85, 0.35, 0.75, 1.0, 0.60];
/// Dark plum paper.
const PAPEL: [f32; 12] = [0.02, 0.006, 0.014, 0.03, 0.01, 0.022, 0.05, 0.015, 0.03, 0.07, 0.02, 0.045];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "ink" => TINTA, "paper" => PAPEL);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "dithering",
    "Dithering",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/dithering.wgsl"),
);

crate::scaffold_model!(Dithering, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Dithering::new())
    }

    /// Optional stages on so every float has something to act on.
    const BASE: &[(&str, f32)] = &[
        ("shape", 4.0),
        ("levels", 4.0),
        ("deformation", 0.3),
        ("waves", 0.2),
        ("detail", 0.2),
        ("noise", 0.2),
        ("animation_mesh", 1.0),
        ("grid", 0.3),
        ("vignette", 0.3),
        ("empty", 0.5),
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
            eprintln!("dithering: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
