//! Model — Beam. A vector-CRT oscilloscope trace, ported from fosfora with a
//! real capability gap and more personalisation than the original had.
//!
//! **Read `beam.wgsl`'s header before wiring this in.** fosfora sampled a
//! captured waveform texture MetalVJ has no equivalent of; this port
//! synthesises a stand-in from three scalars instead. It renders, and it
//! reacts to the same floats a real waveform would, but it is not drawing
//! the actual signal the way fosfora's Beam did. Also: this file draws one
//! frame's fresh energy only, with no phosphor-persistence feedback pass —
//! see IMPORT.md for the two ways to add that back.

use crate::model::{color_at, spec, spec_held, Block, Ctx, FloatSpec, Model, Schema};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    // 0 = scope (horizontal sweep), 1 = radial. Structural: which mode Beam
    // is in is not an intensity, it is a different picture.
    spec_held("mode", 0.0, 1.0, 0.0, 0.1),
    // Reserved for the persistence pass — read by nothing in beam.wgsl today.
    // Kept in the schema now so wiring persistence later does not move every
    // other float's offset.
    spec("decay", 0.80, 0.99, 0.9),
    spec("focus_base", 0.0, 1.0, 0.35),
    spec("intensity", 0.2, 2.0, 1.0),
    // New: fosfora hardcoded the scope's vertical gain (0.40) and the
    // radial excursion (0.28) as two separate constants. One float, because
    // an operator personalising Beam wants "how big is the trace", not two
    // numbers that happen to mean that per mode.
    spec("amplitude", 0.1, 0.6, 0.34),
    // New: radial mode repeats the trace in this many wedges. 1 is fosfora's
    // original single ring. Structural — it changes the shape of the
    // picture, not its intensity.
    spec_held("symmetry", 1.0, 8.0, 1.0, 0.15),
    // New: fosfora's rms/onset gain terms (1.4, 1.5) as dials.
    spec("gain_energy", 0.0, 2.5, 1.4),
    spec("gain_transient", 0.0, 3.0, 1.5),
    // New: fosfora's colour drift was a hardcoded 0.15.
    spec("drift_color", 0.0, 1.0, 0.15),
    // Was u.rms, u.onset, u.zcr, u.beat, u.centroid — fosfora's shared audio
    // globals, now Beam's own driven floats. See aurora.rs for the same move
    // and why: `metal-render` has no equivalent shared uniform to read them off.
    spec("energy", 0.0, 1.0, 0.4),
    spec("transient", 0.0, 1.0, 0.0),
    spec("focus_audio", 0.0, 1.0, 0.2),
    spec("pulse", 0.0, 1.0, 0.0),
    spec("tone", 0.0, 1.0, 0.5),
];

/// New: fosfora's beam colour was `audioPalette`'s fixed warm/cool call.
/// This is the same warm-to-cool walk as a starting point, but it is now a
/// gradient a person can repaint rather than a formula.
const BEAM_COLOR: [f32; 12] = [
    1.00, 0.45, 0.15, //
    1.00, 0.80, 0.30, //
    0.35, 0.65, 1.00, //
    0.15, 0.30, 0.90,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("beam", &BEAM_COLOR),
        crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    beam_color: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
}

pub struct Beam {
    pass: Fullscreen,
}

impl Beam {
    pub fn new() -> Beam {
        Beam { pass: Fullscreen::new("beam", include_str!("../../shaders/beam.wgsl")) }
    }
}

impl Model for Beam {
    fn id(&self) -> &'static str {
        "beam"
    }

    fn name(&self) -> &'static str {
        "Beam"
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
        // FLOATS order: 0 modo, 1 decaimiento, 2 enfoque_base, 3 intensidad,
        // 4 amplitud, 5 simetria, 6 ganancia_energia, 7 ganancia_transitorio,
        // 8 deriva_color, 9 energia, 10 transitorio, 11 enfoque_audio,
        // 12 pulso, 13 tono.
        let uniform = Uniform {
            beam_color: ColorSlotGpu::from_floats(&floats[14..26]),
            a: [ctx.aspect(), ctx.time, floats[0], floats[2]],
            b: [floats[3], floats[4], floats[5], floats[6]],
            c: [floats[7], floats[8], floats[9], floats[10]],
            d: [floats[11], floats[12], floats[13], floats[1]],
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
        // 14 floats + one colour slot. `render` slices `[14..26]`.
        assert_eq!(SCHEMA.width(), 26 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 26 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }
}
