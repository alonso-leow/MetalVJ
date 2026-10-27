//! `caustic-weave`: stitched-light caustics on the procedural scaffold
//! (`shared/scaffold.rs`). Behaviour and parameter list follow the community tool
//! "Caustic Stitch" (two endpoint families joined by `multiplier`-indexed
//! chords, with spread, warp, accent stitching and nine shape modes); the WGSL
//! is original (`shaders/caustic_weave.wgsl`).
//!
//! Own floats (15): `mode points multiplier scale spread line_width accent
//! accent_width accent_variations rotation warp speed glow softness seed`.
//! Slots: `stitch` (line colour), `ember` (accent colour).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("mode", 0.0, 8.0, 3.0, 0.0),
    spec_held("points", 12.0, 240.0, 130.0, 0.2),
    spec("multiplier", 1.0, 24.0, 10.0),
    spec("scale", 0.0, 100.0, 80.0),
    spec("spread", 0.0, 200.0, 55.0),
    spec("line_width", 1.0, 100.0, 18.0),
    spec("accent", 0.0, 100.0, 46.0),
    spec("accent_width", 1.0, 100.0, 6.0),
    spec_held("accent_variations", 0.0, 100.0, 0.0, 0.2),
    spec("rotation", 0.0, 360.0, 90.0),
    spec("warp", 0.0, 100.0, 0.0),
    spec("speed", -2.0, 2.0, 0.4),
    spec("glow", 0.0, 1.0, 0.5),
    spec("softness", 0.0, 1.0, 0.3),
    spec_held("seed", 0.0, 50.0, 3.0, 0.2),
];

/// Stitch light: warm cream through amber.
const STITCH: [f32; 12] = [1.00, 0.95, 0.86, 1.00, 0.80, 0.62, 0.95, 0.62, 0.38, 1.00, 0.90, 0.78];
/// Accent embers: deep orange through bright flame.
const EMBER: [f32; 12] = [0.85, 0.10, 0.02, 1.00, 0.28, 0.05, 0.65, 0.05, 0.01, 1.00, 0.42, 0.12];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "stitch" => STITCH, "ember" => EMBER);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "caustic-weave",
    "Caustic Weave",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/caustic_weave.wgsl"),
);

crate::scaffold_model!(CausticWeave, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(CausticWeave::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("warp_amount", 0.4),
        ("pulse", 0.0),
        ("warp", 55.0),
        ("glow", 0.7),
        ("softness", 0.5),
        ("line_width", 25.0),
        ("accent", 65.0),
        ("accent_width", 50.0),
        ("accent_variations", 60.0),
    ];

    #[test]
    fn standard_checks() {
        testing::check_all(build, BASE);
    }

    #[test]
    fn ms_per_frame() {
        if std::env::var("SCAFFOLD_MEASURE").is_ok() {
            eprintln!("caustic-weave: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
