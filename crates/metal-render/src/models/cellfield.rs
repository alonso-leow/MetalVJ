//! `cellfield`: drifting cellular field with membrane borders and core dots,
//! on the procedural scaffold (`shared/scaffold.rs`). Behaviour and parameter list
//! follow the Sketch "Cellular Field" tool; the WGSL is original
//! (`shaders/cellfield.wgsl`).
//!
//! Own floats (13): `bpm cells core inner_gap membrane warp warp_scale
//! warp_drift warp_complexity speed grain vignette seed`. Slots: `core`,
//! `membrane`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("bpm", 20.0, 240.0, 120.0),
    spec("cells", 0.0, 50.0, 10.0),
    spec("core", 0.0, 3.0, 0.4),
    spec("inner_gap", 0.0, 100.0, 25.0),
    spec("membrane", 0.0, 200.0, 5.0),
    spec("warp", 0.0, 1.0, 0.15),
    spec("warp_scale", 0.0, 1.0, 0.35),
    spec("warp_drift", 0.0, 1.0, 0.3),
    spec("warp_complexity", 0.0, 1.0, 0.35),
    spec("speed", -2.0, 2.0, 1.0),
    spec("grain", 0.0, 0.1, 0.02),
    spec("vignette", 0.0, 1.0, 0.7),
    spec_held("seed", 0.0, 50.0, 3.0, 0.2),
];

/// Linear values: moss, leaf, deep green, pale green.
const CORE: [f32; 12] = [0.246, 0.473, 0.174, 0.350, 0.600, 0.220, 0.180, 0.360, 0.120, 0.550, 0.750, 0.350];
/// Linear values: ember, orange, rust, amber.
const MEMBRANE: [f32; 12] = [0.840, 0.147, 0.026, 0.900, 0.250, 0.040, 0.700, 0.100, 0.020, 1.000, 0.450, 0.100];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "core_color" => CORE, "edge" => MEMBRANE);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "cellfield",
    "Cellfield",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/cellfield.wgsl"),
);

crate::scaffold_model!(Cellfield, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Cellfield::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("cells", 30.0),
        ("core", 1.5),
        ("inner_gap", 40.0),
        ("membrane", 40.0),
        ("warp", 0.5),
        ("warp_scale", 0.5),
        ("warp_drift", 0.5),
        ("warp_complexity", 0.5),
        ("grain", 0.05),
        ("vignette", 0.5),
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
            eprintln!("cellfield: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
