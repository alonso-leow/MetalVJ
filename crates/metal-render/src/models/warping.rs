//! Warping — a hydra-style chained transform synth, in one fixed stage order:
//! geometry -> warp (modulate) -> source -> colour -> feedback. Every stage has
//! an amount where the neutral value is an exact bypass, so a patch is composed
//! by raising amounts (the way hydra's `Mutator` grows one). Written from the
//! documented behaviour of each transform; no code was taken from hydra or
//! hydra-synth.
//!
//! **State.** The composed frame is rendered by `shaders/warping_step.wgsl`
//! into a [`PingPong`] pair and shown by `shaders/warping.wgsl` through the
//! CAM2D and LOOK kits (so the camera never compounds into the feedback). The
//! pair is always allocated; the previous frame is only read when `fb_cant > 0`.
//! One step per render call in which time advanced: the same instant twice
//! steps nothing, and a rewind, a seek or a frame resize starts over.
//!
//! **Deviations from the brief's names.** The brief's own `contraste` collides
//! with LOOK's, so the in-loop one is `contrasta`; `hue` (in the loop, before
//! the camera) is distinct from LOOK's `tono` (after it). TWIST is skipped:
//! `kaleid` and the modulate stage already are its mirror and warp. Added
//! `fb_modo` (blend / layer / diff / mask) for the brief's feedback verbs.

use crate::model::{color_at, background, spec, spec_held, Block, Ctx, FloatSpec, Model, Schema};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::kit::{self, CAM2D_BLOCK, LOOK_BLOCK};
use crate::shared::sim::PingPong;
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    // source mix
    spec("osc_mix", 0.0, 1.0, 0.8),
    spec("noise_mix", 0.0, 1.0, 0.0),
    spec("voronoi_mix", 0.0, 1.0, 0.0),
    spec("shape_mix", 0.0, 1.0, 0.0),
    spec("gradient_mix", 0.0, 1.0, 0.0),
    // source shape
    spec_held("osc_freq", 1.0, 120.0, 14.0, 0.2),
    spec("osc_sync", -2.0, 2.0, 0.3),
    spec("osc_off", 0.0, 1.0, 0.0),
    spec("noise_scale", 0.5, 20.0, 3.0),
    spec("noise_speed", 0.0, 2.0, 0.3),
    spec("voronoi_scale", 1.0, 30.0, 5.0),
    spec("voronoi_blend", 0.0, 1.0, 0.3),
    spec_held("shape_sides", 3.0, 12.0, 5.0, 0.2),
    spec("shape_radius", 0.05, 1.0, 0.35),
    spec("shape_soft", 0.001, 0.5, 0.04),
    // geometry
    spec("rot", -0.5, 0.5, 0.0),
    spec("rot_speed", -1.0, 1.0, 0.0),
    spec("scale", 0.2, 4.0, 1.0),
    spec_held("kaleid", 0.0, 12.0, 0.0, 0.2),
    spec("pixel", 0.0, 1.0, 0.0),
    spec("repeat_x", 1.0, 8.0, 1.0),
    spec("repeat_y", 1.0, 8.0, 1.0),
    spec("scroll_x", -1.0, 1.0, 0.0),
    spec("scroll_y", -1.0, 1.0, 0.0),
    spec("scroll_speed", -1.0, 1.0, 0.0),
    // modulate
    spec("mod_amount", 0.0, 1.0, 0.0),
    // 0 noise, 1 osc, 2 voronoi cells, 3 the picture itself
    spec_held("mod_source", 0.0, 3.0, 0.0, 0.2),
    spec("mod_scale", 0.1, 8.0, 2.0),
    // colour
    spec("hue", 0.0, 1.0, 0.0),
    spec("colorful", 0.0, 1.0, 0.85),
    spec("saturate", 0.0, 2.0, 1.0),
    spec("contrasting", 0.2, 4.0, 1.0),
    spec("threshold", 0.0, 1.0, 0.0),
    spec("posterize", 0.0, 1.0, 0.0),
    spec("invert", 0.0, 1.0, 0.0),
    // feedback
    spec("feedback_amount", 0.0, 1.0, 0.0),
    spec("feedback_zoom", -0.5, 0.5, 0.02),
    spec("feedback_turn", -0.5, 0.5, 0.0),
    // 0 blend, 1 layer, 2 diff, 3 mask
    spec_held("feedback_mode", 0.0, 3.0, 0.0, 0.2),
];

/// Not grey: colorama maps brightness through this, so it carries the look.
const PALETA: [f32; 12] = [0.02, 0.02, 0.18, 0.55, 0.05, 0.50, 1.00, 0.30, 0.12, 1.00, 0.85, 0.45];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("palette", &PALETA),
        background(),
        CAM2D_BLOCK,
        LOOK_BLOCK,
        crate::model::SIGNAL_FLOATS_BLOCK,
        crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

const B_OWN: usize = 0;
const B_PALETA: usize = 1;
const B_FONDO: usize = 2;
const B_CAM: usize = 3;
const B_LOOK: usize = 4;

fn off(block: usize) -> usize {
    kit::offset_of(SCHEMA.blocks, block)
}

/// Index of an own float by name, so the code never hand-counts.
fn fi(name: &str) -> usize {
    FLOATS.iter().position(|s| s.name == name).unwrap_or_else(|| panic!("no float {name}"))
}

/// A forward jump larger than this is a seek and starts the feedback over.
const SEEK_GAP: f32 = 60.0;
const MAX_STATE_W: u32 = 1280;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct StepUniform {
    pal: ColorSlotGpu,
    v: [[f32; 4]; 11],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DisplayUniform {
    background: ColorSlotGpu,
    v: [[f32; 4]; 4],
}

fn step_src() -> &'static str {
    static SRC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SRC.get_or_init(|| format!("{}\n{}", kit::WGSL, include_str!("../../shaders/warping_step.wgsl")))
}

fn display_src() -> &'static str {
    static SRC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SRC.get_or_init(|| format!("{}\n{}", kit::WGSL, include_str!("../../shaders/warping.wgsl")))
}

/// State size for a frame: capped in width, aspect kept.
pub fn state_size(width: u32, height: u32) -> (u32, u32) {
    let w = width.clamp(8, MAX_STATE_W);
    let h = ((height.max(1) as f32) * (w as f32 / width.max(1) as f32)).round().max(8.0) as u32;
    (w, h)
}

pub struct Warping {
    step: Fullscreen,
    display: Fullscreen,
    state: Option<PingPong>,
    last_time: Option<f32>,
}

impl Warping {
    pub fn new() -> Warping {
        Warping {
            step: Fullscreen::with_textures("warping step", step_src(), 1),
            display: Fullscreen::with_textures("warping", display_src(), 1),
            state: None,
            last_time: None,
        }
    }
}

impl Default for Warping {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Warping {
    fn id(&self) -> &'static str {
        "warping"
    }

    fn name(&self) -> &'static str {
        "Warping"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn render(
        &mut self,
        ctx: &Ctx,
        floats: &[f32],
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        let own = &floats[off(B_OWN)..off(B_OWN) + FLOATS.len()];
        let o = |n: &str| own[fi(n)];
        let aspect = ctx.aspect();
        let (w, h) = state_size(ctx.width, ctx.height);

        let rebuilt = self.state.as_ref().map_or(true, |s| (s.width, s.height) != (w, h));
        if rebuilt {
            self.state = Some(PingPong::new(ctx.device, w, h));
        }
        let state = self.state.as_mut().expect("state built above");

        // One step per render in which time moved forward; a rewind or a seek
        // forgets the feedback, the same instant twice does nothing.
        let (do_step, fresh) = match self.last_time {
            None => (true, true),
            _ if rebuilt => (true, true),
            Some(prev) => {
                let dt = ctx.time - prev;
                if !(0.0..=SEEK_GAP).contains(&dt) {
                    (true, true)
                } else {
                    (dt > 0.0, false)
                }
            }
        };
        self.last_time = Some(ctx.time);
        if rebuilt {
            state.reset();
        }
        if fresh {
            state.reset();
        }
        let pal = ColorSlotGpu::from_floats(&floats[off(B_PALETA)..off(B_PALETA) + 12]);
        let su = StepUniform {
            pal,
            v: [
                [aspect, ctx.time, if state.fresh() { 1.0 } else { 0.0 }, o("feedback_mode")],
                [o("osc_mix"), o("noise_mix"), o("voronoi_mix"), o("shape_mix")],
                [o("gradient_mix"), o("osc_freq"), o("osc_sync"), o("osc_off")],
                [o("noise_scale"), o("noise_speed"), o("voronoi_scale"), o("voronoi_blend")],
                [o("shape_sides"), o("shape_radius"), o("shape_soft"), o("rot")],
                [o("rot_speed"), o("scale"), o("kaleid"), o("pixel")],
                [o("repeat_x"), o("repeat_y"), o("scroll_x"), o("scroll_y")],
                [o("scroll_speed"), o("mod_amount"), o("mod_source"), o("mod_scale")],
                [o("hue"), o("colorful"), o("saturate"), o("contrasting")],
                [o("threshold"), o("posterize"), o("invert"), o("feedback_amount")],
                [o("feedback_zoom"), o("feedback_turn"), 0.0, 0.0],
            ],
        };
        if do_step {
            state.step(ctx, &mut self.step, &su);
        }

        let cam = kit::Cam2d::from_floats(&floats[off(B_CAM)..off(B_CAM) + kit::Cam2d::N]);
        let look = kit::Look::from_floats(&floats[off(B_LOOK)..off(B_LOOK) + kit::Look::N]);
        let du = DisplayUniform {
            background: ColorSlotGpu::from_floats(&floats[off(B_FONDO)..off(B_FONDO) + 12]),
            v: [
                [aspect, ctx.time, if ctx.fondo_alpha { 1.0 } else { 0.0 }, 0.0],
                [cam.zoom, cam.pan_x, cam.pan_y, cam.turn],
                [cam.drift, look.shine, look.contrast, look.opacity],
                [look.background_level, look.tone, look.saturation, 0.0],
            ],
        };
        let (view, sampler) = state.current();
        self.display.draw_textured(ctx, target, format, &du, &[(view, sampler)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
    const W: u32 = 64;
    const TIMES: [f32; 4] = [0.0, 0.1, 0.2, 0.3];

    fn frames(gpu: &crate::gpu::Gpu, model: &mut Warping, floats: &[f32], times: &[f32]) -> Vec<u8> {
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, W, W, FORMAT);
        let mut out = vec![];
        for &t in times {
            let ctx = Ctx {
                device: &gpu.device,
                queue: &gpu.queue,
                time: t,
                width: W,
                height: W,
                fondo_alpha: false, detail: 1.0,
            };
            model.render(&ctx, floats, &view, FORMAT);
            out = crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, W, W);
        }
        out
    }

    /// Every stage switched on, so each float has something to act on.
    fn base() -> Vec<f32> {
        let mut f = SCHEMA.defaults();
        let o = off(B_OWN);
        for (n, v) in [
            ("noise_mix", 0.4),
            ("voronoi_mix", 0.4),
            ("shape_mix", 0.4),
            ("gradient_mix", 0.4),
            ("mod_amount", 0.4),
            ("feedback_amount", 0.5),
            ("kaleid", 0.0),
        ] {
            f[o + fi(n)] = v;
        }
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
        assert_eq!(off(B_LOOK) + kit::Look::N, off(B_LOOK + 1));
        assert!(FLOATS.len() <= 40, "own floats exceed the ~40 cap");
        let mut names: Vec<&str> = FLOATS.iter().map(|s| s.name).collect();
        names.extend(kit::CAM2D.iter().chain(kit::LOOK).map(|s| s.name));
        let n = names.len();
        names.sort();
        names.dedup();
        assert_eq!(n, names.len(), "a float name repeats inside the row");
    }

    #[test]
    fn every_float_moves_the_frame() {
        let gpu = crate::gpu::Gpu::new_headless();
        let own_end = FLOATS.len();
        let (cam, look) = (off(B_CAM), off(B_LOOK));
        let mut idx: Vec<(usize, FloatSpec)> = FLOATS.iter().copied().enumerate().collect();
        idx.extend(kit::CAM2D.iter().enumerate().map(|(i, s)| (cam + i, *s)));
        idx.extend(kit::LOOK.iter().enumerate().map(|(i, s)| (look + i, *s)));
        assert!(idx.len() > own_end);
        let b = base();
        let reference = frames(&gpu, &mut Warping::new(), &b, &TIMES);
        for (i, spec) in idx {
            let far = if (spec.max - b[i]).abs() > (b[i] - spec.min).abs() { spec.max } else { spec.min };
            // Hue-like floats are a full turn over 0..1: the far end is the start.
            let far = if spec.name == "tone" || spec.name == "hue" { 0.5 } else { far };
            // The state texture repeats, so a whole-frame pan is a no-op.
            let far = if spec.name.starts_with("pan_") { 0.37 } else { far };
            let mut f = b.clone();
            f[i] = far;
            let got = frames(&gpu, &mut Warping::new(), &f, &TIMES);
            assert!(reference != got, "float `{}` ({} -> {far}) did not move the frame", spec.name, b[i]);
        }
    }

    #[test]
    fn the_defaults_are_not_flat_and_deterministic() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let a = frames(&gpu, &mut Warping::new(), &f, &TIMES);
        let b = frames(&gpu, &mut Warping::new(), &f, &TIMES);
        assert_eq!(a, b);
        let mut distinct = std::collections::HashSet::new();
        for px in a.chunks_exact(4) {
            distinct.insert([px[0], px[1], px[2]]);
        }
        assert!(distinct.len() > 8, "default frame is flat ({} colours)", distinct.len());
    }

    #[test]
    fn feedback_persists_and_holds_still_when_time_does_not_advance() {
        let gpu = crate::gpu::Gpu::new_headless();
        let mut f = SCHEMA.defaults();
        let fb = off(B_OWN) + fi("feedback_amount");
        // With feedback the last frame depends on the history before it.
        f[fb] = 0.9;
        let history = frames(&gpu, &mut Warping::new(), &f, &TIMES);
        let alone = frames(&gpu, &mut Warping::new(), &f, &[0.3]);
        assert_ne!(history, alone, "feedback left no trace of earlier frames");
        // Without feedback it does not.
        f[fb] = 0.0;
        let history = frames(&gpu, &mut Warping::new(), &f, &TIMES);
        let alone = frames(&gpu, &mut Warping::new(), &f, &[0.3]);
        assert_eq!(history, alone, "fb_cant = 0 must be stateless");
        // The same instant twice steps nothing.
        f[fb] = 0.9;
        let mut m = Warping::new();
        let a = frames(&gpu, &mut m, &f, &TIMES);
        let again = frames(&gpu, &mut m, &f, &[0.3]);
        assert_eq!(a, again);
        let later = frames(&gpu, &mut m, &f, &[0.4]);
        assert_ne!(a, later);
    }

    #[test]
    fn a_rewind_or_resize_starts_over_safely() {
        let gpu = crate::gpu::Gpu::new_headless();
        let mut f = SCHEMA.defaults();
        f[off(B_OWN) + fi("feedback_amount")] = 0.9;
        let mut m = Warping::new();
        frames(&gpu, &mut m, &f, &TIMES);
        let rewound = frames(&gpu, &mut m, &f, &[0.0]);
        let fresh = frames(&gpu, &mut Warping::new(), &f, &[0.0]);
        assert_eq!(rewound, fresh);
    }

    #[test]
    fn state_size_keeps_aspect_and_caps_width() {
        assert_eq!(state_size(640, 360), (640, 360));
        let (w, h) = state_size(3840, 2160);
        assert_eq!(w, MAX_STATE_W);
        assert!((h as f32 / w as f32 - 2160.0 / 3840.0).abs() < 0.01);
    }
}
