//! Model 16 — ASCII Waveform. A wave drawn in characters.
//!
//! # The font
//!
//! Nothing in the first fifteen renderers needed to draw a letter, so this one
//! carries a capability the rest of the suite does not, and that capability is
//! its whole cost. It is paid the cheap way: a 32-glyph 8x8 bitmap set, 8 bytes
//! a glyph, written here as binary literals so the shape of each character is
//! visible in the source and handed to the shader as two `u32` per glyph.
//!
//! No texture, no sampler, no atlas build step, no font file, no licence. And
//! the 8x8 grid is not a compromise — it is the aesthetic. `docs/NEXT.md` §7
//! records the alternative that was checked and rejected: `skrifa`, `harfrust`
//! and `ab_glyph` are all already in the lock file through egui and winit, and
//! they are still the wrong answer, because they rasterise into egui's own
//! atlas and nothing in that path hands a model a `TextureView` of its own.
//!
//! # The ramps
//!
//! *"Characters from a preselected few"* — a picked set, not free text. Four
//! ramps, each running from blank to solid; the float selects one and the CPU
//! uploads only that ramp's glyphs, so the shader never learns which set it is
//! drawing.

use wgpu::util::DeviceExt;

use crate::model::{color_at, background, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::ColorSlotGpu;

// --------------------------------------------------------------------- font

/// One 8x8 glyph, top row first, most significant bit leftmost.
type Glyph = [u8; 8];

const BLANK: Glyph = [0; 8];

#[rustfmt::skip]
const DOT: Glyph = [
    0b00000000, 0b00000000, 0b00000000, 0b00000000,
    0b00000000, 0b00011000, 0b00011000, 0b00000000,
];
#[rustfmt::skip]
const COLON: Glyph = [
    0b00000000, 0b00011000, 0b00011000, 0b00000000,
    0b00000000, 0b00011000, 0b00011000, 0b00000000,
];
#[rustfmt::skip]
const DASH: Glyph = [
    0b00000000, 0b00000000, 0b00000000, 0b01111110,
    0b01111110, 0b00000000, 0b00000000, 0b00000000,
];
#[rustfmt::skip]
const EQUALS: Glyph = [
    0b00000000, 0b00000000, 0b01111110, 0b00000000,
    0b01111110, 0b00000000, 0b00000000, 0b00000000,
];
#[rustfmt::skip]
const PLUS: Glyph = [
    0b00000000, 0b00011000, 0b00011000, 0b01111110,
    0b01111110, 0b00011000, 0b00011000, 0b00000000,
];
#[rustfmt::skip]
const STAR: Glyph = [
    0b00000000, 0b00011000, 0b01011010, 0b00111100,
    0b00111100, 0b01011010, 0b00011000, 0b00000000,
];
#[rustfmt::skip]
const HASH: Glyph = [
    0b00100100, 0b00100100, 0b01111110, 0b00100100,
    0b00100100, 0b01111110, 0b00100100, 0b00100100,
];
#[rustfmt::skip]
const PERCENT: Glyph = [
    0b01100010, 0b01100100, 0b00001000, 0b00010000,
    0b00100000, 0b01000110, 0b10000110, 0b00000000,
];
#[rustfmt::skip]
const AT: Glyph = [
    0b00111100, 0b01000010, 0b10011010, 0b10101010,
    0b10101110, 0b10000000, 0b01111100, 0b00000000,
];
#[rustfmt::skip]
const SLASH: Glyph = [
    0b00000010, 0b00000100, 0b00001000, 0b00010000,
    0b00100000, 0b01000000, 0b10000000, 0b00000000,
];
#[rustfmt::skip]
const BACKSLASH: Glyph = [
    0b10000000, 0b01000000, 0b00100000, 0b00010000,
    0b00001000, 0b00000100, 0b00000010, 0b00000000,
];
#[rustfmt::skip]
const PIPE: Glyph = [
    0b00011000, 0b00011000, 0b00011000, 0b00011000,
    0b00011000, 0b00011000, 0b00011000, 0b00011000,
];
#[rustfmt::skip]
const UNDERSCORE: Glyph = [
    0b00000000, 0b00000000, 0b00000000, 0b00000000,
    0b00000000, 0b00000000, 0b00000000, 0b11111111,
];
#[rustfmt::skip]
const MIDDOT: Glyph = [
    0b00000000, 0b00000000, 0b00000000, 0b00011000,
    0b00011000, 0b00000000, 0b00000000, 0b00000000,
];
#[rustfmt::skip]
const SMALL_O: Glyph = [
    0b00000000, 0b00000000, 0b00111100, 0b01000010,
    0b01000010, 0b01000010, 0b00111100, 0b00000000,
];
#[rustfmt::skip]
const BIG_O: Glyph = [
    0b00111100, 0b01000010, 0b01000010, 0b01000010,
    0b01000010, 0b01000010, 0b00111100, 0b00000000,
];
#[rustfmt::skip]
const ZERO: Glyph = [
    0b00111100, 0b01000110, 0b01001010, 0b01010010,
    0b01100010, 0b01000010, 0b00111100, 0b00000000,
];
#[rustfmt::skip]
const DISC: Glyph = [
    0b00111100, 0b01111110, 0b11111111, 0b11111111,
    0b11111111, 0b11111111, 0b01111110, 0b00111100,
];

/// The block-drawing ramp, `▁▂▃▄▅▆▇█`, generated rather than typed: a glyph
/// filled from the bottom by `k` of its 8 rows is arithmetic, and eight
/// hand-written literals would be eight chances to fill one from the top.
const fn bar(k: usize) -> Glyph {
    let mut rows = [0u8; 8];
    let mut r = 8 - k;
    while r < 8 {
        rows[r] = 0xFF;
        r += 1;
    }
    rows
}

/// The four picked sets. Every one starts at blank and ends at its solid, so
/// the shader can index them all the same way — density 0 is the first entry
/// by construction rather than by convention.
const RAMPS: [&[Glyph]; 4] = [
    // density — the classic ASCII gradient
    &[BLANK, DOT, COLON, DASH, EQUALS, PLUS, STAR, HASH, PERCENT, AT],
    // bars — the block elements
    &[BLANK, bar(1), bar(2), bar(3), bar(4), bar(5), bar(6), bar(7), bar(8)],
    // strokes — which read as a plotted line rather than as a texture
    &[BLANK, UNDERSCORE, DASH, SLASH, PIPE, BACKSLASH, PLUS, HASH],
    // circles
    &[BLANK, MIDDOT, DOT, SMALL_O, BIG_O, ZERO, DISC],
];

/// How many glyph slots the uniform carries. Comfortably above the longest
/// ramp; the shader indexes it by the ramp's own length.
const SLOTS: usize = 32;

/// One glyph as the two `u32` the shader unpacks: four rows in each, low byte
/// first.
fn pack(g: &Glyph) -> [u32; 2] {
    let word = |a: u8, b: u8, c: u8, d: u8| {
        a as u32 | (b as u32) << 8 | (c as u32) << 16 | (d as u32) << 24
    };
    [word(g[0], g[1], g[2], g[3]), word(g[4], g[5], g[6], g[7])]
}

/// Every ramp's glyphs back to back, packed as the shader unpacks them. The
/// `fx.ascii` post effect embeds this table in `post.wgsl` (`ASCII_GLYPHS`);
/// a test there keeps the two equal.
pub fn packed_ramps() -> Vec<[u32; 2]> {
    RAMPS.iter().flat_map(|r| r.iter().map(pack)).collect()
}

/// The length of each ramp, in `RAMPS` order.
pub fn ramp_lengths() -> Vec<usize> {
    RAMPS.iter().map(|r| r.len()).collect()
}

// -------------------------------------------------------------------- model

const FLOATS: &[FloatSpec] = &[
    spec("length", 0.05, 2.0, 0.42),
    spec("amplitude", 0.0, 0.48, 0.26),
    spec("speed", -6.0, 6.0, 1.4),
    spec("thickness", 0.02, 0.7, 0.20),
    // How many sine components stack. Structural — it is which waveform this
    // is, not how loud it is.
    spec_held("waves", 1.0, 4.0, 3.0, 0.25),
    // The character grid. Structural for the same reason: swung by the music
    // the whole page of text reflows on every beat.
    spec_held("columns", 16.0, 160.0, 72.0, 0.15),
    // Which picked set. Pinned outright: driven, it would cycle through all
    // four sets continuously, and the set is a choice rather than a level.
    spec_held("play", 0.0, 3.0, 0.0, 0.0),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at(
            "text",
            &[
                0.85, 0.95, 0.90, //
                0.35, 0.95, 0.75, //
                1.00, 0.35, 0.30, //
                1.00, 0.90, 0.70,
            ],
        ),
        background(),
    crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
    crate::model::EFFECTS_COLOR_BLOCK,
],
};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    color: ColorSlotGpu,
    /// The background the glyphs/panels/liquids are drawn onto.
    ///
    /// It is in the uniform rather than under a `shared::backdrop` pass
    /// because this model's own quad covers every pixel and writes with
    /// `BlendState::REPLACE` — a pass underneath would be drawn and then
    /// erased. Compositing it here is also the more honest of the two: the
    /// background and the model end up in the same colour arithmetic.
    background: ColorSlotGpu,
    floats: [f32; 4],
    floats2: [f32; 4],
    floats3: [f32; 4],
    /// 32 glyphs, `.xy` used and `.zw` padding — a uniform array's stride is
    /// 16 bytes whatever is in it, so two u32 and two wasted is the same
    /// buffer as two u32 packed tight would have been.
    glyphs: [[u32; 4]; SLOTS],
}

pub struct Ascii {
    state: Built<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
}

impl Ascii {
    pub fn new() -> Ascii {
        Ascii { state: Built::default() }
    }

    fn build(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
        let shader =
            crate::shared::build_shader(device, "ascii", include_str!("../../shaders/ascii.wgsl"));

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ascii bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ascii layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ascii pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        (pipeline, bind_group_layout)
    }
}

impl Model for Ascii {
    fn id(&self) -> &'static str {
        "ascii"
    }

    fn name(&self) -> &'static str {
        "ASCII Waveform"
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
        let (pipeline, bind_group_layout) =
            self.state.get(format, || Self::build(ctx.device, format));

        let ramp = RAMPS[(floats[6].round().clamp(0.0, 3.0)) as usize];
        let mut glyphs = [[0u32; 4]; SLOTS];
        for (slot, g) in glyphs.iter_mut().zip(ramp.iter()) {
            let [x, y] = pack(g);
            *slot = [x, y, 0, 0];
        }

        // Square cells. The grid is declared in columns because that is what
        // an operator can judge — how many characters across — and the row
        // count falls out of the frame's own aspect. Deriving it the other way
        // gives characters that stretch every time the output resolution
        // changes shape.
        let cols = floats[5].round().clamp(4.0, 160.0);
        let rows = (cols / ctx.aspect().max(0.01)).round().clamp(4.0, 200.0);

        let uniform = Uniform {
            color: ColorSlotGpu::from_floats(&floats[7..19]),
            background: ColorSlotGpu::from_floats(&floats[19..31]),
            floats: [cols, rows, ramp.len() as f32, floats[3]],
            floats2: [floats[0], floats[1], floats[2], ctx.time],
            floats3: [floats[4], ctx.aspect(), 0.0, 0.0],
            glyphs,
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ascii uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ascii bind group"),
            layout: bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ascii encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ascii pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ramp_runs_from_blank_to_something_solid_and_fits_the_uniform() {
        // The shader indexes the ramp by `floor(density * len)`, so density 0
        // must land on a blank cell and the last entry must be the densest —
        // otherwise the wave reads inside out, with the band empty and the
        // background solid, and nothing anywhere errors.
        for (i, ramp) in RAMPS.iter().enumerate() {
            assert!(ramp.len() <= SLOTS, "ramp {i} does not fit the uniform");
            assert!(ramp.len() >= 2, "ramp {i} has nothing to ramp between");
            let lit = |g: &Glyph| g.iter().map(|r| r.count_ones()).sum::<u32>();
            assert_eq!(lit(&ramp[0]), 0, "ramp {i} does not start blank");
            assert!(
                lit(ramp.last().expect("checked non-empty")) > lit(&ramp[1]),
                "ramp {i} does not end denser than it starts"
            );
        }
    }

    #[test]
    fn the_bars_fill_from_the_bottom() {
        // Generated rather than typed precisely so this can be checked once.
        assert_eq!(bar(1), [0, 0, 0, 0, 0, 0, 0, 0xFF]);
        assert_eq!(bar(8), [0xFF; 8]);
        assert_eq!(bar(0), [0; 8]);
    }

    #[test]
    fn a_glyph_packs_into_two_words_the_shader_can_unpack() {
        // The shader reads row `r` as `(word >> (r % 4 * 8)) & 255`, taking
        // rows 0..3 from x and 4..7 from y. If this packing disagreed, every
        // character would be drawn with its rows shuffled.
        let packed = pack(&UNDERSCORE);
        assert_eq!(packed[0], 0, "rows 0-3 of an underscore are empty");
        assert_eq!(packed[1] >> 24, 0xFF, "row 7 is the solid one");
    }
}
