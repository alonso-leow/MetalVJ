//! Model 2 — Cyberspace. See `shaders/cyberspace.wgsl` — a fullscreen
//! pass with a bounded loop over layers, so the Rust side is only the usual
//! pipeline/uniform plumbing.

use wgpu::util::DeviceExt;

use crate::model::{color, background, spec, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    spec("n_layers", 1.0, 8.0, 4.0),
    spec("depth", 0.1, 2.0, 1.0),
    spec("speed_parallax", -2.0, 2.0, 0.4),
    spec("neon", 0.0, 2.0, 0.8),
    spec("glass", 0.0, 1.0, 0.4),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color("panel"),
        color("neon"),
        background(),
    crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
    crate::model::EFFECTS_COLOR_BLOCK,
],
};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    panel_color: ColorSlotGpu,
    neon_color: ColorSlotGpu,
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
}

pub struct Cyberspace {
    state: Built<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
}

impl Cyberspace {
    pub fn new() -> Cyberspace {
        Cyberspace { state: Built::default() }
    }

    fn build(device: &wgpu::Device, format: wgpu::TextureFormat) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
        let shader = crate::shared::build_shader(
            device,
            "cyberspace",
            include_str!("../../shaders/cyberspace.wgsl"),
        );

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cyberspace bgl"),
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
            label: Some("cyberspace layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cyberspace pipeline"),
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

impl Model for Cyberspace {
    fn id(&self) -> &'static str {
        "cyberspace"
    }

    fn name(&self) -> &'static str {
        "Cyberspace"
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

        let uniform = Uniform {
            panel_color: ColorSlotGpu::from_floats(&floats[5..17]),
            neon_color: ColorSlotGpu::from_floats(&floats[17..29]),
            background: ColorSlotGpu::from_floats(&floats[29..41]),
            floats: [floats[0], floats[1], floats[2], floats[3]],
            floats2: [floats[4], ctx.time, ctx.aspect(), 0.0],
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cyberspace uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cyberspace bind group"),
            layout: bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("cyberspace encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cyberspace pass"),
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
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}
