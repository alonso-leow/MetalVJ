//! `ringflow`: concentric sliced rings from a two-focus (Cassini) distance
//! field, on the procedural scaffold (`shared/scaffold.rs`). Behaviour and parameter
//! list follow the sketch-tool "Cassini Flow"; the WGSL is original
//! (`shaders/ringflow.wgsl`), evaluating the field per pixel rather than
//! clipping polygons.
//!
//! Own floats (12): `mode seed core_separation slices scale warp drift
//! slice_width accent rotation speed time`. Slots: `slice`, `accent`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("mode", 0.0, 5.0, 0.0, 0.1),
    spec_held("seed", 0.0, 100.0, 17.0, 0.2),
    spec("core_separation", 0.0, 100.0, 58.0),
    spec("slices", 2.0, 36.0, 14.0),
    spec("scale", 0.0, 100.0, 46.0),
    spec("warp", 0.0, 100.0, 22.0),
    spec("drift_v", 0.0, 100.0, 25.0),
    spec("slice_width", 5.0, 100.0, 62.0),
    spec("accent", 0.0, 100.0, 32.0),
    spec("rotation", 0.0, 360.0, 0.0),
    spec("speed", 0.0, 3.0, 0.6),
    spec("time", 0.0, 60.0, 0.0),
];

/// Slice cream: mid tone, highlight, shadow, deep shadow (linear).
const SLICE: [f32; 12] = [0.807, 0.723, 0.577, 0.950, 0.880, 0.760, 0.550, 0.470, 0.360, 0.320, 0.270, 0.200];
/// Accent green: mid tone, bright, dark, pale (linear).
const ACCENT: [f32; 12] = [0.144, 0.342, 0.110, 0.300, 0.550, 0.220, 0.070, 0.180, 0.060, 0.500, 0.680, 0.340];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "slice" => SLICE, "accent" => ACCENT);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "ringflow",
    "Ringflow",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/ringflow.wgsl"),
);

crate::scaffold_model!(Ringflow, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Ringflow::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("mode", 0.0),
        ("seed", 17.0),
        ("core_separation", 58.0),
        ("slices", 14.0),
        ("scale", 46.0),
        ("warp", 22.0),
        ("drift_v", 25.0),
        ("slice_width", 62.0),
        ("accent", 32.0),
        ("rotation", 0.0),
        ("speed", 0.6),
        ("time", 0.0),
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
            eprintln!("ringflow: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
