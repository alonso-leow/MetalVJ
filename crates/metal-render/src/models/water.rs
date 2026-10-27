//! `water`: A picture seen through caustic, rippling water, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the refraction maths.
//!
//! The picture is a *source*: the `imagen` surface (same mechanism as
//! `corals`' `relleno`); with nothing connected a procedural test image is
//! refracted instead.
//!
//! Own floats (27): the brief's `resaltes capas bordes olas caustica tam`, plus
//! wave heading and fan (`dir dir_var`), speed `vel`, `refrac`, `pliegue`
//! (crest sharpness), `turbulencia` (domain warp), `marea marea_vel` (swell),
//! `ondulas ondulas_freq` (fine ripples), `gotas gotas_vel` (rain rings),
//! `prof` (depth tint), `cromatico`, `reflejo reflejo_dur` (sun glint),
//! `espuma espuma_umbral`, `destellos`, `claro` (see-through, so the
//! background shows), `semilla`. Slots: `agua` (depth tint), `luz` (highlights,
//! foam, glints).

use crate::model::{spec, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("highlights", 0.0, 1.0, 0.15),
    spec("layers", 0.0, 1.0, 0.5),
    spec("edges", 0.0, 1.0, 0.8),
    spec("waves", 0.0, 1.0, 0.3),
    spec("caustic", 0.0, 1.0, 0.3),
    spec("size", 0.1, 4.0, 1.0),
    spec("direction", 0.0, 1.0, 0.1),
    spec("direction_variation", 0.0, 1.0, 0.5),
    spec("speed", -3.0, 3.0, 1.0),
    spec("refract", 0.0, 2.0, 1.0),
    spec("fold", 0.0, 1.0, 0.3),
    spec("turbulence", 0.0, 1.0, 0.25),
    spec("tide", 0.0, 1.0, 0.0),
    spec("tide_speed", -2.0, 2.0, 0.4),
    spec("ripples", 0.0, 1.0, 0.0),
    spec("ripples_freq", 1.0, 20.0, 8.0),
    spec("drops", 0.0, 1.0, 0.0),
    spec("drops_speed", 0.0, 3.0, 1.0),
    spec("depth", 0.0, 1.0, 0.3),
    spec("chromatic", 0.0, 1.0, 0.0),
    spec("reflection", 0.0, 1.0, 0.0),
    spec("reflection_duration", 0.0, 1.0, 0.5),
    spec("foam", 0.0, 1.0, 0.0),
    spec("foam_threshold", 0.3, 1.2, 0.7),
    spec("flashes", 0.0, 1.0, 0.0),
    spec("clear", 0.0, 1.0, 0.0),
    spec("seed", 0.0, 50.0, 0.0),
];

/// Linear values: deep navy, teal, cyan, pale aqua.
const AGUA: [f32; 12] = [0.0, 0.02, 0.08, 0.0, 0.12, 0.2, 0.05, 0.4, 0.5, 0.35, 0.7, 0.75];
/// White, pale cyan, warm white, near white.
const LUZ: [f32; 12] = [1.0, 1.0, 1.0, 0.7, 0.95, 1.0, 1.0, 0.95, 0.8, 0.9, 0.95, 1.0];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "agua" => AGUA, "light" => LUZ);

pub static DEF: ScaffoldDef = ScaffoldDef::new_with_source(
    "water",
    "Water",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/water.wgsl"),
    &["imagen"],
);

crate::scaffold_model!(Water, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Mapping, Model, SurfaceSpec};
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Water::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("tide", 0.4),
        ("ripples", 0.4),
        ("drops", 0.5),
        ("chromatic", 0.4),
        ("reflection", 0.5),
        ("foam", 0.5),
        ("flashes", 0.5),
        ("clear", 0.4),
        ("fold", 0.4),
        ("depth", 0.5),
        ("warp_amount", 0.4),
        ("pulse", 0.0),
    ];

    #[test]
    fn standard_checks() {
        testing::check_all(build, BASE);
    }

    #[test]
    fn declares_one_source_surface() {
        assert_eq!(build().surfaces(), &["imagen"]);
    }

    #[test]
    fn a_connected_source_changes_the_frame() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = build().schema().defaults();
        let none = testing::frame(&gpu, &mut *build(), &f, &[0.5]);
        let mut m = build();
        let px: Vec<u8> = (0..16).flat_map(|i| [(i * 16) as u8, 200, 30, 255]).collect();
        m.set_surface("imagen", Some(SurfaceSpec::image(&px, 4, 4, 1.0, Mapping::Projector, 1.0)));
        let with = testing::frame(&gpu, &mut *m, &f, &[0.5]);
        assert_ne!(none, with);
        m.set_surface("imagen", None);
        assert_eq!(none, testing::frame(&gpu, &mut *m, &f, &[0.5]));
    }

    #[test]
    fn ms_per_frame() {
        if std::env::var("SCAFFOLD_MEASURE").is_ok() {
            eprintln!("water: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
