//! `halftonecmyk`: A picture reprinted as four overprinted CMYK halftone screens, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the screen maths.
//!
//! The picture is the `imagen` *source* surface (procedural test image when
//! unconnected). Own floats (38): the brief's `tam ruido_grid tipo(H) suavidad
//! contraste_img` (named so it does not clash with LOOK's `contraste`),
//! `inunda_c/m/y/k`, `ganancia_c/m/y/k`, `grano_mix grano_sup grano_tam`, plus
//! variability: per-channel screen angles `angulo_c/m/y/k`, `rota_red rota_vel`
//! (rosette spin), `registro registro_dir registro_vel` (misregistration),
//! `densidad`, `sangrado` (ink bleed), `ucr` (black generation), `sat_img`,
//! `brillo_img`, `ondula_tam`, `respira respira_vel`, `fibra` (paper),
//! `mancha` (ink blotching), `posteriza`, `papel_op` (bare-paper opacity), `semilla`. `tipo`: dots, ink, sharp,
//! lines, squares. Slots: `tintas` (C M Y K) and `papel` (paper + fibre tints).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("size", 0.02, 0.6, 0.12),
    spec("noise_grid", 0.0, 1.0, 0.2),
    spec_held("type", 0.0, 4.0, 0.2, 0.2),
    spec("softness", 0.0, 1.0, 0.2),
    spec("contrast_img", 0.3, 3.0, 1.0),
    spec("flood_c", 0.0, 1.0, 0.15),
    spec("flood_m", 0.0, 1.0, 0.0),
    spec("flood_y", 0.0, 1.0, 0.0),
    spec("flood_k", 0.0, 1.0, 0.0),
    spec("gain_c", -1.0, 1.0, 0.3),
    spec("gain_m", -1.0, 1.0, 0.0),
    spec("gain_y", -1.0, 1.0, 0.2),
    spec("gain_k", -1.0, 1.0, 0.0),
    spec("grain_mix", 0.0, 1.0, 0.0),
    spec("grain_surface", 0.0, 1.0, 0.0),
    spec("grain_size", 0.0, 1.0, 0.5),
    spec("angle_c", 0.0, 1.0, 0.0417),
    spec("angle_m", 0.0, 1.0, 0.2083),
    spec("angle_y", 0.0, 1.0, 0.0),
    spec("angle_k", 0.0, 1.0, 0.125),
    spec("rotate_red", 0.0, 1.0, 0.0),
    spec("rotate_speed", -3.0, 3.0, 0.0),
    spec("register", 0.0, 1.0, 0.15),
    spec("register_direction", 0.0, 1.0, 0.1),
    spec("register_speed", 0.0, 3.0, 0.5),
    spec("density", 0.0, 1.0, 1.0),
    spec("indent", 0.0, 1.0, 0.3),
    spec("ucr", 0.0, 1.0, 0.8),
    spec("sat_img", 0.0, 2.0, 1.2),
    spec("ripple_size", 0.0, 1.0, 0.0),
    spec("breathe", 0.0, 1.0, 0.0),
    spec("breathe_speed", 0.0, 4.0, 1.0),
    spec("fiber", 0.0, 1.0, 0.4),
    spec("stain", 0.0, 1.0, 0.0),
    spec("seed", 0.0, 50.0, 0.0),
    spec("shine_img", 0.3, 2.0, 1.0),
    spec("posterize", 0.0, 1.0, 0.0),
    spec("paper_opacity", 0.0, 1.0, 1.0),
];

/// Linear: cyan, magenta, yellow, key black.
const TINTAS: [f32; 12] = [0.0, 0.45, 1.0, 0.97, 0.08, 0.34, 1.0, 0.69, 0.0, 0.017, 0.014, 0.014];
/// Linear: paper, darker fibre, warm fleck, cool fleck.
const PAPEL: [f32; 12] = [0.96, 0.955, 0.905, 0.80, 0.78, 0.70, 0.92, 0.86, 0.72, 0.85, 0.9, 0.95];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "tintas" => TINTAS, "paper" => PAPEL);

pub static DEF: ScaffoldDef = ScaffoldDef::new_with_source(
    "halftonecmyk",
    "Halftone CMYK",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/halftonecmyk.wgsl"),
    &["imagen"],
);

crate::scaffold_model!(Halftonecmyk, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Mapping, Model, SurfaceSpec};
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Halftonecmyk::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("flood_m", 0.2),
        ("flood_y", 0.2),
        ("flood_k", 0.2),
        ("gain_m", 0.3),
        ("gain_k", 0.3),
        ("grain_mix", 0.4),
        ("grain_surface", 0.4),
        ("rotate_red", 0.1),
        ("rotate_speed", 1.0),
        ("register", 0.5),
        ("indent", 0.5),
        ("ripple_size", 0.4),
        ("breathe", 0.4),
        ("stain", 0.5),
        ("posterize", 0.5),
        ("noise_grid", 0.4),
        ("type", 1.0),
        ("warp_amount", 0.4),
        ("paper_opacity", 0.6),
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
            eprintln!("halftonecmyk: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
