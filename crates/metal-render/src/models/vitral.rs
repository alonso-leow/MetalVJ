//! Model 27 — Stained Glass (`stained-glass`). The frame as a leaded window: panes of
//! glass with lead between them, drifting, lighting up a few at a time, and
//! optionally blown apart from the middle.
//!
//! A Voronoi diagram over a jittered lattice — the same 3x3 neighbourhood
//! Sandstorm indexes its grains with, put to the other thing it is good for.
//! The lead is `F2 - F1`, the gap between the nearest seed and the next
//! nearest, which is zero exactly on the boundary between two panes; the shader
//! header has why that is one subtraction instead of a data structure.
//!
//! # What it adds to the suite
//!
//! Nothing here shatters, and nothing here has *regions*. Twenty-six models
//! draw fields, figures and objects; this one partitions the frame, which is a
//! different way of filling it and reads as a different kind of thing on a
//! wall. It is also the only model whose reaction to a beat can be **local** —
//! three panes out of forty catching light is an event in a place, where every
//! other model's response to the same signal is the whole frame doing
//! something at once.
//!
//! # The one constraint that is not cosmetic
//!
//! Seeds must not leave their own lattice cell. A pixel searches the nine cells
//! around its own, so a seed that wandered further would be the nearest seed to
//! pixels that never look at it — and those pixels would attach to someone
//! else, tearing a pane along a straight lattice line. `deriva` is capped in
//! the shader at 0.35 of a cell against a jitter of at most 0.5 for exactly
//! that reason, and [`seed_stays_home`] is the test that keeps the two numbers
//! honest with each other.

use crate::model::{color_at, background, spec, spec_held, Block, Ctx, FloatSpec, Model, Schema};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::ColorSlotGpu;

/// How far a seed may be pushed from its cell centre by `irregular`, and by
/// `deriva`, in cells. Both are also written in vitral.wgsl; [`seed_stays_home`]
/// is the gate that stops them drifting apart.
pub const JITTER: f32 = 0.5;
pub const DRIFT: f32 = 0.35;

const FLOATS: &[FloatSpec] = &[
    // Structural: the number of panes is the composition. Driven by the music
    // the whole window would be re-cut on every transient.
    spec_held("cells", 2.0, 40.0, 11.0, 0.15),
    spec("irregular", 0.0, 1.0, 0.85),
    spec("drift", 0.0, 1.0, 0.18),
    spec("lead", 0.0, 0.09, 0.014),
    spec("bevel", 0.0, 1.0, 0.45),
    // The round the lit panes are dealt on — a clock, like Rings' `ronda`, and
    // held for the same reason: moved by the music it would re-deal mid-round.
    spec_held("period", 0.1, 4.0, 0.7, 0.15),
    spec("lit", 0.0, 1.0, 0.22),
    spec("flash", 0.0, 1.5, 0.7),
    spec("fall", 0.0, 1.0, 0.55),
    spec("burst", 0.0, 2.0, 0.0),
];

/// Cobalt, teal, amber, wine — four different pieces of glass, which is what
/// the model is for. Not white for the ordinary reason: four white stops make
/// every pane the same pane and the window stops being a window.
///
/// These are **linear** values, and that is why they look far too dark written
/// down. The targets this suite renders to encode sRGB on write, so 0.02 here
/// arrives on screen at about 0.16. Chosen against a rendered frame rather than
/// against the numbers: picked by eye in a colour picker they came out as a
/// pastel mosaic, which is a nursery wall and not a leaded window.
const GLASS: [f32; 12] = [
    0.012, 0.045, 0.34, //
    0.010, 0.20, 0.19, //
    0.70, 0.30, 0.015, //
    0.28, 0.010, 0.055,
];

/// Lead is dark and slightly lit along one side. Not black: black lead on a
/// dark `fondo` is a window with no leading at all.
const LEAD: [f32; 12] = [
    0.004, 0.004, 0.005, //
    0.014, 0.014, 0.018, //
    0.045, 0.045, 0.055, //
    0.11, 0.11, 0.13,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("glass", &GLASS),
        color_at("lead", &LEAD),
        background(),
    crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
    crate::model::EFFECTS_COLOR_BLOCK,
],
};

/// The furthest a seed can get from the centre of its own cell, in cells, at
/// the worst combination of `irregular` and `deriva`.
pub fn seed_reach(irregular: f32, drift: f32) -> f32 {
    irregular.clamp(0.0, 1.0) * JITTER + drift.clamp(0.0, 1.0) * DRIFT
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    glass_color: ColorSlotGpu,
    lead_color: ColorSlotGpu,
    background: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
}

pub struct Vitral {
    pass: Fullscreen,
}

impl Vitral {
    pub fn new() -> Vitral {
        Vitral { pass: Fullscreen::new("stained-glass", include_str!("../../shaders/vitral.wgsl")) }
    }
}

impl Model for Vitral {
    fn id(&self) -> &'static str {
        "stained-glass"
    }

    fn name(&self) -> &'static str {
        "Stained Glass"
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
            glass_color: ColorSlotGpu::from_floats(&floats[10..22]),
            lead_color: ColorSlotGpu::from_floats(&floats[22..34]),
            background: ColorSlotGpu::from_floats(&floats[34..46]),
            a: [ctx.aspect(), ctx.time, floats[0].round(), floats[1]],
            b: [floats[2], floats[3], floats[4], floats[5]],
            c: [floats[6], floats[7], floats[8], floats[9]],
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
        // 10 floats + three colour slots. `render` slices `[10..22]`,
        // `[22..34]` and `[34..46]` by hand.
        assert_eq!(SCHEMA.width(), 46 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 46 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    #[test]
    fn seed_stays_home() {
        // The one constraint in this model that is not cosmetic. A pixel
        // searches the nine lattice cells around its own; a seed that got
        // further than one cell from its centre would be nearest to pixels
        // that never look at it, and a pane would tear along a straight
        // lattice line. Checked at the worst combination of both floats, not
        // at the defaults.
        let worst = seed_reach(1.0, 1.0);
        assert!(
            worst < 1.0,
            "at full irregular and full deriva a seed reaches {worst} cells — panes will tear"
        );
        // And with real headroom, because `fwidth` antialiasing needs the
        // neighbour to be genuinely inside the searched ring, not exactly on it.
        assert!(worst < 0.9, "no headroom left: {worst}");
    }

    #[test]
    fn the_lead_is_thinner_than_a_pane_at_every_setting_of_cells() {
        // A `plomo` wide enough to swallow a pane gives a frame of solid lead:
        // it renders, it is deterministic, it is not black, and it is not a
        // window. The render gate cannot see the difference.
        let f = row();
        for &cells in &[2.0f32, 11.0, 40.0] {
            // The shader compares `f2 - f1` against `plomo * celdas * 0.5`,
            // and a typical pane's half-width in that unit is about 0.5.
            let lead = f[3] * cells * 0.5;
            assert!(lead < 0.5, "at celdas {cells} the lead ({lead}) fills the pane");
        }
    }

    #[test]
    fn a_regular_window_is_reachable_and_so_is_a_wild_one() {
        // `irregular` 0 must be a plain grid of squares rather than a
        // degenerate diagram — every seed at its cell centre, all distances
        // equal, which is where a naive F1/F2 would divide by zero.
        assert_eq!(seed_reach(0.0, 0.0), 0.0);
        assert!(seed_reach(1.0, 0.0) < 1.0);
    }
}
