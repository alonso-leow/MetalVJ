//! `chargefield`: electrostatic field lines on the procedural scaffold
//! (`shared/scaffold.rs`). Behaviour and parameter list follow the community tool
//! "Electrostatic Field v1.0"; the WGSL is original (`shaders/chargefield.wgsl`).
//!
//! Own floats (15): `system_shape density scale flow warp pole_force
//! pole_count path_length spread line_width accent motion_bpm speed drift
//! seed`. Slots: `line`, `accent`.
//!
//! Charges are placed by one of four systems (concentric, parallel,
//! converging, resonance); their integer polarity keeps the stream-function
//! contours (`psi = sum q * atan2`) continuous across branch cuts. Line
//! spacing follows `density`, `path_length` dashes the lines along their run,
//! and `flow` / `drift` / `warp` animate the field.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("system_shape", 0.0, 3.0, 3.0, 0.2),
    spec("density", 6.0, 120.0, 90.0),
    spec("scale", 40.0, 300.0, 275.0),
    spec("flow", 0.0, 1.0, 0.58),
    spec("warp", 0.0, 1.0, 0.42),
    spec("pole_force", 0.0, 1.0, 0.52),
    spec_held("pole_count", 3.0, 96.0, 24.0, 0.2),
    spec("path_length", 0.0, 1.0, 0.56),
    spec("spread", 0.0, 2.0, 0.44),
    spec("line_width", 0.5, 6.0, 1.2),
    spec("accent", 0.0, 1.0, 0.18),
    spec("motion_bpm", 30.0, 180.0, 76.0),
    spec("speed", -2.0, 2.0, 0.5),
    spec("drift_v", 0.0, 2.0, 0.5),
    spec_held("seed", 0.0, 50.0, 3.0, 0.2),
];

/// Line colour: cream, warm white, umber, pale parchment (linear).
const LINE: [f32; 12] = [
    0.82, 0.72, 0.55,
    0.95, 0.88, 0.72,
    0.60, 0.50, 0.38,
    0.90, 0.80, 0.60,
];
/// Accent: ember orange, hot orange, deep rust, amber (linear).
const ACCENT: [f32; 12] = [
    0.84, 0.14, 0.026,
    1.00, 0.30, 0.05,
    0.50, 0.05, 0.01,
    1.00, 0.55, 0.12,
];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "line" => LINE, "ember" => ACCENT);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "chargefield",
    "Chargefield",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/chargefield.wgsl"),
);

crate::scaffold_model!(Chargefield, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Chargefield::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("warp_amount", 0.4),
        ("pulse", 0.0),
        ("flow", 0.7),
        ("warp", 0.55),
        ("pole_force", 0.7),
        ("path_length", 0.7),
        ("spread", 0.65),
        ("accent", 0.45),
        ("drift_v", 0.7),
    ];

    #[test]
    fn standard_checks() {
        testing::check_all(build, BASE);
    }

    #[test]
    fn ms_per_frame() {
        if std::env::var("SCAFFOLD_MEASURE").is_ok() {
            eprintln!("chargefield: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
