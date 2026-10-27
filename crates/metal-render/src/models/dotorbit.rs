//! `dotorbit`: Dots circling their own cells on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the orbital maths.
//!
//! Own floats (24): the brief's `pasos_color` (stepped) `tam tam_rango
//! dispersion`, plus `celda cantidad`(stepped) `velocidad vel_rango sentido
//! elipse inclina estela estela_largo pulso_punto pulso_vel destello
//! profundidad halo anillo suavidad jitter color_vel color_radio vel_x vel_y`.
//! Slot: `puntos`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("steps_color", 1.0, 8.0, 4.0, 0.2),
    spec("size", 0.05, 1.0, 0.35),
    spec("size_range", 0.0, 1.0, 0.5),
    spec("dispersion", 0.0, 1.2, 0.8),
    spec("cell", 0.1, 0.8, 0.3),
    spec_held("amount", 1.0, 6.0, 3.0, 0.2),
    spec("speed", -3.0, 3.0, 0.8),
    spec("speed_range", 0.0, 1.0, 0.5),
    spec("sense", 0.0, 1.0, 0.2),
    spec("ellipse", 0.0, 0.9, 0.2),
    spec("tilt", 0.0, 1.0, 0.3),
    spec("wake", 0.0, 1.0, 0.0),
    spec("wake_long", 0.05, 2.0, 0.5),
    spec("pulse_dot", 0.0, 1.0, 0.0),
    spec("pulse_speed", 0.0, 4.0, 1.0),
    spec("flash", 0.0, 1.0, 0.0),
    spec("depth", 0.0, 1.0, 0.0),
    spec("halo", 0.0, 1.0, 0.0),
    spec("ring", 0.0, 1.0, 0.0),
    spec("softness", 0.0, 1.0, 0.1),
    spec("jitter", 0.0, 1.0, 0.3),
    spec("color_speed", -2.0, 2.0, 0.0),
    spec("color_radius", 0.0, 1.0, 0.0),
    spec("speed_x", -2.0, 2.0, 0.0),
    spec("speed_y", -2.0, 2.0, 0.0),
];

/// Linear values: coral, amber, teal, violet.
const PUNTOS: [f32; 12] = [0.90, 0.20, 0.15, 0.95, 0.60, 0.10, 0.10, 0.70, 0.60, 0.40, 0.15, 0.85];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS; "dots" => PUNTOS);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "dotorbit",
    "Dot Orbit",
    &SCHEMA,
    FLOATS,
    1,
    include_str!("../../shaders/dotorbit.wgsl"),
);

crate::scaffold_model!(Dotorbit, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Dotorbit::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("wake", 0.6),
        ("pulse_dot", 0.2),
        ("flash", 0.4),
        ("depth", 0.4),
        ("halo", 0.4),
        ("ring", 0.4),
        ("color_speed", 0.5),
        ("color_radius", 0.3),
        ("speed_x", 0.5),
        ("sense", 0.5),
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
            eprintln!("dotorbit: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
