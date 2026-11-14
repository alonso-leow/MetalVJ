//! Putting two cameras on one clock, by listening to them.
//!
//! Two handheld angles never start together, and a clapperboard is one more
//! thing to forget. But both cameras record the same piano, so the recording
//! itself is the slate: cross-correlate the two audio tracks and the offset
//! falls out to a few milliseconds.
//!
//! The correlation runs on the **energy envelope**, not the waveform. Two mics
//! in different corners of a room hear the same notes with different phase and
//! different colouring, so waveform correlation is weak and can lock onto the
//! wrong cycle. Note onsets, though, happen at the same instant everywhere in
//! the room — and the envelope is essentially an onset trace.
//!
//! The same routine aligns a take to the reference recording the score was
//! transcribed against, which is how frames, notes, and motion end up on a
//! single timeline.

use std::path::Path;
use metal_schema::platform::Command;

/// Audio is decoded to this rate; the envelope is what matters, not fidelity.
const SAMPLE_RATE: u32 = 8_000;
/// Envelope step. 5 ms is well under half a video frame at 30 fps, so the
/// envelope is never the limiting factor in pairing precision.
const HOP: usize = SAMPLE_RATE as usize / 200;
const WINDOW: usize = HOP * 4;
/// Cap on how much audio is correlated, to bound cost on a one-core box.
const MAX_SECONDS: usize = 180;

#[derive(Debug, Clone, Copy)]
pub struct Alignment {
    /// Seconds to add to a time on the first clock to get the second.
    pub offset_seconds: f64,
    /// Peak correlation, `0..=1`.
    pub confidence: f32,
    /// Best rival peak as a fraction of the winner. High means the music is
    /// repetitive enough that the true offset could be a whole phrase away.
    pub ambiguity: f32,
}

impl Alignment {
    /// Whether this is worth trusting without a human looking at it.
    pub fn is_trustworthy(&self) -> bool {
        self.confidence >= 0.5 && self.ambiguity <= 0.8
    }
}

/// Decodes a file's first audio stream to mono f32 samples.
pub fn decode_audio(path: &Path) -> Result<Vec<f32>, SyncError> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-ac",
            "1",
            "-ar",
            &SAMPLE_RATE.to_string(),
            "-f",
            "f32le",
            "-",
        ])
        .output()
        .map_err(|e| SyncError::Ffmpeg(e.to_string()))?;

    if !out.status.success() {
        return Err(SyncError::NoAudio(
            path.display().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    if out.stdout.len() < 4 {
        return Err(SyncError::NoAudio(
            path.display().to_string(),
            "stream is empty".into(),
        ));
    }
    Ok(out
        .stdout
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

/// Short-time loudness, standardised to zero mean and unit variance.
///
/// The log compresses the dynamic range so a fortissimo chord does not drown
/// out a whole quiet phrase, and standardising makes the result independent of
/// how loud each camera happened to record.
fn envelope(samples: &[f32]) -> Vec<f32> {
    let limit = (MAX_SECONDS * SAMPLE_RATE as usize).min(samples.len());
    let samples = &samples[..limit];

    let mut env: Vec<f32> = samples
        .chunks(HOP)
        .enumerate()
        .map(|(i, _)| {
            let start = i * HOP;
            let end = (start + WINDOW).min(samples.len());
            let win = &samples[start..end];
            if win.is_empty() {
                return 0.0;
            }
            let power = win.iter().map(|s| s * s).sum::<f32>() / win.len() as f32;
            (power.sqrt() * 1000.0).ln_1p()
        })
        .collect();

    let n = env.len() as f32;
    if n == 0.0 {
        return env;
    }
    let mean = env.iter().sum::<f32>() / n;
    let var = env.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n;
    let sd = var.sqrt().max(1e-9);
    for v in &mut env {
        *v = (*v - mean) / sd;
    }
    env
}

/// Prefix sums, so the mean and variance of any window are O(1).
struct Prefix {
    sum: Vec<f64>,
    sq: Vec<f64>,
}

impl Prefix {
    fn new(v: &[f32]) -> Self {
        let mut sum = Vec::with_capacity(v.len() + 1);
        let mut sq = Vec::with_capacity(v.len() + 1);
        sum.push(0.0);
        sq.push(0.0);
        for x in v {
            sum.push(sum.last().unwrap() + *x as f64);
            sq.push(sq.last().unwrap() + (*x as f64) * (*x as f64));
        }
        Prefix { sum, sq }
    }

    fn range(&self, lo: usize, hi: usize) -> (f64, f64) {
        (self.sum[hi] - self.sum[lo], self.sq[hi] - self.sq[lo])
    }
}

/// Finds the lag at which `b` best matches `a`.
///
/// Returns the offset such that a moment at time `t` on `a`'s clock is at
/// `t + offset` on `b`'s clock.
pub fn align(a: &[f32], b: &[f32], max_offset_seconds: f64) -> Result<Alignment, SyncError> {
    let (ea, eb) = (envelope(a), envelope(b));
    if ea.len() < 200 || eb.len() < 200 {
        return Err(SyncError::TooShort);
    }
    let hop_seconds = HOP as f64 / SAMPLE_RATE as f64;
    let max_lag = (max_offset_seconds / hop_seconds).round() as isize;

    // Refuse to judge an alignment on a sliver of overlap.
    let min_overlap = (ea.len().min(eb.len()) / 3).max((5.0 / hop_seconds) as usize);

    let (pa, pb) = (Prefix::new(&ea), Prefix::new(&eb));
    let mut scores: Vec<(isize, f32)> = Vec::new();

    for k in -max_lag..=max_lag {
        let lo = 0.max(-k) as usize;
        let hi = (ea.len() as isize).min(eb.len() as isize - k).max(0) as usize;
        if hi <= lo || hi - lo < min_overlap {
            continue;
        }
        let n = (hi - lo) as f64;

        let mut cross = 0.0f64;
        for i in lo..hi {
            cross += ea[i] as f64 * eb[(i as isize + k) as usize] as f64;
        }

        let (sa, sa2) = pa.range(lo, hi);
        let (sb, sb2) = pb.range((lo as isize + k) as usize, (hi as isize + k) as usize);
        let denom = ((n * sa2 - sa * sa) * (n * sb2 - sb * sb)).sqrt();
        if denom <= 1e-9 {
            continue;
        }
        scores.push((k, ((n * cross - sa * sb) / denom) as f32));
    }

    let &(best_lag, best) = scores
        .iter()
        .max_by(|x, y| x.1.total_cmp(&y.1))
        .ok_or(SyncError::TooShort)?;

    // A rival counts only if it is a genuinely different alignment, not the
    // shoulder of the same peak.
    let guard = (0.5 / hop_seconds) as isize;
    let rival = scores
        .iter()
        .filter(|(k, _)| (k - best_lag).abs() > guard)
        .map(|(_, s)| *s)
        .fold(f32::NEG_INFINITY, f32::max);

    Ok(Alignment {
        offset_seconds: best_lag as f64 * hop_seconds,
        confidence: best.clamp(0.0, 1.0),
        ambiguity: if best > 1e-6 && rival.is_finite() {
            (rival / best).clamp(0.0, 1.0)
        } else {
            0.0
        },
    })
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("could not run ffmpeg: {0}")]
    Ffmpeg(String),
    #[error("no usable audio in {0}: {1}\n  pass --offset to set the camera alignment by hand")]
    NoAudio(String, String),
    #[error("not enough audio to align on; pass --offset instead")]
    TooShort,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic piano-ish signal: sparse note onsets with decaying tails.
    fn take(seconds: f32, seed: u32) -> Vec<f32> {
        let n = (seconds * SAMPLE_RATE as f32) as usize;
        let mut out = vec![0.0f32; n];
        let mut rng = seed.wrapping_mul(2654435761).wrapping_add(1);
        let mut next = |m: u32| {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            rng % m
        };
        let mut t = 0usize;
        while t < n {
            let freq = 220.0 * (1 + next(6)) as f32;
            let len = (SAMPLE_RATE as usize / 4) + next(SAMPLE_RATE) as usize;
            for i in 0..len.min(n - t) {
                let phase = i as f32 / SAMPLE_RATE as f32;
                let decay = (-phase * 4.0).exp();
                out[t + i] += (phase * freq * std::f32::consts::TAU).sin() * decay * 0.5;
            }
            t += (SAMPLE_RATE as usize / 8) + next(SAMPLE_RATE / 2) as usize;
        }
        out
    }

    /// A second camera: same room, delayed, quieter, with its own hiss.
    fn second_angle(source: &[f32], delay_seconds: f32, gain: f32, seed: u32) -> Vec<f32> {
        let delay = (delay_seconds * SAMPLE_RATE as f32) as usize;
        let mut rng = seed | 1;
        let mut noise = || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng % 1000) as f32 / 1000.0 - 0.5
        };
        let mut out = vec![0.0f32; delay];
        for s in source {
            out.push(s * gain + noise() * 0.05);
        }
        out
    }

    #[test]
    fn a_known_delay_is_recovered() {
        let a = take(40.0, 7);
        let b = second_angle(&a, 1.25, 0.4, 99);
        let r = align(&a, &b, 10.0).unwrap();
        assert!(
            (r.offset_seconds - 1.25).abs() < 0.02,
            "recovered {}",
            r.offset_seconds
        );
        assert!(r.confidence > 0.8, "confidence {}", r.confidence);
        assert!(r.is_trustworthy());
    }

    #[test]
    fn the_camera_that_started_first_gives_a_negative_offset() {
        let source = take(40.0, 3);
        // Here B is the one holding the extra head, so A lags B.
        let a = second_angle(&source, 0.8, 1.0, 5);
        let b = second_angle(&source, 0.0, 0.6, 11);
        let r = align(&a, &b, 10.0).unwrap();
        assert!(
            (r.offset_seconds + 0.8).abs() < 0.03,
            "recovered {}",
            r.offset_seconds
        );
    }

    #[test]
    fn sub_frame_precision() {
        // 12 ms is a third of a video frame at 30 fps.
        let a = take(40.0, 21);
        let b = second_angle(&a, 0.012, 0.8, 4);
        let r = align(&a, &b, 5.0).unwrap();
        assert!(r.offset_seconds.abs() < 0.02, "recovered {}", r.offset_seconds);
    }

    #[test]
    fn unrelated_takes_are_reported_as_untrustworthy() {
        let a = take(40.0, 1);
        let b = take(40.0, 2);
        let r = align(&a, &b, 10.0).unwrap();
        assert!(
            !r.is_trustworthy(),
            "two different takes must not pass: conf {} amb {}",
            r.confidence,
            r.ambiguity
        );
    }

    #[test]
    fn a_silent_track_is_rejected_rather_than_aligned() {
        let a = take(40.0, 1);
        let silence = vec![0.0f32; 40 * SAMPLE_RATE as usize];
        let r = align(&a, &silence, 5.0);
        assert!(r.is_err() || !r.unwrap().is_trustworthy());
    }

    #[test]
    fn too_short_is_an_error_not_a_guess() {
        let a = take(0.5, 1);
        let b = take(0.5, 2);
        assert!(matches!(align(&a, &b, 5.0), Err(SyncError::TooShort)));
    }
}
