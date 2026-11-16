//! `shatter-mask`: a fractured quadrilateral grid whose surviving shards are
//! filled over a coloured field, on the procedural scaffold (`shared/scaffold.rs`).
//! Behaviour and parameter list follow the "Fractured Mask" community sketch;
//! the WGSL is original (`shaders/shatter_mask.wgsl`).
//!
//! Own floats (16): `layout` (held: natural / columns / rows), `seed` (held),
//! `density cut_asymmetry coverage_distribution fragment_fill fill_opacity`,
//! plus the scaffold's `speed glide scale offset_x offset_y rotation wobble
//! crack_width sheen`. Slots: `fragment` (the mask shards), `field` (the
//! colour behind them) and `seam` (crack/rim light).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("layout", 0.0, 2.0, 0.0, 0.15),
    spec_held("seed", 0.0, 100.0, 18.0, 0.2),
    spec("density", 0.0, 1.0, 1.0),
    spec("cut_asymmetry", 0.0, 1.0, 0.42),
    spec("coverage_distribution", 0.0, 1.0, 0.74),
    spec("fragment_fill", 0.0, 1.0, 1.0),
    spec("fill_opacity", 0.0, 1.0, 1.0),
    spec("speed", -2.0, 2.0, 0.35),
    spec("glide", 0.0, 1.0, 0.12),
    spec("scale", 0.3, 4.0, 1.0),
    spec("offset_x", -1.0, 1.0, 0.0),
    spec("offset_y", -1.0, 1.0, 0.0),
    spec("rotation", -1.0, 1.0, 0.0),
    spec("wobble", 0.0, 1.0, 0.0),
    spec("crack_width", 0.0, 0.5, 0.0),
    spec("sheen", 0.0, 1.0, 0.0),
];

/// Shards: linear near-black brown, four stops.
const FRAGMENT: [f32; 12] = [
    0.010, 0.007, 0.005,
    0.016, 0.011, 0.008,
    0.024, 0.016, 0.011,
    0.035, 0.022, 0.014,
];
/// Field behind the shards: linear ember orange, four stops.
const FIELD: [f32; 12] = [
    0.75, 0.11, 0.018,
    0.84, 0.15, 0.026,
    0.90, 0.20, 0.040,
    1.00, 0.28, 0.060,
];
/// Seam / rim: linear warm cream to amber.
const SEAM: [f32; 12] = [
    0.55, 0.20, 0.045,
    0.72, 0.30, 0.080,
    0.88, 0.42, 0.130,
    1.00, 0.55, 0.190,
];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "fragment" => FRAGMENT, "field" => FIELD, "seam" => SEAM);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "shatter-mask",
    "Shatter Mask",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/shatter_mask.wgsl"),
);

crate::scaffold_model!(ShatterMask, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(ShatterMask::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("layout", 1.0),
        ("density", 0.65),
        ("cut_asymmetry", 0.5),
        ("coverage_distribution", 0.5),
        ("fragment_fill", 0.8),
        ("fill_opacity", 0.8),
        ("speed", 0.6),
        ("glide", 0.35),
        ("scale", 1.3),
        ("offset_x", 0.05),
        ("offset_y", -0.03),
        ("rotation", 0.08),
        ("wobble", 0.4),
        ("crack_width", 0.12),
        ("sheen", 0.5),
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
            eprintln!("shatter-mask: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
