//! The procedural scaffold: everything the scaffold-based models share, so one
//! of them is its float list, its colour slots, its WGSL body and a registry
//! line.
//!
//! Each effect is written from its underlying mathematics - the classic
//! image-processing and generative techniques (domain warping, threshold and
//! error-diffusion screening, metaball and voronoi fields, fbm noise).
//!
//! # Adding a model in an hour
//!
//! 1. Copy `models/colorpanels.rs` and `shaders/colorpanels.wgsl`.
//! 2. In the `.rs`: replace `FLOATS` (<= 40, names ASCII snake_case, enum props
//!    are `spec_held` stepped floats), the colour slot constants (0..=3 slots,
//!    each `[f32; 12]` = 4 colours x rgb, **linear**), and the two lines that
//!    name them in `scaffold_schema!` and `ScaffoldDef::new`.
//! 3. In the `.wgsl`: write `fn scaffold_shade(p, uv) -> vec4<f32>` (rgb, coverage).
//!    Read your floats with the generated `f_<name>()` accessors (one per entry
//!    of `FLOATS`, e.g. `f_densidad()`), colours with `p_pal(slot, t)`. The
//!    helpers in `shaders/scaffold_prelude.wgsl` (`p_rot`, `p_hash`, `p_hash2`,
//!    `p_noise`, `p_fbm`, `p_grain`, `p_edge`, `p_pal`, `p_pal2`, `p_time`,
//!    `p_aspect`) plus everything in `shared.wgsl` and `kit::WGSL` are in scope.
//! 4. `pub mod xxx;` in `models/mod.rs`, `use` + `spec!("xxx",
//!    Xxx)` in `registry.rs` (append at the end), `cargo run -p
//!    metal-render --bin emit-schema`. The gate allow-list is done at the end
//!    by `scripts/gate_add_models.py`, not per model.
//! 5. In the tests call `shared::scaffold::testing::check_all(build, &base)` where
//!    `base` switches every optional stage on so each float has an effect.
//!
//! # What the scaffold does for you
//!
//! Row layout (blocks, in order): `own floats`, `slot 0..n`, `fondo`, `CAM2D`
//! (zoom, pan_x, pan_y, giro, deriva), `TWIST` (warp_cant, warp_freq, espejo,
//! pulso), `LOOK` (brillo, contraste, opacidad, fondo_nivel, tono, saturacion),
//! then the standard signal/effects blocks. Per pixel, in `fs_main`: CAM2D
//! transform, pulse scale, mirror fold, domain warp -> your `scaffold_shade` ->
//! composite over the `fondo` slot at `background_level` -> pulse brightness ->
//! LOOK -> `opacidad` (alpha when `Ctx::fondo_alpha`, for layering). A model
//! therefore never touches the kits; `time` is `Ctx::time` (`p_time()`), speed
//! is just another own float.
//!
//! # Image-input shaders (optional source texture)
//!
//! Nine of these models distort a picture. There is no image asset: they take a
//! *source* through the surface mechanism (`Model::surfaces` / `set_surface`,
//! the same one `corals` uses for `relleno`). Declare it with
//! `ScaffoldDef::new_with_source(.., &["imagen"])` and the scaffold binds the
//! part's texture, forwards `surfaces()`/`set_surface()` from `scaffold_model!`,
//! and exposes in WGSL (file `shaders/scaffold_source.wgsl`):
//! `p_src(p) -> vec4<f32>` (linear rgba of the picture at picture coordinate
//! `p`, cover-fitted to the frame, mirrored at the edges), `p_src_uv(p)` and
//! `p_src_at_uv(uv)`, `p_has_src()`. With no source connected, `p_src` returns
//! a procedural stripe/gradient test image, so the model always renders. Models
//! declared with `ScaffoldDef::new` are unchanged (no binding, empty surfaces).
//!
//! Limits: 48 own floats (the uniform packs them in 12 vec4s; the ~40 row cap
//! is enforced by the layout test), at most 3 colour slots.

use std::sync::OnceLock;

use crate::model::{Ctx, FloatSpec, Model, Schema, SurfaceSpec};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::kit::{self, Cam2d, Look, Twist};
use crate::shared::surface::Part;
use crate::shared::ColorSlotGpu;

pub const MAX_OWN: usize = 48;
pub const MAX_SLOTS: usize = 3;

const PRELUDE_WGSL: &str = include_str!("../../shaders/scaffold_prelude.wgsl");
const SOURCE_WGSL: &str = include_str!("../../shaders/scaffold_source.wgsl");

/// Builds a model `Schema` in the scaffold's block order:
/// `scaffold_schema!(FLOATS; "paleta" => PALETA, ...)`.
#[macro_export]
macro_rules! scaffold_schema {
    ($floats:expr; $($name:literal => $pal:expr),* $(,)?) => {
        $crate::model::Schema {
            blocks: &[
                $crate::model::Block::Floats($floats),
                $($crate::model::color_at($name, &$pal),)*
                $crate::model::background(),
                $crate::shared::kit::CAM2D_BLOCK,
                $crate::shared::kit::TWIST_BLOCK,
                $crate::shared::kit::LOOK_BLOCK,
                $crate::model::SIGNAL_FLOATS_BLOCK,
                $crate::model::EFFECTS_FLOATS_BLOCK,
                $crate::model::EFFECTS_COLOR_BLOCK,
            ],
        }
    };
}

/// Declares the model struct a registry `spec!` needs:
/// `scaffold_model!(Xxx, DEF);` gives `Xxx::new()` and `impl Model`.
#[macro_export]
macro_rules! scaffold_model {
    ($ty:ident, $def:path) => {
        pub struct $ty($crate::shared::scaffold::Scaffold);
        impl $ty {
            pub fn new() -> $ty {
                $ty($crate::shared::scaffold::Scaffold::new(&$def))
            }
        }
        impl Default for $ty {
            fn default() -> Self {
                Self::new()
            }
        }
        impl $crate::model::Model for $ty {
            fn id(&self) -> &'static str {
                self.0.def.id
            }
            fn name(&self) -> &'static str {
                self.0.def.name
            }
            fn schema(&self) -> &'static $crate::model::Schema {
                self.0.def.schema
            }
            fn surfaces(&self) -> &'static [&'static str] {
                self.0.def.surfaces
            }
            fn set_surface(&mut self, part: &str, spec: Option<$crate::model::SurfaceSpec>) {
                self.0.set_surface(part, spec);
            }
            fn render(
                &mut self,
                ctx: &$crate::model::Ctx,
                floats: &[f32],
                target: &wgpu::TextureView,
                format: wgpu::TextureFormat,
            ) {
                self.0.render(ctx, floats, target, format);
            }
        }
    };
}

/// Static description of one scaffold model.
pub struct ScaffoldDef {
    pub id: &'static str,
    pub name: &'static str,
    /// Must be built with `scaffold_schema!` from the same `own` and `slots`.
    pub schema: &'static Schema,
    pub own: &'static [FloatSpec],
    /// Number of own colour slots (0..=3) between `own` and `fondo`.
    pub slots: usize,
    /// The model's WGSL body (defines `scaffold_shade`).
    pub wgsl: &'static str,
    /// Optional source-picture parts (0 or 1 entry); see the module doc.
    pub surfaces: &'static [&'static str],
    src: OnceLock<String>,
}

impl ScaffoldDef {
    pub const fn new(
        id: &'static str,
        name: &'static str,
        schema: &'static Schema,
        own: &'static [FloatSpec],
        slots: usize,
        wgsl: &'static str,
    ) -> ScaffoldDef {
        ScaffoldDef { id, name, schema, own, slots, wgsl, surfaces: &[], src: OnceLock::new() }
    }

    /// As [`ScaffoldDef::new`] for an image-input shader: `surfaces` names the
    /// one picture part (e.g. `&["imagen"]`) the shader distorts.
    pub const fn new_with_source(
        id: &'static str,
        name: &'static str,
        schema: &'static Schema,
        own: &'static [FloatSpec],
        slots: usize,
        wgsl: &'static str,
        surfaces: &'static [&'static str],
    ) -> ScaffoldDef {
        ScaffoldDef { id, name, schema, own, slots, wgsl, surfaces, src: OnceLock::new() }
    }

    /// Block index of the layout positions, so nothing hand-counts.
    fn b_fondo(&self) -> usize {
        1 + self.slots
    }

    /// The full WGSL after the global prelude and the fullscreen vertex stage.
    pub fn source(&self) -> &str {
        self.src.get_or_init(|| {
            let mut acc = String::new();
            for (i, s) in self.own.iter().enumerate() {
                acc.push_str(&format!(
                    "fn f_{}() -> f32 {{ return P.own[{}u].{}; }}\n",
                    s.name,
                    i / 4,
                    ["x", "y", "z", "w"][i % 4]
                ));
            }
            let src = if self.surfaces.is_empty() { "" } else { SOURCE_WGSL };
            format!("{}\n{}\n{}\n{}\n{}", kit::WGSL, PRELUDE_WGSL, src, acc, self.wgsl)
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    background: ColorSlotGpu,
    slots: [ColorSlotGpu; MAX_SLOTS],
    a: [f32; 4],
    cam: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    tw: [f32; 4],
    own: [[f32; 4]; MAX_OWN / 4],
}

pub struct Scaffold {
    pub def: &'static ScaffoldDef,
    pass: Fullscreen,
    surface: Part,
}

impl Scaffold {
    pub fn new(def: &'static ScaffoldDef) -> Scaffold {
        assert!(def.own.len() <= MAX_OWN && def.slots <= MAX_SLOTS, "{}: too many floats or slots", def.id);
        let textures = def.surfaces.len().min(1);
        Scaffold { def, pass: Fullscreen::with_textures(def.id, def.source(), textures), surface: Part::default() }
    }

    /// Hands the source picture (if the def declares one) to the shader. A
    /// scaffold model is a 2D field, not a surface: only the picture half of the
    /// spec is kept, the material half is ignored.
    pub fn set_surface(&mut self, part: &str, spec: Option<SurfaceSpec>) {
        if self.def.surfaces.first() == Some(&part) {
            self.surface.set(spec);
        }
    }

    pub fn render(
        &mut self,
        ctx: &Ctx,
        floats: &[f32],
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        let d = self.def;
        let sch = d.schema;
        let at = |blk: usize| sch.offset(blk);
        let n = d.own.len();
        let mut own = [[0.0f32; 4]; MAX_OWN / 4];
        for i in 0..n {
            own[i / 4][i % 4] = floats[i];
        }
        let mut slots = [ColorSlotGpu::solid(0.0, 0.0, 0.0); MAX_SLOTS];
        for k in 0..d.slots {
            slots[k] = ColorSlotGpu::from_floats(&floats[at(1 + k)..at(1 + k) + 12]);
        }
        let fb = d.b_fondo();
        let cam = Cam2d::from_floats(&floats[at(fb + 1)..at(fb + 1) + Cam2d::N]);
        let tw = Twist::from_floats(&floats[at(fb + 2)..at(fb + 2) + Twist::N]);
        let lk = Look::from_floats(&floats[at(fb + 3)..at(fb + 3) + Look::N]);
        let mut src_aspect = 0.0;
        if !d.surfaces.is_empty() {
            self.surface.sync(ctx.device, ctx.queue);
            let slot = self.surface.slot();
            if !slot.is_fallback() {
                let (w, h) = slot.size();
                src_aspect = w as f32 / h.max(1) as f32;
            }
        }
        let u = Uniform {
            background: ColorSlotGpu::from_floats(&floats[at(fb)..at(fb) + 12]),
            slots,
            a: [ctx.aspect(), ctx.time, if ctx.fondo_alpha { 1.0 } else { 0.0 }, src_aspect],
            cam: [cam.zoom, cam.pan_x, cam.pan_y, cam.turn],
            b: [cam.drift, lk.shine, lk.contrast, lk.opacity],
            c: [lk.background_level, lk.tone, lk.saturation, 0.0],
            tw: [tw.warp_amount, tw.warp_freq, tw.mirror, tw.pulse],
            own,
        };
        if d.surfaces.is_empty() {
            self.pass.draw(ctx, target, format, &u);
        } else {
            let slot = self.surface.slot();
            self.pass.draw_textured(ctx, target, format, &u, &[(slot.view(), slot.sampler())]);
        }
    }
}

/// Shared standard checks for scaffold models. Test-only.
#[cfg(test)]
pub mod testing {
    use super::*;
    use crate::model::Block;

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
    const W: u32 = 64;
    const H: u32 = 48;
    const TIMES: [f32; 3] = [0.3, 0.8, 1.4];

    /// Renders the model at each time and returns the last frame's RGBA8.
    pub fn frame(gpu: &crate::gpu::Gpu, m: &mut dyn Model, floats: &[f32], times: &[f32]) -> Vec<u8> {
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, W, H, FORMAT);
        let mut out = vec![];
        for &t in times {
            let ctx = Ctx {
                device: &gpu.device,
                queue: &gpu.queue,
                time: t,
                width: W,
                height: H,
                fondo_alpha: false,
            };
            m.render(&ctx, floats, &view, FORMAT);
            out = crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, W, H);
        }
        out
    }

    /// Every (name, spec, row index) of the model's own floats and the three kits.
    fn tweakable(schema: &Schema, slots: usize) -> Vec<(usize, FloatSpec)> {
        let mut v = vec![];
        for blk in (0..1).chain(slots + 2..slots + 5) {
            if let Block::Floats(fs) = &schema.blocks[blk] {
                let off = schema.offset(blk);
                v.extend(fs.iter().enumerate().map(|(i, s)| (off + i, *s)));
            }
        }
        v
    }

    /// Own colour slots: the colour blocks right after the own floats, minus `fondo`.
    fn slots_of(s: &Schema) -> usize {
        s.blocks[1..].iter().take_while(|b| matches!(b, Block::ColorSlot(..))).count() - 1
    }

    fn base_row(schema: &Schema, slots: usize, base: &[(&str, f32)]) -> Vec<f32> {
        let mut f = schema.defaults();
        let tw = tweakable(schema, slots);
        for (n, v) in base {
            let i = tw.iter().find(|(_, s)| s.name == *n).unwrap_or_else(|| panic!("no float {n}")).0;
            f[i] = *v;
        }
        // A visible background, or `background_level` has nothing to scale.
        let fb = schema.offset(slots + 1);
        for v in &mut f[fb..fb + 12] {
            *v = 0.3;
        }
        f
    }

    pub fn check_layout(build: fn() -> Box<dyn Model>) {
        let m = build();
        let s = m.schema();
        assert_eq!(s.defaults().len(), s.width());
        let Block::Floats(own) = &s.blocks[0] else { panic!("block 0 must be the own floats") };
        assert!(own.len() <= 40, "own floats exceed the ~40 cap");
        let slots = slots_of(s);
        assert!(slots <= MAX_SLOTS);
        // Kits present, in order, right after `fondo`.
        let names = |blk: usize| -> Vec<&str> {
            let Block::Floats(f) = &s.blocks[blk] else { panic!("block {blk} is not floats") };
            f.iter().map(|x| x.name).collect()
        };
        assert_eq!(names(slots + 2), kit::CAM2D.iter().map(|x| x.name).collect::<Vec<_>>());
        assert_eq!(names(slots + 3), kit::TWIST.iter().map(|x| x.name).collect::<Vec<_>>());
        assert_eq!(names(slots + 4), kit::LOOK.iter().map(|x| x.name).collect::<Vec<_>>());
        let mut all: Vec<&str> = tweakable(s, slots).iter().map(|(_, x)| x.name).collect();
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(n, all.len(), "a float name repeats inside the row");
    }

    /// Deterministic, not black, not flat.
    pub fn check_defaults(build: fn() -> Box<dyn Model>) {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = build().schema().defaults();
        let a = frame(&gpu, &mut *build(), &f, &TIMES);
        let b = frame(&gpu, &mut *build(), &f, &TIMES);
        assert_eq!(a, b, "not deterministic");
        let lum: u32 = a.chunks_exact(4).map(|p| p[0] as u32 + p[1] as u32 + p[2] as u32).sum();
        assert!(lum / (W * H) > 6, "default frame is black");
        let mut distinct = std::collections::HashSet::new();
        for p in a.chunks_exact(4) {
            distinct.insert([p[0], p[1], p[2]]);
        }
        assert!(distinct.len() > 8, "default frame is flat ({} colours)", distinct.len());
    }

    /// Every own float and kit float changes the frame, starting from `base`
    /// (floats that switch optional stages on). Tries the far end, the near end
    /// and an off-grid mid value, so periodic floats (hue, rotations) still count.
    pub fn check_every_float_moves(build: fn() -> Box<dyn Model>, base: &[(&str, f32)]) {
        let gpu = crate::gpu::Gpu::new_headless();
        let m = build();
        let s = m.schema();
        let slots = slots_of(s);
        let b = base_row(s, slots, base);
        let reference = frame(&gpu, &mut *build(), &b, &TIMES);
        for (i, spec) in tweakable(s, slots) {
            let tries = [spec.max, spec.min, spec.min + (spec.max - spec.min) * 0.37];
            let moved = tries.iter().any(|&v| {
                if (v - b[i]).abs() < 1e-6 {
                    return false;
                }
                let mut f = b.clone();
                f[i] = v;
                frame(&gpu, &mut *build(), &f, &TIMES) != reference
            });
            assert!(moved, "float `{}` (base {}) did not move the frame", spec.name, b[i]);
        }
    }

    pub fn check_all(build: fn() -> Box<dyn Model>, base: &[(&str, f32)]) {
        check_layout(build);
        check_defaults(build);
        check_every_float_moves(build, base);
    }

    /// Average ms per frame at 1280x720 (GPU-synchronised readback each frame).
    pub fn measure_ms(build: fn() -> Box<dyn Model>) -> f32 {
        let gpu = crate::gpu::Gpu::new_headless();
        let mut m = build();
        let f = m.schema().defaults();
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, 1280, 720, FORMAT);
        let t0 = std::time::Instant::now();
        let n = 30;
        for i in 0..n {
            let ctx = Ctx {
                device: &gpu.device,
                queue: &gpu.queue,
                time: i as f32 * 0.05,
                width: 1280,
                height: 720,
                fondo_alpha: false,
            };
            m.render(&ctx, &f, &view, FORMAT);
            let _ = crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, 1280, 720);
        }
        t0.elapsed().as_secs_f32() * 1000.0 / n as f32
    }
}
