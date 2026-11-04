//! `gradient`: a radial gradient between the palette colours on the paper
//! scaffold (`shared/scaffold.rs`). Own floats: `radius off_x off_y blur`; one
//! colour slot `colores` (4 colours, linear) that the ramp walks through
//! from centre to edge. `blur` softens the ramp (0 linear, 1 eased).
//! The WGSL is original (`shaders/gradiente.wgsl`).

use crate::model::{spec, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("radius", 0.05, 2.5, 0.8),
    spec("off_x", -1.0, 1.0, 0.0),
    spec("off_y", -1.0, 1.0, 0.0),
    spec("blur", 0.0, 1.0, 0.3),
];

/// Linear values: blue, green, yellow, pink.
const COLORES: [f32; 12] = [0.04, 0.24, 1.00, 0.00, 0.64, 0.35, 1.00, 0.60, 0.04, 0.85, 0.15, 0.30];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS; "colores" => COLORES);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "gradient",
    "Gradient",
    &SCHEMA,
    FLOATS,
    1,
    include_str!("../../shaders/gradiente.wgsl"),
);

crate::scaffold_model!(Gradiente, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Gradiente::new())
    }

    const BASE: &[(&str, f32)] = &[("blur", 0.5), ("off_x", 0.2), ("off_y", -0.1), ("warp_amount", 0.4), ("background_level", 0.0), ("pulse", 0.0)];

    #[test]
    fn standard_checks() {
        testing::check_all(build, BASE);
    }
}
