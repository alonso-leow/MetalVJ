//! Particulas — an N-body particle system after rebound (GPL-3). Written from
//! the algorithm; no code was taken.
//!
//! Up to 65536 bodies orbit a central softened mass, advanced by a fixed-step
//! symplectic leapfrog in a compute pass (`shaders/particulas_step.wgsl`): the
//! merged kick-drift form of velocity Verlet, the same family as rebound's
//! `leapfrog`/`whfast` integrators. On top of the central force the step adds
//! an attractor (a point mass on a prescribed orbit), self gravity from a
//! sampled tile of sources, soft collisions with restitution against one
//! representative body per grid cell, and, as an option, the shearing-sheet
//! boundary of rebound's `boundary.c` (the local Hill frame with the sheared
//! periodic wrap) for a patch of ring. Bodies are drawn as instanced
//! capsule sprites with the CAM3D camera.
//!
//! **Everything one step reads comes from the previous buffers** (two
//! ping-pong pairs), and the only cross-body structures are order independent
//! (the hashed tile, an `atomicMax` grid), so the state is a deterministic
//! function of the seed and the number of steps.
//!
//! **State advances by time**, like Ondas and Nubes: a fractional step rate is
//! carried, the same instant twice steps nothing, and a resize, a seek, a
//! rewind or a change of any initial-condition float reseeds. A reseed needs no
//! burn-in: the initial state is already a disc with winding arms.
//!
//! **Stability is enforced in Rust and in the shader, not by the ranges.**
//! Every float is cleaned (NaN reads as the default) and clamped; the step is
//! capped to a fraction of the dynamical time of the inner edge; the shader
//! caps speed and acceleration, and respawns a body that leaves the kill
//! radius, falls into the core or goes non-finite. The CPU reference
//! ([`central_accel`], [`leapfrog_step`]) is the same integrator in Rust, to
//! assert energy conservation.

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::model::{color_at, background, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::fullscreen::VERTEX;
use crate::shared::kit::{self, CAM3D_BLOCK, LOOK_BLOCK};
use crate::shared::{build_shader, ColorSlotGpu};

const FLOATS: &[FloatSpec] = &[
    // Bodies drawn and simulated, in steps of 4096. Structural.
    spec_held("n_part", 16384.0, 65536.0, 32768.0, 0.25),
    // Central mass and the gravitational constant: GM = masa_central * g.
    spec("mass_central", 0.1, 4.0, 1.0),
    spec("g", 0.1, 4.0, 1.0),
    // Plummer softening of the central force and of the self gravity.
    spec("softening", 0.005, 0.5, 0.04),
    // The fixed step. Structural (capped to the inner dynamical time).
    spec_held("dt", 0.0005, 0.02, 0.006, 0.25),
    // Simulation speed: steps per second are 120 * vel_sim.
    spec("speed_simulation", 0.0, 6.0, 1.0),
    // Velocity dispersion of the initial disc. Structural.
    spec_held("chaos", 0.0, 1.0, 0.08, 0.25),
    // Disc radius (the patch half-size is 1.2x it in sheet mode). Structural.
    spec_held("disc_radius", 0.4, 2.0, 1.0, 0.25),
    // Vertical thickness of the initial disc. Structural.
    spec_held("disc_thickness", 0.0, 1.0, 0.3, 0.25),
    // Tangential speed as a fraction of circular; negative is retrograde,
    // 0 is a radial infall. Structural.
    spec_held("disc_turn", -1.5, 1.5, 1.0, 0.25),
    // The rotation-curve shear q: Omega ~ r^-q. 1.5 is Keplerian, 0.6 is
    // nearly flat. In sheet mode it is the shear rate of the Hill frame.
    spec_held("shear", 0.6, 2.5, 1.5, 0.25),
    // 0 open disc around the central mass, 1 shearing-sheet patch. Structural.
    spec_held("boundary_mode", 0.0, 1.0, 0.0, 0.2),
    // Collision stiffness; 0 switches collisions off.
    spec("collision", 0.0, 1.0, 0.15),
    // Normal restitution of a collision: 0 sticky, 1 elastic.
    spec("restitution", 0.0, 1.0, 0.6),
    // Attractor position, in disc radii (half-patch units in sheet mode).
    spec("attractor_x", -2.0, 2.0, 1.25),
    spec("attractor_y", -2.0, 2.0, 0.0),
    // Attractor mass as a fraction of the central mass.
    spec("attractor_mass", 0.0, 1.0, 0.02),
    // How fast the attractor orbits, 1 = on a circular orbit (with the local
    // flow in sheet mode), 0 = held still, negative = the other way.
    spec("attractor_orbit", -2.0, 2.0, 1.0),
    // Total mass of the disc (self gravity), as a fraction of the central one.
    spec("selfgravity", 0.0, 0.5, 0.03),
    // Seed of the initial conditions. Structural; a change restarts.
    spec_held("seed", 0.0, 1.0, 0.37, 0.0),
    // Sprite radius, its spread per body, and a kick that swells the fast ones.
    spec("size", 0.2, 4.0, 1.0),
    spec("size_variation", 0.0, 1.0, 0.4),
    spec("size_audio", 0.0, 2.0, 0.0),
    // Spread of the brightness per body.
    spec("shine_variation", 0.0, 1.0, 0.4),
    // Tail brightness and length (along the projected velocity).
    spec("wake", 0.0, 1.0, 0.5),
    spec("wake_long", 0.0, 2.0, 0.5),
    // 0 dot, 1 ring, 2 streak, 3 glow. Structural.
    spec_held("texture", 0.0, 3.0, 0.0, 0.2),
    // Colour by radius (0) or by speed (1).
    spec("speed_color", 0.0, 1.0, 0.35),
    // Per-body twinkle.
    spec("flash", 0.0, 1.0, 0.25),
];

/// Hot gold at the inner edge, through rose and violet to a cold blue rim.
const PARTICLE: [f32; 12] = [1.00, 0.78, 0.40, 1.00, 0.30, 0.40, 0.55, 0.20, 0.95, 0.12, 0.42, 1.00];
/// Deep night, brightening a little towards the bottom.
const NIGHT: [f32; 12] = [0.004, 0.005, 0.016, 0.006, 0.008, 0.024, 0.008, 0.010, 0.030, 0.012, 0.014, 0.040];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("particle", &PARTICLE),
        color_at("background", &NIGHT),
        CAM3D_BLOCK,
        LOOK_BLOCK,
        crate::model::SIGNAL_FLOATS_BLOCK,
        crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

const B_OWN: usize = 0;
const B_PARTICLE: usize = 1;
const B_BG: usize = 2;
const B_CAM: usize = 3;
const B_LOOK: usize = 4;

fn off(block: usize) -> usize {
    kit::offset_of(SCHEMA.blocks, block)
}

const F_N: usize = 0;
const F_CENTRAL_MASS: usize = 1;
const F_G: usize = 2;
const F_EPS: usize = 3;
const F_DT: usize = 4;
const F_SIM_SPEED: usize = 5;
const F_CHAOS: usize = 6;
const F_DISC_RADIUS: usize = 7;
const F_DISC_THICKNESS: usize = 8;
const F_DISC_SPIN: usize = 9;
const F_SHEAR: usize = 10;
const F_BOUNDARY_MODE: usize = 11;
const F_COLLISION: usize = 12;
const F_RESTITUTION: usize = 13;
const F_AX: usize = 14;
const F_AY: usize = 15;
const F_ATTRACTOR_MASS: usize = 16;
const F_ATTRACTOR_ORBIT: usize = 17;
const F_AUTOGRAV: usize = 18;
const F_SEED: usize = 19;
const F_SIZE: usize = 20;
const F_SIZE_VAR: usize = 21;
const F_SIZE_AUDIO: usize = 22;
const F_BRIGHT_VAR: usize = 23;
const F_TRAIL: usize = 24;
const F_TRAIL_LEN: usize = 25;
const F_TEXTURE: usize = 26;
const F_SPEED_COLOR: usize = 27;
const F_TWINKLE: usize = 28;

// -- constants, mirrored in shaders/particulas_step.wgsl -------------------

const MAX_N: u32 = 65536;
const GRID: u32 = 256;
/// Half-size of the square the collision grid covers (the kill radius of the
/// largest disc, 4 x 2.0, is outside it on purpose: those bodies do not collide).
const GRID_EXT: f32 = 3.2;
const TILE: f32 = 64.0;
/// Steps per second at `vel_sim` 1.
const STEPS_PER_SECOND: f32 = 120.0;
/// A stall arrives as a slowdown, not a jump.
const MAX_STEPS_PER_FRAME: u32 = 64;
const SEEK_GAP: f32 = 60.0;
/// The scene is drawn this much larger than the simulation, so the default
/// camera frames the disc.
const WORLD_SCALE: f32 = 1.6;

fn clean(row: &[f32], i: usize) -> f32 {
    let s = FLOATS[i];
    let v = row.get(i).copied().filter(|v| v.is_finite()).unwrap_or(s.default);
    v.clamp(s.min, s.max)
}

/// The bodies simulated for a requested `n_part`: a multiple of 4096.
pub fn quantised_n(n: f32) -> u32 {
    let n = if n.is_finite() { n } else { FLOATS[F_N].default };
    (((n / 4096.0).round() * 4096.0).clamp(16384.0, MAX_N as f32)) as u32
}

// ---------------------------------------------------------------------------
// the CPU reference of the integrator
// ---------------------------------------------------------------------------

/// The central force `-GM x / (|x|^2 + eps^2)^(p/2)`. `p` = 3 is Newtonian
/// gravity with Plummer softening; the shader evaluates the same expression.
pub fn central_accel(p: Vec3, gm: f32, eps: f32, pexp: f32) -> Vec3 {
    -gm * p * (p.length_squared() + eps * eps).powf(-0.5 * pexp)
}

/// One step of the merged leapfrog the shader runs: `v` is the half-step
/// velocity, so `v' = v + a(x) dt` then `x' = x + v' dt`.
pub fn leapfrog_step(p: &mut Vec3, v: &mut Vec3, dt: f32, acc: impl Fn(Vec3) -> Vec3) {
    *v += acc(*p) * dt;
    *p += *v * dt;
}

/// The half-step velocity that makes the first step land on `v0`.
pub fn start_velocity(p: Vec3, v0: Vec3, dt: f32, acc: impl Fn(Vec3) -> Vec3) -> Vec3 {
    v0 - 0.5 * dt * acc(p)
}

/// Specific energy with the Plummer potential of the `p` = 3 law.
pub fn energy(p: Vec3, v: Vec3, gm: f32, eps: f32) -> f32 {
    0.5 * v.length_squared() - gm / (p.length_squared() + eps * eps).sqrt()
}

// ---------------------------------------------------------------------------
// parameters
// ---------------------------------------------------------------------------

/// Everything the step and the draw need, from a float row.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub n: u32,
    pub dt: f32,
    pub gm: f32,
    pub eps: f32,
    pub pexp: f32,
    pub q: f32,
    pub omega: f32,
    pub sheet: bool,
    pub radius: f32,
    pub thick: f32,
    pub spin: f32,
    pub chaos: f32,
    pub vref: f32,
    pub att: [f32; 2],
    pub att_gm: f32,
    pub att_orbit: f32,
    pub att_eps: f32,
    pub collision: f32,
    pub kcol: f32,
    pub rest: f32,
    pub pair_w: f32,
    pub vmax: f32,
    pub kill: f32,
    pub half: f32,
    pub accel_cap: f32,
    pub steps_per_second: f32,
    pub seed: u32,
}

impl Params {
    pub fn from_floats(own: &[f32]) -> Params {
        let c = |i| clean(own, i);
        let sheet = c(F_BOUNDARY_MODE) >= 0.5;
        let radius = c(F_DISC_RADIUS);
        let gm = c(F_CENTRAL_MASS) * c(F_G);
        let eps = c(F_EPS);
        let q = c(F_SHEAR);
        let pexp = 2.0 * q;
        let half = 1.2 * radius;
        let omega = gm.sqrt();
        // The dynamical time of the fastest part that matters.
        let tdyn = if sheet { 1.0 / omega } else { ((0.3 * radius).powi(3) / gm).sqrt() };
        let dt = c(F_DT).min(0.15 * tdyn).max(1.0e-5);
        let circ2 = |r: f32| gm * r * r * (r * r + eps * eps).powf(-0.5 * pexp);
        let vref = if sheet { omega * half } else { circ2(radius).sqrt() };
        let cs = 2.0 * GRID_EXT / GRID as f32;
        let d0 = 0.75 * cs;
        let kcol = (c(F_COLLISION) * 0.25 * vref * vref / (d0 * d0)).min(0.3 / (dt * dt));
        let vmax = if sheet {
            4.0 * q.max(1.0) * omega * (half + radius)
        } else {
            3.5 * (gm / (0.3 * radius)).sqrt()
        };
        let accel_cap = if sheet {
            30.0 * gm * half.max(1.0)
        } else {
            30.0 * gm / (0.3 * radius).powi(2)
        };
        Params {
            n: quantised_n(c(F_N)),
            dt,
            gm,
            eps,
            pexp,
            q,
            omega,
            sheet,
            radius,
            thick: c(F_DISC_THICKNESS),
            spin: c(F_DISC_SPIN),
            chaos: c(F_CHAOS),
            vref,
            att: [c(F_AX), c(F_AY)],
            att_gm: c(F_ATTRACTOR_MASS) * gm,
            att_orbit: c(F_ATTRACTOR_ORBIT),
            att_eps: 1.5 * eps + 0.01,
            collision: c(F_COLLISION),
            kcol,
            rest: c(F_RESTITUTION),
            pair_w: c(F_AUTOGRAV) * gm / TILE,
            vmax,
            kill: 4.0 * radius,
            half,
            accel_cap,
            steps_per_second: STEPS_PER_SECOND * c(F_SIM_SPEED),
            seed: (c(F_SEED) * 1000.0).round() as u32,
        }
    }

    /// The attractor at simulation time `t` (f64: a long set must not lose it).
    pub fn attractor_at(&self, t: f64) -> [f32; 3] {
        use std::f64::consts::TAU;
        if self.sheet {
            let h = self.half as f64;
            let x = self.att[0] as f64 * 0.5 * h;
            let y0 = self.att[1] as f64 * 0.5 * h;
            let y = y0 - self.att_orbit as f64 * self.q as f64 * self.omega as f64 * x * t;
            let y = y - 2.0 * h * ((y + h) / (2.0 * h)).floor();
            return [x as f32, y as f32, 0.0];
        }
        let (ax, ay) = (self.att[0] as f64 * self.radius as f64, self.att[1] as f64 * self.radius as f64);
        let r = ax.hypot(ay);
        if r < 1.0e-4 {
            return [0.0; 3];
        }
        let (gm, eps, pexp) = (self.gm as f64, self.eps as f64, self.pexp as f64);
        let om = (gm * (r * r + eps * eps).powf(-0.5 * pexp)).sqrt();
        let ang = (ay.atan2(ax) + self.att_orbit as f64 * om * t).rem_euclid(TAU);
        [(r * ang.cos()) as f32, (r * ang.sin()) as f32, 0.0]
    }

    /// The sheared-wrap offset of the sheet at time `t`.
    pub fn shear_offset(&self, t: f64) -> f32 {
        let l = 2.0 * self.half as f64;
        (self.q as f64 * self.omega as f64 * l * t).rem_euclid(l) as f32
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct StepUniform {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    e: [f32; 4],
    f: [f32; 4],
    g: [f32; 4],
    h: [f32; 4],
    ints: [u32; 4],
}

impl StepUniform {
    fn new(p: &Params, t: f64, counter: u32) -> StepUniform {
        let at = p.attractor_at(t);
        StepUniform {
            a: [p.dt, p.gm, p.eps, p.pexp],
            b: [p.q, p.omega, if p.sheet { 1.0 } else { 0.0 }, p.radius],
            c: [p.thick, p.spin, p.chaos, p.vref],
            d: [at[0], at[1], at[2], p.att_gm],
            e: [p.kcol, p.rest, p.pair_w, 2.0 * GRID_EXT / GRID as f32],
            f: [p.vmax, p.kill, p.half, p.shear_offset(t)],
            g: [GRID_EXT, p.att_eps, p.eps.max(0.01), p.accel_cap],
            h: [p.collision, 0.0, 0.0, 0.0],
            ints: [p.n, p.seed, counter, 0],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawUniform {
    vp: [[f32; 4]; 4],
    particle: ColorSlotGpu,
    background: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    e: [f32; 4],
    f: [f32; 4],
}

/// Eye and view-projection from the CAM3D kit. Deterministic in `time`.
fn view_proj(c: &kit::Cam3d, time: f32, aspect: f32) -> (Mat4, Vec3) {
    let az = c.yaw + c.orbit * time;
    let distance = (c.distance + c.dolly).max(0.25);
    let (sp, cp) = c.pitch.clamp(-1.5, 1.5).sin_cos();
    let shake = c.shake.clamp(0.0, 1.0) * 0.06;
    let tremor = Vec3::new(
        (time * 47.0).sin() + (time * 31.0 + 1.0).sin(),
        (time * 61.0 + 2.0).sin() + (time * 37.0).sin(),
        (time * 53.0 + 4.0).sin(),
    ) * shake;
    let eye = Vec3::new(distance * cp * az.sin(), distance * sp + c.alt, distance * cp * az.cos()) + tremor;
    let fwd = (-eye).try_normalize().unwrap_or(Vec3::NEG_Z);
    let right = fwd.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
    let up = right.cross(fwd);
    let (sr, cr) = c.roll.sin_cos();
    let up = up * cr - right * sr;
    let view = Mat4::look_at_rh(eye, eye + fwd, up);
    let proj = Mat4::perspective_rh(c.fov.clamp(0.3, 2.2), aspect.max(1.0e-3), 0.05, 100.0);
    (proj * view, eye)
}

// ---------------------------------------------------------------------------
// GPU state
// ---------------------------------------------------------------------------

/// Everything that does not depend on the target format: the body buffers,
/// the grid and the compute pipelines. Kept for the model's life.
struct Core {
    pos: [wgpu::Buffer; 2],
    speed: [wgpu::Buffer; 2],
    grid: wgpu::Buffer,
    sim_bgl: wgpu::BindGroupLayout,
    draw_bgl: wgpu::BindGroupLayout,
    seed: wgpu::ComputePipeline,
    clear: wgpu::ComputePipeline,
    build: wgpu::ComputePipeline,
    step: wgpu::ComputePipeline,
}

struct Draw {
    bg: wgpu::RenderPipeline,
    sprites: wgpu::RenderPipeline,
}

type Key = (u32, u32, bool, i32, i32, i32, i32, u32, u32);

#[derive(Default)]
struct Sim {
    wall: Option<f32>,
    key: Key,
    carry: f32,
    /// Simulation time in f64, and the step counter (the hash salt).
    t: f64,
    counter: u32,
    /// Which buffer pair holds the current state.
    cur: usize,
}

pub struct Particulas {
    core: Option<Core>,
    draw: Built<Draw>,
    sim: Sim,
}

fn draw_src() -> &'static str {
    static SRC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SRC.get_or_init(|| format!("{VERTEX}\n{}\n{}", kit::WGSL, include_str!("../../shaders/particulas.wgsl")))
}

impl Particulas {
    pub fn new() -> Particulas {
        Particulas { core: None, draw: Built::default(), sim: Sim::default() }
    }

    fn build_core(device: &wgpu::Device) -> Core {
        let shader = build_shader(device, "particles step", include_str!("../../shaders/particulas_step.wgsl"));
        let body = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (MAX_N * 16) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let pos = [body("particles pos a"), body("particles pos b")];
        let speed = [body("particles vel a"), body("particles vel b")];
        let grid = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particles grid"),
            size: (GRID * GRID * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let storage = |binding: u32, read_only: bool, visibility: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let uniform = |binding: u32, visibility: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let c = wgpu::ShaderStages::COMPUTE;
        let sim_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particles sim bgl"),
            entries: &[
                uniform(0, c),
                storage(1, true, c),
                storage(2, true, c),
                storage(3, false, c),
                storage(4, false, c),
                storage(5, false, c),
            ],
        });
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let draw_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particles draw bgl"),
            entries: &[uniform(0, vf), storage(1, true, wgpu::ShaderStages::VERTEX), storage(2, true, wgpu::ShaderStages::VERTEX)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particles sim layout"),
            bind_group_layouts: &[Some(&sim_bgl)],
            immediate_size: 0,
        });
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Core {
            pos,
            speed,
            grid,
            sim_bgl,
            draw_bgl,
            seed: compute("cs_seed"),
            clear: compute("cs_clear"),
            build: compute("cs_build"),
            step: compute("cs_step"),
        }
    }

    fn build_draw(device: &wgpu::Device, format: wgpu::TextureFormat, core: &Core) -> Draw {
        let shader = build_shader(device, "particles draw", draw_src());
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particles draw layout"),
            bind_group_layouts: &[Some(&core.draw_bgl)],
            immediate_size: 0,
        });
        let make = |label: &str, vs: &str, fs: &str, blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        Draw {
            bg: make("particles bg", "vs_main", "fs_bg", wgpu::BlendState::REPLACE),
            sprites: make("particles sprites", "vs_p", "fs_p", wgpu::BlendState { color: add, alpha: add }),
        }
    }

    /// The reseed decision and the number of steps owed (Nubes' schedule).
    fn schedule(&mut self, time: f32, p: &Params, key: Key) -> (bool, u32) {
        let reseed = match self.sim.wall {
            None => true,
            Some(prev) => key != self.sim.key || !(0.0..=SEEK_GAP).contains(&(time - prev)),
        };
        let dt = self.sim.wall.map_or(0.0, |prev| (time - prev).clamp(0.0, 0.1));
        self.sim.wall = Some(time);
        self.sim.key = key;
        if reseed {
            self.sim.carry = 0.0;
            self.sim.t = 0.0;
            self.sim.counter = 0;
            return (true, 0);
        }
        self.sim.carry += p.steps_per_second * dt;
        let whole = self.sim.carry.floor();
        self.sim.carry -= whole;
        (false, (whole.max(0.0) as u32).min(MAX_STEPS_PER_FRAME))
    }

    /// The bodies as `[x, y, z, vx, vy, vz]`, read back (tests and probes).
    pub fn read_state(&self, device: &wgpu::Device, queue: &wgpu::Queue, n: usize) -> Vec<[f32; 6]> {
        let Some(core) = &self.core else { return Vec::new() };
        let bytes = (n * 16) as u64;
        let mk = || {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("particles readback"),
                size: bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        };
        let (rp, rv) = (mk(), mk());
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("particles readback") });
        enc.copy_buffer_to_buffer(&core.pos[self.sim.cur], 0, &rp, 0, bytes);
        enc.copy_buffer_to_buffer(&core.speed[self.sim.cur], 0, &rv, 0, bytes);
        queue.submit(Some(enc.finish()));
        let read = |b: &wgpu::Buffer| -> Vec<f32> {
            let slice = b.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).expect("readback channel"));
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("poll");
            rx.recv().expect("readback").expect("map");
            let data = slice.get_mapped_range().expect("range");
            let v = bytemuck::cast_slice::<u8, f32>(&data).to_vec();
            drop(data);
            b.unmap();
            v
        };
        let (pv, vv) = (read(&rp), read(&rv));
        (0..n).map(|i| [pv[i * 4], pv[i * 4 + 1], pv[i * 4 + 2], vv[i * 4], vv[i * 4 + 1], vv[i * 4 + 2]]).collect()
    }
}

impl Default for Particulas {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Particulas {
    fn needs_compute(&self) -> bool {
        true
    }

    fn id(&self) -> &'static str {
        "particles"
    }

    fn name(&self) -> &'static str {
        "Particulas"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn render(&mut self, ctx: &Ctx, floats: &[f32], target: &wgpu::TextureView, format: wgpu::TextureFormat) {
        let own = &floats[off(B_OWN)..off(B_OWN) + FLOATS.len()];
        let p = Params::from_floats(own);
        let time = if ctx.time.is_finite() { ctx.time } else { 0.0 };
        let key: Key = (
            p.n,
            p.seed,
            p.sheet,
            (p.radius * 20.0).round() as i32,
            (p.thick * 20.0).round() as i32,
            (p.spin * 8.0).round() as i32,
            (p.chaos * 20.0).round() as i32,
            ctx.width,
            ctx.height,
        );
        let (reseed, steps) = self.schedule(time, &p, key);

        let core = self.core.get_or_insert_with(|| Self::build_core(ctx.device));
        let draw = self.draw.get(format, || Self::build_draw(ctx.device, format, core));

        let mut enc = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("particles") });
        let groups = p.n.div_ceil(64);
        let sim_bg = |k: usize, u: &StepUniform| {
            let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("particles step uniform"),
                contents: bytemuck::bytes_of(u),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("particles sim bg"),
                layout: &core.sim_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: core.pos[k].as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: core.speed[k].as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: core.pos[1 - k].as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: core.speed[1 - k].as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: core.grid.as_entire_binding() },
                ],
            })
        };
        let mut dispatch = |pipeline: &wgpu::ComputePipeline, bind: &wgpu::BindGroup, g: u32| {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("particles"), timestamp_writes: None });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.dispatch_workgroups(g, 1, 1);
        };
        if reseed {
            // Written into buffer pair 0 (in = pair 1, out = pair 0).
            let bg = sim_bg(1, &StepUniform::new(&p, 0.0, 0));
            dispatch(&core.seed, &bg, groups);
            self.sim.cur = 0;
        }
        for _ in 0..steps {
            let bg = sim_bg(self.sim.cur, &StepUniform::new(&p, self.sim.t, self.sim.counter));
            dispatch(&core.clear, &bg, (GRID * GRID).div_ceil(64));
            dispatch(&core.build, &bg, groups);
            dispatch(&core.step, &bg, groups);
            self.sim.cur = 1 - self.sim.cur;
            self.sim.t += p.dt as f64;
            self.sim.counter = self.sim.counter.wrapping_add(1);
        }

        let cam = kit::Cam3d::from_floats(&floats[off(B_CAM)..off(B_CAM) + kit::Cam3d::N]);
        let look = kit::Look::from_floats(&floats[off(B_LOOK)..off(B_LOOK) + kit::Look::N]);
        let (vp, eye) = view_proj(&cam, time, ctx.aspect());
        let slot = |blk: usize| ColorSlotGpu::from_floats(&floats[off(blk)..off(blk) + 12]);
        let c = |i| clean(own, i);
        let extent = if p.sheet { p.half * 1.2 } else { p.radius * 1.15 };
        let uniform = DrawUniform {
            vp: vp.to_cols_array_2d(),
            particle: slot(B_PARTICLE),
            background: slot(B_BG),
            a: [ctx.aspect(), time, if ctx.fondo_alpha { 1.0 } else { 0.0 }, ctx.height.max(1) as f32],
            b: [c(F_SIZE), c(F_SIZE_VAR), c(F_SIZE_AUDIO), c(F_BRIGHT_VAR)],
            c: [c(F_TRAIL), c(F_TRAIL_LEN), c(F_TEXTURE).round(), c(F_SPEED_COLOR)],
            d: [c(F_TWINKLE), p.vref, 1.0 / extent, WORLD_SCALE],
            e: [look.shine, look.contrast, look.opacity, look.background_level],
            f: [look.tone, look.saturation, 0.9, eye.length()],
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("particles draw uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let draw_bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particles draw bg"),
            layout: &core.draw_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: core.pos[self.sim.cur].as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: core.speed[self.sim.cur].as_entire_binding() },
            ],
        });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("particles draw"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &draw_bg, &[]);
            pass.set_pipeline(&draw.bg);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&draw.sprites);
            pass.draw(0..6, 0..p.n);
        }
        ctx.queue.submit(Some(enc.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
    const W: u32 = 64;

    fn frame_at(gpu: &crate::gpu::Gpu, m: &mut Particulas, floats: &[f32], time: f32, w: u32, h: u32) -> Vec<u8> {
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time, width: w, height: h, fondo_alpha: false };
        m.render(&ctx, floats, &view, FORMAT);
        crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, w, h)
    }

    /// Steps the clock a tenth of a second at a time up to `until`, and
    /// returns the last frame.
    fn run(gpu: &crate::gpu::Gpu, m: &mut Particulas, floats: &[f32], until: f32) -> Vec<u8> {
        let frames = (until / 0.1).round() as u32;
        let mut last = Vec::new();
        for k in 0..=frames {
            last = frame_at(gpu, m, floats, k as f32 * 0.1, W, W);
        }
        last
    }

    fn fresh(gpu: &crate::gpu::Gpu, floats: &[f32], until: f32) -> Vec<u8> {
        run(gpu, &mut Particulas::new(), floats, until)
    }

    fn base() -> Vec<f32> {
        let mut f = SCHEMA.defaults();
        for v in &mut f[off(B_BG)..off(B_BG) + 12] {
            *v = 0.3;
        }
        f
    }

    fn distinct(px: &[u8]) -> usize {
        let mut set = std::collections::HashSet::new();
        for p in px.chunks_exact(4) {
            set.insert([p[0], p[1], p[2]]);
        }
        set.len()
    }

    #[test]
    fn widths_and_indices_come_from_the_blocks() {
        assert_eq!(SCHEMA.defaults().len(), SCHEMA.width());
        assert_eq!(off(B_OWN), 0);
        assert_eq!(off(B_PARTICLE), FLOATS.len());
        assert_eq!(off(B_CAM) + kit::Cam3d::N, off(B_LOOK));
        assert_eq!(off(B_LOOK) + kit::Look::N, off(B_LOOK + 1));
        let n = |i: usize| FLOATS[i].name;
        assert_eq!((n(F_N), n(F_CENTRAL_MASS), n(F_G), n(F_EPS), n(F_DT), n(F_SIM_SPEED), n(F_CHAOS)), ("n_part", "mass_central", "g", "softening", "dt", "speed_simulation", "chaos"));
        assert_eq!((n(F_DISC_RADIUS), n(F_DISC_THICKNESS), n(F_DISC_SPIN), n(F_SHEAR), n(F_BOUNDARY_MODE)), ("disc_radius", "disc_thickness", "disc_turn", "shear", "boundary_mode"));
        assert_eq!((n(F_COLLISION), n(F_RESTITUTION), n(F_AX), n(F_AY), n(F_ATTRACTOR_MASS), n(F_ATTRACTOR_ORBIT), n(F_AUTOGRAV), n(F_SEED)), ("collision", "restitution", "attractor_x", "attractor_y", "attractor_mass", "attractor_orbit", "selfgravity", "seed"));
        assert_eq!((n(F_SIZE), n(F_SIZE_VAR), n(F_SIZE_AUDIO), n(F_BRIGHT_VAR), n(F_TRAIL), n(F_TRAIL_LEN), n(F_TEXTURE), n(F_SPEED_COLOR), n(F_TWINKLE)), ("size", "size_variation", "size_audio", "shine_variation", "wake", "wake_long", "texture", "speed_color", "flash"));
        assert_eq!(FLOATS.len(), F_TWINKLE + 1);
        for i in [F_N, F_DT, F_CHAOS, F_DISC_RADIUS, F_DISC_THICKNESS, F_DISC_SPIN, F_SHEAR, F_BOUNDARY_MODE, F_SEED, F_TEXTURE] {
            assert!(FLOATS[i].drive < 1.0, "{} should be held", FLOATS[i].name);
        }
        assert!(Particulas::new().needs_compute());
        assert!(!Particulas::new().camera_track());
    }

    #[test]
    fn a_two_body_circular_orbit_conserves_energy() {
        // GM = 1, r = 1, Newtonian law (p = 3): circular speed 1 (a little
        // under it with the softening, taken from the same formula).
        let (gm, eps, dt) = (1.0f32, 0.04f32, 0.006f32);
        let acc = |x: Vec3| central_accel(x, gm, eps, 3.0);
        let vc = (gm / (1.0 + eps * eps).powf(1.5)).sqrt();
        let mut p = Vec3::new(1.0, 0.0, 0.0);
        let v0 = Vec3::new(0.0, vc, 0.0);
        let mut v = start_velocity(p, v0, dt, acc);
        let e0 = energy(p, v0, gm, eps);
        let l0 = p.cross(v0).length();
        let (mut worst, mut worst_r) = (0.0f32, 0.0f32);
        // ~190 orbits.
        for _ in 0..200_000 {
            let before = v;
            leapfrog_step(&mut p, &mut v, dt, acc);
            let sync = 0.5 * (before + v);
            worst = worst.max(((energy(p, sync, gm, eps) - e0) / e0.abs()).abs());
            worst_r = worst_r.max((p.length() - 1.0).abs());
        }
        assert!(worst < 2e-3, "energy drifted by {worst}");
        assert!(worst_r < 2e-3, "radius wandered by {worst_r}");
        assert!((p.cross(v).length() - l0).abs() / l0 < 1e-3, "angular momentum drifted");
    }

    #[test]
    fn an_eccentric_orbit_conserves_energy_and_does_not_spiral() {
        let (gm, eps, dt) = (1.0f32, 0.02f32, 0.002f32);
        let acc = |x: Vec3| central_accel(x, gm, eps, 3.0);
        let mut p = Vec3::new(1.0, 0.0, 0.0);
        let v0 = Vec3::new(0.0, 0.6, 0.0);
        let mut v = start_velocity(p, v0, dt, acc);
        let e0 = energy(p, v0, gm, eps);
        let mut worst = 0.0f32;
        let mut rmin = f32::MAX;
        for _ in 0..200_000 {
            let before = v;
            leapfrog_step(&mut p, &mut v, dt, acc);
            rmin = rmin.min(p.length());
            // The fixed step's energy error is a phase-locked wobble that is
            // largest at the pericentre; judge it where the orbit is slow.
            if p.length() > 0.95 {
                worst = worst.max(((energy(p, 0.5 * (before + v), gm, eps) - e0) / e0.abs()).abs());
            }
        }
        assert!(worst < 2e-3, "energy drifted by {worst}");
        assert!(rmin > 0.2 && rmin < 0.4, "pericentre {rmin}");
    }

    #[test]
    fn params_stay_finite_and_in_bounds_for_any_row() {
        for pick in 0..6 {
            let own: Vec<f32> = FLOATS
                .iter()
                .enumerate()
                .map(|(i, s)| match pick {
                    0 => s.min,
                    1 => s.max,
                    2 => f32::NAN,
                    3 => f32::INFINITY,
                    4 => -1e30,
                    _ => if i % 2 == 0 { s.min } else { s.max },
                })
                .collect();
            let p = Params::from_floats(&own);
            for v in [p.dt, p.gm, p.eps, p.pexp, p.vref, p.kcol, p.vmax, p.kill, p.half, p.accel_cap, p.pair_w, p.steps_per_second] {
                assert!(v.is_finite() && v >= 0.0, "pick {pick}: {p:?}");
            }
            assert!(p.dt > 0.0 && p.n >= 16384 && p.n <= MAX_N && p.n % 4096 == 0);
            for t in [0.0, 1.0, 1e6, 1e9] {
                assert!(p.attractor_at(t).iter().all(|v| v.is_finite()));
                assert!(p.shear_offset(t).is_finite());
            }
        }
    }

    #[test]
    fn the_defaults_are_not_flat_deterministic_and_move() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let a = fresh(&gpu, &f, 1.0);
        assert_eq!(a, fresh(&gpu, &f, 1.0), "not deterministic");
        assert!(distinct(&a) > 16, "default frame is flat ({} colours)", distinct(&a));
        assert_ne!(a, fresh(&gpu, &f, 0.0), "the bodies must move with time");
    }

    #[test]
    fn time_standing_still_steps_nothing_and_a_rewind_reseeds() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let mut m = Particulas::new();
        let a = frame_at(&gpu, &mut m, &f, 2.0, W, W);
        assert_eq!(a, frame_at(&gpu, &mut m, &f, 2.0, W, W));
        let later = frame_at(&gpu, &mut m, &f, 2.1, W, W);
        assert_ne!(a, later);
        let n = quantised_n(f[F_N]) as usize;
        let before = m.read_state(&gpu.device, &gpu.queue, n);
        let rewound = frame_at(&gpu, &mut m, &f, 0.0, W, W);
        let after = m.read_state(&gpu.device, &gpu.queue, n);
        assert_ne!(before, after, "a rewind must start over");
        assert!(after.iter().all(|b| b.iter().all(|v| v.is_finite())));
        // The start of a run is the same as a fresh instance's first frame.
        assert_eq!(rewound, frame_at(&gpu, &mut Particulas::new(), &f, 0.0, W, W));
        // A resize reseeds too.
        let _ = frame_at(&gpu, &mut m, &f, 0.1, 96, 54);
        let resized = m.read_state(&gpu.device, &gpu.queue, n);
        let mut fresh_m = Particulas::new();
        let _ = frame_at(&gpu, &mut fresh_m, &f, 0.1, 96, 54);
        assert_eq!(resized, fresh_m.read_state(&gpu.device, &gpu.queue, n));
    }

    #[test]
    fn every_float_moves_the_frame() {
        let gpu = crate::gpu::Gpu::new_headless();
        let (cam, look) = (off(B_CAM), off(B_LOOK));
        let mut idx: Vec<(usize, FloatSpec)> = FLOATS.iter().enumerate().map(|(i, s)| (i, *s)).collect();
        idx.extend(kit::CAM3D.iter().enumerate().map(|(i, s)| (cam + i, *s)));
        idx.extend(kit::LOOK.iter().enumerate().map(|(i, s)| (look + i, *s)));
        let b = base();
        let reference = fresh(&gpu, &b, 1.0);
        for (i, spec) in idx {
            let far = if (spec.max - b[i]).abs() > (b[i] - spec.min).abs() { spec.max } else { spec.min };
            let far = if spec.name == "tone" { 0.5 } else { far };
            let mut f = b.clone();
            f[i] = far;
            assert!(reference != fresh(&gpu, &f, 1.0), "float `{}` ({} -> {far}) did not move the frame", spec.name, b[i]);
        }
    }

    #[test]
    fn extreme_floats_keep_the_state_finite_and_bounded() {
        let gpu = crate::gpu::Gpu::new_headless();
        let own_end = FLOATS.len();
        for mode in [0.0f32, 1.0] {
            for pick in 0..4 {
                let mut f = base();
                for (i, s) in FLOATS.iter().enumerate().take(own_end) {
                    f[i] = match pick {
                        0 => s.min,
                        1 => s.max,
                        2 => if i % 2 == 0 { s.min } else { s.max },
                        _ => if i % 2 == 0 { s.max } else { s.min },
                    };
                }
                f[F_BOUNDARY_MODE] = mode;
                f[F_N] = 16384.0; // keep the test quick; the cap is independent of n
                // Out of range, too.
                if pick == 3 {
                    f[F_CENTRAL_MASS] = 1e9;
                    f[F_DT] = 1e9;
                    f[F_EPS] = -5.0;
                    f[F_AX] = f32::NAN;
                }
                let p = Params::from_floats(&f[..own_end]);
                let mut m = Particulas::new();
                let mut last = Vec::new();
                // 3 simulated seconds at up to 6x: hundreds of steps.
                for k in 0..30 {
                    last = frame_at(&gpu, &mut m, &f, k as f32 * 0.1, 32, 32);
                }
                assert!(last.len() == 32 * 32 * 4);
                let st = m.read_state(&gpu.device, &gpu.queue, p.n as usize);
                let bound = if p.sheet { 1.6 * p.half + 8.0 * p.half } else { p.kill * 1.2 + p.vmax * p.dt * 2.0 };
                let vbound = p.vmax * 1.05 + p.half * p.q * p.omega * 2.0;
                for (j, b) in st.iter().enumerate() {
                    assert!(b.iter().all(|v| v.is_finite()), "mode {mode} pick {pick}: body {j} = {b:?}");
                    let r = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt();
                    assert!(r <= bound, "mode {mode} pick {pick}: body {j} at {r} > {bound}");
                    let s = (b[3] * b[3] + b[4] * b[4] + b[5] * b[5]).sqrt();
                    assert!(s <= vbound, "mode {mode} pick {pick}: body {j} speed {s} > {vbound}");
                }
            }
        }
    }

    #[test]
    fn the_shader_conserves_energy_and_angular_momentum_per_body() {
        // With collisions, self gravity, the attractor and the dispersion off,
        // every body is a test particle in the central field: the shader's
        // leapfrog must keep each one's energy and |L| (the CPU reference's
        // invariants), except the few that wander into the respawn core.
        let gpu = crate::gpu::Gpu::new_headless();
        let mut f = SCHEMA.defaults();
        f[F_COLLISION] = 0.0;
        f[F_AUTOGRAV] = 0.0;
        f[F_ATTRACTOR_MASS] = 0.0;
        f[F_CHAOS] = 0.0;
        f[F_N] = 16384.0;
        let p = Params::from_floats(&f[..FLOATS.len()]);
        let mut m = Particulas::new();
        let _ = frame_at(&gpu, &mut m, &f, 0.0, 16, 16);
        let n = p.n as usize;
        let s0 = m.read_state(&gpu.device, &gpu.queue, n);
        for k in 1..=40 {
            let _ = frame_at(&gpu, &mut m, &f, k as f32 * 0.1, 16, 16);
        }
        let s1 = m.read_state(&gpu.device, &gpu.queue, n);
        let steps = (m.sim.counter) as f32;
        assert!(steps > 400.0, "only {steps} steps ran");
        let ev = |b: &[f32; 6], half_kick: bool| {
            let pos = Vec3::new(b[0], b[1], b[2]);
            let mut speed = Vec3::new(b[3], b[4], b[5]);
            if half_kick {
                speed += 0.5 * p.dt * central_accel(pos, p.gm, p.eps, p.pexp);
            }
            (energy(pos, speed, p.gm, p.eps), pos.cross(speed).length())
        };
        let (mut bad, mut checked) = (0, 0);
        for (a, b) in s0.iter().zip(&s1) {
            // The seed stored the half-step-back velocity; synchronise both.
            let (e0, l0) = ev(a, true);
            let (e1, l1) = ev(b, true);
            checked += 1;
            if ((e1 - e0) / e0.abs()).abs() > 5e-3 || ((l1 - l0) / l0.max(1e-3)).abs() > 5e-3 {
                bad += 1;
            }
        }
        assert!(bad * 100 < checked, "{bad} of {checked} bodies broke their invariants");
    }

    #[test]
    fn the_default_disc_is_a_ring_of_light() {
        // Lit pixels cluster away from the centre: the middle of the frame is
        // darker than the ring's radius, and the frame is not a uniform wash.
        let gpu = crate::gpu::Gpu::new_headless();
        let n = 128u32;
        let mut m = Particulas::new();
        let f = SCHEMA.defaults();
        let mut px = Vec::new();
        for k in 0..=10 {
            px = frame_at(&gpu, &mut m, &f, k as f32 * 0.1, n, n);
        }
        let lum = |x: u32, y: u32| {
            let i = ((y * n + x) * 4) as usize;
            px[i] as u32 + px[i + 1] as u32 + px[i + 2] as u32
        };
        let (mut centre, mut ring, mut cn, mut rn) = (0u32, 0u32, 0u32, 0u32);
        for y in 0..n {
            for x in 0..n {
                let (dx, dy) = (x as f32 - n as f32 * 0.5, y as f32 - n as f32 * 0.5);
                let r = (dx * dx + dy * dy).sqrt() / (n as f32 * 0.5);
                if r < 0.1 {
                    centre += lum(x, y);
                    cn += 1;
                } else if (0.3..0.6).contains(&r) {
                    ring += lum(x, y);
                    rn += 1;
                }
            }
        }
        assert!(ring as f32 / rn as f32 > 1.5 * centre as f32 / cn as f32, "no ring: {} vs {}", ring / rn, centre / cn);
    }

    #[test]
    #[ignore = "measurement, not a gate: cargo test -p metal-render particulas -- --ignored --nocapture"]
    fn measure_default_cost() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let (w, h) = (1280, 720);
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let mut m = Particulas::new();
        for n_part in [32768.0f32, 65536.0] {
            let mut f = f.clone();
            f[F_N] = n_part;
            let mut run = |n: u32, t0: f32| {
                for i in 0..n {
                    let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: t0 + i as f32 * 0.016, width: w, height: h, fondo_alpha: false };
                    m.render(&ctx, &f, &view, FORMAT);
                }
                let _ = gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
            };
            run(5, 0.0);
            let t = std::time::Instant::now();
            run(60, 1.0);
            let ms = t.elapsed().as_secs_f32() * 1000.0 / 60.0;
            println!("particles {w}x{h} n={n_part}: {ms:.2} ms/frame");
        }
        let _ = crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, w, h);
    }
}
