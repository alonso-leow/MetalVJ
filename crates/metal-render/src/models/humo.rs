//! Model 28 — Smoke (`smoke`). Smoke rising off a source, curled by a warp and
//! lit from a direction.
//!
//! The one real gap in the suite's vocabulary. Twenty-seven renderers and every
//! one of them is either a hard-edged object or a field with visible structure
//! in it; nothing was soft, and nothing had an edge you could not point at. A
//! room that has been shown spirals, glitch, wireframes and a bouncing ball for
//! an hour has been shown twenty-seven versions of the same *hardness*.
//!
//! # It is not a raymarch, on purpose
//!
//! A march through a 3D field is 24 to 64 samples of a 3D fbm per pixel. That
//! would be the most expensive renderer here by a wide margin, in a project
//! that still has no way to measure what a renderer costs (`docs/NEXT.md`), on
//! a laptop that is also driving a venue. Layered 2D at four octaves is about a
//! tenth of it.
//!
//! What that gives up is genuine self-occlusion. What it keeps is the thing
//! that actually makes smoke read as smoke — the shape of the edge: soft,
//! curling, torn. That comes from the domain warp and the threshold, and both
//! are here in full. The lighting is a one-sample gradient toward `luz_dir`,
//! which is not correct and is not trying to be; it is the term that turns a
//! flat blob into something with a lit side. The shader header has the whole
//! argument.
//!
//! # Where the smoke comes from
//!
//! `origen_y` and `alcance` are the difference between smoke and fog. Without a
//! source the frame is uniformly cloudy and the model is a texture; with one it
//! is a thing happening in a place, which is what can be aimed at a mask.
//!
//! # This model uses the hardened noise, and it is the reason it was hardened
//!
//! `lattice_fbm` and `quintic_noise`, not `fbm`/`noise2`. The old pair plateaus
//! once its argument leaves the low hundreds — whole runs of cells return one
//! value and the field grows flat axis-aligned rectangles. This model warps a
//! field by a field, scaled up to five times, scrolled by a clock that only
//! goes up: it walks the coordinate out to any range you like, and it is a
//! model with no straight line anywhere in its design. `shaders/shared.wgsl`
//! has the diagnosis, from when Mengano and Solvent each found half of it.

use crate::model::{color_at, background, spec, spec_held, Block, Ctx, FloatSpec, Model, Schema};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::ColorSlotGpu;

/// Matches `MAX_CAPAS` in humo.wgsl — two statements of one number.
pub const MAX_LAYERS: f32 = 5.0;

const FLOATS: &[FloatSpec] = &[
    // Both structural: layers and octaves are how much field there is, and a
    // signal swinging them would make the smoke coarser and finer several
    // times a second rather than move it.
    spec_held("layers", 1.0, MAX_LAYERS, 3.0, 0.15),
    spec("rise", 0.0, 2.0, 0.38),
    spec("scale", 0.3, 6.0, 1.5),
    spec_held("detail", 1.0, 6.0, 4.0, 0.15),
    spec("swirl", 0.0, 1.5, 0.55),
    spec("density", 0.0, 2.0, 1.0),
    // The two that decide whether it is smoke or fog.
    spec("cut", 0.0, 1.0, 0.46),
    spec("softness", 0.005, 1.0, 0.3),
    spec("light_direction", 0.0, 6.2832, 1.2),
    spec("light", 0.0, 2.0, 0.9),
    spec("drift", -1.0, 1.0, 0.08),
    spec("origin_y", -1.5, 1.5, -1.1),
    spec("reach", 0.2, 3.0, 1.9),
];

/// Not white, and not four greys either. The four stops here are the *shadowed
/// and lit sides of one material* — that is what the shader walks the gradient
/// by — so they open as a cool dark, two warming greys and a hot rim. Four
/// identical stops give a flat cloud with the lighting term switched off in
/// everything but name, which is the one setting that makes this model look
/// like a mistake.
const SMOKE: [f32; 12] = [
    0.06, 0.07, 0.10, //
    0.22, 0.23, 0.28, //
    0.55, 0.54, 0.56, //
    0.96, 0.90, 0.80,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[Block::Floats(FLOATS), color_at("smoke", &SMOKE), background(), crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK, crate::model::EFFECTS_COLOR_BLOCK],
};

/// How much smoke is allowed at a height, given where the source is and how
/// far it gets — the shader's `column` term.
///
/// On the CPU because it is the difference between smoke and fog, and because
/// it is the one thing here that can silently be zero everywhere: a source
/// above the top of the frame, or an `alcance` shorter than the gap to it, and
/// the model renders a perfectly clean empty frame with every float in range.
pub fn column_at(y: f32, origin_y: f32, reach: f32) -> f32 {
    let up = y - origin_y;
    if up < -0.1 {
        return 0.0;
    }
    let a = smoothstep(-0.1, 0.15, up);
    let b = 1.0 - smoothstep(0.0, reach.max(0.05), up.max(0.0));
    a * b
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    smoke: ColorSlotGpu,
    background: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
}

pub struct Humo {
    pass: Fullscreen,
}

impl Humo {
    pub fn new() -> Humo {
        Humo { pass: Fullscreen::new("smoke", include_str!("../../shaders/humo.wgsl")) }
    }
}

impl Model for Humo {
    fn id(&self) -> &'static str {
        "smoke"
    }

    fn name(&self) -> &'static str {
        "Smoke"
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
        let uniform = Uniform {
            smoke: ColorSlotGpu::from_floats(&floats[13..25]),
            background: ColorSlotGpu::from_floats(&floats[25..37]),
            a: [ctx.aspect(), ctx.time, floats[0].round(), floats[1]],
            b: [floats[2], floats[3].round(), floats[4], floats[5]],
            c: [floats[6], floats[7], floats[8], floats[9]],
            d: [floats[10], floats[11], floats[12], 0.0],
        };
        self.pass.draw(ctx, target, format, &uniform);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> Vec<f32> {
        SCHEMA.defaults()
    }

    #[test]
    fn the_schema_and_the_slices_render_reads_are_the_same_width() {
        // 13 floats + two colour slots. `render` slices `[13..25]` and `[25..37]`.
        assert_eq!(SCHEMA.width(), 37 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 37 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    #[test]
    fn the_default_smoke_actually_reaches_the_frame() {
        // The silent failure this model has: a source below the bottom edge
        // with an `alcance` too short to climb to it renders a clean empty
        // frame, deterministic, no NaN, every float in range. The render gate
        // would pass it and so would a glance at the sliders.
        let f = row();
        let (origin_y, reach) = (f[11], f[12]);
        let mut best = 0.0f32;
        let mut y = -1.0;
        while y <= 1.0 {
            best = best.max(column_at(y, origin_y, reach));
            y += 0.02;
        }
        assert!(best > 0.5, "the default smoke never fills more than {best} of any row");
    }

    #[test]
    fn there_is_no_smoke_below_the_source() {
        // The whole reason the source exists: smoke that appeared under its own
        // origin is fog with a gradient on it.
        let f = row();
        assert_eq!(column_at(f[11] - 0.5, f[11], f[12]), 0.0);
        assert_eq!(column_at(f[11] - 0.11, f[11], f[12]), 0.0);
    }

    #[test]
    fn the_smoke_dissipates_within_its_reach() {
        // And the other half: smoke that never thins out is a wall of grey
        // across the top of the frame.
        let f = row();
        let far = column_at(f[11] + f[12] + 0.2, f[11], f[12]);
        assert!(far < 0.05, "still {far} dense past its own alcance");
    }

    #[test]
    fn every_source_position_in_range_leaves_the_frame_finite() {
        for &origin_y in &[-1.5f32, -1.1, 0.0, 1.5] {
            for &reach in &[0.2f32, 1.9, 3.0] {
                for &y in &[-1.0f32, 0.0, 1.0] {
                    let c = column_at(y, origin_y, reach);
                    assert!(c.is_finite() && (0.0..=1.0).contains(&c));
                }
            }
        }
    }
}
