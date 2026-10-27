//! `halftonedots`: A picture as a halftone dot screen, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the screen maths.
//!
//! The picture is a *source* (`imagen` surface, as in `flutedglass`);
//! unconnected, the procedural test image is screened instead.
//!
//! Own floats (27): the brief's `colores_orig tipo(H) invierte grid(H) tam
//! radio contraste_punto` (renamed: LOOK owns `contraste`) `grano_mix grano_sup
//! grano_tam`, plus variability floats: `angulo` (screen angle), `jitter_pos`,
//! `jitter_tam`, `nitidez`, `tinte` (per-dot palette pick), `umbral`, `ola`
//! `ola_freq ola_vel ola_dir` (size wave), `respira respira_vel`, `lente`,
//! `vel_x vel_y` (lattice scroll), `semilla`. Slot: `punto`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("colors_origin", 0.0, 1.0, 0.0),
    // tipo: 0 classic, 1 gooey, 2 holes, 3 soft
    spec_held("type", 0.0, 3.0, 0.0, 0.2),
    spec("invert", 0.0, 1.0, 0.0),
    // grid: 0 square, 1 hex
    spec_held("grid", 0.0, 1.0, 0.0, 0.2),
    spec("size", 0.01, 0.2, 0.04),
    spec("radius", 0.2, 1.6, 1.0),
    spec("contrast_dot", 0.0, 1.0, 0.3),
    spec("grain_mix", 0.0, 1.0, 0.0),
    spec("grain_surface", 0.0, 1.0, 0.0),
    spec("grain_size", 0.2, 3.0, 1.0),
    spec("angle", 0.0, 1.0, 0.0),
    spec("jitter_pos", 0.0, 1.0, 0.0),
    spec("jitter_size", 0.0, 1.0, 0.0),
    spec("sharpness", 0.0, 1.0, 0.8),
    spec("tint", 0.0, 1.0, 0.0),
    spec("threshold", 0.0, 0.9, 0.0),
    spec("wave", 0.0, 1.0, 0.0),
    spec("wave_freq", 0.1, 4.0, 1.0),
    spec("wave_speed", -3.0, 3.0, 0.6),
    spec("wave_direction", 0.0, 1.0, 0.1),
    spec("breathe", 0.0, 1.0, 0.0),
    spec("breathe_speed", 0.0, 4.0, 1.0),
    spec("lens", 0.0, 2.0, 0.0),
    spec("speed_x", -2.0, 2.0, 0.0),
    spec("speed_y", -2.0, 2.0, 0.0),
    spec("seed", 0.0, 50.0, 0.0),
];

/// Linear values: warm white, cyan, pink, amber.
const PUNTO: [f32; 12] = [0.90, 0.88, 0.80, 0.10, 0.70, 0.90, 0.90, 0.15, 0.50, 0.95, 0.60, 0.10];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "punto" => PUNTO);

pub static DEF: ScaffoldDef = ScaffoldDef::new_with_source(
    "halftonedots",
    "Halftone Dots",
    &SCHEMA,
    FLOATS,
    1,
    include_str!("../../shaders/halftonedots.wgsl"),
    &["imagen"],
);

crate::scaffold_model!(Halftonedots, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Mapping, Model, SurfaceSpec};
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Halftonedots::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("colors_origin", 0.5),
        ("invert", 0.3),
        ("grain_mix", 0.3),
        ("grain_surface", 0.3),
        ("jitter_pos", 0.3),
        ("jitter_size", 0.3),
        ("tint", 0.4),
        ("threshold", 0.2),
        ("wave", 0.4),
        ("breathe", 0.4),
        ("lens", 0.4),
        ("speed_x", 0.5),
        ("type", 3.0),
        ("grid", 1.0),
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
            eprintln!("halftonedots: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
