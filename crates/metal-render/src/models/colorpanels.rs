//! `colorpanels`: A ring of translucent glass panels turning around a vertical axis, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the geometry.
//!
//! Own floats (16) on top of the scaffold kits (CAM2D 5, TWIST 4, LOOK 6):
//! the brief's `densidad largo bordes blur fade_in fade_out gradiente`, the
//! props `angulo_1/2` (top and bottom lean) and `velocidad`, plus `fase`,
//! `ancho`, `radio`, `perspectiva`, `cristal` (panel opacity) and `ondula`
//! (sway) for variability. `densidad` is a fractional panel count (x2.5), the
//! last panel fading in, so it is safe to modulate.

use crate::model::{spec, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("density", 0.4, 9.6, 3.0),
    spec("long", 0.2, 2.5, 1.1),
    spec("edges", 0.0, 1.0, 0.45),
    spec("blur", 0.0, 1.0, 0.0),
    spec("fade_in", 0.0, 1.0, 0.35),
    spec("fade_out", 0.0, 1.0, 0.3),
    spec("gradient", 0.0, 1.0, 0.4),
    spec("angle_1", -1.0, 1.0, 0.0),
    spec("angle_2", -1.0, 1.0, 0.0),
    spec("speed", -2.0, 2.0, 0.35),
    spec("phase", 0.0, 1.0, 0.0),
    spec("width", 0.03, 0.6, 0.2),
    spec("radius", 0.0, 1.0, 0.45),
    spec("perspective", 0.0, 0.9, 0.35),
    spec("crystal", 0.1, 1.0, 0.7),
    spec("ripple", 0.0, 1.0, 0.0),
];

/// Linear values (targets encode sRGB on write): violet, magenta, amber, cyan.
const PANELES: [f32; 12] = [0.28, 0.06, 0.75, 0.80, 0.05, 0.30, 0.95, 0.40, 0.04, 0.05, 0.60, 0.80];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS; "paneles" => PANELES);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "colorpanels",
    "Color Panels",
    &SCHEMA,
    FLOATS,
    1,
    include_str!("../../shaders/colorpanels.wgsl"),
);

crate::scaffold_model!(Colorpanels, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Colorpanels::new())
    }

    /// Optional stages on so every float has something to act on.
    const BASE: &[(&str, f32)] = &[("blur", 0.2), ("warp_amount", 0.4), ("ripple", 0.3), ("pulse", 0.0)];

    #[test]
    fn standard_checks() {
        testing::check_all(build, BASE);
    }

    #[test]
    fn ms_per_frame() {
        if std::env::var("SCAFFOLD_MEASURE").is_ok() {
            eprintln!("colorpanels: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
