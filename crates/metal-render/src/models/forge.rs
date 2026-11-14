//! `forge`: the model whose shader somebody wrote today — usually the AI.
//!
//! Every other model in the suite is WGSL compiled into the binary. This one
//! draws whatever WGSL it is handed, validated by `metal_forge::check` (naga,
//! with line and column on failure) before this device ever sees it: the
//! custom-shader path `metal-forge` opened for *testing* a shader (`raster`,
//! `bake_clip`), now on air.
//!
//! # The contract a forge shader is written against
//!
//! The same fullscreen shape `metal-forge` compiles: `fs_main(in: FsVOut)` with
//! `in.uv` in `0..1`, y down, and one uniform struct at `@group(0)
//! @binding(0)` whose members are `floats`, `floats2`, … (`vec4<f32>` each):
//!
//! | member    | x          | y           | z         | w          |
//! |-----------|------------|-------------|-----------|------------|
//! | `floats`  | time (s)   | aspect w/h  | 0         | 0          |
//! | `floats2` | rms        | bandwidth   | onset     | centroid   |
//! | `floats3` | chroma C   | C#          | D         | D#         |
//! | `floats4` | chroma E   | F           | F#        | G          |
//! | `floats5` | chroma G#  | A           | A#        | B          |
//!
//! `floats2`..`floats5` are the sixteen digest columns — the ingest — as
//! they arrive, each `0..1`, with no mapping in between: `metal-perform` writes
//! them into this model's row directly rather than running `drive::row` over
//! them. A shader that declares fewer members gets the first ones.
//!
//! An **effect** (`fx.forge`) is the same contract plus the picture it
//! works on: `@group(0) @binding(1) var src: texture_2d<f32>;` and
//! `@group(0) @binding(2) var samp: sampler;` — see [`ForgeFx`].
//!
//! # Why a model at all
//!
//! Because a layer is a model: the stack, the graph's `gen.*` nodes, the
//! budget, fan-out, spans, masks — all of it keys off a model index. Making
//! the custom shader one more entry in the table (`forge`, appended, the
//! table is append-only) is what lets it be cut to a contour, spanned across
//! five screens or routed to two projectors with nothing downstream knowing
//! it is different. Which WGSL a `forge` layer draws is its own field
//! (`Layer::shader`), handed here through [`Model::set_source`] right before
//! each frame.

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::model::{spec, Block, Ctx, FloatSpec, Model, Schema};
use crate::shared::fullscreen::Fullscreen;

/// The sixteen ingest columns, in digest order — `metal_perform::digest::COLUMNS`
/// spelled as identifiers. `metal-perform` checks the two lists name the same
/// columns (`the_forge_rows_are_the_digest_columns`).
const FLOATS: &[FloatSpec] = &[
    spec("rms", 0.0, 1.0, 0.0),
    spec("bandwidth", 0.0, 1.0, 0.0),
    spec("onset", 0.0, 1.0, 0.0),
    spec("centroid", 0.0, 1.0, 0.0),
    spec("chroma_c", 0.0, 1.0, 0.0),
    spec("chroma_c_sharp", 0.0, 1.0, 0.0),
    spec("chroma_d", 0.0, 1.0, 0.0),
    spec("chroma_d_sharp", 0.0, 1.0, 0.0),
    spec("chroma_e", 0.0, 1.0, 0.0),
    spec("chroma_f", 0.0, 1.0, 0.0),
    spec("chroma_f_sharp", 0.0, 1.0, 0.0),
    spec("chroma_g", 0.0, 1.0, 0.0),
    spec("chroma_g_sharp", 0.0, 1.0, 0.0),
    spec("chroma_a", 0.0, 1.0, 0.0),
    spec("chroma_a_sharp", 0.0, 1.0, 0.0),
    spec("chroma_b", 0.0, 1.0, 0.0),
];

/// How many ingest floats a forge row starts with.
pub const INGEST: usize = FLOATS.len();

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        crate::model::SIGNAL_FLOATS_BLOCK,
        crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

/// What a `forge` layer with no shader (or a broken one) draws: dark
/// diagonal stripes, so a missing shader reads as missing on the wall
/// instead of as black.
const SLATE: &str = r#"
struct Slate { a: vec4<f32> };
@group(0) @binding(0) var<uniform> s: Slate;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let p = in.uv * vec2<f32>(s.a.y, 1.0) * 14.0;
    let band = step(0.5, fract(p.x + p.y + s.a.x * 0.2));
    return vec4<f32>(mix(vec3<f32>(0.03, 0.05, 0.09), vec3<f32>(0.08, 0.11, 0.18), band), 1.0);
}
"#;

/// The uniform values a forge shader reads, in member order: time and
/// aspect, then the ingest.
pub fn uniform_values(time: f32, aspect: f32, ingest: &[f32]) -> Vec<f32> {
    let mut v = vec![time, aspect, 0.0, 0.0];
    v.extend(ingest.iter().take(INGEST).copied());
    v.resize(4 + INGEST, 0.0);
    v
}

/// One validated shader, compiled for one colour format.
struct Compiled {
    source_hash: u64,
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    checked: metal_forge::Checked,
}

fn hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Builds a pipeline for an already-checked shader, catching whatever
/// validation error the backend still raises (a limit naga does not know)
/// as an `Err` instead of the device's uncaptured-error panic.
fn build(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &str,
    checked: &metal_forge::Checked,
) -> Result<(wgpu::RenderPipeline, wgpu::BindGroupLayout), String> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(checked.source.clone().into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &metal_forge::layout_entries(checked.effect),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
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
    match pollster::block_on(scope.pop()) {
        Some(e) => Err(format!("{label}: the GPU refused the shader: {e}")),
        None => Ok((pipeline, layout)),
    }
}

/// Keeps one compiled pipeline per shader key, rebuilt when the source or
/// the target format changes.
#[derive(Default)]
struct Cache {
    compiled: HashMap<String, Compiled>,
}

impl Cache {
    fn get(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        key: &str,
        checked: &metal_forge::Checked,
    ) -> Result<&Compiled, String> {
        let h = hash(&checked.source);
        let fresh = self
            .compiled
            .get(key)
            .is_some_and(|c| c.source_hash == h && c.format == format);
        if !fresh {
            let (pipeline, layout) = build(device, format, &format!("forge:{key}"), checked)?;
            self.compiled.insert(
                key.to_string(),
                Compiled { source_hash: h, format, pipeline, layout, checked: checked.clone() },
            );
        }
        Ok(self.compiled.get(key).expect("inserted above"))
    }
}

fn draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &wgpu::TextureView,
    c: &Compiled,
    values: &[f32],
    input: Option<(&wgpu::TextureView, &wgpu::Sampler)>,
) {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("forge") });
    draw_into(device, &mut encoder, target, c, values, input);
    queue.submit(Some(encoder.finish()));
}

/// [`draw`] recorded into the caller's encoder (no submit): the GPU-compose
/// pass path.
fn draw_into(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    c: &Compiled,
    values: &[f32],
    input: Option<(&wgpu::TextureView, &wgpu::Sampler)>,
) {
    let bytes = metal_forge::uniform_bytes(c.checked.uniform_size, &c.checked.float_offsets, values);
    let ubuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("forge uniform"),
        contents: &bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }];
    if let Some((view, sampler)) = input {
        entries.push(wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) });
        entries.push(wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) });
    }
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("forge"),
        layout: &c.layout,
        entries: &entries,
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("forge"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&c.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// The generator. See the module doc for the contract.
pub struct Forge {
    /// The shader the next frame draws: its key (the layer's shader id) and
    /// the checked source.
    current: Option<(String, metal_forge::Checked)>,
    cache: Cache,
    slate: Fullscreen,
    /// Why the last shader could not draw, if it could not — the slate is
    /// drawn instead, and this is what a caller reports.
    pub last_error: Option<String>,
}

impl Forge {
    pub fn new() -> Forge {
        Forge { current: None, cache: Cache::default(), slate: Fullscreen::new("forge-slate", SLATE), last_error: None }
    }
}

impl Default for Forge {
    fn default() -> Forge {
        Forge::new()
    }
}

impl Model for Forge {
    fn id(&self) -> &'static str {
        "forge"
    }

    fn name(&self) -> &'static str {
        "Forge (custom shader)"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn render(&mut self, ctx: &Ctx, floats: &[f32], target: &wgpu::TextureView, format: wgpu::TextureFormat) {
        if let Some((key, checked)) = &self.current {
            match self.cache.get(ctx.device, format, key, checked) {
                Ok(c) => {
                    let values = uniform_values(ctx.time, ctx.aspect(), &floats[..INGEST.min(floats.len())]);
                    draw(ctx.device, ctx.queue, target, c, &values, None);
                    self.last_error = None;
                    return;
                }
                Err(e) => self.last_error = Some(e),
            }
        }
        self.slate.draw(ctx, target, format, &[ctx.time, ctx.aspect(), 0.0, 0.0]);
    }

    fn set_source(&mut self, key: &str, wgsl: &str) -> Result<(), String> {
        let checked = metal_forge::check(wgsl).map_err(|e| e.to_string())?;
        if checked.effect {
            self.current = None;
            return Err(format!(
                "{key} samples an input picture: it is an effect, wire it as an `fx.forge` node after a source"
            ));
        }
        self.current = Some((key.to_string(), checked));
        Ok(())
    }
}

/// The `fx.forge` pass: a custom WGSL effect over a finished picture.
///
/// Runs after the generator's own pass and post chain, on the RGBA frame
/// that came back — one upload, one pass, one readback. The input is
/// uploaded as sRGB and the target is sRGB, so a shader that returns
/// `textureSample(src, samp, in.uv)` hands back the picture it was given.
#[derive(Default)]
pub struct ForgeFx {
    cache: Cache,
}

impl ForgeFx {
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        key: &str,
        wgsl: &str,
        input: &[u8],
        w: u32,
        h: u32,
        time: f32,
        ingest: &[f32],
    ) -> Result<Vec<u8>, String> {
        if w == 0 || h == 0 || input.len() != (w * h * 4) as usize {
            return Err(format!("{key}: the input is not a {w}x{h} RGBA frame"));
        }
        let checked = metal_forge::check(wgsl).map_err(|e| e.to_string())?;
        if !checked.effect {
            return Err(format!(
                "{key} does not sample an input picture (`@binding(1)` texture, `@binding(2)` sampler): \
                 it is a generator, use it as a `forge` source"
            ));
        }
        let c = self.cache.get(device, format, key, &checked)?;
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("forge fx input"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            input,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("forge fx"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let (target, target_view) = crate::gpu::offscreen_target(device, w, h, format);
        let values = uniform_values(time, w as f32 / h as f32, ingest);
        draw(device, queue, &target_view, c, &values, Some((&view, &sampler)));
        Ok(crate::gpu::read_rgba8(device, queue, &target, w, h))
    }

    /// [`ForgeFx::apply`] on textures: `input` is sampled and `output`
    /// written (views in `format`), recorded into `enc`. No upload, no
    /// readback, no submit.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        enc: &mut wgpu::CommandEncoder,
        format: wgpu::TextureFormat,
        key: &str,
        wgsl: &str,
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
        aspect: f32,
        time: f32,
        ingest: &[f32],
    ) -> Result<(), String> {
        let checked = metal_forge::check(wgsl).map_err(|e| e.to_string())?;
        if !checked.effect {
            return Err(format!("{key} does not sample an input picture: it is a generator"));
        }
        let c = self.cache.get(device, format, key, &checked)?;
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("forge fx"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let values = uniform_values(time, aspect, ingest);
        draw_into(device, enc, output, c, &values, Some((input, &sampler)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_is_the_ingest_then_the_shared_blocks() {
        assert_eq!(INGEST, 16);
        assert_eq!(SCHEMA.width(), INGEST + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    #[test]
    fn the_uniform_is_time_and_aspect_then_the_ingest() {
        let ingest: Vec<f32> = (0..16).map(|i| i as f32 / 16.0).collect();
        let v = uniform_values(2.5, 1.5, &ingest);
        assert_eq!(&v[..4], &[2.5, 1.5, 0.0, 0.0]);
        assert_eq!(&v[4..], &ingest[..]);
        assert_eq!(uniform_values(0.0, 1.0, &[]).len(), 20, "a short row is padded, not a panic");
    }

    #[test]
    fn an_effect_is_refused_as_a_generator_and_a_broken_shader_says_where() {
        let mut f = Forge::new();
        let err = f.set_source("bad", "fn fs_main() { let x = ; }").unwrap_err();
        assert!(err.contains("line"), "{err}");
        let effect = include_str!("../../../../data/agent/shaders/cup-caustics.wgsl");
        assert!(f.set_source("cup", effect).unwrap_err().contains("fx.forge"));
        let turtle = include_str!("../../../../data/agent/shaders/turtle.wgsl");
        f.set_source("turtle", turtle).expect("the turtle is a generator");
    }
}
