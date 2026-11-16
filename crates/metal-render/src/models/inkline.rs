//! `inkline`: a hand-inked doodle sheet on the procedural scaffold
//! (`shared/scaffold.rs`). Behaviour and controls follow the built-in Doodle
//! tool: brush family (pencil / chalk / watercolour / paint), stroke thickness,
//! softness, boil variation count and speed, ink and paper colours, plus the
//! tool's shape stamps. The WGSL is original (`shaders/inkline.wgsl`); no
//! canvas path calls are reproduced, the strokes are distance-to-curve fields.
//!
//! Own floats (23): `brush shape thickness softness variations speed count span
//! wave frequency angle wobble taper pressure grain texture splatter bleed dry
//! bristle wash opacity seed`. Slots: `ink`, `paper`, `wash`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("brush", 0.0, 3.0, 0.0, 0.2),
    spec_held("shape", 0.0, 3.0, 0.0, 0.2),
    spec("thickness", 0.004, 0.12, 0.045),
    spec("softness", 0.0, 1.0, 0.35),
    spec_held("variations", 2.0, 8.0, 4.0, 0.2),
    spec("speed", -4.0, 4.0, 1.5),
    spec_held("count", 1.0, 9.0, 6.0, 0.15),
    spec("span", 0.2, 1.4, 1.0),
    spec("wave", 0.0, 0.25, 0.06),
    spec("frequency", 0.1, 4.0, 1.0),
    spec("angle", -1.5708, 1.5708, 0.0),
    spec("wobble", 0.0, 1.0, 0.5),
    spec("taper", 0.0, 1.0, 0.7),
    spec("pressure", 0.0, 1.0, 0.5),
    spec("grain", 0.0, 1.0, 0.5),
    spec("texture", 1.0, 40.0, 12.0),
    spec("splatter", 0.0, 1.0, 0.2),
    spec("bleed", 0.0, 1.0, 0.25),
    spec("dry", 0.0, 1.0, 0.3),
    spec("bristle", 0.0, 1.0, 0.35),
    spec("wash", 0.0, 1.0, 0.3),
    spec("ink_opacity", 0.0, 1.0, 0.92),
    spec_held("seed", 0.0, 50.0, 7.0, 0.2),
];

/// Ink: blue-black, graphite, warm sepia graphite.
const INK: [f32; 12] = [
    0.012, 0.014, 0.026, //
    0.028, 0.033, 0.058, //
    0.075, 0.070, 0.062, //
    0.160, 0.130, 0.100,
];
/// Paper: warm cream to tan.
const PAPER: [f32; 12] = [
    0.955, 0.925, 0.855, //
    0.925, 0.885, 0.790, //
    0.860, 0.800, 0.680, //
    0.780, 0.710, 0.570,
];
/// Wash: pale blue-grey to deep slate.
const WASH: [f32; 12] = [
    0.420, 0.500, 0.620, //
    0.280, 0.380, 0.520, //
    0.160, 0.240, 0.340, //
    0.620, 0.700, 0.780,
];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "ink" => INK, "paper" => PAPER, "wash" => WASH);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "inkline",
    "Inkline",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/inkline.wgsl"),
);

crate::scaffold_model!(Inkline, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Inkline::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("brush", 0.0),
        ("shape", 0.0),
        ("softness", 0.5),
        ("speed", 1.5),
        ("wobble", 0.6),
        ("taper", 0.7),
        ("pressure", 0.6),
        ("grain", 0.6),
        ("splatter", 0.5),
        ("bleed", 0.5),
        ("dry", 0.5),
        ("bristle", 0.5),
        ("wash", 0.5),
        ("ink_opacity", 0.9),
        ("warp_amount", 0.3),
        ("pulse", 0.0),
    ];

    #[test]
    fn standard_checks() {
        testing::check_all(build, BASE);
    }

    #[test]
    fn ms_per_frame() {
        if std::env::var("SCAFFOLD_MEASURE").is_ok() {
            eprintln!("inkline: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
