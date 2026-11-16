//! Proof frames for the four parallax worlds: every scenario as a PNG, one
//! contact sheet per world, and a 120-frame run checked for pops, blank and
//! white frames.
//!
//! `cargo run -p metal-render --example parallax_proof -- [out-dir] [only-world]`
//! (default out-dir `/tmp/claude-1000/parallax-shots`).
use std::path::{Path, PathBuf};

use metal_render::headless::Renderer;
use metal_schema::{Block, Schema};

const W: u32 = 480;
const H: u32 = 270;

fn put(schema: &Schema, row: &mut [f32], name: &str, v: f32) {
    let mut at = 0;
    for b in schema.blocks {
        match b {
            Block::Floats(fs) => {
                if let Some(k) = fs.iter().position(|f| f.name == name) {
                    row[at + k] = v.clamp(fs[k].min, fs[k].max);
                    return;
                }
            }
            Block::ColorSlot(n, _) if *n == name => panic!("{name} is a colour slot"),
            _ => {}
        }
        at += b.width();
    }
    panic!("no float {name}");
}

fn slot(schema: &Schema, row: &mut [f32], name: &str, c: [[f32; 3]; 4]) {
    let mut at = 0;
    for b in schema.blocks {
        if let Block::ColorSlot(n, _) = b {
            if *n == name {
                for i in 0..4 {
                    row[at + i * 3..at + i * 3 + 3].copy_from_slice(&c[i]);
                }
                return;
            }
        }
        at += b.width();
    }
    panic!("no slot {name}");
}

fn cam(schema: &Schema, row: &mut [f32], v: [f32; 7]) {
    let mut at = 0;
    for b in schema.blocks {
        if let Block::Camera(_) = b {
            row[at..at + 7].copy_from_slice(&v);
            return;
        }
        at += b.width();
    }
}

type Tweak = fn(&Schema, &mut [f32]);

fn scenarios() -> Vec<(&'static str, f32, Tweak)> {
    vec![
        ("default", 3.0, |_, _| {}),
        ("scroll-right", 8.0, |s, r| put(s, r, "direction", 1.0)),
        ("scroll-left", 8.0, |s, r| put(s, r, "direction", -1.0)),
        ("still", 8.0, |s, r| put(s, r, "direction", 0.0)),
        ("speeds-extreme", 8.0, |s, r| {
            put(s, r, "speed", 3.0);
            put(s, r, "speed_1", -2.0);
            put(s, r, "speed_3", 3.0);
            put(s, r, "speed_5", 3.0);
        }),
        ("depths-reversed", 3.0, |s, r| {
            for (i, d) in [0.6, 1.5, 4.0, 12.0, 40.0].iter().enumerate() {
                put(s, r, &format!("prof_{}", i + 1), *d);
            }
        }),
        ("depths-flat-far", 3.0, |s, r| {
            for i in 1..=5 {
                put(s, r, &format!("prof_{i}"), 40.0);
            }
        }),
        ("depths-close", 3.0, |s, r| {
            for (i, d) in [3.0, 2.2, 1.4, 0.9, 0.5].iter().enumerate() {
                put(s, r, &format!("prof_{}", i + 1), *d);
            }
        }),
        ("light-0", 3.0, |s, r| put(s, r, "light", 0.0)),
        ("light-max-magenta", 3.0, |s, r| {
            put(s, r, "light", 3.0);
            put(s, r, "halo", 2.0);
            slot(s, r, "lights", [[1.0, 0.2, 0.7], [1.0, 0.5, 0.1], [0.9, 0.1, 1.0], [1.0, 0.3, 0.3]]);
        }),
        ("light-max-cyan", 3.0, |s, r| {
            put(s, r, "light", 3.0);
            put(s, r, "halo", 1.5);
            slot(s, r, "lights", [[0.2, 0.9, 1.0], [0.3, 1.0, 0.6], [0.5, 0.6, 1.0], [0.2, 1.0, 0.9]]);
        }),
        ("recolour", 3.0, |s, r| {
            slot(s, r, "layers", [[0.55, 0.20, 0.40], [0.40, 0.12, 0.32], [0.25, 0.06, 0.22], [0.10, 0.02, 0.10]]);
            slot(s, r, "objects", [[1.0, 0.75, 0.2], [1.0, 0.4, 0.1], [0.9, 0.9, 0.3], [0.6, 0.3, 0.1]]);
            slot(s, r, "background", [[0.05, 0.02, 0.10], [0.25, 0.05, 0.25], [0.55, 0.15, 0.35], [0.20, 0.05, 0.15]]);
            slot(s, r, "fog_c", [[0.4, 0.1, 0.3]; 4]);
        }),
        ("camera-high-wide", 3.0, |s, r| cam(s, r, [1.0, 0.8, 0.5, 0.0, -0.6, -10.0, 85.0])),
        ("camera-tele", 3.0, |s, r| cam(s, r, [0.0, 0.0, 0.0, 0.3, 0.1, -10.0, 28.0])),
        ("camera-bob-max", 5.3, |s, r| {
            put(s, r, "swing", 1.0);
            put(s, r, "swing_speed", 3.0);
        }),
        ("head-x-min", 3.0, |s, r| put(s, r, "cam_x", -2.0)),
        ("head-x-max", 3.0, |s, r| put(s, r, "cam_x", 2.0)),
        ("head-y-min", 3.0, |s, r| put(s, r, "cam_y", -2.0)),
        ("head-y-max", 3.0, |s, r| put(s, r, "cam_y", 2.0)),
        ("head-z-min", 3.0, |s, r| put(s, r, "cam_z", -2.0)),
        ("head-z-max", 3.0, |s, r| put(s, r, "cam_z", 2.0)),
        ("pixel-big-dense", 3.0, |s, r| {
            put(s, r, "pixel", 1.0);
            put(s, r, "size", 2.5);
            put(s, r, "density", 1.0);
        }),
        ("sparse-small", 3.0, |s, r| {
            put(s, r, "density", 0.0);
            put(s, r, "size", 0.4);
        }),
        ("late-1h", 3600.0, |_, _| {}),
    ]
}

fn write_png(path: &Path, px: &[u8], w: u32, h: u32) {
    let f = std::fs::File::create(path).expect("create png");
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(px).unwrap();
}

struct Sheet {
    cols: u32,
    px: Vec<u8>,
    w: u32,
    h: u32,
}

impl Sheet {
    fn new(n: usize, cols: u32, tw: u32, th: u32) -> Sheet {
        let rows = (n as u32).div_ceil(cols);
        let (w, h) = (cols * (tw + 4) + 4, rows * (th + 4) + 4);
        Sheet { cols, px: vec![16; (w * h * 4) as usize], w, h }
    }
    fn place(&mut self, k: usize, img: &[u8], tw: u32, th: u32) {
        let (cx, cy) = (k as u32 % self.cols, k as u32 / self.cols);
        let (ox, oy) = (4 + cx * (tw + 4), 4 + cy * (th + 4));
        for y in 0..th {
            let src = (y * tw * 4) as usize;
            let dst = (((oy + y) * self.w + ox) * 4) as usize;
            self.px[dst..dst + (tw * 4) as usize].copy_from_slice(&img[src..src + (tw * 4) as usize]);
        }
    }
}

fn stats(px: &[u8]) -> (f32, f32, f32) {
    let n = px.len() / 4;
    let (mut sum, mut sq, mut white) = (0f64, 0f64, 0usize);
    for p in px.chunks(4) {
        let l = (p[0] as f64 + p[1] as f64 + p[2] as f64) / (3.0 * 255.0);
        sum += l;
        sq += l * l;
        if p[0] > 245 && p[1] > 245 && p[2] > 245 {
            white += 1;
        }
    }
    let mean = sum / n as f64;
    ((mean) as f32, ((sq / n as f64 - mean * mean).max(0.0).sqrt()) as f32, white as f32 / n as f32)
}

fn diff(a: &[u8], b: &[u8]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as f64).sum::<f64>() as f32
        / a.len() as f32
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(a.first().cloned().unwrap_or("/tmp/claude-1000/parallax-shots".into()));
    let only = a.get(1).cloned();
    let mut r = Renderer::new();
    let ids = r.ids();
    for world in ["parallax-cave", "parallax-sky", "parallax-fields", "parallax-city"] {
        if only.as_deref().is_some_and(|o| o != world) {
            continue;
        }
        let i = ids.iter().position(|id| *id == world).expect("registered");
        let schema = r.schemas()[i];
        let dir = out.join(world);
        std::fs::create_dir_all(&dir).unwrap();
        let sc = scenarios();
        let mut sheet = Sheet::new(sc.len(), 5, W, H);
        for (k, (name, t, tweak)) in sc.iter().enumerate() {
            let mut row = schema.defaults();
            tweak(schema, &mut row);
            let (px, w, h) = r.solo(i, &row, *t, W, H, false);
            let (mean, sd, white) = stats(&px);
            let flag = if sd < 0.02 || white > 0.2 || mean > 0.9 { "  <-- CHECK" } else { "" };
            println!("{world} {k:02}-{name}: mean {mean:.3} sd {sd:.3} white {white:.3}{flag}");
            write_png(&dir.join(format!("{k:02}-{name}.png")), &px, w, h);
            sheet.place(k, &px, w, h);
        }
        write_png(&out.join(format!("sheet-{world}.png")), &sheet.px, sheet.w, sheet.h);

        // 120 consecutive frames at 30 fps, at the start of a set and an hour in.
        for (tag, t0) in [("t10", 10.0f32), ("t3600", 3600.0)] {
            let row = schema.defaults();
            let mut prev: Option<Vec<u8>> = None;
            let mut diffs = Vec::new();
            let mut bad = 0;
            let mut seq = Sheet::new(12, 4, W / 2, H / 2);
            for f in 0..120 {
                let (px, w, h) = r.solo(i, &row, t0 + f as f32 / 30.0, W, H, false);
                let (mean, sd, white) = stats(&px);
                if sd < 0.02 || white > 0.2 || mean > 0.9 {
                    bad += 1;
                }
                if let Some(p) = &prev {
                    diffs.push(diff(p, &px));
                }
                if f % 10 == 0 {
                    // half-size thumbnail
                    let mut th = vec![0u8; ((w / 2) * (h / 2) * 4) as usize];
                    for y in 0..h / 2 {
                        for x in 0..w / 2 {
                            let s = (((y * 2) * w + x * 2) * 4) as usize;
                            let d = ((y * (w / 2) + x) * 4) as usize;
                            th[d..d + 4].copy_from_slice(&px[s..s + 4]);
                        }
                    }
                    seq.place(f / 10, &th, w / 2, h / 2);
                }
                prev = Some(px);
            }
            let mut sorted = diffs.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = sorted[sorted.len() / 2];
            let max = *sorted.last().unwrap();
            let pops = diffs.iter().filter(|d| **d > (med * 4.0).max(1.5)).count();
            println!(
                "{world} seq {tag}: frame diff median {med:.2} max {max:.2}, pops {pops}, blank/white frames {bad}"
            );
            write_png(&out.join(format!("seq-{world}-{tag}.png")), &seq.px, seq.w, seq.h);
        }
    }
}
