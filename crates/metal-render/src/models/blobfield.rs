//! `blobfield`: a grid of merging molecular blobs, ported from the built-in
//! "Molecules" tool. Each cell seeds its own radius, phase and pulsation; the
//! additive gaussian field is normalised and read through the tool's six-stop
//! thermal palette. The WGSL body is an original re-implementation
//! (`shaders/blobfield.wgsl`).
//!
//! Own floats (16): `columns rows symmetry speed variance blob_size softness
//! core_size core_glow snap gain bg_dark bloom_threshold bloom_amount seed
//! palette`. Slots: `cool` (thermal stops 0..3), `hot` (stops 4..5).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("columns", 3.0, 16.0, 6.0, 1.0),
    spec_held("rows", 3.0, 16.0, 6.0, 1.0),
    spec_held("symmetry", 0.0, 3.0, 1.0, 1.0),
    spec("speed", 0.0, 2.0, 1.23),
    spec("variance", 0.0, 2.0, 1.0),
    spec("blob_size", 0.15, 1.0, 0.5),
    spec("softness", 0.3, 1.6, 1.07),
    spec("core_size", 0.05, 0.8, 0.4),
    spec("core_glow", 0.0, 2.5, 0.7),
    spec("snap", 0.4, 3.0, 1.58),
    spec("gain", 0.3, 3.0, 1.38),
    spec("bg_dark", 0.0, 1.0, 0.0),
    spec("bloom_threshold", 0.0, 1.5, 1.0),
    spec("bloom_amount", 0.0, 2.0, 0.0),
    spec_held("seed", 0.0, 1000.0, 0.0, 1.0),
    spec_held("palette", 0.0, 4.0, 0.0, 1.0),
];

/// Thermal stops 0..3, linear: near-black, navy, teal, sand.
const COOL: [f32; 12] = [
    0.00368, 0.00121, 0.00182, 0.00605, 0.01161, 0.03689, 0.03190, 0.10462, 0.09759, 0.70838,
    0.58408, 0.34191,
];
/// Thermal stops 4..5, linear: orange, deep red (the last two repeat because
/// `grad4` always walks four control points).
const HOT: [f32; 12] = [
    0.93011, 0.13287, 0.00439, 0.74540, 0.00561, 0.00212, 0.74540, 0.00561, 0.00212, 0.74540,
    0.00561, 0.00212,
];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "cool" => COOL, "hot" => HOT);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "blobfield",
    "Blobfield",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/blobfield.wgsl"),
);

crate::scaffold_model!(Blobfield, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Blobfield::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("bloom_amount", 0.8),
        ("bloom_threshold", 0.4),
        ("bg_dark", 0.3),
        ("variance", 0.6),
        ("core_glow", 0.8),
        ("softness", 1.1),
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
            eprintln!("blobfield: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
