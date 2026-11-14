//! Model 13 — Kaleidoscope. Geoterrain's arms, evaluated as a field and folded
//! into a mirrored wedge.
//!
//! Asked for on 2026-08-17 as a model "based on Geoterrain", and it is: the
//! arms use Geoterrain's own parametrisation. It is not built on Geoterrain's
//! *pipeline* — that one tessellates lines, and a fold has nothing to fold a
//! triangle strip against. See `shaders/kaleido.wgsl`.
//!
//! The same fold is also a common effect (`Post::kaleido`), where it folds
//! every model in the suite for the same six lines of shader. Both, on purpose:
//! `docs/NEXT.md` §3 recommended the effect and the ask named the model, and
//! what a kaleidoscope folds is most of what it looks like.

use wgpu::util::DeviceExt;

use crate::model::{color_at, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    // How many mirrors. Structural: this is which kaleidoscope you are looking
    // through, and a live signal sweeping it makes the whole figure count its
    // own sides on every beat, which reads as a fault rather than as movement.
    spec_held("mirrors", 2.0, 16.0, 6.0, 0.2),
    // More arms than Geoterrain draws, and much less twist. The two floats
    // trade against each other: `torsion` sweeps the field along a ray and
    // `brazos` repeats it around one, so a high twist and a low arm count
    // folds into concentric rings — technically a kaleidoscope of a spiral,
    // and visibly a target. Angular repetition has to dominate.
    spec_held("arms", 1.0, 12.0, 5.0, 0.3),
    spec("torsion", -24.0, 24.0, 2.6),
    spec("sharpness", 0.5, 24.0, 5.0),
    spec("radius", 0.2, 2.0, 1.05),
    // Radial mirrors. Below 1 is the plain single disc; 1 tiles the field
    // outward without mirroring it, which is what reaches a wide frame's
    // corners, and above that it starts folding in r as well as in theta.
    spec_held("rings", 0.0, 5.0, 1.0, 0.25),
    spec("turn", -2.0, 2.0, 0.35),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        // Not white. A kaleidoscope is coloured glass — four white stops leave
        // a grey mandala, which passes every gate and is not the model.
        color_at(
            "crystal",
            &[
                0.06, 0.05, 0.12, //
                0.80, 0.16, 0.30, //
                1.00, 0.62, 0.20, //
                0.55, 0.92, 0.95,
            ],
        ),
    crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
    crate::model::EFFECTS_COLOR_BLOCK,
],
};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    color: ColorSlotGpu,
    floats: [f32; 4],
    floats2: [f32; 4],
    floats3: [f32; 4],
}

pub struct Kaleido {
    state: Built<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
}

impl Kaleido {
    pub fn new() -> Kaleido {
        Kaleido { state: Built::default() }
    }

    fn build(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
        let shader = crate::shared::build_shader(
            device,
            "kaleido",
            include_str!("../../shaders/kaleido.wgsl"),
        );

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("kaleido bgl"),
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
            label: Some("kaleido layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("kaleido pipeline"),
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

impl Model for Kaleido {
    fn id(&self) -> &'static str {
        "kaleido"
    }

    fn name(&self) -> &'static str {
        "Kaleidoscope"
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
            color: ColorSlotGpu::from_floats(&floats[7..19]),
            floats: [floats[0], floats[1], floats[2], floats[3]],
            floats2: [floats[4], floats[5], floats[6], ctx.time],
            floats3: [ctx.aspect(), 0.0, 0.0, 0.0],
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("kaleido uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("kaleido bind group"),
            layout: bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("kaleido encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("kaleido pass"),
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
