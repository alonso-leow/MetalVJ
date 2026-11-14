//! The two checks that do not need a fetched model: the hash helper against a
//! known digest, and the `load_checked` gate firing before any ONNX call. The
//! third test runs a real fixture only when one is present, and skips loudly
//! otherwise — the repository never carries a binary, so CI without a model
//! must still pass.

use std::path::PathBuf;

use metal_ml::{MlError, Model};

/// A file's sha256 is the one piece of fetch/load trust that can be tested in
/// isolation. `abc` is the canonical vector, so a regression in the hashing
/// path is caught without an ONNX binary anywhere near the test.
#[test]
fn sha256_of_abc_matches_the_known_digest() {
    let dir = std::env::temp_dir().join("metal-ml-sha");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("abc");
    std::fs::write(&path, b"abc").unwrap();
    let got = metal_ml::sha256_file(&path).unwrap();
    assert_eq!(
        got,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A wrong hash must be rejected by the check, before the runtime is asked to
/// parse the file. The file here is not ONNX at all; if `load_checked` reached
/// `ort` the error would be an `Ort`, not a `HashMismatch`.
#[test]
fn load_checked_rejects_a_bad_hash_before_touching_onnx() {
    let dir = std::env::temp_dir().join("metal-ml-badhash");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("not.onnx");
    std::fs::write(&path, b"not an onnx model").unwrap();
    match Model::load_checked(&path, "deadbeef") {
        Err(MlError::HashMismatch { .. }) => {}
        Err(other) => panic!("expected HashMismatch, got {other:?}"),
        Ok(_) => panic!("expected a hash mismatch, but the model loaded"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Load and run a fixture model when one is available: `FW_ML_TEST_MODEL`, or
/// `tests/fixtures/identity.onnx`, or skip. No fixture is committed, so an
/// absent one is the normal case and prints why rather than failing.
#[test]
fn runs_a_fixture_model_when_present() {
    let path = std::env::var("FW_ML_TEST_MODEL")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("tests/fixtures/identity.onnx"));
    if !path.is_file() {
        eprintln!(
            "metal-ml: skipping fixture test — no model at {} (set FW_ML_TEST_MODEL to run it)",
            path.display()
        );
        return;
    }
    let model = Model::load(&path).expect("fixture loads");
    let out = model.run(&[1.0, 2.0, 3.0, 4.0], &[1, 4]).expect("runs");
    assert!(!out.is_empty(), "a model returns at least one output");
    let first = &out[0];
    assert!(!first.shape.is_empty(), "an output has a shape");
    assert_eq!(
        first.shape.iter().product::<usize>(),
        first.data.len(),
        "shape {shape:?} does not match {} data elements",
        first.data.len(),
        shape = first.shape
    );
}

/// The low-footprint session (one thread, no arena) must answer exactly what
/// the default one answers: it changes resources, never the numbers.
#[test]
fn light_session_matches_default_session() {
    use std::path::PathBuf;
    let path = std::env::var("FW_ML_TEST_MODEL")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("tests/fixtures/identity.onnx"));
    if !path.is_file() {
        eprintln!("metal-ml: skipping light-session test — no model at {}", path.display());
        return;
    }
    // Basic Pitch's audio window (`AUDIO_N_SAMPLES`); set FW_ML_TEST_MODEL to a
    // model with this input to run the comparison.
    let n = 43_844;
    let input: Vec<f32> = (0..n).map(|i| ((i % 97) as f32 - 48.0) / 48.0).collect();
    let shape = [1, n, 1];
    let full = Model::load(&path).expect("default loads").run(&input, &shape).expect("runs");
    let light = Model::load_light(&path).expect("light loads").run(&input, &shape).expect("runs");
    // Same graph, different thread schedule: the floats may differ in the last
    // bits, so compare shapes exactly and values within a tolerance.
    assert_eq!(full.len(), light.len());
    for (a, b) in full.iter().zip(&light) {
        assert_eq!(a.shape, b.shape);
        let worst = a.data.iter().zip(&b.data).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-4, "light session drifted by {worst}");
    }
}
