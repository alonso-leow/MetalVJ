//! Fluido — Navier-Stokes liquid colour, after WebGL-Fluid-Simulation.
//!
//! The classic stable-fluids pipeline on the GPU (`shaders/fluido_sim.wgsl`):
//! per step, splats, curl, vorticity confinement, divergence, Jacobi pressure,
//! gradient subtract, then semi-Lagrangian advection of velocity and dye.
//! `shaders/fluido.wgsl` lights the dye as a thin film and adds bloom and
//! sunrays. Written from the algorithm; no code was taken (the reference is
//! MIT and is credited in NOTICE).
//!
//! **Audio drives splats, not a mouse.** `splat_n` emitters follow one of
//! four patterns (`emisor_modo`: orbit, sweeping line, random, mirrored),
//! each streaming a little dye and force every step, with a burst on every
//! kick (`splat_ritmo` per second). Colours walk the `tinta` palette and
//! drift with `tono_deriva`.
//!
//! **Stability lives here.** Every float is cleaned and clamped; the
//! structural floats are quantised; dissipation and force ranges are bounded;
//! the shader clamps every texel it writes. Semi-Lagrangian advection and the
//! projection are stable at any in-range setting.
//!
//! **State advances by time**, at a fixed 60 steps per second: a fractional
//! step is carried, the same instant twice steps nothing, a resize, rewind,
//! seek or structural change reseeds and burns in. All randomness is a hash
//! of (emitter, kick index), never a clock.

use std::sync::OnceLock;

use crate::model::{color_at, background, spec, spec_held, Block, Ctx, FloatSpec, Model, Schema};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::kit::{self, CAM2D_BLOCK, LOOK_BLOCK, TWIST_BLOCK};
use crate::shared::sim::STATE_FORMAT;
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    // Velocity grid height in cells (quantised to 16). Structural.
    spec_held("simulation_resolution", 32.0, 256.0, 128.0, 0.2),
    // Dye grid height in texels (quantised to 64). Structural.
    spec_held("dye_resolution", 128.0, 1024.0, 512.0, 0.2),
    // Dye fade per second.
    spec("density_dissipation", 0.0, 4.0, 1.3),
    // Velocity fade per second.
    spec("speed_dissipation", 0.0, 4.0, 0.25),
    // Fraction of the pressure kept from the previous step.
    spec("pressure", 0.0, 1.0, 0.8),
    // Jacobi iterations per step. Structural.
    spec_held("pressure_iter", 4.0, 40.0, 20.0, 0.2),
    // Vorticity confinement: how hard small eddies are kept alive.
    spec("vorticity", 0.0, 60.0, 28.0),
    // Splat radius, in hundredths of the frame (squared falloff).
    spec("radius_splat", 0.05, 1.0, 0.22),
    // Force of a splat; the stream is a fraction of it, a kick is all of it.
    spec("force_splat", 0.0, 20000.0, 6000.0),
    // Emitters.
    spec("splat_n", 1.0, 12.0, 4.0),
    // Kicks per second (the beat rate).
    spec("splat_rhythm", 0.0, 8.0, 1.5),
    // 0 orbit, 1 sweeping line, 2 random, 3 mirrored. Structural.
    spec_held("emitter_mode", 0.0, 3.0, 0.0, 0.2),
    // Centre of the emitter pattern (y from the top).
    spec("emitter_x", 0.0, 1.0, 0.5),
    spec("emitter_y", 0.0, 1.0, 0.5),
    // Pattern speed in turns of the cycle per second; sign reverses it.
    spec("emitter_turn", -2.0, 2.0, 0.4),
    // Buoyancy of the dye (positive y falls toward the bottom of the frame).
    spec("gravity_x", -1.0, 1.0, 0.0),
    spec("gravity_y", -1.0, 1.0, 0.0),
    spec("bloom", 0.0, 2.0, 0.5),
    spec("bloom_threshold", 0.0, 1.0, 0.5),
    spec("sunrays", 0.0, 1.0, 0.3),
    // Ray length: how slowly the rays fade along their march.
    spec("sunray_weight", 0.2, 1.0, 0.8),
    // Bump shading and glint from the dye's own gradient.
    spec("shading", 0.0, 1.0, 0.5),
    spec("iridescence", 0.0, 1.0, 0.1),
    // Speed at which the splat palette position drifts.
    spec("tone_drift", 0.0, 2.0, 0.4),
];

/// Linear: magenta, cyan, amber, violet.
const TINTA: [f32; 12] = [1.0, 0.04, 0.35, 0.03, 0.55, 1.0, 1.0, 0.45, 0.04, 0.35, 0.08, 1.0];

const BLOCKS: &[Block] = &[
    Block::Floats(FLOATS),
    CAM2D_BLOCK,
    LOOK_BLOCK,
    TWIST_BLOCK,
    color_at("ink", &TINTA),
    background(),
    crate::model::SIGNAL_FLOATS_BLOCK,
    crate::model::EFFECTS_FLOATS_BLOCK,
    crate::model::EFFECTS_COLOR_BLOCK,
];

pub static SCHEMA: Schema = Schema { blocks: BLOCKS };

const I_CAM: usize = 1;
const I_LOOK: usize = 2;
const I_TWIST: usize = 3;
const I_TINTA: usize = 4;
const I_FONDO: usize = 5;

const F_SIM: usize = 0;
const F_DYE: usize = 1;
const F_DDISIP: usize = 2;
const F_VDISIP: usize = 3;
const F_PRES: usize = 4;
const F_ITER: usize = 5;
const F_VORT: usize = 6;
const F_RADIO: usize = 7;
const F_FUERZA: usize = 8;
const F_N: usize = 9;
const F_RITMO: usize = 10;
const F_MODO: usize = 11;
const F_EX: usize = 12;
const F_EY: usize = 13;
const F_GIRO: usize = 14;
const F_GX: usize = 15;
const F_GY: usize = 16;
const F_BLOOM: usize = 17;
const F_UMBRAL: usize = 18;
const F_RAYS: usize = 19;
const F_PESO: usize = 20;
const F_SOMBRA: usize = 21;
const F_IRI: usize = 22;
const F_DERIVA: usize = 23;

const DT: f32 = 1.0 / 60.0;
const STEPS_PER_SECOND: f32 = 60.0;
const MAX_STEPS_PER_FRAME: u32 = 6;
const BURN_IN: u32 = 90;
const SEEK_GAP: f32 = 30.0;
const MAX_EMITTERS: usize = 12;
/// Largest velocity a texel may hold, in cells/s at sim_res 128 (scaled).
const V_MAX: f32 = 900.0;
const DYE_MAX: f32 = 16.0;
/// Share of `fuerza_splat` / dye a stream adds each step.
const STREAM_FORCE: f32 = 0.05;
const STREAM_DYE: f32 = 0.028;
const BURST_FORCE: f32 = 0.9;
const BURST_DYE: f32 = 0.4;
const GRAVITY_SCALE: f32 = 260.0;

fn off(block: usize) -> usize {
    kit::offset_of(SCHEMA.blocks, block)
}

/// A float cleaned: NaN/inf become the default, clamped to its range.
fn clean(row: &[f32], i: usize) -> f32 {
    let s = FLOATS[i];
    let v = row.get(i).copied().filter(|v| v.is_finite()).unwrap_or(s.default);
    v.clamp(s.min, s.max)
}

fn quant(v: f32, q: f32, lo: f32, hi: f32) -> f32 {
    ((v / q).round() * q).clamp(lo, hi)
}

/// Everything the passes need from a float row. Quantised and clamped.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub sim_h: u32,
    pub dye_h: u32,
    pub dens_disip: f32,
    pub speed_dissipation: f32,
    pub pressure: f32,
    pub iter: u32,
    pub vort: f32,
    pub radius: f32,
    pub fuerza: f32,
    pub n: usize,
    pub rhythm: f32,
    pub mode: u32,
    pub ex: f32,
    pub ey: f32,
    pub turn: f32,
    pub grav: [f32; 2],
    pub bloom: f32,
    pub threshold: f32,
    pub rays: f32,
    pub peso: f32,
    pub shadow: f32,
    pub iri: f32,
    pub drift: f32,
}

impl Params {
    pub fn from_floats(row: &[f32]) -> Params {
        let g = |i| clean(row, i);
        Params {
            sim_h: quant(g(F_SIM), 16.0, 32.0, 256.0) as u32,
            dye_h: quant(g(F_DYE), 64.0, 128.0, 1024.0) as u32,
            dens_disip: g(F_DDISIP),
            speed_dissipation: g(F_VDISIP),
            pressure: g(F_PRES),
            iter: g(F_ITER).round().clamp(4.0, 40.0) as u32,
            vort: g(F_VORT),
            radius: g(F_RADIO),
            fuerza: g(F_FUERZA),
            n: (g(F_N).round() as usize).clamp(1, MAX_EMITTERS),
            rhythm: g(F_RITMO),
            mode: g(F_MODO).round().clamp(0.0, 3.0) as u32,
            ex: g(F_EX),
            ey: g(F_EY),
            turn: g(F_GIRO),
            grav: [g(F_GX), g(F_GY)],
            bloom: g(F_BLOOM),
            threshold: g(F_UMBRAL),
            rays: g(F_RAYS),
            peso: g(F_PESO),
            shadow: g(F_SOMBRA),
            iri: g(F_IRI),
            drift: g(F_DERIVA),
        }
    }

    fn res_scale(&self) -> f32 {
        self.sim_h as f32 / 128.0
    }
}

/// `(sim w, sim h, dye w, dye h)` for an aspect ratio.
pub fn grid_sizes(p: &Params, aspect: f32) -> (u32, u32, u32, u32) {
    let a = if aspect.is_finite() { aspect.clamp(0.25, 4.0) } else { 1.0 };
    let w = |h: u32| ((h as f32 * a).round() as u32).clamp(16, 2048);
    (w(p.sim_h), p.sim_h, w(p.dye_h), p.dye_h)
}

fn hash(a: u32, b: u32, salt: u32) -> f32 {
    let mut h = a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA6B) ^ salt.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    (h >> 8) as f32 / 16_777_216.0
}

/// Triangle wave 0..1..0, so the palette walk never jumps at its ends.
fn tri(x: f32) -> f32 {
    (x.rem_euclid(1.0) * 2.0 - 1.0).abs()
}

/// One emitter at one instant: position (uv, y from the top), force
/// direction (unit), palette position.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Emitter {
    pub pos: [f32; 2],
    pub dir: [f32; 2],
    pub hue: f32,
}

/// The emitters for the pattern at pattern phase `phase` (turns), kick
/// index `kick` and palette phase `hue`.
pub fn emitters(p: &Params, aspect: f32, phase: f32, kick: u32, hue: f32) -> Vec<Emitter> {
    let a = if aspect.is_finite() { aspect.clamp(0.25, 4.0) } else { 1.0 };
    let n = p.n;
    let tau = std::f32::consts::TAU;
    let sign = if p.turn < 0.0 { -1.0 } else { 1.0 };
    let r = 0.27;
    let clampp = |v: f32| v.clamp(0.04, 0.96);
    (0..n)
        .map(|i| {
            let hue_i = tri(i as f32 / n as f32 * 0.6 + hue);
            let (pos, dir) = match p.mode {
                0 => {
                    let ang = phase * tau + tau * i as f32 / n as f32;
                    let (s, c) = ang.sin_cos();
                    // tangent, tipped inward, so neighbours roll into eddies
                    let (ts, tc) = (0.55f32.sin(), 0.55f32.cos());
                    let d = [-s * sign * tc - c * ts, c * sign * tc - s * ts];
                    ([p.ex + r * c / a, p.ey + r * s], d)
                }
                1 => {
                    let x = (phase * 0.5 + (i as f32 + 0.5) / n as f32).rem_euclid(1.0);
                    let up = if i % 2 == 0 { 1.0 } else { -1.0 };
                    let wob = (phase * tau + i as f32 * 1.7).sin() * 0.35;
                    ([x, p.ey], [wob, up * sign])
                }
                2 => {
                    let ang = hash(i as u32, kick, 3) * tau;
                    let pos = [0.1 + 0.8 * hash(i as u32, kick, 1), 0.1 + 0.8 * hash(i as u32, kick, 2)];
                    (pos, [ang.cos(), ang.sin()])
                }
                _ => {
                    let half = n.div_ceil(2);
                    let j = i / 2;
                    let ang = phase * tau + tau * j as f32 / half as f32;
                    let (s, c) = ang.sin_cos();
                    let (ts, tc) = (0.55f32.sin(), 0.55f32.cos());
                    let mut d = [-s * sign * tc - c * ts, c * sign * tc - s * ts];
                    let mut pos = [p.ex + r * c / a, p.ey + r * s];
                    if i % 2 == 1 {
                        pos[0] = 2.0 * p.ex - pos[0];
                        d[0] = -d[0];
                    }
                    (pos, d)
                }
            };
            Emitter { pos: [clampp(pos[0]), clampp(pos[1])], dir, hue: hue_i }
        })
        .collect()
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SimU {
    ink: ColorSlotGpu,
    p0: [f32; 4],
    p1: [f32; 4],
    p2: [f32; 4],
    sp0: [[f32; 4]; MAX_EMITTERS],
    sp1: [[f32; 4]; MAX_EMITTERS],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DisplayU {
    background: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    e: [f32; 4],
    f: [f32; 4],
    g: [f32; 4],
    h: [f32; 4],
}

fn display_src() -> &'static str {
    static SRC: OnceLock<String> = OnceLock::new();
    SRC.get_or_init(|| format!("{}\n{}", kit::WGSL, include_str!("../../shaders/fluido.wgsl")))
}

/// A render target with a view.
struct Tex {
    _t: wgpu::Texture,
    v: wgpu::TextureView,
}

fn make_tex(device: &wgpu::Device, w: u32, h: u32) -> Tex {
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fluido field"),
        size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: STATE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let v = t.create_view(&wgpu::TextureViewDescriptor::default());
    Tex { _t: t, v }
}

/// A ping-pong field: read `cur()`, write `next()`, then `swap()`.
struct Field {
    t: [Tex; 2],
    cur: usize,
}

impl Field {
    fn new(device: &wgpu::Device, w: u32, h: u32) -> Field {
        Field { t: [make_tex(device, w, h), make_tex(device, w, h)], cur: 0 }
    }
    fn cur(&self) -> &wgpu::TextureView {
        &self.t[self.cur].v
    }
    fn next(&self) -> &wgpu::TextureView {
        &self.t[1 - self.cur].v
    }
    fn swap(&mut self) {
        self.cur = 1 - self.cur;
    }
}

struct Gpu {
    speed: Field,
    pres: Field,
    dye: Field,
    curl: Tex,
    div: Tex,
    bloom_a: Tex,
    bloom_b: Tex,
    ray_m: Tex,
    ray_o: Tex,
    smp: wgpu::Sampler,
    key: (u32, u32, u32, u32),
}

impl Gpu {
    fn new(device: &wgpu::Device, key: (u32, u32, u32, u32)) -> Gpu {
        let (sw, sh, dw, dh) = key;
        let (bw, bh) = ((dw / 4).max(16), (dh / 4).max(16));
        Gpu {
            speed: Field::new(device, sw, sh),
            pres: Field::new(device, sw, sh),
            dye: Field::new(device, dw, dh),
            curl: make_tex(device, sw, sh),
            div: make_tex(device, sw, sh),
            bloom_a: make_tex(device, bw, bh),
            bloom_b: make_tex(device, bw, bh),
            ray_m: make_tex(device, bw, bh),
            ray_o: make_tex(device, bw, bh),
            smp: device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            key,
        }
    }
}

#[derive(Default)]
struct Clock {
    wall: Option<f32>,
    carry: f32,
    /// Steps taken since the last reseed.
    steps: u32,
    /// Pattern phase in turns, accumulated so a changing speed never jumps.
    phase: f32,
    /// Kick phase in kicks, accumulated likewise.
    kicks: f32,
    kick_idx: u32,
    /// Palette phase.
    hue: f32,
}

pub struct Fluido {
    sim: Fullscreen,
    display: Fullscreen,
    gpu: Option<Gpu>,
    clock: Clock,
}

impl Fluido {
    pub fn new() -> Fluido {
        Fluido {
            sim: Fullscreen::with_textures("fluido sim", include_str!("../../shaders/fluido_sim.wgsl"), 2),
            display: Fullscreen::with_textures("fluid", display_src(), 3),
            gpu: None,
            clock: Clock::default(),
        }
    }
}

impl Default for Fluido {
    fn default() -> Self {
        Self::new()
    }
}

struct Pass<'a> {
    ctx: &'a Ctx<'a>,
    fs: &'a mut Fullscreen,
    ink: ColorSlotGpu,
    res_scale: f32,
    aspect: f32,
    sim_texel: [f32; 2],
}

impl Pass<'_> {
    fn uniform(&self, mode: f32, texel: [f32; 2], p1: [f32; 4]) -> SimU {
        SimU {
            ink: self.ink,
            p0: [mode, texel[0], texel[1], DT],
            p1,
            p2: [self.sim_texel[0], self.sim_texel[1], self.aspect, self.res_scale],
            sp0: [[0.0; 4]; MAX_EMITTERS],
            sp1: [[0.0; 4]; MAX_EMITTERS],
        }
    }

    fn draw(&mut self, u: &SimU, target: &wgpu::TextureView, a: &wgpu::TextureView, b: &wgpu::TextureView, smp: &wgpu::Sampler) {
        self.fs.draw_textured(self.ctx, target, STATE_FORMAT, u, &[(a, smp), (b, smp)]);
    }
}

impl Model for Fluido {
    fn id(&self) -> &'static str {
        "fluid"
    }

    fn name(&self) -> &'static str {
        "Fluido"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn render(&mut self, ctx: &Ctx, floats: &[f32], target: &wgpu::TextureView, format: wgpu::TextureFormat) {
        let own = &floats[..FLOATS.len()];
        let p = Params::from_floats(own);
        let aspect = if ctx.aspect().is_finite() { ctx.aspect().clamp(0.25, 4.0) } else { 1.0 };
        let key = grid_sizes(&p, aspect);
        let time = if ctx.time.is_finite() { ctx.time } else { 0.0 };

        let clock = &mut self.clock;
        let reseed = match clock.wall {
            None => true,
            Some(prev) => {
                self.gpu.as_ref().map_or(true, |g| g.key != key) || !(0.0..=SEEK_GAP).contains(&(time - prev))
            }
        };
        let dt = clock.wall.map_or(0.0, |prev| (time - prev).clamp(0.0, 0.1));
        clock.wall = Some(time);

        if self.gpu.as_ref().map_or(true, |g| g.key != key) {
            self.gpu = Some(Gpu::new(ctx.device, key));
        }
        let gpu = self.gpu.as_mut().expect("built above");

        let mut steps = 0;
        if reseed {
            *clock = Clock { wall: Some(time), ..Clock::default() };
            steps = BURN_IN;
        } else {
            clock.carry += STEPS_PER_SECOND * dt;
            let whole = clock.carry.floor();
            clock.carry -= whole;
            steps = steps.max((whole.max(0.0) as u32).min(MAX_STEPS_PER_FRAME));
        }

        let slot = |blk: usize| ColorSlotGpu::from_floats(&floats[off(blk)..off(blk) + 12]);
        let mut pass = Pass {
            ctx,
            fs: &mut self.sim,
            ink: slot(I_TINTA),
            res_scale: p.res_scale(),
            aspect,
            sim_texel: [1.0 / key.0 as f32, 1.0 / key.1 as f32],
        };

        if reseed {
            let u = pass.uniform(13.0, pass.sim_texel, [0.0; 4]);
            let (v, pr, dy) = (&mut gpu.speed, &mut gpu.pres, &mut gpu.dye);
            for f in [v, pr, dy] {
                pass.draw(&u, f.next(), f.cur(), f.cur(), &gpu.smp);
                f.swap();
            }
        }

        for _ in 0..steps {
            run_step(&mut pass, gpu, &p, clock);
        }

        post(&mut pass, gpu, &p);

        let cam = kit::Cam2d::from_floats(&floats[off(I_CAM)..off(I_CAM) + kit::Cam2d::N]);
        let look = kit::Look::from_floats(&floats[off(I_LOOK)..off(I_LOOK) + kit::Look::N]);
        let tw = kit::Twist::from_floats(&floats[off(I_TWIST)..off(I_TWIST) + kit::Twist::N]);
        let uniform = DisplayU {
            background: slot(I_FONDO),
            a: [aspect, time, 1.0 / key.2 as f32, 1.0 / key.3 as f32],
            b: [p.shadow, p.iri, p.bloom, p.rays],
            c: [if ctx.fondo_alpha { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
            e: [cam.zoom, cam.pan_x, cam.pan_y, cam.turn],
            f: [cam.drift, look.shine, look.contrast, look.opacity],
            g: [look.background_level, look.tone, look.saturation, 0.0],
            h: [tw.warp_amount, tw.warp_freq, tw.mirror, tw.pulse],
        };
        self.display.draw_textured(
            ctx,
            target,
            format,
            &uniform,
            &[(gpu.dye.cur(), &gpu.smp), (&gpu.bloom_a.v, &gpu.smp), (&gpu.ray_o.v, &gpu.smp)],
        );
    }
}

/// One fixed-size simulation step.
fn run_step(pass: &mut Pass, gpu: &mut Gpu, p: &Params, clock: &mut Clock) {
    // Clocks: pattern, kicks, palette. All accumulate per step.
    clock.phase += p.turn * 0.25 * DT;
    clock.hue += p.drift * 0.07 * DT;
    clock.kicks += p.rhythm * DT;
    let kick = clock.kicks.floor() as u32;
    let burst = clock.steps == 0 || kick != clock.kick_idx;
    if kick != clock.kick_idx {
        clock.hue += 0.113;
    }
    clock.kick_idx = kick;
    clock.steps += 1;

    let em = emitters(p, pass.aspect, clock.phase, kick, clock.hue);
    let rs = p.res_scale();
    let sim_t = pass.sim_texel;
    let vmax = V_MAX * rs.max(1.0);
    let smp = &gpu.smp;

    // Splats.
    let mut u = pass.uniform(7.0, sim_t, [0.0; 4]);
    let radius = (p.radius / 100.0) * if burst { 2.2 } else { 1.0 };
    let gain_f = p.fuerza * DT * rs * if burst { BURST_FORCE } else { STREAM_FORCE };
    let gain_d = if burst { BURST_DYE } else { STREAM_DYE };
    for (i, e) in em.iter().enumerate() {
        u.sp0[i] = [e.pos[0], e.pos[1], e.dir[0] * gain_f, e.dir[1] * gain_f];
        u.sp1[i] = [e.hue, gain_d, radius, 0.0];
    }
    let g = GRAVITY_SCALE * rs * DT;
    u.p1 = [p.grav[0] * g, p.grav[1] * g, em.len() as f32, vmax];
    pass.draw(&u, gpu.speed.next(), gpu.speed.cur(), gpu.dye.cur(), smp);
    gpu.speed.swap();

    let mut ud = u;
    ud.p0 = [8.0, 1.0 / gpu.key.2 as f32, 1.0 / gpu.key.3 as f32, DT];
    ud.p1 = [0.0, DYE_MAX, em.len() as f32, 0.0];
    pass.draw(&ud, gpu.dye.next(), gpu.dye.cur(), gpu.dye.cur(), smp);
    gpu.dye.swap();

    // Curl, vorticity confinement.
    let u = pass.uniform(0.0, sim_t, [0.0; 4]);
    pass.draw(&u, &gpu.curl.v, gpu.speed.cur(), gpu.speed.cur(), smp);
    let u = pass.uniform(1.0, sim_t, [p.vort * rs, vmax, 0.0, 0.0]);
    pass.draw(&u, gpu.speed.next(), gpu.speed.cur(), &gpu.curl.v, smp);
    gpu.speed.swap();

    // Divergence, pressure.
    let u = pass.uniform(2.0, sim_t, [0.0; 4]);
    pass.draw(&u, &gpu.div.v, gpu.speed.cur(), gpu.speed.cur(), smp);
    let u = pass.uniform(3.0, sim_t, [p.pressure, 0.0, 0.0, 0.0]);
    pass.draw(&u, gpu.pres.next(), gpu.pres.cur(), gpu.pres.cur(), smp);
    gpu.pres.swap();
    let u = pass.uniform(4.0, sim_t, [0.0; 4]);
    for _ in 0..p.iter {
        pass.draw(&u, gpu.pres.next(), gpu.pres.cur(), &gpu.div.v, smp);
        gpu.pres.swap();
    }
    let u = pass.uniform(5.0, sim_t, [0.0, vmax, 0.0, 0.0]);
    pass.draw(&u, gpu.speed.next(), gpu.pres.cur(), gpu.speed.cur(), smp);
    gpu.speed.swap();

    // Advection: velocity by itself, then the dye by the new velocity.
    let u = pass.uniform(6.0, sim_t, [p.speed_dissipation, vmax, 0.0, 0.0]);
    pass.draw(&u, gpu.speed.next(), gpu.speed.cur(), gpu.speed.cur(), smp);
    gpu.speed.swap();
    let dye_t = [1.0 / gpu.key.2 as f32, 1.0 / gpu.key.3 as f32];
    let u = pass.uniform(6.0, dye_t, [p.dens_disip, DYE_MAX, 0.0, 0.0]);
    pass.draw(&u, gpu.dye.next(), gpu.dye.cur(), gpu.speed.cur(), smp);
    gpu.dye.swap();
}

/// Bloom and sunrays, rebuilt from the current dye every frame.
fn post(pass: &mut Pass, gpu: &mut Gpu, p: &Params) {
    let smp = &gpu.smp;
    let (bw, bh) = ((gpu.key.2 / 4).max(16) as f32, (gpu.key.3 / 4).max(16) as f32);
    let tx = [1.0 / bw, 1.0 / bh];
    if p.bloom > 0.0 {
        let u = pass.uniform(9.0, tx, [p.threshold, 0.0, 0.0, 0.0]);
        pass.draw(&u, &gpu.bloom_a.v, gpu.dye.cur(), gpu.dye.cur(), smp);
        for k in [1.0f32, 2.5] {
            let uh = pass.uniform(10.0, tx, [tx[0] * k, 0.0, 0.0, 0.0]);
            pass.draw(&uh, &gpu.bloom_b.v, &gpu.bloom_a.v, &gpu.bloom_a.v, smp);
            let uv = pass.uniform(10.0, tx, [0.0, tx[1] * k, 0.0, 0.0]);
            pass.draw(&uv, &gpu.bloom_a.v, &gpu.bloom_b.v, &gpu.bloom_b.v, smp);
        }
    }
    if p.rays > 0.0 {
        let u = pass.uniform(11.0, tx, [0.0; 4]);
        pass.draw(&u, &gpu.ray_m.v, gpu.dye.cur(), gpu.dye.cur(), smp);
        let decay = 0.82 + 0.17 * ((p.peso - 0.2) / 0.8).clamp(0.0, 1.0);
        let u = pass.uniform(12.0, tx, [p.ex, p.ey, decay, 0.0]);
        pass.draw(&u, &gpu.ray_o.v, &gpu.ray_m.v, &gpu.ray_m.v, smp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{offscreen_target, Gpu as Dev};

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

    fn frames(gpu: &Dev, f: &[f32], times: &[f32]) -> Vec<u8> {
        let mut m = Fluido::new();
        let mut last = Vec::new();
        for &t in times {
            let (tex, view) = offscreen_target(&gpu.device, S, S, FMT);
            let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: t, width: S, height: S, fondo_alpha: false, detail: 1.0 };
            m.render(&ctx, f, &view, FMT);
            last = read(gpu, &tex);
        }
        last
    }

    fn read(gpu: &Dev, tex: &wgpu::Texture) -> Vec<u8> {
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

    fn values(px: &[u8]) -> Vec<f32> {
        px.chunks_exact(2).map(|c| half::f16::from_le_bytes([c[0], c[1]]).to_f32()).collect()
    }

    fn mean_luma(px: &[u8]) -> f32 {
        let v = values(px);
        v.iter().sum::<f32>() / v.len() as f32
    }

    #[test]
    fn schema_reads_the_right_offsets() {
        let d = SCHEMA.defaults();
        assert_eq!(d.len(), SCHEMA.width());
        assert_eq!(d[offset("zoom")], 1.0);
        assert_eq!(d[offset("shine")], 1.0);
        assert_eq!(d[offset("simulation_resolution")], 128.0);
        assert_eq!(FLOATS.len(), F_DERIVA + 1);
    }

    #[test]
    fn structural_floats_are_quantised_and_clamped() {
        let mut row = SCHEMA.defaults();
        row[F_SIM] = 100.0;
        row[F_DYE] = 5000.0;
        row[F_ITER] = f32::NAN;
        row[F_FUERZA] = f32::INFINITY;
        let p = Params::from_floats(&row);
        assert_eq!(p.sim_h % 16, 0);
        assert_eq!(p.dye_h, 1024);
        assert_eq!(p.iter, 20);
        assert!(p.fuerza.is_finite() && p.fuerza <= 20000.0);
    }

    #[test]
    fn emitters_stay_in_the_frame_in_every_mode() {
        let mut row = SCHEMA.defaults();
        row[F_N] = 12.0;
        for mode in 0..4 {
            row[F_MODO] = mode as f32;
            let p = Params::from_floats(&row);
            for (ph, k) in [(0.0, 0), (3.7, 9), (-2.2, 400)] {
                let e = emitters(&p, 1.78, ph, k, 0.3);
                assert_eq!(e.len(), 12);
                for m in e {
                    assert!(m.pos.iter().all(|v| (0.0..=1.0).contains(v)), "{m:?}");
                    assert!(m.dir.iter().all(|v| v.is_finite()));
                    assert!((0.0..=1.0).contains(&m.hue));
                }
            }
        }
    }

    #[test]
    fn mirrored_pairs_mirror_about_the_centre() {
        let mut row = SCHEMA.defaults();
        row[F_MODO] = 3.0;
        row[F_N] = 4.0;
        let p = Params::from_floats(&row);
        let e = emitters(&p, 1.0, 0.13, 0, 0.0);
        assert!((e[0].pos[0] + e[1].pos[0] - 2.0 * p.ex).abs() < 1e-5);
        assert!((e[0].pos[1] - e[1].pos[1]).abs() < 1e-5);
    }

    #[test]
    fn every_float_changes_the_frame() {
        let gpu = Dev::new_headless();
        let base = SCHEMA.defaults();
        let times = [0.0, 0.37];
        let reference = frames(&gpu, &base, &times);
        assert!(mean_luma(&reference) > 0.0, "defaults are flat");
        assert_eq!(reference, frames(&gpu, &base, &times), "not deterministic");

        let all: Vec<&FloatSpec> =
            BLOCKS.iter().filter_map(|b| if let Block::Floats(fs) = b { Some(*fs) } else { None }).take(4).flat_map(|fs| fs.iter()).collect();
        let mut dead = Vec::new();
        for spec in all {
            let name = spec.name;
            let o = offset(name);
            let mut base_row = base.clone();
            let mut row = base.clone();
            match name {
                "background_level" => {
                    let at = off(I_FONDO);
                    for r in [&mut base_row, &mut row] {
                        r[at..at + 12].fill(0.4);
                    }
                }
                "warp_freq" => {
                    base_row[offset("warp_amount")] = 0.8;
                    row[offset("warp_amount")] = 0.8;
                }
                "sunray_weight" => {
                    base_row[offset("sunrays")] = 0.8;
                    row[offset("sunrays")] = 0.8;
                }
                "bloom_threshold" => {
                    base_row[offset("bloom")] = 1.5;
                    row[offset("bloom")] = 1.5;
                }
                "emitter_turn" | "emitter_x" | "emitter_y" => {}
                _ => {}
            }
            let far = if (spec.max - spec.default).abs() > (spec.default - spec.min).abs() { spec.max } else { spec.min };
            row[o] = match name {
                "tone" => 0.5,
                "mirror" => 5.0,
                "emitter_mode" => 2.0,
                "simulation_resolution" => 64.0,
                "dye_resolution" => 256.0,
                "pulse" => 1.0,
                "splat_n" => 9.0,
                _ => far,
            };
            if frames(&gpu, &base_row, &times) == frames(&gpu, &row, &times) {
                dead.push(name);
            }
        }
        assert!(dead.is_empty(), "floats that do not change the frame: {dead:?}");
    }

    #[test]
    fn colour_slots_and_effect_blocks_change_the_frame() {
        let gpu = Dev::new_headless();
        let base = SCHEMA.defaults();
        let times = [0.0, 0.37];
        let want = frames(&gpu, &base, &times);
        let mut row = base.clone();
        let at = off(I_TINTA);
        row[at..at + 12].fill(0.2);
        assert_ne!(want, frames(&gpu, &row, &times), "ink");
        let mut row = base.clone();
        let at = off(I_FONDO);
        row[at..at + 12].fill(0.5);
        assert_ne!(want, frames(&gpu, &row, &times), "background");
    }

    #[test]
    fn holds_still_when_time_does_not_advance() {
        let gpu = Dev::new_headless();
        let f = SCHEMA.defaults();
        assert_eq!(frames(&gpu, &f, &[1.0, 1.0, 1.0]), frames(&gpu, &f, &[1.0]));
    }

    #[test]
    fn the_flow_moves_with_time() {
        let gpu = Dev::new_headless();
        let f = SCHEMA.defaults();
        assert_ne!(frames(&gpu, &f, &[0.0, 1.0]), frames(&gpu, &f, &[0.0]));
    }

    /// Many frames at the declared extremes: every pixel stays finite and
    /// bounded, whatever the combination.
    #[test]
    fn extreme_floats_stay_finite_and_bounded() {
        let gpu = Dev::new_headless();
        let mut rows: Vec<Vec<f32>> = Vec::new();
        let specs: Vec<&FloatSpec> =
            BLOCKS.iter().filter_map(|b| if let Block::Floats(fs) = b { Some(*fs) } else { None }).take(2).flat_map(|fs| fs.iter()).collect();
        for pick in 0..4u32 {
            let mut row = SCHEMA.defaults();
            let mut o = 0;
            for b in BLOCKS.iter().take(4) {
                if let Block::Floats(fs) = b {
                    for (j, s) in fs.iter().enumerate() {
                        // Keep the grids small so the run is quick; the
                        // other floats go to an extreme.
                        row[o + j] = match (pick, s.name) {
                            (_, "simulation_resolution") => 64.0,
                            (_, "dye_resolution") => 192.0,
                            (0, _) => s.max,
                            (1, _) => s.min,
                            (2, _) => if (j + o) % 2 == 0 { s.max } else { s.min },
                            _ => if (j + o) % 3 == 0 { s.min } else { s.max },
                        };
                    }
                    o += fs.len();
                }
            }
            rows.push(row);
        }
        let _ = specs;
        for (k, row) in rows.iter().enumerate() {
            let mut m = Fluido::new();
            for i in 0..120 {
                let (tex, view) = offscreen_target(&gpu.device, S, S, FMT);
                let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: i as f32 * 0.04, width: S, height: S, fondo_alpha: false, detail: 1.0 };
                m.render(&ctx, row, &view, FMT);
                if i % 20 == 19 {
                    let v = values(&read(&gpu, &tex));
                    assert!(v.iter().all(|x| x.is_finite()), "row {k} frame {i} not finite");
                    assert!(v.iter().all(|x| x.abs() < 2000.0), "row {k} frame {i} unbounded");
                }
            }
        }
    }

    #[test]
    #[ignore]
    fn ms_per_frame() {
        let gpu = Dev::new_headless();
        let f = SCHEMA.defaults();
        let (w, h) = (1280, 720);
        let mut m = Fluido::new();
        let (tex, view) = offscreen_target(&gpu.device, w, h, FMT);
        let mut t = 0.0;
        for _ in 0..10 {
            t += 1.0 / 60.0;
            let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: t, width: w, height: h, fondo_alpha: false, detail: 1.0 };
            m.render(&ctx, &f, &view, FMT);
        }
        gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
        let t0 = std::time::Instant::now();
        let n = 60;
        for _ in 0..n {
            t += 1.0 / 60.0;
            let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: t, width: w, height: h, fondo_alpha: false, detail: 1.0 };
            m.render(&ctx, &f, &view, FMT);
        }
        gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
        let _ = tex;
        println!("fluido 1280x720 default: {:.2} ms/frame", t0.elapsed().as_secs_f64() * 1000.0 / n as f64);
    }
}
