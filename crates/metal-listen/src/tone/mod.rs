//! The tone block — Basic Pitch's harmony summary, appended to the digest row.
//!
//! Six columns (plan D2), after the voice block, in `metal_schema::TONE_COLUMNS`
//! order. The block is a *mood*: where the harmony sits (a hue stored as a
//! 2D vector, D3), how high, how thick, how major, and how sure. It never
//! carries notes — see `the tone design notes` §1.
//!
//! # The pieces
//!
//! - [`reducer`] — §4's pure math. Always compiled, model-free, unit-tested on
//!   synthetic posteriors. It is decision D5: the one reducer used live and
//!   offline, so the same song gives the same columns either way.
//! - [`basic_pitch`] — window framing and calling `metal-ml` (step 6).
//! - [`live`] — the ring buffer and inference thread (step 8 / D7).
//! - [`Tone`] — what [`crate::Digester`] owns: a reducer and, when a model is
//!   actually present, a live runner.
//!
//! # No model is a supported state
//!
//! `data/models/basic-pitch/model.onnx` is fetched, not committed (D9). When it
//! is absent — a fresh checkout, a minimal build — [`Tone`] runs in "no model"
//! mode: all-zero columns and confidence 0, so the colour mapping falls back to
//! chroma exactly as it did before the block existed (D8). Nothing here
//! fabricates a result it did not measure.

pub mod reducer;

#[cfg(feature = "tone")]
pub mod basic_pitch;

#[cfg(feature = "tone")]
pub mod live;

pub use reducer::Reducer;

/// The six column names, in order — declared once in [`metal_schema`]. Indices
/// 0..6 of a tone block, and `MUSIC + VOICE..` of a digest row.
pub const COLUMNS: [&str; metal_schema::TONE] = metal_schema::TONE_COLUMNS;

/// The tone path the digester owns: the shared [`Reducer`], an optional
/// [`live::Runner`] (present only when a model actually loaded), and the last
/// six columns to hand back.
///
/// # Why the reducer is here *and* in the runner
///
/// The runner's worker thread owns its own reducer so inference never touches
/// the digester's thread (D7). The reducer on this struct is the one the
/// offline path ([`Tone::reduce_offline`]) drives directly, on the calling
/// thread. Both are the same [`Reducer`] type and the same math, which is D5's
/// "one reducer" made concrete: the two paths differ only in who calls
/// `push_frame`.
#[cfg(feature = "tone")]
pub struct Tone {
    reducer: Reducer,
    runner: Option<live::Runner>,
    /// All-zero columns held for the no-model path, so [`Tone::columns`] never
    /// has to allocate or special-case.
    cols: [f32; metal_schema::TONE],
    /// Where the model was expected to live, kept so [`Tone::reduce_offline`]
    /// can open its own session without sharing the worker's.
    model_path: Option<std::path::PathBuf>,
}

#[cfg(feature = "tone")]
impl Tone {
    /// Build the tone path.
    ///
    /// `model_path` is `data/models/basic-pitch/model.onnx`. When the file is
    /// missing or fails to load, [`live::Runner::start`] returns `None` and
    /// Tone runs in "no model" mode: all-zero columns, conf 0, so colour falls
    /// back to chroma (D8).
    pub fn new(model_path: Option<&std::path::Path>) -> Tone {
        let runner = model_path.and_then(live::Runner::start);
        Tone {
            reducer: Reducer::new(),
            runner,
            cols: [0.0; metal_schema::TONE],
            model_path: model_path.map(|p| p.to_path_buf()),
        }
    }

    /// Live: push fresh 22 050 Hz mono samples. The runner appends them to its
    /// ring buffer and wakes; inference happens on its own thread (D7) and
    /// latches a result the digester reads per row. A no-model Tone drops them.
    pub fn push(&mut self, samples: &[f32]) {
        if let Some(r) = &self.runner {
            r.push(samples);
        }
    }

    /// The latest six columns. All zero in no-model mode.
    pub fn columns(&self) -> [f32; metal_schema::TONE] {
        self.runner
            .as_ref()
            .and_then(|r| r.latest().map(|(cols, _)| cols))
            .unwrap_or(self.cols)
    }

    /// True only when a model actually loaded.
    pub fn loaded(&self) -> bool {
        self.runner.is_some()
    }

    /// Last inference wall time in ms, 0.0 when never run / no model.
    pub fn latency_ms(&self) -> f32 {
        self.runner.as_ref().map(|r| r.latency_ms()).unwrap_or(0.0)
    }

    /// Offline/file path: feed a whole decoded mono 22 050 Hz buffer through
    /// the same reducer the live path uses, one model window per hop, returning
    /// the six columns for every 32 Hz digest row.
    ///
    /// The live runner and this method deliberately do not share a session: the
    /// runner's model lives on its worker thread and this runs on the caller's.
    /// With no model loaded the result is the honest all-zero block for every
    /// row — the same columns a model-less live stream would emit.
    ///
    /// Step 7 wires this into `file.rs`; this batch only freezes the seam.
    pub fn reduce_offline(&mut self, samples: &[f32]) -> Vec<[f32; metal_schema::TONE]> {
        use metal_schema::TONE;

        // One row per 1/32 s, the canonical digest grid.
        let rows = (samples.len() as u64 * 32 / crate::SR as u64) as usize;
        let mut out = vec![[0.0f32; TONE]; rows];
        if !self.loaded() {
            return out;
        }
        let Some(path) = self.model_path.as_ref() else {
            return out;
        };
        // A fresh session for this pass; if it will not open, stay all-zero
        // rather than inventing output.
        let model = match metal_ml::Model::load_light(path) {
            Ok(m) => m,
            Err(_) => return out,
        };
        self.reducer.reset();
        let hop = (crate::SR / 32) as usize;
        let per_hop = (hop / basic_pitch::FFT_HOP).max(1);
        for (i, row) in out.iter_mut().enumerate() {
            let end = (i + 1) * hop;
            if end > samples.len() {
                break;
            }
            let start = end.saturating_sub(basic_pitch::AUDIO_N_SAMPLES);
            if let Ok(frames) = basic_pitch::run_window(&model, &samples[start..end]) {
                let newest = basic_pitch::newest(&frames, per_hop);
                for j in 0..newest.note.len() {
                    self.reducer.push_frame(
                        &newest.note[j],
                        &newest.onset[j],
                        1.0 / basic_pitch::FPS,
                    );
                }
                *row = self.reducer.columns();
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::reducer::KEYS;
    use super::Reducer;

    /// Basic Pitch's frame interval: one FFT hop (256) at 22 050 Hz. Written
    /// out rather than read from `basic_pitch::FFT_HOP` because that module is
    /// `tone`-gated while this test must also run in a minimal build — but the
    /// number is the same one, and it is the whole point of D5.
    const DT: f32 = 256.0 / crate::SR as f32;

    /// A fixed-seed linear congruential generator. No `rand` dependency, and
    /// reproducible across runs and machines: the parity theorem is structural,
    /// so the only thing this noise is for is to make the frames not all-zero.
    struct Lcg(u64);

    impl Lcg {
        fn next_unit(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            // Top 24 bits → 0.0..1.0 without touching the sign.
            ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
        }
    }

    /// 200 deterministic frames of a rising C-major arpeggio with mild noise.
    /// This is the synthetic posteriorgram §6 step 9 asks for: a pitched signal
    /// whose reducer state actually moves, so a batching difference would have
    /// something to show up in.
    fn synthetic_frames() -> Vec<([f32; KEYS], [f32; KEYS])> {
        // C major triad: MIDI 60/64/67 → keys 39/43/46 (key k is MIDI k+21).
        let triad = [39usize, 43, 46];
        let mut lcg = Lcg(0x1234_5678_9abc_def0);
        let mut out = Vec::with_capacity(200);
        for f in 0..200usize {
            let mut note = [0.0f32; KEYS];
            let mut onset = [0.0f32; KEYS];
            // One note of the arpeggio every four frames; the other two sustain
            // softly, so the reducer accumulates a moving chord rather than a
            // single class.
            let k = triad[(f / 4) % triad.len()];
            for &o in &triad {
                note[o] = if o == k { 0.8 } else { 0.3 };
            }
            if f % 4 == 0 {
                onset[k] = 1.0;
            }
            for v in note.iter_mut() {
                *v = (*v + 0.02 * lcg.next_unit()).clamp(0.0, 1.0);
            }
            out.push((note, onset));
        }
        out
    }

    /// Step 9's structural parity guarantee (D5).
    ///
    /// The same frames reduce to the same columns whether they arrive one at a
    /// time (the offline path) or in batches (the live worker, which pushes a
    /// whole model hop's frames consecutively). Because both paths share one
    /// `Reducer` and advance it with the same per-frame `dt`, the *model's* hop
    /// size — how many frames happen to be new since the last inference — cannot
    /// change the numbers. If that ever stopped being true, colour would drift
    /// between a song played live and the same song digested from disk.
    #[test]
    fn batching_frames_does_not_change_the_reduction() {
        let frames = synthetic_frames();

        let mut one_at_a_time = Reducer::new();
        let mut batched = Reducer::new();
        let mut a_snaps: Vec<[f32; metal_schema::TONE]> = Vec::new();
        let mut b_snaps: Vec<[f32; metal_schema::TONE]> = Vec::new();

        // Offline style: frame, frame, frame…, snapshot after every batch of 8.
        for (i, (note, onset)) in frames.iter().enumerate() {
            one_at_a_time.push_frame(note, onset, DT);
            if (i + 1) % 8 == 0 {
                a_snaps.push(one_at_a_time.columns());
            }
        }

        // Live style: eight `push_frame` calls in a row per snapshot, same dt.
        for group in frames.chunks(8) {
            for (note, onset) in group {
                batched.push_frame(note, onset, DT);
            }
            b_snaps.push(batched.columns());
        }

        assert_eq!(a_snaps.len(), b_snaps.len(), "different number of snapshots");
        assert!(!a_snaps.is_empty(), "the fixture produced no snapshots");
        for (i, (a, b)) in a_snaps.iter().zip(&b_snaps).enumerate() {
            for c in 0..metal_schema::TONE {
                assert!(
                    (a[c] - b[c]).abs() < 1e-6,
                    "snapshot {i} column {} ({}) differs: {} vs {}",
                    c,
                    metal_schema::TONE_COLUMNS[c],
                    a[c],
                    b[c]
                );
            }
        }
    }

    /// A mono 16-bit wav of `samples` at `rate`, in the container the decoder
    /// reads. A local copy of `file.rs`'s test helper because a test module in
    /// a different file cannot reach it.
    #[cfg(all(feature = "decode", feature = "tone"))]
    fn wav_bytes(samples: &[i16], rate: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + samples.len() as u32 * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        bytes
    }

    /// With no model (D9), the offline tone pass must leave the music prefix and
    /// voice block **bit-identical** to the plain digest and zero the six tone
    /// columns, reporting `loaded == false`. This is what makes the block safe
    /// to ship before `model.onnx` is fetched.
    #[cfg(all(feature = "decode", feature = "tone"))]
    #[test]
    fn no_model_leaves_the_other_blocks_untouched_and_zeroes_the_tone() {
        let dir = std::env::temp_dir().join(format!("metal-listen-tone-{}", std::process::id()));
        metal_vfs::create_dir_all(&dir).unwrap();
        let wav = dir.join("tone.wav");
        let rate = 22_050u32;
        let samples: Vec<i16> = (0..rate / 2)
            .map(|i| {
                ((i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin() * 12_000.0) as i16
            })
            .collect();
        metal_vfs::write(&wav, wav_bytes(&samples, rate)).unwrap();

        let plain = crate::file::digest_file(&wav).unwrap();
        let (with_tone, loaded) = crate::file::digest_file_with_tone(&wav, None).unwrap();

        assert!(!loaded, "no model path means nothing was loaded");
        assert_eq!(plain.len(), with_tone.len(), "splicing the block changed the row count");
        let boundary = crate::MUSIC + crate::VOICE;
        for (i, (a, b)) in plain.iter().zip(&with_tone).enumerate() {
            assert_eq!(
                &a[..boundary],
                &b[..boundary],
                "row {i}: music prefix / voice block moved"
            );
            for (c, &v) in b[boundary..].iter().enumerate() {
                assert_eq!(
                    v,
                    0.0,
                    "row {i} column {} ({}..{}) must be zero with no model",
                    boundary + c,
                    metal_schema::TONE_COLUMNS[c],
                    boundary
                );
            }
        }
        let _ = metal_vfs::remove_dir_all(&dir);
    }
}
