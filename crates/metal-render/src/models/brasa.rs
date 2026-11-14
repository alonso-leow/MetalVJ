//! Model `ember` — the Doom-fire cellular automaton. Heat is injected on a
//! source row and climbs the grid one row per step, losing a random amount at
//! each row and being pushed sideways by the wind; a palette turns the heat
//! into flame. Written from the algorithm (`the effect design notes`
//! §2.8), no code copied.
//!
//! **Stateful, like Turing.** The grid is the accumulation, so it persists
//! between frames, and keeps the property that matters: the simulation
//! advances only when time advances. The same instant twice steps nothing; a
//! fresh instance always seeds and burns in the same way, because every
//! random number is a hash of (cell, step index), never a wall clock. The
//! burn-in replays the ignition at the times just before `ctx.time`, so the
//! first frame already shows the rhythm.
//!
//! **No spectrum reaches a model**, so `fuente_modo` 1 is a stand-in: eight
//! bands, each an envelope of its own speed, bass strongest, sped up by
//! `ignicion_ritmo`. Mode 0 is a line (shaped by `ignicion_forma`), mode 2 a
//! set of `emisores_n` wandering emitters.
//!
//! `subida` carries the sign (+ the flame rises from the bottom row, - it
//! falls from the top) and, by its magnitude, the simulation rate. `espejo`
//! (kit TWIST) folds the display into a radial fire.

use std::sync::OnceLock;

use crate::model::{color_at, background, spec, spec_held, Block, Ctx, FloatSpec, Model, Schema};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::kit::{self, offset_of, Cam2d, Look, Twist};
use crate::shared::sim::PingPong;
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    spec_held("resolution_x", 24.0, 256.0, 128.0, 0.1),
    spec_held("resolution_y", 24.0, 256.0, 96.0, 0.1),
    spec("cooling", 0.0, 1.0, 0.45),
    spec("cooling_noise", 0.0, 1.0, 0.6),
    spec("wind", -1.0, 1.0, 0.1),
    spec("wind_speed", 0.0, 1.0, 0.3),
    spec("turbulence", 0.0, 1.0, 0.3),
    spec("ignition_rhythm", 0.0, 8.0, 1.0),
    spec("ignition_width", 0.05, 1.0, 0.8),
    spec_held("ignition_shape", 0.0, 3.0, 0.0, 0.1),
    spec_held("source_mode", 0.0, 2.0, 1.0, 0.1),
    spec("emitters_n", 1.0, 12.0, 4.0),
    spec("flame_height", 0.2, 2.0, 1.0),
    spec("rise", -2.0, 2.0, 1.0),
    spec("pixel", 0.0, 1.0, 0.15),
    spec("palette_pos", -0.5, 0.5, 0.0),
    spec("palette_width", 0.3, 3.0, 1.2),
    spec("palette_gamma", 0.3, 3.0, 1.0),
    spec("embers", 0.0, 1.0, 0.3),
    spec("smoke", 0.0, 1.0, 0.25),
    spec("distortion_heat", 0.0, 1.0, 0.2),
];

/// Linear values: black, deep red, orange, near-white.
const FUEGO: [f32; 12] = [
    0.0, 0.0, 0.0, //
    0.55, 0.03, 0.005, //
    1.0, 0.32, 0.02, //
    1.0, 0.92, 0.65,
];

const BLOCKS: &[Block] = &[
    Block::Floats(FLOATS),
    kit::CAM2D_BLOCK,
    kit::LOOK_BLOCK,
    kit::TWIST_BLOCK,
    color_at("fire", &FUEGO),
    background(),
    crate::model::SIGNAL_FLOATS_BLOCK,
    crate::model::EFFECTS_FLOATS_BLOCK,
    crate::model::EFFECTS_COLOR_BLOCK,
];

pub static SCHEMA: Schema = Schema { blocks: BLOCKS };

const I_CAM: usize = 1;
const I_LOOK: usize = 2;
const I_TWIST: usize = 3;
const I_FUEGO: usize = 4;
const I_FONDO: usize = 5;

/// Simulation steps per second at `|subida|` = 1.
pub const STEP_RATE: f32 = 30.0;
const MAX_STEPS_PER_FRAME: u32 = 8;
/// A jump past this (either way) reseeds instead of stepping.
const SEEK_GAP: f32 = 30.0;

/// Mean heat lost per step. One row is climbed per step, so a flame reaches
/// about `1 / decay_mean` rows: `cooling` 0 lets it overshoot the frame
/// twice, 1 holds it to a quarter, and `flame_height` stretches it.
pub fn decay_mean(cooling: f32, flame_height: f32, rows: f32) -> f32 {
    (0.5 + 3.5 * cooling.clamp(0.0, 1.0)) / (flame_height.max(0.05) * rows.max(1.0))
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct StepUniform {
    s: [u32; 4],
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DisplayUniform {
    fire: ColorSlotGpu,
    background: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    e: [f32; 4],
    f: [f32; 4],
    g: [f32; 4],
}

fn display_wgsl() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| format!("{}\n{}", kit::WGSL, include_str!("../../shaders/brasa.wgsl")))
}

pub struct Brasa {
    step: Fullscreen,
    display: Fullscreen,
    grid: Option<PingPong>,
    last_time: Option<f32>,
    carry: f32,
    steps: u32,
}

impl Brasa {
    pub fn new() -> Brasa {
        Brasa {
            step: Fullscreen::with_textures("brasa step", include_str!("../../shaders/brasa_step.wgsl"), 1),
            display: Fullscreen::with_textures("ember", display_wgsl(), 1),
            grid: None,
            last_time: None,
            carry: 0.0,
            steps: 0,
        }
    }
}

fn step_uniform(f: &[f32], index: u32, fresh: bool, source_time: f32, w: u32, h: u32) -> StepUniform {
    StepUniform {
        s: [index, fresh as u32, (f[13] >= 0.0) as u32, f[10].round().clamp(0.0, 2.0) as u32],
        a: [w as f32, h as f32, decay_mean(f[2], f[12], h as f32), f[3]],
        b: [f[4], f[5], f[6], source_time],
        c: [f[7], f[8], f[9].round(), f[11]],
        d: [f[18], f[19], 0.0, 0.0],
    }
}

impl Model for Brasa {
    fn id(&self) -> &'static str {
        "ember"
    }

    fn name(&self) -> &'static str {
        "Brasa"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn render(&mut self, ctx: &Ctx, f: &[f32], target: &wgpu::TextureView, format: wgpu::TextureFormat) {
        let w = f[0].round().clamp(24.0, 256.0) as u32;
        let h = f[1].round().clamp(24.0, 256.0) as u32;
        let rate = STEP_RATE * f[13].abs().min(2.0);

        let resized = self.grid.as_ref().map_or(true, |g| g.width != w || g.height != h);
        let sought = self
            .last_time
            .map_or(false, |l| ctx.time < l - 0.5 || ctx.time > l + SEEK_GAP);
        if resized {
            self.grid = Some(PingPong::new(ctx.device, w, h));
        }
        if resized || sought || self.last_time.is_none() {
            // Seed: clear, then replay `h + 16` steps ending at `ctx.time`.
            let grid = self.grid.as_mut().expect("grid built above");
            grid.reset();
            let u = step_uniform(f, 0, true, ctx.time, w, h);
            grid.step(ctx, &mut self.step, &u);
            let burn = h + 16;
            let dt = 1.0 / STEP_RATE;
            for i in 0..burn {
                let t = ctx.time - (burn - 1 - i) as f32 * dt;
                let u = step_uniform(f, i, false, t, w, h);
                grid.step(ctx, &mut self.step, &u);
            }
            self.steps = burn;
            self.carry = 0.0;
        } else if let Some(last) = self.last_time {
            let dt = ctx.time - last;
            if dt > 0.0 && rate > 0.5 {
                let owed = self.carry + dt * rate;
                let n = (owed.floor() as u32).min(MAX_STEPS_PER_FRAME);
                self.carry = if n as f32 >= owed.floor() { owed - owed.floor() } else { 0.0 };
                let grid = self.grid.as_mut().expect("grid exists");
                for i in 0..n {
                    let t = ctx.time - (n - 1 - i) as f32 / rate;
                    let u = step_uniform(f, self.steps, false, t, w, h);
                    grid.step(ctx, &mut self.step, &u);
                    self.steps = self.steps.wrapping_add(1);
                }
            }
        }
        self.last_time = Some(match self.last_time {
            Some(l) if ctx.time < l && ctx.time >= l - 0.5 => l,
            _ => ctx.time,
        });

        let cam = Cam2d::from_floats(&f[offset_of(BLOCKS, I_CAM)..][..Cam2d::N]);
        let look = Look::from_floats(&f[offset_of(BLOCKS, I_LOOK)..][..Look::N]);
        let tw = Twist::from_floats(&f[offset_of(BLOCKS, I_TWIST)..][..Twist::N]);
        let fire = offset_of(BLOCKS, I_FUEGO);
        let fon = offset_of(BLOCKS, I_FONDO);
        let env = if f[7] > 0.0 { (-3.0 * (ctx.time * f[7]).rem_euclid(1.0)).exp() } else { 1.0 };
        let u = DisplayUniform {
            fire: ColorSlotGpu::from_floats(&f[fire..fire + 12]),
            background: ColorSlotGpu::from_floats(&f[fon..fon + 12]),
            a: [ctx.aspect(), ctx.time, w as f32, h as f32],
            b: [f[14], f[15], f[16], f[17]],
            c: [f[19], f[20], f[18], env],
            d: [cam.zoom, cam.pan_x, cam.pan_y, cam.turn],
            e: [cam.drift, look.shine, look.contrast, look.opacity],
            f: [look.background_level, look.tone, look.saturation, tw.warp_amount],
            g: [tw.warp_freq, tw.mirror, tw.pulse, 0.0],
        };
        let (view, sampler) = self.grid.as_ref().expect("grid exists").current();
        self.display.draw_textured(ctx, target, format, &u, &[(view, sampler)]);
    }
}

// ---------------------------------------------------------------------------
// CPU reference of the decay rule, for the tests.
// ---------------------------------------------------------------------------

/// Same integer hash as `bh` in brasa_step.wgsl.
#[cfg(test)]
fn bh(x: i32, y: i32, n: u32, salt: u32) -> f32 {
    let mut h = (x as u32)
        .wrapping_mul(747796405)
        .wrapping_add((y as u32).wrapping_mul(2891336453))
        .wrapping_add(n.wrapping_mul(277803737))
        .wrapping_add(salt.wrapping_mul(1664525))
        .wrapping_add(1013904223);
    h = ((h >> ((h >> 28) + 4)) ^ h).wrapping_mul(277803737);
    h = (h >> 22) ^ h;
    (h & 0xff_ffff) as f32 / 16_777_216.0
}

/// One column of the automaton without wind: the source row at heat 1, every
/// row above it the row below minus a noisy decay. Returns the number of rows
/// still hotter than `thr` after `steps` steps.
#[cfg(test)]
fn column_height(rows: usize, decay: f32, noise: f32, steps: u32, thr: f32) -> usize {
    let mut col = vec![0.0f32; rows];
    col[0] = 1.0;
    for n in 0..steps {
        let prev = col.clone();
        for r in 1..rows {
            let d = decay * (1.0 - noise + noise * 2.0 * bh(0, r as i32, n, 3));
            col[r] = (prev[r - 1] - d).max(0.0);
        }
    }
    col.iter().filter(|&&v| v > thr).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{offscreen_target, Gpu};

    const FMT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
    const S: u32 = 48;

    fn offset(name: &str) -> usize {
        let mut o = 0;
        for b in BLOCKS {
            if let Block::Floats(fs) = b {
                if let Some(i) = fs.iter().position(|s| s.name == name) {
                    return o + i;
                }
            }
            o += b.width();
        }
        panic!("no float {name}");
    }

    /// Renders a sequence of times on one instance; returns the last frame.
    fn frames(gpu: &Gpu, f: &[f32], times: &[f32]) -> Vec<u8> {
        let mut m = Brasa::new();
        let mut last = Vec::new();
        for &t in times {
            let (tex, view) = offscreen_target(&gpu.device, S, S, FMT);
            let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: t, width: S, height: S, fondo_alpha: false, detail: 1.0 };
            m.render(&ctx, f, &view, FMT);
            last = read(gpu, &tex);
        }
        last
    }

    fn read(gpu: &Gpu, tex: &wgpu::Texture) -> Vec<u8> {
        let bpr = (S * 8).next_multiple_of(256);
        let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (bpr * S) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(S) },
            },
            wgpu::Extent3d { width: S, height: S, depth_or_array_layers: 1 },
        );
        gpu.queue.submit(Some(enc.finish()));
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
        rx.recv().unwrap().unwrap();
        let data = slice.get_mapped_range().unwrap();
        let mut out = Vec::new();
        for r in 0..S {
            let s = (r * bpr) as usize;
            out.extend_from_slice(&data[s..s + (S * 8) as usize]);
        }
        out
    }

    fn mean_luma(px: &[u8]) -> f32 {
        let v: Vec<f32> = px.chunks_exact(2).map(|c| half::f16::from_le_bytes([c[0], c[1]]).to_f32()).collect();
        v.iter().sum::<f32>() / v.len() as f32
    }

    #[test]
    fn schema_reads_the_right_offsets() {
        let d = SCHEMA.defaults();
        assert_eq!(d.len(), SCHEMA.width());
        assert_eq!(d[offset("zoom")], 1.0);
        assert_eq!(d[offset("shine")], 1.0);
    }

    #[test]
    fn heat_height_scales_with_enfriamiento() {
        let rows = 200;
        let h = |e: f32| column_height(rows, decay_mean(e, 1.0, rows as f32), 0.5, 400, 0.02);
        let (a, b, c) = (h(0.1), h(0.5), h(0.9));
        assert!(a > b && b > c, "colder air must give shorter flames: {a} {b} {c}");
        // Without noise the column is exactly 1 / decay rows tall.
        let e = 0.5;
        let want = 1.0 / decay_mean(e, 1.0, rows as f32);
        let got = column_height(rows, decay_mean(e, 1.0, rows as f32), 0.0, 400, 0.0) as f32;
        assert!((got - want).abs() <= 2.0, "height {got} vs 1/decay {want}");
        // llama_alt stretches the same rule.
        assert!(column_height(rows, decay_mean(0.5, 1.5, rows as f32), 0.5, 400, 0.02) > b);
    }

    #[test]
    fn every_float_changes_the_frame() {
        let gpu = Gpu::new_headless();
        let base = SCHEMA.defaults();
        let times = [0.0, 0.37];
        let reference = frames(&gpu, &base, &times);
        assert!(mean_luma(&reference) > 0.0, "defaults are flat");
        assert_eq!(reference, frames(&gpu, &base, &times), "not deterministic");

        let mut dead = Vec::new();
        let names: Vec<&str> =
            BLOCKS.iter().filter_map(|b| if let Block::Floats(fs) = b { Some(*fs) } else { None }).take(4).flat_map(|fs| fs.iter().map(|s| s.name)).collect();
        for name in names {
            let o = offset(name);
            let spec = BLOCKS.iter().filter_map(|b| if let Block::Floats(fs) = b { Some(*fs) } else { None }).flat_map(|fs| fs.iter()).find(|s| s.name == name).unwrap();
            let mut row = base.clone();
            // Dependent floats need their mode on.
            let mut base_row = base.clone();
            match name {
                "emitters_n" => { base_row[offset("source_mode")] = 2.0; row[offset("source_mode")] = 2.0; }
                "ignition_shape" | "ignition_width" => { base_row[offset("source_mode")] = 0.0; row[offset("source_mode")] = 0.0; }
                "background_level" => {
                    // The default background is black, which a level cannot change.
                    let at = offset_of(BLOCKS, I_FONDO);
                    for r in [&mut base_row, &mut row] {
                        r[at..at + 12].fill(0.4);
                    }
                }
                "warp_freq" => { base_row[offset("warp_amount")] = 0.8; row[offset("warp_amount")] = 0.8; }
                _ => {}
            }
            let far = if (spec.max - spec.default).abs() > (spec.default - spec.min).abs() { spec.max } else { spec.min };
            row[o] = match name {
                "mirror" => 5.0,
                "ignition_shape" => 2.0,
                "pulse" => 1.0,
                _ => far,
            };
            // Documented exception: the magnitude of subida changes only the
            // rate, so it is compared across elapsed time (below).
            if name == "rise" { row[o] = -1.0; }
            let want = frames(&gpu, &base_row, &times);
            let got = frames(&gpu, &row, &times);
            if want == got {
                dead.push(name);
            }
        }
        assert!(dead.is_empty(), "floats that do not change the frame: {dead:?}");
    }

    #[test]
    fn subida_magnitude_changes_the_dynamics_not_the_first_frame() {
        let gpu = Gpu::new_headless();
        let mut slow = SCHEMA.defaults();
        slow[offset("rise")] = 0.3;
        let times = [0.0, 0.5, 1.0];
        assert_ne!(frames(&gpu, &SCHEMA.defaults(), &times), frames(&gpu, &slow, &times));
    }

    #[test]
    fn holds_still_when_time_does_not_advance() {
        let gpu = Gpu::new_headless();
        let f = SCHEMA.defaults();
        assert_eq!(frames(&gpu, &f, &[1.0, 1.0, 1.0]), frames(&gpu, &f, &[1.0]));
    }
}
