//! H0 (rule 11): measure the detector before choosing one.
//!
//! Runs a detector over `N` frames of a synthetic clip at 640×360 and prints
//! frames per second and end-to-end latency per frame, for every arm this
//! build can actually run:
//!
//! | arm | how it is built |
//! |---|---|
//! | `stand-in` | always: the plumbing arm, no inference, the honest floor |
//! | `mediapipe` | `--features mediapipe` and a model the `ort` session can open |
//!
//! The target from `the design notes` § H0 is **≥ 30 fps and ≤ 50 ms camera-to-picture**.
//! A number is printed even when nothing reaches it, because a slow honest
//! number is the measurement; only the Pi row is a hardware step this host
//! cannot take.
//!
//! Usage:
//!
//! ```text
//! cargo run -p metal-sense --example vision_bench
//! cargo run -p metal-sense --example vision_bench --features mediapipe
//! ```
//!
//! `FW_VISION_MODEL` names the ONNX model for the `mediapipe` arm; without it
//! the arm looks under `~/.cache/metalvj/models/pose_landmarker_heavy.onnx`,
//! the same path `scripts/setup-vision` writes and `MediaPipeDetector::try_load`
//! reads. `FW_VISION_FRAMES` overrides the frame count (default 120).

use std::time::Instant;

use metal_sense::{Detector, Image, Mode, StandInDetector};

/// The measurement resolution `the design notes` § H0 pins.
const W: u32 = 640;
const H: u32 = 360;

/// A synthetic frame: a dark field with a bright square that walks across it,
/// so a detector that looks at pixels has something to find, and one that does
/// not is still fed the same bytes.
fn synthetic(i: u64) -> Image {
    let mut rgba = vec![24u8; (W * H * 4) as usize];
    let step = (i % W as u64) as u32;
    for y in 0..48u32 {
        for x in 0..48u32 {
            let px = (step + x) % W;
            let py = H / 2 - 24 + y;
            let at = ((py * W + px) * 4) as usize;
            rgba[at] = 230;
            rgba[at + 1] = 210;
            rgba[at + 2] = 180;
        }
    }
    Image::new(W, H, rgba)
}

struct Arm {
    name: &'static str,
    detector: Box<dyn Detector>,
}

fn main() {
    let frames: u64 = std::env::var("FW_VISION_FRAMES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);

    let mut arms: Vec<Arm> = vec![Arm {
        name: "stand-in",
        detector: Box::new(StandInDetector::fixed(
            Mode::Eyes,
            metal_sense::Control::new([("eye_x", 0.5), ("eye_y", 0.5), ("eye_z_est", 0.5)]),
        )),
    }];

    #[cfg(feature = "mediapipe")]
    {
        match mediapipe_arm() {
            Ok(d) => arms.push(Arm { name: "mediapipe", detector: d }),
            Err(e) => eprintln!("mediapipe arm unavailable: {e}"),
        }
    }
    #[cfg(not(feature = "mediapipe"))]
    eprintln!("mediapipe arm not built: rebuild with `--features mediapipe`");

    println!("vision bench · {W}×{H} · {frames} frames");
    println!("{:<12} {:>10} {:>12} {:>12}", "arm", "fps", "mean ms", "p95 ms");
    for arm in arms.iter_mut() {
        let mut times = Vec::with_capacity(frames as usize);
        let start = Instant::now();
        for i in 0..frames {
            let frame = synthetic(i);
            let t = Instant::now();
            let _ = arm.detector.detect(&frame);
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        let wall = start.elapsed().as_secs_f64();
        let fps = frames as f64 / wall.max(1e-9);
        times.sort_by(|a, b| a.total_cmp(b));
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        let p95 = times[((times.len() as f64 * 0.95) as usize).min(times.len() - 1)];
        println!("{:<12} {fps:>10.1} {mean:>12.2} {p95:>12.2}", arm.name);
    }
}

#[cfg(feature = "mediapipe")]
fn mediapipe_arm() -> Result<Box<dyn Detector>, String> {
    let path = std::env::var("FW_VISION_MODEL").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        metal_config::app_dir_in(&std::path::Path::new(&home).join(".cache"))
            .join("models/pose_landmarker_heavy.onnx")
            .to_string_lossy()
            .into_owned()
    });
    if !std::path::Path::new(&path).is_file() {
        return Err(format!("no model at {path} (run scripts/setup-vision)"));
    }
    metal_sense::MediaPipeDetector::new(Mode::Eyes, &path).map(|d| Box::new(d) as Box<dyn Detector>)
}
