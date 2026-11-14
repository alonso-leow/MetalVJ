//! Ondas — a 2D electromagnetic field, after Meep.
//!
//! The TMz Yee scheme (Ez, Hx, Hy) leapfrogged on a [`PingPong`] grid: the
//! previous state is the only input of a step, so one fullscreen pass is one
//! leapfrog (`shaders/ondas_step.wgsl`), `shaders/ondas.wgsl` draws it. The
//! material (a dielectric block, lens or prism, or a wall of slits) is
//! analytic in both shaders from the `epsilon_*` floats, so it needs no
//! texture and moving it is live. Written from the equations; no code was
//! taken from Meep (GPL-2-or-later).
//!
//! **Stability is enforced here, not by the declared ranges.** The effective
//! Courant number `courant * c_speed` is clamped to [`MAX_COURANT`] (the 2D
//! limit is 1/sqrt(2)) before it reaches the shader; every float is cleaned of
//! NaN/inf and clamped to its range; the border is a matched conductivity
//! ramp, a small bulk loss keeps a closed cavity from growing, and the shader
//! hard-limits the field as a last resort.
//!
//! **State advances by time**, like Trafico: a fractional step rate is
//! carried, the same instant twice steps nothing, and a resize, rewind or
//! seek reseeds (an empty field plus a burn-in so the first frame has waves).
//! The CPU reference ([`Fdtd`]) at the bottom is the same scheme in Rust, to
//! assert the stability claims.

use crate::model::{color_at, background, spec, spec_held, Block, Ctx, FloatSpec, Model, Schema};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::kit::{self, CAM2D_BLOCK, LOOK_BLOCK, TWIST_BLOCK};
use crate::shared::sim::PingPong;
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    // Source position, as a fraction of the frame (y from the top).
    spec("source_x", 0.0, 1.0, 0.22),
    spec("source_y", 0.0, 1.0, 0.5),
    // Cycles per time unit; the wavelength is 1/freq grid cells. Structural.
    spec_held("source_freq", 0.03, 0.3, 0.09, 0.2),
    // Spatial width of the source, in grid cells.
    spec("source_width", 0.6, 8.0, 1.6),
    // 0 continuous, 1 gaussian, 2 ricker. Structural.
    spec_held("source_type", 0.0, 2.0, 0.0, 0.2),
    // How many sources, in a column about (x, y). Structural.
    spec_held("n_sources", 1.0, 6.0, 1.0, 0.2),
    // Phase between consecutive sources (times pi).
    spec("source_phase", -1.0, 1.0, 0.0),
    // Beat: a rising edge through 0.5 injects a ricker pulse of that strength.
    spec("source_pulse", 0.0, 1.0, 0.0),
    // Relative permittivity of the shape.
    spec("epsilon_block", 1.0, 12.0, 4.0),
    // 0 block, 1 lens, 2 prism, 3 wall with slits. Structural.
    spec_held("epsilon_shape", 0.0, 3.0, 1.0, 0.2),
    spec("epsilon_x", 0.0, 1.0, 0.62),
    spec("epsilon_y", 0.0, 1.0, 0.5),
    // Quarter turns: -1..1 is +-90 degrees.
    spec("epsilon_turn", -1.0, 1.0, 0.0),
    // Openings in the slit wall. Structural.
    spec_held("slit_n", 1.0, 6.0, 2.0, 0.2),
    // Opening width in grid cells.
    spec("slit_width", 1.0, 24.0, 6.0),
    // Border absorber strength (PML-lite). 0 = a mirror box.
    spec("conductivity", 0.0, 1.0, 0.6),
    // Declared Courant number, clamped to MAX_COURANT in Rust. Structural.
    spec_held("current", 0.1, 0.9, 0.5, 0.2),
    // Wave speed scale (also bounded by the Courant clamp).
    spec("c_speed", 0.2, 1.4, 1.0),
    // 0 Ez, 1 |E|, 2 energy, 3 Poynting. Structural.
    spec_held("field_mode", 0.0, 3.0, 0.0, 0.2),
    spec("gain", 0.2, 12.0, 2.5),
    spec("isolines", 0.0, 1.0, 0.0),
    // The flux monitor line and its time-averaged intensity profile.
    spec("flow_marks", 0.0, 1.0, 0.0),
    // Size of the shape, in frame heights. (Not in the brief; a shape needs one.)
    spec("epsilon_size", 0.05, 0.5, 0.18),
];

/// Signed field: negative peak, quiet, quiet, positive peak.
const CAMPO: [f32; 12] = [0.0, 0.30, 0.75, 0.01, 0.02, 0.07, 0.07, 0.02, 0.01, 1.0, 0.68, 0.25];
const MATERIAL: [f32; 12] = [0.10, 0.25, 0.35, 0.20, 0.45, 0.55, 0.35, 0.65, 0.70, 0.80, 0.90, 0.95];
const MARCA: [f32; 12] = [0.2, 0.2, 0.3, 0.4, 0.6, 0.8, 0.8, 0.9, 1.0, 1.0, 1.0, 1.0];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("field", &CAMPO),
        color_at("material", &MATERIAL),
        color_at("mark", &MARCA),
        background(),
        CAM2D_BLOCK,
        LOOK_BLOCK,
        TWIST_BLOCK,
        crate::model::SIGNAL_FLOATS_BLOCK,
        crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

const B_OWN: usize = 0;
const B_CAMPO: usize = 1;
const B_MATERIAL: usize = 2;
const B_MARCA: usize = 3;
const B_FONDO: usize = 4;
const B_CAM: usize = 5;
const B_LOOK: usize = 6;
const B_TWIST: usize = 7;

fn off(block: usize) -> usize {
    kit::offset_of(SCHEMA.blocks, block)
}

const F_X: usize = 0;
const F_Y: usize = 1;
const F_FREQ: usize = 2;
const F_ANCHO: usize = 3;
const F_TIPO: usize = 4;
const F_N: usize = 5;
const F_FASE: usize = 6;
const F_PULSO: usize = 7;
const F_EPS: usize = 8;
const F_FORMA: usize = 9;
const F_EX: usize = 10;
const F_EY: usize = 11;
const F_GIRO: usize = 12;
const F_REND_N: usize = 13;
const F_REND_W: usize = 14;
const F_COND: usize = 15;
const F_COURANT: usize = 16;
const F_CVEL: usize = 17;
const F_MODO: usize = 18;
const F_GAN: usize = 19;
const F_ISO: usize = 20;
const F_FLUJO: usize = 21;
const F_TAM: usize = 22;

/// The 2D Yee limit is 1/sqrt(2) ~ 0.707; stay under it.
pub const MAX_COURANT: f32 = 0.7;
/// Grid cells over the frame height.
pub const GRID_H: u32 = 192;
const MAX_W: u32 = 512;
const BURN_IN: u32 = 150;
const MAX_STEPS_PER_FRAME: u32 = 16;
const STEPS_PER_SECOND: f32 = 220.0;
const SEEK_GAP: f32 = 60.0;

/// A float row cleaned: NaN/inf become the default, everything is clamped to
/// its declared range. Nothing unbounded reaches the shader.
fn clean(row: &[f32], i: usize) -> f32 {
    let s = FLOATS[i];
    let v = row.get(i).copied().filter(|v| v.is_finite()).unwrap_or(s.default);
    v.clamp(s.min, s.max)
}

/// The Courant number the scheme actually runs at, always in (0, MAX_COURANT].
pub fn effective_courant(current: f32, c_speed: f32) -> f32 {
    let c = if current.is_finite() { current } else { 0.5 };
    let v = if c_speed.is_finite() { c_speed } else { 1.0 };
    (c.clamp(0.02, MAX_COURANT) * v.clamp(0.0, 4.0)).clamp(0.02, MAX_COURANT)
}

pub fn grid_size(aspect: f32) -> (u32, u32) {
    let a = if aspect.is_finite() { aspect } else { 1.0 };
    let w = ((GRID_H as f32) * a.max(0.1)).round().clamp(64.0, MAX_W as f32) as u32;
    (w, GRID_H)
}

/// Everything one step needs, from a float row. Shared by the GPU step and
/// the CPU reference so the two cannot drift.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub w: u32,
    pub h: u32,
    pub s: f32,
    pub time: f32,
    pub src_x: f32,
    pub src_y: f32,
    pub freq: f32,
    pub width: f32,
    pub tipo: f32,
    pub n: f32,
    pub phase: f32,
    pub cond: f32,
    /// Start time of the beat pulse, or -1.
    pub pulse_t0: f32,
    pub pulse_amp: f32,
    pub eps: f32,
    pub shape: f32,
    pub cx: f32,
    pub cy: f32,
    pub turn: f32,
    pub size: f32,
    pub rend_n: f32,
    pub rend_w: f32,
}

impl Params {
    pub fn from_floats(row: &[f32], w: u32, h: u32, time: f32, pulse: Option<(f32, f32)>) -> Params {
        let g = |i| clean(row, i);
        Params {
            w,
            h,
            s: effective_courant(g(F_COURANT), g(F_CVEL)),
            time,
            src_x: g(F_X) * w as f32,
            src_y: g(F_Y) * h as f32,
            freq: g(F_FREQ),
            width: g(F_ANCHO),
            tipo: g(F_TIPO).round(),
            n: g(F_N).round(),
            phase: g(F_FASE) * std::f32::consts::PI,
            cond: g(F_COND),
            pulse_t0: pulse.map_or(-1.0, |p| p.0),
            pulse_amp: pulse.map_or(0.0, |p| p.1),
            eps: g(F_EPS),
            shape: g(F_FORMA).round(),
            cx: g(F_EX) * w as f32,
            cy: g(F_EY) * h as f32,
            turn: g(F_GIRO) * std::f32::consts::FRAC_PI_2,
            size: g(F_TAM),
            rend_n: g(F_REND_N).round(),
            rend_w: g(F_REND_W),
        }
    }

    fn m0(&self) -> [f32; 4] {
        [self.eps, self.shape, self.cx, self.cy]
    }
    fn m1(&self) -> [f32; 4] {
        [self.turn, self.size, self.rend_n, self.rend_w]
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct StepUniform {
    dims: [f32; 4],
    src: [f32; 4],
    src2: [f32; 4],
    pulse: [f32; 4],
    m0: [f32; 4],
    m1: [f32; 4],
}

impl StepUniform {
    fn new(p: &Params) -> StepUniform {
        StepUniform {
            dims: [p.w as f32, p.h as f32, p.s, p.time],
            src: [p.src_x, p.src_y, p.freq, p.width],
            src2: [p.tipo, p.n, p.phase, p.cond],
            pulse: [p.pulse_t0, p.pulse_amp, 0.0, 0.0],
            m0: p.m0(),
            m1: p.m1(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DisplayUniform {
    field: ColorSlotGpu,
    material: ColorSlotGpu,
    mark: ColorSlotGpu,
    background: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    m0: [f32; 4],
    m1: [f32; 4],
    e: [f32; 4],
    f: [f32; 4],
    g: [f32; 4],
    h: [f32; 4],
}

fn display_src() -> &'static str {
    static SRC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SRC.get_or_init(|| {
        format!(
            "{}\n{}\n{}",
            kit::WGSL,
            include_str!("../../shaders/ondas_material.wgsl"),
            include_str!("../../shaders/ondas.wgsl")
        )
    })
}

fn step_src() -> &'static str {
    static SRC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SRC.get_or_init(|| {
        format!(
            "{}\n{}",
            include_str!("../../shaders/ondas_material.wgsl"),
            include_str!("../../shaders/ondas_step.wgsl")
        )
    })
}

#[derive(Default)]
struct Sim {
    wall: Option<f32>,
    carry: f32,
    /// Simulated time: the sum of the Courant numbers of the steps taken.
    tsim: f32,
    key: (u32, u32),
    pulse_high: bool,
    pulse: Option<(f32, f32)>,
}

pub struct Ondas {
    step: Fullscreen,
    display: Fullscreen,
    grid: Option<PingPong>,
    sim: Sim,
}

impl Ondas {
    pub fn new() -> Ondas {
        Ondas {
            step: Fullscreen::with_textures("ondas step", step_src(), 1),
            display: Fullscreen::with_textures("waves", display_src(), 1),
            grid: None,
            sim: Sim::default(),
        }
    }
}

impl Default for Ondas {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Ondas {
    fn id(&self) -> &'static str {
        "waves"
    }

    fn name(&self) -> &'static str {
        "Ondas"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn render(&mut self, ctx: &Ctx, floats: &[f32], target: &wgpu::TextureView, format: wgpu::TextureFormat) {
        let own = &floats[off(B_OWN)..off(B_OWN) + FLOATS.len()];
        let aspect = ctx.aspect();
        let (w, h) = grid_size(aspect);
        let key = (w, h);
        let time = if ctx.time.is_finite() { ctx.time } else { 0.0 };

        let reseed = match self.sim.wall {
            None => true,
            Some(prev) => key != self.sim.key || !(0.0..=SEEK_GAP).contains(&(time - prev)),
        };
        let dt = self.sim.wall.map_or(0.0, |prev| (time - prev).clamp(0.0, 0.1));
        self.sim.wall = Some(time);
        self.sim.key = key;

        let pv = clean(own, F_PULSO);
        let high = pv > 0.5;
        let edge = high && !self.sim.pulse_high;
        self.sim.pulse_high = high;
        let mut steps = BURN_IN;
        if reseed {
            self.sim.carry = 0.0;
            self.sim.tsim = 0.0;
            self.sim.pulse = None;
        } else {
            self.sim.carry += STEPS_PER_SECOND * dt;
            let whole = self.sim.carry.floor();
            self.sim.carry -= whole;
            steps = (whole.max(0.0) as u32).min(MAX_STEPS_PER_FRAME);
        }
        if edge {
            self.sim.pulse = Some((self.sim.tsim, pv));
        }

        if self.grid.as_ref().map_or(true, |g| (g.width, g.height) != (w, h)) {
            self.grid = Some(PingPong::new(ctx.device, w, h));
        }
        let grid = self.grid.as_mut().expect("grid built above");
        if reseed {
            grid.reset();
            // An empty field: pulse.z = 1 makes the step pass write zeros.
            let mut u = StepUniform::new(&Params::from_floats(own, w, h, 0.0, None));
            u.pulse[2] = 1.0;
            grid.step(ctx, &mut self.step, &u);
        }
        for _ in 0..steps {
            let p = Params::from_floats(own, w, h, self.sim.tsim, self.sim.pulse);
            grid.step(ctx, &mut self.step, &StepUniform::new(&p));
            self.sim.tsim += p.s;
        }

        let p = Params::from_floats(own, w, h, self.sim.tsim, self.sim.pulse);
        let cam = kit::Cam2d::from_floats(&floats[off(B_CAM)..off(B_CAM) + kit::Cam2d::N]);
        let look = kit::Look::from_floats(&floats[off(B_LOOK)..off(B_LOOK) + kit::Look::N]);
        let tw = kit::Twist::from_floats(&floats[off(B_TWIST)..off(B_TWIST) + kit::Twist::N]);
        let slot = |blk: usize| ColorSlotGpu::from_floats(&floats[off(blk)..off(blk) + 12]);
        let uniform = DisplayUniform {
            field: slot(B_CAMPO),
            material: slot(B_MATERIAL),
            mark: slot(B_MARCA),
            background: slot(B_FONDO),
            a: [aspect, time, w as f32, h as f32],
            b: [clean(own, F_MODO).round(), clean(own, F_GAN), clean(own, F_ISO), clean(own, F_FLUJO)],
            c: [if ctx.fondo_alpha { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
            m0: p.m0(),
            m1: p.m1(),
            e: [cam.zoom, cam.pan_x, cam.pan_y, cam.turn],
            f: [cam.drift, look.shine, look.contrast, look.opacity],
            g: [look.background_level, look.tone, look.saturation, 0.0],
            h: [tw.warp_amount, tw.warp_freq, tw.mirror, tw.pulse],
        };
        let (view, sampler) = grid.current();
        self.display.draw_textured(ctx, target, format, &uniform, &[(view, sampler)]);
    }
}

// ---------------------------------------------------------------------------
// CPU reference
// ---------------------------------------------------------------------------

/// The same scheme as the step shader, in plain Rust.
pub struct Fdtd {
    pub w: usize,
    pub h: usize,
    pub ez: Vec<f32>,
    pub hx: Vec<f32>,
    pub hy: Vec<f32>,
}

const BORDER: f32 = 14.0;
const BULK: f32 = 0.0008;
const LIMIT: f32 = 200.0;

/// Signed distance to the shape or wall, in cells (negative inside): the
/// `on_sd` of `ondas_material.wgsl`.
fn sd(c: (f32, f32), p: &Params) -> f32 {
    let (dx, dy) = (c.0 - p.cx, c.1 - p.cy);
    let (cs, sn) = ((-p.turn).cos(), (-p.turn).sin());
    let (qx, qy) = (cs * dx - sn * dy, sn * dx + cs * dy);
    let t = (p.size * p.h as f32).max(1.0);
    match p.shape as i32 {
        0 => {
            let (ax, ay) = (qx.abs() - t, qy.abs() - t * 0.6);
            ax.max(0.0).hypot(ay.max(0.0)) + ax.max(ay).min(0.0)
        }
        1 => {
            let r = 1.6 * t;
            ((qx - t).hypot(qy) - r).max((qx + t).hypot(qy) - r)
        }
        2 => {
            let kk = 1.732_050_8f32;
            let (mut px, mut py) = ((qx / t).abs() - 1.0, qy / t + 1.0 / kk);
            if px + kk * py > 0.0 {
                (px, py) = ((px - kk * py) * 0.5, (-kk * px - py) * 0.5);
            }
            px -= px.clamp(-2.0, 0.0);
            -px.hypot(py) * py.signum() * t
        }
        _ => {
            let n = p.rend_n.max(1.0);
            let wd = p.rend_w.max(1.0);
            let spacing = (0.7 * p.h as f32 / n).max(wd + 3.0);
            let idx = (qy / spacing + (n - 1.0) * 0.5 + 0.5).floor().clamp(0.0, n - 1.0);
            let cy = (idx - (n - 1.0) * 0.5) * spacing;
            let d = (qy - cy).abs() - wd * 0.5;
            (qx.abs() - 1.5).max(-d)
        }
    }
}

fn wave(tipo: i32, t0: f32, f: f32, ph: f32) -> f32 {
    use std::f32::consts::PI;
    let tw = 2.0 * PI * f;
    let t = t0 - ph / tw;
    match tipo {
        0 => {
            let x = (t * f * 0.5).clamp(0.0, 1.0);
            (tw * t).sin() * x * x * (3.0 - 2.0 * x)
        }
        1 => {
            let per = 14.0 / f;
            let tt = t - (t / per).floor() * per;
            let w = 1.5 / f;
            let d = tt - 4.0 * w;
            (-d * d / (2.0 * w * w)).exp() * (tw * tt).sin()
        }
        _ => {
            let per = 8.0 / f;
            let tt = t - (t / per).floor() * per;
            ricker(tt, f)
        }
    }
}

fn ricker(t: f32, f: f32) -> f32 {
    let a = std::f32::consts::PI * f * (t - 1.5 / f);
    (1.0 - 2.0 * a * a) * (-a * a).exp()
}

impl Fdtd {
    pub fn new(w: usize, h: usize) -> Fdtd {
        let n = w * h;
        Fdtd { w, h, ez: vec![0.0; n], hx: vec![0.0; n], hy: vec![0.0; n] }
    }

    fn sigma(&self, x: f32, y: f32, cond: f32) -> f32 {
        let d = x.min(self.w as f32 - x).min(y.min(self.h as f32 - y));
        let r = ((BORDER - d) / BORDER).clamp(0.0, 1.0);
        BULK + cond * 0.6 * r * r
    }

    fn e(&self, x: i64, y: i64) -> f32 {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            0.0
        } else {
            self.ez[y as usize * self.w + x as usize]
        }
    }

    /// One leapfrog. `p.w`/`p.h` must match the grid.
    pub fn step(&mut self, p: &Params) {
        let (w, h) = (self.w, self.h);
        let s = p.s;
        // Half step: H from the old E.
        let mut hx = vec![0.0f32; w * h];
        let mut hy = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let k = self.sigma(x as f32 + 0.5, y as f32 + 0.5, p.cond) * s * 0.5;
                let (da, db) = ((1.0 - k) / (1.0 + k), s / (1.0 + k));
                let e = self.ez[i];
                hx[i] = da * self.hx[i] - db * (self.e(x as i64, y as i64 + 1) - e);
                hy[i] = da * self.hy[i] + db * (self.e(x as i64 + 1, y as i64) - e);
            }
        }
        let n = p.n.max(1.0) as i32;
        let spacing = 0.1 * h as f32;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
                let hxm = if y > 0 { hx[i - w] } else { 0.0 };
                let hym = if x > 0 { hy[i - 1] } else { 0.0 };
                let curl = (hy[i] - hym) - (hx[i] - hxm);
                let d = sd((cx, cy), p);
                let wall = p.shape as i32 == 3 && d < 0.0;
                let eps = if p.shape as i32 != 3 && d < 0.0 { p.eps.max(1.0) } else { 1.0 };
                let k = self.sigma(cx, cy, p.cond) * s * 0.5;
                let mut ez = ((1.0 - k) / (1.0 + k)) * self.ez[i] + s / (eps * (1.0 + k)) * curl;
                let wd = p.width.max(0.3);
                let mut inj = 0.0;
                for j in 0..n.min(6) {
                    let sy = p.src_y + (j as f32 - (n - 1) as f32 * 0.5) * spacing;
                    let (ddx, ddy) = (cx - p.src_x, cy - sy);
                    let blob = (-(ddx * ddx + ddy * ddy) / (2.0 * wd * wd)).exp();
                    inj += blob * wave(p.tipo as i32, p.time, p.freq, p.phase * (j + 1) as f32);
                }
                if p.pulse_t0 >= 0.0 {
                    let (ddx, ddy) = (cx - p.src_x, cy - p.src_y);
                    let wp = 2.0 * wd;
                    inj += p.pulse_amp * (-(ddx * ddx + ddy * ddy) / (2.0 * wp * wp)).exp() * ricker(p.time - p.pulse_t0, p.freq);
                }
                ez += s / eps * 2.0 * inj;
                if wall {
                    ez = 0.0;
                }
                self.ez[i] = ez.clamp(-LIMIT, LIMIT);
            }
        }
        for i in 0..w * h {
            self.hx[i] = hx[i].clamp(-LIMIT, LIMIT);
            self.hy[i] = hy[i].clamp(-LIMIT, LIMIT);
        }
    }

    /// Total field energy, `sum 0.5 (E^2 + Hx^2 + Hy^2)`.
    pub fn energy(&self) -> f32 {
        (0..self.w * self.h)
            .map(|i| 0.5 * (self.ez[i] * self.ez[i] + self.hx[i] * self.hx[i] + self.hy[i] * self.hy[i]))
            .sum()
    }

    pub fn finite(&self) -> bool {
        self.ez.iter().chain(&self.hx).chain(&self.hy).all(|v| v.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
    const W: u32 = 64;

    fn frames(gpu: &crate::gpu::Gpu, model: &mut Ondas, floats: &[f32], times: &[f32]) -> Vec<u8> {
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, W, W, FORMAT);
        let mut out = vec![];
        for &t in times {
            let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: t, width: W, height: W, fondo_alpha: false, detail: 1.0 };
            model.render(&ctx, floats, &view, FORMAT);
            out = crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, W, W);
        }
        out
    }

    const TIMES: [f32; 4] = [0.0, 0.1, 0.2, 0.3];

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
        assert_eq!(FLOATS.len(), F_TAM + 1);
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
        let reference = frames(&gpu, &mut Ondas::new(), &b, &TIMES);
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
            // The slit floats only exist on the slit wall: compare on it.
            if name.starts_with("slit") {
                let mut wall = b.clone();
                wall[F_FORMA] = 3.0;
                r = frames(&gpu, &mut Ondas::new(), &wall, &TIMES);
                f = wall;
            }
            f[i] = far;
            let got = frames(&gpu, &mut Ondas::new(), &f, &TIMES);
            assert!(r != got, "float `{name}` ({} -> {far}) did not move the frame", b[i]);
        }
    }

    #[test]
    fn the_defaults_are_not_flat_and_hold_still_without_time() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let mut m = Ondas::new();
        let a = frames(&gpu, &mut m, &f, &[1.0]);
        let again = frames(&gpu, &mut m, &f, &[1.0]);
        assert_eq!(a, again, "the same instant twice must not step the field");
        let mut distinct = std::collections::HashSet::new();
        for px in a.chunks_exact(4) {
            distinct.insert([px[0], px[1], px[2]]);
        }
        assert!(distinct.len() > 16, "default frame is flat ({} colours)", distinct.len());
        let later = frames(&gpu, &mut m, &f, &[1.1]);
        assert_ne!(a, later, "a tenth of a second later the waves must have moved");
    }

    #[test]
    fn deterministic_and_a_seek_or_resize_reseeds() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let a = frames(&gpu, &mut Ondas::new(), &f, &TIMES);
        let b = frames(&gpu, &mut Ondas::new(), &f, &TIMES);
        assert_eq!(a, b);
        let mut m = Ondas::new();
        let first = frames(&gpu, &mut m, &f, &TIMES);
        assert_eq!(first, a);
        // Rewinding to the start gives the first frame of a fresh instance.
        let fresh = frames(&gpu, &mut Ondas::new(), &f, &[0.0]);
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
                    _ => if i % 2 == 0 { s.max } else { s.min },
                };
            }
            f[F_PULSO] = 1.0;
            rows.push(f);
        }
        rows
    }

    #[test]
    fn the_courant_number_is_always_clamped_in_rust() {
        for c in [0.0, 0.1, 0.5, 0.9, 5.0, f32::NAN, f32::INFINITY, -1.0] {
            for v in [0.0, 0.2, 1.0, 1.4, 50.0, f32::NAN, f32::INFINITY, -3.0] {
                let s = effective_courant(c, v);
                assert!(s > 0.0 && s <= MAX_COURANT, "courant {c} c_vel {v} gave {s}");
            }
        }
        for row in extreme_rows() {
            let p = Params::from_floats(&row, 96, 96, 0.0, None);
            assert!(p.s <= MAX_COURANT);
        }
    }

    #[test]
    fn the_cpu_reference_stays_finite_and_bounded_with_extreme_floats() {
        for (r, row) in extreme_rows().iter().enumerate() {
            for tipo in [0.0, 1.0, 2.0] {
                let mut row = row.clone();
                row[F_TIPO] = tipo;
                let mut sim = Fdtd::new(96, 96);
                let mut t = 0.0;
                let mut peak = 0.0f32;
                for step in 0..800 {
                    let pulse = if step >= 100 { Some((10.0, 1.0)) } else { None };
                    let p = Params::from_floats(&row, 96, 96, t, pulse);
                    sim.step(&p);
                    t += p.s;
                    peak = peak.max(sim.energy());
                }
                assert!(sim.finite(), "row {r} tipo {tipo}: non-finite field");
                assert!(peak.is_finite() && peak < 5.0e6, "row {r} tipo {tipo}: energy {peak}");
            }
        }
    }

    #[test]
    fn the_border_absorbs_and_a_mirror_box_does_not() {
        let run = |cond: f32| {
            let mut row = SCHEMA.defaults();
            row[F_TIPO] = 2.0;
            row[F_COND] = cond;
            row[F_EPS] = 1.0;
            let mut sim = Fdtd::new(96, 96);
            let mut t = 0.0;
            for _ in 0..900 {
                let p = Params::from_floats(&row, 96, 96, t, None);
                sim.step(&p);
                t += p.s;
            }
            // The source rings out after ~14 periods only for the pulsed types
            // between repeats; compare late energy.
            sim.energy()
        };
        let open = run(1.0);
        let closed = run(0.0);
        assert!(open < closed, "absorbing border left {open}, mirror box {closed}");
    }

    fn read_f16(gpu: &crate::gpu::Gpu, texture: &wgpu::Texture, w: u32, h: u32) -> Vec<f32> {
        let bpr = (w * 8).next_multiple_of(256);
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ondas readback"),
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
    fn the_gpu_frame_stays_finite_after_many_steps_with_extreme_floats() {
        let gpu = crate::gpu::Gpu::new_headless();
        let fmt = wgpu::TextureFormat::Rgba16Float;
        for (r, row) in extreme_rows().iter().enumerate() {
            for mode in [0.0, 2.0, 3.0] {
                let mut row = row.clone();
                row[F_MODO] = mode;
                let mut m = Ondas::new();
                let (tex, view) = crate::gpu::offscreen_target(&gpu.device, W, W, fmt);
                for k in 0..40 {
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
            }
        }
    }
}

