//! The tone reducer — §4's math, and nothing else.
//!
//! Basic Pitch emits, once per model frame, two 88-long posteriorgrams:
//! `note[k]` (is key `k` sounding) and `onset[k]` (did it just start). The
//! reducer turns the stream of those into one small "recent harmony" summary
//! per digest tick. It is the whole of decision D4, and it is pure: no model,
//! no audio, no clock, just `push_frame` in and [`Reducer::columns`] out.
//!
//! # Why an average, not notes
//!
//! A note event needs its *end*, which live is a second or two away. An
//! exponential average of the posteriors only needs the newest frames, so the
//! delay is one inference hop (see `the tone design notes` §1). The average is also the
//! thing colour actually wants: a mood, not a melody.
//!
//! # Why the decay is `exp(-dt/τ)`
//!
//! `w[k]` is the decayed activation per key. Each frame decays everything that
//! came before and adds this frame's evidence scaled by `dt`, so a note held
//! for a second weighs about `τ` (1.5 s) worth regardless of the frame rate.
//! The onset term `β·onset[k]` (β = 0.1) is a small *impulse*: it makes a new
//! chord readable on the frame it starts rather than only after it has
//! accumulated, so colour does not feel late.
//!
//! # Why the hue is a vector (D3)
//!
//! `tone_x`/`tone_y` are the pitch-class distribution projected onto the circle
//! of fifths as a 2D vector, then mapped into 0..1. Storing a vector and not an
//! angle is the point: angles break under pooling and smoothing (359° and 1°
//! average to 180°, the far side of the circle), while vectors average to what
//! they should. The circle of fifths, not the chromatic circle, keeps related
//! keys neighbouring hues — C and G land 30° apart rather than 210°.
//!
//! This module is **always compiled**, even with the `tone` feature off: it is
//! the frozen, testable heart of the block, and a minimal build can still run
//! its tests.

use metal_schema::TONE;

/// Keys the model covers: MIDI 21..=108 inclusive, one array slot each.
pub const KEYS: usize = 88;
/// Lowest MIDI note in the posteriorgram; `key k` is MIDI `k + MIDI_LOW`.
pub const MIDI_LOW: u32 = 21;
/// Highest MIDI note in the posteriorgram, inclusive.
pub const MIDI_HIGH: u32 = 108;
/// The §4 window, τ = 1.5 s: how much recent past the average remembers.
pub const DEFAULT_TAU: f32 = 1.5;
/// Onset impulse weight — small, so it nudges rather than dominates.
pub const BETA: f32 = 0.1;
/// A key counts as "active" for the density count above this activation.
pub const EPS: f32 = 1e-3;

/// The decayed state: one activation per key, plus the confidence latch.
///
/// Deliberately small and `Clone`/`Debug`: a reducer is a value, so a test can
/// snapshot it and the offline path can hold one per song.
#[derive(Clone, Debug)]
pub struct Reducer {
    /// τ in seconds, always finite and strictly positive (see [`Reducer::with_tau`]).
    tau: f32,
    /// `w[k]` — the decayed activation of key `k`.
    w: [f32; KEYS],
    /// `tone_conf`: the decayed max of `note`, so silence lets it fall.
    conf: f32,
}

impl Reducer {
    /// A reducer at the default τ = [`DEFAULT_TAU`].
    pub fn new() -> Self {
        Self::with_tau(DEFAULT_TAU)
    }

    /// A reducer at an explicit τ.
    ///
    /// τ is clamped strictly positive because it is a divisor in the decay
    /// `exp(-dt/τ)`: zero would divide by zero, a negative value would make the
    /// decay grow. A non-finite τ (NaN, ∞) falls back to the default rather
    /// than poisoning every later frame. `with_tau(0.0)` therefore still
    /// decays, just very fast.
    pub fn with_tau(tau: f32) -> Self {
        let tau = if tau.is_finite() { tau.max(1e-3) } else { DEFAULT_TAU };
        Reducer { tau, w: [0.0; KEYS], conf: 0.0 }
    }

    /// Forget all history — the start of a new song or a re-seek.
    pub fn reset(&mut self) {
        self.w = [0.0; KEYS];
        self.conf = 0.0;
    }

    /// Advance one Basic Pitch frame.
    ///
    /// `note` and `onset` are 88 long, key 0 = MIDI [`MIDI_LOW`]. `dt` is the
    /// frame interval in seconds (Basic Pitch's own `FFT_HOP / 22050`). A
    /// non-finite or negative `dt` is ignored, and a non-finite posterior is
    /// treated as zero, so one bad model output cannot poison the state.
    pub fn push_frame(&mut self, note: &[f32; KEYS], onset: &[f32; KEYS], dt: f32) {
        if !dt.is_finite() || dt < 0.0 {
            return;
        }
        let decay = (-dt / self.tau).exp();
        let mut max_note = 0.0f32;
        for k in 0..KEYS {
            let n = if note[k].is_finite() { note[k].clamp(0.0, 1.0) } else { 0.0 };
            let o = if onset[k].is_finite() { onset[k].clamp(0.0, 1.0) } else { 0.0 };
            // §4 D4: decay what was there, add this frame's sustained evidence
            // weighted by dt, add a small onset impulse.
            self.w[k] = self.w[k] * decay + n * dt + BETA * o;
            max_note = max_note.max(n);
        }
        // D7's confidence: the loudest pitched key in this frame, but decayed,
        // so it falls to zero a few τ after the last pitched frame.
        self.conf = (self.conf * decay).max(max_note).clamp(0.0, 1.0);
    }

    /// The six columns in `metal_schema::TONE_COLUMNS` order:
    /// `tone_x`, `tone_y`, `tone_register`, `tone_density`, `tone_mode`,
    /// `tone_conf`.
    pub fn columns(&self) -> [f32; TONE] {
        let pc = self.pitch_classes();
        let (x, y) = self.circle(pc);
        [x, y, self.register(), self.density(), self.mode(pc), self.conf]
    }

    /// The confidence column — see [`Reducer::push_frame`]. 0 with no model or
    /// no pitched content, which is what makes the chroma fallback (D8) fire.
    pub fn conf(&self) -> f32 {
        self.conf
    }

    /// How diffuse the harmony is, for the gradient's spread: 0 is a single
    /// focused pitch class (one narrow hue), 1 is maximally spread.
    ///
    /// The stored `(x, y)` are the unit harmony vector halved and offset into
    /// 0..1, so the *unit* length is recovered by doubling the distance from the
    /// centre `(0.5, 0.5)`. A single pitch class sits exactly on the rim, length
    /// 1, spread 0; a distribution spread evenly around the circle cancels to
    /// the centre, length 0, spread 1.
    pub fn spread(&self) -> f32 {
        let (x, y) = self.circle(self.pitch_classes());
        let (cx, cy) = (x - 0.5, y - 0.5);
        let len = (cx * cx + cy * cy).sqrt() * 2.0;
        (1.0 - len).clamp(0.0, 1.0)
    }

    /// The 12 pitch classes: `pc[c]` is the sum of `w[k]` over every key whose
    /// MIDI note is `c` mod 12. Key `k` is MIDI `k + MIDI_LOW`.
    fn pitch_classes(&self) -> [f32; 12] {
        let mut pc = [0.0f32; 12];
        for k in 0..KEYS {
            let c = ((k as u32 + MIDI_LOW) % 12) as usize;
            pc[c] += self.w[k];
        }
        pc
    }

    /// `(tone_x, tone_y)` from the pitch-class sums — the vector on the circle
    /// of fifths mapped into 0..1 (D3).
    ///
    /// `θ_c = 2π·(7c mod 12)/12` puts the fifths in order: C at 0°, G at 30°,
    /// and so on. A silent state (norm ≈ 0) has no direction, so it reads the
    /// neutral centre rather than dividing by zero.
    fn circle(&self, pc: [f32; 12]) -> (f32, f32) {
        let (mut vx, mut vy, mut norm) = (0.0f32, 0.0f32, 0.0f32);
        for c in 0..12 {
            let theta = std::f32::consts::TAU * ((7 * c) % 12) as f32 / 12.0;
            vx += pc[c] * theta.cos();
            vy += pc[c] * theta.sin();
            norm += pc[c];
        }
        if norm <= 1e-6 {
            return (0.5, 0.5);
        }
        (0.5 + 0.5 * (vx / norm), 0.5 + 0.5 * (vy / norm))
    }

    /// `tone_register`: the `w`-weighted mean MIDI note, normalised over
    /// 21..108. Low notes read dark, high notes bright. No weight means no
    /// register, so it sits at the neutral 0.5.
    fn register(&self) -> f32 {
        let sum: f32 = self.w.iter().sum();
        if sum <= 1e-6 {
            return 0.5;
        }
        let midi: f32 = self
            .w
            .iter()
            .enumerate()
            .map(|(k, &w)| w * (k as f32 + MIDI_LOW as f32))
            .sum::<f32>()
            / sum;
        ((midi - MIDI_LOW as f32) / (MIDI_HIGH - MIDI_LOW) as f32).clamp(0.0, 1.0)
    }

    /// `tone_density`: how thick the harmony is, on a soft clip.
    ///
    /// It mixes two things the ear reads as "full": *how many* keys are active
    /// (above [`EPS`], counted against 8) and *how much* activation there is in
    /// total (`Σw`). Either alone is wrong — a single loud key is not thick, and
    /// eight barely-there keys are not either. `t/(1+t)` maps the unbounded sum
    /// into 0..1 without a fitted scale, so a loud chord saturates gently
    /// rather than clipping.
    fn density(&self) -> f32 {
        let active = self.w.iter().filter(|&&w| w > EPS).count() as f32;
        let sum: f32 = self.w.iter().sum();
        let t = active / 8.0 + sum;
        (t / (1.0 + t)).clamp(0.0, 1.0)
    }

    /// `tone_mode`: a major/minor template match on `pc`, 0 = minor, 1 = major.
    ///
    /// The root is the strongest pitch class (lowest index on a tie, 0 if the
    /// distribution is empty). Building the major and minor triads on it and
    /// comparing their `pc` mass gives `0.5 + 0.5·(maj−min)/(maj+min)` — the
    /// major third (root+4) versus the minor third (root+3) over the shared
    /// fifth (root+7). No mass means no mode, so it sits at 0.5.
    fn mode(&self, pc: [f32; 12]) -> f32 {
        let mut r = 0usize;
        let mut best = pc[0];
        for c in 1..12 {
            if pc[c] > best {
                best = pc[c];
                r = c;
            }
        }
        let root = pc[r];
        let maj = root + pc[(r + 4) % 12] + pc[(r + 7) % 12];
        let min = root + pc[(r + 3) % 12] + pc[(r + 7) % 12];
        if maj + min <= 1e-6 {
            return 0.5;
        }
        (0.5 + 0.5 * (maj - min) / (maj + min)).clamp(0.0, 1.0)
    }
}

impl Default for Reducer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Basic Pitch's frame interval: one FFT hop at 22 050 Hz.
    const DT: f32 = 256.0 / 22_050.0;

    fn zeros() -> [f32; KEYS] {
        [0.0; KEYS]
    }

    fn held(keys: &[usize]) -> [f32; KEYS] {
        let mut n = [0.0f32; KEYS];
        for &k in keys {
            n[k] = 1.0;
        }
        n
    }

    fn push(r: &mut Reducer, note: &[f32; KEYS], frames: usize) {
        for _ in 0..frames {
            r.push_frame(note, &zeros(), DT);
        }
    }

    #[test]
    fn a_sustained_c_is_a_focused_vector_at_c() {
        // Middle C is key 39 (39 + 21 = 60, 60 mod 12 = 0). On the circle of
        // fifths C sits at 0°, so the vector points straight at +x: the stored
        // x is 1.0 and the y is the neutral 0.5.
        let mut r = Reducer::new();
        let c = held(&[39]);
        push(&mut r, &c, 60);
        let col = r.columns();
        assert!((col[0] - 1.0).abs() < 1e-3, "tone_x {}", col[0]);
        assert!((col[1] - 0.5).abs() < 1e-3, "tone_y {}", col[1]);

        // One pitch class is maximally focused, so the spread is ~0.
        assert!(r.spread() < 1e-3, "spread {}", r.spread());

        // And the register is exactly where MIDI 60 sits in 21..108.
        let want = (60.0 - 21.0) / 87.0;
        assert!((col[2] - want).abs() < 1e-3, "register {} want {want}", col[2]);
    }

    #[test]
    fn a_major_triad_reads_more_major_than_a_minor_one() {
        // C major = C E G = keys 39, 43, 46; C minor = C Eb G = keys 39, 42, 46.
        // Same decay, same root, same fifth — only the third differs, and that
        // is exactly what tone_mode is for.
        let mut major = Reducer::new();
        let mut minor = Reducer::new();
        push(&mut major, &held(&[39, 43, 46]), 40);
        push(&mut minor, &held(&[39, 42, 46]), 40);
        let mj = major.columns()[4];
        let mn = minor.columns()[4];
        assert!(mj > 0.5, "major mode {mj} should be above 0.5");
        assert!(mn < 0.5, "minor mode {mn} should be below 0.5");
        assert!(mj > mn, "major {mj} must beat minor {mn}");
    }

    #[test]
    fn silence_is_zero_confidence_and_finite_defaults() {
        let mut r = Reducer::new();
        // Even fed a frame of zeros, a fresh reducer has nothing to be
        // confident about, and its derived columns are the neutral defaults
        // rather than NaN.
        r.push_frame(&zeros(), &zeros(), DT);
        let col = r.columns();
        assert_eq!(col[5], 0.0, "tone_conf on silence");
        for (c, &v) in col.iter().enumerate() {
            assert!(v.is_finite(), "column {c} is not finite: {v}");
        }
        assert!((col[0] - 0.5).abs() < 1e-6);
        assert!((col[1] - 0.5).abs() < 1e-6);
        assert!((col[2] - 0.5).abs() < 1e-6, "no weight means neutral register");
        assert_eq!(col[3], 0.0, "no active keys means zero density");
        assert!((col[4] - 0.5).abs() < 1e-6, "no mass means neutral mode");
    }

    #[test]
    fn adjacent_fifths_average_between_them_not_opposite() {
        // C (key 39, pc 0) and G (key 46, midi 67, pc 7) are neighbours on the
        // circle of fifths: 0° and 30°. On the chromatic circle they are 7
        // semitones apart and would average to something quite different; this
        // is the 359°/1° property D3 buys. The mean vector must land between
        // them — both x and y above the centre.
        let mut r = Reducer::new();
        push(&mut r, &held(&[39, 46]), 60);
        let col = r.columns();
        assert!(col[0] > 0.5, "x {} should lean toward both fifths", col[0]);
        assert!(col[1] > 0.5, "y {} should rise off the axis toward G", col[1]);
        // Focused-ish but not a single class: some spread, but well under 1.
        let s = r.spread();
        assert!(s > 0.0 && s < 0.5, "spread {s} for two adjacent fifths");
    }

    #[test]
    fn a_zero_tau_is_clamped_and_still_decays() {
        // τ is a divisor in the decay; `with_tau(0.0)` must not panic or
        // produce NaN, and the state must still fall back to zero after the
        // pitch stops.
        let mut r = Reducer::with_tau(0.0);
        let c = held(&[39]);
        r.push_frame(&c, &zeros(), DT);
        let before = r.conf();
        assert!(before > 0.5, "confidence should have latched, got {before}");
        for _ in 0..8 {
            r.push_frame(&zeros(), &zeros(), 0.1);
        }
        assert!(r.conf() < before, "conf {} did not decay from {before}", r.conf());
        assert!(r.conf().is_finite(), "conf went non-finite");
        assert!(r.conf() < 1e-3, "a near-zero tau should decay almost at once");
    }

    #[test]
    fn a_non_finite_posterior_cannot_poison_the_state() {
        let mut r = Reducer::new();
        let nan = [f32::NAN; KEYS];
        let inf = [f32::INFINITY; KEYS];
        r.push_frame(&nan, &inf, DT);
        // A later good frame still behaves as if from a clean state.
        let c = held(&[39]);
        r.push_frame(&c, &zeros(), DT);
        let col = r.columns();
        for (i, &v) in col.iter().enumerate() {
            assert!(v.is_finite(), "column {i} is not finite: {v}");
        }
        assert!(r.spread().is_finite());
        assert!(col[5] > 0.0, "a real note after garbage should still register");
    }

    #[test]
    fn density_grows_with_active_keys() {
        let mut one = Reducer::new();
        let mut many = Reducer::new();
        push(&mut one, &held(&[39]), 30);
        push(&mut many, &held(&[39, 42, 46, 50, 53]), 30);
        assert!(
            many.columns()[3] > one.columns()[3],
            "five keys ({}) must read thicker than one ({})",
            many.columns()[3],
            one.columns()[3]
        );
    }
}
