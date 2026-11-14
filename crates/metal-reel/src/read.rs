//! What a frame *is*, in twelve numbers — the digest of the image, read the
//! other way round from the digest of the music.
//!
//! # The other way round
//!
//! `docs/AUDIO_FEATURES.md` fixes the musical digest at sixteen columns on an
//! 8 Hz grid: one row per 125 ms, held between rows, a staircase by
//! construction. This is the same idea pointed at a picture, and two things
//! about it are deliberately not the same:
//!
//! - **It is not on a grid tied to the music.** A clip has its own clock. The
//!   ladder is sampled at [`crate::index::FPS`] frames a second of *the clip*,
//!   and a descriptor between two samples is the **interpolation** of the two —
//!   which is why a transmission does not stairstep even though the digest
//!   does. `Clip::at` is that interpolation.
//! - **It is computed once, at ingest, and never again.** The music has to be
//!   analysed while it plays; a file does not change.
//!
//! # The vocabulary is Lumetri's, and that is on purpose
//!
//! Exposure, contrast, highlights, shadows, saturation, temperature, tint —
//! that is the Basic Correction panel, and it is the vocabulary somebody who
//! grades footage already has. A descriptor whose columns are `pc1..pc12` would
//! describe the same frames and be unreadable on a lane, and the whole reason
//! `metal-router`'s twelve features are named `energia`/`pulso` rather than `f0`
//! is that *"why did it pick that"* has to be answerable by reading them.
//!
//! Two columns are not on that panel:
//!
//! - **`detalle`** — how much edge there is. It is what separates a wide static
//!   shot from a crowd, and neither exposure nor contrast can tell them apart.
//! - **`movimiento`** — how much the frame changed from the one before it. The
//!   only column here that is not a property of a still, and the one the
//!   Curator leans on hardest: a passage that moves belongs under a passage of
//!   music that moves.
//!
//! # `r`, `g`, `b` are redundant with `temperatura`/`tinte`, and they stay
//!
//! Temperature and tint are a rotation of the three channel means; carrying
//! both is three numbers of genuine redundancy out of twelve. They stay because
//! they are two different questions an operator asks — *"is this shot warm"* is
//! not *"how much red is in it"* — and because both models over this are
//! ridge-penalised, which is precisely the tool for correlated inputs. It is
//! named here rather than left to be discovered as a bug.
//!
//! # 0..1, and this one keeps the promise
//!
//! `metal-dash::digest::Frame` promises every column in 0..1 and does not keep it:
//! measured over the corpus, bandwidth runs to 4209 and onset to 38, because
//! those columns are computed elsewhere and arrive already wrong. Every column
//! here is computed *in this function* and clamped *in this function*, so the
//! promise is enforced where it is made. [`describe`] has a test that walks
//! degenerate frames — pure black, pure white, one pixel — through it.

/// How many numbers describe a frame.
pub const COLUMNS: usize = 12;

/// What each one is. The studio labels the lane with these, so an operator sees
/// "saturacion" and not "i4".
pub const NAMES: [&str; COLUMNS] = [
    "exposure",
    "contrast",
    "altas",
    "bajas",
    "saturation",
    "temperatura",
    "tint",
    "r",
    "g",
    "b",
    "detail",
    "motion",
];

/// A decoded proxy frame: 8-bit RGB, tightly packed, no stride.
pub struct Rgb {
    pub width: u32,
    pub height: u32,
    /// `width * height * 3` bytes.
    pub px: Vec<u8>,
}

impl Rgb {
    pub fn pixels(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }

    fn at(&self, i: usize) -> (f32, f32, f32) {
        let p = i * 3;
        (
            self.px[p] as f32 / 255.0,
            self.px[p + 1] as f32 / 255.0,
            self.px[p + 2] as f32 / 255.0,
        )
    }
}

/// Rec. 709 luma, on the **encoded** signal rather than on light.
///
/// Not a shortcut. Every column here is a Lumetri reading and Lumetri reads the
/// signal, so "exposure" means the mean of what is in the file. Linearising
/// first would give a number that is more physical and that no colourist would
/// recognise — a normally exposed shot would report about 0.2 and look
/// underexposed to anyone reading the lane.
pub fn luma(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Where a pixel counts as a highlight, and where as a shadow. The two
/// thresholds Lumetri's own Highlights and Shadows sliders act around, and they
/// are stated here once because `altas` and `bajas` are otherwise two magic
/// numbers in a loop.
const HIGH: f32 = 0.75;
const LOW: f32 = 0.25;

/// `1 - exp(-k·g)`, for the two columns that measure a *difference* rather than
/// a level.
///
/// # Why these two are not scaled linearly like everything else
///
/// Measured on real material, the mean absolute luma difference between two
/// adjacent pixels runs about 0.005 on something soft and about 0.08 on
/// something as busy as footage gets; between two consecutive frames it is
/// about 0.02 on ordinary movement and 0.3–0.6 across a cut. Both quantities
/// live in the first tenth of their arithmetic range and then have a long tail
/// nothing ever reaches.
///
/// A linear scale with a clamp has to choose which end to serve. Tuned for the
/// tail, every real frame reports in the first fifth and the column is
/// effectively constant — which is what it did: `detalle` came out 0.08 for a
/// mandelbrot and 0.11 for a shot full of structure, two numbers a model cannot
/// tell apart. Tuned for the middle, a fine dither and a cut both clamp to 1.0
/// and the tail stops existing.
///
/// This has no tail to clip and spends its resolution where the material is. It
/// is not a perceptual curve and does not claim to be; it is the honest shape
/// for a quantity that is bounded above by something nothing reaches.
fn compress(mean_difference: f64, k: f64) -> f32 {
    (1.0 - (-k * mean_difference.max(0.0)).exp()) as f32
}

/// Describes one frame, against the frame before it.
///
/// `previous` is `None` for the first frame of a clip, which makes
/// `movimiento` zero — the honest answer, since there is nothing to have moved
/// from. It is not carried across a cut between clips: two unrelated files
/// differ completely, and a `movimiento` of 1.0 at every clip boundary would be
/// a column that mostly reports where the boundaries are.
pub fn describe(frame: &Rgb, previous: Option<&Rgb>) -> [f32; COLUMNS] {
    let mut out = [0.0f32; COLUMNS];
    let n = frame.pixels();
    if n == 0 || frame.px.len() < n * 3 {
        return out;
    }

    let (mut sr, mut sg, mut sb, mut sl, mut ssat) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let (mut high, mut low) = (0usize, 0usize);
    let mut lumas: Vec<f32> = Vec::with_capacity(n);

    for i in 0..n {
        let (r, g, b) = frame.at(i);
        let l = luma(r, g, b);
        sr += r as f64;
        sg += g as f64;
        sb += b as f64;
        sl += l as f64;
        // HSV saturation. Not chroma in a perceptual space: the question this
        // column answers is "how far off grey is this", which is what the
        // Saturation slider does, and the perceptual version would disagree
        // with the control an operator reaches for.
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        ssat += if max > 1e-6 { ((max - min) / max) as f64 } else { 0.0 };
        if l > HIGH {
            high += 1;
        }
        if l < LOW {
            low += 1;
        }
        lumas.push(l);
    }

    let nf = n as f32;
    let (mr, mg, mb) = ((sr / n as f64) as f32, (sg / n as f64) as f32, (sb / n as f64) as f32);

    out[0] = (sl / n as f64) as f32;

    // Before the spread, because that reorders `lumas` and this reads it in
    // pixel order.
    out[10] = detail(frame, &lumas);

    // Contrast as a spread, not as a standard deviation, and the same spread
    // `metal_router::features` uses for `dinamica`: the 10th to the 90th
    // percentile. A handful of blown speculars or one black letterbox bar move
    // a min-to-max range to 1.0 and tell you nothing about the shot.
    out[1] = spread(&mut lumas);

    out[2] = high as f32 / nf;
    out[3] = low as f32 / nf;
    out[4] = (ssat / n as f64) as f32;

    // Warm is high. Lumetri's Temperature slider goes blue at the left and
    // amber at the right, so red over blue is the positive direction, and 0.5
    // is neutral rather than 0 — a column that rested at an end could only ever
    // be pushed one way by a model that standardises its inputs.
    out[5] = 0.5 + (mr - mb) * 0.5;
    // And magenta is high, which is also Lumetri's direction: Tint goes green
    // at the left. So it is the *other two* against green.
    out[6] = 0.5 + ((mr + mb) * 0.5 - mg) * 0.5;

    out[7] = mr;
    out[8] = mg;
    out[9] = mb;
    out[11] = match previous {
        Some(p) if p.width == frame.width && p.height == frame.height => motion(frame, p),
        // A size change is a different clip, or a ladder rebuilt at another
        // proxy width. Either way there is no motion to report between them.
        _ => 0.0,
    };

    for v in out.iter_mut() {
        *v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    }
    out
}

/// The 10th to the 90th percentile of `v`, nearest-rank, **reordering `v`**.
///
/// The percentiles are the same ones `metal_router::features` takes for `dinamica`
/// and are picked nearest-rank for the same reason: interpolating between two
/// of a few hundred thousand samples buys nothing and has edge cases at the ends
/// that this does not.
///
/// # Why this selects rather than sorts
///
/// It used to clone `lumas` and sort it. At a 640 px ladder that is a 1.2 MB
/// allocation and an n·log n comparison sort of ~316,000 floats **per rung**,
/// and it was about nine tenths of the cost of indexing a folder — a
/// seven-minute clip took two minutes, of which ffmpeg was nine seconds.
///
/// Two `select_nth_unstable` calls answer exactly the same question in O(n) with
/// no allocation: the value at a rank is all that is wanted, and sorting the
/// other 316,000 into order to read two of them is work whose result is thrown
/// away. The second select runs on the sub-slice below the first, which is
/// already known to hold every value under the 90th percentile.
fn spread(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let last = v.len() - 1;
    let hi_at = (last as f32 * 0.9).round() as usize;
    let lo_at = (last as f32 * 0.1).round() as usize;
    let (_, hi, _) = v.select_nth_unstable_by(hi_at, f32::total_cmp);
    let hi = *hi;
    // `hi_at` is 0 only when there is one sample, and then both ranks are it.
    let lo = if hi_at == 0 {
        hi
    } else {
        let below = &mut v[..hi_at];
        let (_, lo, _) = below.select_nth_unstable_by(lo_at.min(hi_at - 1), f32::total_cmp);
        *lo
    };
    hi - lo
}

/// Mean gradient magnitude between adjacent pixels, over every pixel.
///
/// # This function was subsampled twice and was wrong both times
///
/// It began as a two-pixel gradient taken every two pixels, which is exactly
/// blind at that period: one-pixel stripes sample the *same* value at both ends
/// and report a `detalle` of zero, so the busiest frame a library can hold came
/// out as its flattest — in range, deterministic, nothing in any log.
///
/// The obvious repair was to keep a stride but measure the gradient over one
/// pixel. That is blind too, at the stride's own period: with a stride of
/// three, every visited pixel lands at the head of a three-pixel run and its
/// neighbour is always the same value. The gate here caught the second version
/// with the same message it caught the first.
///
/// **The lesson is the general one and it is why there is no stride now.** A
/// regular subsample of a derivative aliases against a regular pattern at
/// *some* period, and footage is full of regular patterns: dither, grain, a
/// grid, blinds, a crowd, a chain-link fence. The saving was never real either
/// — every other column here already walks the whole frame once, and this runs
/// at ingest beside a JPEG decode that costs several times more.
///
/// The number does move a little with proxy width, since a wider ladder
/// resolves finer structure. That is stated rather than hidden: `detalle` is
/// compared between clips *of one library*, and a library has one width by
/// construction.
fn detail(frame: &Rgb, lumas: &[f32]) -> f32 {
    /// Chosen so that the busiest ordinary footage — foliage, a crowd, small
    /// text — lands around three quarters rather than around a tenth.
    const RESPONSE: f64 = 20.0;
    let (w, h) = (frame.width as usize, frame.height as usize);
    if w < 2 || h < 2 {
        return 0.0;
    }
    let mut total = 0.0f64;
    let mut count = 0usize;
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            let i = y * w + x;
            total += ((lumas[i + 1] - lumas[i]).abs() + (lumas[i + w] - lumas[i]).abs()) as f64;
            count += 1;
        }
    }
    if count == 0 {
        return 0.0;
    }
    compress(total / count as f64, RESPONSE)
}

/// Mean absolute luma difference against the previous frame, scaled so that
/// ordinary movement lands in the middle of the range rather than in the first
/// tenth of it.
///
/// A cut saturates this to 1.0, and that is wanted: a cut *is* the largest
/// thing that can happen between two frames, and the Curator using it to prefer
/// cutty footage under a busy passage is the behaviour, not a bug.
///
/// Every pixel, for the same reason [`detail`] walks every pixel. This is a
/// mean rather than a derivative so a stride would be a fair estimate of it,
/// but a module with one subsampled loop in it is a module where the next
/// person has to work out which loops are safe to subsample, and that question
/// has already cost two bugs here.
fn motion(frame: &Rgb, previous: &Rgb) -> f32 {
    /// Chosen so that a cut between two ordinary shots — which differ by far
    /// less than black from white — still reads as near 1, and a pan reads as a
    /// third rather than as a twentieth.
    const RESPONSE: f64 = 8.0;
    let n = frame.pixels();
    if n == 0 {
        return 0.0;
    }
    let mut total = 0.0f64;
    for i in 0..n {
        let (r, g, b) = frame.at(i);
        let (pr, pg, pb) = previous.at(i);
        total += (luma(r, g, b) - luma(pr, pg, pb)).abs() as f64;
    }
    compress(total / n as f64, RESPONSE)
}

/// Two descriptors, blended. What a transmission reads between two rungs of the
/// ladder — the interpolation this whole module is defined around.
pub fn lerp(a: &[f32; COLUMNS], b: &[f32; COLUMNS], t: f32) -> [f32; COLUMNS] {
    let t = t.clamp(0.0, 1.0);
    let mut out = [0.0f32; COLUMNS];
    for i in 0..COLUMNS {
        out[i] = a[i] + (b[i] - a[i]) * t;
    }
    out
}

/// The mean of a stretch of descriptors — what a whole passage of a clip looks
/// like, which is the unit the Curator actually chooses between.
pub fn average(rows: &[[f32; COLUMNS]]) -> [f32; COLUMNS] {
    let mut out = [0.0f32; COLUMNS];
    if rows.is_empty() {
        return out;
    }
    for r in rows {
        for i in 0..COLUMNS {
            out[i] += r[i];
        }
    }
    for v in out.iter_mut() {
        *v /= rows.len() as f32;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, r: u8, g: u8, b: u8) -> Rgb {
        let mut px = Vec::with_capacity((w * h * 3) as usize);
        for _ in 0..(w * h) {
            px.extend_from_slice(&[r, g, b]);
        }
        Rgb { width: w, height: h, px }
    }

    /// Half black, half white, split down the middle.
    fn split(w: u32, h: u32) -> Rgb {
        let mut px = Vec::with_capacity((w * h * 3) as usize);
        for _ in 0..h {
            for x in 0..w {
                let v = if x < w / 2 { 0u8 } else { 255u8 };
                px.extend_from_slice(&[v, v, v]);
            }
        }
        Rgb { width: w, height: h, px }
    }

    #[test]
    fn every_column_lands_in_0_1_on_frames_designed_to_break_it() {
        // The promise the musical digest makes and does not keep. This is the
        // gate that keeps it here, and the frames are the degenerate ones: no
        // pixels, one pixel, pure black, pure white, and a hard edge.
        let cases = [
            Rgb { width: 0, height: 0, px: vec![] },
            flat(1, 1, 255, 0, 0),
            flat(8, 8, 0, 0, 0),
            flat(8, 8, 255, 255, 255),
            split(16, 16),
        ];
        for (k, frame) in cases.iter().enumerate() {
            let d = describe(frame, Some(&flat(8, 8, 0, 0, 0)));
            for (i, v) in d.iter().enumerate() {
                assert!(
                    v.is_finite() && (0.0..=1.0).contains(v),
                    "case {k}: {} = {v}",
                    NAMES[i]
                );
            }
        }
    }

    #[test]
    fn a_flat_grey_frame_has_no_contrast_no_saturation_and_no_detail() {
        let d = describe(&flat(32, 32, 128, 128, 128), None);
        assert!(d[1] < 1e-3, "contraste {}", d[1]);
        assert!(d[4] < 1e-3, "saturacion {}", d[4]);
        assert!(d[10] < 1e-3, "detalle {}", d[10]);
        assert!((d[5] - 0.5).abs() < 1e-3, "temperatura {}", d[5]);
        assert!((d[6] - 0.5).abs() < 1e-3, "tinte {}", d[6]);
    }

    #[test]
    fn exposure_and_the_two_tails_agree_with_each_other() {
        let dark = describe(&flat(16, 16, 20, 20, 20), None);
        let bright = describe(&flat(16, 16, 240, 240, 240), None);
        assert!(dark[0] < bright[0]);
        assert_eq!(dark[3], 1.0, "a dark frame is all shadow");
        assert_eq!(dark[2], 0.0, "and no highlight");
        assert_eq!(bright[2], 1.0);
        assert_eq!(bright[3], 0.0);
    }

    #[test]
    fn a_hard_edge_reads_as_contrast_and_as_detail() {
        // The two are different questions and this frame is the one that
        // separates them: the same black and white, one edge or a thousand.
        let one_edge = describe(&split(64, 64), None);
        assert!(one_edge[1] > 0.9, "contraste {}", one_edge[1]);

        let mut px = Vec::new();
        for _ in 0..64 {
            for x in 0..64u32 {
                let v = if x % 2 == 0 { 0u8 } else { 255 };
                px.extend_from_slice(&[v, v, v]);
            }
        }
        let stripes = describe(&Rgb { width: 64, height: 64, px }, None);
        assert!((stripes[1] - one_edge[1]).abs() < 0.05, "same contrast");
        // Stated as a gap rather than as a multiple: `compress` is deliberately
        // flat at the top, so a sixty-fold difference in the raw measurement is
        // a threefold one in the column, and a ratio assertion here would be an
        // assertion about the curve rather than about the two frames.
        assert!(stripes[10] > 0.9 && one_edge[10] < 0.4, "{} vs {}", stripes[10], one_edge[10]);
    }

    /// The regression gate for the two bugs `detail` describes, and it caught
    /// the second fix as well as the first.
    ///
    /// A subsampled derivative is blind at the subsample's own period, so the
    /// busiest frame a library can hold reports as its flattest. Every period
    /// from one pixel up has to register, in both axes, because grain, dither,
    /// blinds and a distant crowd are all one or two pixels at proxy width.
    #[test]
    fn detail_sees_stripes_of_every_period_in_both_axes() {
        for period in 1..=5usize {
            for vertical in [false, true] {
                let mut px = Vec::new();
                for y in 0..64usize {
                    for x in 0..64usize {
                        let along = if vertical { y } else { x };
                        let v = if (along / period) % 2 == 0 { 0u8 } else { 255 };
                        px.extend_from_slice(&[v, v, v]);
                    }
                }
                let d = describe(&Rgb { width: 64, height: 64, px }, None);
                assert!(
                    d[10] > 0.2,
                    "{}stripes {period}px apart reported detalle {} — blind at that period",
                    if vertical { "horizontal " } else { "vertical " },
                    d[10]
                );
            }
        }
    }

    #[test]
    fn temperature_is_warm_at_the_top_and_tint_is_magenta_at_the_top() {
        // Lumetri's own directions. Getting either backwards is invisible in
        // every other test here and wrong on every lane that draws them.
        let amber = describe(&flat(8, 8, 220, 160, 60), None);
        let blue = describe(&flat(8, 8, 60, 160, 220), None);
        assert!(amber[5] > 0.5 && blue[5] < 0.5, "{} vs {}", amber[5], blue[5]);

        let magenta = describe(&flat(8, 8, 200, 60, 200), None);
        let green = describe(&flat(8, 8, 60, 200, 60), None);
        assert!(magenta[6] > 0.5 && green[6] < 0.5, "{} vs {}", magenta[6], green[6]);
    }

    #[test]
    fn motion_is_zero_against_itself_and_saturates_across_a_cut() {
        let a = flat(32, 32, 40, 40, 40);
        let b = flat(32, 32, 250, 250, 250);
        assert_eq!(describe(&a, Some(&flat(32, 32, 40, 40, 40)))[11], 0.0);
        // Near 1 rather than exactly 1: `compress` has no clip in it, so
        // nothing reaches the top and a cut between two *ordinary* shots — the
        // real case, which differs by far less than black from white — is not
        // squashed against the same rail as this one.
        assert!(describe(&b, Some(&a))[11] > 0.95);
    }

    /// The scaling gate, and the measurement that forced `compress` to exist.
    ///
    /// Both of these columns describe a difference, and both live in the first
    /// tenth of their arithmetic range on real material. The failure to avoid
    /// is a column that is technically in 0..1 and in practice constant: the
    /// linear version reported 0.08 for a mandelbrot and 0.11 for a shot full
    /// of structure, which is not a difference a model can use.
    #[test]
    fn a_difference_column_uses_its_range_on_material_that_actually_exists() {
        // A pattern with a transition every eight pixels — about as much
        // structure as ordinary footage carries, not a test chart.
        let mut px = Vec::new();
        for y in 0..64usize {
            for x in 0..64usize {
                let v = if ((x / 8) + (y / 8)) % 2 == 0 { 90u8 } else { 150 };
                px.extend_from_slice(&[v, v, v]);
            }
        }
        let busy = describe(&Rgb { width: 64, height: 64, px }, None);
        assert!(
            (0.15..0.95).contains(&busy[10]),
            "ordinary structure reported detalle {} — the column is compressed against an end",
            busy[10]
        );

        // And a modest frame-to-frame change is not squashed to nothing.
        let moved = describe(&flat(32, 32, 120, 120, 120), Some(&flat(32, 32, 96, 96, 96)));
        assert!(
            (0.2..0.9).contains(&moved[11]),
            "a tenth of a stop of movement reported movimiento {}",
            moved[11]
        );
    }

    #[test]
    fn a_previous_frame_of_another_size_is_not_motion() {
        // Otherwise every clip boundary and every rebuilt ladder reports a cut.
        let d = describe(&flat(32, 32, 40, 40, 40), Some(&flat(16, 16, 250, 250, 250)));
        assert_eq!(d[11], 0.0);
    }

    #[test]
    fn interpolating_between_two_rungs_stays_between_them() {
        let a = describe(&flat(8, 8, 10, 10, 10), None);
        let b = describe(&flat(8, 8, 240, 240, 240), None);
        let mid = lerp(&a, &b, 0.5);
        assert!((mid[0] - (a[0] + b[0]) / 2.0).abs() < 1e-6);
        assert_eq!(lerp(&a, &b, 0.0), a);
        assert_eq!(lerp(&a, &b, 1.0), b);
        // And out of range is clamped rather than extrapolated: a descriptor
        // past the end of the ladder is the last rung, not a frame that never
        // existed.
        assert_eq!(lerp(&a, &b, 4.0), b);
        assert_eq!(lerp(&a, &b, -1.0), a);
    }

    #[test]
    fn averaging_nothing_is_zeros_rather_than_a_panic() {
        assert_eq!(average(&[]), [0.0; COLUMNS]);
    }

    #[test]
    fn selecting_the_spread_agrees_with_sorting_for_it() {
        // `spread` replaced a clone-and-sort that was nine tenths of the cost of
        // indexing a folder. It has to answer the same question exactly, not
        // approximately — `contraste` feeds a trained model, so a spread that
        // drifted by a rank would be a silent retraining.
        fn by_sorting(v: &[f32]) -> f32 {
            let mut s = v.to_vec();
            s.sort_by(f32::total_cmp);
            let at = |q: f32| s[((s.len() - 1) as f32 * q).round() as usize];
            at(0.9) - at(0.1)
        }
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        for n in [1usize, 2, 3, 9, 10, 11, 999, 1000] {
            let v: Vec<f32> = (0..n)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    (seed >> 40) as f32 / 16777216.0
                })
                .collect();
            assert_eq!(spread(&mut v.clone()), by_sorting(&v), "at n = {n}");
        }
        // Every value the same is a real frame — a black slate — and the
        // degenerate case for a selection.
        assert_eq!(spread(&mut [0.4; 64]), 0.0);
        assert_eq!(spread(&mut []), 0.0);
    }
}
