//! `magnet-spikes`: a raymarched ferrofluid sphere whose surface erupts into
//! noise-driven spikes, on the procedural scaffold (`shared/scaffold.rs`). Behaviour and
//! parameter list follow the built-in "Ferrofluid Spikes" tool (blob size, spike
//! height/density, sharpness, glossiness, zoom, motion, rotation, direction and
//! the two colour wells); the WGSL is original
//! (`shaders/magnet_spikes.wgsl`).
//!
//! Own floats (19): `blob_size spike_height spike_density sharpness glossiness
//! cam_dist speed rotation_speed direction wobble wobble_freq light_angle
//! fresnel reflectivity specular ambient rim pitch seed`. `speed` is the tool's
//! motion speed, `cam_dist` its zoom (higher = farther), and `direction` is a
//! held 0..3 enum (left, up, down, right) selecting the orbit axis. Slots:
//! `fluid` (the metal body), `sheen` (environment, specular and rim light). The
//! tool's background well is the scaffold's `fondo`; misses are transparent so
//! that slot shows through.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec("blob_size", 0.3, 1.6, 1.0),
    spec("spike_height", 0.0, 0.8, 0.38),
    spec("spike_density", 0.5, 14.0, 5.0),
    spec("sharpness", 0.5, 8.0, 2.6),
    spec("glossiness", 2.0, 96.0, 42.0),
    spec("cam_dist", 2.8, 10.0, 4.6),
    spec("speed", -2.0, 2.0, 0.55),
    spec("rotation_speed", -1.5, 1.5, 0.28),
    spec_held("direction", 0.0, 3.0, 0.0, 0.2),
    spec("wobble", 0.0, 0.25, 0.06),
    spec("wobble_freq", 0.5, 6.0, 2.0),
    spec("light_angle", 0.0, 1.0, 0.18),
    spec("fresnel", 0.0, 2.0, 0.85),
    spec("reflectivity", 0.0, 1.5, 0.65),
    spec("specular", 0.0, 2.0, 0.8),
    spec("ambient", 0.0, 1.0, 0.22),
    spec("rim", 0.0, 2.0, 0.6),
    spec("pitch", -0.8, 0.8, 0.12),
    spec_held("seed", 0.0, 50.0, 3.0, 0.2),
];

/// Fluid: near-black blue steel through bright chrome (linear).
const FLUID: [f32; 12] = [
    0.020, 0.026, 0.038,
    0.075, 0.095, 0.130,
    0.240, 0.280, 0.350,
    0.780, 0.840, 0.940,
];
/// Sheen: white, cold blue, warm amber, pale violet (linear).
const SHEEN: [f32; 12] = [
    1.00, 1.00, 1.00,
    0.45, 0.68, 1.00,
    1.00, 0.72, 0.38,
    0.78, 0.82, 1.00,
];

pub static SCHEMA: crate::model::Schema =
    crate::scaffold_schema!(FLOATS; "fluid" => FLUID, "sheen" => SHEEN);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "magnet-spikes",
    "Magnet Spikes",
    &SCHEMA,
    FLOATS,
    2,
    include_str!("../../shaders/magnet_spikes.wgsl"),
);

crate::scaffold_model!(MagnetSpikes, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(MagnetSpikes::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("spike_height", 0.55),
        ("sharpness", 3.0),
        ("glossiness", 30.0),
        ("speed", 0.8),
        ("rotation_speed", 0.5),
        ("direction", 1.0),
        ("wobble", 0.12),
        ("wobble_freq", 3.0),
        ("light_angle", 0.35),
        ("fresnel", 1.2),
        ("reflectivity", 1.0),
        ("specular", 1.2),
        ("ambient", 0.45),
        ("rim", 1.0),
        ("pitch", 0.3),
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
            eprintln!("magnet-spikes: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
