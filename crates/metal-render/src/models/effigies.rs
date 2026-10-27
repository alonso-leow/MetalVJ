//! `effigies`: procedurally shaded head-and-bust effigies on the paper
//! scaffold. The parameter set follows the built-in "Humans" tool (pose,
//! material, light, floppy physics, grid views, perspective); the silhouettes,
//! materials and WGSL are original, with no meshes or upstream assets.
//!
//! Own floats (32): `size pitch yaw roll offset_x offset_y speed spin_dir
//! material light floppy stiffness swing hold gravity gravity_strength
//! gravity_x gravity_y grid grid_amount grid_radius grid_speed grid_dir
//! wall_cols wall_rows wall_brick wall_zoom tunnel_flow tunnel_twist
//! tunnel_angle perspective seed`. Slots: `skin`, `rim`, `cloth`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("size", 0.4, 2.0, 0.9),
    spec("pitch", -60.0, 60.0, 0.0),
    spec("yaw", -180.0, 180.0, 0.0),
    spec("roll", -180.0, 180.0, 0.0),
    spec("offset_x", -60.0, 60.0, 0.0),
    spec("offset_y", -60.0, 60.0, 0.0),
    spec("speed", -2.0, 2.0, 0.35),
    spec_held("spin_dir", 0.0, 3.0, 3.0, 0.2),
    spec_held("material", 0.0, 8.0, 0.0, 0.2),
    spec_held("light", 0.0, 3.0, 0.0, 0.2),
    spec("floppy", 0.0, 1.0, 0.0),
    spec("stiffness", 0.0, 100.0, 50.0),
    spec("swing", 0.0, 100.0, 50.0),
    spec_held("hold", 0.0, 2.0, 1.0, 0.2),
    spec("gravity", 0.0, 1.0, 0.0),
    spec("gravity_strength", 0.0, 100.0, 50.0),
    spec("gravity_x", -100.0, 100.0, 0.0),
    spec("gravity_y", -100.0, 100.0, 0.0),
    spec_held("grid", 0.0, 3.0, 0.0, 0.2),
    spec("grid_amount", 1.0, 12.0, 3.0),
    spec("grid_radius", 0.0, 100.0, 40.0),
    spec("grid_speed", -2.0, 2.0, 0.4),
    spec_held("grid_dir", 0.0, 3.0, 0.0, 0.2),
    spec("wall_cols", 1.0, 8.0, 3.0),
    spec("wall_rows", 1.0, 8.0, 3.0),
    spec("wall_brick", 0.0, 1.0, 0.3),
    spec("wall_zoom", 0.3, 2.5, 1.0),
    spec("tunnel_flow", -2.0, 2.0, 0.0),
    spec("tunnel_twist", -2.0, 2.0, 0.0),
    spec("tunnel_angle", -180.0, 180.0, 0.0),
    spec("perspective", 0.0, 100.0, 40.0),
    spec_held("seed", 0.0, 50.0, 3.0, 0.2),
];

/// Skin / clay: deep umber, warm ochre, pale highlight, burnt shadow (linear).
const SKIN: [f32; 12] = [
    0.34, 0.16, 0.09, 0.66, 0.38, 0.20, 0.92, 0.68, 0.47, 0.28, 0.11, 0.06,
];
/// Rim / reflection: cool sky, slate, near-white, deep blue (linear).
const RIM: [f32; 12] = [
    0.62, 0.78, 1.00, 0.30, 0.48, 0.78, 0.94, 0.97, 1.00, 0.12, 0.20, 0.40,
];
/// Cloth / hair: near-black indigo, dark teal, steel, ink (linear).
const CLOTH: [f32; 12] = [
    0.012, 0.018, 0.030, 0.04, 0.09, 0.12, 0.10, 0.19, 0.21, 0.006, 0.010, 0.018,
];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "skin" => SKIN, "rim" => RIM, "cloth" => CLOTH);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "effigies",
    "Effigies",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/effigies.wgsl"),
);

crate::scaffold_model!(Effigies, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Effigies::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("warp_amount", 0.4),
        ("pulse", 0.0),
        ("size", 1.0),
        ("pitch", 10.0),
        ("yaw", 25.0),
        ("roll", 12.0),
        ("offset_x", 4.0),
        ("offset_y", -3.0),
        ("speed", 0.5),
        ("spin_dir", 1.0),
        ("material", 3.0),
        ("light", 1.0),
        ("floppy", 0.5),
        ("stiffness", 50.0),
        ("swing", 50.0),
        ("hold", 1.0),
        ("gravity", 0.5),
        ("gravity_strength", 50.0),
        ("gravity_x", 30.0),
        ("gravity_y", 20.0),
        ("grid", 2.0),
        ("grid_amount", 2.0),
        ("grid_radius", 40.0),
        ("grid_speed", 0.5),
        ("grid_dir", 1.0),
        ("wall_cols", 3.0),
        ("wall_rows", 2.0),
        ("wall_brick", 0.4),
        ("wall_zoom", 0.9),
        ("tunnel_flow", 0.5),
        ("tunnel_twist", 0.5),
        ("tunnel_angle", 30.0),
        ("perspective", 50.0),
        ("seed", 2.0),
    ];

    #[test]
    fn standard_checks() {
        testing::check_all(build, BASE);
    }

    #[test]
    fn ms_per_frame() {
        if std::env::var("SCAFFOLD_MEASURE").is_ok() {
            eprintln!(
                "effigies: {:.2} ms/frame at 1280x720",
                testing::measure_ms(build)
            );
        }
    }
}
