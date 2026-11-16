//! The thick-line pipeline Geoterrain and Escape both draw through.
//! Each model owns none of the GPU state itself — it just turns its floats
//! into a `Vec<Vertex>` per strip and hands them here.

use wgpu::util::DeviceExt;

use super::surface::{layout_entries, MaterialU, SurfaceSlot};
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
    part: MaterialU,
}

pub struct Line2dPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    /// The picture slot a draw without a part binds: the 1x1 fallback texel.
    fallback: std::cell::OnceCell<SurfaceSlot>,
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
            &format!(
                "{}\n{}\n{}",
                include_str!("../../shaders/surface.wgsl"),
                include_str!("../../shaders/material.wgsl"),
                include_str!("../../shaders/line2d.wgsl"),
            ),
        );

        let mut entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }];
        entries.extend(layout_entries(1));
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("line2d bind group layout"),
            entries: &entries,
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

        Line2dPipeline { pipeline, bind_group_layout, fallback: std::cell::OnceCell::new() }
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
        self.draw_part(ctx, target, vertices, strips, color, None);
    }

    /// [`Self::draw`] with the strokes' surface: the part's material uniform
    /// and picture slot (`shared::surface::PartSet`, already synced). The
    /// material tints the stroke colour through `material_apply` on a flat
    /// 2D contract: the point is `(uv, 0)` and the normal faces the camera.
    /// `None`, or a part with no material and no picture, is the plain draw.
    pub fn draw_part(
        &self,
        ctx: &Ctx,
        target: &wgpu::TextureView,
        vertices: &[Vertex],
        strips: &[Strip],
        color: ColorSlotGpu,
        part: Option<(MaterialU, &SurfaceSlot)>,
    ) {
        let (part_u, slot) = match part {
            Some((u, slot)) => (u, slot),
            None => (
                MaterialU::default(),
                self.fallback.get_or_init(|| SurfaceSlot::new(ctx.device, ctx.queue)),
            ),
        };
        let uniform = Uniform { color, misc: [ctx.aspect(), ctx.time, 0.0, 0.0], part: part_u };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("line2d uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("line2d bind group"),
            layout: &self.bind_group_layout,
            entries: &{
                let mut e = vec![wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }];
                e.extend(super::surface::bind_entries(&[slot]));
                e
            },
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
