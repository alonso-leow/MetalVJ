//! `morphform`: a single paper-cut form that shatters into displaced copies on
//! a trigger envelope, on the procedural scaffold (`shared/scaffold.rs`). Behaviour and
//! parameter list follow the community "FormShift" tool; the WGSL is original
//! (`shaders/morphform.wgsl`) and recreates the six shatter families.
//!
//! Own floats (12): `scene shape trigger impact scale stretch_x stretch_y
//! rotation split shutter speed seed`. Slots: `shape`, `accent`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("scene", 0.0, 5.0, 3.0, 0.2),
    spec_held("shape", 0.0, 5.0, 4.0, 0.2),
    spec("trigger", 0.0, 100.0, 0.0),
    spec("impact", 0.0, 100.0, 70.0),
    spec("scale", 0.0, 100.0, 70.0),
    spec("stretch_x", 0.0, 100.0, 20.0),
    spec("stretch_y", 0.0, 100.0, 0.0),
    spec("rotation", 0.0, 100.0, 50.0),
    spec("split", 0.0, 100.0, 62.0),
    spec("shutter", 0.0, 100.0, 62.0),
    spec("speed", -2.0, 2.0, 0.5),
    spec_held("seed", 0.0, 50.0, 3.0, 0.2),
];

/// Shape: cream, warm sand, rust, bone.
const SHAPE: [f32; 12] = [0.92, 0.88, 0.80, 0.98, 0.74, 0.48, 0.72, 0.30, 0.12, 0.96, 0.93, 0.85];
/// Accent: ember, amber, deep orange, pale gold.
const ACCENT: [f32; 12] = [1.00, 0.32, 0.08, 1.00, 0.52, 0.12, 0.85, 0.16, 0.04, 1.00, 0.72, 0.30];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "shape" => SHAPE, "accent" => ACCENT);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "morphform",
    "Morphform",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/morphform.wgsl"),
);

crate::scaffold_model!(Morphform, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Morphform::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("warp_amount", 0.4),
        ("pulse", 0.0),
        ("scene", 3.0),
        ("shape", 4.0),
        ("trigger", 80.0),
        ("impact", 70.0),
        ("scale", 70.0),
        ("stretch_x", 20.0),
        ("stretch_y", 0.0),
        ("rotation", 50.0),
        ("split", 62.0),
        ("shutter", 62.0),
        ("speed", 0.5),
        ("seed", 3.0),
    ];

    #[test]
    fn standard_checks() {
        testing::check_all(build, BASE);
    }

    #[test]
    fn ms_per_frame() {
        if std::env::var("SCAFFOLD_MEASURE").is_ok() {
            eprintln!("morphform: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
