//! `spiral`: Multi-arm spiral strokes (Archimedean to logarithmic) on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the spiral maths.
//!
//! Own floats (27): the brief's `densidad distorsion trazo trazo_cono
//! trazo_punta ruido ruido_frec suavidad`, plus `brazos mezcla_log velocidad
//! hueco borde centro_x centro_y respiro ondas ondas_lobulos alternar
//! parpadeo_brazo color_brazo color_radio color_trazo doble glow apriete_vaiven`
//! and `giro_brazos`. Slots: `trazo` (arms), `doble` (counter spiral + glow).

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("density", 0.0, 1.0, 0.5),
    spec("distortion", 0.0, 1.0, 0.0),
    spec("stroke", 0.0, 1.0, 0.5),
    spec("stroke_cone", -1.0, 1.0, 0.0),
    spec("stroke_tip", 0.0, 1.0, 0.0),
    spec("noise", 0.0, 1.0, 0.0),
    spec("noise_freq", 0.0, 1.0, 0.5),
    spec("softness", 0.0, 1.0, 0.1),
    spec_held("arms", 1.0, 12.0, 1.0, 0.2),
    spec("mix_log", 0.0, 1.0, 0.0),
    spec("speed", -3.0, 3.0, 0.5),
    spec("hole", 0.0, 1.0, 0.0),
    spec("edge", 0.0, 1.0, 0.0),
    spec("center_x", -0.8, 0.8, 0.0),
    spec("center_y", -0.8, 0.8, 0.0),
    spec("breath", 0.0, 1.0, 0.0),
    spec("waves", 0.0, 1.0, 0.0),
    spec_held("waves_lobes", 1.0, 12.0, 5.0, 0.2),
    spec("alternate", 0.0, 1.0, 0.0),
    spec("blink_arm", 0.0, 1.0, 0.0),
    spec("color_arm", 0.0, 1.0, 0.5),
    spec("color_radius", 0.0, 1.0, 0.0),
    spec("color_stroke", 0.0, 1.0, 0.0),
    spec("double", 0.0, 1.0, 0.0),
    spec("glow", 0.0, 1.0, 0.0),
    spec("squeeze_swing", 0.0, 1.0, 0.0),
    spec("turn_arms", -1.0, 1.0, 0.0),
];

/// Arms: lime, teal, violet, rose (linear).
const TRAZO: [f32; 12] = [0.60, 0.90, 0.10, 0.02, 0.60, 0.50, 0.30, 0.10, 0.90, 0.80, 0.20, 0.30];
/// Counter spiral and glow: orange, white, blue, magenta.
const DOBLE: [f32; 12] = [1.00, 0.30, 0.02, 1.00, 0.95, 0.80, 0.05, 0.25, 1.00, 0.80, 0.05, 0.60];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "stroke" => TRAZO, "double" => DOBLE);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "spiral",
    "Spiral",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/spiral.wgsl"),
);

crate::scaffold_model!(Spiral, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Spiral::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("distortion", 0.4),
        ("stroke_cone", 0.4),
        ("stroke_tip", 0.5),
        ("noise", 0.4),
        ("arms", 3.0),
        ("mix_log", 0.4),
        ("hole", 0.2),
        ("edge", 0.3),
        ("breath", 0.4),
        ("waves", 0.4),
        ("alternate", 0.5),
        ("blink_arm", 0.4),
        ("color_radius", 0.4),
        ("color_stroke", 0.4),
        ("double", 0.5),
        ("glow", 0.5),
        ("squeeze_swing", 0.4),
        ("turn_arms", 0.3),
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
            eprintln!("spiral: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
