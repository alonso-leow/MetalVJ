//! 2026-10-01: video → library → HAP proxy → resident on the GPU.
//!
//! The whole ingestion path on a real ffmpeg and a real adapter. Skipped
//! (passes empty) where ffmpeg cannot write a test clip or has no `hap`.

use metal_render::gpu::Gpu;
use metal_render::shared::footage::Frames;
use metal_reel::index::{self, Options};

#[test]
fn a_video_reaches_the_gpu_through_its_hap_proxy() {
    let dir = std::env::temp_dir().join(format!("fw-hap-gpu-{}", std::process::id()));
    let source = dir.join("source");
    std::fs::create_dir_all(&source).unwrap();
    let made = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x180:rate=25", "-t", "1", "-pix_fmt", "yuv420p"])
        .arg(source.join("bars.mp4"))
        .status();
    if !made.is_ok_and(|s| s.success()) {
        return;
    }
    let data = dir.join("data");
    let lib = index::build(&source, &data, "t", &Options { width: 160, ..Options::default() }, &mut |_| {}).unwrap();
    let clip = lib.clip(0).unwrap();
    let Some(hap) = lib.hap(clip) else {
        return; // no `hap` encoder in this ffmpeg
    };
    assert_eq!(hap.frames(), clip.rungs());
    assert_eq!((hap.index.width % 4, hap.index.height % 4), (0, 0));

    let gpu = Gpu::new_headless();
    let mut frames = Frames::default();
    for k in 0..clip.rungs() {
        assert!(frames.resident(&gpu.device, &gpu.queue, &lib, clip, k).is_some(), "rung {k}");
    }
    let bc = gpu.device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
    let want = if bc { wgpu::TextureFormat::Bc1RgbaUnormSrgb } else { wgpu::TextureFormat::Rgba8UnormSrgb };
    eprintln!("resident as {:?}", frames.format());
    assert_eq!(frames.format(), Some(want));
    assert_eq!(frames.allocated(), clip.rungs());
    let _ = std::fs::remove_dir_all(&dir);
}
