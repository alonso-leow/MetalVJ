//! The two filterbanks, ported from librosa.
//!
//! These are fixed matrices — they depend only on the sample rate, the FFT
//! size and the bin count, all of which are constants here — so they are built
//! once at startup and never touched again. Building them costs about a
//! millisecond and the alternative was checking half a megabyte of float
//! literals into the repository, where nobody could ever have reviewed them.
//!
//! **They are ported, so they are tested against the original.**
//! `tests/reference.rs` compares both matrices element-by-element against
//! `librosa.filters.mel` and `librosa.filters.chroma` dumped to `.npy`. That
//! test is the only reason to trust a hand-translation of code this fiddly:
//! every constant below (`200/3`, `log(6.4)/27`, the `-3` roll, the `1.5`
//! prepended chroma bin) is a place where a plausible-looking mistake produces
//! a plausible-looking spectrum that is quietly wrong.

/// Frequency, in Hz, of each FFT bin — `k * sr / n_fft` for the real half.
pub fn fft_frequencies(sr: u32, n_fft: usize) -> Vec<f32> {
    let bins = n_fft / 2 + 1;
    (0..bins).map(|k| k as f32 * sr as f32 / n_fft as f32).collect()
}

// --- the Slaney mel scale --------------------------------------------------
//
// librosa's default is `htk=False`, which is *not* the textbook HTK formula.
// It is linear below 1 kHz and logarithmic above, and the seam between the two
// is what the magic numbers encode. Getting this wrong shifts every mel band
// and the error is invisible until you diff against the original.

const F_SP: f64 = 200.0 / 3.0;
const MIN_LOG_HZ: f64 = 1000.0;
const MIN_LOG_MEL: f64 = MIN_LOG_HZ / F_SP; // 15.0

fn logstep() -> f64 {
    (6.4f64).ln() / 27.0
}

fn hz_to_mel(f: f64) -> f64 {
    if f >= MIN_LOG_HZ {
        MIN_LOG_MEL + (f / MIN_LOG_HZ).ln() / logstep()
    } else {
        f / F_SP
    }
}

fn mel_to_hz(mel: f64) -> f64 {
    if mel >= MIN_LOG_MEL {
        MIN_LOG_HZ * ((mel - MIN_LOG_MEL) * logstep()).exp()
    } else {
        F_SP * mel
    }
}

/// A dense `(rows, bins)` matrix applied to one spectrum per frame.
///
/// Dense rather than sparse on purpose. The mel bank is 128×1025, so a frame
/// costs 131 k multiply-adds and there are 43 frames a second — six MFLOP/s,
/// which is nothing, and the sparse version would have been the kind of
/// optimisation that buys nothing and costs a bug.
pub struct Filterbank {
    pub rows: usize,
    pub bins: usize,
    weights: Vec<f32>,
}

impl Filterbank {
    pub fn get(&self, row: usize, bin: usize) -> f32 {
        self.weights[row * self.bins + bin]
    }

    /// `out[r] = sum_k weights[r][k] * spec[k]`.
    pub fn apply(&self, spec: &[f32], out: &mut [f32]) {
        debug_assert_eq!(spec.len(), self.bins);
        debug_assert_eq!(out.len(), self.rows);
        for r in 0..self.rows {
            let w = &self.weights[r * self.bins..(r + 1) * self.bins];
            let mut acc = 0.0f32;
            for k in 0..self.bins {
                acc += w[k] * spec[k];
            }
            out[r] = acc;
        }
    }
}

/// `librosa.filters.mel(sr, n_fft, n_mels, fmin=0, fmax=sr/2, htk=False, norm="slaney")`
pub fn mel(sr: u32, n_fft: usize, n_mels: usize) -> Filterbank {
    let bins = n_fft / 2 + 1;
    let fftfreqs = fft_frequencies(sr, n_fft);

    let fmin = 0.0f64;
    let fmax = sr as f64 / 2.0;
    let (min_mel, max_mel) = (hz_to_mel(fmin), hz_to_mel(fmax));

    // n_mels + 2 edges: every band is a triangle over three consecutive ones.
    let n_pts = n_mels + 2;
    let mel_f: Vec<f64> = (0..n_pts)
        .map(|i| {
            let m = min_mel + (max_mel - min_mel) * i as f64 / (n_pts - 1) as f64;
            mel_to_hz(m)
        })
        .collect();

    let fdiff: Vec<f64> = mel_f.windows(2).map(|w| w[1] - w[0]).collect();

    let mut weights = vec![0.0f32; n_mels * bins];
    for i in 0..n_mels {
        for (k, &f) in fftfreqs.iter().enumerate() {
            let f = f as f64;
            // Rising edge of triangle i, falling edge of the one before it.
            let lower = (f - mel_f[i]) / fdiff[i];
            let upper = (mel_f[i + 2] - f) / fdiff[i + 1];
            let v = lower.min(upper).max(0.0);
            weights[i * bins + k] = v as f32;
        }
        // norm="slaney": each band integrates to the same area, so a wide
        // high band does not simply because it is wide.
        let enorm = 2.0 / (mel_f[i + 2] - mel_f[i]);
        for k in 0..bins {
            weights[i * bins + k] = (weights[i * bins + k] as f64 * enorm) as f32;
        }
    }

    Filterbank { rows: n_mels, bins, weights }
}

fn hz_to_octs(f: f64, tuning: f64, bins_per_octave: f64) -> f64 {
    let a440 = 440.0 * 2f64.powf(tuning / bins_per_octave);
    (f / (a440 / 16.0)).log2()
}

/// `librosa.filters.chroma(sr, n_fft, n_chroma=12, tuning=0.0, ctroct=5.0,
/// octwidth=2, norm=2, base_c=True)`
///
/// **`tuning` is fixed at 0.0 here and librosa's default is not.**
/// `chroma_stft(tuning=None)` estimates the tuning off the whole spectrogram
/// before it builds this bank, which is a two-pass operation and therefore not
/// something a live stream can do. A440 is the assumption; `lib.rs` records
/// what it costs and the reference test measures it.
pub fn chroma(sr: u32, n_fft: usize, n_chroma: usize) -> Filterbank {
    let bins = n_fft / 2 + 1;
    let fftfreqs = fft_frequencies(sr, n_fft);
    let nc = n_chroma as f64;

    // librosa drops the DC bin, computes over the rest, then prepends one
    // synthetic bin to stand in for it. The 1.5 octaves below the first real
    // bin is arbitrary and is simply what the original does.
    let mut frqbins: Vec<f64> = Vec::with_capacity(bins);
    frqbins.push(nc * hz_to_octs(fftfreqs[1] as f64, 0.0, nc) - 1.5 * nc);
    for &f in &fftfreqs[1..] {
        frqbins.push(nc * hz_to_octs(f as f64, 0.0, nc));
    }

    // Width of each bin in chroma units, floored at 1 so the narrow low bins
    // do not produce impossibly sharp Gaussians.
    let mut binwidth: Vec<f64> = frqbins.windows(2).map(|w| (w[1] - w[0]).max(1.0)).collect();
    binwidth.push(1.0);

    let n_chroma2 = (nc / 2.0).round();
    let mut weights = vec![0.0f32; n_chroma * bins];
    for c in 0..n_chroma {
        for k in 0..bins {
            // Distance from bin k to pitch class c, wrapped to ±6 semitones.
            let d = frqbins[k] - c as f64;
            let d = (d + n_chroma2 + 10.0 * nc).rem_euclid(nc) - n_chroma2;
            weights[c * bins + k] = (-0.5 * (2.0 * d / binwidth[k]).powi(2)).exp() as f32;
        }
    }

    // norm=2 down the chroma axis: each FFT bin's twelve weights are a unit
    // vector, so no bin gets a louder vote than another.
    for k in 0..bins {
        let mut n = 0.0f64;
        for c in 0..n_chroma {
            n += (weights[c * bins + k] as f64).powi(2);
        }
        let n = n.sqrt();
        // librosa leaves a column alone rather than dividing by ~0.
        if n > f32::EPSILON as f64 {
            for c in 0..n_chroma {
                weights[c * bins + k] = (weights[c * bins + k] as f64 / n) as f32;
            }
        }
    }

    // Gaussian weighting toward the middle octaves — chroma from a sub-bass
    // rumble or a cymbal is mostly noise, and this is what says so.
    for k in 0..bins {
        let w = (-0.5 * ((frqbins[k] / nc - 5.0) / 2.0).powi(2)).exp();
        for c in 0..n_chroma {
            weights[c * bins + k] = (weights[c * bins + k] as f64 * w) as f32;
        }
    }

    // base_c: rotate so row 0 is C rather than A.
    let shift = 3 * (n_chroma / 12);
    let mut rolled = vec![0.0f32; n_chroma * bins];
    for c in 0..n_chroma {
        let src = (c + shift) % n_chroma;
        rolled[c * bins..(c + 1) * bins]
            .copy_from_slice(&weights[src * bins..(src + 1) * bins]);
    }

    Filterbank { rows: n_chroma, bins, weights: rolled }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mel_scale_is_linear_below_a_kilohertz_and_log_above() {
        // The seam is the whole point of the Slaney scale; check both sides of
        // it and that they meet.
        assert!((hz_to_mel(500.0) - 500.0 / F_SP).abs() < 1e-9);
        assert!((hz_to_mel(1000.0) - MIN_LOG_MEL).abs() < 1e-9);
        assert!(hz_to_mel(2000.0) > MIN_LOG_MEL);
        for hz in [0.0, 100.0, 999.0, 1000.0, 4000.0, 11025.0] {
            assert!((mel_to_hz(hz_to_mel(hz)) - hz).abs() < 1e-6, "roundtrip at {hz}");
        }
    }

    #[test]
    fn every_mel_band_is_a_nonnegative_triangle_that_holds_energy() {
        let fb = mel(22050, 2048, 128);
        assert_eq!((fb.rows, fb.bins), (128, 1025));
        for r in 0..fb.rows {
            let mut sum = 0.0;
            for k in 0..fb.bins {
                let v = fb.get(r, k);
                assert!(v >= 0.0, "band {r} bin {k} is negative");
                sum += v;
            }
            assert!(sum > 0.0, "band {r} is empty — it would report silence forever");
        }
    }

    #[test]
    fn chroma_has_twelve_rows_and_each_bin_votes_somewhere() {
        let fb = chroma(22050, 2048, 12);
        assert_eq!((fb.rows, fb.bins), (12, 1025));
        // The octave weighting kills the extremes, so only check the middle
        // of the spectrum where a pitch class is a meaningful thing to claim.
        for k in 20..500 {
            let total: f32 = (0..12).map(|c| fb.get(c, k)).sum();
            assert!(total > 0.0, "bin {k} votes for no pitch class at all");
        }
    }
}
