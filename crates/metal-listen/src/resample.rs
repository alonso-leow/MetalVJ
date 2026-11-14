//! Whatever the sound card runs at, into whatever rate a caller needs.
//!
//! The digest wants [`crate::SR`] (22 050 Hz); the voice path wants 16 000 Hz
//! for `webrtc-vad`. [`Resampler::new`] is the digest target and
//! [`Resampler::with_target`] takes either.
//!
//! A capture device hands over 44 100 or 48 000 Hz and the digest is only
//! defined at [`crate::SR`]. Taking every other sample would be the obvious
//! thing and is the wrong one: everything above 11 025 Hz folds back down into
//! the band as alias, and the folded energy lands squarely in the spectral
//! columns as brightness that is not in the music. A cymbal would raise the
//! centroid of the bass.
//!
//! So this is a windowed-sinc resampler, which low-passes and resamples in one
//! operation — the same thing `librosa.load(sr=22050)` does before the corpus
//! ever sees a number.
//!
//! Written here rather than pulled in because the streaming case is the whole
//! requirement: a capture callback delivers whatever block size it feels like,
//! and the filter state has to survive across those blocks without a click at
//! every seam. That is the part a general resampler makes awkward and it is
//! about eighty lines to own outright.

/// Half the kernel width. 32 gives a stopband deep enough that the alias sits
/// below the noise floor of anything a venue will play.
const HALF: usize = 32;
/// The kernel is precomputed at this many fractional offsets rather than
/// evaluated per sample — a sine per tap per sample is real money at 48 kHz,
/// and the phase quantisation this trades it for is inaudible.
const PHASES: usize = 512;

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-9 {
        1.0
    } else {
        let pix = std::f64::consts::PI * x;
        pix.sin() / pix
    }
}

/// Blackman — a gentler taper than Hann, which buys stopband depth at the cost
/// of a wider transition. Depth is what matters here: transition width shows up
/// as a slightly soft top octave, alias shows up as tones that were never
/// played.
fn blackman(t: f64) -> f64 {
    let x = (t + 1.0) * 0.5; // t in [-1,1] -> x in [0,1]
    0.42 - 0.5 * (2.0 * std::f64::consts::PI * x).cos()
        + 0.08 * (4.0 * std::f64::consts::PI * x).cos()
}

pub struct Resampler {
    /// Input rate. The step is exactly `from / to` input samples per output
    /// sample and is kept as that ratio rather than as a float — see below.
    from: u32,
    /// Output rate. [`crate::SR`] for the digest, 16 000 for the voice path.
    to: u32,
    kernel: Vec<f32>,
    buf: Vec<f32>,
    /// Whole-sample position of the next output, from `buf[0]`.
    int_pos: usize,
    /// Fractional part of that position, over `to`.
    ///
    /// # Why this is not an `f64`
    ///
    /// It was, and it was wrong. A float position is advanced by repeated
    /// addition and pulled back down every time the buffer is trimmed, and
    /// float addition is not associative — so *when* the trimming happened
    /// changed the accumulated rounding, and the phase landed in a different
    /// one of the 512 kernel slots. The output then depended on the block size
    /// the sound card happened to deliver, which is not a thing the caller
    /// controls or can even observe.
    ///
    /// `arriving_in_ragged_blocks_gives_the_same_samples_as_arriving_at_once`
    /// caught it at about 1e-4 per sample. Integers cannot drift, cannot
    /// disagree between two call patterns, and are still exact six hours into
    /// a set, which a float accumulator would not be.
    num: u32,
}

impl Resampler {
    /// From `from` Hz to [`crate::SR`].
    pub fn new(from: u32) -> Resampler {
        Self::with_target(from, crate::SR)
    }

    /// From `from` Hz to `to` Hz.
    pub fn with_target(from: u32, to: u32) -> Resampler {
        let ratio = to as f64 / from as f64;
        // Downsampling has to cut at the *output* Nyquist; upsampling only
        // needs the input's, so the cutoff stops moving once ratio passes 1.
        let fc = 0.5 * ratio.min(1.0);

        let taps = 2 * HALF + 1;
        let mut kernel = vec![0.0f32; PHASES * taps];
        for p in 0..PHASES {
            let frac = p as f64 / PHASES as f64;
            for (i, slot) in kernel[p * taps..(p + 1) * taps].iter_mut().enumerate() {
                let t = i as f64 - HALF as f64 - frac;
                let w = blackman(t / (HALF as f64 + 1.0));
                *slot = (2.0 * fc * sinc(2.0 * fc * t) * w) as f32;
            }
        }

        Resampler {
            from,
            to,
            kernel,
            // Primed so the first real sample has history to convolve against.
            buf: vec![0.0; HALF],
            int_pos: HALF,
            num: 0,
        }
    }

    /// Feed input, collect however many output samples that produced.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.buf.extend_from_slice(input);
        let taps = 2 * HALF + 1;
        let sr = self.to;

        while self.int_pos + HALF + 1 < self.buf.len() {
            // floor(frac * PHASES) with no float anywhere in it.
            let phase =
                ((self.num as u64 * PHASES as u64) / sr as u64) as usize;
            let k = &self.kernel[phase * taps..(phase + 1) * taps];

            let mut acc = 0.0f32;
            let start = self.int_pos - HALF;
            for i in 0..taps {
                acc += k[i] * self.buf[start + i];
            }
            out.push(acc);

            // Advance by exactly `from / SR` input samples.
            self.num += self.from;
            self.int_pos += (self.num / sr) as usize;
            self.num %= sr;
        }

        // Drop what is now behind the kernel. Without this the buffer grows for
        // the length of the set.
        let keep_from = self.int_pos.saturating_sub(HALF);
        if keep_from > 0 {
            self.buf.drain(..keep_from);
            self.int_pos -= keep_from;
        }
    }
}

/// Average the channels. The offline path loads mono and every column is
/// defined on that, so a stereo stream has to be folded the same way rather
/// than analysed one side at a time.
pub fn to_mono(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    if channels <= 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    for frame in interleaved.chunks_exact(channels) {
        out.push(frame.iter().sum::<f32>() / channels as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, sr: u32, secs: f64) -> Vec<f32> {
        let n = (sr as f64 * secs) as usize;
        (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * freq * i as f64 / sr as f64).sin() as f32)
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|&v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn the_output_runs_at_the_rate_it_promised() {
        let mut r = Resampler::new(48000);
        let mut out = Vec::new();
        r.process(&sine(1000.0, 48000, 1.0), &mut out);
        let n = out.len() as f32;
        assert!(
            (n - crate::SR as f32).abs() < 100.0,
            "one second of 48 kHz gave {n} samples, wanted about {}",
            crate::SR
        );
    }

    #[test]
    fn a_tone_inside_the_band_survives_at_its_own_level() {
        let mut r = Resampler::new(48000);
        let mut out = Vec::new();
        r.process(&sine(1000.0, 48000, 1.0), &mut out);
        // Ignore the first and last few hundred samples: the primed zeros and
        // the unfinished tail are not steady state.
        let body = &out[500..out.len() - 500];
        let level = rms(body);
        assert!(
            (level - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.05,
            "a full-scale sine came through at {level}"
        );
    }

    #[test]
    fn a_tone_above_the_new_nyquist_is_rejected_rather_than_folded() {
        // 16 kHz at 48 k would alias to 48000/2... specifically it would fold
        // to |16000 - 22050| = 6050 Hz and sound like a note nobody played.
        // This is the entire reason the resampler exists.
        let mut r = Resampler::new(48000);
        let mut out = Vec::new();
        r.process(&sine(16000.0, 48000, 1.0), &mut out);
        let body = &out[500..out.len() - 500];
        let level = rms(body);
        assert!(level < 0.02, "16 kHz should have been filtered out, got {level}");
    }

    #[test]
    fn arriving_in_ragged_blocks_gives_the_same_samples_as_arriving_at_once() {
        let audio = sine(440.0, 48000, 0.5);
        let mut whole = Resampler::new(48000);
        let mut a = Vec::new();
        whole.process(&audio, &mut a);

        let mut chunked = Resampler::new(48000);
        let mut b = Vec::new();
        let mut i = 0;
        for size in [64usize, 1, 4096, 333, 17].iter().cycle() {
            if i >= audio.len() {
                break;
            }
            let end = (i + size).min(audio.len());
            chunked.process(&audio[i..end], &mut b);
            i = end;
        }

        assert_eq!(a.len(), b.len(), "block size changed the sample count");
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            assert!((x - y).abs() < 1e-6, "sample {i} differs: {x} vs {y}");
        }
    }

    #[test]
    fn a_rate_that_needs_no_conversion_is_left_alone() {
        let mut r = Resampler::new(crate::SR);
        let mut out = Vec::new();
        let audio = sine(500.0, crate::SR, 0.2);
        r.process(&audio, &mut out);
        let n = out.len();
        assert!((n as i64 - audio.len() as i64).abs() < 70);
        let level = rms(&out[200..n - 200]);
        assert!((level - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.05);
    }

    #[test]
    fn folding_to_mono_averages_the_sides() {
        let mut out = Vec::new();
        to_mono(&[1.0, 0.0, 0.5, 0.5, -1.0, 1.0], 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5, 0.0]);
    }
}
