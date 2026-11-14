//! `liquidmetal`: Chrome-like liquid metal poured over a shape, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the shading maths.
//!
//! The shape is the `imagen` source (alpha/luminance); unconnected, a
//! procedural silhouette chosen by `forma` is used.
//!
//! Own floats (26): the brief's `forma(H) repeticion suavidad desplaza_r
//! desplaza_b distorsion contorno angulo`, plus `tam velocidad mascara bisel
//! relieve domo fresnel bandas_duras ondula ondula_freq anisotropia tinte
//! brillo_esp fluir pulsa fondo_reflejo borde semilla`. Slots: `metal`
//! (band gradient), `tinte` (tint and rim light).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    // forma (procedural shape when no source): 0 disc, 1 daisy, 2 diamond, 3 metaballs
    spec_held("shape", 0.0, 3.0, 2.0, 0.2),
    spec("repetition", 0.5, 12.0, 2.0),
    spec("softness", 0.0, 1.0, 0.1),
    spec("displace_r", -1.0, 1.0, 0.3),
    spec("displace_b", -1.0, 1.0, 0.3),
    spec("distortion", 0.0, 0.5, 0.07),
    spec("outline", 0.0, 1.0, 0.4),
    spec("angle", 0.0, 1.0, 0.19),
    spec("size", 0.2, 1.4, 0.6),
    spec("speed", -3.0, 3.0, 0.5),
    spec("mask", 0.0, 1.0, 0.3),
    spec("bevel", 0.02, 0.4, 0.12),
    spec("relief", 0.0, 2.0, 1.0),
    spec("dome", 0.0, 1.0, 0.3),
    spec("fresnel", 0.0, 2.0, 0.5),
    spec("bands_hard", 0.0, 1.0, 0.3),
    spec("ripple", 0.0, 1.0, 0.0),
    spec("ripple_freq", 1.0, 30.0, 8.0),
    spec("anisotropy", 0.0, 1.0, 0.0),
    spec("tint", 0.0, 1.0, 0.2),
    spec("shine_specular", 0.0, 1.0, 0.3),
    spec("flow", 0.0, 1.0, 0.15),
    spec("pulses", 0.0, 1.0, 0.0),
    spec("background_reflection", 0.0, 1.0, 0.4),
    spec("edge", 0.0, 0.5, 0.05),
    spec("seed", 0.0, 50.0, 0.0),
];

/// Linear: dark steel, mid grey, light grey, near white.
const METAL: [f32; 12] = [0.02, 0.02, 0.025, 0.25, 0.25, 0.27, 0.55, 0.55, 0.58, 1.0, 1.0, 1.0];
/// White, ice blue, warm gold, magenta.
const TINTE: [f32; 12] = [1.0, 1.0, 1.0, 0.5, 0.75, 1.0, 1.0, 0.7, 0.25, 0.9, 0.3, 0.7];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "metal" => METAL, "tint" => TINTE);

pub static DEF: ScaffoldDef = ScaffoldDef::new_with_source(
    "liquidmetal",
    "Liquid Metal",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/liquidmetal.wgsl"),
    &["imagen"],
)
.with_material();

crate::scaffold_model!(Liquidmetal, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Mapping, Model, SurfaceSpec};
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Liquidmetal::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("ripple", 0.5),
        ("anisotropy", 0.3),
        ("tint", 0.5),
        ("flow", 0.5),
        ("pulses", 0.4),
        ("fresnel", 0.6),
        ("shine_specular", 0.6),
        ("bands_hard", 0.5),
        ("background_reflection", 0.5),
        ("dome", 0.4),
        ("edge", 0.2),
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
        let px: Vec<u8> = (0..16).flat_map(|i| [(i * 16) as u8, 200, 30, (i * 16) as u8]).collect();
        m.set_surface("imagen", Some(SurfaceSpec::image(&px, 4, 4, 1.0, Mapping::Projector, 1.0)));
        let with = testing::frame(&gpu, &mut *m, &f, &[0.5]);
        assert_ne!(none, with);
        m.set_surface("imagen", None);
        assert_eq!(none, testing::frame(&gpu, &mut *m, &f, &[0.5]));
    }

    #[test]
    fn ms_per_frame() {
        if std::env::var("SCAFFOLD_MEASURE").is_ok() {
            eprintln!("liquidmetal: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
