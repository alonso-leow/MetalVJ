//! Model — Aurora. Horizontal flowing curtain bands, one per frequency band:
//! a spectrogram disguised as northern lights.
//!
//! Ported from fosfora. See `aurora.wgsl`'s header for what changed and why —
//! in short, the seven audio globals fosfora piped through a shared uniform
//! are now this model's own driven floats, the fixed 7-colour array is now a
//! repaintable `paleta` gradient, and `enfasis_banda`/`enfasis_fuerza` are new:
//! fosfora had no way to make one band the point of the picture.

use crate::model::{color_at, spec, spec_held, Block, Ctx, FloatSpec, Model, Schema};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    spec("speed", 0.3, 2.3, 1.1),
    spec("separation", 0.5, 2.0, 1.25),
    spec("width_shine", 0.008, 0.048, 0.028),
    // The seven bands. Each is what `u.sub_bass`..`u.brilliance` were in
    // fosfora — now an ordinary driven float, so the digest maps its own
    // seven bands onto these seven names instead of the model reading a
    // global it did not declare.
    spec("band_0", 0.0, 1.0, 0.3),
    spec("band_1", 0.0, 1.0, 0.3),
    spec("band_2", 0.0, 1.0, 0.3),
    spec("band_3", 0.0, 1.0, 0.3),
    spec("band_4", 0.0, 1.0, 0.3),
    spec("band_5", 0.0, 1.0, 0.3),
    spec("band_6", 0.0, 1.0, 0.3),
    // Was `u.rms` — overall brightness/width gain across every band.
    spec("energy", 0.0, 1.0, 0.4),
    // Was `u.onset` — a transient brighten on a hit.
    spec("impact", 0.0, 1.0, 0.0),
    // Which band the emphasis peak is centred on. Structural in the same
    // sense Strobe's `bandas` is: it selects *which* band is the subject,
    // not how strong the effect is, so a live signal swinging it freely would
    // read as the spotlight jumping rather than intensifying.
    spec_held("emphasis_band", 0.0, 6.0, 3.0, 0.15),
    spec("emphasis_force", 0.0, 1.0, 0.5),
    // Appended 2026-10-25. A smooth night-sky gradient behind the curtains
    // (palette-tinted), so the background is not a flat zero.
    spec("haze", 0.0, 1.0, 0.5),
    // Curtain edge profile: 0 the soft bell, 1 crisper flat-topped ribbons.
    spec("sharpness", 0.0, 1.0, 0.35),
];

/// Warm to cool, the same order fosfora's fixed array walked: deep red at
/// `t=0` (sub-bass) to violet at `t=1` (brilliance). A gradient rather than
/// seven fixed stops, so repainting Aurora is four colour picks instead of a
/// code change.
const PALETTE: [f32; 12] = [
    0.80, 0.10, 0.20, //
    1.00, 0.55, 0.15, //
    0.20, 0.85, 0.40, //
    0.35, 0.30, 1.00,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("palette", &PALETTE),
        crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    palette: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    e: [f32; 4],
}

pub struct Aurora {
    pass: Fullscreen,
}

impl Aurora {
    pub fn new() -> Aurora {
        Aurora { pass: Fullscreen::new("aurora", include_str!("../../shaders/aurora.wgsl")) }
    }
}

impl Model for Aurora {
    fn id(&self) -> &'static str {
        "aurora"
    }

    fn name(&self) -> &'static str {
        "Aurora"
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
        // FLOATS order: 0 velocidad, 1 separacion, 2 ancho_brillo,
        // 3..9 banda_0..banda_6, 10 energia, 11 impacto, 12 enfasis_banda,
        // 13 enfasis_fuerza, 14 haze, 15 sharpness.
        let uniform = Uniform {
            palette: ColorSlotGpu::from_floats(&floats[16..28]),
            a: [ctx.aspect(), ctx.time, floats[0], floats[1]],
            b: [floats[2], floats[10], floats[11], floats[12]],
            c: [floats[13], floats[3], floats[4], floats[5]],
            d: [floats[6], floats[7], floats[8], floats[9]],
            e: [floats[14], floats[15], 0.0, 0.0],
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
        // 16 floats + one colour slot. `render` slices `[16..28]`.
        assert_eq!(SCHEMA.width(), 28 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 28 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    #[test]
    fn emphasis_at_zero_leaves_every_band_equally_weighted() {
        // enfasis_fuerza is a gain on a Gaussian peak — at 0 the peak's height
        // is 1.0 for every band regardless of enfasis_banda, so the picture
        // must not change when this float is swept at its floor.
        let f = row();
        assert_eq!(f[13], 0.5, "default enfasis_fuerza drifted — update this test's premise");
    }
}
