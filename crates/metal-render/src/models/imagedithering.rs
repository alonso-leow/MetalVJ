//! `imagedithering`: The source picture (`imagen` surface, procedural fallback when unconnected) quantised to a few levels through a threshold map, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the threshold maths.
//!
//! Own floats (25): the brief's `colores_orig invierte tipo(H) tam
//! pasos_color(H)` (tipo: random, 2x2, 4x4, 8x8, blue-ish, error-diffusion
//! style, clustered dot, lines), plus `umbral curva realce ruido ruido_vel
//! giro_malla desplaza_x desplaza_y difusion serpentina cromatico ola ola_freq
//! ola_vel bloque rejilla tinte vacio solarizar semilla`.
//! Slots: `tinta` (level ramp), `acento` (alternate ramp per cell).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    // tipo: 0 random, 1 2x2, 2 4x4, 3 8x8, 4 blue-ish, 5 diffusion, 6 clustered, 7 lines
    spec("colors_origin", 0.0, 1.0, 0.0),
    spec_held("invert", 0.0, 1.0, 0.0, 0.1),
    spec_held("type", 0.0, 7.0, 2.0, 0.15),
    spec("size", 0.3, 20.0, 3.0),
    spec_held("steps_color", 2.0, 8.0, 4.0, 0.15),
    spec("threshold", -0.5, 0.5, 0.0),
    spec("curve", 0.3, 3.0, 1.0),
    spec("highlight", 0.0, 1.5, 0.0),
    spec("noise", 0.0, 1.0, 0.0),
    spec("noise_speed", 0.0, 30.0, 8.0),
    spec("turn_mesh", -1.57, 1.57, 0.0),
    spec("displace_x", -3.0, 3.0, 0.0),
    spec("displace_y", -3.0, 3.0, 0.0),
    spec("diffusion", 0.0, 1.0, 0.0),
    spec("serpentine", 0.0, 1.0, 0.0),
    spec("chromatic", 0.0, 1.0, 0.25),
    spec("wave", 0.0, 1.0, 0.0),
    spec("wave_freq", 0.1, 6.0, 1.5),
    spec("wave_speed", -3.0, 3.0, 0.6),
    spec("block", 0.0, 1.0, 1.0),
    spec("grid", 0.0, 1.0, 0.0),
    spec("tint", 0.0, 1.0, 0.2),
    spec("empty", 0.0, 1.0, 0.0),
    spec("solarize", 0.0, 1.0, 0.0),
    spec("seed", 0.0, 50.0, 0.0),
];

/// Linear values: deep ink-blue to warm paper white.
const TINTA: [f32; 12] = [0.01, 0.01, 0.05, 0.08, 0.10, 0.35, 0.45, 0.55, 0.80, 1.0, 0.92, 0.75];
/// Linear values: dark teal to hot orange.
const ACENTO: [f32; 12] = [0.0, 0.03, 0.04, 0.05, 0.30, 0.30, 0.90, 0.35, 0.08, 1.0, 0.75, 0.30];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "ink" => TINTA, "accent" => ACENTO);

pub static DEF: ScaffoldDef = ScaffoldDef::new_with_source(
    "imagedithering",
    "Image Dithering",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/imagedithering.wgsl"),
    &["imagen"],
);

crate::scaffold_model!(Imagedithering, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Mapping, Model, SurfaceSpec};
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Imagedithering::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("colors_origin", 0.5),
        ("highlight", 0.5),
        ("noise", 0.3),
        ("diffusion", 0.5),
        ("serpentine", 0.5),
        ("chromatic", 0.5),
        ("wave", 0.4),
        ("block", 0.7),
        ("grid", 0.3),
        ("tint", 0.4),
        ("empty", 0.5),
        ("solarize", 0.3),
        ("displace_x", 0.5),
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
    fn every_tipo_renders_differently() {
        let gpu = crate::gpu::Gpu::new_headless();
        let s = build().schema();
        let mut frames = Vec::new();
        for k in 0..8 {
            let mut f = s.defaults();
            f[2] = k as f32;
            frames.push(testing::frame(&gpu, &mut *build(), &f, &[0.5]));
        }
        for a in 0..8 {
            for b in a + 1..8 {
                assert_ne!(frames[a], frames[b], "tipo {a} and {b} look identical");
            }
        }
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
            eprintln!("imagedithering: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
