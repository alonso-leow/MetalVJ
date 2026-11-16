//! `concrete`: a brutalist facade on the procedural scaffold (`shared/scaffold.rs`).
//! The layout, palette and motion follow the built-in Brutalist tool (a grid of
//! blocks/slabs with window-shaped openings, fill/outline and rotation); the
//! WGSL is original (`shaders/concrete.wgsl`).
//!
//! Own floats (29): `shape columns rows speed spacing fill stroke
//! rotation_speed block_w block_h slab windows window_w window_h window_cols
//! window_rows grid grid_width bevel relief grime granular light accent
//! palette open glow sway seed`. Slots: `blocks`, `windows`, `accent`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("shape", 0.0, 8.0, 8.0, 0.2),
    spec("columns", 2.0, 24.0, 8.0),
    spec("rows", 2.0, 20.0, 6.0),
    spec("speed", -2.0, 2.0, 0.35),
    spec("spacing", 0.0, 0.65, 0.12),
    spec_held("fill", 0.0, 1.0, 1.0, 0.2),
    spec("stroke", 0.005, 0.6, 0.18),
    spec("rotation_speed", 0.0, 3.0, 0.35),
    spec("block_w", 0.0, 1.0, 0.86),
    spec("block_h", 0.0, 1.0, 0.72),
    spec("slab", 0.0, 1.0, 0.35),
    spec("windows", 0.0, 1.0, 0.75),
    spec("window_w", 0.0, 1.0, 0.48),
    spec("window_h", 0.0, 1.0, 0.55),
    spec("window_cols", 0.0, 1.0, 0.35),
    spec("window_rows", 0.0, 1.0, 0.50),
    spec("grid", 0.0, 1.0, 0.55),
    spec("grid_width", 0.0, 1.0, 0.14),
    spec("bevel", 0.0, 1.0, 0.50),
    spec("relief", 0.0, 1.0, 0.45),
    spec("grime", 0.0, 1.0, 0.35),
    spec("granular", 0.0, 1.0, 0.22),
    spec("light", 0.0, 1.0, 0.60),
    spec("accent", 0.0, 1.0, 0.40),
    spec("palette", 0.0, 1.0, 0.50),
    spec("open", 0.0, 1.0, 0.20),
    spec("glow", 0.0, 1.0, 0.35),
    spec("sway", 0.0, 1.0, 0.30),
    spec_held("seed", 0.0, 50.0, 3.0, 0.2),
];

/// Blocks: pale concrete, mid grey, shadowed stone, warm plaster.
const BLOQUES: [f32; 12] =
    [0.62, 0.58, 0.50, 0.42, 0.39, 0.33, 0.23, 0.21, 0.18, 0.76, 0.72, 0.62];
/// Windows: black glass, teal glass, amber light, pale blue light.
const VENTANAS: [f32; 12] =
    [0.006, 0.008, 0.012, 0.015, 0.040, 0.055, 0.85, 0.48, 0.12, 0.30, 0.50, 0.58];
/// Accent: cream, rust, charcoal, warm white.
const ACENTO: [f32; 12] =
    [0.82, 0.76, 0.62, 0.40, 0.16, 0.06, 0.05, 0.05, 0.045, 0.95, 0.92, 0.82];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "blocks" => BLOQUES, "windows" => VENTANAS, "accent" => ACENTO);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "concrete",
    "Concrete",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/concrete.wgsl"),
);

crate::scaffold_model!(Concrete, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Concrete::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("shape", 8.0),
        ("columns", 8.0),
        ("rows", 6.0),
        ("speed", 0.4),
        ("spacing", 0.12),
        ("fill", 1.0),
        ("stroke", 0.2),
        ("rotation_speed", 0.5),
        ("block_w", 0.8),
        ("block_h", 0.75),
        ("slab", 0.4),
        ("windows", 0.8),
        ("window_w", 0.5),
        ("window_h", 0.55),
        ("window_cols", 0.5),
        ("window_rows", 0.55),
        ("grid", 0.6),
        ("grid_width", 0.2),
        ("bevel", 0.5),
        ("relief", 0.5),
        ("grime", 0.4),
        ("granular", 0.3),
        ("light", 0.6),
        ("accent", 0.5),
        ("palette", 0.5),
        ("open", 0.25),
        ("glow", 0.5),
        ("sway", 0.5),
        ("seed", 7.0),
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
            eprintln!("concrete: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
