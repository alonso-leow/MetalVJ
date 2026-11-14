//! One triangle, one uniform, one fragment shader — the shape most of the 2D
//! models in this suite actually are.
//!
//! # Why this exists, and why it is not a refactor of everything
//!
//! Rings, Puzzle, Scratch, Lazer and Sandstorm each carry their own copy of the
//! same hundred and twenty lines: a bind group layout with one uniform buffer
//! in it, a pipeline layout, a pipeline over a vertex shader that draws
//! `0..3`, an encoder, and a pass. The five copies are byte-for-byte the same
//! apart from a label. That was fine at one and defensible at two; the five
//! asked for on 2026-08-23 would have made it ten, and ten copies of a thing is
//! not a pattern, it is a place for one of them to quietly differ.
//!
//! So the boilerplate is here, once, and a model that is *a field over the
//! frame* is now its `Schema`, its uniform, and its WGSL — the three things
//! that are actually about that model.
//!
//! **The models that already have their own copy keep it, for now.** Migrating
//! them is a separate change with a separate risk: each one would have to be
//! re-verified against the render gate, and a gate that only checks "not black,
//! no NaN, deterministic" would not catch a blend state or a colour format
//! quietly changing on the way through. They can move one at a time, each with
//! its own before-and-after. Writing this down so the duplication that is left
//! reads as a decision rather than as an oversight.
//!
//! # What it does not do
//!
//! It does not own the *background*. A model using this still composites its
//! own `fondo` in its fragment shader, because that is the whole reason these
//! models do not run the shared backdrop pass: their own quad covers every
//! pixel. And it does not hold state between frames — `Built` keys the pipeline
//! on the target format for the reason `model::Built` explains, and that is the
//! only thing here that survives a frame.

use wgpu::util::DeviceExt;

use crate::model::{Built, Ctx};

/// The vertex half, shared by every model that uses this: one oversized
/// triangle covering the frame, handing the fragment stage a 0..1 `uv` with y
/// down. Concatenated ahead of the model's own WGSL, after the global prelude.
pub const VERTEX: &str = r#"
struct FsVOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> FsVOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    let xy = p[vi];
    var out: FsVOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return out;
}
"#;

/// A fullscreen fragment pass with exactly one uniform bound at `@group(0)
/// @binding(0)`, and optionally `N` texture/sampler pairs at bindings
/// `1, 2, 3, 4, ...` (texture at `1 + 2i`, sampler at `2 + 2i`).
pub struct Fullscreen {
    label: &'static str,
    wgsl: &'static str,
    textures: usize,
    state: Built<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
}

impl Fullscreen {
    /// `wgsl` supplies `fs_main` and its uniform struct. `VERTEX` and the
    /// global prelude are prepended for it.
    pub fn new(label: &'static str, wgsl: &'static str) -> Fullscreen {
        Fullscreen::with_textures(label, wgsl, 0)
    }

    /// As [`Fullscreen::new`], with `textures` texture/sampler pairs in group
    /// 0 after the uniform. Pass them to [`Fullscreen::draw_textured`].
    pub fn with_textures(label: &'static str, wgsl: &'static str, textures: usize) -> Fullscreen {
        Fullscreen { label, wgsl, textures, state: Built::default() }
    }

    /// Draws the frame. `uniform` is written fresh every frame on purpose:
    /// these models are pure functions of a float row and a clock, so there is
    /// no version of this where the buffer is worth keeping and diffing.
    pub fn draw<U: bytemuck::Pod>(
        &mut self,
        ctx: &Ctx,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        uniform: &U,
    ) {
        self.draw_textured(ctx, target, format, uniform, &[]);
    }

    /// [`Fullscreen::draw`] with the texture/sampler pairs this pass was built
    /// for (`textures.len()` must equal the count given to `with_textures`).
    pub fn draw_textured<U: bytemuck::Pod>(
        &mut self,
        ctx: &Ctx,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        uniform: &U,
        textures: &[(&wgpu::TextureView, &wgpu::Sampler)],
    ) {
        assert_eq!(textures.len(), self.textures, "{}: texture pairs do not match the layout", self.label);
        let (label, wgsl, count) = (self.label, self.wgsl, self.textures);
        let (pipeline, layout) =
            self.state.get(format, || build(ctx.device, format, label, wgsl, count));

        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::bytes_of(uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }];
        for (i, (view, sampler)) in textures.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 1 + 2 * i as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 2 + 2 * i as u32,
                resource: wgpu::BindingResource::Sampler(sampler),
            });
        }
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &entries,
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
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

fn build(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &'static str,
    wgsl: &str,
    textures: usize,
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let shader = crate::shared::build_shader(device, label, &format!("{VERTEX}\n{wgsl}"));

    let mut entries = vec![wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }];
    for i in 0..textures as u32 {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 1 + 2 * i,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 2 + 2 * i,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
    }
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &entries,
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(&bind_group_layout)],
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

    (pipeline, bind_group_layout)
}
