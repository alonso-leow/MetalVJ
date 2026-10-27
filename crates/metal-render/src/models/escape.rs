//! Model 6 — Escape. Strings crossing the frame and swaying, drawn through
//! the same `line2d` pipeline Geoterrain uses. Shape is a closed-form
//! function of `(index, arc position, time)` rather than a discrete
//! integrator — a VJ instrument needs "what does this preset look like at
//! this instant," not energy-conserving physics, and a pure function of
//! time is what makes a saved preset reproduce exactly and the render gate
//! deterministic without tracking simulation state across frames.
//!
//! # Three things this model learned the hard way
//!
//! **A string has no visible end.** It used to hang from `y = 0.85` and stop
//! about a unit down, which put both of its ends inside the frame — so the
//! model read as a handful of short ribbons floating in a rectangle rather
//! than as strings passing through it. Every span is now built from the
//! frame's own support function ([`Frame`]) and overshoots it, in *both*
//! directions, at every rotation. Nothing here may shorten a span: whatever
//! `tension` and `sube_baja` do, they do it to the pattern travelling along
//! the string, never to its length.
//!
//! **The strings turn, and so does everything measured against them.** `giro`
//! rotates the whole family. The width of a stroke is therefore taken in NDC
//! and converted back, not applied along `x` — a fixed `x` normal is the
//! right answer only while the strings are vertical, and looks like a
//! thickness slider that also changes with rotation once they are not.
//!
//! **Plucking is stateless.** `pulsacion` fires events on a clock derived
//! from `t` alone, and a string's ring is found by tracing *backwards* over
//! recent events to the last one that named it — see [`ring`]. No frame-to-
//! frame state, so the determinism gate still holds.

use glam::Vec2;

use crate::model::{color, background, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::backdrop::Backdrop;
use crate::shared::line2d::{Line2dPipeline, Strip, Vertex};
use crate::shared::ColorSlotGpu;

/// The most strings the model will draw, and so also the largest batch a
/// single pluck can name.
const MAX_STRINGS: u32 = 24;

const FLOATS: &[FloatSpec] = &[
    spec("n", 1.0, MAX_STRINGS as f32, 7.0),
    // 1.0 spaces the family evenly across the frame whatever the rotation;
    // above that it spills out of shot, which is a look and not a mistake.
    spec("spread", 0.1, 1.5, 1.0),
    // Degrees. 0 is the vertical hang the model always had, 90 lays them
    // flat, 45 puts them on the diagonal. Structural: swinging the whole
    // family's angle from the audio is a spinning frame, not a livelier one.
    spec_held("turn", 0.0, 180.0, 0.0, 0.08),
    spec("tension", 0.0, 1.0, 0.4),
    spec("variation", 0.0, 1.0, 0.3),
    spec("thickness", 0.002, 0.03, 0.008),
    spec("escape", 0.0, 1.0, 0.25),
    spec("rise_low", 0.0, 1.0, 0.3),
    // The guitar half. 0 leaves every string on the shared sway — the global
    // behaviour this model has always had — and anything above it starts
    // picking strings out of the family instead.
    spec("pulsation", 0.0, 8.0, 0.0),
    // How many adjacent strings one pluck takes: 1 is a picked note, `n` is
    // a strum. Structural for the same reason `giro` is — it selects which
    // instrument this is, and sweeping it from the audio just blurs the two.
    spec_held("group", 1.0, MAX_STRINGS as f32, 1.0, 0.3),
    spec("sustained", 0.0, 1.0, 0.45),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[Block::Floats(FLOATS), color("escape"), background(), crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK, crate::model::EFFECTS_COLOR_BLOCK],
};

pub struct Escape {
    pipeline: Built<Line2dPipeline>,
    backdrop: Built<Backdrop>,
}

impl Escape {
    pub fn new() -> Escape {
        Escape { pipeline: Built::default(), backdrop: Built::default() }
    }
}

/// The frame as the strings see it: which way they run, which way they are
/// stacked, and how far each of those two directions has to reach to leave
/// nothing showing.
///
/// The reach is the rectangle's support function, not its half-diagonal. The
/// drawable area in this pipeline's space is `x` in `±aspect` by `y` in `±1`
/// (`line2d` divides `x` by the aspect on the way to NDC), so the distance
/// from the centre to the frame's boundary *in a given direction* is
/// `aspect·|d.x| + |d.y|`. Using the diagonal instead would be correct but
/// slack: at `giro = 0` on a 16:9 frame it makes every span twice as long as
/// it needs to be, and `spread = 1` scatter the strings well outside the shot.
struct Frame {
    /// Unit vector along a string. `giro = 0` is straight down, which is the
    /// direction the model hung in before it could rotate.
    dir: Vec2,
    /// Unit vector across the family, at right angles to `dir`.
    perp: Vec2,
    /// Centre to boundary along `dir`.
    reach_along: f32,
    /// Centre to boundary along `perp`.
    reach_across: f32,
    aspect: f32,
}

impl Frame {
    fn new(rotation_radians: f32, aspect: f32) -> Frame {
        let (sin, cos) = rotation_radians.sin_cos();
        let dir = Vec2::new(sin, -cos);
        let perp = Vec2::new(cos, sin);
        Frame {
            dir,
            perp,
            reach_along: aspect * dir.x.abs() + dir.y.abs(),
            reach_across: aspect * perp.x.abs() + perp.y.abs(),
            aspect,
        }
    }

    /// The offset that thickens a stroke by `half_width` **as seen**.
    ///
    /// A stroke's width is a screen quantity: half a pixel of a 1080-wide
    /// frame is half a pixel whichever way the stroke points. But this
    /// model's positions live in a space the vertex shader squeezes
    /// horizontally by the aspect, so an offset applied there is worth less
    /// on screen the more of it lies along `x`. This takes the normal in NDC,
    /// where the width is honest, and converts it back.
    fn stroke_offset(&self, half_width: f32) -> Vec2 {
        let ndc_dir = Vec2::new(self.dir.x / self.aspect, self.dir.y).normalize_or_zero();
        let ndc_normal = Vec2::new(-ndc_dir.y, ndc_dir.x);
        Vec2::new(ndc_normal.x * self.aspect, ndc_normal.y) * half_width
    }
}

/// `hash11` from `shared.wgsl`, on the CPU.
///
/// The same arithmetic on purpose: a pluck order picked here and a pluck
/// order picked in a shader that disagreed would be two different instruments
/// wearing one name.
fn hash11(x: f32) -> f32 {
    let v = (x * 127.1).sin() * 43758.5453123;
    v - v.floor()
}

/// How many pluck events back to look for one that named a given string.
///
/// At `n = 24` and `grupo = 1` a string is named about once every 24 events,
/// so a window shorter than that would silence the outliers entirely and a
/// much longer one only finds plucks that have already rung out. Two full
/// rounds is the honest compromise, and at 24 strings it costs about a
/// thousand iterations a frame.
const LOOKBACK: u32 = 48;

/// How hard string `i` is still ringing, from the pluck clock alone.
///
/// Walks *backwards* from the current event, so the first event that names
/// the string is by construction the most recent one — no state to carry
/// between frames, which is what keeps `render` a pure function of
/// `(floats, time)` and the determinism gate honest.
///
/// Each event names `group` **adjacent** strings starting from a hashed
/// index. Adjacent rather than scattered because that is what a hand does:
/// one finger is a note, four fingers across the neck is a chord, and a
/// random subset of six strings is neither.
fn ring(i: u32, n: u32, t: f32, rate: f32, group: u32, tau: f32) -> f32 {
    if rate <= 0.0 || n == 0 {
        return 0.0;
    }
    let now = t * rate;
    let current = now.floor();
    for back in 0..LOOKBACK {
        let k = current - back as f32;
        if k < 0.0 {
            break;
        }
        let first = (hash11(k) * n as f32).floor().max(0.0) as u32 % n;
        let named = (0..group).any(|g| (first + g) % n == i);
        if named {
            let age = (now - k) / rate;
            return (-age / tau.max(0.01)).exp();
        }
    }
    0.0
}

impl Model for Escape {
    fn id(&self) -> &'static str {
        "escape"
    }

    fn name(&self) -> &'static str {
        "Escape"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn backdrop(&self) -> bool {
        true
    }

    fn render(
        &mut self,
        ctx: &Ctx,
        floats: &[f32],
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        let n = (floats[0].round().max(1.0) as u32).min(MAX_STRINGS);
        let spread = floats[1];
        let rotation = floats[2].to_radians();
        let tension = floats[3];
        let variation = floats[4];
        let thickness = floats[5].max(0.0005);
        let escape = floats[6];
        let seesaw = floats[7];
        let pluck = floats[8].max(0.0);
        let group = (floats[9].round().max(1.0) as u32).min(n);
        let sustain = floats[10];
        let color = ColorSlotGpu::from_floats(&floats[11..23]);
        let background = ColorSlotGpu::from_floats(&floats[23..35]);

        let backdrop = self.backdrop.get(format, || Backdrop::new(ctx.device, format));
        // The background walks with the strings rather than staying pinned to
        // the frame: a diagonal family over a top-to-bottom gradient reads as
        // two unrelated images laid on top of each other.
        backdrop.draw(ctx, target, background, rotation, ctx.fondo_alpha);

        let pipeline = self
            .pipeline
            .get(format, || Line2dPipeline::new(ctx.device, format));

        let frame = Frame::new(rotation, ctx.aspect());
        let t = ctx.time;

        // How far the sway can throw a point sideways. The span is padded by
        // it because at `giro = 45°` a sideways throw at the very end of a
        // string moves that end along the frame's boundary as well as across
        // it, and a span cut to the boundary exactly would show a tip.
        let ambient = 0.02 + tension * 0.10;
        let pluck_swing = 0.03 + escape * 0.35;
        let half_len = frame.reach_along + (ambient + pluck_swing) * 1.5 + 0.05;

        // Resolution along a span. It scales with the span so a diagonal — up
        // to twice the length of a vertical one on a wide frame — is not
        // sampled half as finely and does not go visibly polygonal.
        let density = ((48.0 * half_len).round() as u32).clamp(24, 256);

        let tau = 0.08 + sustain * 1.4;
        let mut vertices = Vec::with_capacity((n * density * 2) as usize);
        let mut strips = Vec::with_capacity(n as usize);

        for i in 0..n {
            let fi = i as f32;

            // Cell centres, so the family is evenly spaced with a half gap at
            // each edge rather than with two strings sitting on the boundary.
            let across = if n == 1 { 0.0 } else { (fi + 0.5) / n as f32 * 2.0 - 1.0 };
            let base = frame.perp * across * frame.reach_across * spread;

            // What the string is doing right now. Two sources, one name, so
            // nothing downstream has to branch: the free-running burst the
            // model always had while nothing is picking at it, and the pluck
            // envelope once something is.
            let excite = if pluck > 0.0 {
                ring(i, n, t, pluck, group, tau)
            } else {
                let cycle = (t * 0.35 + fi * 0.9).fract();
                let burst = (1.0 - (cycle * 6.0).clamp(0.0, 1.0)).max(0.0);
                burst * burst
            };

            // `variacion` is what separates the two halves of this model: at
            // 0 every string carries the same phase and the family sways as
            // one body, which is the global control; turned up, each string
            // gets its own and they come apart.
            let own_phase = fi * 2.1 * variation;
            let waves = 1.0 + variation * 4.0;
            let ring_freq = 6.0 + tension * 14.0;

            let start = vertices.len() as u32;
            for s in 0..density {
                let frac = s as f32 / (density - 1) as f32;
                // −1..1 along the string: 0 is the middle of the frame and
                // both ends are past its boundary.
                let along = (frac * 2.0 - 1.0) * half_len;

                // A pluck's shape: widest in the middle of the frame, nothing
                // at the ends. It is what a plucked span looks like, and it
                // also holds the ends still, which is the other half of never
                // letting one show.
                let bow = (along / half_len * std::f32::consts::FRAC_PI_2).cos();

                let sway = (along / frame.reach_along * std::f32::consts::PI * waves
                    + t * (0.8 + tension * 2.4)
                    + t * seesaw * 2.0
                    + own_phase)
                    .sin();
                let rung = (along / frame.reach_along * std::f32::consts::PI
                    + t * ring_freq
                    + fi)
                    .sin();

                let lateral = sway * ambient + bow * rung * excite * pluck_swing;
                let center = base + frame.dir * along + frame.perp * lateral;

                // Plucked strings swell. With a white slot it is the only cue
                // that says which string was picked; with a gradient the
                // colour walk below says it too.
                let hw = thickness * 0.5 * (1.0 + excite * 0.6);
                let offset = frame.stroke_offset(hw);
                let ct = colour_at(frac, t, fi, seesaw, variation, excite);
                vertices.push(Vertex::new(center + offset, ct));
                vertices.push(Vertex::new(center - offset, ct));
            }
            strips.push(Strip { start, len: vertices.len() as u32 - start });
        }

        pipeline.draw(ctx, target, &vertices, &strips, color);
    }
}

/// Where along the colour slot a point of a string is drawn.
///
/// `sube_baja` used to move the anchor up and down, which was only visible
/// because the anchor was in frame. With no visible end, sliding the *pattern*
/// is the only way motion along a string can be seen at all, so that is what
/// it now does.
///
/// Ping-ponged rather than wrapped: a wrapped walk jumps from the slot's last
/// stop to its first once per cycle, and on a four-stop slot that is a hard
/// band running along every string.
fn colour_at(frac: f32, t: f32, fi: f32, seesaw: f32, variation: f32, excite: f32) -> f32 {
    let slide = t * seesaw * 0.4 + fi * 0.13 * variation;
    let w = (frac * 0.5 + slide).fract();
    let walk = if w > 0.5 { 2.0 - 2.0 * w } else { 2.0 * w };
    // A pluck pushes the string toward the far end of its slot, so activation
    // reads as a colour and not only as a thickness.
    (walk + excite * 0.3).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The framing promise, checked as arithmetic rather than by eye: at every
    /// rotation and on frames of both orientations, a span must start outside
    /// the frame and end outside it. This is the property the old model broke,
    /// and it broke it silently — the render gate is perfectly happy with
    /// short ribbons in a black rectangle.
    #[test]
    fn a_string_leaves_the_frame_at_every_rotation() {
        for aspect in [16.0 / 9.0, 1.0, 9.0 / 16.0] {
            for degrees in (0..=180).step_by(5) {
                let frame = Frame::new((degrees as f32).to_radians(), aspect);
                let half_len = frame.reach_along + 0.05;
                let tip = frame.dir * half_len;
                // Outside means past the boundary in this pipeline's space:
                // `x` in ±aspect, `y` in ±1.
                assert!(
                    tip.x.abs() > aspect - 1e-4 || tip.y.abs() > 1.0 - 1e-4,
                    "{degrees}° on a {aspect} frame stops inside it at {tip:?}",
                );
            }
        }
    }

    /// The other half of the same promise: the strings must be stacked across
    /// enough of the frame to cover it, or `spread = 1` leaves a bare margin
    /// down one side at some angles and not at others.
    #[test]
    fn the_family_spans_the_frame_across_at_every_rotation() {
        for aspect in [16.0 / 9.0, 1.0] {
            for degrees in (0..=180).step_by(5) {
                let frame = Frame::new((degrees as f32).to_radians(), aspect);
                let edge = frame.perp * frame.reach_across;
                assert!(
                    edge.x.abs() > aspect - 1e-4 || edge.y.abs() > 1.0 - 1e-4,
                    "{degrees}° on a {aspect} frame stacks short at {edge:?}",
                );
            }
        }
    }

    /// A stroke's width is a screen quantity. Rotating the family must not
    /// change how thick it looks, which a fixed `x` normal quietly does.
    #[test]
    fn a_stroke_keeps_its_width_as_the_family_turns() {
        let aspect = 16.0 / 9.0;
        for degrees in (0..=180).step_by(5) {
            let frame = Frame::new((degrees as f32).to_radians(), aspect);
            let offset = frame.stroke_offset(0.01);
            // Back through what the vertex shader does to get to NDC.
            let on_screen = Vec2::new(offset.x / aspect, offset.y).length();
            assert!(
                (on_screen - 0.01).abs() < 1e-5,
                "{degrees}° draws a {on_screen} half-width where 0.01 was asked for",
            );
        }
    }

    /// Individual picking and strumming have to be genuinely different, and
    /// the ring has to decay. Without this, `grupo` is a slider that changes a
    /// number nobody can see.
    #[test]
    fn plucking_names_a_batch_and_then_lets_it_ring_out() {
        let n = 8;
        let rate = 4.0;

        // At `grupo = 1` the family is not all doing the same thing.
        let picked: Vec<f32> = (0..n).map(|i| ring(i, n, 3.0, rate, 1, 0.3)).collect();
        let loudest = picked.iter().cloned().fold(0.0f32, f32::max);
        let quietest = picked.iter().cloned().fold(1.0f32, f32::min);
        assert!(loudest > 0.2, "nothing was ringing at all: {picked:?}");
        assert!(quietest < loudest * 0.5, "one pluck lit the whole family: {picked:?}");

        // At `grupo = n` every string is named by every event, so they are.
        let strummed: Vec<f32> = (0..n).map(|i| ring(i, n, 3.0, rate, n, 0.3)).collect();
        for (i, v) in strummed.iter().enumerate() {
            assert!((v - strummed[0]).abs() < 1e-6, "string {i} missed the strum");
        }

        // And a ring decays rather than sitting on.
        let now = ring(0, n, 3.0, rate, n, 0.3);
        let later = ring(0, n, 3.0 + 0.24, rate, n, 0.3);
        assert!(later < now, "the ring did not decay: {now} then {later}");

        // Off is off, and off is what leaves the model on its shared sway.
        assert_eq!(ring(0, n, 3.0, 0.0, 1, 0.3), 0.0);
    }
}
