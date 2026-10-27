//! Model 26 — Moiré. Several rulings of lines laid over each other, slightly
//! out of agreement, so that the picture is the *difference* between them.
//!
//! Geoterrain's counterpart, and worth reading next to it: that model makes a
//! figure out of parametric maths and shows you the figure. This one makes two
//! deliberately dull figures and shows you what appears between them. Nothing
//! in this file draws the pattern, and there is nowhere in the shader you could
//! point at and say the pattern is there.
//!
//! That is also why it is a good model to drive hard. A beat frequency
//! amplifies: a signal that nudges `giro` by a degree moves the interference
//! across the whole frame, so this model gets a large visible response out of a
//! small honest change to a float. Most of the suite needs the opposite — a
//! float swung across its whole range to look like it did anything.
//!
//! # Cost
//!
//! Six layers at most, one `fract` and a `smoothstep` each. It is, with
//! Strobe, one of the two cheapest renderers in the suite, and neither of them
//! is cheap by being simple to look at.

use crate::model::{color_at, background, spec, spec_held, Block, Ctx, FloatSpec, Model, Schema};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::ColorSlotGpu;

/// Matches `MAX_CAPAS` in moire.wgsl — two statements of one number.
pub const MAX_LAYERS: f32 = 6.0;

const FLOATS: &[FloatSpec] = &[
    // Structural: adding a ruling changes what the pattern *is*, not how
    // strong it is. Driven freely the frame would re-form several times a
    // second rather than move.
    spec_held("layers", 1.0, MAX_LAYERS, 3.0, 0.15),
    spec("step", 0.004, 0.15, 0.022),
    // Signed, so zero is a pattern held still — which an operator does reach
    // for, to line the rulings up by hand.
    spec("turn", -1.0, 1.0, 0.08),
    // The float the whole model lives on: at 0 every layer is the same ruling
    // exactly on top of itself and there is no interference at all.
    spec("shift", 0.0, 1.0, 0.06),
    spec("thickness", 0.02, 0.98, 0.45),
    spec("curve", 0.0, 1.0, 0.0),
    spec("zoom", 0.15, 3.0, 1.0),
    spec("mix", 0.0, 1.0, 0.35),
    spec("contrast", 0.2, 4.0, 2.2),
    spec("separation", -1.0, 1.0, 0.12),
];

/// Not white: a moiré is read by its *nodes*, and four white stops make the
/// bright ones and the dim ones the same colour, which is the one thing that
/// stops the interference being visible as interference.
///
/// Linear values — see the same note in `models::vitral`. The dark end has to
/// be genuinely near zero or the weave never closes, and the bright end has to
/// clear 1.0's neighbourhood or the nodes never open.
const LINES: [f32; 12] = [
    0.004, 0.006, 0.012, //
    0.03, 0.14, 0.42, //
    0.30, 0.72, 0.85, //
    1.00, 0.98, 0.88,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[Block::Floats(FLOATS), color_at("lines", &LINES), background(), crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK, crate::model::EFFECTS_COLOR_BLOCK],
};

/// The beat frequency two rulings at `pitch`, separated by angle `d`, produce —
/// the spacing of the pattern the frame actually shows.
///
/// Nothing renders with this. It is here because it is the number the model is
/// *about* and it is nowhere in the shader, so it is the only way to assert
/// that the defaults produce a pattern at a size a person can see rather than
/// one finer than a pixel or wider than the frame.
pub fn beat_spacing(pitch: f32, angle: f32) -> f32 {
    let s = (angle * 0.5).sin().abs();
    if s < 1e-5 {
        return f32::INFINITY; // parallel rulings never beat
    }
    pitch / (2.0 * s)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    lines: ColorSlotGpu,
    background: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
}

pub struct Moire {
    pass: Fullscreen,
}

impl Moire {
    pub fn new() -> Moire {
        Moire { pass: Fullscreen::new("moire", include_str!("../../shaders/moire.wgsl")) }
    }
}

impl Model for Moire {
    fn id(&self) -> &'static str {
        "moire"
    }

    fn name(&self) -> &'static str {
        "Moiré"
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
            lines: ColorSlotGpu::from_floats(&floats[10..22]),
            background: ColorSlotGpu::from_floats(&floats[22..34]),
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
        // 10 floats + two colour slots. `render` slices `[10..22]` and `[22..34]`.
        assert_eq!(SCHEMA.width(), 34 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 34 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    #[test]
    fn the_defaults_beat_at_a_size_a_person_can_see() {
        // The failure this model has that nothing else catches, and that the
        // render gate cannot see: a pattern finer than a pixel is grey, and one
        // wider than the frame is a gradient. Both render, neither is a moiré,
        // and the float row looks perfectly reasonable in both cases.
        let f = row();
        let (pitch, offset) = (f[1], f[3]);
        // What the shader spreads the layers across: `desfase * TAU/4`.
        let angle = offset * std::f32::consts::TAU * 0.25;
        let spacing = beat_spacing(pitch, angle);
        assert!(
            (0.02..2.0).contains(&spacing),
            "the default pattern beats every {spacing} of a frame — invisible or not a pattern"
        );
        assert!(spacing > pitch * 3.0, "the beat is no coarser than the ruling that made it");
    }

    #[test]
    fn rulings_in_perfect_agreement_do_not_beat_at_all() {
        // `desfase` 0 is inside the declared range and is what an operator
        // reaches for to line the layers up by hand. It must be a still frame
        // of plain stripes, not a divide by zero.
        assert!(beat_spacing(0.022, 0.0).is_infinite());
    }

    #[test]
    fn a_smaller_angle_makes_a_broader_pattern() {
        // The property the whole model trades on, and the reason a small
        // honest change to a float here is a large change on the wall.
        let wide = beat_spacing(0.02, 0.02);
        let tight = beat_spacing(0.02, 0.4);
        assert!(wide > tight * 5.0, "closing the angle did not open the pattern");
    }
}
