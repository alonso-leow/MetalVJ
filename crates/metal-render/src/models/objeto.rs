//! Objeto — a Gielis superformula object.
//!
//! The surface is the spherical product of two superformula curves
//! `r(φ) = (|cos(mφ/4)/a|^n2 + |sin(mφ/4)/b|^n3)^(-1/n1)`: one in latitude θ,
//! one in longitude φ. Written from the maths; no code was taken from anywhere.
//!
//! **Why ray-marching, not a mesh.** The product has an exact implicit form
//! (see `shaders/objeto.wgsl`): in each longitude half-plane the surface is the
//! latitude curve with its horizontal axis scaled by `r2(φ)`. That makes one
//! fullscreen pass with two superformula evaluations per march step (a few
//! `exp`/`log`, no texture reads), no vertex buffer, no rebuild when a shape
//! float is swept at beat rate, and per-pixel normals instead of faceting. A
//! mesh via `shared::mesh` would need a CPU regrid on every shape change.
//! `measure_default_cost` (ignored test, run with `--ignored --nocapture`)
//! prints the per-frame time.
//!
//! **Auto-fit.** The CPU samples the deformed surface each frame and scales the
//! object so its farthest point sits at radius 1; `cam_dist` is therefore in
//! object radii whatever the shape floats say. That is also what keeps extreme
//! values visible: a tiny `a`/`b` shrinks the raw object, not the picture.
//!
//! **Safety.** `gielis` floors every base and exponent and clamps the result
//! to `[e^-6, e^2]`, so n1 → 0, a/b → 0 and large m stay finite. `m1`/`m2` are
//! rounded to integers (symmetry count) and held (`spec_held`).

use glam::Vec3;

use crate::model::{color_at, background, spec, spec_held, Block, Ctx, FloatSpec, Model, Schema, SurfaceSpec};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::kit::{self, CAM3D_BLOCK, LOOK_BLOCK};
use crate::shared::surface::{MaterialU, PartSet};
use crate::shared::ColorSlotGpu;

/// The one part a material can be laid on: the whole superformula surface.
const SURFACES: &[&str] = &["body"];

const FLOATS: &[FloatSpec] = &[
    // Latitude curve (θ): symmetry count, exponents, axes.
    spec_held("m1", 0.0, 12.0, 4.0, 0.25),
    spec("n1_1", 0.1, 10.0, 1.4),
    spec("n2_1", 0.1, 20.0, 2.0),
    spec("n3_1", 0.1, 20.0, 2.5),
    spec("a1", 0.2, 3.0, 1.0),
    spec("b1", 0.2, 3.0, 1.0),
    // Longitude curve (φ).
    spec_held("m2", 0.0, 12.0, 6.0, 0.25),
    spec("n1_2", 0.1, 10.0, 1.2),
    spec("n2_2", 0.1, 20.0, 1.8),
    spec("n3_2", 0.1, 20.0, 1.8),
    spec("a2", 0.2, 3.0, 1.0),
    spec("b2", 0.2, 3.0, 1.0),
    // Deformations of the finished surface: twist about the pole axis (turns
    // of π per half-height), per-axis stretch, taper along the axis.
    spec("torsion", -2.0, 2.0, 0.25),
    spec("stretch_x", 0.3, 2.5, 1.0),
    spec("stretch_y", 0.3, 2.5, 1.0),
    spec("stretch_z", 0.3, 2.5, 1.0),
    spec("taper", -0.9, 0.9, 0.0),
    // Light. luz_x / luz_y are the horizontal plane, luz_alt the height.
    spec("light_x", -1.0, 1.0, 0.6),
    spec("light_y", -1.0, 1.0, 0.7),
    spec("light_height", -1.0, 1.0, 0.6),
    spec("light_hard", 0.0, 1.0, 0.3),
    spec("ambient", 0.0, 1.0, 0.25),
    spec("specular", 0.0, 2.0, 0.5),
    spec("shine_edge", 0.0, 2.0, 0.4),
    spec("fog", 0.0, 1.0, 0.2),
    // Surface pattern that offsets the colour-slot position.
    spec("texture_scale", 0.5, 24.0, 6.0),
    // 0 latitude bands, 1 meridians, 2 checker, 3 noise, 4 spiral. Held.
    spec_held("texture_type", 0.0, 4.0, 0.0, 0.2),
    spec("texture_contrast", 0.0, 1.5, 0.35),
    spec("texture_speed", -4.0, 4.0, 0.3),
];

const SUPERFICIE: [f32; 12] = [0.10, 0.12, 0.45, 0.55, 0.15, 0.65, 0.95, 0.45, 0.25, 1.0, 0.9, 0.7];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("surface", &SUPERFICIE),
        background(),
        CAM3D_BLOCK,
        LOOK_BLOCK,
        crate::model::SIGNAL_FLOATS_BLOCK,
        crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

const B_OWN: usize = 0;
const B_SUP: usize = 1;
const B_FONDO: usize = 2;
const B_CAM: usize = 3;
const B_LOOK: usize = 4;

fn off(block: usize) -> usize {
    kit::offset_of(SCHEMA.blocks, block)
}

const F_M1: usize = 0;
const F_M2: usize = 6;
const F_TORSION: usize = 12;
const F_STRETCH: usize = 13;
const F_TAPER: usize = 16;
const F_LUZ: usize = 17;
const F_LUZ_DURA: usize = 20;
const F_AMBIENTE: usize = 21;
const F_TEX: usize = 25;

/// One superformula curve's six parameters.
#[derive(Clone, Copy, Debug)]
pub struct Gielis {
    pub m: f32,
    pub n1: f32,
    pub n2: f32,
    pub n3: f32,
    pub a: f32,
    pub b: f32,
}

impl Gielis {
    fn from(f: &[f32]) -> Gielis {
        Gielis { m: f[0], n1: f[1], n2: f[2], n3: f[3], a: f[4], b: f[5] }
    }
}

/// `r(φ)`, finite for every input (NaN parameters read as 0).
pub fn gielis(phi: f32, g: &Gielis) -> f32 {
    let fin = |v: f32| if v.is_finite() { v } else { 0.0 };
    let m = fin(g.m).round().clamp(0.0, 64.0);
    let arg = m * fin(phi) * 0.25;
    let a = fin(g.a).max(0.05);
    let b = fin(g.b).max(0.05);
    let t1 = (arg.cos().abs() / a).max(1e-6);
    let t2 = (arg.sin().abs() / b).max(1e-6);
    let s = (fin(g.n2).max(0.05) * t1.ln()).exp() + (fin(g.n3).max(0.05) * t2.ln()).exp();
    let e = (-s.max(1e-9).ln() / fin(g.n1).max(0.05)).clamp(-6.0, 2.0);
    e.exp()
}

/// A surface point of the undeformed object (z is the pole axis).
pub fn surface_point(theta: f32, phi: f32, g1: &Gielis, g2: &Gielis) -> Vec3 {
    let r1 = gielis(theta, g1);
    let r2 = gielis(phi, g2);
    Vec3::new(r1 * theta.cos() * r2 * phi.cos(), r1 * theta.cos() * r2 * phi.sin(), r1 * theta.sin())
}

#[derive(Clone, Copy, Debug)]
struct Deform {
    torsion: f32,
    taper: f32,
    stretch: Vec3,
}

impl Deform {
    fn from(own: &[f32]) -> Deform {
        Deform {
            torsion: own[F_TORSION],
            taper: own[F_TAPER].clamp(-0.9, 0.9),
            stretch: Vec3::new(own[F_STRETCH], own[F_STRETCH + 1], own[F_STRETCH + 2])
                .clamp(Vec3::splat(0.05), Vec3::splat(4.0)),
        }
    }

    fn apply(&self, p: Vec3, zmax: f32) -> Vec3 {
        let zn = p.z / zmax;
        let k = (1.0 - self.taper * zn).max(0.1);
        let ang = self.torsion * zn * std::f32::consts::PI;
        let (s, c) = ang.sin_cos();
        let (x, y) = (p.x * k, p.y * k);
        Vec3::new(c * x - s * y, s * x + c * y, p.z) * self.stretch
    }
}

/// `(zmax, bound)`: the base half-height and the radius of the sphere around
/// the deformed surface, sampled on a 64 x 128 grid and padded.
fn extent(g1: &Gielis, g2: &Gielis, d: &Deform) -> (f32, f32) {
    const NT: usize = 64;
    const NP: usize = 128;
    let at = |i: usize, j: usize| {
        let th = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / NT as f32;
        let ph = -std::f32::consts::PI + std::f32::consts::TAU * j as f32 / NP as f32;
        surface_point(th, ph, g1, g2)
    };
    let mut zmax = 0.0f32;
    for i in 0..=NT {
        for j in 0..NP {
            zmax = zmax.max(at(i, j).z.abs());
        }
    }
    let zmax = zmax.max(0.01);
    let mut bound = 0.0f32;
    for i in 0..=NT {
        for j in 0..NP {
            bound = bound.max(d.apply(at(i, j), zmax).length());
        }
    }
    (zmax, (bound * 1.03).max(0.02))
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    sup: ColorSlotGpu,
    background: ColorSlotGpu,
    a: [f32; 4],
    g1: [f32; 4],
    g1b: [f32; 4],
    g2: [f32; 4],
    g2b: [f32; 4],
    d: [f32; 4],
    light: [f32; 4],
    mat: [f32; 4],
    tex: [f32; 4],
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
            include_str!("../../shaders/objeto.wgsl"),
        )
    })
}

pub struct Objeto {
    pass: Fullscreen,
    /// `body`'s picture and material, one `MaterialU` appended to `Uniform`.
    surfaces: PartSet<1>,
}

impl Objeto {
    pub fn new() -> Objeto {
        Objeto {
            pass: Fullscreen::with_textures("object", shader_src(), SURFACES.len()),
            surfaces: PartSet::new(SURFACES),
        }
    }
}

impl Default for Objeto {
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

impl Model for Objeto {
    fn id(&self) -> &'static str {
        "object"
    }

    fn name(&self) -> &'static str {
        "Objeto"
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
        let own = &floats[off(B_OWN)..off(B_OWN) + FLOATS.len()];
        let g1 = Gielis::from(&own[F_M1..F_M1 + 6]);
        let g2 = Gielis::from(&own[F_M2..F_M2 + 6]);
        let d = Deform::from(own);
        let (zmax, bound) = extent(&g1, &g2, &d);
        let cam = kit::Cam3d::from_floats(&floats[off(B_CAM)..off(B_CAM) + kit::Cam3d::N]);
        let look = kit::Look::from_floats(&floats[off(B_LOOK)..off(B_LOOK) + kit::Look::N]);
        let (eye, right, up, fwd, tan_half) = camera(&cam, ctx.time);
        // The camera distance is in object radii: the object is normalised.
        let min_s = d.stretch.min_element().min(1.0);
        let lip = (0.6 * min_s * (1.0 - 0.5 * d.taper.abs()) / (1.0 + 0.6 * d.torsion.abs()))
            .clamp(0.06, 0.6);
        let ld = Vec3::new(own[F_LUZ], own[F_LUZ + 2], own[F_LUZ + 1]);
        let ld = ld.try_normalize().unwrap_or(Vec3::Y);
        let slot = |blk: usize| ColorSlotGpu::from_floats(&floats[off(blk)..off(blk) + 12]);
        let v4 = |v: Vec3, w: f32| [v.x, v.y, v.z, w];
        self.surfaces.sync(ctx.device, ctx.queue);
        let uniform = Uniform {
            sup: slot(B_SUP),
            background: slot(B_FONDO),
            a: [ctx.aspect(), ctx.time, if ctx.fondo_alpha { 1.0 } else { 0.0 }, bound],
            g1: [g1.m.round(), g1.n1, g1.n2, g1.n3],
            g1b: [g1.a, g1.b, zmax, lip],
            g2: [g2.m.round(), g2.n1, g2.n2, g2.n3],
            g2b: [g2.a, g2.b, d.torsion, d.taper],
            d: [d.stretch.x, d.stretch.y, d.stretch.z, 2.0 * tan_half / ctx.height.max(1) as f32],
            light: v4(ld, own[F_LUZ_DURA]),
            mat: [own[F_AMBIENTE], own[F_AMBIENTE + 1], own[F_AMBIENTE + 2], own[F_AMBIENTE + 3]],
            tex: [own[F_TEX], own[F_TEX + 1], own[F_TEX + 2], own[F_TEX + 3]],
            l: [look.shine, look.contrast, look.opacity, look.background_level],
            l2: [look.tone, look.saturation, tan_half, eye.length()],
            eye: v4(eye, 0.0),
            right: v4(right, 0.0),
            up: v4(up, 0.0),
            fwd: v4(fwd, 0.0),
            mat_body: self.surfaces.uniform(0),
        };
        self.pass.draw_textured(ctx, target, format, &uniform, &self.surfaces.bind_pairs());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
    const W: u32 = 64;
    const TIMES: [f32; 2] = [0.0, 0.7];
    /// The frozen `object` frame wearing the matrix grid (kind 6, mix 1).
    const GOLDEN_OBJETO_MATRIX: u64 = 0x93d00756b3aea871;

    fn frame_at(gpu: &crate::gpu::Gpu, model: &mut Objeto, floats: &[f32], time: f32, w: u32, h: u32) -> Vec<u8> {
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time, width: w, height: h, fondo_alpha: false };
        model.render(&ctx, floats, &view, FORMAT);
        crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, w, h)
    }

    fn frames(gpu: &crate::gpu::Gpu, floats: &[f32]) -> Vec<u8> {
        let mut m = Objeto::new();
        TIMES.iter().flat_map(|&t| frame_at(gpu, &mut m, floats, t, W, W)).collect()
    }

    fn base() -> Vec<f32> {
        let mut f = SCHEMA.defaults();
        for v in &mut f[off(B_FONDO)..off(B_FONDO) + 12] {
            *v = 0.3;
        }
        f
    }

    fn circle() -> Gielis {
        Gielis { m: 4.0, n1: 2.0, n2: 2.0, n3: 2.0, a: 1.0, b: 1.0 }
    }

    #[test]
    fn widths_come_from_the_blocks() {
        assert_eq!(SCHEMA.defaults().len(), SCHEMA.width());
        assert_eq!(off(B_OWN), 0);
        assert_eq!(off(B_SUP), FLOATS.len());
        assert_eq!(off(B_CAM) + kit::Cam3d::N, off(B_LOOK));
        assert_eq!(off(B_LOOK) + kit::Look::N, off(B_LOOK + 1));
        assert!(!Objeto::new().camera_track());
    }

    #[test]
    fn indices_match_the_names() {
        let n = |i: usize| FLOATS[i].name;
        assert_eq!((n(F_M1), n(F_M2), n(F_TORSION), n(F_STRETCH), n(F_TAPER)), ("m1", "m2", "torsion", "stretch_x", "taper"));
        assert_eq!((n(F_LUZ), n(F_LUZ_DURA), n(F_AMBIENTE), n(F_TEX)), ("light_x", "light_hard", "ambient", "texture_scale"));
        assert!(FLOATS[F_M1].drive < 1.0 && FLOATS[F_M2].drive < 1.0);
    }

    #[test]
    fn the_cpu_reference_gives_the_known_curves() {
        // m=4, n=2, a=b=1: (cos² + sin²)^(-1/2) = 1, a circle.
        for i in 0..64 {
            let phi = -PI + i as f32 * 0.1;
            assert!((gielis(phi, &circle()) - 1.0).abs() < 1e-4);
        }
        // n -> large: the unit superellipse tends to the square, corner at √2.
        let sq = Gielis { n1: 40.0, n2: 40.0, n3: 40.0, ..circle() };
        assert!((gielis(0.0, &sq) - 1.0).abs() < 0.02);
        assert!((gielis(FRAC_PI_4, &sq) - 2f32.sqrt()).abs() < 0.06);
        // n = 1: the diamond |x|+|y| = 1: r(π/4) = 1/√2, r(0) = 1.
        let dia = Gielis { n1: 1.0, n2: 1.0, n3: 1.0, ..circle() };
        assert!((gielis(FRAC_PI_4, &dia) - 0.5f32.sqrt()).abs() < 1e-4);
        assert!((gielis(0.0, &dia) - 1.0).abs() < 1e-4);
        // m scales the symmetry: m=8 repeats every π/2 -> same r at 0 and π/2... (m/4 = 2)
        let star = Gielis { m: 8.0, n1: 1.0, n2: 1.0, n3: 1.0, ..circle() };
        assert!((gielis(0.3, &star) - gielis(0.3 + PI, &star)).abs() < 1e-4);
        // Two circles give the unit sphere.
        for (th, ph) in [(0.2, 1.0), (-1.0, 2.5), (FRAC_PI_2, 0.0), (0.7, -2.0)] {
            let p = surface_point(th, ph, &circle(), &circle());
            assert!((p.length() - 1.0).abs() < 1e-4, "{p:?}");
        }
        let (zmax, bound) = extent(&circle(), &circle(), &Deform::from(&{
            let mut o = SCHEMA.defaults();
            o[F_TORSION] = 0.0;
            o[F_STRETCH..F_STRETCH + 3].fill(1.0);
            o[F_TAPER] = 0.0;
            o
        }));
        assert!((zmax - 1.0).abs() < 1e-3 && (bound - 1.03).abs() < 0.01);
    }

    #[test]
    fn the_extremes_of_every_parameter_stay_finite() {
        let vals = [0.0, 1e-9, 0.05, 0.1, 1.0, 20.0, 1e4, -3.0, f32::NAN, f32::INFINITY];
        for &m in &vals {
            for &n1 in &vals {
                for &n2 in &vals {
                    for &ab in &vals {
                        let g = Gielis { m, n1, n2, n3: n2, a: ab, b: ab };
                        for i in 0..40 {
                            let r = gielis(-PI + i as f32 * 0.16, &g);
                            assert!(r.is_finite() && r > 0.0, "{g:?} -> {r}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_defaults_are_not_flat_and_deterministic() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let a = frames(&gpu, &f);
        assert_eq!(a, frames(&gpu, &f));
        let first = &a[..(W * W * 4) as usize];
        let mut distinct = std::collections::HashSet::new();
        for px in first.chunks_exact(4) {
            distinct.insert([px[0], px[1], px[2]]);
        }
        assert!(distinct.len() > 16, "default frame is flat ({} colours)", distinct.len());
        assert_ne!(first, &a[(W * W * 4) as usize..], "the camera/texture must move with time");
    }

    #[test]
    fn every_float_moves_the_frame() {
        let gpu = crate::gpu::Gpu::new_headless();
        let (cam, look) = (off(B_CAM), off(B_LOOK));
        let mut idx: Vec<(usize, FloatSpec)> = FLOATS.iter().enumerate().map(|(i, s)| (i, *s)).collect();
        idx.extend(kit::CAM3D.iter().enumerate().map(|(i, s)| (cam + i, *s)));
        idx.extend(kit::LOOK.iter().enumerate().map(|(i, s)| (look + i, *s)));
        let b = base();
        let reference = frames(&gpu, &b);
        for (i, spec) in idx {
            let far = if (spec.max - b[i]).abs() > (b[i] - spec.min).abs() { spec.max } else { spec.min };
            let far = if spec.name == "tone" { 0.5 } else { far };
            let mut f = b.clone();
            f[i] = far;
            assert!(reference != frames(&gpu, &f), "float `{}` ({} -> {far}) did not move the frame", spec.name, b[i]);
        }
    }

    #[test]
    fn corner_values_render_finite_non_flat_frames() {
        let gpu = crate::gpu::Gpu::new_headless();
        let own_end = FLOATS.len();
        for pick in 0..3 {
            let mut f = base();
            for (i, s) in FLOATS.iter().enumerate().take(own_end) {
                f[i] = match pick {
                    0 => s.min,
                    1 => s.max,
                    _ => if i % 2 == 0 { s.min } else { s.max },
                };
            }
            // Out of range too: below the minimum of the shape floats.
            if pick == 2 {
                for i in [1, 4, 5, 7, 10, 11] {
                    f[i] = 0.0;
                }
            }
            let px = frames(&gpu, &f);
            let mut distinct = std::collections::HashSet::new();
            for p in px.chunks_exact(4) {
                distinct.insert([p[0], p[1], p[2]]);
            }
            assert!(distinct.len() > 3, "corner {pick} drew a flat frame ({} colours)", distinct.len());
        }
    }

    #[test]
    fn a_sphere_covers_the_disc_the_perspective_predicts() {
        // Two circles -> unit sphere (bound 1.03 after padding, but the
        // surface itself is radius 1). Camera at default distance.
        let gpu = crate::gpu::Gpu::new_headless();
        let mut f = SCHEMA.defaults();
        let set = |f: &mut Vec<f32>, at: usize, g: [f32; 6]| f[at..at + 6].copy_from_slice(&g);
        set(&mut f, F_M1, [4.0, 2.0, 2.0, 2.0, 1.0, 1.0]);
        set(&mut f, F_M2, [4.0, 2.0, 2.0, 2.0, 1.0, 1.0]);
        f[F_TORSION] = 0.0;
        f[off(B_SUP)..off(B_SUP) + 12].fill(0.8);
        f[F_AMBIENTE] = 0.9;
        f[F_AMBIENTE + 2] = 0.0; // no rim
        f[F_AMBIENTE + 3] = 0.0; // no fog
        let c = off(B_CAM);
        f[c + 1] = 0.0; // pitch 0
        let n = 160u32;
        let px = frame_at(&gpu, &mut Objeto::new(), &f, 0.0, n, n);
        let lit = px.chunks_exact(4).filter(|p| p[0] > 120).count() as f32;
        let (distance, fov) = (kit::CAM3D[3].default, kit::CAM3D[4].default);
        let ang = (1.0f32 / 1.03 / distance).asin(); // auto-fit pads the bound by 3%
        let r_px = ang.tan() / (fov * 0.5).tan() * n as f32 * 0.5;
        let want = PI * r_px * r_px;
        assert!((lit - want).abs() / want < 0.06, "lit {lit} vs predicted disc {want}");
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
        let mut model = Objeto::new();
        assert_eq!(model.surfaces(), &["body"]);
        let draw = |model: &mut Objeto| {
            let ctx = Ctx {
                device: &gpu.device,
                queue: &gpu.queue,
                time: 1.0,
                width: w,
                height: h,
                fondo_alpha: false,
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

    /// FNV-1a over the frame's RGBA bytes, written out so the value is small
    /// enough to freeze by hand — the same hash the engine's golden corpus uses.
    fn frame_hash(px: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in px {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// The matrix grid is the M7 look: worn by `object` — a raymarched solid
    /// with no line geometry — it is the proof that the grid is a finish any
    /// 3D model takes, not a mesh `matrix` alone owns. The frame is frozen
    /// (re-freeze with `FW_PRINT_GOLDEN=1 cargo test -p metal-render objeto --
    /// --nocapture`), deterministic, and clearing the part restores the plain
    /// frame exactly.
    #[test]
    fn the_matrix_grid_is_a_golden_frame_on_objeto() {
        use crate::model::{MaterialSpec, SurfaceSpec};
        let gpu = crate::gpu::Gpu::new_headless();
        let f = base();
        let (w, h) = (2 * W, 2 * W);
        let mut model = Objeto::new();
        let plain = frame_at(&gpu, &mut model, &f, 3.0, w, h);

        let matrix = MaterialSpec { kind: 6.0, mix: 1.0, scale: 1.0, ..Default::default() };
        model.set_surface("body", Some(SurfaceSpec::material(matrix)));
        let grid = frame_at(&gpu, &mut model, &f, 3.0, w, h);
        assert_ne!(plain, grid, "the grid did not reach the frame");

        // A grid is lines, not a wash: many distinct colours and a wide range.
        let mut distinct = std::collections::HashSet::new();
        for px in grid.chunks_exact(4) {
            distinct.insert([px[0], px[1], px[2]]);
        }
        assert!(distinct.len() > 64, "the grid drew flat ({} colours)", distinct.len());

        let again = frame_at(&gpu, &mut model, &f, 3.0, w, h);
        assert_eq!(grid, again, "the same instant must be the same frame");
        let hash = frame_hash(&grid);
        if std::env::var("FW_PRINT_GOLDEN").is_ok() {
            eprintln!("GOLDEN objeto-matrix {hash:#018x}");
        }
        assert_eq!(hash, GOLDEN_OBJETO_MATRIX, "the objeto grid frame changed; re-freeze GOLDEN_OBJETO_MATRIX");

        model.set_surface("body", None);
        let restored = frame_at(&gpu, &mut model, &f, 3.0, w, h);
        assert_eq!(plain, restored, "clearing the grid did not restore the frame");
    }

    #[test]
    #[ignore = "measurement, not a gate: cargo test -p metal-render objeto -- --ignored --nocapture"]
    fn measure_default_cost() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let (w, h) = (1280, 720);
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let mut m = Objeto::new();
        let mut run = |n: u32| {
            for i in 0..n {
                let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: i as f32 * 0.016, width: w, height: h, fondo_alpha: false };
                m.render(&ctx, &f, &view, FORMAT);
            }
            let _ = gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        };
        run(3);
        let t = std::time::Instant::now();
        run(30);
        let ms = t.elapsed().as_secs_f32() * 1000.0 / 30.0;
        let _ = crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, w, h);
        println!("objeto {w}x{h}: {ms:.2} ms/frame");
    }
}
