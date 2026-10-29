//! Per-model cost, CPU and GPU, with the floats moving the whole time.
//!
//!     cargo run --release -p metal-render --bin render-bench -- --w 1920 --h 1080
//!
//! For every model it draws `--frames` frames through
//! [`Renderer::solo_chain_into`] (the live path: no readback), sweeping every
//! expressive float (drive >= 0.4) through its range so a model that only gets
//! expensive at some setting shows it. Per frame it records:
//!
//! - `model_cpu`: time inside `Model::render` alone (encode + submit + any
//!   CPU simulation the model does);
//! - `cpu`: the whole call (model + post chain + effects);
//! - `gpu`: the wait for the queue to drain right after — what the GPU still
//!   had to do once the CPU let go (a lower bound on GPU time, an honest
//!   ranking).
//!
//! The first frame is reported apart as `build` (pipeline/shader compile).
//! Output: a table sorted by `cpu + gpu` p50 on stdout, `--csv <path>` for
//! the raw per-model numbers. `--only <id>` benches one model.

use std::time::{Duration, Instant};

use metal_render::gpu::offscreen_target;
use metal_render::headless::Renderer;
use metal_render::shared::post::EstelaParams;
use metal_schema::{Block, FloatSpec};

struct Args {
    w: u32,
    h: u32,
    frames: u32,
    only: Option<String>,
    csv: Option<String>,
    still: bool,
}

fn parse_args() -> Args {
    let mut a = Args { w: 1920, h: 1080, frames: 120, only: None, csv: None, still: false };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut next = || it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--w" => a.w = next().parse().expect("bad --w"),
            "--h" => a.h = next().parse().expect("bad --h"),
            "--frames" => a.frames = next().parse().expect("bad --frames"),
            "--only" => a.only = Some(next()),
            "--csv" => a.csv = Some(next()),
            "--still" => a.still = true,
            other => panic!("unknown argument {other}"),
        }
    }
    a
}

/// A float swept through its legal range over time; structural floats
/// (drive < 0.4) stay put, same rule as `model-demo --vary`.
fn sweep(s: &FloatSpec, field: usize, t: f32) -> f32 {
    if s.drive < 0.4 || s.max <= s.min {
        return s.default;
    }
    let frac = 0.5 + 0.48 * (t * (0.7 + field as f32 * 0.13) + field as f32).sin();
    s.min + (s.max - s.min) * frac
}

fn row(schema: &metal_schema::Schema, t: f32, still: bool) -> Vec<f32> {
    if still {
        return schema.defaults();
    }
    let mut out = Vec::with_capacity(schema.width());
    let mut field = 0usize;
    for block in schema.blocks {
        match block {
            Block::Floats(fs) => {
                for s in fs.iter() {
                    out.push(sweep(s, field, t));
                    field += 1;
                }
            }
            Block::ColorSlot(_, c) => out.extend(c.iter()),
            Block::Camera(view) => out.extend(view),
        }
    }
    out
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

struct Row {
    id: &'static str,
    build: f64,
    model_cpu: [f64; 2],
    cpu: [f64; 2],
    gpu: [f64; 2],
}

fn main() {
    let a = parse_args();
    let mut r = Renderer::new();
    eprintln!("adapter backend {} (real hardware: {})", r.backend_name(), r.real_adapter());
    let (device, _) = r.device_queue();
    let (target, _) = offscreen_target(&device, a.w, a.h, wgpu::TextureFormat::Rgba8UnormSrgb);
    let ids = r.ids();
    let schemas = r.schemas();
    let wake = EstelaParams::default();
    let mut rows = Vec::new();

    for (i, id) in ids.iter().enumerate() {
        if schemas[i].width() == 0 || !r.runs_here(i) {
            continue;
        }
        if a.only.as_deref().is_some_and(|o| o != *id) {
            continue;
        }
        let draw = |r: &mut Renderer, t: f32| -> (Duration, Duration, Duration) {
            let floats = row(schemas[i], t, a.still);
            let c0 = Instant::now();
            r.solo_chain_into(i, &floats, t, false, &[], &wake, &target);
            let cpu = c0.elapsed();
            let g0 = Instant::now();
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            (r.last_model_cpu, cpu, g0.elapsed())
        };
        let b0 = Instant::now();
        draw(&mut r, 0.0);
        let build = ms(b0.elapsed());
        let (mut mc, mut c, mut g) = (vec![], vec![], vec![]);
        for f in 1..=a.frames {
            let (m, cpu, gpu) = draw(&mut r, f as f32 / 60.0);
            mc.push(ms(m));
            c.push(ms(cpu));
            g.push(ms(gpu));
        }
        let two = |v: &mut Vec<f64>| [pct(v, 0.5), pct(v, 0.95)];
        let row = Row { id, build, model_cpu: two(&mut mc), cpu: two(&mut c), gpu: two(&mut g) };
        eprintln!(
            "{:<22} cpu {:6.2} gpu {:6.2} ms",
            row.id, row.cpu[0], row.gpu[0]
        );
        rows.push(row);
    }

    rows.sort_by(|x, y| (y.cpu[0] + y.gpu[0]).partial_cmp(&(x.cpu[0] + x.gpu[0])).unwrap());
    println!(
        "{:<22} {:>8} {:>9} {:>9} {:>8} {:>8} {:>8} {:>8}",
        "model", "build", "mcpu p50", "mcpu p95", "cpu p50", "cpu p95", "gpu p50", "gpu p95"
    );
    for x in &rows {
        println!(
            "{:<22} {:8.1} {:9.2} {:9.2} {:8.2} {:8.2} {:8.2} {:8.2}",
            x.id, x.build, x.model_cpu[0], x.model_cpu[1], x.cpu[0], x.cpu[1], x.gpu[0], x.gpu[1]
        );
    }
    if let Some(path) = a.csv {
        let mut s = String::from("model,build_ms,model_cpu_p50,model_cpu_p95,cpu_p50,cpu_p95,gpu_p50,gpu_p95\n");
        for x in &rows {
            s += &format!(
                "{},{:.2},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3}\n",
                x.id, x.build, x.model_cpu[0], x.model_cpu[1], x.cpu[0], x.cpu[1], x.gpu[0], x.gpu[1]
            );
        }
        std::fs::write(&path, s).expect("write csv");
        eprintln!("wrote {path}");
    }
}
