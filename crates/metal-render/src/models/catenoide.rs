//! Catenoide — a ray-cast tunnel room with a catenoid wall.
//!
//! The wall profile is `r(z) = radio · cosh(z / c)` with `z` wrapped to
//! `[-P/2, P/2]` (`P = 8`), so the neck of radius `radio` repeats every `P`
//! units and flares between; `cuello` is `c`: small = a tight hourglass,
//! large = nearly a straight pipe. The cross-section is a radial function
//! (circle / square / hex / star) morphed by the held float `seccion`, twisted
//! along z by `torsion`. One fullscreen sphere-tracing pass; the camera is
//! inside, driven forward by `avance` (+ the beat kick `avance_ritmo`).
//! Written from the maths; the shader mirrors [`catenoid_radius`]/[`section`].
//!
//! **Safety.** Every input is clamped to its declared range (NaN reads as the
//! default); the profile's `cosh` argument is capped at 4, and the camera is
//! kept within 0.4·radio of the axis, which is inside the wall for every
//! section (smallest radial factor 0.55), so each corner value is a tunnel.

use glam::Vec3;

use crate::model::{color_at, background, spec, spec_held, Block, Ctx, FloatSpec, Model, Schema, SurfaceSpec};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::kit::{self, CAM3D_BLOCK, LOOK_BLOCK};
use crate::shared::surface::{MaterialU, PartSet};
use crate::shared::ColorSlotGpu;

/// Length of one neck-to-neck period along z.
pub const PERIOD: f32 = 8.0;

/// The one part a material can be laid on: the catenoid wall.
const SURFACES: &[&str] = &["body"];

const FLOATS: &[FloatSpec] = &[
    spec("neck", 0.6, 8.0, 2.2),
    spec("radius", 0.3, 3.0, 1.0),
    // 0 circle, 1 square, 2 hex, 3 star; between = morph. Held.
    spec_held("section", 0.0, 3.0, 2.0, 0.25),
    spec("torsion", -2.0, 2.0, 0.15),
    spec("spiral", -3.0, 3.0, 0.0),
    spec("advance", -6.0, 6.0, 2.0),
    spec("advance_rhythm", 0.0, 4.0, 0.0),
    spec("roller", -2.0, 2.0, 0.1),
    // 0 rings, 1 panels, 2 lights, 3 stripes; between = blend. Held.
    spec_held("texture_type", 0.0, 3.0, 0.0, 0.2),
    spec("texture_scale", 0.25, 4.0, 1.0),
    spec("texture_speed", -4.0, 4.0, 0.0),
    spec("rings_density", 0.2, 4.0, 1.0),
    spec("rings_width", 0.05, 0.9, 0.2),
    spec("panel_density", 2.0, 32.0, 12.0),
    spec("lights_density", 0.2, 4.0, 1.0),
    spec("lights_pulse", 0.0, 1.0, 0.5),
    spec("fog", 0.0, 1.0, 0.5),
    spec("fog_color_mix", 0.0, 1.0, 0.3),
    spec("edges_neon", 0.0, 2.0, 1.0),
];

const F_CUELLO: usize = 0;
const F_RADIO: usize = 1;
const F_SECCION: usize = 2;
const F_TORSION: usize = 3;
const F_ESPIRAL: usize = 4;
const F_AVANCE: usize = 5;
const F_RITMO: usize = 6;
const F_RODILLO: usize = 7;
const F_TIPO: usize = 8;
const F_ESCALA: usize = 9;
const F_VEL: usize = 10;
const F_ANILLOS: usize = 11;
const F_ANCHO: usize = 12;
const F_PANEL: usize = 13;
const F_LUCES: usize = 14;
const F_PULSO: usize = 15;
const F_NIEBLA: usize = 16;
const F_NMIX: usize = 17;
const F_NEON: usize = 18;

const PARED: [f32; 12] = [0.04, 0.03, 0.18, 0.10, 0.10, 0.45, 0.15, 0.50, 0.80, 0.70, 0.90, 1.0];
const NEON: [f32; 12] = [1.0, 0.1, 0.7, 0.9, 0.2, 1.0, 0.1, 0.9, 1.0, 1.0, 1.0, 0.8];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("wall", &PARED),
        color_at("neon", &NEON),
        background(),
        CAM3D_BLOCK,
        LOOK_BLOCK,
        crate::model::SIGNAL_FLOATS_BLOCK,
        crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

const B_OWN: usize = 0;
const B_PARED: usize = 1;
const B_NEON: usize = 2;
const B_FONDO: usize = 3;
const B_CAM: usize = 4;
const B_LOOK: usize = 5;

fn off(block: usize) -> usize {
    kit::offset_of(SCHEMA.blocks, block)
}

fn fin(v: f32, spec: &FloatSpec) -> f32 {
    if v.is_finite() {
        v.clamp(spec.min, spec.max)
    } else {
        spec.default
    }
}

/// Wall radius of the catenoid at axial position `z`: `radio · cosh(zw / c)`
/// with `zw` wrapped to `[-P/2, P/2]` and the argument capped at 4.
pub fn catenoid_radius(z: f32, radius: f32, c: f32) -> f32 {
    let zw = z - PERIOD * (z / PERIOD + 0.5).floor();
    radius * (zw.abs() / c.max(0.05)).min(4.0).cosh()
}

fn poly(a: f32, n: f32, ins: f32) -> f32 {
    let seg = std::f32::consts::TAU / n;
    let ph = ((a / seg).rem_euclid(1.0) - 0.5) * seg;
    ins / ph.cos().max(0.2)
}

fn star(a: f32) -> f32 {
    let s = ((a * 5.0 / std::f32::consts::TAU).rem_euclid(1.0) * 2.0 - 1.0).abs();
    0.55 + (1.05 - 0.55) * s
}

/// Cross-section radial factor at `angle`: 0 circle, 1 square, 2 hex, 3 star,
/// smoothstep-morphed between neighbours.
pub fn section(angle: f32, section: f32) -> f32 {
    let kind = |k: i32| match k {
        0 => 1.0,
        1 => poly(angle, 4.0, 0.85),
        2 => poly(angle, 6.0, 0.92),
        _ => star(angle),
    };
    let x = section.clamp(0.0, 3.0);
    let i = x.floor().min(2.0);
    let t = (x - i).clamp(0.0, 1.0);
    let f = t * t * (3.0 - 2.0 * t);
    kind(i as i32) * (1.0 - f) + kind(i as i32 + 1) * f
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    wall: ColorSlotGpu,
    neon: ColorSlotGpu,
    background: ColorSlotGpu,
    a: [f32; 4],
    s: [f32; 4],
    m: [f32; 4],
    tx: [f32; 4],
    t2: [f32; 4],
    f: [f32; 4],
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
            include_str!("../../shaders/catenoide.wgsl"),
        )
    })
}

pub struct Catenoide {
    pass: Fullscreen,
    surfaces: PartSet<1>,
}

impl Catenoide {
    pub fn new() -> Catenoide {
        Catenoide {
            pass: Fullscreen::with_textures("catenoid", shader_src(), SURFACES.len()),
            surfaces: PartSet::new(SURFACES),
        }
    }
}

impl Default for Catenoide {
    fn default() -> Self {
        Self::new()
    }
}

/// Eye, right, up, forward and tan(fov/2). The tunnel runs along +z; yaw,
/// pitch and roll steer the view inside it.
fn camera(c: &kit::Cam3d, own: &[f32], time: f32) -> (Vec3, Vec3, Vec3, Vec3, f32) {
    let radius = own[F_RADIO];
    let z = time * own[F_AVANCE] + own[F_RITMO] * 0.8 + c.dolly + c.distance;
    let orb = c.orbit * time;
    let orb_r = c.orbit.abs().min(1.0) * 0.3 * radius;
    let alt = c.alt / 5.0 * 0.4 * radius;
    let shake = c.shake.clamp(0.0, 1.0) * 0.06 * radius;
    let tremor = Vec3::new(
        (time * 47.0).sin() + (time * 31.0 + 1.0).sin(),
        (time * 61.0 + 2.0).sin() + (time * 37.0).sin(),
        0.0,
    ) * shake;
    let mut xy = glam::Vec2::new(orb.cos() * orb_r, orb.sin() * orb_r + alt) + tremor.truncate();
    let lim = 0.4 * radius;
    if xy.length() > lim {
        xy = xy.normalize() * lim;
    }
    let eye = Vec3::new(xy.x, xy.y, z);
    let (sy, cy) = c.yaw.sin_cos();
    let (sp, cp) = c.pitch.clamp(-1.5, 1.5).sin_cos();
    let fwd = Vec3::new(sy * cp, sp, cy * cp);
    let mut right = Vec3::Y.cross(fwd).try_normalize().unwrap_or(Vec3::X);
    let mut up = fwd.cross(right);
    let (sr, cr) = c.roll.sin_cos();
    let (r2, u2) = (right * cr + up * sr, up * cr - right * sr);
    right = r2;
    up = u2;
    (eye, right, up, fwd, (c.fov.clamp(0.3, 2.2) * 0.5).tan())
}

impl Model for Catenoide {
    fn id(&self) -> &'static str {
        "catenoid"
    }

    fn name(&self) -> &'static str {
        "Catenoide"
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
        let raw = &floats[off(B_OWN)..off(B_OWN) + FLOATS.len()];
        let own: Vec<f32> = raw.iter().zip(FLOATS).map(|(v, s)| fin(*v, s)).collect();
        let cam = kit::Cam3d::from_floats(&floats[off(B_CAM)..off(B_CAM) + kit::Cam3d::N]);
        let look = kit::Look::from_floats(&floats[off(B_LOOK)..off(B_LOOK) + kit::Look::N]);
        let (eye, right, up, fwd, tan_half) = camera(&cam, &own, ctx.time);
        let slot = |blk: usize| ColorSlotGpu::from_floats(&floats[off(blk)..off(blk) + 12]);
        let v4 = |v: Vec3| [v.x, v.y, v.z, 0.0];
        self.surfaces.sync(ctx.device, ctx.queue);
        let uniform = Uniform {
            wall: slot(B_PARED),
            neon: slot(B_NEON),
            background: slot(B_FONDO),
            a: [ctx.aspect(), ctx.time, if ctx.fondo_alpha { 1.0 } else { 0.0 }, eye.z],
            s: [own[F_CUELLO], own[F_RADIO], own[F_SECCION], own[F_TORSION]],
            m: [own[F_ESPIRAL], own[F_RODILLO], tan_half, 0.0],
            tx: [own[F_TIPO], own[F_ESCALA], own[F_VEL], own[F_ANILLOS]],
            t2: [own[F_ANCHO], own[F_PANEL], own[F_LUCES], own[F_PULSO]],
            f: [own[F_NIEBLA], own[F_NMIX], own[F_NEON], 0.0],
            l: [look.shine, look.contrast, look.opacity, look.background_level],
            l2: [look.tone, look.saturation, 0.0, 0.0],
            eye: v4(eye),
            right: v4(right),
            up: v4(up),
            fwd: v4(fwd),
            mat_body: self.surfaces.uniform(0),
        };
        self.pass.draw_textured(ctx, target, format, &uniform, &self.surfaces.bind_pairs());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
    const W: u32 = 64;
    const TIMES: [f32; 2] = [0.0, 0.7];

    fn frame_at(gpu: &crate::gpu::Gpu, model: &mut Catenoide, floats: &[f32], time: f32, w: u32, h: u32) -> Vec<u8> {
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time, width: w, height: h, fondo_alpha: false };
        model.render(&ctx, floats, &view, FORMAT);
        crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, w, h)
    }

    fn frames(gpu: &crate::gpu::Gpu, floats: &[f32]) -> Vec<u8> {
        let mut m = Catenoide::new();
        TIMES.iter().flat_map(|&t| frame_at(gpu, &mut m, floats, t, W, W)).collect()
    }

    fn base() -> Vec<f32> {
        let mut f = SCHEMA.defaults();
        for v in &mut f[off(B_FONDO)..off(B_FONDO) + 12] {
            *v = 0.3;
        }
        f
    }

    fn colours(px: &[u8]) -> usize {
        let mut s = std::collections::HashSet::new();
        for p in px.chunks_exact(4) {
            s.insert([p[0], p[1], p[2]]);
        }
        s.len()
    }

    #[test]
    fn widths_come_from_the_blocks() {
        assert_eq!(SCHEMA.defaults().len(), SCHEMA.width());
        assert_eq!(off(B_OWN), 0);
        assert_eq!(off(B_PARED), FLOATS.len());
        assert_eq!(off(B_CAM) + kit::Cam3d::N, off(B_LOOK));
        assert_eq!(off(B_LOOK) + kit::Look::N, off(B_LOOK + 1));
        assert!(!Catenoide::new().camera_track());
    }

    #[test]
    fn indices_match_the_names() {
        let n = |i: usize| FLOATS[i].name;
        assert_eq!((n(F_CUELLO), n(F_RADIO), n(F_SECCION), n(F_TORSION), n(F_ESPIRAL)), ("neck", "radius", "section", "torsion", "spiral"));
        assert_eq!((n(F_AVANCE), n(F_RITMO), n(F_RODILLO), n(F_TIPO), n(F_ESCALA), n(F_VEL)), ("advance", "advance_rhythm", "roller", "texture_type", "texture_scale", "texture_speed"));
        assert_eq!((n(F_ANILLOS), n(F_ANCHO), n(F_PANEL), n(F_LUCES), n(F_PULSO)), ("rings_density", "rings_width", "panel_density", "lights_density", "lights_pulse"));
        assert_eq!((n(F_NIEBLA), n(F_NMIX), n(F_NEON)), ("fog", "fog_color_mix", "edges_neon"));
        assert_eq!(FLOATS.len(), F_NEON + 1);
        assert!(FLOATS[F_SECCION].drive < 1.0 && FLOATS[F_TIPO].drive < 1.0);
    }

    #[test]
    fn the_cpu_catenoid_profile_matches_cosh() {
        // Neck at z = 0 is `radio`; r = radio·cosh(z/c) inside the period.
        assert!((catenoid_radius(0.0, 1.5, 2.0) - 1.5).abs() < 1e-6);
        for &z in &[0.5f32, 1.0, 2.5, -3.0] {
            let want = 1.5 * (z / 2.0).cosh();
            assert!((catenoid_radius(z, 1.5, 2.0) - want).abs() < 1e-4, "z={z}");
        }
        // Symmetric about the neck, periodic, never below the neck.
        assert!((catenoid_radius(1.7, 1.0, 2.0) - catenoid_radius(-1.7, 1.0, 2.0)).abs() < 1e-5);
        assert!((catenoid_radius(1.7, 1.0, 2.0) - catenoid_radius(1.7 + PERIOD, 1.0, 2.0)).abs() < 1e-3);
        assert!((catenoid_radius(PERIOD * 0.5, 1.0, 2.0) - 2.0f32.cosh()).abs() < 1e-3);
        // A larger c opens the neck: flatter profile.
        assert!(catenoid_radius(3.0, 1.0, 8.0) < catenoid_radius(3.0, 1.0, 1.0));
        // Capped and finite for the tightest neck.
        let r = catenoid_radius(4.0, 3.0, 0.0);
        assert!(r.is_finite() && r <= 3.0 * 4.0f32.cosh() + 1e-3);
        // Sections: circle is 1; polygons/stars stay inside [0.5, 1.3].
        for i in 0..64 {
            let a = i as f32 * 0.1;
            assert!((section(a, 0.0) - 1.0).abs() < 1e-6);
            for s in [0.5, 1.0, 1.5, 2.0, 2.5, 3.0] {
                let v = section(a, s);
                assert!((0.5..1.3).contains(&v), "section({a},{s}) = {v}");
            }
        }
        // Square: corner at angle 0 (.85·√2), flat wall (inradius .85) at π/4.
        assert!((section(0.0, 1.0) - 0.85 * 2f32.sqrt()).abs() < 1e-3);
        assert!((section(std::f32::consts::FRAC_PI_4, 1.0) - 0.85).abs() < 1e-4);
    }

    #[test]
    fn the_defaults_are_not_flat_and_deterministic() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let a = frames(&gpu, &f);
        assert_eq!(a, frames(&gpu, &f));
        let first = &a[..(W * W * 4) as usize];
        assert!(colours(first) > 16, "default frame is flat ({} colours)", colours(first));
        assert_ne!(first, &a[(W * W * 4) as usize..], "the tunnel must move with time");
    }

    #[test]
    fn every_float_moves_the_frame() {
        let gpu = crate::gpu::Gpu::new_headless();
        let (cam, look) = (off(B_CAM), off(B_LOOK));
        let mut idx: Vec<(usize, FloatSpec)> = FLOATS.iter().enumerate().map(|(i, s)| (i, *s)).collect();
        idx.extend(kit::CAM3D.iter().enumerate().map(|(i, s)| (cam + i, *s)));
        idx.extend(kit::LOOK.iter().enumerate().map(|(i, s)| (look + i, *s)));
        let b = base();
        for (i, spec) in idx {
            // Texture-specific floats only show on their own texture type.
            let tipo = match spec.name {
                "panel_density" => Some(1.0),
                "lights_density" | "lights_pulse" => Some(2.0),
                "spiral" => Some(3.0),
                _ => None,
            };
            let mut b = b.clone();
            if let Some(t) = tipo {
                b[F_TIPO] = t;
            }
            let reference = frames(&gpu, &b);
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
        for pick in 0..4 {
            let mut f = base();
            for (i, s) in FLOATS.iter().enumerate() {
                f[i] = match pick {
                    0 => s.min,
                    1 => s.max,
                    2 => if i % 2 == 0 { s.min } else { s.max },
                    _ => if i % 2 == 0 { s.max } else { s.min },
                };
            }
            // Out of range / non-finite inputs are clamped, not propagated.
            if pick == 3 {
                f[F_CUELLO] = 0.0;
                f[F_RADIO] = f32::NAN;
                f[F_SECCION] = 99.0;
            }
            let px = frames(&gpu, &f);
            assert!(colours(&px) > 3, "corner {pick} drew a flat frame ({} colours)", colours(&px));
        }
    }

    #[test]
    fn a_straight_pipe_looks_down_a_vanishing_point() {
        // Circle, c large (near-cylinder): the centre of the frame looks far
        // down the tube so it is darker/foggier than a ring near the edge.
        let gpu = crate::gpu::Gpu::new_headless();
        let mut f = SCHEMA.defaults();
        f[F_SECCION] = 0.0;
        f[F_CUELLO] = 8.0;
        f[F_TORSION] = 0.0;
        f[off(B_CAM) + 1] = 0.0; // pitch 0
        let n = 64u32;
        let px = frame_at(&gpu, &mut Catenoide::new(), &f, 0.0, n, n);
        let at = |x: u32, y: u32| {
            let i = ((y * n + x) * 4) as usize;
            px[i] as i32 + px[i + 1] as i32 + px[i + 2] as i32
        };
        // Circular symmetry about the centre: opposite pixels match closely.
        let d = (at(8, 32) - at(55, 32)).abs() + (at(32, 8) - at(32, 55)).abs();
        assert!(d < 120, "a centred circular tunnel should be near-symmetric, off by {d}");
        assert!(colours(&px) > 8);
    }

    /// Every kind is a surface: at `mat_mix` 1 it changes the frame, and
    /// clearing `body` restores the unpainted frame byte for byte.
    #[test]
    fn a_body_material_changes_the_frame_and_clearing_it_restores_it() {
        use crate::model::{MaterialSpec, SurfaceSpec};
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let mut model = Catenoide::new();
        assert_eq!(model.surfaces(), &["body"]);
        let plain = frame_at(&gpu, &mut model, &f, 1.0, W, W);
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
            let painted = frame_at(&gpu, &mut model, &f, 1.0, W, W);
            assert_ne!(plain, painted, "{name} did not change the frame");
            model.set_surface("body", None);
            let restored = frame_at(&gpu, &mut model, &f, 1.0, W, W);
            assert_eq!(plain, restored, "clearing {name} did not restore the frame");
        }
    }
}
