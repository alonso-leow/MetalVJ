//! YIN — the pitch and the voicing, cheap enough for the room.
//!
//! `the voice design notes` decision 12 fixes the shape: the FFT difference function,
//! a 40 ms window, τ for 55–1100 Hz, the absolute threshold 0.15 and parabolic
//! interpolation. It returns **`(f0, periodicity)`**, not just presence:
//! [`crate::voice`] needs voicing to tell a sung vowel from a whisper, and
//! pitch stability to tell singing from talking.
//!
//! # Where the window sits
//!
//! [`Yin::analyze`] is handed the *most recent* [`W`] samples and looks only
//! backwards. That is what makes the analyser causal: a frame at time `t`
//! never reads a sample after `t`. The cost is a 40 ms window fill, which the
//! plan already accepts (live has no look-ahead; ~30 ms of latency).

use std::sync::Arc;

use rustfft::{num_complex::Complex, Fft, FftPlanner};

/// The voice path's own rate. `webrtc-vad` only accepts 8/16/32/48 kHz and the
/// digest runs at 22 050, so the voice path resamples to this.
pub const VOICE_SR: u32 = 16_000;
/// Difference-function window: 40 ms.
pub const W: usize = 640;
/// τ floor: 1100 Hz.
pub const TAU_MIN: usize = VOICE_SR as usize / 1100;
/// τ ceiling: 55 Hz.
pub const TAU_MAX: usize = VOICE_SR as usize / 55;
/// YIN's absolute threshold.
pub const THRESHOLD: f32 = 0.15;
/// The F0 range the rig cares about, in Hz.
pub const F0_MIN: f32 = 55.0;
pub const F0_MAX: f32 = 1100.0;

/// The FFT-difference pitch detector. One instance per analyser.
pub struct Yin {
    n: usize,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    buf: Vec<Complex<f32>>,
    /// τ of the chosen dip and the difference function, reused across calls.
    d: Vec<f32>,
    cmndf: Vec<f32>,
    suffix: Vec<f32>,
}

impl Yin {
    pub fn new() -> Yin {
        let want = W + TAU_MAX + 1;
        let mut n = 1;
        while n < want {
            n <<= 1;
        }
        let mut planner = FftPlanner::new();
        Yin {
            n,
            fft: planner.plan_fft_forward(n),
            ifft: planner.plan_fft_inverse(n),
            buf: vec![Complex::new(0.0, 0.0); n],
            d: vec![0.0; TAU_MAX + 1],
            cmndf: vec![1.0; TAU_MAX + 1],
            suffix: vec![0.0; W + 1],
        }
    }

    /// `(f0, periodicity)` for `last`, the most recent [`W`] samples (whose last
    /// element is the newest). `(0, 0)` for silence or an unvoiced window.
    ///
    /// `periodicity = 1 − d′(τ*)`, in `0..1`; the caller decides the voiced
    /// threshold. `f0` is `VOICE_SR / τ*` after interpolation, and is clamped
    /// to `0` outside [`F0_MIN`]..[`F0_MAX`] so the caller never has to guard.
    pub fn analyze(&mut self, last: &[f32]) -> (f32, f32) {
        let w = W.min(last.len());
        if w < TAU_MAX * 2 {
            return (0.0, 0.0);
        }
        let x = &last[last.len() - w..];

        for c in self.buf.iter_mut() {
            *c = Complex::new(0.0, 0.0);
        }
        for (i, &v) in x.iter().enumerate() {
            self.buf[i].re = v;
        }
        self.fft.process(&mut self.buf);
        for c in self.buf.iter_mut() {
            *c = Complex::new(c.norm_sqr(), 0.0);
        }
        self.ifft.process(&mut self.buf);

        // suffix[j] = Σ_{j}^{w-1} x², so d(τ) = r(0) + suffix[τ] − 2 r(τ).
        self.suffix[w] = 0.0;
        for j in (0..w).rev() {
            self.suffix[j] = self.suffix[j + 1] + x[j] * x[j];
        }
        let scale = 1.0 / self.n as f32;
        let r0 = self.buf[0].re * scale;
        if r0 < 1e-9 {
            return (0.0, 0.0);
        }

        let tau_hi = TAU_MAX.min(w - 1);
        for tau in 1..=tau_hi {
            let r_tau = self.buf[tau].re * scale;
            self.d[tau] = (r0 + self.suffix[tau] - 2.0 * r_tau).max(0.0);
        }
        // tau_hi may be < TAU_MAX only for very short windows; zero the tail.
        for tau in tau_hi + 1..=TAU_MAX {
            self.d[tau] = 0.0;
        }

        let mut running = 0.0f32;
        for tau in 1..=TAU_MAX {
            running += self.d[tau];
            self.cmndf[tau] = if running > 1e-12 {
                self.d[tau] * tau as f32 / running
            } else {
                1.0
            };
        }

        let lo = TAU_MIN.max(1);
        let tau = (lo..=TAU_MAX)
            .find(|&t| self.cmndf[t] < THRESHOLD)
            .unwrap_or_else(|| {
                (lo..=TAU_MAX)
                    .min_by(|&a, &b| self.cmndf[a].total_cmp(&self.cmndf[b]))
                    .unwrap_or(lo)
            });

        let mut tau_f = tau as f32;
        if tau > lo && tau < TAU_MAX {
            let (a, b, c) = (self.cmndf[tau - 1], self.cmndf[tau], self.cmndf[tau + 1]);
            let denom = a - 2.0 * b + c;
            if denom.abs() > 1e-9 {
                tau_f += 0.5 * (a - c) / denom;
            }
        }
        let periodicity = (1.0 - self.cmndf[tau]).clamp(0.0, 1.0);
        let f0 = VOICE_SR as f32 / tau_f;
        if (F0_MIN..=F0_MAX).contains(&f0) {
            (f0, periodicity)
        } else {
            (0.0, periodicity)
        }
    }
}

impl Default for Yin {
    fn default() -> Self {
        Yin::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, secs: f32) -> Vec<f32> {
        let n = (VOICE_SR as f32 * secs) as usize;
        (0..n)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / VOICE_SR as f32).sin() * 0.5)
            .collect()
    }

    fn cents(a: f32, b: f32) -> f32 {
        1200.0 * (a / b).log2()
    }

    #[test]
    fn yin_finds_110_220_440_within_a_quarter_tone() {
        let mut y = Yin::new();
        for f in [110.0f32, 220.0, 440.0] {
            let audio = sine(f, 0.5);
            let (got, periodicity) = y.analyze(&audio);
            let err = cents(got, f).abs();
            assert!(
                err < 50.0,
                "YIN read {got:.1} Hz for {f} Hz ({err:.1} cents, a quarter tone is 50)"
            );
            assert!(periodicity > 0.8, "a pure tone should be strongly periodic, got {periodicity}");
        }
    }

    #[test]
    fn silence_gives_no_pitch_and_no_voicing() {
        let mut y = Yin::new();
        let (f0, periodicity) = y.analyze(&vec![0.0; W]);
        assert_eq!(f0, 0.0);
        assert_eq!(periodicity, 0.0);
    }
}
