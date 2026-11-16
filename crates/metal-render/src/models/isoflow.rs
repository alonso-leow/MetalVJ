//! `isoflow`: warped contour bands with variable line thickness, layer shadows
//! and an optional typographic mark layer, on the procedural scaffold
//! (`shared/scaffold.rs`). Behaviour and parameter list follow the community
//! "Fluid Contours" sketch; the WGSL is original (`shaders/isoflow.wgsl`).
//!
//! A domain-warped sine/cosine field is quantised into bands. The fractional
//! part of the field inside each band is the ink, with a per-band width from a
//! hashed sine, so lines vary in thickness; `spacing` opens a paper gap on the
//! inside of every band. `palette` selects between a two-colour ink/accent
//! scheme (with large red islands) and a five-colour cycling palette. `shadow`
//! adds a soft cast shadow and a lifted shine on the band lip; `lettering`
//! replaces every `text_every`-th band with a dotted stroke of ink, and
//! `margin` masks the outer paper border.
//!
//! Own floats (12): `density size warp speed spacing irregularity shadow seed
//! palette lettering text_every margin`. Slots: `paper_ink`, `accent_violet`,
//! `violet_blue`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("density", 2.0, 18.0, 7.0),
    spec("size", 0.45, 2.5, 1.0),
    spec("warp", 0.0, 2.5, 1.1),
    spec("speed", 0.0, 2.0, 0.22),
    spec("spacing", 0.0, 0.9, 0.12),
    spec("irregularity", 0.0, 1.0, 0.32),
    spec("shadow", 0.0, 1.0, 0.0),
    spec_held("seed", 0.0, 999.0, 23.0, 0.2),
    spec_held("palette", 0.0, 1.0, 0.0, 0.0),
    spec_held("lettering", 0.0, 2.0, 0.0, 0.0),
    spec_held("text_every", 2.0, 12.0, 5.0, 0.0),
    spec("margin", 0.0, 0.15, 0.035),
];

/// Paper -> ink: stops 0/1 are paper, stops 2/3 are ink.
const PAPER_INK: [f32; 12] =
    [0.760, 0.702, 0.598, 0.760, 0.702, 0.598, 0.006, 0.007, 0.005, 0.006, 0.007, 0.005];
/// Accent -> violet.
const ACCENT_VIOLET: [f32; 12] =
    [0.890, 0.040, 0.025, 0.890, 0.040, 0.025, 0.475, 0.262, 0.525, 0.475, 0.262, 0.525];
/// Violet -> blue.
const VIOLET_BLUE: [f32; 12] =
    [0.475, 0.262, 0.525, 0.475, 0.262, 0.525, 0.002, 0.148, 0.283, 0.002, 0.148, 0.283];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "paper_ink" => PAPER_INK, "accent_violet" => ACCENT_VIOLET, "violet_blue" => VIOLET_BLUE);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "isoflow",
    "Isoflow",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/isoflow.wgsl"),
);

crate::scaffold_model!(Isoflow, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Isoflow::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("warp_amount", 0.4),
        ("pulse", 0.0),
        ("shadow", 0.4),
        ("lettering", 1.0),
    ];

    #[test]
    fn standard_checks() {
        testing::check_all(build, BASE);
    }

    #[test]
    fn ms_per_frame() {
        if std::env::var("SCAFFOLD_MEASURE").is_ok() {
            eprintln!("isoflow: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
