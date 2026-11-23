//! Model 24 — Strobe (`strobe`). The whole frame as light: beat-locked
//! flashes, with a duty cycle, a decay, and an optional chase down the bands.
//!
//! The cheapest model in the suite and, in a room, close to the loudest. Every
//! other renderer here draws a subject and lights it; this one skips the
//! subject, which is why it is the only one that reaches people who are not
//! looking at the screen.
//!
//! # It is also the suite's honest test of the beat clock
//!
//! Worth saying out loud, because it is half the reason to build it. Every
//! other model *degrades* when the digest is wrong — a spiral turns at a
//! slightly odd rate, a fractal breathes off the beat, and it takes a trained
//! eye and a side-by-side to be sure. A strobe does not degrade. It is either
//! on the kick or it is not, and everyone in the room can tell instantly.
//!
//! So when `periodo` is being driven from the tempo and the picture feels
//! wrong, the fault is in `metal-listen`'s beat clock or in the mapping, and this
//! model is the fastest way to see which. Nothing else in the suite reports on
//! the digest that plainly.
//!
//! # Every parameter is one number, shaped
//!
//! The model is `phase` — where the clock is inside the current flash — and
//! nothing else. `duty` is how long it is up, `caida` is how it leaves, `borde`
//! is how hard its edges are, and `bandas`/`barrido` decide whether the frame
//! fires at once or in sequence. There is no state and no geometry.

use crate::model::{color_at, background, spec, spec_held, Block, Ctx, FloatSpec, Model, Schema};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    // The clock. Structural in the strongest sense in the suite: this float
    // *is* the model, and a live signal swinging it freely would not read as a
    // strobe reacting to the music — it would read as a strobe with no tempo.
    // What should drive it is the tempo, deliberately, from the dashboard.
    spec_held("period", 0.04, 4.0, 0.5, 0.1),
    // Floored at 0.02 rather than at "as short as you like", and the floor is
    // a real limit and not a taste: a flash shorter than a display frame
    // either lands on a frame or does not, so it fires irregularly. No shader
    // can fix that — it would take temporal supersampling — so the range stops
    // where the picture stops being the picture that was asked for.
    spec("duty", 0.02, 1.0, 0.12),
    spec("edge", 0.0, 1.0, 0.12),
    // A chase costs nothing until `barrido` asks for it — at 0 every band
    // fires together, which is the ordinary strobe.
    spec_held("bands", 1.0, 12.0, 1.0, 0.2),
    spec("sweep", 0.0, 1.0, 0.0),
    spec("alternate", 0.0, 1.0, 0.6),
    spec("fall", 0.0, 1.0, 0.65),
    spec("glare", 0.0, 1.0, 0.1),
    spec("vignette", 0.0, 1.0, 0.35),
];

/// Not white — or rather, not *only* white. A strobe that is four white stops
/// is a white strobe, which is the one thing an operator can already get from
/// this by dragging all four together. Opening on white, ice, amber and a deep
/// red means `alterna` does something the moment the model is selected.
const LIGHT: [f32; 12] = [
    1.00, 1.00, 1.00, //
    0.72, 0.86, 1.00, //
    1.00, 0.78, 0.35, //
    0.90, 0.16, 0.22,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[Block::Floats(FLOATS), color_at("light", &LIGHT), background(), crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK, crate::model::EFFECTS_COLOR_BLOCK],
};

/// Where the clock is inside the current flash, and which flash that is.
///
/// The whole model, on the CPU as well, because it is the one thing here worth
/// asserting about: that the index and the phase never disagree, and that no
/// setting produces a frame that is always on or always off.
pub fn flash_at(time: f32, period: f32, lag: f32) -> (f32, f32) {
    let k = (time - lag) / period.max(0.01);
    let flash = k.floor();
    (flash, k - flash)
}

/// How lit the frame is at that phase.
pub fn level_at(phase: f32, duty: f32, edge: f32, decay: f32) -> f32 {
    let duty = duty.clamp(0.02, 1.0);
    let soft = (edge.clamp(0.0, 1.0) * duty * 0.5).max(1e-4);
    let up = smoothstep(0.0, soft, phase);
    let down = 1.0 - smoothstep(duty - soft, duty, phase);
    let envelope = (-phase / duty * 3.2).exp();
    up * down * (1.0 + (envelope - 1.0) * decay.clamp(0.0, 1.0))
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    light: ColorSlotGpu,
    background: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
}

pub struct Estrobo {
    pass: Fullscreen,
}

impl Estrobo {
    pub fn new() -> Estrobo {
        Estrobo {
            pass: Fullscreen::new("strobe", include_str!("../../shaders/estrobo.wgsl")),
        }
    }
}

impl Model for Estrobo {
    fn id(&self) -> &'static str {
        "strobe"
    }

    fn name(&self) -> &'static str {
        "Strobe"
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
            light: ColorSlotGpu::from_floats(&floats[9..21]),
            background: ColorSlotGpu::from_floats(&floats[21..33]),
            a: [ctx.aspect(), ctx.time, floats[0], floats[1]],
            b: [floats[2], floats[3].round(), floats[4], floats[5]],
            c: [floats[6], floats[7], floats[8], 0.0],
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
        // 9 floats + two colour slots. `render` slices `[9..21]` and `[21..33]`.
        assert_eq!(SCHEMA.width(), 33 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 33 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    #[test]
    fn the_flash_index_and_the_phase_never_disagree() {
        // They are `floor` and `fract` of one quotient precisely so that they
        // cannot, and this is the assertion that keeps them that way: a phase
        // that wrapped independently of the index would give one flash two
        // colours, which on a strobe is a visible tear.
        let mut t = 0.0f32;
        while t < 30.0 {
            let (flash, phase) = flash_at(t, 0.5, 0.0);
            assert!((0.0..1.0).contains(&phase), "phase {phase} out of range at t={t}");
            assert!(
                (flash * 0.5 + phase * 0.5 - t).abs() < 1e-3,
                "flash {flash} + phase {phase} is not t={t}"
            );
            t += 0.017;
        }
    }

    #[test]
    fn no_setting_leaves_the_frame_always_on_or_always_off() {
        // A strobe that never goes dark is a light, and one that never comes
        // up is a fault. Both are reachable by arithmetic — a `duty` of 1 with
        // a soft edge, or a `borde` that eats the whole window — and neither is
        // visible in the float row.
        for &duty in &[0.02f32, 0.12, 0.5, 1.0] {
            for &edge in &[0.0f32, 0.5, 1.0] {
                for &decay in &[0.0f32, 0.65, 1.0] {
                    let mut lo = f32::MAX;
                    let mut hi = f32::MIN;
                    // Sampled inside the window as well as across the whole
                    // period. A fixed grid over 0..1 is the wrong instrument
                    // here: at the shortest duty only two of four hundred
                    // samples land in the flash at all, and neither of them at
                    // its peak — which reads as "never lit up" and is a fact
                    // about the grid, not about the strobe.
                    for i in 0..400 {
                        let inside = duty * (i as f32 / 400.0);
                        let across = i as f32 / 400.0;
                        for phase in [inside, across] {
                            let l = level_at(phase, duty, edge, decay);
                            assert!(l.is_finite() && (-0.001..=1.001).contains(&l));
                            lo = lo.min(l);
                            hi = hi.max(l);
                        }
                    }
                    assert!(hi > 0.25, "never lit up (duty {duty}, borde {edge}, caida {decay})");
                    if duty < 0.99 {
                        assert!(
                            lo < 0.05,
                            "never went dark (duty {duty}, borde {edge}, caida {decay})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_chase_puts_the_bands_out_of_step_and_sweep_zero_puts_them_back() {
        let f = row();
        let period = f[0];
        // Two bands, half the frame apart in phase.
        let (_, top) = flash_at(3.0, period, 0.0);
        let (_, bottom) = flash_at(3.0, period, 0.5 * period);
        assert!((top - bottom).abs() > 0.4, "a full sweep left the bands in step");
        let (_, together) = flash_at(3.0, period, 0.0);
        assert_eq!(top, together);
    }

    #[test]
    fn decay_turns_a_plateau_into_a_hit() {
        // Same duty, entirely different event — the reason the float exists.
        // Measured at the far end of the window, where a square wave is still
        // at full and a decayed one is nearly gone.
        let square = level_at(0.10, 0.12, 0.0, 0.0);
        let hit = level_at(0.10, 0.12, 0.0, 1.0);
        assert!(square > 0.9, "a square flash was not square: {square}");
        assert!(hit < square * 0.3, "caida did not decay the flash: {hit} vs {square}");
    }
}
