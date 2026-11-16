//! `hairlines`: a family of curved strands around a head anchor, on the paper
//! scaffold (`shared/scaffold.rs`). Behaviour and parameter list follow the
//! community Canvas2D tool "Homer's Hair"; the WGSL is original
//! (`shaders/hairlines.wgsl`). Instead of porting draw calls, the shader
//! evaluates the distance to the animated strands per pixel.
//!
//! Own floats (14): `system repetitions spacing displacement phase_offset
//! scale_step rotation_step curve_tension warp propagation line_width
//! motion_bpm speed seed`. Slots: `strands` (the hair line), `root` (the head
//! anchor glow), `sheen` (the bright core / halo of a line).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("system", 0.0, 3.0, 3.0, 0.2),
    spec_held("repetitions", 2.0, 100.0, 24.0, 0.2),
    spec("spacing", 0.0, 200.0, 58.0),
    spec("displacement", 0.0, 200.0, 54.0),
    spec("phase_offset", 0.0, 360.0, 18.0),
    spec("scale_step", -8.0, 8.0, 0.8),
    spec("rotation_step", -30.0, 30.0, 2.2),
    spec("curve_tension", 0.0, 400.0, 168.0),
    spec("warp", 0.0, 200.0, 34.0),
    spec("propagation", 0.0, 200.0, 70.0),
    spec("line_width", 0.5, 6.0, 1.2),
    spec("motion_bpm", 0.0, 260.0, 72.0),
    spec("speed", -3.0, 3.0, 1.0),
    spec_held("seed", 0.0, 50.0, 0.0, 0.2),
];

/// Strand: warm cream.
const STRANDS: [f32; 12] = [
    0.807, 0.723, 0.578, 0.807, 0.723, 0.578, 0.807, 0.723, 0.578, 0.807, 0.723, 0.578,
];
/// Root glow: pale gold to ember.
const ROOT: [f32; 12] = [
    1.00, 0.72, 0.35, 1.00, 0.86, 0.55, 0.95, 0.60, 0.25, 1.00, 0.80, 0.42,
];
/// Sheen: cold white to a faint blue rim.
const SHEEN: [f32; 12] = [
    1.00, 0.98, 0.92, 0.90, 0.95, 1.00, 1.00, 0.90, 0.80, 0.85, 0.92, 1.00,
];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "strands" => STRANDS, "root" => ROOT, "sheen" => SHEEN);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "hairlines",
    "Hairlines",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/hairlines.wgsl"),
)
.with_render_scale(0.6);

crate::scaffold_model!(Hairlines, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Hairlines::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("system", 2.0),
        ("repetitions", 20.0),
        ("line_width", 2.0),
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
            eprintln!("hairlines: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
