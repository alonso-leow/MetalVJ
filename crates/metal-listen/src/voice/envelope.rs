//! The speech-band envelope, the syllable rate, and the transient detectors.
//!
//! Three cheap things the rig needs and the pitch path cannot give it:
//!
//! - **speech-band share** (300–3400 Hz) and **hiss** (4–8 kHz), off a small
//!   FFT. Presence is gated on the first; sibilance and whispering read the
//!   second.
//! - **syllable rate**: the 4–8 Hz modulation of the speech-band envelope,
//!   mapped `0..12 Hz → 0..1`. Rap and fast speech sit high, singing low.
//! - **plosive** and **hiss** detectors: a short level dip and then a
//!   broadband burst is a `p/b/m` release; sustained high hiss with low
//!   voicing is a fricative.

use std::sync::Arc;

use rustfft::{num_complex::Complex, Fft, FftPlanner};

use super::yin::VOICE_SR;

/// Small analysis FFT: 16 ms at 16 kHz.
pub const BAND_FFT: usize = 256;
/// A frame is "hiss" above this 4–8 kHz share.
pub const HISS_GATE: f32 = 0.05;

/// One frame's band measurements.
#[derive(Clone, Copy, Debug, Default)]
pub struct Bands {
    /// 300–3400 Hz share of total power, 0..1.
    pub speech_share: f32,
    /// 4–8 kHz share of total power, 0..1.
    pub hiss_share: f32,
    /// Unnormalised speech-band power, for a loudness envelope.
    pub speech_power: f32,
}

/// Speech-band and hiss energy shares, recomputed per frame.
pub struct BandEnergy {
    n: usize,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    buf: Vec<Complex<f32>>,
    speech: Vec<usize>,
    hiss: Vec<usize>,
    total: Vec<usize>,
}

impl BandEnergy {
    pub fn new() -> BandEnergy {
        let n = BAND_FFT;
        let mut planner = FftPlanner::new();
        let window = (0..n)
            .map(|i| {
                let t = i as f32 / (n - 1) as f32;
                0.5 - 0.5 * (std::f32::consts::TAU * t).cos()
            })
            .collect();
        let (mut speech, mut hiss, mut total) = (Vec::new(), Vec::new(), Vec::new());
        for k in 1..n / 2 {
            let hz = k as f32 * VOICE_SR as f32 / n as f32;
            if (300.0..=3400.0).contains(&hz) {
                speech.push(k);
            }
            if (4000.0..=7900.0).contains(&hz) {
                hiss.push(k);
            }
            total.push(k);
        }
        BandEnergy {
            n,
            fft: planner.plan_fft_forward(n),
            window,
            buf: vec![Complex::new(0.0, 0.0); n],
            speech,
            hiss,
            total,
        }
    }

    /// Band measurements for the most recent [`BAND_FFT`] samples.
    pub fn analyze(&mut self, last: &[f32]) -> Bands {
        let n = self.n.min(last.len());
        if n == 0 {
            return Bands::default();
        }
        let x = &last[last.len() - n..];
        for (i, c) in self.buf.iter_mut().enumerate() {
            *c = if i < n {
                Complex::new(x[i] * self.window[i], 0.0)
            } else {
                Complex::new(0.0, 0.0)
            };
        }
        self.fft.process(&mut self.buf);
        // 300–3400 Hz is lower than BAND_FFT/2's 8 kHz, so every index exists.
        let mut total = 0.0f32;
        let mut speech = 0.0f32;
        let mut hiss = 0.0f32;
        for &k in &self.total {
            total += self.buf[k].norm_sqr();
        }
        if total < 1e-12 {
            return Bands::default();
        }
        for &k in &self.speech {
            speech += self.buf[k].norm_sqr();
        }
        for &k in &self.hiss {
            hiss += self.buf[k].norm_sqr();
        }
        Bands {
            speech_share: speech / total,
            hiss_share: hiss / total,
            speech_power: speech / self.n as f32,
        }
    }
}

impl Default for BandEnergy {
    fn default() -> Self {
        BandEnergy::new()
    }
}

/// Syllable rate in Hz from the speech-band envelope `env` (one value per
/// 10 ms frame). Counts envelope peaks above the mean; clamps to `0..12`.
pub fn syllable_rate(env: &[f32]) -> f32 {
    if env.len() < 5 {
        return 0.0;
    }
    let mean = env.iter().sum::<f32>() / env.len() as f32;
    if mean <= 1e-9 {
        return 0.0;
    }
    let threshold = mean * 1.1;
    let mut peaks = 0usize;
    let mut above = false;
    for &v in env {
        if v > threshold && !above {
            peaks += 1;
            above = true;
        } else if v < mean * 0.9 {
            above = false;
        }
    }
    let secs = env.len() as f32 * 0.01;
    (peaks as f32 / secs).clamp(0.0, 12.0)
}

/// `0..1` plosive score: a recent level dip followed by a broadband burst.
/// `rms` and `hiss` are the per-frame histories, oldest first, same length.
pub fn plosive(rms: &[f32], hiss: &[f32]) -> f32 {
    if rms.len() < 4 {
        return 0.0;
    }
    let now = *rms.last().unwrap();
    let peak = rms.iter().cloned().fold(f32::MIN, f32::max).max(1e-6);
    let recent_min = rms[rms.len().saturating_sub(4)..rms.len() - 1]
        .iter()
        .cloned()
        .fold(f32::MAX, f32::min);
    let dip = (1.0 - recent_min / peak).clamp(0.0, 1.0);
    let burst = (now / peak).clamp(0.0, 1.0);
    let hissy = hiss.last().copied().unwrap_or(0.0);
    (dip * burst * (0.5 + hissy)).clamp(0.0, 1.0)
}

/// `0..1` hiss/fricative score: high 4–8 kHz share with low voicing.
pub fn hiss_score(hiss_share: f32, periodicity: f32) -> f32 {
    let unvoiced = (1.0 - periodicity).clamp(0.0, 1.0);
    (hiss_share * 2.0).clamp(0.0, 1.0) * unvoiced
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syllable_rate_is_high_for_fast_bursts_and_low_for_a_flat_envelope() {
        // 6 bursts a second for 2 s.
        let env: Vec<f32> = (0..200)
            .map(|i| {
                let t = i as f32 * 0.01;
                if (t * 6.0).fract() < 0.3 {
                    1.0
                } else {
                    0.05
                }
            })
            .collect();
        let fast = syllable_rate(&env);
        assert!((4.0..=8.0).contains(&fast), "6 Hz bursts read as {fast} Hz");
        let flat = syllable_rate(&vec![1.0; 200]);
        assert!(flat < 1.0, "a flat envelope is not syllables, got {flat}");
    }
}
