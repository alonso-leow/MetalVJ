//! The tone block's colour gradient — the tone design notes §5, in one home.
//!
//! **Why this lives in `metal-notation` and not in `metal-perform`.** The gradient
//! there reads the six tone columns out of a digest [`Frame`], and the proof
//! sheet that renders a digest's tone block (`metal-render`'s `palette-proof`)
//! needs the same arithmetic. `metal-perform` optionally depends on `metal-render`,
//! so `metal-render` cannot depend on `metal-perform` without a cycle — but
//! `metal-notation` is a leaf (`metal-rig` only) and both already reach it for
//! [`crate::color`]. Putting the pure formula here gives it exactly one home
//! and lets `metal-perform::drive` and the proof sheet share it byte for byte.
//!
//! The formula is deliberately the one `drive::tone_slot` already shipped:
//! centre and spread are the tone vector's angle and its inverted length, the
//! number of stops grows with the spread, register sets lightness, density
//! sets chroma and the alternate-stop lightness variety, and mode warms or
//! cools the gradient. Keeping it identical is what lets the drive refactor be
//! a pure move rather than a behaviour change.

use crate::color::{to_srgb, Oklch};

/// Which colour family the gradient is drawn in.
///
/// This is the notation-local twin of `metal-perform::drive::Palette`; the two are
/// mapped at the seam because `metal-perform` cannot import `metal-render` and this
/// crate owns no engine vocabulary. [`Palette::MetalVJ`] is the house one
/// (the accent red, varying only lightness and chroma); [`Palette::Spectrum`]
/// uses the harmony angle directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Palette {
    MetalVJ,
    Spectrum,
}

/// §5 of `the tone design notes`: four RGB (0..1) stops from the tone block.
///
/// `centre` is the circle-of-fifths hue in degrees, `spread` 0..1 (focused ↔
/// diffuse), and `register`, `density`, `mode` are the digest columns of the
/// same names, 0..1. Each stop is an OKLCh colour converted with the crate's
/// only colour conversion, [`to_srgb`], and divided to 0..1.
///
/// A non-finite input is not left to poison `to_srgb`: each column falls back
/// to the neutral the drive path uses (`spread` 0, `register` 0.5, `density` 0,
/// `mode` 0.5, and the MetalVJ accent 27° for a non-finite centre), so a
/// NaN in a digest row paints the brand rather than a colour from garbage.
pub fn tone_gradient(
    centre_deg: f32,
    spread: f32,
    register: f32,
    density: f32,
    mode: f32,
    palette: Palette,
) -> [[f32; 3]; 4] {
    // Guard first: everything below reads these and any one of them being NaN
    // would reach `to_srgb` through the Oklch. The fallbacks are the drive
    // path's own neutral values (the tone design notes §5 guard).
    let centre_deg = if centre_deg.is_finite() { centre_deg } else { 27.0 };
    let spread = if spread.is_finite() { spread.clamp(0.0, 1.0) } else { 0.0 };
    let register = if register.is_finite() { register.clamp(0.0, 1.0) } else { 0.5 };
    let density = if density.is_finite() { density.clamp(0.0, 1.0) } else { 0.0 };
    let mode = if mode.is_finite() { mode.clamp(0.0, 1.0) } else { 0.5 };

    // 2 stops focused .. 4 diffuse; the step widens with the spread and the
    // mode biases the whole gradient ±15° (the tone design notes §5).
    let n = (2 + (2.0 * spread).round() as usize).clamp(2, 4);
    let step = 20.0 + 70.0 * spread;
    let mode_bias = (mode - 0.5) * 30.0;

    // The MetalVJ hue: the accent nudged by where the centre sits, so a
    // warm or cool harmony is legible while the image stays the brand colour.
    let fw_hue = 27.0 + 10.0 * (centre_deg / 180.0).clamp(-1.0, 1.0);

    let mut stops = Vec::with_capacity(n);
    for i in 0..n {
        let hue_i = centre_deg + mode_bias + (i as f32 - (n as f32 - 1.0) / 2.0) * step;
        // Lightness alternates by density, so a thick chord varies more between
        // its stops and a sparse one stays a near-flat ramp.
        let alt = if i % 2 == 0 { 1.0 } else { -1.0 };
        let l = (0.30 + 0.50 * register + 0.10 * density * alt).clamp(0.0, 1.0);
        let c = 0.04 + 0.18 * density;
        let h = match palette {
            Palette::Spectrum => hue_i,
            Palette::MetalVJ => fw_hue,
        };
        stops.push(Oklch { l, c, h });
    }

    let mut out = [[0.0f32; 3]; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        // Four slots, n stops cycled: a focused harmony shows its two colours
        // twice, a diffuse one four different ones.
        let rgb = to_srgb(stops[i % n]);
        *slot = [rgb.r as f32 / 255.0, rgb.g as f32 / 255.0, rgb.b as f32 / 255.0];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many of the four slots are visually different colours, by 8-bit
    /// triple — the observable a "n stops" claim actually has when slots are
    /// always four. The same measure `metal-perform::drive`'s tests use.
    fn distinct(stops: [[f32; 3]; 4]) -> usize {
        let mut seen: Vec<[i32; 3]> = Vec::new();
        for c in stops {
            let t = [
                (c[0] * 255.0).round() as i32,
                (c[1] * 255.0).round() as i32,
                (c[2] * 255.0).round() as i32,
            ];
            if !seen.contains(&t) {
                seen.push(t);
            }
        }
        seen.len()
    }

    /// Focused harmony gives two close stops; diffuse gives up to four further
    /// apart (the tone design notes §5). Spectrum is the palette where the count is legible
    /// because the stops differ in hue.
    #[test]
    fn spread_adds_stops() {
        let focused = distinct(tone_gradient(0.0, 0.0, 0.5, 0.5, 0.5, Palette::Spectrum));
        let diffuse = distinct(tone_gradient(0.0, 1.0, 0.5, 0.5, 0.5, Palette::Spectrum));
        assert_eq!(focused, 2, "a focused harmony must be two stops");
        assert_eq!(diffuse, 4, "a diffuse harmony must be four stops");
        assert!(focused < diffuse, "spread did not add stops");
    }

    /// The contract the renderers rely on: every channel a displayable 0..1,
    /// and never a NaN, across the whole input space.
    #[test]
    fn every_stop_is_finite_and_in_gamut() {
        for &palette in &[Palette::MetalVJ, Palette::Spectrum] {
            for centre in [-720.0f32, -180.0, -37.0, 0.0, 27.0, 90.0, 359.0, 720.0] {
                for spread in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
                    for register in [0.0f32, 0.5, 1.0] {
                        for density in [0.0f32, 0.5, 1.0] {
                            for mode in [0.0f32, 0.5, 1.0] {
                                for stop in tone_gradient(
                                    centre, spread, register, density, mode, palette,
                                ) {
                                    for v in stop {
                                        assert!(
                                            v.is_finite() && (0.0..=1.0).contains(&v),
                                            "{palette:?} left gamut at \
                                             c={centre} s={spread} r={register} \
                                             d={density} m={mode}: {v}"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// MetalVJ keeps its single hue family: every stop stays red-led, so
    /// the brand has not leaked into blue or green.
    #[test]
    fn the_metalvj_gradient_stays_red() {
        for centre in [-180.0f32, -90.0, 0.0, 90.0, 180.0] {
            for register in [0.0f32, 0.5, 1.0] {
                for density in [0.0f32, 0.5, 1.0] {
                    for stop in tone_gradient(
                        centre, 0.6, register, density, 0.5, Palette::MetalVJ,
                    ) {
                        assert!(
                            stop[0] >= stop[1] - 1e-3 && stop[0] >= stop[2] - 1e-3,
                            "not red-led at c={centre} r={register} d={density}: {stop:?}"
                        );
                    }
                }
            }
        }
    }

    /// A non-finite input must not reach `to_srgb`: it falls back to the neutral
    /// gradient and still paints four displayable stops.
    #[test]
    fn a_non_finite_input_does_not_panic() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let stops = tone_gradient(bad, bad, bad, bad, bad, Palette::Spectrum);
            // The fallbacks are spread 0, register 0.5, density 0, mode 0.5 and
            // the accent centre — a finite, focused two-stop gradient.
            assert!(distinct(stops) <= 2, "a NaN input must fall back to focused");
            for stop in stops {
                for v in stop {
                    assert!(v.is_finite() && (0.0..=1.0).contains(&v));
                }
            }
        }
    }
}
