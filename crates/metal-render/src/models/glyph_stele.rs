//! `glyph-stele`: a monolith of lettering that is sung, eroded and cracked,
//! on the procedural scaffold (`shared/scaffold.rs`). Behaviour and parameter list
//! follow the sketch-tool "Monolito de Letras"; the WGSL is original
//! (`shaders/glyph_stele.wgsl`). Glyphs are procedural 3x5 stroked blocks, not
//! a font or a texture, so the model needs no source input.
//!
//! Own floats (21): `cell_size monolith margin speed threshold crack_len
//! displace size_var seed graves medios agudos stroke density future_alpha
//! dots head flicker grain soft accent`. Slots: `glyph`, `wound` (accent),
//! `slab` (the stone).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("cell_size", 0.02, 0.25, 0.075),
    spec("monolith", 0.2, 1.0, 0.86),
    spec("margin", 0.0, 0.25, 0.06),
    spec("speed", -2.0, 2.0, 0.5),
    spec("threshold", 0.0, 0.95, 0.5),
    spec("crack_len", 0.0, 12.0, 4.0),
    spec("displace", 0.0, 1.0, 0.55),
    spec("size_var", 0.0, 1.0, 0.6),
    spec_held("seed", 0.0, 50.0, 0.0, 0.2),
    spec("graves", 0.0, 1.0, 0.35),
    spec("medios", 0.0, 1.0, 0.3),
    spec("agudos", 0.0, 1.0, 0.3),
    spec("stroke", 0.05, 0.95, 0.34),
    spec("density", 0.0, 1.0, 0.75),
    spec("future_alpha", 0.0, 1.0, 0.3),
    spec("dots", 0.0, 1.0, 0.6),
    spec("head", 0.0, 1.0, 0.9),
    spec("flicker", 0.0, 1.0, 0.2),
    spec("grain", 0.0, 1.0, 0.15),
    spec("soft", 0.0, 1.0, 0.2),
    spec("accent", 0.0, 1.0, 0.9),
];

/// Linear values: lead grey, chalk white.
const GLYPH: [f32; 12] = [0.22, 0.23, 0.26, 0.36, 0.37, 0.40, 0.55, 0.56, 0.58, 0.78, 0.78, 0.80];
/// Linear values: deep orange to ember.
const WOUND: [f32; 12] = [0.80, 0.04, 0.01, 1.00, 0.10, 0.01, 1.00, 0.22, 0.03, 1.00, 0.40, 0.08];
/// Linear values: near black to dark stone.
const SLAB: [f32; 12] = [0.010, 0.010, 0.014, 0.026, 0.026, 0.034, 0.055, 0.055, 0.066, 0.100, 0.100, 0.115];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "glyph" => GLYPH, "wound" => WOUND, "slab" => SLAB);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "glyph-stele",
    "Glyph Stele",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/glyph_stele.wgsl"),
);

crate::scaffold_model!(GlyphStele, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(GlyphStele::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("cell_size", 0.065),
        ("monolith", 0.80),
        ("margin", 0.08),
        ("speed", 0.60),
        ("threshold", 0.35),
        ("crack_len", 6.0),
        ("displace", 0.50),
        ("size_var", 0.60),
        ("seed", 3.0),
        ("graves", 0.60),
        ("medios", 0.50),
        ("agudos", 0.60),
        ("stroke", 0.40),
        ("density", 0.70),
        ("future_alpha", 0.50),
        ("dots", 0.60),
        ("head", 0.80),
        ("flicker", 0.40),
        ("grain", 0.30),
        ("soft", 0.40),
        ("accent", 0.80),
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
            eprintln!("glyph-stele: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
