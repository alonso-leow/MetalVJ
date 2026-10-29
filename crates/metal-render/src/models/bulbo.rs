//! Bulbo — ray-marched 3D fractals, after mandelbulber2.
//!
//! Eight curated distance estimators, written from the maths of each formula
//! (mandelbulber2 is GPL-3; no code was copied):
//!
//! | `formula` | fractal | iteration | DE |
//! |---|---|---|---|
//! | 0 | Mandelbulb | `z -> z^p` in spherical coordinates `+ c` | `0.5 r ln r / |dz|` |
//! | 1 | Bristorbrot | `(x²-y²-z², y(2x-z), z(2x+y)) + c` | log |
//! | 2 | Quaternion Julia | `q -> q² + c`, a 3D slice of the 4D set | log |
//! | 3 | Mandelbox | box fold, sphere fold, scale, `+ c` | `0.5 r / |dz|` |
//! | 4 | Amazing Box | xy box fold, radial scale, `+ c` | linear |
//! | 5 | Menger sponge | abs, sort, scale 3, shift | `sdBox / 3ⁿ` |
//! | 6 | Sierpinski tetrahedron | tetra fold, scale 2, shift | `sdTetra / 2ⁿ` |
//! | 7 | Kleinian | wrap, half turn, inversion | limit-set DE |
//!
//! `formula` is held and rounded; `mezcla_f` crossfades towards the *next*
//! formula (7 wraps to 0), so the AI can morph continuously by automating one
//! float. The shader evaluates the second estimator only while `mezcla_f > 0`.
//!
//! **Uniform parameters.** One set of floats drives all eight, each reading it
//! in the way that formula can use: `potencia` is the Mandelbulb power;
//! `iteraciones` the iteration count; `escala_f` scales `c` (escape-time) or
//! the scale factor of the box and IFS formulas, as a multiple of the nominal
//! one (1 = classic); `pliegue` is the fold limit of the boxes, the hole size
//! of the IFS, the angle offset of the bulb and the 4D slice of the quaternion;
//! `offset_*` shifts `c` (or the IFS cell); `rot_*` rotates `z` every iteration;
//! `julia_*` + `julia_mix` replace the pixel constant by a fixed one (the
//! quaternion is a Julia set by nature, so there `julia_mix` slides it *towards*
//! the pixel constant instead).
//!
//! **Colour.** The orbit trap (`trampa_modo`: 0 point, 1 coordinate planes, 2 z
//! axis, 3 unit sphere, 4 escape iteration) indexes the `orbita` gradient with
//! `trampa_ciclos` repeats, blended with a normal-based position by
//! `trampa_mezcla`.
//!
//! **Cost.** At most `pasos` (default 56) march steps, a four-tap normal, three
//! AO taps and a 14-step soft shadow; no reduced-resolution pass. Run
//! `measure_cost` (ignored; `--ignored --nocapture`) for ms/frame per formula.
//! Every number the shader divides by is floored and `iteraciones`, `pasos`
//! and `potencia` are clamped here, so in-range floats are finite.

use glam::{Mat3, Vec3};

use crate::model::{color_at, background, spec, spec_held, Block, Ctx, FloatSpec, Model, Schema, SurfaceSpec};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::kit::{self, CAM3D_BLOCK, LOOK_BLOCK};
use crate::shared::surface::{MaterialU, PartSet};
use crate::shared::ColorSlotGpu;

pub const FORMULAS: usize = 8;

/// The one part a material can be laid on: the fractal surface.
const SURFACES: &[&str] = &["body"];

const FLOATS: &[FloatSpec] = &[
    spec_held("formula", 0.0, 7.0, 0.0, 0.25),
    spec("mix_f", 0.0, 1.0, 0.0),
    spec_held("power", 2.0, 16.0, 8.0, 0.25),
    spec_held("iterations", 2.0, 16.0, 8.0, 0.25),
    spec("scale_f", 0.5, 2.0, 1.0),
    spec("fold", 0.2, 2.0, 1.0),
    spec("offset_x", -1.0, 1.0, 0.0),
    spec("offset_y", -1.0, 1.0, 0.0),
    spec("offset_z", -1.0, 1.0, 0.0),
    spec("rot_x", -1.5, 1.5, 0.0),
    spec("rot_y", -1.5, 1.5, 0.0),
    spec("rot_z", -1.5, 1.5, 0.0),
    spec("julia_x", -2.0, 2.0, -0.2),
    spec("julia_y", -2.0, 2.0, 0.65),
    spec("julia_z", -2.0, 2.0, 0.2),
    spec("julia_mix", 0.0, 1.0, 0.0),
    spec_held("steps", 16.0, 96.0, 56.0, 0.25),
    spec("precision", 0.3, 3.0, 1.0),
    spec("occlusion", 0.0, 1.0, 0.6),
    spec("shadow", 0.0, 1.0, 0.5),
    spec("glow", 0.0, 2.0, 0.5),
    spec("fog", 0.0, 1.0, 0.3),
    spec_held("trap_mode", 0.0, 4.0, 0.0, 0.2),
    spec("trap_mix", 0.0, 1.0, 0.85),
    spec("trap_cycles", 0.25, 8.0, 2.5),
    spec("light_x", -1.0, 1.0, 0.5),
    spec("light_y", -1.0, 1.0, 0.6),
    spec("light_hard", 0.0, 1.0, 0.3),
];

const F_FORMULA: usize = 0;
const F_MEZCLA: usize = 1;
const F_POT: usize = 2;
const F_ITER: usize = 3;
const F_ESCALA: usize = 4;
const F_PLIEGUE: usize = 5;
const F_OFFSET: usize = 6;
const F_ROT: usize = 9;
const F_JULIA: usize = 12;
const F_JMIX: usize = 15;
const F_PASOS: usize = 16;
const F_PREC: usize = 17;
const F_AOC: usize = 18;
const F_SOMBRA: usize = 19;
const F_GLOW: usize = 20;
const F_NIEBLA: usize = 21;
const F_TRAMPA: usize = 22;
const F_TMEZCLA: usize = 23;
const F_TCICLOS: usize = 24;
const F_LUZ: usize = 25;
const F_LUZ_DURA: usize = 27;

// Violet shadow, magenta, ember orange, pale gold.
const ORBITA: [f32; 12] = [0.06, 0.04, 0.22, 0.65, 0.12, 0.55, 0.98, 0.45, 0.12, 1.0, 0.92, 0.65];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("orbit", &ORBITA),
        background(),
        CAM3D_BLOCK,
        LOOK_BLOCK,
        crate::model::SIGNAL_FLOATS_BLOCK,
        crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

const B_OWN: usize = 0;
const B_ORB: usize = 1;
const B_FONDO: usize = 2;
const B_CAM: usize = 3;
const B_LOOK: usize = 4;

fn off(block: usize) -> usize {
    kit::offset_of(SCHEMA.blocks, block)
}

// ---------------------------------------------------------------------------
// CPU references (formula coordinates, no per-iteration rotation, c = pixel).
// These mirror `bu_bulb` and `bu_menger` in shaders/bulbo.wgsl.

/// Mandelbulb distance estimate at `p` (formula coordinates). Negative inside.
pub fn de_mandelbulb(p: Vec3, power: f32, iters: u32) -> f32 {
    let (mut z, mut dr) = (p, 1.0f32);
    let mut r = z.length();
    for _ in 0..iters {
        r = z.length();
        if r > 4.0 {
            break;
        }
        let th = (z.z / r.max(1e-6)).clamp(-1.0, 1.0).acos() * power;
        let ph = z.y.atan2(z.x) * power;
        let rp = r.powf(power - 1.0);
        dr = rp * power * dr + 1.0;
        z = rp * r * Vec3::new(th.sin() * ph.cos(), th.sin() * ph.sin(), th.cos()) + p;
    }
    r = z.length();
    0.5 * r.max(1e-6).ln() * r / dr
}

/// Menger sponge (cube of half-width 1) by the sort-and-shift fold. Negative
/// inside the solid.
pub fn de_menger(p: Vec3, iters: u32) -> f32 {
    let (s, off) = (3.0f32, 2.0f32);
    let (mut z, mut dr) = (p, 1.0f32);
    for _ in 0..iters {
        z = z.abs();
        if z.x < z.y {
            z = Vec3::new(z.y, z.x, z.z);
        }
        if z.x < z.z {
            z = Vec3::new(z.z, z.y, z.x);
        }
        if z.y < z.z {
            z = Vec3::new(z.x, z.z, z.y);
        }
        z *= s;
        z.x -= off;
        z.y -= off;
        if z.z > 0.5 * off {
            z.z -= off;
        }
        dr *= s;
    }
    let q = z.abs() - Vec3::splat(1.0);
    (q.max(Vec3::ZERO).length() + q.x.max(q.y.max(q.z)).min(0.0)) / dr
}

// ---------------------------------------------------------------------------

/// The model's own floats, clamped into range (NaN reads as the default).
#[derive(Clone, Debug)]
struct Own {
    v: [f32; 28],
}

impl Own {
    fn from(floats: &[f32]) -> Own {
        let mut v = [0.0; 28];
        for (i, s) in FLOATS.iter().enumerate() {
            let x = floats.get(i).copied().unwrap_or(s.default);
            v[i] = if x.is_finite() { x.clamp(s.min, s.max) } else { s.default };
        }
        Own { v }
    }
    fn formula(&self) -> usize {
        (self.v[F_FORMULA].round() as usize).min(FORMULAS - 1)
    }
}

/// Rotation applied to `z` every iteration: Rz * Ry * Rx.
fn rotation(x: f32, y: f32, z: f32) -> Mat3 {
    Mat3::from_rotation_z(z) * Mat3::from_rotation_y(y) * Mat3::from_rotation_x(x)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    orb: ColorSlotGpu,
    background: ColorSlotGpu,
    a: [f32; 4],
    f: [f32; 4],
    g: [f32; 4],
    o: [f32; 4],
    j: [f32; 4],
    r0: [f32; 4],
    r1: [f32; 4],
    r2: [f32; 4],
    h: [f32; 4],
    t: [f32; 4],
    light: [f32; 4],
    l: [f32; 4],
    l2: [f32; 4],
    eye: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    fwd: [f32; 4],
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
            include_str!("../../shaders/bulbo.wgsl"),
        )
    })
}

pub struct Bulbo {
    pass: Fullscreen,
    surfaces: PartSet<1>,
}

impl Bulbo {
    pub fn new() -> Bulbo {
        Bulbo {
            pass: Fullscreen::with_textures("bulb", shader_src(), SURFACES.len()),
            surfaces: PartSet::new(SURFACES),
        }
    }
}

impl Default for Bulbo {
    fn default() -> Self {
        Self::new()
    }
}

/// Eye, right, up, forward from the CAM3D kit. Deterministic in `time`.
fn camera(c: &kit::Cam3d, time: f32) -> (Vec3, Vec3, Vec3, Vec3, f32) {
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
    let mut right = fwd.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
    let mut up = right.cross(fwd);
    let (sr, cr) = c.roll.sin_cos();
    let (r2, u2) = (right * cr + up * sr, up * cr - right * sr);
    right = r2;
    up = u2;
    let tan_half = (c.fov.clamp(0.3, 2.2) * 0.5).tan();
    (eye, right, up, fwd, tan_half)
}

fn uniform(ctx: &Ctx, floats: &[f32]) -> Uniform {
    let own = Own::from(&floats[off(B_OWN)..off(B_OWN) + FLOATS.len()]);
    let o = &own.v;
    let cam = kit::Cam3d::from_floats(&floats[off(B_CAM)..off(B_CAM) + kit::Cam3d::N]);
    let look = kit::Look::from_floats(&floats[off(B_LOOK)..off(B_LOOK) + kit::Look::N]);
    let (eye, right, up, fwd, tan_half) = camera(&cam, ctx.time);
    let fa = own.formula();
    let fb = (fa + 1) % FORMULAS;
    let rot = rotation(o[F_ROT], o[F_ROT + 1], o[F_ROT + 2]);
    let (r0, r1, r2) = (rot.row(0), rot.row(1), rot.row(2));
    // Light is given in the camera's frame so it always lights the visible side.
    let ld = (right * o[F_LUZ] + up * o[F_LUZ + 1] - fwd * 0.8).try_normalize().unwrap_or(Vec3::Y);
    let slot = |blk: usize| ColorSlotGpu::from_floats(&floats[off(blk)..off(blk) + 12]);
    let v4 = |v: Vec3, w: f32| [v.x, v.y, v.z, w];
    Uniform {
        orb: slot(B_ORB),
        background: slot(B_FONDO),
        a: [ctx.aspect(), ctx.time, if ctx.fondo_alpha { 1.0 } else { 0.0 }, 0.0],
        f: [fa as f32, fb as f32, o[F_MEZCLA], o[F_POT]],
        g: [o[F_ITER].round(), o[F_ESCALA], o[F_PLIEGUE], o[F_PASOS].round()],
        o: [o[F_OFFSET], o[F_OFFSET + 1], o[F_OFFSET + 2], o[F_PREC]],
        j: [o[F_JULIA], o[F_JULIA + 1], o[F_JULIA + 2], o[F_JMIX]],
        r0: v4(r0, 0.0),
        r1: v4(r1, 0.0),
        r2: v4(r2, 0.0),
        h: [o[F_AOC], o[F_SOMBRA], o[F_GLOW], o[F_NIEBLA]],
        t: [o[F_TRAMPA].round(), o[F_TMEZCLA], o[F_TCICLOS], 0.0],
        light: v4(ld, o[F_LUZ_DURA]),
        l: [look.shine, look.contrast, look.opacity, look.background_level],
        l2: [look.tone, look.saturation, tan_half, 2.0 * tan_half / ctx.height.max(1) as f32],
        eye: v4(eye, 0.0),
        right: v4(right, 0.0),
        up: v4(up, 0.0),
        fwd: v4(fwd, 0.0),
        // Filled by `render` from the model's `PartSet`, which has no device.
        mat_body: MaterialU::default(),
    }
}

impl Model for Bulbo {
    fn id(&self) -> &'static str {
        "bulb"
    }

    fn name(&self) -> &'static str {
        "Bulbo"
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
        self.surfaces.sync(ctx.device, ctx.queue);
        let mut u = uniform(ctx, floats);
        u.mat_body = self.surfaces.uniform(0);
        self.pass.draw_textured(ctx, target, format, &u, &self.surfaces.bind_pairs());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
    const W: u32 = 64;
    const TIMES: [f32; 2] = [0.0, 0.7];

    fn frame_at(gpu: &crate::gpu::Gpu, model: &mut Bulbo, floats: &[f32], time: f32, w: u32, h: u32) -> Vec<u8> {
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time, width: w, height: h, fondo_alpha: false, detail: 1.0 };
        model.render(&ctx, floats, &view, FORMAT);
        crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, w, h)
    }

    fn frames(gpu: &crate::gpu::Gpu, floats: &[f32]) -> Vec<u8> {
        let mut m = Bulbo::new();
        TIMES.iter().flat_map(|&t| frame_at(gpu, &mut m, floats, t, W, W)).collect()
    }

    /// Defaults with a visible grey background, and `fid` as the formula.
    fn base(fid: usize) -> Vec<f32> {
        let mut f = SCHEMA.defaults();
        for v in &mut f[off(B_FONDO)..off(B_FONDO) + 12] {
            *v = 0.3;
        }
        f[F_FORMULA] = fid as f32;
        f
    }

    fn colours(px: &[u8]) -> usize {
        let mut set = std::collections::HashSet::new();
        for p in px.chunks_exact(4) {
            set.insert([p[0], p[1], p[2]]);
        }
        set.len()
    }

    #[test]
    fn widths_come_from_the_blocks() {
        assert_eq!(SCHEMA.defaults().len(), SCHEMA.width());
        assert_eq!(off(B_ORB), FLOATS.len());
        assert_eq!(off(B_CAM) + kit::Cam3d::N, off(B_LOOK));
        assert!(!Bulbo::new().camera_track());
        assert!(FLOATS.len() + kit::Cam3d::N + kit::Look::N <= 44);
        assert_eq!(Own { v: [0.0; 28] }.v.len(), FLOATS.len());
    }

    #[test]
    fn indices_match_the_names() {
        let n = |i: usize| FLOATS[i].name;
        assert_eq!(
            (n(F_FORMULA), n(F_MEZCLA), n(F_POT), n(F_ITER), n(F_ESCALA), n(F_PLIEGUE)),
            ("formula", "mix_f", "power", "iterations", "scale_f", "fold")
        );
        assert_eq!((n(F_OFFSET), n(F_ROT), n(F_JULIA), n(F_JMIX)), ("offset_x", "rot_x", "julia_x", "julia_mix"));
        assert_eq!((n(F_PASOS), n(F_PREC), n(F_AOC), n(F_SOMBRA), n(F_GLOW), n(F_NIEBLA)), ("steps", "precision", "occlusion", "shadow", "glow", "fog"));
        assert_eq!((n(F_TRAMPA), n(F_TMEZCLA), n(F_TCICLOS), n(F_LUZ), n(F_LUZ_DURA)), ("trap_mode", "trap_mix", "trap_cycles", "light_x", "light_hard"));
        for i in [F_FORMULA, F_POT, F_ITER, F_PASOS, F_TRAMPA] {
            assert!(FLOATS[i].drive < 1.0, "{} must be held", FLOATS[i].name);
        }
        assert_eq!(FLOATS[F_FORMULA].max as usize, FORMULAS - 1);
    }

    #[test]
    fn mandelbulb_de_separates_inside_from_outside() {
        let d = |x, y, z| de_mandelbulb(Vec3::new(x, y, z), 8.0, 12);
        assert!(d(0.02, 0.01, 0.03) < 0.0, "the middle is in the set");
        assert!(d(0.3, 0.0, 0.0) < 0.0);
        assert!(d(0.0, 0.0, 0.6) < 0.0);
        assert!(d(1.5, 0.0, 0.0) > 0.0 && d(0.0, 0.0, 2.0) > 0.0 && d(1.2, 1.2, 1.2) > 0.0);
        // Power 2 is the 3D Mandelbrot-like body: the x axis tip is at -2.
        assert!(de_mandelbulb(Vec3::new(0.2, 0.0, 0.0), 2.0, 16) < 0.0);
        assert!(de_mandelbulb(Vec3::new(2.5, 0.0, 0.0), 2.0, 16) > 0.0);
    }

    #[test]
    fn mandelbulb_de_never_overshoots_the_surface() {
        // Sphere-trace towards the middle from several directions: the walk
        // must close in on the surface from outside without ever going inside.
        for dir in [Vec3::new(1.0, 0.3, 0.2), Vec3::new(-0.4, 1.0, 0.5), Vec3::new(0.2, -0.3, 1.0), Vec3::new(1.0, 1.0, 1.0)] {
            let dir = dir.normalize();
            let mut t = 0.0f32;
            let mut hit = false;
            let aim = Vec3::new(0.1, 0.05, 0.1);
            for _ in 0..400 {
                let d = de_mandelbulb(aim + dir * (3.0 - t), 8.0, 12);
                assert!(d > -1e-3, "walked into the interior at t={t}");
                if d < 1e-3 {
                    hit = true;
                    break;
                }
                t += d * 0.9;
            }
            assert!(hit, "ray along {dir:?} never reached the surface");
        }
    }

    #[test]
    fn menger_de_matches_the_known_solid() {
        let d = |x, y, z| de_menger(Vec3::new(x, y, z), 5);
        // A corner of the cube is solid; the centre is in the central hole.
        assert!(d(0.95, 0.95, 0.95) < 0.0);
        assert!(d(0.0, 0.0, 0.0) > 0.0);
        // Middle of a face is a cross-shaped hole at every level.
        assert!(d(0.0, 0.0, 0.95) > 0.0);
        // Edge cube at level 1: (0.8, 0.8, 0.0) is on the solid edge column.
        assert!(d(0.8, 0.8, 0.0) < 0.0);
        // Outside the cube, and the estimate is close to the box distance.
        assert!(d(2.0, 0.0, 0.0) > 0.0);
        assert!((d(3.0, 0.9, 0.9) - 2.0).abs() < 0.3);
        // Level 1 removes the face-centre column: (0, 0.9, 0) is empty.
        assert!(d(0.0, 0.9, 0.0) > 0.0);
    }

    #[test]
    fn menger_de_never_overshoots_the_surface() {
        for dir in [Vec3::new(1.0, 0.31, 0.17), Vec3::new(0.3, 1.0, 0.45), Vec3::new(0.52, 0.2, 1.0)] {
            let dir = dir.normalize();
            let mut t = 0.0f32;
            let mut hit = false;
            // Aim at a solid corner cube so the ray cannot slip down a tunnel.
            let aim = Vec3::new(0.85, 0.8, 0.9);
            for _ in 0..300 {
                let d = de_menger(aim + dir * (4.0 - t), 5);
                assert!(d > -1e-3, "walked into the solid at t={t}");
                if d < 1e-3 {
                    hit = true;
                    break;
                }
                t += d * 0.9;
            }
            assert!(hit, "ray along {dir:?} never reached the sponge");
        }
    }

    #[test]
    fn out_of_range_floats_are_sanitised() {
        let mut f = vec![f32::NAN; FLOATS.len()];
        f[F_ITER] = 1e9;
        f[F_PASOS] = -5.0;
        f[F_POT] = f32::INFINITY;
        let o = Own::from(&f);
        assert!(o.v.iter().all(|v| v.is_finite()));
        assert_eq!(o.v[F_ITER], FLOATS[F_ITER].max);
        assert_eq!(o.v[F_PASOS], FLOATS[F_PASOS].min);
        assert_eq!(o.v[F_POT], FLOATS[F_POT].default);
        assert_eq!(Own::from(&[]).formula(), 0);
        let mut g = vec![0.0; FLOATS.len()];
        g[F_FORMULA] = 99.0;
        assert_eq!(Own::from(&g).formula(), FORMULAS - 1);
    }

    #[test]
    fn every_formula_renders_a_non_flat_deterministic_default() {
        let gpu = crate::gpu::Gpu::new_headless();
        for fid in 0..FORMULAS {
            let f = base(fid);
            let a = frames(&gpu, &f);
            assert_eq!(a, frames(&gpu, &f), "formula {fid} is not deterministic");
            let first = &a[..(W * W * 4) as usize];
            assert!(colours(first) > 24, "formula {fid} drew a flat default frame ({} colours)", colours(first));
            // The background grey must still be visible somewhere (not filled).
            let bg = [first[0] as i32, first[1] as i32, first[2] as i32];
            let covered = first
                .chunks_exact(4)
                .filter(|p| (0..3).any(|k| (p[k] as i32 - bg[k]).abs() > 10))
                .count();
            assert!(covered > (W * W / 40) as usize, "formula {fid}: object not visible ({covered} px)");
            assert!(covered < (W * W * 39 / 40) as usize, "formula {fid}: object fills the whole frame");
        }
    }

    #[test]
    fn every_float_moves_the_frame() {
        let gpu = crate::gpu::Gpu::new_headless();
        let (cam, look) = (off(B_CAM), off(B_LOOK));
        let mut idx: Vec<(usize, FloatSpec)> = FLOATS.iter().enumerate().map(|(i, s)| (i, *s)).collect();
        idx.extend(kit::CAM3D.iter().enumerate().map(|(i, s)| (cam + i, *s)));
        idx.extend(kit::LOOK.iter().enumerate().map(|(i, s)| (look + i, *s)));
        // The Julia mix sits half way so the Julia constant has something to do;
        // mezcla_f sits at a quarter so that `formula` and the second estimator
        // both matter.
        let mut bases: Vec<Vec<f32>> = (0..FORMULAS).map(base).collect();
        for b in &mut bases {
            b[F_JMIX] = 0.5;
        }
        let refs: Vec<Vec<u8>> = bases.iter().map(|b| frames(&gpu, b)).collect();
        for (i, spec) in idx {
            let mut moved = false;
            for (fid, b) in bases.iter().enumerate() {
                if i == F_FORMULA && fid > 0 {
                    break;
                }
                let far = if (spec.max - b[i]).abs() > (b[i] - spec.min).abs() { spec.max } else { spec.min };
                let far = if spec.name == "tone" { 0.5 } else { far };
                let mut f = b.clone();
                f[i] = far;
                if refs[fid] != frames(&gpu, &f) {
                    moved = true;
                    break;
                }
            }
            assert!(moved, "float `{}` did not move the frame in any formula", spec.name);
        }
    }

    #[test]
    fn mezcla_crossfades_between_neighbours() {
        let gpu = crate::gpu::Gpu::new_headless();
        let mut a = base(3);
        let at0 = frames(&gpu, &a);
        a[F_MEZCLA] = 0.5;
        let half = frames(&gpu, &a);
        a[F_MEZCLA] = 1.0;
        let full = frames(&gpu, &a);
        assert_ne!(at0, half);
        assert_ne!(half, full);
        // Full mix is the next formula (4) at the same floats.
        assert_eq!(full, frames(&gpu, &base(4)));
        // 7 wraps to 0.
        let mut w = base(7);
        w[F_MEZCLA] = 1.0;
        assert_eq!(frames(&gpu, &w), frames(&gpu, &base(0)));
    }

    #[test]
    fn corner_values_render_valid_frames_for_every_formula() {
        // Combined extremes can legitimately push the set out of view (c far
        // outside it), so the gate is validity: never an all-black or
        // all-white frame, which is what a NaN/Inf leak would draw.
        let gpu = crate::gpu::Gpu::new_headless();
        for fid in 0..FORMULAS {
            for pick in 0..3 {
                let mut f = base(fid);
                for (i, s) in FLOATS.iter().enumerate() {
                    if i == F_FORMULA || i == F_MEZCLA {
                        continue;
                    }
                    f[i] = match pick {
                        0 => s.min,
                        1 => s.max,
                        _ => {
                            if i % 2 == 0 {
                                s.min
                            } else {
                                s.max
                            }
                        }
                    };
                }
                let px = frames(&gpu, &f);
                let black = px.chunks_exact(4).all(|p| p[..3] == [0, 0, 0]);
                let white = px.chunks_exact(4).all(|p| p[..3] == [255, 255, 255]);
                assert!(!black && !white, "formula {fid} corner {pick} drew a black or white frame");
            }
        }
    }

    #[test]
    fn each_float_alone_at_its_extremes_keeps_the_object_visible() {
        let gpu = crate::gpu::Gpu::new_headless();
        for fid in 0..FORMULAS {
            for (i, s) in FLOATS.iter().enumerate() {
                if i == F_MEZCLA {
                    continue;
                }
                for far in [s.min, s.max] {
                    if i == F_FORMULA {
                        continue;
                    }
                    let mut f = base(fid);
                    f[i] = far;
                    let px = frames(&gpu, &f);
                    let first = &px[..(W * W * 4) as usize];
                    assert!(colours(first) > 3, "formula {fid}: `{}` = {far} left a flat frame ({} colours)", s.name, colours(first));
                }
            }
        }
    }

    /// Every kind is a surface: at `mat_mix` 1 it changes the frame, and
    /// clearing `body` restores the unpainted frame byte for byte.
    #[test]
    fn a_body_material_changes_the_frame_and_clearing_it_restores_it() {
        use crate::model::{MaterialSpec, SurfaceSpec};
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (128u32, 128u32);
        let floats = SCHEMA.defaults();
        let (texture, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let mut model = Bulbo::new();
        assert_eq!(model.surfaces(), &["body"]);
        let draw = |model: &mut Bulbo| {
            let ctx = Ctx {
                device: &gpu.device,
                queue: &gpu.queue,
                time: 1.0,
                width: w,
                height: h,
                fondo_alpha: false, detail: 1.0,
            };
            model.render(&ctx, &floats, &view, FORMAT);
            crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &texture, w, h)
        };
        let plain = draw(&mut model);
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
            let painted = draw(&mut model);
            assert_ne!(plain, painted, "{name} did not change the frame");
            model.set_surface("body", None);
            let restored = draw(&mut model);
            assert_eq!(plain, restored, "clearing {name} did not restore the frame");
        }
    }

    #[test]
    #[ignore = "measurement, not a gate: cargo test -p metal-render bulbo -- --ignored --nocapture"]
    fn measure_cost() {
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (1280, 720);
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        for fid in 0..FORMULAS {
            for mix in [0.0, 0.5] {
                let mut f = SCHEMA.defaults();
                f[F_FORMULA] = fid as f32;
                f[F_MEZCLA] = mix;
                let mut m = Bulbo::new();
                let mut run = |n: u32| {
                    for i in 0..n {
                        let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: i as f32 * 0.016, width: w, height: h, fondo_alpha: false, detail: 1.0 };
                        m.render(&ctx, &f, &view, FORMAT);
                    }
                    let _ = gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
                };
                run(3);
                let t = std::time::Instant::now();
                run(20);
                let ms = t.elapsed().as_secs_f32() * 1000.0 / 20.0;
                let _ = crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, w, h);
                println!("bulbo formula {fid} mezcla {mix} {w}x{h}: {ms:.2} ms/frame");
            }
        }
    }
}
