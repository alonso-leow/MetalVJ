//! Inline `material_apply` against a baked albedo picture, on the raymarched
//! models, in one run so the ratio holds under load.
//!
//!     cargo run --release -p metal-render --bin material-bench
//!
//! Per model and resolution it renders these configs round-robin (frame k of
//! every config before frame k+1 of any), so a load spike hits all of them:
//! `plain` (no material), `baked` (a 1024x1024 picture on the part, mix 1, no
//! material: the cost of sampling a pre-baked albedo, not of baking it) and
//! each material kind inline at mix 1. Each frame is encode + submit + a wait
//! for the queue to drain. Prints p50 ms and the ratio to `baked`.

use std::time::Instant;

use metal_render::gpu::{offscreen_target, Gpu};
use metal_render::model::{Ctx, Mapping, MaterialSpec, Model, SurfaceSpec};
use metal_render::registry::all_models;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const KINDS: [&str; 7] = ["matte", "concrete", "wood", "leather", "metal", "glass", "matrix"];

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn loadavg() -> String {
    std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim().to_string()
}

fn main() {
    let frames: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(60);
    let gpu = Gpu::new_headless();
    let info = gpu.adapter.get_info();
    println!("adapter: {} ({:?}, {:?})", info.name, info.device_type, info.backend);
    println!("loadavg before: {}", loadavg());
    let px: Vec<u8> = (0..1024 * 1024)
        .flat_map(|i| {
            let (x, y) = ((i % 1024) as f32, (i / 1024) as f32);
            let v = ((x * 0.07).sin() * (y * 0.05).cos() * 0.5 + 0.5) * 255.0;
            [v as u8, (255.0 - v) as u8, 90, 255]
        })
        .collect();
    for id in ["scene3d", "bulb"] {
        for (w, h) in [(1280u32, 720u32), (1920, 1080)] {
            let mut model: Box<dyn Model> = all_models().into_iter().find(|m| m.id() == id).expect("model");
            let floats = model.schema().defaults();
            let (_tex, view) = offscreen_target(&gpu.device, w, h, FORMAT);
            // (name, spec per part)
            let mut configs: Vec<(String, Option<SurfaceSpec>)> = vec![
                ("plain".into(), None),
                ("baked".into(), Some(SurfaceSpec::image(&px, 1024, 1024, 1.0, Mapping::Object, 1.0))),
            ];
            for (k, name) in KINDS.iter().enumerate() {
                let m = MaterialSpec { kind: k as f32, mix: 1.0, scale: 1.0, ..Default::default() };
                configs.push((format!("inline {name}"), Some(SurfaceSpec::material(m))));
            }
            let mut times: Vec<Vec<f64>> = vec![Vec::new(); configs.len()];
            for round in 0..frames + 5 {
                for (c, (_, spec)) in configs.iter().enumerate() {
                    for part in model.surfaces() {
                        model.set_surface(part, spec.clone());
                    }
                    let ctx = Ctx {
                        device: &gpu.device,
                        queue: &gpu.queue,
                        time: 1.0 + round as f32 * 0.016,
                        width: w,
                        height: h,
                        fondo_alpha: false,
                        detail: 1.0,
                    };
                    let t0 = Instant::now();
                    model.render(&ctx, &floats, &view, FORMAT);
                    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
                    if round >= 5 {
                        times[c].push(t0.elapsed().as_secs_f64() * 1000.0);
                    }
                }
            }
            let p50: Vec<f64> = times.iter_mut().map(|t| pct(t, 0.5)).collect();
            println!("\n{id} {w}x{h}  ({frames} frames/config, round-robin)");
            println!("{:<16} {:>8} {:>10} {:>10}", "config", "p50 ms", "vs baked", "vs plain");
            for (c, (name, _)) in configs.iter().enumerate() {
                println!("{:<16} {:8.2} {:9.2}x {:9.2}x", name, p50[c], p50[c] / p50[1], p50[c] / p50[0]);
            }
        }
    }
    println!("\nloadavg after: {}", loadavg());
}
