//! `flutedglass`: A picture seen through ribbed (fluted) glass, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the refraction maths.
//!
//! The picture is a *source*: the `imagen` surface (same mechanism as
//! `corals`' `relleno`). With nothing connected a procedural stripe/gradient
//! test image is distorted instead.
//!
//! Own floats (30): the brief's `sombras luces tam forma(H) angulo
//! distorsion_forma(H) distorsion desplaza estira blur bordes margen grano_mix
//! grano_sup`, plus variability floats: flute wobble (`onda_amp onda_freq
//! onda_vel`), `irregular` (uneven widths), `barrido` (flutes slide),
//! `alterna` (alternating refraction), `cromatico` (colour fringing),
//! `respira respira_vel` (breathing distortion), `facetas` (stepped facets),
//! `reflejo reflejo_vel` (travelling specular streak), `margen_suave`, `tinte`
//! (per-flute tint from the `luz` slot). Slots: `sombra`, `luz`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    // forma: 0 lines, 1 wave, 2 zigzag, 3 grid, 4 petals (radial-ish bend)
    spec_held("shape", 0.0, 4.0, 0.0, 0.2),
    spec("shadows", 0.0, 1.0, 0.25),
    spec("lights", 0.0, 1.0, 0.1),
    spec("size", 0.02, 0.6, 0.12),
    spec("angle", 0.0, 1.0, 0.0),
    // distorsion_forma: 0 prism, 1 lens, 2 contour, 3 cascade, 4 flat
    spec_held("distortion_shape", 0.0, 4.0, 0.0, 0.2),
    spec("distortion", 0.0, 1.5, 0.5),
    spec("displace", -1.0, 1.0, 0.0),
    spec("stretch", 0.0, 3.0, 0.0),
    spec("blur", 0.0, 1.0, 0.0),
    spec("edges", 0.0, 1.0, 0.25),
    spec("margin", 0.0, 0.45, 0.0),
    spec("grain_mix", 0.0, 1.0, 0.0),
    spec("grain_surface", 0.0, 1.0, 0.0),
    spec("wave_amp", 0.0, 1.5, 0.0),
    spec("wave_freq", 0.1, 4.0, 1.0),
    spec("wave_speed", -3.0, 3.0, 0.5),
    spec("irregular", 0.0, 1.0, 0.0),
    spec("sweep", -3.0, 3.0, 0.0),
    spec("alternate", 0.0, 1.0, 0.0),
    spec("chromatic", 0.0, 1.0, 0.0),
    spec("breathe", 0.0, 1.0, 0.0),
    spec("breathe_speed", 0.0, 4.0, 1.0),
    spec("facets", 0.0, 1.0, 0.0),
    spec("reflection", 0.0, 1.0, 0.0),
    spec("reflection_speed", -3.0, 3.0, 0.6),
    spec("margin_soft", 0.0, 0.3, 0.02),
    spec("tint", 0.0, 1.0, 0.0),
    spec("seed", 0.0, 50.0, 0.0),
];

/// Linear values: near-black blue, deep violet, navy, black.
const SOMBRA: [f32; 12] = [0.0, 0.0, 0.0, 0.01, 0.01, 0.04, 0.03, 0.0, 0.08, 0.0, 0.0, 0.0];
/// White, warm white, pale cyan, pale pink.
const LUZ: [f32; 12] = [1.0, 1.0, 1.0, 1.0, 0.85, 0.6, 0.6, 0.9, 1.0, 1.0, 0.7, 0.85];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "shadow" => SOMBRA, "light" => LUZ);

pub static DEF: ScaffoldDef = ScaffoldDef::new_with_source(
    "flutedglass",
    "Fluted Glass",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/flutedglass.wgsl"),
    &["imagen"],
)
.with_material();

crate::scaffold_model!(Flutedglass, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Mapping, Model, SurfaceSpec};
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Flutedglass::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("shadows", 0.6),
        ("lights", 0.5),
        ("blur", 0.3),
        ("edges", 0.5),
        ("margin", 0.1),
        ("displace", 0.3),
        ("stretch", 0.3),
        ("grain_mix", 0.3),
        ("grain_surface", 0.3),
        ("wave_amp", 0.3),
        ("irregular", 0.3),
        ("sweep", 0.5),
        ("alternate", 0.4),
        ("chromatic", 0.4),
        ("breathe", 0.4),
        ("facets", 0.3),
        ("reflection", 0.5),
        ("tint", 0.4),
        ("distortion_shape", 1.0),
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
            eprintln!("flutedglass: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
