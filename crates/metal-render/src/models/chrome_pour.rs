//! `chrome-pour`: a pool of liquid chrome with an animated 3D-lit surface, on
//! the procedural scaffold (`shared/scaffold.rs`). Behaviour and controls follow the
//! built-in "liquid metal" tool (a beveled liquid body, procedural key/rim/fill
//! environment, Fresnel metal reflection, rotating surface); the WGSL is
//! original (`shaders/chrome_pour.wgsl`).
//!
//! Own floats (28): tool controls `zoom rotate stroke_weight bevel speed
//! spin_dir pour` plus surface shaping `bulge lobes lobe_depth ripple
//! ripple_freq flow flow_speed warp`, the material `roughness reflection
//! fresnel specular diffuse ambient key_light rim_light fill_light
//! background_reflection tint edge_soft`, and `seed`.
//! Slots: `metal`, `light`, `tint`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("pour_zoom", 0.3, 2.5, 1.0),
    spec("rotate", 0.0, 1.0, 0.0),
    spec("stroke_weight", 0.05, 2.0, 1.0),
    spec("bevel", 0.02, 0.6, 0.18),
    spec("speed", -3.0, 3.0, 0.8),
    // spin_dir: 0 right, 1 left, 2 up, 3 down.
    spec_held("spin_dir", 0.0, 3.0, 0.0, 0.2),
    spec("pour", 0.1, 1.0, 0.62),
    spec("bulge", 0.0, 1.0, 0.5),
    spec_held("lobes", 0.0, 8.0, 5.0, 0.2),
    spec("lobe_depth", 0.0, 1.0, 0.35),
    spec("ripple", 0.0, 1.0, 0.25),
    spec("ripple_freq", 1.0, 30.0, 8.0),
    spec("flow", 0.0, 1.0, 0.45),
    spec("flow_speed", -2.0, 2.0, 0.5),
    spec("warp", 0.0, 1.0, 0.35),
    spec("roughness", 0.0, 1.0, 0.18),
    spec("reflection", 0.0, 1.0, 0.65),
    spec("fresnel", 0.0, 2.0, 0.6),
    spec("specular", 0.0, 2.0, 1.2),
    spec("diffuse", 0.0, 1.0, 0.34),
    spec("ambient", 0.0, 1.0, 0.14),
    spec("key_light", 0.0, 2.0, 1.0),
    spec("rim_light", 0.0, 2.0, 0.8),
    spec("fill_light", 0.0, 2.0, 0.5),
    spec("background_reflection", 0.0, 1.0, 0.55),
    spec("tint", 0.0, 1.0, 0.2),
    spec("edge_soft", 0.0, 0.6, 0.15),
    spec_held("seed", 0.0, 50.0, 3.0, 0.2),
];

/// Linear: near-black steel, dark chrome, bright chrome, white highlight.
const METAL: [f32; 12] = [0.01, 0.012, 0.015, 0.12, 0.13, 0.16, 0.48, 0.50, 0.55, 1.0, 1.0, 1.0];
/// Warm key, cool rim, magenta fill, pale gold.
const LIGHT: [f32; 12] = [1.0, 0.95, 0.85, 0.35, 0.6, 1.0, 0.9, 0.3, 0.7, 1.0, 0.8, 0.35];
/// Deep blue, cyan, violet, warm bronze.
const TINT: [f32; 12] = [0.02, 0.03, 0.08, 0.1, 0.5, 0.7, 0.35, 0.15, 0.5, 0.8, 0.45, 0.2];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "metal" => METAL, "light" => LIGHT, "tint" => TINT);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "chrome-pour",
    "Chrome Pour",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/chrome_pour.wgsl"),
);

crate::scaffold_model!(ChromePour, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(ChromePour::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("bulge", 0.5),
        ("lobe_depth", 0.5),
        ("ripple", 0.5),
        ("flow", 0.5),
        ("warp", 0.4),
        ("tint", 0.5),
        ("background_reflection", 0.6),
        ("rim_light", 0.8),
        ("fill_light", 0.6),
        ("edge_soft", 0.2),
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
            eprintln!("chrome-pour: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
