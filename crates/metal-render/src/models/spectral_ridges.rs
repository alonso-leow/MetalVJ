//! `spectral-ridges`: a scrolling spectrogram drawn as stacked ridge lines on
//! the procedural scaffold (`shared/scaffold.rs`). The parameter list follows the
//! built-in Spectrogram tool's ridge ("Lines") field view — zones across, rows
//! (bass to highs), height, release, hill width, round to square, stroke, zoom,
//! rotate, tilt, offsets, fade and colour-by — plus this port's own speed,
//! gain, shimmer, scan, glow and seed. The WGSL is original
//! (`shaders/spectral_ridges.wgsl`): a hashed column/band energy field, a
//! temporal release tail, and each band's ridge rising inside its own row.
//!
//! Own floats (23): `columns rows height release hill_width squareness stroke
//! zoom_field yaw tilt offset_x offset_y fade floor ceiling color_by speed seed gain
//! shimmer scan glow drift`. Slots: `spectrum`, `void`, `crest`. The frame's
//! outer edge is deliberately left transparent so the scaffold's `fondo`
//! background shows through.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("columns", 8.0, 64.0, 36.0, 0.15),
    spec_held("rows", 6.0, 40.0, 22.0, 0.15),
    spec("height", 0.0, 1.0, 0.65),
    spec("release", 0.0, 1.0, 0.45),
    spec("hill_width", 0.0, 1.0, 0.50),
    spec("squareness", 0.0, 1.0, 0.25),
    spec("stroke", 0.5, 8.0, 2.5),
    spec("zoom_field", 0.4, 2.0, 1.0),
    spec("yaw", -0.5, 0.5, 0.0),
    spec("tilt", -0.8, 0.8, 0.30),
    spec("offset_x", -1.0, 1.0, 0.0),
    spec("offset_y", -1.0, 1.0, 0.0),
    spec("fade", 0.0, 1.0, 0.15),
    spec("floor", 0.0, 1.0, 0.0),
    spec("ceiling", 0.0, 1.0, 0.40),
    spec_held("color_by", 0.0, 1.0, 0.0, 0.2),
    spec("speed", 0.0, 3.0, 0.90),
    spec_held("seed", 0.0, 50.0, 5.0, 0.2),
    spec("gain", 0.0, 2.0, 0.80),
    spec("shimmer", 0.0, 1.0, 0.15),
    spec("scan", 0.0, 1.0, 0.20),
    spec("glow", 0.0, 1.0, 0.30),
    spec("ridge_drift", 0.0, 1.0, 0.15),
];

/// Spectrum (linear): deep indigo, violet, ember red, hot amber.
const SPECTRUM: [f32; 12] = [
    0.030, 0.010, 0.100, //
    0.320, 0.030, 0.420, //
    0.920, 0.200, 0.120, //
    1.000, 0.800, 0.300,
];
/// Void (linear): near-black blues, one step per frequency quarter.
const VOID: [f32; 12] = [
    0.004, 0.006, 0.015, //
    0.010, 0.012, 0.035, //
    0.020, 0.018, 0.060, //
    0.040, 0.030, 0.100,
];
/// Crest (linear): icy cyan through to warm white.
const CREST: [f32; 12] = [
    0.250, 0.750, 1.000, //
    0.550, 0.920, 1.000, //
    1.000, 1.000, 1.000, //
    1.000, 0.820, 0.550,
];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "spectrum" => SPECTRUM, "void" => VOID, "crest" => CREST);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "spectral-ridges",
    "Spectral Ridges",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/spectral_ridges.wgsl"),
);

crate::scaffold_model!(SpectralRidges, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(SpectralRidges::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("columns", 36.0),
        ("rows", 22.0),
        ("height", 0.7),
        ("release", 0.5),
        ("hill_width", 0.55),
        ("squareness", 0.3),
        ("stroke", 3.0),
        ("zoom_field", 1.1),
        ("yaw", 0.2),
        ("tilt", 0.35),
        ("offset_x", 0.15),
        ("offset_y", -0.1),
        ("fade", 0.3),
        ("floor", 0.4),
        ("ceiling", 0.5),
        ("color_by", 1.0),
        ("speed", 1.0),
        ("seed", 7.0),
        ("gain", 0.9),
        ("shimmer", 0.3),
        ("scan", 0.4),
        ("glow", 0.5),
        ("ridge_drift", 0.35),
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
            eprintln!("spectral-ridges: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
