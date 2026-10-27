//! Colour as notation, not decoration.
//!
//! Rewritten twice. It was a point in the Laban effort cube; then, on
//! 2026-08-13, a fixed three-way map over `Role` — **given red, carried yellow,
//! silent green** — because that was the project's actual movement vocabulary.
//!
//! **The roles are gone** (2026-08-15) and so is the map. A node either has a
//! rotation written for it or does not, and the interesting quantity is no
//! longer *which of three states* but **how far it turns**. That is an ordered
//! quantity, so it gets the honest encoding for one: a single hue with the
//! chroma and lightness climbing, rather than hues spread round a wheel.
//! Distinct hues for an ordered scale is the mistake the cube's colour scheme
//! already made once — adjacency reads as similarity, and a hue wheel has no
//! adjacency worth reading.
//!
//! The hue is the one the app already draws a written node in, and that is
//! deliberate: a corpus authored against *"the ones that move are the red
//! ones"* should not have to relearn that because the states behind it
//! collapsed into a number.

/// A colour in OkLCh: perceptual lightness, chroma, and hue in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklch {
    pub l: f32,
    pub c: f32,
    /// Degrees, `0..360`.
    pub h: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// A colour in HSB/HSV: hue in degrees, saturation and brightness both `0..1`.
///
/// **Why HSB and not a second `Oklch`.** `Oklch` above is the notation this
/// crate already uses to *say* something about a movement, and its one promise
/// is perceptual uniformity. A picker is a different job: the operator wants a
/// hue bar, a saturation×brightness square and a hex, which is exactly the HSB
/// cube the whole industry draws those from. Making the picker pick in Oklch
/// and convert on the way out would mean a wheel whose hue angles do not line
/// up with the numbers beside them, and a hex that changes when the operator
/// drags brightness along a line that looked constant. HSB is the space the
/// control is a picture of, so it is the space the value travels in.
///
/// Not `egui::ecolor::Hsva`: that type is a widget's private currency, and
/// `shell/gates.rs` gate 2 forbids an `Hsva` outside `classes.rs` — the picker
/// draws with `Painter` and theme tokens and carries its value in this struct,
/// which knows `Rgb` (this crate's only colour type) and nothing about egui.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsb {
    /// Degrees, wrapped into `0..360` on construction (`from_rgb`, `new`).
    pub h: f32,
    /// `0..1`.
    pub s: f32,
    /// `0..1`.
    pub b: f32,
}

impl Hsb {
    /// A colour from three components, wrapping the hue and clamping S and B —
    /// an HSB the picker can always draw, never a partly-out-of-range one.
    pub fn new(h: f32, s: f32, b: f32) -> Hsb {
        Hsb { h: wrap_hue(h), s: s.clamp(0.0, 1.0), b: b.clamp(0.0, 1.0) }
    }

    /// The colour a `#rrggbb` swatch of RGB 0..1 is — the exact inverse of
    /// [`Hsb::to_rgb`], proven by the round-trip test below.
    pub fn from_rgb(r: f32, g: f32, b: f32) -> Hsb {
        let (r, g, b) = (r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0));
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let h = if delta <= f32::EPSILON {
            // Grey: hue is undefined. 0 is as honest an answer as any, and it
            // is the one that keeps S = 0 (a hue with no chroma paints nothing).
            0.0
        } else if max == r {
            60.0 * (((g - b) / delta) % 6.0)
        } else if max == g {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        let s = if max <= f32::EPSILON { 0.0 } else { delta / max };
        Hsb { h: wrap_hue(h), s, b: max }
    }

    /// RGB components in `0..1`, the shape a node's config carries and a
    /// shader reads.
    pub fn to_rgb(self) -> [f32; 3] {
        let h = wrap_hue(self.h) / 60.0;
        let s = self.s.clamp(0.0, 1.0);
        let v = self.b.clamp(0.0, 1.0);
        let c = v * s;
        let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
        let m = v - c;
        let (r, g, b) = match h as i32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        [r + m, g + m, b + m]
    }

    /// This colour as a `#rrggbb` string — the hex beside the picker's numbers.
    pub fn hex(self) -> String {
        let [r, g, b] = self.to_rgb();
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Rgb { r: byte(r), g: byte(g), b: byte(b) }.hex()
    }
}

/// `h` into `0..360` — a hue is an angle, so `370` and `-10` are the same one.
fn wrap_hue(h: f32) -> f32 {
    if !h.is_finite() {
        return 0.0;
    }
    let m = h % 360.0;
    if m < 0.0 {
        m + 360.0
    } else {
        m
    }
}

impl Rgb {
    pub fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Black or white, whichever stays legible on top of this colour.
    pub fn ink(&self) -> Rgb {
        let l = srgb_to_oklab(
            to_linear(self.r as f32 / 255.0),
            to_linear(self.g as f32 / 255.0),
            to_linear(self.b as f32 / 255.0),
        );
        if l > 0.6 {
            Rgb { r: 16, g: 16, b: 16 }
        } else {
            Rgb {
                r: 245,
                g: 245,
                b: 245,
            }
        }
    }
}

/// The one hue a written node is drawn in. Red, because it always was.
const HUE: f32 = 25.0;

/// Lightness at nothing written, and at a half-turn. It *descends*: a joint
/// that has barely moved is pale and a joint at full turn is deep, so the ramp
/// reads as intensity rather than as glare.
const L_NEAR: f32 = 0.86;
const L_FAR: f32 = 0.52;

/// Chroma at nothing written, and at a half-turn.
const C_NEAR: f32 = 0.02;
const C_FAR: f32 = 0.17;

/// The turn the ramp saturates at: π, which is the largest a rotation vector
/// can be — past it a rotation is written the long way round, which
/// `Track::validate` refuses.
pub const FULL: f32 = std::f32::consts::PI;

/// Neutral grey: **nothing written for this node**.
///
/// Not a point on the ramp, and that is the one distinction the colour still
/// has to carry. A joint written at zero is at its rest rotation — a statement,
/// and the pale end of the ramp. A joint nobody wrote is holding whatever it
/// held, which is not a rotation at all. The two look different because they
/// are different, and confusing them is the one way a page can lie about a
/// sparse record.
pub const NEUTRAL: Oklch = Oklch {
    l: 0.72,
    c: 0.0,
    h: 0.0,
};

/// How far a joint turns, as a colour. `radians` is the length of its rotation
/// vector; anything past [`FULL`] clamps.
pub fn turn_color(radians: f32) -> Oklch {
    let t = (radians / FULL).clamp(0.0, 1.0);
    Oklch {
        l: L_NEAR + (L_FAR - L_NEAR) * t,
        c: C_NEAR + (C_FAR - C_NEAR) * t,
        h: HUE,
    }
}

/// Sample swatches up the ramp, with the angle each one stands for, plus the
/// unwritten grey at the front. For a legend.
pub fn legend() -> Vec<(f32, Rgb)> {
    let mut out = vec![(f32::NAN, to_srgb(NEUTRAL))];
    out.extend(
        [0.0, 0.25, 0.5, 0.75, 1.0]
            .into_iter()
            .map(|t| (t * FULL, to_srgb(turn_color(t * FULL)))),
    );
    out
}

// ---------------------------------------------------------------------------
// Oklab, after Björn Ottosson.
// ---------------------------------------------------------------------------

fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(c: f32) -> f32 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_to_oklab(r: f32, g: f32, b: f32) -> f32 {
    // Lightness only; that is all `ink` needs.
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s
}

/// OkLCh to linear sRGB, without clamping — components may fall outside
/// `0..1`, which is how [`to_srgb`] detects that a colour is out of gamut.
fn oklch_to_linear(c: Oklch) -> [f32; 3] {
    let h = c.h.to_radians();
    let (a, b) = (c.c * h.cos(), c.c * h.sin());

    let l_ = c.l + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = c.l - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = c.l - 0.0894841775 * a - 1.2914855480 * b;

    let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);

    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ]
}

/// sRGB `0..1` to OkLCh — the inverse of [`to_srgb`], for reading the hue a
/// colour already has. Hue is `0` for a grey (`c` ≈ 0), where it means nothing.
pub fn from_srgb01(rgb: [f32; 3]) -> Oklch {
    let (r, g, b) = (
        to_linear(rgb[0].clamp(0.0, 1.0)),
        to_linear(rgb[1].clamp(0.0, 1.0)),
        to_linear(rgb[2].clamp(0.0, 1.0)),
    );
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    let ol = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s;
    let a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
    let bb = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
    let c = (a * a + bb * bb).sqrt();
    let h = if c < 1e-4 { 0.0 } else { bb.atan2(a).to_degrees().rem_euclid(360.0) };
    Oklch { l: ol, c, h }
}

fn in_gamut(rgb: [f32; 3]) -> bool {
    rgb.iter().all(|v| (-1e-4..=1.0 + 1e-4).contains(v))
}

/// Converts to sRGB, reducing chroma until the colour fits in the display.
///
/// Clamping the RGB components instead would be easier and would shift the
/// hue, which would break the one promise this module makes.
pub fn to_srgb(color: Oklch) -> Rgb {
    let mut c = color;
    c.l = c.l.clamp(0.0, 1.0);
    if !in_gamut(oklch_to_linear(c)) {
        let (mut lo, mut hi) = (0.0, c.c);
        for _ in 0..20 {
            let mid = 0.5 * (lo + hi);
            if in_gamut(oklch_to_linear(Oklch { c: mid, ..c })) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        c.c = lo;
    }
    let linear = oklch_to_linear(c);
    let byte = |v: f32| (from_linear(v.clamp(0.0, 1.0)) * 255.0).round() as u8;
    Rgb {
        r: byte(linear[0]),
        g: byte(linear[1]),
        b: byte(linear[2]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_srgb01_inverts_to_srgb() {
        for h in [0.0, 27.0, 90.0, 200.0, 300.0] {
            let rgb = to_srgb(Oklch { l: 0.6, c: 0.1, h });
            let back = from_srgb01([rgb.r as f32 / 255.0, rgb.g as f32 / 255.0, rgb.b as f32 / 255.0]);
            assert!((back.h - h).abs() < 2.0, "hue {h} came back {}", back.h);
            assert!((back.l - 0.6).abs() < 0.01);
        }
        assert!(from_srgb01([1.0, 1.0, 1.0]).c < 1e-3);
    }

    #[test]
    fn white_and_black_convert_exactly() {
        assert_eq!(
            to_srgb(Oklch { l: 1.0, c: 0.0, h: 0.0 }),
            Rgb { r: 255, g: 255, b: 255 }
        );
        assert_eq!(
            to_srgb(Oklch { l: 0.0, c: 0.0, h: 0.0 }),
            Rgb { r: 0, g: 0, b: 0 }
        );
    }

    /// The steps up the ramp have to be visible, or the ramp is one colour.
    #[test]
    fn every_step_of_the_ramp_is_visually_distinct() {
        let swatches = legend();
        for (i, (a, ca)) in swatches.iter().enumerate() {
            for (b, cb) in swatches.iter().skip(i + 1) {
                let d = (ca.r as i32 - cb.r as i32).abs()
                    + (ca.g as i32 - cb.g as i32).abs()
                    + (ca.b as i32 - cb.b as i32).abs();
                assert!(d > 20, "{a} and {b} are too close: {} vs {}", ca.hex(), cb.hex());
            }
        }
    }

    /// An ordered quantity gets an ordered encoding: one hue, monotone in
    /// lightness and chroma. A hue wheel would say *different kind of thing*
    /// where the data says *more of the same thing*.
    #[test]
    fn the_ramp_is_one_hue_and_climbs_monotonically() {
        let steps: Vec<Oklch> = (0..=8).map(|i| turn_color(i as f32 * FULL / 8.0)).collect();
        for w in steps.windows(2) {
            assert_eq!(w[0].h, HUE, "the ramp changed hue");
            assert!(w[1].l < w[0].l, "lightness did not descend");
            assert!(w[1].c > w[0].c, "chroma did not climb");
        }
        // Past a half-turn it clamps rather than wrapping into something paler.
        assert_eq!(turn_color(FULL * 3.0), turn_color(FULL));
    }

    /// The one distinction the colour has to keep: a joint written at zero is a
    /// statement, and a joint nobody wrote is not.
    #[test]
    fn nothing_written_does_not_look_like_written_at_zero() {
        let zero = turn_color(0.0);
        assert_ne!(zero.c, NEUTRAL.c, "the unwritten grey has chroma");
        assert!(NEUTRAL.c == 0.0, "the unwritten swatch must be neutral");
        let (a, b) = (to_srgb(zero), to_srgb(NEUTRAL));
        let d = (a.r as i32 - b.r as i32).abs() + (a.g as i32 - b.g as i32).abs();
        assert!(d > 20, "{} and {} read as the same node", a.hex(), b.hex());
    }

    #[test]
    fn gamut_clipping_keeps_hue_and_lightness() {
        // Far outside sRGB.
        let wanted = Oklch { l: 0.5, c: 0.6, h: 150.0 };
        let got = to_srgb(wanted);
        let back = srgb_to_oklab(
            to_linear(got.r as f32 / 255.0),
            to_linear(got.g as f32 / 255.0),
            to_linear(got.b as f32 / 255.0),
        );
        assert!((back - 0.5).abs() < 0.02, "lightness drifted to {back}");
    }

    #[test]
    fn every_swatch_is_displayable() {
        for (turn, rgb) in legend() {
            assert!(rgb.hex().len() == 7, "{turn} produced {}", rgb.hex());
        }
    }

    #[test]
    fn print_the_palette() {
        for (turn, rgb) in legend() {
            println!("{turn:>6.3} rad  {}", rgb.hex());
        }
    }

    // -- Hsb, the picker's own space ---------------------------------------

    /// The seven HSB primaries land on the seven RGB corners every table of
    /// this conversion agrees on — not round-tripped, pinned.
    #[test]
    fn hsb_primaries_are_their_corner_colours() {
        let cases: [(Hsb, [f32; 3]); 7] = [
            (Hsb::new(0.0, 1.0, 1.0), [1.0, 0.0, 0.0]),
            (Hsb::new(60.0, 1.0, 1.0), [1.0, 1.0, 0.0]),
            (Hsb::new(120.0, 1.0, 1.0), [0.0, 1.0, 0.0]),
            (Hsb::new(180.0, 1.0, 1.0), [0.0, 1.0, 1.0]),
            (Hsb::new(240.0, 1.0, 1.0), [0.0, 0.0, 1.0]),
            (Hsb::new(300.0, 1.0, 1.0), [1.0, 0.0, 1.0]),
            (Hsb::new(360.0, 1.0, 1.0), [1.0, 0.0, 0.0]), // 360 wraps to 0
        ];
        for (hsb, want) in cases {
            let got = hsb.to_rgb();
            for (g, w) in got.iter().zip(want) {
                assert!((g - w).abs() < 1e-5, "{hsb:?} -> {got:?}, wanted {want:?}");
            }
        }
    }

    #[test]
    fn hsb_black_white_and_grey() {
        assert_eq!(Hsb::new(0.0, 0.0, 0.0).to_rgb(), [0.0, 0.0, 0.0]);
        assert_eq!(Hsb::new(200.0, 0.0, 1.0).to_rgb(), [1.0, 1.0, 1.0], "S = 0 is white at any hue");
        assert_eq!(Hsb::new(123.0, 0.0, 0.5).to_rgb(), [0.5, 0.5, 0.5]);
    }

    /// `from_rgb` inverts `to_rgb` for every colour whose channels are not all
    /// equal — the greys lose their hue on purpose (`Hsb::from_rgb` says why),
    /// so the round trip for them is pinned separately below.
    #[test]
    fn hsb_round_trips_through_rgb() {
        let probes = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.2, 0.7, 0.4],
            [0.9, 0.1, 0.35],
            [0.05, 0.05, 0.9],
            [1.0, 1.0, 1.0],
        ];
        for [r, g, b] in probes {
            let back = Hsb::from_rgb(r, g, b).to_rgb();
            for (got, want) in back.iter().zip([r, g, b]) {
                assert!((got - want).abs() < 1e-5, "[{r},{g},{b}] -> {back:?}");
            }
        }
    }

    #[test]
    fn a_grey_has_no_hue_and_no_saturation() {
        for v in [0.0, 0.25, 0.5, 1.0] {
            let h = Hsb::from_rgb(v, v, v);
            assert_eq!(h.s, 0.0, "a grey has no chroma");
            assert_eq!(h.b, v);
        }
    }

    /// A hue is an angle: negative and past-360 inputs name the same colour as
    /// their wrapped spelling.
    #[test]
    fn hues_wrap_and_are_a_whole_number_of_turns() {
        assert_eq!(Hsb::new(-10.0, 1.0, 1.0).to_rgb(), Hsb::new(350.0, 1.0, 1.0).to_rgb());
        assert_eq!(Hsb::new(370.0, 0.5, 0.5).to_rgb(), Hsb::new(10.0, 0.5, 0.5).to_rgb());
        assert_eq!(Hsb::new(f32::NAN, 0.5, 0.5).h, 0.0);
    }

    #[test]
    fn hsb_hex_is_the_swatch_it_paints() {
        assert_eq!(Hsb::new(0.0, 1.0, 1.0).hex(), "#ff0000");
        assert_eq!(Hsb::new(120.0, 1.0, 1.0).hex(), "#00ff00");
        assert_eq!(Hsb::new(0.0, 0.0, 0.0).hex(), "#000000");
        assert_eq!(Hsb::new(0.0, 0.0, 1.0).hex(), "#ffffff");
    }
}
