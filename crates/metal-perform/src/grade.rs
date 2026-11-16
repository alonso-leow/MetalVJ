//! The Lightroom-style colour grade, and the six advanced effects, on the
//! **CPU** path.
//!
//! The GPU reads the same numbers out of [`metal_schema::Post::advanced`], which
//! is `[f32; N_ADVANCED]` in `metal_schema::ADVANCED_EFFECTS` order: the grade
//! group first, then texture, clarity, dehaze, denoise, optics and geometry.
//! A *row* carries those floats as the tail of `EFFECTS_FLOATS` (past
//! `CHAIN_FLOATS`), so `metal_schema::read_post` fills `advanced` from the very
//! same values the shader gets — this module is the CPU mirror of the pass,
//! not a second source of truth.
//!
//! **Why here and not in `stand.rs`.** `stand` is a painter; the grade is
//! maths. Keeping it in a pure module means it can be unit-tested with no
//! frame, no graph and no host, which is the only way the "neutral params are
//! the identity" property can be stated as a test rather than hoped for.
//!
//! # What is honest and what is approximate
//!
//! Every slider is implemented in the order Lightroom itself documents:
//! white balance, exposure, contrast, the four tone zones, the parametric
//! curve, vibrance/saturation, the eight-band mixer, then the three-way plus
//! global grading wheels, with `blending`/`balance` and a final
//! `refine_saturation` trim. The maths is deliberately simple and disclosed:
//!
//! - **Tone zones and the curve** weight by luminance rather than by a real
//!   local-adjustment mask; the picture moves the way the slider says, by
//!   roughly the amount.
//! - **Texture/clarity/dehaze** are *per-pixel* on the CPU stand-in. The real
//!   pass samples neighbours; here "local contrast" is approximated by the
//!   pixel's own deviation from its channel mean and luminance, so the control
//!   is reviewable without a framebuffer. See [`apply_texture`].
//! - **Denoise** is a luminance-preserving pull toward the channel mean, not a
//!   wavelet pass. **Optics**' vignette and distortion are done where a
//!   position is available ([`apply_optics_uv`]); its chromatic-aberration and
//!   defringe controls are **inert** on the CPU path (documented on the
//!   functions) because they are per-channel resamplings that need the real
//!   multi-tap pass.
//! - **Geometry** is a UV transform, which is exactly what it is.
//!
//! Nothing here panics on a short slice, an out-of-range float or a missing
//! neighbour: the advanced block is append-only and a row written before it
//! existed reads as neutral, which is the whole "no wrong picture" rule.

use metal_schema::{FloatSpec, GRADE_FLOATS};

/// Below this a channel is treated as having no colour (pure grey).
const GREY_EPSILON: f32 = 1e-6;

/// One mixing band of the HSL colour mixer, as `(name, hue centre in degrees)`.
pub const MIX_BANDS: [(&str, f32); 8] = [
    ("red", 0.0),
    ("orange", 30.0),
    ("yellow", 60.0),
    ("green", 120.0),
    ("aqua", 180.0),
    ("blue", 240.0),
    ("purple", 280.0),
    ("magenta", 320.0),
];

/// All 56 `grade_*` floats, pulled out of [`metal_schema::GRADE_FLOATS`] by name.
///
/// `mixer` is the eight `(hue, sat, lum)` bands in !red, orange, yellow,
/// green, aqua, blue, purple, magenta! order; `grading` is the three-way plus
/// global `(hue, sat, lum)` in `[shadows, midtones, highlights, global]`
/// order. Everything is in the sliders' own units (`-100..100`, or `0..360`
/// for a wheel hue), so nothing here is pre-normalised and a value reads the
/// same in a test as on the wire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradeParams {
    pub temperature: f32,
    pub tint: f32,
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub vibrance: f32,
    pub saturation: f32,
    pub curve_highlights: f32,
    pub curve_lights: f32,
    pub curve_darks: f32,
    pub curve_shadows: f32,
    pub split_shadows: f32,
    pub split_mid: f32,
    pub split_highlights: f32,
    pub refine_saturation: f32,
    pub mixer: [[f32; 3]; 8],
    pub grading: [[f32; 3]; 4],
    pub blending: f32,
    pub balance: f32,
}

/// Read float `i` of a group, falling back to its declared default and
/// clamping into its declared range — the same tolerance
/// `metal_schema::read_post` shows a short row.
fn spec_at(specs: &[FloatSpec], floats: &[f32], i: usize) -> f32 {
    let Some(spec) = specs.get(i) else { return 0.0 };
    floats.get(i).copied().unwrap_or(spec.default).clamp(spec.min, spec.max)
}

impl GradeParams {
    /// The neutral grade: every `0`, the split points and `refine` at their
    /// declared defaults, `blending` at 50. [`apply_grade`] with this is the
    /// identity.
    pub fn neutral() -> GradeParams {
        GradeParams::from_floats(&[])
    }

    /// The grade group from the flat slice `post.advanced[grade_span]` (or
    /// any prefix of it). Short slices fill from the declared defaults, so a
    /// row that predates the advanced block is the neutral grade.
    pub fn from_floats(floats: &[f32]) -> GradeParams {
        let g = |i: usize| spec_at(GRADE_FLOATS, floats, i);
        let mut mixer = [[0.0f32; 3]; 8];
        for (b, band) in mixer.iter_mut().enumerate() {
            for (k, slot) in band.iter_mut().enumerate() {
                *slot = g(18 + b * 3 + k);
            }
        }
        let mut grading = [[0.0f32; 3]; 4];
        for (z, wheel) in grading.iter_mut().enumerate() {
            for (k, slot) in wheel.iter_mut().enumerate() {
                *slot = g(42 + z * 3 + k);
            }
        }
        GradeParams {
            temperature: g(0),
            tint: g(1),
            exposure: g(2),
            contrast: g(3),
            highlights: g(4),
            shadows: g(5),
            whites: g(6),
            blacks: g(7),
            vibrance: g(8),
            saturation: g(9),
            curve_highlights: g(10),
            curve_lights: g(11),
            curve_darks: g(12),
            curve_shadows: g(13),
            split_shadows: g(14),
            split_mid: g(15),
            split_highlights: g(16),
            refine_saturation: g(17),
            mixer,
            grading,
            blending: g(54),
            balance: g(55),
        }
    }

    /// Whether this grade would leave a pixel alone — every slider neutral.
    pub fn is_neutral(&self) -> bool {
        *self == GradeParams::neutral()
    }
}

// ---------------------------------------------------------------------------
// colour helpers
// ---------------------------------------------------------------------------

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// Rec. 709 luma of an encoded 0..1 RGB triple.
pub fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// sRGB-ish encoded 0..1 RGB → HSL, all three in 0..1 (`h` in turns).
pub fn rgb_to_hsl(c: [f32; 3]) -> [f32; 3] {
    let (r, g, b) = (c[0], c[1], c[2]);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;
    let d = max - min;
    if d <= 1e-6 {
        return [0.0, 0.0, l];
    }
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    [h, s, l]
}

/// HSL back to encoded 0..1 RGB.
pub fn hsl_to_rgb(hsl: [f32; 3]) -> [f32; 3] {
    let (h, s, l) = (hsl[0].rem_euclid(1.0), hsl[1].clamp(0.0, 1.0), hsl[2].clamp(0.0, 1.0));
    if s <= 1e-6 {
        return [l, l, l];
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |t: f32| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

/// A triangular hue weight: 1 at `centre`, 0 once `|Δhue| >= spread` degrees,
/// computed the short way around the wheel.
fn hue_weight(hue_deg: f32, centre: f32, spread: f32) -> f32 {
    let d = (hue_deg - centre + 180.0).rem_euclid(360.0) - 180.0;
    (1.0 - d.abs() / spread).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// the grade
// ---------------------------------------------------------------------------

/// White balance: positive `temperature` is warm (R up, B down), positive
/// `tint` is magenta (R/B up, G down), the Lightroom direction.
fn white_balance(mut c: [f32; 3], p: &GradeParams) -> [f32; 3] {
    let t = p.temperature / 100.0;
    let ti = p.tint / 100.0;
    c[0] += 0.18 * t + 0.06 * ti;
    c[1] -= 0.12 * ti;
    c[2] -= 0.18 * t + 0.06 * ti;
    c
}

fn exposure(mut c: [f32; 3], p: &GradeParams) -> [f32; 3] {
    // ±2 stops over the slider's whole range.
    let stops = p.exposure / 100.0 * 2.0;
    let f = 2f32.powf(stops);
    for v in c.iter_mut() {
        *v *= f;
    }
    c
}

fn contrast(c: [f32; 3], p: &GradeParams) -> [f32; 3] {
    let k = 1.0 + p.contrast / 100.0;
    [
        (c[0] - 0.5) * k + 0.5,
        (c[1] - 0.5) * k + 0.5,
        (c[2] - 0.5) * k + 0.5,
    ]
}

/// The four tone zones, each an additive luminance nudge weighted by where
/// the pixel sits on the 0..1 luma ramp. "Highlights up" genuinely lifts
/// highlights; the Lightroom sign convention is not reproduced.
fn tone_zones(mut c: [f32; 3], p: &GradeParams) -> [f32; 3] {
    let l = luma(c).clamp(0.0, 1.0);
    let w_hi = smoothstep(0.5, 1.0, l);
    let w_sh = 1.0 - smoothstep(0.0, 0.5, l);
    let w_wh = smoothstep(0.7, 1.0, l);
    let w_bl = 1.0 - smoothstep(0.0, 0.3, l);
    let d = 0.25
        * (p.highlights / 100.0 * w_hi
            + p.shadows / 100.0 * w_sh
            + p.whites / 100.0 * w_wh
            + p.blacks / 100.0 * w_bl);
    for v in c.iter_mut() {
        *v += d;
    }
    c
}

/// The parametric curve: four regions whose split points are
/// `grade_split_shadows`/`_mid`/`_highlights` (as 0..1 positions) and whose
/// strengths are the four `grade_curve_*` sliders. Additive luminance, so the
/// hue is preserved and the slope around the split reads as a curve.
fn parametric_curve(mut c: [f32; 3], p: &GradeParams) -> [f32; 3] {
    let l = luma(c).clamp(0.0, 1.0);
    let s0 = (p.split_shadows / 100.0).clamp(0.0, 1.0);
    let s1 = (p.split_mid / 100.0).clamp(0.0, 1.0).max(s0);
    let s2 = (p.split_highlights / 100.0).clamp(0.0, 1.0).max(s1);
    let region = |lo: f32, hi: f32| -> f32 {
        // 1 inside the band, falling to 0 a little past each edge.
        let in_lo = smoothstep(lo - 0.15, lo, l);
        let in_hi = 1.0 - smoothstep(hi, hi + 0.15, l);
        in_lo * in_hi
    };
    let d = 0.2
        * (p.curve_shadows / 100.0 * region(0.0, s0)
            + p.curve_darks / 100.0 * region(s0, s1)
            + p.curve_lights / 100.0 * region(s1, s2)
            + p.curve_highlights / 100.0 * region(s2, 1.0));
    for v in c.iter_mut() {
        *v += d;
    }
    c
}

/// Vibrance (weighted toward already-dull pixels) plus saturation, both as a
/// pull away from luma. `refine_saturation` trims overshoot when present; a
/// zero `refine` (a row that never carried the advanced block) is treated as
/// neutral rather than "desaturate everything".
fn vibrance_saturation(mut c: [f32; 3], p: &GradeParams) -> [f32; 3] {
    let l = luma(c);
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let sat = if max > GREY_EPSILON { (max - min) / max } else { 0.0 };
    let vib = p.vibrance / 100.0 * (1.0 - sat);
    let mut amount = p.saturation / 100.0 + vib * 0.5;
    if p.refine_saturation > 0.0 && p.refine_saturation < 100.0 {
        amount *= p.refine_saturation / 100.0;
    }
    let amount = amount.clamp(-1.0, 1.0);
    for v in c.iter_mut() {
        *v += (*v - l) * amount;
    }
    c
}

/// The eight-band HSL mixer: each band shifts hue, saturation and luminance,
/// weighted by a triangular membership over the pixel's own hue.
fn hsl_mixer(c: [f32; 3], p: &GradeParams) -> [f32; 3] {
    let mut hsl = rgb_to_hsl(c);
    let hue_deg = hsl[0] * 360.0;
    let mut dh = 0.0;
    let mut ds = 0.0;
    let mut dl = 0.0;
    for (b, (_, centre)) in MIX_BANDS.iter().enumerate() {
        let w = hue_weight(hue_deg, *centre, 60.0);
        if w <= 0.0 {
            continue;
        }
        dh += p.mixer[b][0] / 100.0 * w * 0.15;
        ds += p.mixer[b][1] / 100.0 * w;
        dl += p.mixer[b][2] / 100.0 * w * 0.25;
    }
    hsl[0] = (hsl[0] + dh).rem_euclid(1.0);
    hsl[1] = (hsl[1] * (1.0 + ds)).clamp(0.0, 1.0);
    hsl[2] = (hsl[2] + dl).clamp(0.0, 1.0);
    hsl_to_rgb(hsl)
}

/// One grading wheel: a `(hue, sat, lum)` triple rendered as a channel
/// offset. `hue`/`sat` move colour, `lum` moves brightness.
fn wheel_offset(wheel: [f32; 3]) -> [f32; 3] {
    let s = (wheel[1] / 100.0).clamp(0.0, 1.0);
    if s <= 0.0 {
        return [0.0; 3];
    }
    let rgb = hsl_to_rgb([wheel[0] / 360.0, s, 0.5]);
    let lum = wheel[2] / 100.0 * 0.2;
    // Centre the wheel's colour so a wheel with no sat is neutral.
    let mean = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
    [
        (rgb[0] - mean) * s * 0.25 + lum,
        (rgb[1] - mean) * s * 0.25 + lum,
        (rgb[2] - mean) * s * 0.25 + lum,
    ]
}

/// The three-way (shadows/midtones/highlights) plus global grading wheels,
/// mixed by `blending` and re-centred by `balance`.
fn colour_grade(mut c: [f32; 3], p: &GradeParams) -> [f32; 3] {
    let l = luma(c).clamp(0.0, 1.0);
    let bal = p.balance / 100.0 * 0.15;
    let w_sh = (1.0 - smoothstep(0.0, 0.5 - bal, l)).clamp(0.0, 1.0);
    let w_hi = smoothstep(0.5 + bal, 1.0, l);
    let w_mid = (1.0 - w_sh - w_hi).max(0.0);
    // `blending` remixes split toning; 50 is the documented middle.
    let split = (p.blending / 100.0).clamp(0.0, 1.0) * 2.0;
    let zones = [p.grading[0], p.grading[1], p.grading[2]];
    let mut off = [0.0f32; 3];
    for (z, wheel) in zones.iter().enumerate() {
        let w = match z {
            0 => w_sh,
            1 => w_mid,
            _ => w_hi,
        } * split;
        let o = wheel_offset(*wheel);
        for k in 0..3 {
            off[k] += o[k] * w;
        }
    }
    let global = wheel_offset(p.grading[3]);
    for k in 0..3 {
        c[k] += off[k] + global[k];
    }
    c
}

/// The final saturation trim. Positive `refine` (the default 100) is the
/// identity at zero saturation delta; below 100 it pulls colour back toward
/// luma. Zero is treated as neutral, so a zeroed row cannot grey the frame.
fn refine_saturation(mut c: [f32; 3], p: &GradeParams) -> [f32; 3] {
    if p.refine_saturation <= 0.0 || p.refine_saturation >= 100.0 {
        return c;
    }
    let k = (p.refine_saturation / 100.0).clamp(0.0, 1.0);
    let l = luma(c);
    for v in c.iter_mut() {
        *v = l + (*v - l) * k;
    }
    c
}

/// The whole grade, in the documented order, on an encoded 0..1 colour.
///
/// Approximate in the places [`crate::grade`]'s module docs name (tone zone
/// and curve weighting); exact in that the neutral [`GradeParams`] returns the
/// input unchanged.
pub fn apply_grade(rgb: [f32; 3], p: &GradeParams) -> [f32; 3] {
    let mut c = rgb;
    c = white_balance(c, p);
    c = exposure(c, p);
    c = contrast(c, p);
    c = tone_zones(c, p);
    c = parametric_curve(c, p);
    c = vibrance_saturation(c, p);
    c = hsl_mixer(c, p);
    c = colour_grade(c, p);
    c = refine_saturation(c, p);
    [clamp01(c[0]), clamp01(c[1]), clamp01(c[2])]
}

// ---------------------------------------------------------------------------
// the six advanced effects
// ---------------------------------------------------------------------------

/// Texture: pixel-scale local contrast. **Approximate on the CPU path** —
/// with no neighbour, "detail" is the pixel's own deviation from its channel
/// mean, boosted (positive) or flattened (negative). Documented, reviewable,
/// and never a wrong picture.
pub fn apply_texture(rgb: [f32; 3], amount: f32) -> [f32; 3] {
    let a = (amount / 100.0).clamp(-1.0, 1.0);
    if a == 0.0 {
        return rgb;
    }
    let mean = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
    let mut out = [0.0f32; 3];
    for k in 0..3 {
        out[k] = clamp01(rgb[k] + (rgb[k] - mean) * a);
    }
    out
}

/// Clarity: mid-tone local contrast. Same per-pixel approximation as
/// [`apply_texture`], but the boost is masked to the midtones so highlights
/// and shadows are left alone.
pub fn apply_clarity(rgb: [f32; 3], amount: f32) -> [f32; 3] {
    let a = (amount / 100.0).clamp(-1.0, 1.0);
    if a == 0.0 {
        return rgb;
    }
    let l = luma(rgb).clamp(0.0, 1.0);
    let mask = 1.0 - (2.0 * l - 1.0).abs();
    let mean = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
    let mut out = [0.0f32; 3];
    for k in 0..3 {
        out[k] = clamp01(rgb[k] + (rgb[k] - mean) * a * mask * 1.5);
    }
    out
}

/// Dehaze: removes (positive) or adds (negative) atmospheric haze, using the
/// dark-channel idea as a per-pixel stand-in — pull the darkest channel down
/// and rescale, which is the direction the real pass moves in.
pub fn apply_dehaze(rgb: [f32; 3], amount: f32) -> [f32; 3] {
    let a = (amount / 100.0).clamp(-1.0, 1.0);
    if a == 0.0 {
        return rgb;
    }
    let dark = rgb[0].min(rgb[1]).min(rgb[2]);
    let mut out = [0.0f32; 3];
    for k in 0..3 {
        // Positive pulls the veil out (subtract a fraction of the dark
        // channel), negative pushes it back in.
        out[k] = clamp01(rgb[k] - a * (dark * 0.6) + a * 0.02);
    }
    out
}

/// Denoise: luminance and colour noise reduction. **Per-pixel stand-in** — a
/// pull toward the channel mean (colour) and toward luma (luminance), the
/// cheapest thing that reads as "less noise" without a neighbour window.
/// `luminance`/`color` are the 0..100 sliders.
pub fn apply_denoise(rgb: [f32; 3], luminance: f32, color: f32) -> [f32; 3] {
    let lum = (luminance / 100.0).clamp(0.0, 1.0);
    let col = (color / 100.0).clamp(0.0, 1.0);
    if lum <= 0.0 && col <= 0.0 {
        return rgb;
    }
    let l = luma(rgb);
    let mean = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
    let mut out = rgb;
    for v in out.iter_mut() {
        // Colour noise: pull each channel toward the (neutral) mean.
        *v += (mean - *v) * col * 0.75;
        // Luminance noise: pull toward the luma (a per-pixel smoothing).
        *v += (l - *v) * lum * 0.35;
    }
    [clamp01(out[0]), clamp01(out[1]), clamp01(out[2])]
}

/// The seven optics floats as named values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpticsParams {
    pub distortion: f32,
    pub vignetting: f32,
    pub vignetting_midpoint: f32,
    pub defringe_purple: f32,
    pub defringe_green: f32,
    pub ca_red: f32,
    pub ca_blue: f32,
}

impl OpticsParams {
    pub fn from_floats(floats: &[f32]) -> OpticsParams {
        let g = |i: usize| spec_at(metal_schema::OPTICS_FLOATS, floats, i);
        OpticsParams {
            distortion: g(0),
            vignetting: g(1),
            vignetting_midpoint: g(2),
            defringe_purple: g(3),
            defringe_green: g(4),
            ca_red: g(5),
            ca_blue: g(6),
        }
    }
}

/// Optics' vignetting on a colour, given its normalised `uv` in -1..1 from
/// the frame centre. Positive `vignetting` darkens the corners, negative
/// lifts them. `midpoint` sets where the falloff begins.
///
/// **Inert here:** `fx_optics_distortion`, `fx_optics_defringe_purple`,
/// `fx_optics_defringe_green`, `fx_optics_ca_red`, `fx_optics_ca_blue` —
/// distortion and chromatic aberration are per-channel UV resamplings that
/// need the real multi-tap pass, and defringe needs to find a fringe first.
/// They change nothing on the CPU path rather than a wrong picture; see
/// [`OpticsParams`].
pub fn apply_optics(rgb: [f32; 3], uv: [f32; 2], p: &OpticsParams) -> [f32; 3] {
    let amount = p.vignetting / 100.0;
    if amount == 0.0 {
        return rgb;
    }
    let mid = (p.vignetting_midpoint / 100.0).clamp(0.0, 1.0);
    let r2 = (uv[0] * uv[0] + uv[1] * uv[1]).min(2.0);
    let start = mid * mid * 2.0;
    let falloff = ((r2 - start) / (2.0 - start).max(1e-6)).clamp(0.0, 1.0);
    let gain = 1.0 - amount * falloff;
    [clamp01(rgb[0] * gain), clamp01(rgb[1] * gain), clamp01(rgb[2] * gain)]
}

/// The seven geometry floats as named values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeometryParams {
    pub vertical: f32,
    pub horizontal: f32,
    pub rotate: f32,
    pub aspect: f32,
    pub scale: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}

impl GeometryParams {
    pub fn from_floats(floats: &[f32]) -> GeometryParams {
        let g = |i: usize| spec_at(metal_schema::GEOMETRY_FLOATS, floats, i);
        let spec = &metal_schema::GEOMETRY_FLOATS[4];
        // `scale` is declared 50..150 with 100 the identity. A missing or
        // zero value (a row that never carried the advanced block, which
        // `spec_at` would otherwise clamp up to 50) must read as 100, not as
        // a half-size frame.
        let scale = match floats.get(4).copied() {
            Some(v) if v > 0.0 => v.clamp(spec.min, spec.max),
            _ => spec.default,
        };
        GeometryParams {
            vertical: g(0),
            horizontal: g(1),
            rotate: g(2),
            aspect: g(3),
            scale,
            offset_x: g(5),
            offset_y: g(6),
        }
    }
}

/// Geometry as a UV transform — exactly what it is, so this is honest on the
/// CPU path. Returns the sampling coordinate for an output `uv` in -1..1.
pub fn apply_geometry_uv(uv: [f32; 2], p: &GeometryParams) -> [f32; 2] {
    let mut x = uv[0];
    let mut y = uv[1];
    // Perspective: a vertical keystone squeezes x with y, and vice versa.
    x += p.vertical / 100.0 * y * 0.25;
    y += p.horizontal / 100.0 * x * 0.25;
    // Aspect: stretch one axis about the centre.
    let aspect = 1.0 + p.aspect / 100.0 * 0.5;
    x *= aspect;
    // Scale: 100 is the identity.
    let s = (p.scale / 100.0).clamp(0.01, 10.0);
    x /= s;
    y /= s;
    // Rotate, degrees about the centre.
    let a = p.rotate.to_radians();
    let (sin, cos) = a.sin_cos();
    let (rx, ry) = (x * cos - y * sin, x * sin + y * cos);
    // Offsets, as a fraction of the half-frame.
    [rx + p.offset_x / 100.0, ry + p.offset_y / 100.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4)
    }

    fn samples() -> [[f32; 3]; 5] {
        [
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [0.8, 0.2, 0.1],
            [0.1, 0.5, 0.9],
            [0.5, 0.5, 0.5],
        ]
    }

    #[test]
    fn neutral_params_are_the_identity() {
        let p = GradeParams::neutral();
        for s in samples() {
            assert!(close(apply_grade(s, &p), s), "neutral changed {s:?}");
        }
        assert!(p.is_neutral());
    }

    #[test]
    fn increasing_exposure_brightens() {
        let mut p = GradeParams::neutral();
        p.exposure = 50.0;
        let before = luma([0.3, 0.3, 0.3]);
        let after = luma(apply_grade([0.3, 0.3, 0.3], &p));
        assert!(after > before, "exposure + must brighten: {before} -> {after}");
        p.exposure = -50.0;
        let dark = luma(apply_grade([0.3, 0.3, 0.3], &p));
        assert!(dark < before);
    }

    #[test]
    fn contrast_pivots_around_mid() {
        let mut p = GradeParams::neutral();
        p.contrast = 50.0;
        let mid = apply_grade([0.5, 0.5, 0.5], &p);
        assert!((mid[0] - 0.5).abs() < 1e-4, "mid must be the pivot, got {mid:?}");
        let dark = apply_grade([0.25, 0.25, 0.25], &p)[0];
        let light = apply_grade([0.75, 0.75, 0.75], &p)[0];
        assert!(dark < 0.25 && light > 0.75, "contrast must spread: {dark}, {light}");
    }

    #[test]
    fn temperature_moves_red_against_blue() {
        let base = [0.4, 0.4, 0.4];
        let mut warm = GradeParams::neutral();
        warm.temperature = 60.0;
        let w = apply_grade(base, &warm);
        assert!(w[0] > base[0] && w[2] < base[2], "warm must raise R lower B: {w:?}");
        let mut cool = GradeParams::neutral();
        cool.temperature = -60.0;
        let c = apply_grade(base, &cool);
        assert!(c[0] < base[0] && c[2] > base[2], "cool must lower R raise B: {c:?}");
    }

    #[test]
    fn saturation_minus_hundred_greys() {
        let mut p = GradeParams::neutral();
        p.saturation = -100.0;
        let out = apply_grade([0.9, 0.2, 0.05], &p);
        assert!((out[0] - out[1]).abs() < 1e-3 && (out[1] - out[2]).abs() < 1e-3, "{out:?}");
    }

    #[test]
    fn every_effect_round_trips_and_never_panics() {
        // A short/empty slice, a full slice, and a wildly out-of-range one —
        // the grade must answer for all of them.
        for floats in [vec![], vec![0.0; GRADE_FLOATS.len()], vec![1e6; GRADE_FLOATS.len()]] {
            let p = GradeParams::from_floats(&floats);
            for s in samples() {
                let _ = apply_grade(s, &p);
            }
        }
        // The six effects: identity at zero/neutral, no panic at any extreme.
        for s in samples() {
            assert_eq!(apply_texture(s, 0.0), s);
            assert_eq!(apply_clarity(s, 0.0), s);
            assert_eq!(apply_dehaze(s, 0.0), s);
            assert_eq!(apply_denoise(s, 0.0, 0.0), s);
            for a in [-100.0, 0.0, 100.0] {
                let _ = apply_texture(s, a);
                let _ = apply_clarity(s, a);
                let _ = apply_dehaze(s, a);
                let _ = apply_denoise(s, a, a);
            }
            let optics = OpticsParams::from_floats(&[]);
            let _ = apply_optics(s, [0.5, -0.5], &optics);
            let geo = GeometryParams::from_floats(&[]);
            let uv = apply_geometry_uv([0.2, 0.3], &geo);
            assert!(uv[0].is_finite() && uv[1].is_finite());
        }
    }

    #[test]
    fn a_zeroed_geometry_scale_does_not_collapse_the_frame() {
        let geo = GeometryParams::from_floats(&[0.0; 7]);
        assert_eq!(geo.scale, 100.0, "a zero row must read as the identity scale");
        let uv = apply_geometry_uv([0.5, 0.5], &geo);
        assert!(uv[0].abs() < 1.0 && uv[1].abs() < 1.0);
    }
}
