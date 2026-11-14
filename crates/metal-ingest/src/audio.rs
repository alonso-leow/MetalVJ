//! Audio for the export mux (plan §9.1 E3).
//!
//! `export.video` was video-only through E1: `encode_rgba`/the stepped
//! [`crate::export::Encoder`] carried an explicit `-an`. E3 gives the export
//! path an optional audio track, and this module is the arithmetic behind it —
//! it sums the sources at the target rate/channels, then a limiter, without
//! dragging in ffmpeg or a codec: what lands in the container is a temp WAV
//! this module writes and [`crate::export`] hands to ffmpeg as a second input.
//!
//! # What is deliberately simple
//!
//! A full BS.1770 loudness pass and a look-ahead true-peak limiter with
//! latency compensation is the right tool for a broadcast bound; here a **per-sample hard limiter** (clamp to ±1) is the honest
//! floor the plan asks for ("a simple limiter/normalisation is enough"), and
//! the mix itself is exact: sources already at the target rate/channels are
//! summed sample for sample, so two quiet tones produce their arithmetic sum.
//! What is *not* here is a resampler with any pretension to quality — a linear
//! interpolation between neighbouring frames, enough to get a 44.1 kHz song
//! onto a 48 kHz track without a pitch shift.

use std::path::Path;

/// Interleaved 32-bit float PCM at a known rate and channel count.
///
/// Interleaved (frame `n` is `samples[n*channels .. (n+1)*channels]`), which is
/// what both a decoder and a WAV writer want without a transpose in between.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioTrack {
    pub rate: u32,
    pub channels: u16,
    /// `channels` samples per frame; `samples.len()` is always a whole number
    /// of frames, which every constructor here keeps true.
    pub samples: Vec<f32>,
}

impl AudioTrack {
    /// An empty track at `rate`/`channels`.
    pub fn new(rate: u32, channels: u16) -> AudioTrack {
        AudioTrack { rate, channels: channels.max(1), samples: Vec::new() }
    }

    /// `frames` frames of silence at `rate`/`channels`.
    pub fn silence(rate: u32, channels: u16, frames: usize) -> AudioTrack {
        AudioTrack { rate, channels: channels.max(1), samples: vec![0.0; frames * channels.max(1) as usize] }
    }

    /// Whole frames carried.
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    /// Seconds of audio carried.
    pub fn duration(&self) -> f64 {
        if self.rate == 0 {
            0.0
        } else {
            self.frames() as f64 / self.rate as f64
        }
    }

    /// Whether there is no audio at all (no frames).
    pub fn is_empty(&self) -> bool {
        self.frames() == 0
    }

    /// The frame index at `seconds`, clamped to the track.
    pub fn frame_at(&self, seconds: f64) -> usize {
        let f = (seconds.max(0.0) * self.rate as f64).round() as i64;
        f.clamp(0, self.frames() as i64) as usize
    }

    /// `[from, to)` seconds, at this track's own rate/channels.
    pub fn slice_seconds(&self, from: f64, to: f64) -> AudioTrack {
        let ch = self.channels.max(1) as usize;
        let a = self.frame_at(from);
        let b = self.frame_at(to.max(from));
        AudioTrack {
            rate: self.rate,
            channels: self.channels,
            samples: self.samples[a * ch..b * ch].to_vec(),
        }
    }

    /// Resampled (linear) and channel-mapped to `rate`/`channels`.
    ///
    /// Exact identity when nothing has to change — the mix's arithmetic
    /// depends on that, so a source already at the target rate/channels is
    /// copied, never round-tripped through the interpolator.
    pub fn to_rate_channels(&self, rate: u32, channels: u16) -> AudioTrack {
        let channels = channels.max(1);
        if rate == self.rate && channels == self.channels {
            return self.clone();
        }
        let src_ch = self.channels.max(1) as usize;
        let dst_ch = channels as usize;
        let src_frames = self.frames();
        if src_frames == 0 || self.rate == 0 || rate == 0 {
            return AudioTrack { rate, channels, samples: Vec::new() };
        }
        let dst_frames = ((src_frames as f64) * rate as f64 / self.rate as f64).round().max(1.0) as usize;
        let mut out = Vec::with_capacity(dst_frames * dst_ch);
        let ratio = self.rate as f64 / rate as f64;
        for i in 0..dst_frames {
            let pos = i as f64 * ratio;
            let i0 = pos.floor() as usize;
            let i1 = (i0 + 1).min(src_frames - 1);
            let t = (pos - i0 as f64) as f32;
            for c in 0..dst_ch {
                let s0 = src_channel(&self.samples, src_ch, i0, c);
                let s1 = src_channel(&self.samples, src_ch, i1, c);
                out.push(s0 + (s1 - s0) * t);
            }
        }
        AudioTrack { rate, channels, samples: out }
    }

    /// Interleaved little-endian signed 16-bit PCM — the bytes a raw WAV body
    /// (and ffmpeg's `s16le`) carry.
    pub fn to_s16le(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.samples.len() * 2);
        for s in &self.samples {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
}

/// Sample of frame `frame` on output channel `c`, mapping channel counts:
/// identity when they match, a mono source duplicated across every output
/// channel, and any other source read channel-for-channel (wrapping to its
/// last) so the mix never silently drops a channel.
fn src_channel(interleaved: &[f32], src_ch: usize, frame: usize, c: usize) -> f32 {
    let sc = if src_ch == 1 { 0 } else { c.min(src_ch - 1) };
    interleaved[frame * src_ch + sc]
}

/// Sums `sources` at `rate`/`channels`, converting each one first, then
/// applies the hard limiter (per-sample clamp to ±1).
///
/// The output runs as long as the longest converted source; short sources are
/// zero-padded by the sum. Empty input is an empty track, not an error — "no
/// audio asked for" is the caller's `None`, one layer up.
pub fn mix(sources: &[AudioTrack], rate: u32, channels: u16) -> AudioTrack {
    let channels = channels.max(1);
    let converted: Vec<AudioTrack> = sources.iter().map(|s| s.to_rate_channels(rate, channels)).collect();
    let frames = converted.iter().map(AudioTrack::frames).max().unwrap_or(0);
    let mut sum = vec![0.0f32; frames * channels as usize];
    for src in &converted {
        for (o, s) in sum.iter_mut().zip(&src.samples) {
            *o += *s;
        }
    }
    // The simple limiter: a hard ceiling, so quiet material is bit-exact and
    // only a summed peak above full scale is touched.
    for s in &mut sum {
        *s = s.clamp(-1.0, 1.0);
    }
    AudioTrack { rate, channels, samples: sum }
}

/// Writes `track` as a 16-bit PCM WAV — the temp file [`crate::export`] hands
/// ffmpeg as a second input. Hand-rolled (a 44-byte header and the raw
/// samples) rather than pulling a WAV writer in for one call.
pub fn write_wav(path: &Path, track: &AudioTrack) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
    }
    let data = track.to_s16le();
    let channels = track.channels.max(1) as u32;
    let rate = track.rate;
    let byte_rate = rate * channels * 2;
    let block_align = (channels * 2) as u16;
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&(channels as u16).to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    metal_vfs::write(path, &wav).map_err(|e| e.to_string())
}

/// Reads a 16-bit PCM WAV written by [`write_wav`] — the no-decoder fallback
/// `metal-perform`'s `audio.file` executor uses when it is built without
/// `metal-listen`'s `decode` feature (a plain WAV needs no symphonia).
///
/// Only what that writer emits: PCM (`fmt ` tag 1), 16 bits, any rate and
/// channel count. Anything else is refused by name rather than mis-read.
pub fn read_wav(path: &Path) -> Result<AudioTrack, String> {
    let bytes = metal_vfs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(format!("{} is not a RIFF/WAVE file", path.display()));
    }
    let mut pos = 12usize;
    let (mut rate, mut channels, mut bits) = (0u32, 0u16, 0u16);
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
        let body = bytes.get(pos + 8..pos + 8 + size).ok_or_else(|| "truncated WAV chunk".to_string())?;
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err("short fmt chunk".into());
                }
                let tag = u16::from_le_bytes([body[0], body[1]]);
                if tag != 1 {
                    return Err(format!("WAV is not 16-bit PCM (format tag {tag})"));
                }
                channels = u16::from_le_bytes([body[2], body[3]]);
                rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                bits = u16::from_le_bytes([body[14], body[15]]);
            }
            b"data" => data = Some(body),
            _ => {}
        }
        // Chunks are word-aligned: an odd size carries one pad byte.
        pos += 8 + size + (size & 1);
    }
    if bits != 16 {
        return Err(format!("WAV is {bits}-bit; only 16-bit PCM is read"));
    }
    if rate == 0 || channels == 0 {
        return Err("WAV has no fmt header".into());
    }
    let data = data.ok_or_else(|| "WAV has no data chunk".to_string())?;
    let samples = data
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / i16::MAX as f32)
        .collect();
    Ok(AudioTrack { rate, channels, samples })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A pure tone: `samples[n] = amp * sin(2π f n / rate)`.
    fn tone(rate: u32, channels: u16, frames: usize, hz: f64, amp: f32) -> AudioTrack {
        let mut samples = Vec::with_capacity(frames * channels as usize);
        for n in 0..frames {
            let v = amp * (2.0 * std::f64::consts::PI * hz * n as f64 / rate as f64).sin() as f32;
            for _ in 0..channels {
                samples.push(v);
            }
        }
        AudioTrack { rate, channels, samples }
    }

    /// The plan's mix test: two tones at the target rate/channels sum sample
    /// for sample, and because neither is near full scale the limiter leaves
    /// the arithmetic untouched.
    #[test]
    fn two_tones_sum_to_their_expected_samples() {
        let (rate, ch) = (48_000u32, 1u16);
        let a = tone(rate, ch, 256, 220.0, 0.2);
        let b = tone(rate, ch, 256, 440.0, 0.3);
        let m = mix(&[a.clone(), b.clone()], rate, ch);
        assert_eq!(m.frames(), 256);
        for i in 0..a.samples.len() {
            let want = a.samples[i] + b.samples[i];
            assert!((m.samples[i] - want).abs() < 1e-6, "sample {i}: {} vs {want}", m.samples[i]);
        }
    }

    /// The limiter is a ceiling, not a compressor: a sum that stays inside
    /// full scale passes through, and one that does not is clamped.
    #[test]
    fn the_limiter_clamps_only_what_overflows() {
        let rate = 48_000;
        let quiet = AudioTrack { rate, channels: 1, samples: vec![0.4, -0.4] };
        let m = mix(&[quiet.clone(), quiet], rate, 1);
        assert_eq!(m.samples, vec![0.8, -0.8]);
        let loud = AudioTrack { rate, channels: 1, samples: vec![0.9] };
        let m = mix(&[loud.clone(), loud], rate, 1);
        assert_eq!(m.samples, vec![1.0]);
    }

    /// A mono source becomes the same value on both channels; the length of
    /// the mix is the longest source's, zero-padded.
    #[test]
    fn a_mono_source_spreads_and_a_short_one_is_zero_padded() {
        let mono = AudioTrack { rate: 48_000, channels: 1, samples: vec![0.5] };
        let stereo = AudioTrack { rate: 48_000, channels: 2, samples: vec![0.1, -0.1, 0.2, -0.2] };
        let m = mix(&[mono, stereo], 48_000, 2);
        assert_eq!(m.frames(), 2);
        assert_eq!(m.samples, vec![0.6, 0.4, 0.2, -0.2]);
    }

    /// Resampling changes the frame count but not the channel count, and an
    /// identity conversion is a copy (byte-exact, which the mix relies on).
    #[test]
    fn conversion_is_identity_at_the_same_rate_and_resamples_otherwise() {
        let t = tone(44_100, 2, 441, 1000.0, 0.5);
        assert_eq!(t.to_rate_channels(44_100, 2), t);
        let up = t.to_rate_channels(48_000, 2);
        assert_eq!(up.rate, 48_000);
        assert_eq!(up.channels, 2);
        assert_eq!(up.frames(), 480);
    }

    /// The 16-bit conversion clamps and round-trips a known sample.
    #[test]
    fn a_track_writes_and_reads_back_through_a_wav() {
        let dir = std::env::temp_dir().join(format!("fw-ingest-audio-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let path: PathBuf = dir.join("t.wav");
        let t = AudioTrack { rate: 44_100, channels: 2, samples: vec![0.0, 0.5, -0.5, 1.0, -1.0, 0.25] };
        write_wav(&path, &t).unwrap();
        let back = read_wav(&path).unwrap();
        assert_eq!(back.rate, 44_100);
        assert_eq!(back.channels, 2);
        assert_eq!(back.frames(), 3);
        for (a, b) in t.samples.iter().zip(&back.samples) {
            assert!((a - b).abs() < 2e-4, "{a} vs {b}");
        }
        metal_vfs::remove_dir_all(&dir).unwrap();
    }

    /// `slice_seconds` picks the frames covering `[from, to)`.
    #[test]
    fn a_slice_covers_its_seconds() {
        let t = tone(1000, 1, 1000, 1.0, 0.1);
        let s = t.slice_seconds(0.1, 0.3);
        assert_eq!(s.frames(), 200);
        assert_eq!(s.rate, 1000);
    }
}
