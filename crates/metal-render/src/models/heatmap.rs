//! `heatmap`: A source picture shape as a thermal map, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the ramp maths.
//!
//! The picture is the `imagen` source surface (luminance x alpha drives the
//! heat); unconnected, the procedural test image is used.
//!
//! Own floats (27): the brief's `contorno angulo ruido glow_in glow_out` plus
//! `velocidad radio umbral suaviza invierte bandas(H) banda_suave fluir
//! ruido_escala ruido_vel llama pulso_calor ondas ondas_freq dispersa brasa
//! tinte_glow gamma desfase ciclo grano semilla`. Slots: `calor` (ramp),
//! `halo` (outer glow tint), `nucleo` (white-hot core).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("outline", 0.0, 1.0, 0.5),
    spec("angle", 0.0, 1.0, 0.25),
    spec("noise", 0.0, 1.0, 0.35),
    spec("glow_in", 0.0, 1.0, 0.7),
    spec("glow_out", 0.0, 1.0, 0.6),
    spec("speed", -3.0, 3.0, 0.6),
    spec("radius", 0.01, 0.4, 0.09),
    spec("threshold", 0.05, 0.95, 0.4),
    spec("soften", 0.0, 1.0, 0.15),
    spec_held("invert", 0.0, 1.0, 0.0, 0.2),
    spec_held("bands", 0.0, 12.0, 0.0, 0.2),
    spec("band_soft", 0.0, 1.0, 0.3),
    spec("flow", 0.0, 3.0, 1.0),
    spec("noise_scale", 0.5, 8.0, 2.5),
    spec("noise_speed", -3.0, 3.0, 0.5),
    spec("flame", 0.0, 1.5, 0.3),
    spec("pulse_heat", 0.0, 1.0, 0.0),
    spec("waves", 0.0, 1.0, 0.0),
    spec("waves_freq", 0.2, 4.0, 1.0),
    spec("scattered", 0.0, 1.0, 0.0),
    spec("ember", 0.0, 1.0, 0.4),
    spec("tint_glow", 0.0, 1.0, 0.5),
    spec("gamma", -1.0, 1.0, 0.0),
    spec("shift", -0.5, 0.5, 0.0),
    spec("cycle", 0.0, 2.0, 0.0),
    spec("grain", 0.0, 1.0, 0.0),
    spec("seed", 0.0, 50.0, 0.0),
];

/// Linear: black, deep red, orange, yellow-white (thermal ramp).
const CALOR: [f32; 12] = [0.0, 0.0, 0.02, 0.35, 0.01, 0.02, 0.9, 0.2, 0.0, 1.0, 0.7, 0.1];
/// Linear: dark violet, magenta, red, orange (halo tint).
const HALO: [f32; 12] = [0.02, 0.0, 0.05, 0.3, 0.0, 0.25, 0.7, 0.05, 0.1, 0.9, 0.3, 0.05];
/// Linear: dark, orange, pale yellow, white (core).
const NUCLEO: [f32; 12] = [0.0, 0.0, 0.0, 0.8, 0.3, 0.05, 1.0, 0.9, 0.5, 1.0, 1.0, 1.0];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "calor" => CALOR, "halo" => HALO, "core" => NUCLEO);

pub static DEF: ScaffoldDef = ScaffoldDef::new_with_source(
    "heatmap",
    "Heatmap",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/heatmap.wgsl"),
    &["imagen"],
);

crate::scaffold_model!(Heatmap, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Mapping, Model, SurfaceSpec};
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Heatmap::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("pulse_heat", 0.4),
        ("waves", 0.5),
        ("scattered", 0.4),
        ("bands", 4.0),
        ("band_soft", 0.4),
        ("cycle", 0.5),
        ("grain", 0.4),
        ("tint_glow", 0.5),
        ("ember", 0.5),
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
            eprintln!("heatmap: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
