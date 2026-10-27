//! Model 8 — Chamber. The grid's topology and the 6 cube-face transforms
//! are fixed geometry — built once and cached — while height comes from
//! `shaders/chamber.wgsl`'s travelling pulse, evaluated per vertex from the
//! floats and time alone.

use std::f32::consts::FRAC_PI_2;

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::model::{color, camera, background, spec, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::backdrop::Backdrop;
use crate::shared::mesh::{self as m3, DepthCache, Vertex};
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    spec("origin_x", 0.0, 1.0, 0.5),
    spec("origin_y", 0.0, 1.0, 0.5),
    spec("height", 0.0, 0.8, 0.35),
    spec("propagation", 0.05, 1.0, 0.4),
    spec("delay", 0.0, 1.0, 0.4),
    spec("gravity", 0.0, 1.0, 0.4),
    spec("faces", 1.0, 6.0, 6.0),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        // Inside the room, near the middle of it, with all six faces on by
        // default. From outside and with one face the model rendered as a
        // small trapezoid floating in black — an object in a frame. A chamber
        // is not an object: every direction is a wall, so from in here the
        // grid covers the frame and the pulse arrives from around you.
        camera([0.0, 0.05, 0.30], [0.0, -0.12, -1.0], 96.0),
        color("mesh"),
        background(),
        Block::Floats(CAMERA_TRACK_FLOATS),
    crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
    crate::model::EFFECTS_COLOR_BLOCK,
],
};

/// Reserved for `cam.track` (H4), Turing's convention: appended after the
/// model's own blocks, just before the shared signal/effects tail, so nothing
/// already declared shifts (plan rule 2).
const CAMERA_TRACK_FLOATS: &[FloatSpec] = &[
    spec("cam_x", -3.0, 3.0, 0.0),
    spec("cam_y", -3.0, 3.0, 0.0),
    spec("cam_z", -3.0, 3.0, 0.0),
];

/// Vertices per face edge. The pulse is evaluated per vertex, so a coarse grid
/// shows as pixel-sized facets that crawl while floats animate.
const GRID: u32 = 96;

/// How wide one face is. Exactly the cube's own edge length: at 1.8 against a
/// half-extent of 1 the six faces did not reach each other, so the room had a
/// 0.2-wide gap down every seam — invisible with one face and a lit crack
/// through to the void with six.
const FACE: f32 = 2.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    color: ColorSlotGpu,
    floats: [f32; 4],
    floats2: [f32; 4],
    view_proj: [[f32; 4]; 4],
    faces: [[[f32; 4]; 4]; 6],
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    depth: DepthCache,
}

pub struct Chamber {
    gpu: Built<Gpu>,
    backdrop: Built<Backdrop>,
}

impl Chamber {
    pub fn new() -> Chamber {
        Chamber { gpu: Built::default(), backdrop: Built::default() }
    }

    fn build_grid(device: &wgpu::Device) -> (wgpu::Buffer, wgpu::Buffer, u32) {
        let mut vertices = Vec::with_capacity((GRID * GRID) as usize);
        for gz in 0..GRID {
            for gx in 0..GRID {
                let x = (gx as f32 / (GRID - 1) as f32 - 0.5) * FACE;
                let z = (gz as f32 / (GRID - 1) as f32 - 0.5) * FACE;
                vertices.push(Vertex::new(Vec3::new(x, 0.0, z), Vec3::Y, 0.0));
            }
        }
        let mut indices = Vec::with_capacity(((GRID - 1) * (GRID - 1) * 6) as usize);
        for gz in 0..GRID - 1 {
            for gx in 0..GRID - 1 {
                let i0 = gz * GRID + gx;
                let i1 = i0 + 1;
                let i2 = i0 + GRID;
                let i3 = i2 + 1;
                indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
            }
        }
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chamber vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chamber indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        (vbuf, ibuf, indices.len() as u32)
    }

    /// The 6 interior faces of a unit cube (half-extent 1), each mapped so
    /// the grid's local +Y (its "height") points inward.
    fn cube_faces() -> [[[f32; 4]; 4]; 6] {
        let h = 1.0;
        let xforms = [
            Mat4::from_translation(Vec3::new(0.0, -h, 0.0)),
            Mat4::from_translation(Vec3::new(0.0, h, 0.0)) * Mat4::from_rotation_z(std::f32::consts::PI),
            Mat4::from_translation(Vec3::new(-h, 0.0, 0.0)) * Mat4::from_rotation_z(FRAC_PI_2),
            Mat4::from_translation(Vec3::new(h, 0.0, 0.0)) * Mat4::from_rotation_z(-FRAC_PI_2),
            Mat4::from_translation(Vec3::new(0.0, 0.0, -h)) * Mat4::from_rotation_x(-FRAC_PI_2),
            Mat4::from_translation(Vec3::new(0.0, 0.0, h)) * Mat4::from_rotation_x(FRAC_PI_2),
        ];
        xforms.map(|m| m.to_cols_array_2d())
    }

    fn build(device: &wgpu::Device, format: wgpu::TextureFormat) -> Gpu {
        let shader = crate::shared::build_shader(device, "chamber", include_str!("../../shaders/chamber.wgsl"));
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chamber bgl"),
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
        let pipeline = m3::build_pipeline(
            device,
            format,
            &shader,
            &bind_group_layout,
            "chamber pipeline",
            wgpu::PrimitiveTopology::TriangleList,
            Some(m3::VERTEX_LAYOUT),
        );
        let (vertex_buffer, index_buffer, index_count) = Self::build_grid(device);
        Gpu {
            pipeline,
            bind_group_layout,
            vertex_buffer,
            index_buffer,
            index_count,
            depth: DepthCache::default(),
        }
    }
}

impl Model for Chamber {
    fn id(&self) -> &'static str {
        "chamber"
    }

    fn name(&self) -> &'static str {
        "Chamber"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn backdrop(&self) -> bool {
        true
    }

    /// H4: a `cam.track` node may drive the reserved `cam_x/cam_y/cam_z`.
    fn camera_track(&self) -> bool {
        true
    }

    fn render(
        &mut self,
        ctx: &Ctx,
        floats: &[f32],
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        let gpu = self.gpu.get(format, || Self::build(ctx.device, format));

        let background = ColorSlotGpu::from_floats(&floats[26..38]);
        let cam = &floats[7..14];
        let track = crate::model::camera_track_offset(floats);
        let view_proj = crate::model::tracked_view_proj(
            Vec3::new(cam[0], cam[1], cam[2]),
            Vec3::new(cam[3], cam[4], cam[5]),
            cam[6],
            ctx.aspect(),
            track,
        );

        let uniform = Uniform {
            color: ColorSlotGpu::from_floats(&floats[14..26]),
            floats: [floats[0], floats[1], floats[2], floats[3]],
            floats2: [floats[4], floats[5], ctx.time, floats[6]],
            view_proj,
            faces: Self::cube_faces(),
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chamber uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chamber bind group"),
            layout: &gpu.bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        let depth_view = gpu.depth.get(ctx.device, ctx.width, ctx.height);
        let n_faces = (floats[6].round() as u32).clamp(1, 6);

        // The background, first and as its own pass — it is what clears the
        // frame now, and the pass below loads over it. See `shared::backdrop`
        // for why this is a gradient rather than a clear colour.
        self.backdrop
            .get(format, || Backdrop::new(ctx.device, format))
            .draw(ctx, target, background, 0.0, ctx.fondo_alpha);

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("chamber encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("chamber pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, gpu.vertex_buffer.slice(..));
            pass.set_index_buffer(gpu.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..gpu.index_count, 0, 0..n_faces);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}
