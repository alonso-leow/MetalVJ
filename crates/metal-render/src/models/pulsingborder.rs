//! `pulsingborder`: A glowing rounded frame whose border carries orbiting colour spots, a breathing pulse, smoke and a bloom halo, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the border maths.
//!
//! Own floats (30): the brief's `redondez grosor suavidad aspecto intensidad
//! bloom puntos puntos_tam pulso_b humo humo_tam margen`, plus `velocidad
//! orbita contraflujo base elipse onda onda_freq doble doble_sep cortes
//! cortes_dens chispas vacio halo_interior halo_radio aberracion ciclo
//! fase_pulso grano`. Slots: `borde` (spots and line), `smoke` (smoke and
//! fill), `halo` (bloom).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("roundness", 0.0, 1.0, 0.25),
    spec("thickness", 0.0, 1.0, 0.1),
    spec("softness", 0.0, 1.0, 0.75),
    // 0 frame, 1 square, 2 4:3, 3 16:9, 4 9:16
    spec_held("aspect", 0.0, 4.0, 0.0, 0.2),
    spec("intensity", 0.0, 2.0, 0.6),
    spec("bloom", 0.0, 2.0, 0.5),
    spec_held("dots", 0.0, 8.0, 4.0, 0.2),
    spec("dots_size", 0.0, 1.0, 0.5),
    spec("pulse_b", 0.0, 1.0, 0.25),
    spec("smoke", 0.0, 1.0, 0.3),
    spec("smoke_size", 0.0, 1.0, 0.6),
    spec("margin", 0.0, 0.5, 0.0),
    spec("speed", -3.0, 3.0, 1.0),
    spec("orbit", -2.0, 2.0, 0.25),
    spec("counterflow", 0.0, 1.0, 0.0),
    spec("base", 0.0, 1.0, 0.3),
    spec("ellipse", 0.0, 1.0, 0.0),
    spec("wave", 0.0, 1.0, 0.0),
    spec_held("wave_freq", 1.0, 24.0, 6.0, 0.2),
    spec("double", 0.0, 1.0, 0.0),
    spec("double_separation", 0.0, 1.0, 0.4),
    spec("cuts", 0.0, 1.0, 0.0),
    spec_held("cuts_density", 2.0, 80.0, 24.0, 0.2),
    spec("sparks", 0.0, 1.0, 0.0),
    spec("empty", 0.0, 1.0, 0.0),
    spec("halo_interior", 0.0, 1.0, 0.3),
    spec("halo_radius", 0.0, 1.0, 0.3),
    spec("aberration", 0.0, 1.0, 0.0),
    spec("cycle", -2.0, 2.0, 0.0),
    spec("phase_pulse", 0.0, 1.0, 0.0),
    spec("grain", 0.0, 1.0, 0.0),
];

/// Linear values: cyan, magenta, red-orange, pale violet.
const BORDE: [f32; 12] = [0.004, 0.533, 0.982, 0.694, 0.008, 0.855, 1.00, 0.05, 0.027, 0.60, 0.40, 1.00];
/// Smoke and fill: deep blue, violet, plum, dark red.
const HUMO: [f32; 12] = [0.01, 0.08, 0.50, 0.30, 0.02, 0.45, 0.45, 0.01, 0.25, 0.50, 0.03, 0.05];
/// Bloom halo: azure, pink, orange, white.
const HALO: [f32; 12] = [0.02, 0.35, 1.00, 0.80, 0.05, 0.70, 1.00, 0.15, 0.03, 1.00, 0.90, 0.90];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "edge" => BORDE, "smoke" => HUMO, "halo" => HALO);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "pulsingborder",
    "Pulsing Border",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/pulsingborder.wgsl"),
);

crate::scaffold_model!(Pulsingborder, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Pulsingborder::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("counterflow", 0.5),
        ("wave", 0.4),
        ("double", 0.5),
        ("cuts", 0.5),
        ("sparks", 0.5),
        ("empty", 0.4),
        ("aberration", 0.4),
        ("cycle", 0.3),
        ("phase_pulse", 0.3),
        ("grain", 0.3),
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
            eprintln!("pulsingborder: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
