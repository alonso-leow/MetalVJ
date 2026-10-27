//! Traffic — the Biham–Middleton–Levine traffic automaton.
//!
//! Red cars drive right on even steps, blue cars drive down on odd ones, on a
//! torus. Below a density of about 0.35 everything flows; above it the grid
//! locks into diagonal jams. `densidad` swept across that threshold is the
//! model's beat target. Written from the algorithm; no code was taken from any
//! implementation.
//!
//! **State.** The grid lives in a [`PingPong`] pair (`shaders/trafico_step.wgsl`
//! advances it, `shaders/trafico.wgsl` draws it). Like Turing it is a pure
//! function of (floats, time) in the sense that matters: the grid advances when
//! time advances, the same instant twice steps nothing, and a fresh instance
//! always starts from the same seed. It reseeds on `semilla`, on `res` / frame
//! shape change, on a rising edge of `reinicio_ritmo`, and on a rewind or seek.
//!
//! **Density is live.** Every cell carries a fixed random *rank*; it holds a
//! car iff `rank < densidad`, and a move swaps the two cells' ranks. So
//! `densidad`, `p_rojo` and `tercer_tipo` change the population without a
//! reseed and without any neighbourhood estimate (see the step shader).
//!
//! The CPU functions at the bottom ([`Grid`]) are the same rules in plain Rust;
//! they exist so the jam transition can be measured and asserted.

use crate::model::{
    color_at, background, spec, spec_held, Block, Ctx, FloatSpec, Model, Schema, SurfaceSpec,
};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::kit::{self, CAM2D_BLOCK, LOOK_BLOCK, TWIST_BLOCK};
use crate::shared::sim::PingPong;
use crate::shared::surface::{MaterialU, PartSet};
use crate::shared::ColorSlotGpu;

/// The one part a material can be laid on: the road surface and its cars.
const SURFACES: &[&str] = &["body"];

const FLOATS: &[FloatSpec] = &[
    // The phase-transition knob: ~0.35 is the jam threshold. Live (no reseed).
    spec("density", 0.02, 0.95, 0.30),
    // Red share of the cars (the rest are blue). Live.
    spec("p_red", 0.0, 1.0, 0.5),
    // Grid cells over the frame height. Structural: changing it rebuilds and
    // reseeds the grid.
    spec_held("resolution", 16.0, 192.0, 64.0, 0.0),
    // Extra steps per second pushed by a beat-driven signal.
    spec("step_rhythm", 0.0, 1.0, 0.0),
    // Steps per second.
    spec("step_speed", 0.0, 60.0, 10.0),
    // Probability that a car idles on a turn it would have moved (variety).
    spec("noise_turn", 0.0, 1.0, 0.0),
    // Share of diagonal cars (a third species; adds a third phase). Live.
    spec("third_type", 0.0, 1.0, 0.0),
    // Pinned: a reseed throws the whole grid away.
    spec_held("seed", 0.0, 1.0, 0.0, 0.0),
    // A rising edge through 0.5 reseeds: a beat-driven wipe.
    spec("reset_rhythm", 0.0, 1.0, 0.0),
    // Cell shape: 0 box, 1 disc, 2 diamond, 3 bar along travel. Structural.
    spec_held("shape", 0.0, 3.0, 0.0, 0.2),
    spec("cell_gap", 0.0, 0.6, 0.12),
    spec("cell_roundness", 0.0, 1.0, 0.25),
    spec("shine_red", 0.0, 3.0, 1.0),
    spec("shine_blue", 0.0, 3.0, 1.0),
    spec("glow", 0.0, 1.0, 0.0),
    // Fading trail left behind each car.
    spec("wake", 0.0, 1.0, 0.0),
    // Floor tilt: makes the grid a receding 3D road.
    spec("perspective", 0.0, 1.0, 0.0),
];

/// Not white: the three roles must be told apart by colour.
const RED: [f32; 12] = [0.55, 0.04, 0.03, 0.85, 0.10, 0.06, 0.95, 0.22, 0.10, 1.0, 0.45, 0.20];
const BLUE: [f32; 12] = [0.03, 0.10, 0.55, 0.05, 0.25, 0.85, 0.10, 0.45, 0.95, 0.25, 0.70, 1.0];
const EMPTY: [f32; 12] = [0.012, 0.014, 0.022, 0.016, 0.018, 0.030, 0.012, 0.014, 0.022, 0.020, 0.022, 0.035];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("red", &RED),
        color_at("blue", &BLUE),
        color_at("empty", &EMPTY),
        background(),
        CAM2D_BLOCK,
        LOOK_BLOCK,
        TWIST_BLOCK,
        crate::model::SIGNAL_FLOATS_BLOCK,
        crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

// Block indices in SCHEMA.blocks, so offsets come from the block widths.
const B_OWN: usize = 0;
const B_RED: usize = 1;
const B_BLUE: usize = 2;
const B_EMPTY: usize = 3;
const B_BG: usize = 4;
const B_CAM: usize = 5;
const B_LOOK: usize = 6;
const B_TWIST: usize = 7;

fn off(block: usize) -> usize {
    kit::offset_of(SCHEMA.blocks, block)
}

// Own float indices.
const F_DENSIDAD: usize = 0;
const F_P_ROJO: usize = 1;
const F_RES: usize = 2;
const F_PASO_RITMO: usize = 3;
const F_PASO_VEL: usize = 4;
const F_RUIDO: usize = 5;
const F_THIRD: usize = 6;
const F_SEMILLA: usize = 7;
const F_REINICIO: usize = 8;
const F_FORMA: usize = 9;
const F_GAP: usize = 10;
const F_REDONDEZ: usize = 11;
const F_BRILLO_ROJO: usize = 12;
const F_BRILLO_AZUL: usize = 13;
const F_GLOW: usize = 14;
const F_ESTELA: usize = 15;
const F_PERSPECTIVA: usize = 16;

/// Steps run on a (re)seed so the first frame already has structure.
const BURN_IN: u32 = 24;
const MAX_STEPS_PER_FRAME: u32 = 16;
/// Extra steps per second at `paso_ritmo` = 1.
const RITMO_STEPS: f32 = 60.0;
/// A forward jump larger than this is a seek and reseeds.
const SEEK_GAP: f32 = 60.0;
const MAX_W: u32 = 512;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct StepUniform {
    mode: u32,
    step: u32,
    w: u32,
    h: u32,
    seed: u32,
    nph: u32,
    pad: [u32; 2],
    a: [f32; 4],
    b: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DisplayUniform {
    red: ColorSlotGpu,
    blue: ColorSlotGpu,
    empty: ColorSlotGpu,
    background: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    e: [f32; 4],
    f: [f32; 4],
    g: [f32; 4],
    h: [f32; 4],
    /// `body`'s picture and `mat_*`, see `material.wgsl`.
    mat_body: MaterialU,
}

fn shader_src() -> &'static str {
    static SRC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SRC.get_or_init(|| {
        format!(
            "{}\n{}\n{}\n{}",
            include_str!("../../shaders/surface.wgsl"),
            include_str!("../../shaders/material.wgsl"),
            kit::WGSL,
            include_str!("../../shaders/trafico.wgsl"),
        )
    })
}

/// Grid size for a frame: `res` cells tall, as many wide as the aspect needs.
pub fn grid_size(res: f32, aspect: f32) -> (u32, u32) {
    let h = res.round().clamp(16.0, 192.0) as u32;
    let w = ((h as f32) * aspect.max(0.1)).round().clamp(16.0, MAX_W as f32) as u32;
    (w, h)
}

fn trail_decay(wake: f32) -> f32 {
    0.35 + 0.6 * wake.clamp(0.0, 1.0)
}

#[derive(Default)]
struct Sim {
    wall: Option<f32>,
    carry: f32,
    steps_done: u32,
    /// `(w, h, semilla key, reinicio count)` of the live grid.
    key: (u32, u32, u32, u32),
    reinicio_high: bool,
    reinicios: u32,
}

pub struct Trafico {
    step: Fullscreen,
    display: Fullscreen,
    grid: Option<PingPong>,
    sim: Sim,
    surfaces: PartSet<1>,
}

impl Trafico {
    pub fn new() -> Trafico {
        Trafico {
            step: Fullscreen::with_textures("trafico step", include_str!("../../shaders/trafico_step.wgsl"), 1),
            // Two pictures: the grid state, then the `body` surface.
            display: Fullscreen::with_textures("traffic", shader_src(), 1 + SURFACES.len()),
            grid: None,
            sim: Sim::default(),
            surfaces: PartSet::new(SURFACES),
        }
    }

    /// Whether to reseed and how many steps to advance (fractional rate is
    /// carried, a stall arrives as a slowdown, dt = 0 is zero steps).
    fn schedule(&mut self, time: f32, rate: f32, key: (u32, u32, u32, u32)) -> (bool, u32) {
        let reseed = match self.sim.wall {
            None => true,
            Some(prev) => {
                let dt = time - prev;
                key != self.sim.key || !(0.0..=SEEK_GAP).contains(&dt)
            }
        };
        let dt = self.sim.wall.map_or(0.0, |prev| (time - prev).clamp(0.0, 0.1));
        self.sim.wall = Some(time);
        self.sim.key = key;
        if reseed {
            self.sim.carry = 0.0;
            return (true, BURN_IN);
        }
        self.sim.carry += rate * dt;
        let whole = self.sim.carry.floor();
        self.sim.carry -= whole;
        (false, (whole.max(0.0) as u32).min(MAX_STEPS_PER_FRAME))
    }
}

impl Default for Trafico {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Trafico {
    fn id(&self) -> &'static str {
        "traffic"
    }

    fn name(&self) -> &'static str {
        "Traffic"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn is_3d(&self) -> bool {
        true
    }

    fn surfaces(&self) -> &'static [&'static str] {
        self.surfaces.surfaces()
    }

    fn set_surface(&mut self, part: &str, spec: Option<SurfaceSpec>) {
        self.surfaces.set(part, spec);
    }

    fn render(
        &mut self,
        ctx: &Ctx,
        floats: &[f32],
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        let own = &floats[off(B_OWN)..off(B_OWN) + FLOATS.len()];
        let aspect = ctx.aspect();
        let (w, h) = grid_size(own[F_RES], aspect);

        let high = own[F_REINICIO] > 0.5;
        if high && !self.sim.reinicio_high {
            self.sim.reinicios = self.sim.reinicios.wrapping_add(1);
        }
        self.sim.reinicio_high = high;
        let seed_key = (own[F_SEMILLA].clamp(0.0, 1.0) * 1000.0).round() as u32;
        let key = (w, h, seed_key, self.sim.reinicios);

        let rate = own[F_PASO_VEL].max(0.0) + own[F_PASO_RITMO].clamp(0.0, 1.0) * RITMO_STEPS;
        let (reseed, steps) = self.schedule(ctx.time, rate, key);

        let third = own[F_THIRD].clamp(0.0, 1.0);
        let nph = if third > 0.001 { 3 } else { 2 };
        let a = [
            own[F_DENSIDAD].clamp(0.0, 1.0),
            own[F_P_ROJO].clamp(0.0, 1.0),
            own[F_RUIDO].clamp(0.0, 1.0),
            third,
        ];
        let b = [trail_decay(own[F_ESTELA]), 0.0, 0.0, 0.0];
        let salt = seed_key.wrapping_mul(0x9e37_79b1).wrapping_add(self.sim.reinicios.wrapping_mul(0x85eb_ca6b));
        let su = |mode: u32, step: u32| StepUniform { mode, step, w, h, seed: salt, nph, pad: [0; 2], a, b };

        if self.grid.as_ref().map_or(true, |g| (g.width, g.height) != (w, h)) {
            self.grid = Some(PingPong::new(ctx.device, w, h));
        }
        let grid = self.grid.as_mut().expect("grid built above");
        if reseed {
            grid.reset();
            grid.step(ctx, &mut self.step, &su(0, 0));
            self.sim.steps_done = 0;
        }
        let todo = if reseed { BURN_IN } else { steps };
        for _ in 0..todo {
            grid.step(ctx, &mut self.step, &su(2, self.sim.steps_done));
            self.sim.steps_done = self.sim.steps_done.wrapping_add(1);
        }

        let cam = kit::Cam2d::from_floats(&floats[off(B_CAM)..off(B_CAM) + kit::Cam2d::N]);
        let look = kit::Look::from_floats(&floats[off(B_LOOK)..off(B_LOOK) + kit::Look::N]);
        let tw = kit::Twist::from_floats(&floats[off(B_TWIST)..off(B_TWIST) + kit::Twist::N]);
        let slot = |blk: usize| ColorSlotGpu::from_floats(&floats[off(blk)..off(blk) + 12]);
        self.surfaces.sync(ctx.device, ctx.queue);
        let uniform = DisplayUniform {
            red: slot(B_RED),
            blue: slot(B_BLUE),
            empty: slot(B_EMPTY),
            background: slot(B_BG),
            a: [aspect, ctx.time, w as f32, h as f32],
            b: [a[0], a[1], a[3], own[F_ESTELA]],
            c: [own[F_FORMA], own[F_GAP], own[F_REDONDEZ], own[F_GLOW]],
            d: [
                own[F_BRILLO_ROJO],
                own[F_BRILLO_AZUL],
                own[F_PERSPECTIVA],
                if ctx.fondo_alpha { 1.0 } else { 0.0 },
            ],
            e: [cam.zoom, cam.pan_x, cam.pan_y, cam.turn],
            f: [cam.drift, look.shine, look.contrast, look.opacity],
            g: [look.background_level, look.tone, look.saturation, 0.0],
            h: [tw.warp_amount, tw.warp_freq, tw.mirror, tw.pulse],
            mat_body: self.surfaces.uniform(0),
        };
        let (view, sampler) = grid.current();
        let pairs = [
            (view, sampler),
            (self.surfaces.slot(0).view(), self.surfaces.slot(0).sampler()),
        ];
        self.display.draw_textured(ctx, target, format, &uniform, &pairs);
    }
}

// ---------------------------------------------------------------------------
// CPU reference
// ---------------------------------------------------------------------------

/// The same rules as the step shader, in plain Rust (red/blue only, no noise).
pub struct Grid {
    pub w: usize,
    pub h: usize,
    /// 0 empty, 1 red, 2 blue.
    pub cell: Vec<u8>,
}

impl Grid {
    /// A deterministic random grid at `density`, `p_red` of the cars red.
    pub fn seeded(w: usize, h: usize, density: f32, p_red: f32, seed: u64) -> Grid {
        let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 40) as f32 / (1u64 << 24) as f32
        };
        let cell = (0..w * h)
            .map(|_| {
                if next() < density {
                    if next() < p_red { 1 } else { 2 }
                } else {
                    0
                }
            })
            .collect();
        Grid { w, h, cell }
    }

    fn at(&self, x: usize, y: usize) -> u8 {
        self.cell[(y % self.h) * self.w + (x % self.w)]
    }

    /// One tick: even steps move red right, odd steps move blue down.
    /// Returns `(moved, movers)` for the moving species.
    pub fn step(&mut self, n: u32) -> (usize, usize) {
        let (mover, dx, dy) = if n % 2 == 0 { (1u8, 1, 0) } else { (2u8, 0, 1) };
        let old = self.cell.clone();
        let (mut moved, mut movers) = (0, 0);
        for y in 0..self.h {
            for x in 0..self.w {
                if old[y * self.w + x] != mover {
                    continue;
                }
                movers += 1;
                let (nx, ny) = ((x + dx) % self.w, (y + dy) % self.h);
                if old[ny * self.w + nx] == 0 {
                    self.cell[y * self.w + x] = 0;
                    self.cell[ny * self.w + nx] = mover;
                    moved += 1;
                }
            }
        }
        (moved, movers)
    }

    pub fn cars(&self) -> usize {
        self.cell.iter().filter(|&&c| c != 0).count()
    }
}

/// Mean fraction of cars that move per step, averaged over the last `tail`
/// steps of a `steps`-step run from a random grid: ~1 in free flow, ~0 jammed.
pub fn mean_velocity(density: f32, size: usize, steps: u32, tail: u32) -> f32 {
    let mut g = Grid::seeded(size, size, density, 0.5, 7);
    let (mut sum, mut n) = (0.0f32, 0u32);
    for i in 0..steps {
        let (m, t) = g.step(i);
        if i >= steps - tail && t > 0 {
            sum += m as f32 / t as f32;
            n += 1;
        }
    }
    sum / n.max(1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
    const W: u32 = 64;

    fn frames(gpu: &crate::gpu::Gpu, model: &mut Trafico, floats: &[f32], times: &[f32]) -> Vec<u8> {
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, W, W, FORMAT);
        let mut out = vec![];
        for &t in times {
            let ctx = Ctx {
                device: &gpu.device,
                queue: &gpu.queue,
                time: t,
                width: W,
                height: W,
                fondo_alpha: false,
            };
            model.render(&ctx, floats, &view, FORMAT);
            out = crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, W, W);
        }
        out
    }

    const TIMES: [f32; 4] = [0.0, 0.1, 0.2, 0.3];

    fn base() -> Vec<f32> {
        let mut f = SCHEMA.defaults();
        // Make the structural-only floats observable at their baseline.
        f[off(B_TWIST)] = 0.5; // warp_cant, so warp_freq has something to scale
        // fondo defaults to black, which fondo_nivel cannot scale: give it a colour.
        for v in &mut f[off(B_BG)..off(B_BG) + 12] {
            *v = 0.3;
        }
        f
    }

    #[test]
    fn widths_come_from_the_blocks() {
        assert_eq!(SCHEMA.defaults().len(), SCHEMA.width());
        assert_eq!(off(B_OWN), 0);
        assert_eq!(off(B_CAM) + kit::Cam2d::N, off(B_LOOK));
        assert_eq!(off(B_LOOK) + kit::Look::N, off(B_TWIST));
        assert_eq!(off(B_TWIST) + kit::Twist::N, off(B_TWIST + 1));
    }

    #[test]
    fn every_float_moves_the_frame() {
        let gpu = crate::gpu::Gpu::new_headless();
        let own_end = FLOATS.len();
        let (cam, look, twist) = (off(B_CAM), off(B_LOOK), off(B_TWIST));
        let mut idx: Vec<(usize, &'static str)> =
            FLOATS.iter().enumerate().map(|(i, s)| (i, s.name)).collect();
        for (start, kit_specs) in [(cam, kit::CAM2D), (look, kit::LOOK), (twist, kit::TWIST)] {
            idx.extend(kit_specs.iter().enumerate().map(|(i, s)| (start + i, s.name)));
        }
        assert!(idx.len() > own_end);
        let b = base();
        let reference = frames(&gpu, &mut Trafico::new(), &b, &TIMES);
        for (i, name) in idx {
            let spec = if i < own_end {
                FLOATS[i]
            } else if i >= twist {
                kit::TWIST[i - twist]
            } else if i >= look {
                kit::LOOK[i - look]
            } else {
                kit::CAM2D[i - cam]
            };
            let far = if (spec.max - b[i]).abs() > (b[i] - spec.min).abs() { spec.max } else { spec.min };
            // A mirror fold is a no-op below 2 segments; the diagonal-car and
            // reseed floats need the extreme to matter.
            // `tono` is a full hue turn over 0..1, so its far end equals its start.
            let far = if name == "tone" { 0.5 } else { far };
            let mut f = b.clone();
            f[i] = far;
            let got = frames(&gpu, &mut Trafico::new(), &f, &TIMES);
            assert!(reference != got, "float `{name}` ({} -> {far}) did not move the frame", b[i]);
        }
    }

    #[test]
    fn the_defaults_are_not_flat_and_hold_still_without_time() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let mut m = Trafico::new();
        let a = frames(&gpu, &mut m, &f, &[1.0]);
        let again = frames(&gpu, &mut m, &f, &[1.0]);
        assert_eq!(a, again, "the same instant twice must not step the grid");
        let mut distinct = std::collections::HashSet::new();
        for px in a.chunks_exact(4) {
            distinct.insert([px[0], px[1], px[2]]);
        }
        assert!(distinct.len() > 8, "default frame is flat ({} colours)", distinct.len());
        let later = frames(&gpu, &mut m, &f, &[1.1]);
        assert_ne!(a, later, "a tenth of a second later the cars must have moved");
    }

    #[test]
    fn a_fresh_instance_reproduces_and_a_resize_reseeds_safely() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let a = frames(&gpu, &mut Trafico::new(), &f, &TIMES);
        let b = frames(&gpu, &mut Trafico::new(), &f, &TIMES);
        assert_eq!(a, b);
        let mut m = Trafico::new();
        frames(&gpu, &mut m, &f, &TIMES);
        let mut big = f.clone();
        big[F_RES] = 120.0;
        let r = frames(&gpu, &mut m, &big, &[0.4]);
        assert!(r.chunks_exact(4).any(|p| p[0] != p[1]), "resized grid drew nothing");
        let rewound = frames(&gpu, &mut m, &f, &[0.0]);
        assert!(!rewound.is_empty());
    }

    #[test]
    fn the_jam_transition_separates_free_flow_from_gridlock() {
        let free = mean_velocity(0.2, 64, 300, 40);
        let jam = mean_velocity(0.6, 64, 300, 40);
        assert!(free > 0.9, "density 0.2 should flow, got {free}");
        assert!(jam < 0.2, "density 0.6 should jam, got {jam}");
        assert!(free - jam > 0.6);
    }

    #[test]
    fn the_cpu_step_conserves_cars() {
        let mut g = Grid::seeded(40, 30, 0.4, 0.5, 3);
        let n = g.cars();
        for i in 0..50 {
            g.step(i);
        }
        assert_eq!(n, g.cars());
    }

    /// Every kind is a surface: at `mat_mix` 1 it changes the frame, and
    /// clearing `body` restores the frame byte for byte. The clock is held at
    /// one instant, so the grid is the same for every draw.
    #[test]
    fn a_body_material_changes_the_frame_and_clearing_it_restores_it() {
        use crate::model::{MaterialSpec, SurfaceSpec};
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let mut model = Trafico::new();
        assert_eq!(model.surfaces(), &["body"]);
        let plain = frames(&gpu, &mut model, &f, &[1.0]);
        let kinds = ["matte", "concrete", "wood", "leather", "metal", "glass", "matrix"];
        for (kind, name) in kinds.iter().enumerate() {
            model.set_surface(
                "body",
                Some(SurfaceSpec {
                    image: None,
                    material: Some(MaterialSpec {
                        kind: kind as f32,
                        mix: 1.0,
                        scale: 1.0,
                        ..Default::default()
                    }),
                }),
            );
            let painted = frames(&gpu, &mut model, &f, &[1.0]);
            assert_ne!(plain, painted, "{name} did not change the frame");
            model.set_surface("body", None);
            let restored = frames(&gpu, &mut model, &f, &[1.0]);
            assert_eq!(plain, restored, "clearing {name} did not restore the frame");
        }
    }
}
