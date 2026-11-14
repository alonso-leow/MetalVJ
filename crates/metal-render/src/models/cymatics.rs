//! `cymatics`: a Chladni standing-wave plate, ported from the built-in
//! Chladni sketch's float list (`pattern frequency handle_x handle_y vibration
//! waves scatter grain_amount grain_size speed`, plus the sketch's grain and
//! background colours). The WGSL is original (`shaders/cymatics.wgsl`): a
//! modal sum on a square plate, nodal lines, settling sand grains, rotational
//! symmetry and a slow drift.
//!
//! Own floats (16): the sketch's `pattern` (held enum), `frequency`, `handle_x`,
//! `handle_y`, `vibration`, `waves`, `scatter`, `grain_amount`, `grain_size`,
//! `speed`, plus `rotation symmetry drift glow line_width seed` (the last is a
//! held seed). Slots: `grains`, `nodal`. The background is the scaffold's
//! `fondo` slot.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("pattern", 0.0, 5.0, 0.0, 0.2),
    spec("frequency", 0.5, 24.0, 6.0),
    spec("handle_x", 0.0, 100.0, 50.0),
    spec("handle_y", 0.0, 100.0, 50.0),
    spec("vibration", 0.0, 100.0, 35.0),
    spec("waves", 0.0, 100.0, 30.0),
    spec("scatter", 0.0, 100.0, 15.0),
    spec("grain_amount", 0.0, 100.0, 55.0),
    spec("grain_size", 0.5, 6.0, 1.6),
    spec("speed", -2.0, 2.0, 0.6),
    spec("rotation", 0.0, 1.0, 0.0),
    spec_held("symmetry", 1.0, 8.0, 2.0, 0.2),
    spec("sway", 0.0, 1.0, 0.3),
    spec("glow", 0.0, 1.0, 0.3),
    spec("line_width", 0.0, 1.0, 0.4),
    spec_held("seed", 0.0, 50.0, 7.0, 0.2),
];

/// Sand grains: chalk white, warm ivory, pale sand, a cool blue-white.
const GRAINS: [f32; 12] = [0.95, 0.92, 0.84, 0.86, 0.80, 0.64, 1.0, 0.93, 0.78, 0.72, 0.82, 0.90];
/// Nodal glow: cyan, ice blue, pale gold, white.
const NODAL: [f32; 12] = [0.10, 0.72, 0.88, 0.44, 0.78, 1.0, 1.0, 0.78, 0.35, 1.0, 1.0, 1.0];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "grains" => GRAINS, "nodal" => NODAL);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "cymatics",
    "Cymatics",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/cymatics.wgsl"),
);

crate::scaffold_model!(Cymatics, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Cymatics::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("pattern", 2.0),
        ("frequency", 7.0),
        ("handle_x", 35.0),
        ("handle_y", 62.0),
        ("vibration", 55.0),
        ("waves", 45.0),
        ("scatter", 30.0),
        ("grain_amount", 62.0),
        ("grain_size", 2.8),
        ("speed", 0.7),
        ("rotation", 0.35),
        ("symmetry", 3.0),
        ("sway", 0.5),
        ("glow", 0.5),
        ("line_width", 0.4),
        ("seed", 9.0),
        ("warp_amount", 0.4),
        ("pulse", 0.2),
    ];

    #[test]
    fn standard_checks() {
        testing::check_all(build, BASE);
    }

    #[test]
    fn ms_per_frame() {
        if std::env::var("SCAFFOLD_MEASURE").is_ok() {
            eprintln!("cymatics: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
