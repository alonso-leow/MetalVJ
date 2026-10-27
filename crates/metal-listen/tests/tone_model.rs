//! Step 3's end-to-end check: a synthetic held tone through the real Basic
//! Pitch model and the tone reducer. Runs only when the model is fetched; on a
//! checkout without `data/models/basic-pitch/model.onnx` each test prints why
//! and passes, so CI without the binary stays green.

#![cfg(feature = "tone")]

use std::path::PathBuf;

use metal_listen::tone::basic_pitch::{self, AUDIO_N_SAMPLES, FPS};
use metal_listen::tone::reducer::{Reducer, KEYS};
use metal_listen::SR;

fn model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/models/basic-pitch/model.onnx")
}

/// Load the model, or `None` with a skip message when it is not fetched.
fn load_model(test: &str) -> Option<metal_ml::Model> {
    let path = model_path();
    if !path.is_file() {
        eprintln!("{test}: skipping — no model at {} (fetch it first)", path.display());
        return None;
    }
    Some(metal_ml::Model::load(&path).expect("the fetched Basic Pitch model loads"))
}

/// A sum of sines at `freqs` Hz, `secs` long at the canonical rate, at 0.2
/// amplitude each so the sum stays well inside [-1, 1].
fn chord(freqs: &[f32], secs: f32) -> Vec<f32> {
    let n = (SR as f32 * secs) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / SR as f32;
            freqs
                .iter()
                .map(|f| (t * f * std::f32::consts::TAU).sin() * 0.2)
                .sum()
        })
        .collect()
}

/// Run the model over a steady signal in whole windows and reduce the frames
/// the way the live path does, returning the reducer and the mean note
/// posterior per key over every frame.
fn run_and_reduce(model: &metal_ml::Model, samples: &[f32]) -> (Reducer, [f32; KEYS], usize) {
    let mut reducer = Reducer::new();
    let mut mean = [0.0f32; KEYS];
    let mut frames_total = 0usize;
    let hop = 2_048usize;
    let mut end = AUDIO_N_SAMPLES.min(samples.len());
    while end <= samples.len() {
        let frames = basic_pitch::run_window(model, &samples[end - AUDIO_N_SAMPLES..end])
            .expect("a full window runs");
        let newest = basic_pitch::newest(&frames, 8);
        for i in 0..newest.note.len() {
            reducer.push_frame(&newest.note[i], &newest.onset[i], 1.0 / FPS);
        }
        for f in &frames.note {
            for k in 0..KEYS {
                mean[k] += f[k];
            }
            frames_total += 1;
        }
        if end == samples.len() {
            break;
        }
        end = (end + hop).min(samples.len());
    }
    for v in mean.iter_mut() {
        *v /= frames_total.max(1) as f32;
    }
    (reducer, mean, frames_total)
}

/// Sum the mean posterior into the 12 pitch classes and return the top `n`
/// (MIDI note mod 12 = pitch class; key k is MIDI k + 21).
fn top_pitch_classes(mean: &[f32; KEYS], n: usize) -> Vec<usize> {
    let mut pc = [0.0f32; 12];
    for (k, v) in mean.iter().enumerate() {
        pc[(k + 21) % 12] += v;
    }
    let mut order: Vec<usize> = (0..12).collect();
    order.sort_by(|&a, &b| pc[b].partial_cmp(&pc[a]).unwrap());
    order.truncate(n);
    order
}

#[test]
fn a_sustained_a4_reads_as_pitch_class_a_with_tone_out() {
    let Some(model) = load_model("a_sustained_a4") else { return };
    // Two seconds of 440 Hz: enough for several full windows and a settled
    // reducer.
    let samples = chord(&[440.0], 2.0);
    let (reducer, mean, frames) = run_and_reduce(&model, &samples);
    assert!(frames > 0, "the model produced frames");

    // A4 is MIDI 69 = pitch class 9 (A). It must be the loudest pitch class.
    assert_eq!(top_pitch_classes(&mean, 1), vec![9], "dominant pitch class must be A");

    let cols = reducer.columns();
    assert!(cols.iter().any(|v| *v != 0.0), "tone block is not all zero: {cols:?}");
    assert!(reducer.conf() > 0.3, "a held note is confident: conf {}", reducer.conf());
    assert!(cols.iter().all(|v| v.is_finite()), "every column is finite");
}

#[test]
fn a_c_major_triad_reads_as_pitch_classes_c_e_g() {
    let Some(model) = load_model("a_c_major_triad") else { return };
    // C4, E4, G4: 261.63, 329.63, 392.00 Hz.
    let samples = chord(&[261.63, 329.63, 392.00], 2.0);
    let (_, mean, _) = run_and_reduce(&model, &samples);
    let mut top = top_pitch_classes(&mean, 3);
    top.sort();
    assert_eq!(top, vec![0, 4, 7], "the three loudest pitch classes must be C, E, G");
}

#[test]
fn silence_reads_as_low_confidence() {
    let Some(model) = load_model("silence") else { return };
    let samples = vec![0.0f32; SR as usize * 2];
    let (reducer, _, _) = run_and_reduce(&model, &samples);
    // The model's note head still emits a low floor on true silence (measured
    // ~0.16), so "zero confidence" is not what the model does. The check is that
    // silence stays well below a held note's confidence (> 0.3 above).
    assert!(
        reducer.conf() < 0.3,
        "silence must read as low confidence, got {}",
        reducer.conf()
    );
}
