//! `sheettexture`: A lit sheet with fibres, tooth, crumples, folds, water drops and ageing over an optional source picture, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the relief maths.
//!
//! The picture is the `imagen` source surface; unconnected, the scaffold's
//! procedural test image is printed on the sheet.
//!
//! Own floats (25): the brief's `contraste_papel` (`contraste` is LOOK's)
//! `rugosidad fibra fibra_tam arrugas arrugas_tam pliegues n_pliegues(H) fade
//! gotas semilla`, plus variability floats: light `luz_angulo luz_altura
//! luz_vel` (orbiting light), `relieve`, `satinado`, `mezcla`, `tinte`,
//! `envejece mancha_tam`, `viento` (fibres drift), `aliento` (crumples
//! breathe), `pliegues_vel` (folds sway), `gotas_tam`, `grano`.
//! Slots: `papel` (shadow to light ramp), `tinta`, `mancha`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("contrast_paper", 0.0, 1.0, 0.3),
    spec("roughness", 0.0, 1.0, 0.4),
    spec("fiber", 0.0, 1.0, 0.3),
    spec("fiber_size", 0.02, 1.0, 0.2),
    spec("wrinkles", 0.0, 1.0, 0.3),
    spec("wrinkles_size", 0.05, 1.0, 0.35),
    spec("folds", 0.0, 1.0, 0.65),
    spec_held("n_folds", 1.0, 12.0, 5.0, 1.0),
    spec("fade", 0.0, 1.0, 0.0),
    spec("drops", 0.0, 1.0, 0.2),
    spec("seed", 0.0, 50.0, 5.8),
    spec("light_angle", 0.0, 1.0, 0.12),
    spec("light_height", 0.0, 1.0, 0.45),
    spec("light_speed", -2.0, 2.0, 0.15),
    spec("relief", 0.0, 3.0, 1.0),
    spec("satin", 0.0, 1.0, 0.1),
    spec("mix", 0.0, 1.0, 0.7),
    spec("tint", 0.0, 1.0, 0.3),
    spec("age", 0.0, 1.0, 0.0),
    spec("stain_size", 0.05, 1.0, 0.4),
    spec("wind", -4.0, 4.0, 0.3),
    spec("breath", 0.0, 1.0, 0.2),
    spec("folds_speed", 0.0, 3.0, 0.3),
    spec("drops_size", 0.03, 0.6, 0.2),
    spec("grain", 0.0, 1.0, 0.1),
];

/// Linear: shadow blue-grey, mid, light, white (shadow to light).
const PAPEL: [f32; 12] = [0.30, 0.34, 0.42, 0.62, 0.66, 0.72, 0.88, 0.89, 0.91, 1.0, 1.0, 1.0];
/// Linear: #9fadbc-ish ink tint, then three more.
const TINTA: [f32; 12] = [0.35, 0.42, 0.50, 0.55, 0.62, 0.72, 0.85, 0.85, 0.85, 1.0, 1.0, 1.0];
/// Linear: ageing stain, tea / ochre / brown / damp grey.
const MANCHA: [f32; 12] = [0.55, 0.40, 0.22, 0.65, 0.50, 0.30, 0.40, 0.30, 0.20, 0.50, 0.50, 0.50];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "paper" => PAPEL, "ink" => TINTA, "stain" => MANCHA);

pub static DEF: ScaffoldDef = ScaffoldDef::new_with_source(
    "sheettexture",
    "Sheet Texture",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/sheettexture.wgsl"),
    &["imagen"],
);

crate::scaffold_model!(Sheettexture, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Mapping, Model, SurfaceSpec};
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Sheettexture::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("contrast_paper", 0.6),
        ("roughness", 0.5),
        ("fiber", 0.5),
        ("wrinkles", 0.5),
        ("folds", 0.7),
        ("fade", 0.4),
        ("drops", 0.6),
        ("satin", 0.5),
        ("age", 0.6),
        ("tint", 0.5),
        ("mix", 0.7),
        ("breath", 0.5),
        ("grain", 0.4),
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
            eprintln!("sheettexture: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
