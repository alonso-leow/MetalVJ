//! Nubes — the 2D compressible Euler equations drawn as clouds, after
//! Mocz's finite-volume tutorial (finitevolume-python, GPL-3). Written from
//! the equations; no code was taken.
//!
//! Four conserved fields (rho, rho*u, rho*v, E) live in one `Rgba16Float`
//! [`PingPong`] grid. One fullscreen pass is one MUSCL-Hancock step
//! (`shaders/nubes_step.wgsl`): primitive slopes (optionally limited), a
//! half-step predictor, face extrapolation, Rusanov fluxes, a conservative
//! update on a periodic domain. `shaders/nubes.wgsl` draws the state. The
//! initial condition is the Kelvin-Helmholtz one of the source: a dense layer
//! sliding past a light one, plus a small perturbation (here several modes
//! with seeded phases, so the billows are not one sine).
//!
//! **Stability is enforced here and in the shader, not by the ranges.** `cfl`
//! is clamped to [`MAX_CFL`]; the time step is *fixed* by the row (a bound on
//! the signal speed, `smax`), because a global min over the grid does not exist
//! in a fragment pass; the shader clamps density, speed and pressure (and thus
//! the sound speed) to that bound so the bound is true by construction. The
//! shader writes only states rebuilt from clamped primitives, so the field
//! cannot go NaN or grow without bound. The state keeps `E - Eref` (ambient
//! pressure energy removed) to spend float16 precision on the deviations.
//!
//! **State advances by time**, like Ondas: a fractional step rate is carried,
//! the same instant twice steps nothing, and a resize, seek, rewind, seed
//! change or a rising `reinicio` reseeds (initial condition plus a burn-in so
//! the first frame already has billows). The CPU reference ([`Euler`]) at the
//! bottom is the same scheme in Rust, to assert conservation and to check the
//! shader against.

use crate::model::{color_at, spec, spec_held, Block, Ctx, FloatSpec, Model, Schema};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::kit::{self, CAM2D_BLOCK, LOOK_BLOCK, TWIST_BLOCK};
use crate::shared::sim::PingPong;
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    // Ideal-gas gamma. Structural.
    spec_held("gamma", 1.1, 2.0, 1.6667, 0.2),
    // Courant factor; clamped to MAX_CFL in Rust. Structural.
    spec_held("cfl", 0.05, 0.4, 0.35, 0.2),
    // 0 = raw central slopes, 1 = the limited slopes of the source.
    spec("limiter", 0.0, 1.0, 0.35),
    // Grid cells over the frame height. Structural.
    spec_held("resolution", 64.0, 256.0, 128.0, 0.2),
    // Width of the shear layer, in frame heights (initial condition).
    spec("kh_layer_width", 0.004, 0.12, 0.03),
    // Relative speed of the two streams (initial condition).
    spec("kh_shear", 0.0, 1.2, 0.5),
    // Amplitude of the seeded perturbation (initial condition).
    spec("kh_perturb", 0.0, 0.4, 0.12),
    // Seed of the perturbation phases. Structural; a change restarts.
    spec_held("kh_seed", 0.0, 1.0, 0.37, 0.2),
    // Density of the central band and of the rest (initial condition).
    spec("density_high", 0.5, 5.0, 2.0),
    spec("density_low", 0.2, 3.0, 1.0),
    // Vertical temperature stratification of the initial density.
    spec("temp_gradient", -1.0, 1.0, 0.0),
    // Buoyancy: gravity on the density anomaly (sign = direction).
    spec("gravity", -1.0, 1.0, 0.1),
    // A rising edge through 0.5 puffs a burst of heat and cloud.
    spec("inject_rhythm", 0.0, 1.0, 0.0),
    spec("inject_x", 0.0, 1.0, 0.5),
    spec("inject_y", 0.0, 1.0, 0.5),
    spec("inject_force", 0.0, 1.0, 0.6),
    // Momentum diffusion.
    spec("viscosity", 0.0, 1.0, 0.2),
    // 0 density, 1 pressure, 2 vorticity, 3 schlieren. Structural.
    spec_held("mode", 0.0, 3.0, 0.0, 0.2),
    // Named `contraste_nube`: `contraste` belongs to the LOOK kit.
    spec("contrast_cloud", 0.3, 4.0, 1.4),
    // Sensitivity of the density-gradient (schlieren) layer.
    spec("schlieren", 0.0, 3.0, 0.3),
    // Light direction on the screen plane (y up).
    spec("light_x", -1.0, 1.0, -0.6),
    spec("light_y", -1.0, 1.0, 0.7),
    // Cloud self-shadow and relief strength.
    spec("shadow", 0.0, 1.0, 0.7),
    // A rising edge through 0.5 retriggers the initial condition.
    spec("reset", 0.0, 1.0, 0.0),
    // Simulation speed. (Not in the brief; a flow needs a clock.)
    spec("speed", 0.0, 3.0, 1.0),
];

/// Pale blue haze, soft white, white, warm white: thin to dense.
const NUBE: [f32; 12] = [0.36, 0.48, 0.70, 0.62, 0.69, 0.84, 0.80, 0.82, 0.88, 0.90, 0.88, 0.84];
/// Deep sky, top to bottom.
const SKY: [f32; 12] = [0.02, 0.04, 0.10, 0.03, 0.07, 0.17, 0.05, 0.10, 0.22, 0.08, 0.14, 0.28];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("cloud", &NUBE),
        color_at("background", &SKY),
        CAM2D_BLOCK,
        LOOK_BLOCK,
        TWIST_BLOCK,
        crate::model::SIGNAL_FLOATS_BLOCK,
        crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

const B_OWN: usize = 0;
const B_NUBE: usize = 1;
const B_FONDO: usize = 2;
const B_CAM: usize = 3;
const B_LOOK: usize = 4;
const B_TWIST: usize = 5;

fn off(block: usize) -> usize {
    kit::offset_of(SCHEMA.blocks, block)
}

const F_GAMMA: usize = 0;
const F_CFL: usize = 1;
const F_LIMITER: usize = 2;
const F_RES: usize = 3;
const F_CAPA: usize = 4;
const F_CIZALLA: usize = 5;
const F_PERTURB: usize = 6;
const F_SEMILLA: usize = 7;
const F_ALTA: usize = 8;
const F_BAJA: usize = 9;
const F_TEMP: usize = 10;
const F_GRAV: usize = 11;
const F_RITMO: usize = 12;
const F_IX: usize = 13;
const F_IY: usize = 14;
const F_IFUERZA: usize = 15;
const F_VISC: usize = 16;
const F_MODO: usize = 17;
const F_CONTRASTE: usize = 18;
const F_SCHLIEREN: usize = 19;
const F_LUZX: usize = 20;
const F_LUZY: usize = 21;
const F_SOMBRA: usize = 22;
const F_REINICIO: usize = 23;
const F_VEL: usize = 24;

/// The 2D unsplit scheme is stable for cfl <= 0.5; stay under it.
pub const MAX_CFL: f32 = 0.4;
/// Ambient pressure of the initial condition (the source's 2.5).
pub const P0: f32 = 2.5;
const MAX_W: u32 = 512;
const MAX_STEPS_PER_FRAME: u32 = 64;
/// Physical time units per second at `velocidad` 1.
const TIME_RATE: f32 = 0.4;
const SEEK_GAP: f32 = 60.0;
const PUFF_STEPS: u32 = 6;
const RHO_MAX: f32 = 60.0;

fn clean(row: &[f32], i: usize) -> f32 {
    let s = FLOATS[i];
    let v = row.get(i).copied().filter(|v| v.is_finite()).unwrap_or(s.default);
    v.clamp(s.min, s.max)
}

/// Grid size for an aspect ratio and a requested height (rounded to 8 cells so
/// a slow drift of `resolucion` does not restart the flow every frame).
pub fn grid_size(aspect: f32, res: f32) -> (u32, u32) {
    let a = if aspect.is_finite() { aspect } else { 1.0 };
    let h = ((res / 8.0).round() * 8.0).clamp(64.0, 256.0);
    let w = (h * a.max(0.1)).round().clamp(64.0, MAX_W as f32);
    (w as u32, h as u32)
}

/// Everything one step needs, from a float row. Shared by the GPU step and
/// the CPU reference so the two cannot drift.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub w: u32,
    pub h: u32,
    pub gamma: f32,
    /// dt/dx: fixed by the row (`cfl / smax`).
    pub r: f32,
    pub limiter: f32,
    pub visc: f32,
    pub grav_dt: f32,
    pub rho_mean: f32,
    pub e_ref: f32,
    pub rho_floor: f32,
    pub p_floor: f32,
    pub vmax: f32,
    pub cmax: f32,
    pub rho_hi: f32,
    pub rho_lo: f32,
    pub shear: f32,
    pub width: f32,
    pub perturb: f32,
    pub tgrad: f32,
    pub cycles: [f32; 4],
    pub phases: [f32; 4],
    pub amps: [f32; 4],
}

/// The deterministic hash behind the seeded phases (no float trig, so the
/// GPU never has to reproduce it: the phases travel in the uniform).
fn hash01(seed: u32, k: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B1) ^ k.wrapping_mul(0x85EB_CA6B).wrapping_add(0xC2B2_AE35);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297A_2D39);
    x ^= x >> 15;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// The seed as an integer: `kh_semilla` in thousandths.
pub fn seed_of(row: &[f32]) -> u32 {
    (clean(row, F_SEMILLA) * 1000.0).round() as u32
}

impl Params {
    pub fn from_floats(row: &[f32], w: u32, h: u32) -> Params {
        let g = |i| clean(row, i);
        let gamma = g(F_GAMMA);
        let cfl = g(F_CFL).min(MAX_CFL);
        let tg = g(F_TEMP);
        let (hi, lo) = (g(F_ALTA), g(F_BAJA));
        let shear = g(F_CIZALLA);
        let strat = 1.0 - 0.35 * tg.abs();
        let rho_min = hi.min(lo) * strat;
        let rho_max = hi.max(lo) * (1.0 + 0.35 * tg.abs());
        let rho_floor = (0.3 * rho_min).max(0.03);
        let c0 = (gamma * P0 / rho_min).sqrt();
        let cmax = 1.3 * c0;
        let vmax = (0.6 + 1.3 * shear).min(3.0);
        let r = cfl / (cmax + vmax);
        let seed = seed_of(row);
        let aspect = w as f32 / h as f32;
        let mut cycles = [0.0; 4];
        let mut phases = [0.0; 4];
        let mut amps = [0.0; 4];
        for j in 0..4 {
            // Whole cycles across the width, so the domain stays periodic.
            cycles[j] = (aspect * (2.0 + 2.0 * j as f32) + (hash01(seed, 10 + j as u32) - 0.5)).round().max(1.0);
            phases[j] = hash01(seed, j as u32) * std::f32::consts::TAU;
            amps[j] = [1.0, 0.7, 0.5, 0.35][j];
        }
        Params {
            w,
            h,
            gamma,
            r,
            limiter: g(F_LIMITER),
            visc: 0.005 * g(F_VISC),
            grav_dt: g(F_GRAV) * r / h as f32,
            rho_mean: 0.5 * (rho_min + rho_max),
            e_ref: P0 / (gamma - 1.0),
            rho_floor,
            p_floor: 0.05 * P0,
            vmax,
            cmax,
            rho_hi: hi,
            rho_lo: lo,
            shear,
            width: g(F_CAPA),
            perturb: g(F_PERTURB),
            tgrad: tg,
            cycles,
            phases,
            amps,
        }
    }

    /// Density range the display maps to the ramp.
    pub fn rho_range(&self) -> (f32, f32) {
        let k = 0.35 * self.tgrad.abs();
        (self.rho_hi.min(self.rho_lo) * (1.0 - k), self.rho_hi.max(self.rho_lo) * (1.0 + k))
    }

    /// Steps per second for a `velocidad`.
    pub fn steps_per_second(&self, speed: f32) -> f32 {
        TIME_RATE * speed * self.h as f32 / self.r
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct StepUniform {
    dims: [f32; 4],
    lim: [f32; 4],
    cap: [f32; 4],
    cap2: [f32; 4],
    ic0: [f32; 4],
    ic1: [f32; 4],
    cyc: [f32; 4],
    phs: [f32; 4],
    amp: [f32; 4],
    inj: [f32; 4],
}

impl StepUniform {
    /// `init`: write the initial condition instead of stepping. `inj` is
    /// (x cells, y cells, amount this step, radius cells).
    fn new(p: &Params, init: bool, inj: [f32; 4]) -> StepUniform {
        StepUniform {
            dims: [p.w as f32, p.h as f32, p.r, p.gamma],
            lim: [p.limiter, p.visc, p.grav_dt, p.rho_mean],
            cap: [p.e_ref, p.rho_floor, p.p_floor, p.vmax],
            cap2: [p.cmax, if init { 1.0 } else { 0.0 }, P0, 0.0],
            ic0: [p.rho_hi, p.rho_lo, p.shear, p.width],
            ic1: [p.perturb, p.tgrad, 0.0, 0.0],
            cyc: p.cycles,
            phs: p.phases,
            amp: p.amps,
            inj,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DisplayUniform {
    cloud: ColorSlotGpu,
    background: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    e: [f32; 4],
    f: [f32; 4],
    g: [f32; 4],
    h: [f32; 4],
}

fn display_src() -> &'static str {
    static SRC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SRC.get_or_init(|| format!("{}\n{}", kit::WGSL, include_str!("../../shaders/nubes.wgsl")))
}

#[derive(Default)]
struct Sim {
    wall: Option<f32>,
    carry: f32,
    key: (u32, u32, u32),
    reinicio_high: bool,
    ritmo_high: bool,
    puff_left: u32,
    puff: [f32; 2],
    puff_amount: f32,
}

pub struct Nubes {
    step: Fullscreen,
    display: Fullscreen,
    grid: Option<PingPong>,
    sim: Sim,
}

impl Nubes {
    pub fn new() -> Nubes {
        Nubes {
            step: Fullscreen::with_textures("nubes step", include_str!("../../shaders/nubes_step.wgsl"), 1),
            display: Fullscreen::with_textures("clouds", display_src(), 1),
            grid: None,
            sim: Sim::default(),
        }
    }
}

impl Default for Nubes {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Nubes {
    fn id(&self) -> &'static str {
        "clouds"
    }

    fn name(&self) -> &'static str {
        "Nubes"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn render(&mut self, ctx: &Ctx, floats: &[f32], target: &wgpu::TextureView, format: wgpu::TextureFormat) {
        let own = &floats[off(B_OWN)..off(B_OWN) + FLOATS.len()];
        let aspect = ctx.aspect();
        let (w, h) = grid_size(aspect, clean(own, F_RES));
        let key = (w, h, seed_of(own));
        let time = if ctx.time.is_finite() { ctx.time } else { 0.0 };
        let p = Params::from_floats(own, w, h);

        let high = clean(own, F_REINICIO) > 0.5;
        let restart = high && !self.sim.reinicio_high;
        self.sim.reinicio_high = high;
        let reseed = restart
            || match self.sim.wall {
                None => true,
                Some(prev) => key != self.sim.key || !(0.0..=SEEK_GAP).contains(&(time - prev)),
            };
        let dt = self.sim.wall.map_or(0.0, |prev| (time - prev).clamp(0.0, 0.1));
        self.sim.wall = Some(time);
        self.sim.key = key;

        let beat = clean(own, F_RITMO) > 0.5;
        let edge = beat && !self.sim.ritmo_high;
        self.sim.ritmo_high = beat;

        if self.grid.as_ref().map_or(true, |g| (g.width, g.height) != (w, h)) {
            self.grid = Some(PingPong::new(ctx.device, w, h));
        }
        let grid = self.grid.as_mut().expect("grid built above");

        let steps;
        if reseed {
            self.sim.carry = 0.0;
            self.sim.puff_left = 0;
            grid.reset();
            grid.step(ctx, &mut self.step, &StepUniform::new(&p, true, [0.0; 4]));
            steps = h; // burn-in: the first frame already has billows
        } else {
            self.sim.carry += p.steps_per_second(clean(own, F_VEL)) * dt;
            let whole = self.sim.carry.floor();
            self.sim.carry -= whole;
            steps = (whole.max(0.0) as u32).min(MAX_STEPS_PER_FRAME);
        }
        if edge {
            self.sim.puff_left = PUFF_STEPS;
            self.sim.puff = [clean(own, F_IX) * w as f32, clean(own, F_IY) * h as f32];
            self.sim.puff_amount = clean(own, F_IFUERZA) * PUFF_AMOUNT / PUFF_STEPS as f32;
        }
        let radius = (0.07 * h as f32).max(2.0);
        for _ in 0..steps {
            let inj = if self.sim.puff_left > 0 {
                self.sim.puff_left -= 1;
                [self.sim.puff[0], self.sim.puff[1], self.sim.puff_amount, radius]
            } else {
                [0.0; 4]
            };
            grid.step(ctx, &mut self.step, &StepUniform::new(&p, false, inj));
        }

        let cam = kit::Cam2d::from_floats(&floats[off(B_CAM)..off(B_CAM) + kit::Cam2d::N]);
        let look = kit::Look::from_floats(&floats[off(B_LOOK)..off(B_LOOK) + kit::Look::N]);
        let tw = kit::Twist::from_floats(&floats[off(B_TWIST)..off(B_TWIST) + kit::Twist::N]);
        let slot = |blk: usize| ColorSlotGpu::from_floats(&floats[off(blk)..off(blk) + 12]);
        let (rlo, rhi) = p.rho_range();
        let uniform = DisplayUniform {
            cloud: slot(B_NUBE),
            background: slot(B_FONDO),
            a: [aspect, time, w as f32, h as f32],
            b: [clean(own, F_MODO).round(), clean(own, F_CONTRASTE), clean(own, F_SCHLIEREN), clean(own, F_SOMBRA)],
            c: [if ctx.fondo_alpha { 1.0 } else { 0.0 }, clean(own, F_LUZX), clean(own, F_LUZY), p.gamma],
            d: [rlo, rhi, P0, p.e_ref],
            e: [cam.zoom, cam.pan_x, cam.pan_y, cam.turn],
            f: [cam.drift, look.shine, look.contrast, look.opacity],
            g: [look.background_level, look.tone, look.saturation, 0.15 * (0.2 + p.shear)],
            h: [tw.warp_amount, tw.warp_freq, tw.mirror, tw.pulse],
        };
        let (view, sampler) = grid.current();
        self.display.draw_textured(ctx, target, format, &uniform, &[(view, sampler)]);
    }
}

/// Total heat/cloud/push of a puff at `inyecta_fuerza` 1, split over its steps.
const PUFF_AMOUNT: f32 = 0.5;

// ---------------------------------------------------------------------------
// CPU reference
// ---------------------------------------------------------------------------

type V4 = [f32; 4];

/// The same scheme as the step shader, in plain Rust: a periodic grid of
/// stored states `(rho, rho*u, rho*v, E - Eref)`.
pub struct Euler {
    pub w: usize,
    pub h: usize,
    pub s: Vec<V4>,
}

fn add(a: V4, b: V4) -> V4 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}
fn sub(a: V4, b: V4) -> V4 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]]
}
fn scale(a: V4, k: f32) -> V4 {
    [a[0] * k, a[1] * k, a[2] * k, a[3] * k]
}

impl Euler {
    /// The initial condition of `shaders/nubes_step.wgsl::initial`.
    pub fn new(p: &Params) -> Euler {
        let (w, h) = (p.w as usize, p.h as usize);
        let mut s = vec![[0.0; 4]; w * h];
        for j in 0..h {
            for i in 0..w {
                let (cx, cy) = (i as f32 + 0.5, j as f32 + 0.5);
                let y = cy / h as f32;
                let wid = p.width.max(0.002);
                let band = 0.5 * (1.0 - (((y - 0.5).abs() - 0.25) / wid).tanh());
                let mut rho = p.rho_lo + (p.rho_hi - p.rho_lo) * band;
                rho *= 1.0 - 0.35 * p.tgrad * (std::f32::consts::TAU * y).sin();
                let vx = p.shear * (2.0 * band - 1.0);
                let sig = 0.0354 + wid;
                let env = (-(y - 0.25).powi(2) / (2.0 * sig * sig)).exp() + (-(y - 0.75).powi(2) / (2.0 * sig * sig)).exp();
                let kx = cx / w as f32 * std::f32::consts::TAU;
                let wave: f32 = (0..4).map(|k| p.amps[k] * (p.cycles[k] * kx + p.phases[k]).sin()).sum();
                let vy = p.perturb * wave * env;
                s[j * w + i] = cons(p, [rho, vx, vy, P0]);
            }
        }
        Euler { w, h, s }
    }

    fn at(&self, x: i64, y: i64) -> V4 {
        let (w, h) = (self.w as i64, self.h as i64);
        self.s[(y.rem_euclid(h) * w + x.rem_euclid(w)) as usize]
    }

    /// One step with no sources (no viscosity, gravity or puff beyond `p`).
    pub fn step(&mut self, p: &Params) {
        let (w, h) = (self.w as i64, self.h as i64);
        let recons: Vec<Recon> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| self.recon(p, x, y))
            .collect();
        let rc = |x: i64, y: i64| &recons[(y.rem_euclid(h) * w + x.rem_euclid(w)) as usize];
        let mut next = self.s.clone();
        for y in 0..h {
            for x in 0..w {
                let me = rc(x, y);
                let fxm = flux(p, rc(x - 1, y).xp, me.xm, 0);
                let fxp = flux(p, me.xp, rc(x + 1, y).xm, 0);
                let fym = flux(p, rc(x, y - 1).yp, me.ym, 1);
                let fyp = flux(p, me.yp, rc(x, y + 1).ym, 1);
                let d = add(sub(fxp, fxm), sub(fyp, fym));
                let s0 = self.at(x, y);
                let mut s = sub(s0, scale(d, p.r));
                if p.visc > 0.0 {
                    let (a, b, c, e) = (self.at(x - 1, y), self.at(x + 1, y), self.at(x, y - 1), self.at(x, y + 1));
                    for k in 1..3 {
                        s[k] += p.visc * (a[k] + b[k] + c[k] + e[k] - 4.0 * s0[k]);
                    }
                }
                if p.grav_dt != 0.0 {
                    let an = s0[0] - p.rho_mean;
                    let vy = s0[2] / s0[0].max(p.rho_floor);
                    s[2] += p.grav_dt * an;
                    s[3] += p.grav_dt * an * vy;
                }
                next[(y * w + x) as usize] = cons(p, prim(p, s));
            }
        }
        self.s = next;
    }

    fn recon(&self, p: &Params, x: i64, y: i64) -> Recon {
        let c = prim(p, self.at(x, y));
        let xl = prim(p, self.at(x - 1, y));
        let xr = prim(p, self.at(x + 1, y));
        let yl = prim(p, self.at(x, y - 1));
        let yr = prim(p, self.at(x, y + 1));
        let gx = limit(p, c, xl, xr, scale(sub(xr, xl), 0.5));
        let gy = limit(p, c, yl, yr, scale(sub(yr, yl), 0.5));
        let (r, g) = (p.r, p.gamma);
        let [rho, vx, vy, pr] = c;
        let hf = [
            rho - 0.5 * r * (vx * gx[0] + rho * gx[1] + vy * gy[0] + rho * gy[2]),
            vx - 0.5 * r * (vx * gx[1] + vy * gy[1] + gx[3] / rho),
            vy - 0.5 * r * (vx * gx[2] + vy * gy[2] + gy[3] / rho),
            pr - 0.5 * r * (g * pr * (gx[1] + gy[2]) + vx * gx[3] + vy * gy[3]),
        ];
        Recon {
            xm: safe(p, sub(hf, scale(gx, 0.5))),
            xp: safe(p, add(hf, scale(gx, 0.5))),
            ym: safe(p, sub(hf, scale(gy, 0.5))),
            yp: safe(p, add(hf, scale(gy, 0.5))),
        }
    }

    /// Total mass (the sum of rho over the grid).
    pub fn mass(&self) -> f64 {
        self.s.iter().map(|c| c[0] as f64).sum()
    }
    pub fn momentum(&self) -> (f64, f64) {
        (self.s.iter().map(|c| c[1] as f64).sum(), self.s.iter().map(|c| c[2] as f64).sum())
    }
    /// Total energy including the reference offset.
    pub fn energy(&self, p: &Params) -> f64 {
        self.s.iter().map(|c| (c[3] + p.e_ref) as f64).sum()
    }
    pub fn finite(&self) -> bool {
        self.s.iter().all(|c| c.iter().all(|v| v.is_finite()))
    }
}

struct Recon {
    xm: V4,
    xp: V4,
    ym: V4,
    yp: V4,
}

fn prim(p: &Params, s: V4) -> V4 {
    let rho = s[0].clamp(p.rho_floor, RHO_MAX);
    let vx = (s[1] / rho).clamp(-p.vmax, p.vmax);
    let vy = (s[2] / rho).clamp(-p.vmax, p.vmax);
    let e = s[3] + p.e_ref;
    let pr = (e - 0.5 * rho * (vx * vx + vy * vy)) * (p.gamma - 1.0);
    let pmax = (rho * p.cmax * p.cmax / p.gamma).max(p.p_floor);
    [rho, vx, vy, pr.clamp(p.p_floor, pmax)]
}

fn cons(p: &Params, q: V4) -> V4 {
    let e = q[3] / (p.gamma - 1.0) + 0.5 * q[0] * (q[1] * q[1] + q[2] * q[2]);
    [q[0], q[0] * q[1], q[0] * q[2], e - p.e_ref]
}

fn safe(p: &Params, q: V4) -> V4 {
    [q[0].max(p.rho_floor), q[1].clamp(-p.vmax, p.vmax), q[2].clamp(-p.vmax, p.vmax), q[3].max(p.p_floor)]
}

fn limit(p: &Params, f: V4, fl: V4, fr: V4, g: V4) -> V4 {
    let mut o = g;
    for k in 0..4 {
        let eps = |v: f32| if v == 0.0 { 1.0e-8 } else { 0.0 };
        let g1 = ((f[k] - fl[k]) / (g[k] + eps(g[k]))).clamp(0.0, 1.0) * g[k];
        let g2 = ((fr[k] - f[k]) / (g1 + eps(g1))).clamp(0.0, 1.0) * g1;
        o[k] = g[k] + (g2 - g[k]) * p.limiter;
    }
    o
}

/// Rusanov flux (central average minus half the fastest wave speed times the
/// jump, right minus left) of (rho, rho*vx, rho*vy, E) across a face with normal `ax`.
fn flux(p: &Params, l: V4, r: V4, ax: usize) -> V4 {
    let g = p.gamma;
    let (n, t) = if ax == 0 { (1, 2) } else { (2, 1) };
    let (vnl, vtl, vnr, vtr) = (l[n], l[t], r[n], r[t]);
    let enl = l[3] / (g - 1.0) + 0.5 * l[0] * (vnl * vnl + vtl * vtl);
    let enr = r[3] / (g - 1.0) + 0.5 * r[0] * (vnr * vnr + vtr * vtr);
    let rs = 0.5 * (l[0] + r[0]);
    let mn = 0.5 * (l[0] * vnl + r[0] * vnr);
    let mt = 0.5 * (l[0] * vtl + r[0] * vtr);
    let es = 0.5 * (enl + enr);
    let ps = ((g - 1.0) * (es - 0.5 * (mn * mn + mt * mt) / rs)).max(0.0);
    let c = ((g * l[3] / l[0]).sqrt() + vnl.abs()).max((g * r[3] / r[0]).sqrt() + vnr.abs());
    let fm = mn - c * 0.5 * (r[0] - l[0]);
    let fn_ = mn * mn / rs + ps - c * 0.5 * (r[0] * vnr - l[0] * vnl);
    let ft = mn * mt / rs - c * 0.5 * (r[0] * vtr - l[0] * vtl);
    let fe = (es + ps) * mn / rs - c * 0.5 * (enr - enl);
    if ax == 0 {
        [fm, fn_, ft, fe]
    } else {
        [fm, ft, fn_, fe]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
    const W: u32 = 64;
    const TIMES: [f32; 6] = [0.0, 0.1, 0.2, 0.3, 0.4, 0.5];

    /// Render a sequence of (time, floats) on one instance; the last frame.
    fn seq(gpu: &crate::gpu::Gpu, model: &mut Nubes, frames: &[(f32, &[f32])]) -> Vec<u8> {
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, W, W, FORMAT);
        let mut out = vec![];
        for &(t, floats) in frames {
            let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: t, width: W, height: W, fondo_alpha: false, detail: 1.0 };
            model.render(&ctx, floats, &view, FORMAT);
            out = crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, W, W);
        }
        out
    }

    fn frames(gpu: &crate::gpu::Gpu, model: &mut Nubes, floats: &[f32], times: &[f32]) -> Vec<u8> {
        let v: Vec<(f32, &[f32])> = times.iter().map(|&t| (t, floats)).collect();
        seq(gpu, model, &v)
    }

    fn base() -> Vec<f32> {
        let mut f = SCHEMA.defaults();
        f[off(B_TWIST)] = 0.5; // warp_cant, so warp_freq has something to scale
        for v in &mut f[off(B_FONDO)..off(B_FONDO) + 12] {
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
        assert_eq!(FLOATS.len(), F_VEL + 1);
        let mut names: Vec<&str> = FLOATS.iter().map(|s| s.name).collect();
        names.sort();
        let n = names.len();
        names.dedup();
        assert_eq!(n, names.len(), "an own float repeats a name");
        for k in kit::LOOK.iter().chain(kit::CAM2D).chain(kit::TWIST) {
            assert!(!FLOATS.iter().any(|s| s.name == k.name), "`{}` collides with a kit float", k.name);
        }
    }

    #[test]
    fn every_float_moves_the_frame() {
        let gpu = crate::gpu::Gpu::new_headless();
        let own_end = FLOATS.len();
        let (cam, look, twist) = (off(B_CAM), off(B_LOOK), off(B_TWIST));
        let mut idx: Vec<(usize, &'static str)> = FLOATS.iter().enumerate().map(|(i, s)| (i, s.name)).collect();
        for (start, k) in [(cam, kit::CAM2D), (look, kit::LOOK), (twist, kit::TWIST)] {
            idx.extend(k.iter().enumerate().map(|(i, s)| (start + i, s.name)));
        }
        let b = base();
        let reference = frames(&gpu, &mut Nubes::new(), &b, &TIMES);
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
            let far = if name == "tone" { 0.5 } else { far };
            let mut f = b.clone();
            let mut r = reference.clone();
            if name == "reset" {
                // A rising edge mid-sequence retriggers the initial condition.
                let mut hi = b.clone();
                hi[i] = 1.0;
                let with: Vec<(f32, &[f32])> =
                    TIMES.iter().enumerate().map(|(k, &t)| (t, if k >= 3 { &hi[..] } else { &b[..] })).collect();
                let got = seq(&gpu, &mut Nubes::new(), &with);
                assert!(r != got, "float `reinicio` did not move the frame");
                continue;
            }
            // The puff floats only matter once the beat fires.
            if name.starts_with("inject_") && name != "inject_rhythm" {
                let mut beat = b.clone();
                beat[F_RITMO] = 1.0;
                r = frames(&gpu, &mut Nubes::new(), &beat, &TIMES);
                f = beat;
            }
            f[i] = far;
            let got = frames(&gpu, &mut Nubes::new(), &f, &TIMES);
            assert!(r != got, "float `{name}` ({} -> {far}) did not move the frame", b[i]);
        }
    }

    #[test]
    fn the_defaults_are_not_flat_and_hold_still_without_time() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let mut m = Nubes::new();
        let a = frames(&gpu, &mut m, &f, &[1.0]);
        let again = frames(&gpu, &mut m, &f, &[1.0]);
        assert_eq!(a, again, "the same instant twice must not step the flow");
        let mut distinct = std::collections::HashSet::new();
        for px in a.chunks_exact(4) {
            distinct.insert([px[0], px[1], px[2]]);
        }
        assert!(distinct.len() > 16, "default frame is flat ({} colours)", distinct.len());
        let later = frames(&gpu, &mut m, &f, &[1.1]);
        assert_ne!(a, later, "a tenth of a second later the clouds must have moved");
    }

    #[test]
    fn deterministic_and_a_seek_or_resize_reseeds() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let a = frames(&gpu, &mut Nubes::new(), &f, &TIMES);
        let b = frames(&gpu, &mut Nubes::new(), &f, &TIMES);
        assert_eq!(a, b);
        let mut m = Nubes::new();
        let first = frames(&gpu, &mut m, &f, &TIMES);
        assert_eq!(first, a);
        let fresh = frames(&gpu, &mut Nubes::new(), &f, &[0.0]);
        let rewound = frames(&gpu, &mut m, &f, &[0.0]);
        assert_eq!(fresh, rewound);
    }

    fn extreme_rows() -> Vec<Vec<f32>> {
        let mut rows = vec![];
        for pick in 0..3 {
            let mut f = SCHEMA.defaults();
            for (i, s) in FLOATS.iter().enumerate() {
                f[i] = match pick {
                    0 => s.max,
                    1 => s.min,
                    _ => {
                        if i % 2 == 0 {
                            s.max
                        } else {
                            s.min
                        }
                    }
                };
            }
            f[F_RITMO] = 1.0;
            f[F_RES] = 64.0; // keep the test quick
            rows.push(f);
        }
        rows
    }

    #[test]
    fn cfl_and_the_time_step_are_always_clamped_in_rust() {
        for c in [0.0, 0.1, 0.4, 0.9, 5.0, f32::NAN, f32::INFINITY, -1.0] {
            for g in [0.0, 1.1, 1.67, 2.0, 9.0, f32::NAN, -3.0] {
                let mut row = SCHEMA.defaults();
                row[F_CFL] = c;
                row[F_GAMMA] = g;
                let p = Params::from_floats(&row, 128, 128);
                assert!(p.r > 0.0 && p.r * (p.cmax + p.vmax) <= MAX_CFL + 1e-6, "cfl {c} gamma {g}: r {}", p.r);
            }
        }
        for row in extreme_rows() {
            let p = Params::from_floats(&row, 96, 96);
            assert!(p.r * (p.cmax + p.vmax) <= MAX_CFL + 1e-6);
        }
    }

    fn params(w: u32, h: u32) -> Params {
        let mut row = SCHEMA.defaults();
        row[F_VISC] = 0.0;
        row[F_GRAV] = 0.0;
        Params::from_floats(&row, w, h)
    }

    #[test]
    fn one_kh_step_conserves_mass_momentum_and_energy_on_the_periodic_grid() {
        let p = params(96, 64);
        let mut sim = Euler::new(&p);
        let (m0, e0) = (sim.mass(), sim.energy(&p));
        let (px0, py0) = sim.momentum();
        sim.step(&p);
        let (m1, e1) = (sim.mass(), sim.energy(&p));
        assert!(((m1 - m0) / m0).abs() < 1e-6, "mass {m0} -> {m1}");
        assert!(((e1 - e0) / e0).abs() < 1e-5, "energy {e0} -> {e1}");
        let (px1, py1) = sim.momentum();
        assert!((px1 - px0).abs() < 1e-2 * m0 * 1e-2 + 1e-3 && (py1 - py0).abs() < 1e-3, "momentum drifted");
        for _ in 0..60 {
            sim.step(&p);
        }
        assert!(sim.finite());
        assert!(((sim.mass() - m0) / m0).abs() < 1e-5, "mass after 61 steps {}", sim.mass());
        assert!(((sim.energy(&p) - e0) / e0).abs() < 1e-4, "energy after 61 steps");
    }

    #[test]
    fn the_flow_actually_rolls_up() {
        let mut p = params(96, 96);
        p.limiter = 0.0;
        p.width = 0.02;
        let mut sim = Euler::new(&p);
        let vy_rms = |s: &Euler| {
            let n = s.s.len() as f32;
            (s.s.iter().map(|c| (c[2] / c[0]).powi(2)).sum::<f32>() / n).sqrt()
        };
        let before = vy_rms(&sim);
        let mut peak = 0.0f32;
        for _ in 0..1200 {
            sim.step(&p);
            peak = peak.max(vy_rms(&sim));
        }
        assert!(peak > 2.0 * before, "the shear layer did not grow ({before} -> peak {peak})");
    }

    #[test]
    fn the_cpu_reference_stays_finite_and_bounded_with_extreme_floats() {
        for (r, row) in extreme_rows().iter().enumerate() {
            let p = Params::from_floats(row, 64, 64);
            let mut sim = Euler::new(&p);
            for _ in 0..300 {
                sim.step(&p);
            }
            assert!(sim.finite(), "row {r}: non-finite state");
            for c in &sim.s {
                let q = prim(&p, *c);
                assert!(q[0] <= RHO_MAX && q[1].abs() <= p.vmax && q[2].abs() <= p.vmax && q[3] > 0.0);
            }
        }
    }

    fn read_f16(gpu: &crate::gpu::Gpu, texture: &wgpu::Texture, w: u32, h: u32) -> Vec<f32> {
        let bpr = (w * 8).next_multiple_of(256);
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nubes readback"),
            size: (bpr * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        gpu.queue.submit(Some(enc.finish()));
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
        rx.recv().unwrap().unwrap();
        let data = slice.get_mapped_range().expect("range");
        let mut out = vec![];
        for row in 0..h {
            let s = (row * bpr) as usize;
            for px in data[s..s + (w * 8) as usize].chunks_exact(2) {
                out.push(half::f16::from_le_bytes([px[0], px[1]]).to_f32());
            }
        }
        out
    }

    #[test]
    fn the_shader_matches_the_cpu_reference() {
        let gpu = crate::gpu::Gpu::new_headless();
        let p = params(64, 64);
        let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: 0.0, width: 64, height: 64, fondo_alpha: false, detail: 1.0 };
        let mut pass = Fullscreen::with_textures("nubes step test", include_str!("../../shaders/nubes_step.wgsl"), 1);
        let mut grid = PingPong::new(&gpu.device, p.w, p.h);
        grid.step(&ctx, &mut pass, &StepUniform::new(&p, true, [0.0; 4]));
        let mut cpu = Euler::new(&p);
        let compare = |grid: &PingPong, cpu: &Euler, tol: f32, what: &str| {
            let g = read_f16(&gpu, grid.current_texture(), p.w, p.h);
            let mut worst = 0.0f32;
            for (i, c) in cpu.s.iter().enumerate() {
                for k in 0..4 {
                    worst = worst.max((g[i * 4 + k] - c[k]).abs());
                }
            }
            assert!(worst < tol, "{what}: worst |gpu - cpu| = {worst}");
            worst
        };
        let w0 = compare(&grid, &cpu, 2e-3, "initial condition");
        for _ in 0..16 {
            grid.step(&ctx, &mut pass, &StepUniform::new(&p, false, [0.0; 4]));
            cpu.step(&p);
        }
        let w1 = compare(&grid, &cpu, 4e-2, "16 steps");
        eprintln!("gpu vs cpu: init {w0:.5}, after 16 steps {w1:.5}");
    }

    #[test]
    fn the_gpu_state_stays_finite_and_bounded_after_many_frames_with_extreme_floats() {
        let gpu = crate::gpu::Gpu::new_headless();
        let fmt = wgpu::TextureFormat::Rgba16Float;
        for (r, row) in extreme_rows().iter().enumerate() {
            for mode in [0.0, 1.0, 2.0, 3.0] {
                let mut row = row.clone();
                row[F_MODO] = mode;
                let mut m = Nubes::new();
                let (tex, view) = crate::gpu::offscreen_target(&gpu.device, W, W, fmt);
                for k in 0..60 {
                    let ctx = Ctx {
                        device: &gpu.device,
                        queue: &gpu.queue,
                        time: k as f32 * 0.1,
                        width: W,
                        height: W,
                        fondo_alpha: false, detail: 1.0,
                    };
                    m.render(&ctx, &row, &view, fmt);
                }
                let px = read_f16(&gpu, &tex, W, W);
                assert!(px.iter().all(|v| v.is_finite()), "row {r} modo {mode}: non-finite pixel");
                let g = m.grid.as_ref().unwrap();
                let st = read_f16(&gpu, g.current_texture(), g.width, g.height);
                let p = Params::from_floats(&row, g.width, g.height);
                for c in st.chunks_exact(4) {
                    assert!(c.iter().all(|v| v.is_finite()), "row {r} modo {mode}: non-finite state");
                    assert!(c[0] > 0.0 && c[0] <= RHO_MAX * 1.01, "row {r}: rho {}", c[0]);
                    assert!(c[1].abs() <= p.vmax * RHO_MAX * 1.01 && c[2].abs() <= p.vmax * RHO_MAX * 1.01);
                }
            }
        }
    }
}
