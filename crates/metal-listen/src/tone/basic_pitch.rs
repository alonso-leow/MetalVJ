//! Basic Pitch window framing — step 6.
//!
//! Basic Pitch is a frame model: one 2 s, 22 050 Hz mono window in, two
//! 88-key posteriorgrams out per model frame (`note`, `onset`). This module is
//! the framing around it — how big a window is, how fast its frames advance,
//! and how to read the outputs the runtime hands back — not the model itself.
//! [`run_window`] is the single call [`live`](super::live) and the offline path
//! share, so both see the exact same posteriors for the same audio (D5).
//!
//! The numbers are Basic Pitch's own, fixed by its training, not tuned here:
//! changing them would change what the model means.

use crate::tone::reducer::KEYS;

/// Basic Pitch's input window: 2 s at 22 050 Hz. The exported graph has a fixed
/// input of `[1, 43844, 1]` (verified against `icassp_2022/nmp.onnx`), so every
/// call must present exactly this many samples.
pub const AUDIO_N_SAMPLES: usize = 43_844;
/// Basic Pitch's frame hop, in samples of the input.
pub const FFT_HOP: usize = 256;
/// Model frames per second — ≈86.1, from the hop and rate above.
pub const FPS: f32 = 22_050.0 / FFT_HOP as f32;
/// The graph output names, as the tf2onnx export of `nmp.onnx` names them.
///
/// The session reports its outputs as `[:2, :1, :0]` (contour is `:0`, not the
/// first output), so the outputs are looked up by name, never by position.
/// `:1` is note (sustained, ~0.6 on a held A4) and `:2` is onset (a spike on
/// the attack, decaying to ~0.1) — checked by running a 440 Hz sine through the
/// model. Mirrored in `data/models/basic-pitch/model.toml`.
pub const NOTE_OUTPUT: &str = "StatefulPartitionedCall:1";
pub const ONSET_OUTPUT: &str = "StatefulPartitionedCall:2";

/// One window's worth of frames, in time order.
///
/// `note` and `onset` run parallel: `note[t][k]` and `onset[t][k]` are the
/// posteriors for key `k` at frame `t`, key 0 = MIDI 21.
#[derive(Clone, Debug, Default)]
pub struct Frames {
    pub note: Vec<[f32; KEYS]>,
    pub onset: Vec<[f32; KEYS]>,
}

/// Run one model window and return the frames it produced, in time order.
///
/// `window` is aligned to the **end** of the model's fixed input: a short window
/// (the start of a file, a live stream that has not filled yet) is zero-padded
/// at the front so its newest sample sits where the newest frames are read
/// from; a long window keeps only its last [`AUDIO_N_SAMPLES`] samples. The
/// outputs are picked by name ([`NOTE_OUTPUT`], [`ONSET_OUTPUT`]), each shaped
/// `[1, T, 88]` — flattened and chunked into `KEYS`-long frames. A shape that
/// does not divide into whole frames, a missing output, or a pair with different
/// frame counts is an error rather than a guess.
pub fn run_window(model: &metal_ml::Model, window: &[f32]) -> Result<Frames, String> {
    let mut padded = vec![0.0f32; AUDIO_N_SAMPLES];
    let n = window.len().min(AUDIO_N_SAMPLES);
    padded[AUDIO_N_SAMPLES - n..].copy_from_slice(&window[window.len() - n..]);

    let outputs = model
        .run(&padded, &[1, AUDIO_N_SAMPLES, 1])
        .map_err(|e| e.to_string())?;
    let names = model.output_names();
    let pick = |want: &str| -> Result<&metal_ml::Tensor, String> {
        names
            .iter()
            .position(|n| n == want)
            .and_then(|i| outputs.get(i))
            .ok_or_else(|| format!("basic pitch output {want} is missing (have {names:?})"))
    };
    let note = frames_of(pick(NOTE_OUTPUT)?)?;
    let onset = frames_of(pick(ONSET_OUTPUT)?)?;
    if note.len() != onset.len() {
        return Err(format!(
            "note has {} frames, onset has {}",
            note.len(),
            onset.len()
        ));
    }
    Ok(Frames { note, onset })
}

/// Flatten a `[1, T, 88]` (or `[T, 88]`) tensor into `T` frames.
fn frames_of(t: &metal_ml::Tensor) -> Result<Vec<[f32; KEYS]>, String> {
    if t.data.len() % KEYS != 0 {
        return Err(format!(
            "output of {} values is not a whole number of {} key frames",
            t.data.len(),
            KEYS
        ));
    }
    Ok(t.data
        .chunks_exact(KEYS)
        .map(|c| {
            let mut f = [0.0f32; KEYS];
            f.copy_from_slice(c);
            f
        })
        .collect())
}

/// The newest `n` frames of `frames`.
///
/// A live hop covers several model frames at once (the hop is longer than
/// [`FFT_HOP`]); the worker feeds only the frames that are new since the last
/// hop into the reducer, so history is not counted twice. Asking for more than
/// there are returns everything.
pub fn newest(frames: &Frames, n: usize) -> Frames {
    let tail = |v: &Vec<[f32; KEYS]>| v[v.len().saturating_sub(n)..].to_vec();
    Frames {
        note: tail(&frames.note),
        onset: tail(&frames.onset),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(v: f32) -> [f32; KEYS] {
        [v; KEYS]
    }

    #[test]
    fn newest_takes_the_tail_and_clamps_when_asked_for_more() {
        let frames = Frames {
            note: (0..5).map(|i| frame(i as f32)).collect(),
            onset: (0..5).map(|i| frame((i + 100) as f32)).collect(),
        };

        let two = newest(&frames, 2);
        assert_eq!(two.note.len(), 2);
        assert_eq!(two.note[0][0], 3.0);
        assert_eq!(two.note[1][0], 4.0);
        assert_eq!(two.onset[1][0], 104.0);

        // More than exist gives everything, not a panic.
        let all = newest(&frames, 99);
        assert_eq!(all.note.len(), 5);
        assert_eq!(all.note[0][0], 0.0);

        // Zero gives an empty, still well-formed pair.
        let none = newest(&frames, 0);
        assert!(none.note.is_empty() && none.onset.is_empty());
    }

    #[test]
    fn a_missing_model_path_is_an_error_not_a_panic() {
        // `run_window` needs a loaded `Model`, and the only way to get one is a
        // real `.onnx`. There is no model committed here (D9), so the earliest
        // honest assertion is that opening a missing path is `Err` — which is
        // exactly what makes `Tone` fall back to the no-model path rather than
        // panicking on a fresh checkout.
        let path = std::path::Path::new("data/models/basic-pitch/does-not-exist.onnx");
        assert!(
            metal_ml::Model::load(path).is_err(),
            "a missing model must be an error"
        );
    }
}
