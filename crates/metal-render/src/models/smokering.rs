//! `smokering`: A ring of turbulent smoke around a hollow centre, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the turbulence maths.
//!
//! Own floats (27): the brief's `ruido_escala grosor radio forma_in
//! iteraciones` (stepped), plus ring shape `elipse angulo lobulos ondulacion
//! asimetria latido expansion`; smoke `vel remolino giro_humo filamentos
//! contraste_humo lacunaridad desgaste semilla`; light `hilo hilo_ancho
//! brillo_nucleo halo halo_radio color_vel aberracion`.
//! Slots: `smoke`, `nucleo`, `brillo`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("noise_scale", 0.5, 8.0, 3.0),
    spec("thickness", 0.05, 1.0, 0.65),
    spec("radius", 0.02, 0.45, 0.25),
    spec("shape_in", 0.0, 1.0, 0.7),
    spec_held("iterations", 1.0, 8.0, 8.0, 0.1),
    spec("ellipse", 0.4, 2.5, 1.0),
    spec("angle", 0.0, 1.0, 0.0),
    spec_held("lobes", 1.0, 8.0, 3.0, 0.1),
    spec("ripple", 0.0, 1.0, 0.0),
    spec("asymmetry", 0.0, 1.0, 0.0),
    spec("beat", 0.0, 1.0, 0.0),
    spec("expansion", 0.0, 1.0, 0.0),
    spec("speed", -2.0, 2.0, 0.5),
    spec("swirl", -3.0, 3.0, 0.4),
    spec("turn_smoke", -2.0, 2.0, 0.2),
    spec("filaments", 0.0, 1.0, 0.3),
    spec("contrast_smoke", 0.0, 3.0, 1.0),
    spec("lacunarity", 1.5, 3.0, 2.0),
    spec("wear", 0.0, 1.0, 0.2),
    spec("seed", 0.0, 50.0, 0.0),
    spec("thread", 0.0, 1.0, 0.3),
    spec("thread_width", 0.002, 0.1, 0.02),
    spec("shine_core", 0.0, 2.0, 0.6),
    spec("halo", 0.0, 1.0, 0.3),
    spec("halo_radius", 0.05, 1.0, 0.3),
    spec("color_speed", 0.0, 2.0, 0.2),
    spec("aberration", 0.0, 1.0, 0.0),
];

/// Linear: smoke inks (ink violet, ember red, amber, pale cream).
const HUMO: [f32; 12] = [0.02, 0.01, 0.06, 0.45, 0.06, 0.05, 0.85, 0.40, 0.08, 0.95, 0.85, 0.65];
/// Core tint: deep blue, magenta, orange, white.
const NUCLEO: [f32; 12] = [0.01, 0.02, 0.10, 0.50, 0.05, 0.30, 0.95, 0.35, 0.10, 0.95, 0.95, 0.95];
/// Thread / halo: warm white, cyan, pink, gold.
const BRILLO: [f32; 12] = [0.95, 0.85, 0.75, 0.30, 0.80, 0.95, 0.90, 0.30, 0.60, 0.95, 0.65, 0.15];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "smoke" => HUMO, "core" => NUCLEO, "shine" => BRILLO);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "smokering",
    "Smoke Ring",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/smokering.wgsl"),
);

crate::scaffold_model!(Smokering, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Smokering::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("ripple", 0.4),
        ("asymmetry", 0.3),
        ("beat", 0.3),
        ("expansion", 0.3),
        ("aberration", 0.4),
        ("angle", 0.1),
        ("ellipse", 1.2),
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
            eprintln!("smokering: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
