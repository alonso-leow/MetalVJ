//! Model 7 — Entrance. Instanced billboards, one draw call, zero
//! per-frame CPU buffer: `shaders/entrance.wgsl` derives every particle's
//! position from `@builtin(instance_index)` alone.

use wgpu::util::DeviceExt;

use crate::model::{color, background, spec, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::backdrop::Backdrop;
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    spec("speed_advance", 0.0, 2.0, 0.5),
    spec("density", 8.0, 1500.0, 260.0),
    spec("scale", 0.02, 0.5, 0.10),
    spec("variation_palette", 0.0, 1.0, 0.6),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[Block::Floats(FLOATS), color("elements"), background(), crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK, crate::model::EFFECTS_COLOR_BLOCK],
};

const DEPTH_RANGE: f32 = 3.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    color: ColorSlotGpu,
    floats: [f32; 4],
    floats2: [f32; 4],
}

pub struct Entrance {
    state: Built<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
    backdrop: Built<Backdrop>,
}

impl Entrance {
    pub fn new() -> Entrance {
        Entrance { state: Built::default(), backdrop: Built::default() }
    }

    fn build(device: &wgpu::Device, format: wgpu::TextureFormat) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
        let shader = crate::shared::build_shader(
            device,
            "entrance",
            include_str!("../../shaders/entrance.wgsl"),
        );

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("entrance bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("entrance layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("entrance pipeline"),
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
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        (pipeline, bind_group_layout)
    }
}

impl Model for Entrance {
    fn id(&self) -> &'static str {
        "entrance"
    }

    fn name(&self) -> &'static str {
        "Entrance"
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
        let (pipeline, bind_group_layout) =
            self.state.get(format, || Self::build(ctx.device, format));

        let background = ColorSlotGpu::from_floats(&floats[16..28]);
        let n_particles = (floats[1].round() as u32).max(1);
        let uniform = Uniform {
            color: ColorSlotGpu::from_floats(&floats[4..16]),
            floats: [floats[0], floats[1], floats[2], floats[3]],
            floats2: [ctx.time, ctx.aspect(), DEPTH_RANGE, 0.0],
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("entrance uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("entrance bind group"),
            layout: bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        // The background, first and as its own pass — it is what clears the
        // frame now, and the pass below loads over it. See `shared::backdrop`
        // for why this is a gradient rather than a clear colour.
        self.backdrop
            .get(format, || Backdrop::new(ctx.device, format))
            .draw(ctx, target, background, 0.0, ctx.fondo_alpha);

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("entrance encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("entrance pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..4, 0..n_particles);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}
