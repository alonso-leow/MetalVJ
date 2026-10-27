//! The thick-line pipeline Geoterrain and Escape both draw through.
//! Each model owns none of the GPU state itself — it just turns its floats
//! into a `Vec<Vertex>` per strip and hands them here.

use wgpu::util::DeviceExt;

use super::ColorSlotGpu;
use crate::model::Ctx;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub t: f32,
    pub _pad: f32,
}

impl Vertex {
    pub fn new(pos: glam::Vec2, t: f32) -> Vertex {
        Vertex { pos: pos.into(), t, _pad: 0.0 }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    color: ColorSlotGpu,
    misc: [f32; 4],
}

pub struct Line2dPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

/// One continuous triangle strip: `verts[start..start+len]` pulled from the
/// shared vertex buffer built for this frame.
pub struct Strip {
    pub start: u32,
    pub len: u32,
}

impl Line2dPipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Line2dPipeline {
        let shader = super::build_shader(
            device,
            "line2d",
            include_str!("../../shaders/line2d.wgsl"),
        );

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("line2d bind group layout"),
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
            label: Some("line2d pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("line2d pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(vertex_layout)],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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

        Line2dPipeline { pipeline, bind_group_layout }
    }

    /// Draws each strip in `strips` from one shared vertex buffer built out
    /// of `vertices`, **over whatever is already in `target`**.
    ///
    /// It used to clear to black first. It does not any more because both
    /// models that draw through it now run `shared::backdrop` ahead of it, and
    /// a clear here would wipe that background out. A caller that wants black
    /// draws the backdrop with a black slot, which is what `fondo()` starts at.
    pub fn draw(
        &self,
        ctx: &Ctx,
        target: &wgpu::TextureView,
        vertices: &[Vertex],
        strips: &[Strip],
        color: ColorSlotGpu,
    ) {
        let uniform = Uniform { color, misc: [ctx.aspect(), 0.0, 0.0, 0.0] };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("line2d uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("line2d bind group"),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        // Created ahead of the render pass so its lifetime plainly covers
        // every draw call made against it below.
        let vbuf = (!vertices.is_empty()).then(|| {
            ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("line2d vertices"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("line2d encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("line2d pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if let Some(vbuf) = &vbuf {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.set_vertex_buffer(0, vbuf.slice(..));
                for strip in strips {
                    if strip.len >= 2 {
                        pass.draw(strip.start..strip.start + strip.len, 0..1);
                    }
                }
            }
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}
