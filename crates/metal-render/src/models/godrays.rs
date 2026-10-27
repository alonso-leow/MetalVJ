//! `godrays`: Light shafts fanning from a point with a core glow and a bloom halo, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the radial-sampling maths.
//!
//! Own floats (24): the brief's `bloom intensidad densidad moteado med_tam
//! med_int`, plus `velocidad origen_x origen_y rayos anchura largo curvatura
//! vaiven parpadeo abanico dir estrias pasillo halo_radio halo_pulso
//! aberracion polvo mezcla_color`. Slots: `rayos` (shafts), `bloom` (halo),
//! `nucleo` (core glow).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("bloom", 0.0, 2.0, 0.4),
    spec("intensity", 0.0, 2.0, 0.8),
    spec("density", 0.0, 1.0, 0.3),
    spec("speckle", 0.0, 1.0, 0.3),
    spec("medium_size", 0.0, 1.0, 0.2),
    spec("medium_intensity", 0.0, 2.0, 0.4),
    spec("speed", -3.0, 3.0, 0.75),
    spec("origin_x", -0.9, 0.9, 0.0),
    spec("origin_y", -0.9, 0.9, -0.3),
    spec_held("rays", 2.0, 40.0, 10.0, 0.2),
    spec("width", 0.0, 1.0, 0.3),
    spec("long", 0.0, 1.0, 0.5),
    spec("curvature", -1.5, 1.5, 0.0),
    spec("swing", 0.0, 1.0, 0.0),
    spec("blink", 0.0, 1.0, 0.0),
    spec_held("fan", 0.05, 1.0, 1.0, 0.2),
    spec("direction", 0.0, 1.0, 0.25),
    spec("streaks", 0.0, 1.0, 0.0),
    spec("corridor", 0.0, 1.0, 0.0),
    spec("halo_radius", 0.0, 1.0, 0.25),
    spec("halo_pulse", 0.0, 1.0, 0.0),
    spec("aberration", 0.0, 1.0, 0.0),
    spec("dust", 0.0, 1.0, 0.0),
    spec("mix_color", 0.0, 1.0, 0.3),
];

/// Linear values: white, warm yellow, orange, pink.
const RAYOS: [f32; 12] = [1.00, 1.00, 1.00, 1.00, 0.75, 0.10, 1.00, 0.25, 0.01, 1.00, 0.10, 0.15];
/// Bloom halo: blue, violet, magenta, deep blue.
const BLOOM: [f32; 12] = [0.00, 0.00, 1.00, 0.25, 0.05, 0.80, 0.80, 0.05, 0.50, 0.00, 0.40, 0.00];
/// Core: white, cream, amber, orange.
const NUCLEO: [f32; 12] = [1.00, 1.00, 1.00, 1.00, 0.90, 0.60, 1.00, 0.55, 0.10, 1.00, 0.25, 0.02];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "rays" => RAYOS, "bloom" => BLOOM, "core" => NUCLEO);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "godrays",
    "God Rays",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/godrays.wgsl"),
);

crate::scaffold_model!(Godrays, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Godrays::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("curvature", 0.4),
        ("swing", 0.4),
        ("blink", 0.3),
        ("fan", 0.6),
        ("streaks", 0.4),
        ("corridor", 0.3),
        ("halo_pulse", 0.4),
        ("aberration", 0.4),
        ("dust", 0.5),
        ("speckle", 0.5),
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
            eprintln!("godrays: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
