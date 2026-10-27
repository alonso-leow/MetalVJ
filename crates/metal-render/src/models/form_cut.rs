//! `form-cut`: a cut-out form. A source figure (dot, block, outline, ring or
//! bar field) is laid down in the base slot, then repeated in the main slot and
//! shattered into displaced, rotated fragments that reveal the base beneath.
//! Behaviour and parameter list follow FormCutter; the WGSL is original
//! (`shaders/form_cut.wgsl`).
//!
//! Own floats (11): `cut_mode source trigger scale stroke_width cut_density
//! cut_size displacement rotation speed seed`. Slots: `main` (the cut layer),
//! `base` (the figure underneath).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("cut_mode", 0.0, 5.0, 2.0, 0.0),
    spec_held("source", 0.0, 5.0, 2.0, 0.0),
    spec("trigger", 0.0, 1.0, 0.65),
    spec("scale", 0.0, 1.0, 0.30),
    spec("stroke_width", 0.0, 1.0, 0.35),
    spec("cut_density", 0.0, 1.0, 0.80),
    spec("cut_size", 0.0, 1.0, 0.60),
    spec("displacement", 0.0, 1.0, 0.74),
    spec("rotation", 0.0, 1.0, 0.40),
    spec("speed", -2.0, 2.0, 0.50),
    spec_held("seed", 0.0, 50.0, 3.0, 0.2),
];

/// Cut layer: warm paper creams (linear).
const MAIN: [f32; 12] = [
    0.90, 0.86, 0.80, 0.84, 0.79, 0.72, 0.95, 0.92, 0.86, 0.80, 0.74, 0.66,
];
/// Base figure: orange, with ember variations (linear).
const BASE_COL: [f32; 12] = [
    0.84, 0.15, 0.03, 0.78, 0.22, 0.05, 0.88, 0.10, 0.02, 0.72, 0.13, 0.04,
];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "main" => MAIN, "base" => BASE_COL);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "form-cut",
    "Form Cut",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/form_cut.wgsl"),
);

crate::scaffold_model!(FormCut, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(FormCut::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("warp_amount", 0.4),
        ("pulse", 0.0),
        ("scale", 0.55),
        ("stroke_width", 0.6),
        ("cut_density", 0.7),
        ("cut_size", 0.8),
        ("displacement", 0.9),
        ("rotation", 0.7),
    ];

    #[test]
    fn standard_checks() {
        testing::check_all(build, BASE);
    }

    #[test]
    fn ms_per_frame() {
        if std::env::var("SCAFFOLD_MEASURE").is_ok() {
            eprintln!("form-cut: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
