//! Model 12 — Fractals. A Julia orbit on Domain's pipeline: one fullscreen
//! fragment pass and no geometry. Asked for on 2026-08-17 as one of two models
//! "based on Geoterrain"; it is Domain's shape, not Geoterrain's, because a
//! fractal is a field over the frame and not a set of lines.

use wgpu::util::DeviceExt;

use crate::model::{color_at, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    spec("zoom", 0.2, 8.0, 1.15),
    // The two coordinates of the Julia constant. This pair is what makes it
    // one fractal rather than another, so both are structural: swept by the
    // music they leave the connected region and the frame goes to a dusting of
    // isolated points — in range, no error, and no longer a fractal. The same
    // failure Turing's feed and kill rates produce, see `FloatSpec::drive`.
    spec_held("c_real", -1.2, 1.2, -0.79, 0.25),
    spec_held("c_imag", -1.2, 1.2, 0.15, 0.25),
    // Iterations decide how much *detail* exists, not how bright it is; driven
    // hard it flickers the whole filigree in and out on every beat.
    spec_held("detail", 16.0, 320.0, 150.0, 0.3),
    spec("drift", 0.0, 1.0, 0.35),
    spec("fold", 0.0, 1.0, 0.0),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        // Not white. Four white stops make escape time invisible — the whole
        // image collapses to one flat sheet, which passes every render gate
        // and shows nothing. This starts on the suite's own greys and red.
        color_at(
            "escape",
            &[
                0.04, 0.04, 0.05, //
                0.55, 0.10, 0.10, //
                0.95, 0.35, 0.18, //
                1.00, 0.95, 0.88,
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
}

pub struct Fractals {
    state: Built<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
}

impl Fractals {
    pub fn new() -> Fractals {
        Fractals { state: Built::default() }
    }

    fn build(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
        let shader = crate::shared::build_shader(
            device,
            "fractals",
            include_str!("../../shaders/fractals.wgsl"),
        );

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fractals bgl"),
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
            label: Some("fractals layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fractals pipeline"),
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

impl Model for Fractals {
    fn id(&self) -> &'static str {
        "fractals"
    }

    fn name(&self) -> &'static str {
        "Fractals"
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
            color: ColorSlotGpu::from_floats(&floats[6..18]),
            floats: [floats[0], floats[1], floats[2], floats[3]],
            // The aspect rides in the uniform: a fullscreen fragment pass has
            // no vertex stage to correct in, and without it the set is an
            // ellipse on every output this suite actually draws to.
            floats2: [floats[4], floats[5], ctx.time, ctx.aspect()],
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fractals uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fractals bind group"),
            layout: bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fractals encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fractals pass"),
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
