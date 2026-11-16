//! Model 11 — Oracle. One UV-sphere mesh (static, built once) reused
//! for every eye instance; `shaders/oracle.wgsl` does the actual iris/pupil/
//! eyelid/gaze work per fragment. See that file's header for why zones on
//! a sphere replace separate lid and iris geometry.

use glam::Vec3;
use wgpu::util::DeviceExt;

use crate::model::{camera, color, color_at, solid4, spec, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::mesh::{self as m3, DepthCache, Vertex};
use crate::shared::ColorSlotGpu;

/// The literal the pupil used to be, kept as the slot's opening value.
const PUPIL_BLACK: [f32; 12] = solid4(0.01, 0.01, 0.015);
/// A skin the eye reads against. Not white: the frame is cleared to this, and
/// a white field is a white *screen*, which is the one thing a VJ output must
/// never look like by accident.
const SKIN: [f32; 12] = solid4(0.34, 0.20, 0.19);

const FLOATS: &[FloatSpec] = &[
    spec("gaze_x", 0.0, 1.0, 0.5),
    spec("gaze_y", 0.0, 1.0, 0.5),
    spec("speed_scan", 0.05, 4.0, 0.8),
    spec("eyelid", 0.0, 1.0, 1.0),
    spec("pupil", 0.0, 1.0, 0.4),
    spec("iris", 0.0, 1.0, 0.55),
    spec("n_eyes", 1.0, 8.0, 1.0),
    // Where the eye sits and how big it is, independent of the camera.
    //
    // The camera block could do all three, and that is exactly why these
    // exist: `metal-dash::drive` never drives a camera (`docs/METALVJ.md`),
    // so an eye that can only be moved by moving the camera is an eye that
    // cannot be moved by the music at all. 0.5 is centred; the range either
    // side is a little over one frame width, so an eye can leave and return.
    spec("pos_x", 0.0, 1.0, 0.5),
    spec("pos_y", 0.0, 1.0, 0.5),
    spec("size", 0.2, 4.0, 1.7),
    // Rolls the whole eye about the view axis, which is what turns a row of
    // eyes into a fan and what makes a blink close on a diagonal.
    spec("turn", -1.0, 1.0, 0.0),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        // Head on, close enough that the iris fills the frame.
        camera([0.0, 0.0, 2.4], [0.0, 0.0, 0.0], 40.0),
        color("iris"),
        color("background"),
        color("shine"),
        // The black of the eye, which was literally `vec3(0.01, 0.01, 0.015)`
        // in the shader — the one part of this model that could not be
        // coloured, in a suite whose whole colour story is that every part can.
        // It keeps that literal as its default: white here would open the model
        // with a white pupil on a white sclera, which is an eye with no pupil.
        color_at("pupil_color", &PUPIL_BLACK),
        // The skin, and the frame behind it: the pass clears to this and the
        // lids are drawn in it, so closing the lids does not reveal a black
        // rectangle where a face should be — it closes an eye in a face. See
        // `render` for why the clear and the lid have to agree exactly.
        color_at("skin", &SKIN),
        Block::Floats(CAMERA_TRACK_FLOATS),
    crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
    crate::model::EFFECTS_COLOR_BLOCK,
],
};

/// Reserved for `cam.track` (H4) — see `models::turing`'s block for the
/// convention and the reasoning.
const CAMERA_TRACK_FLOATS: &[FloatSpec] = &[
    spec("cam_x", -3.0, 3.0, 0.0),
    spec("cam_y", -3.0, 3.0, 0.0),
    spec("cam_z", -3.0, 3.0, 0.0),
];

/// Where `grad4` is sampled for the flat skin, on both sides. The lid and the
/// clear have to pick the same point or the eyelid has a seam.
const SKIN_T: f32 = 0.5;

const RINGS: u32 = 20;
const SEGMENTS: u32 = 28;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    iris_color: ColorSlotGpu,
    background_color: ColorSlotGpu,
    highlight_color: ColorSlotGpu,
    pupil_color: ColorSlotGpu,
    skin_color: ColorSlotGpu,
    floats: [f32; 4],
    floats2: [f32; 4],
    floats3: [f32; 4],
    view_proj: [[f32; 4]; 4],
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    depth: DepthCache,
}

pub struct Oracle {
    gpu: Built<Gpu>,
}

impl Oracle {
    pub fn new() -> Oracle {
        Oracle { gpu: Built::default() }
    }

    fn build_sphere(device: &wgpu::Device) -> (wgpu::Buffer, wgpu::Buffer, u32) {
        let mut vertices = Vec::with_capacity(((RINGS + 1) * (SEGMENTS + 1)) as usize);
        for r in 0..=RINGS {
            let v = r as f32 / RINGS as f32;
            let phi = v * std::f32::consts::PI;
            for s in 0..=SEGMENTS {
                let uu = s as f32 / SEGMENTS as f32;
                let theta = uu * std::f32::consts::TAU;
                let dir = Vec3::new(phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin());
                vertices.push(Vertex::new(dir * 0.45, dir, uu));
            }
        }
        let mut indices = Vec::new();
        let stride = SEGMENTS + 1;
        for r in 0..RINGS {
            for s in 0..SEGMENTS {
                let i0 = r * stride + s;
                let i1 = i0 + 1;
                let i2 = i0 + stride;
                let i3 = i2 + 1;
                indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
            }
        }
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("oracle sphere vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("oracle sphere indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        (vbuf, ibuf, indices.len() as u32)
    }

    fn build(device: &wgpu::Device, format: wgpu::TextureFormat) -> Gpu {
        let shader = crate::shared::build_shader(device, "oracle", include_str!("../../shaders/oracle.wgsl"));
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("oracle bgl"),
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
            "oracle pipeline",
            wgpu::PrimitiveTopology::TriangleList,
            Some(m3::VERTEX_LAYOUT),
        );
        let (vertex_buffer, index_buffer, index_count) = Self::build_sphere(device);
        Gpu { pipeline, bind_group_layout, vertex_buffer, index_buffer, index_count, depth: DepthCache::default() }
    }
}

impl Model for Oracle {
    fn id(&self) -> &'static str {
        "oracle"
    }

    fn name(&self) -> &'static str {
        "Oracle"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
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

        let cam = &floats[11..18];
        let view_proj = crate::model::tracked_view_proj(
            Vec3::new(cam[0], cam[1], cam[2]),
            Vec3::new(cam[3], cam[4], cam[5]),
            cam[6],
            ctx.aspect(),
            crate::model::camera_track_offset(floats),
        );

        let n_eyes = (floats[6].round() as u32).max(1);
        let skin_color = ColorSlotGpu::from_floats(&floats[66..78]);
        let uniform = Uniform {
            iris_color: ColorSlotGpu::from_floats(&floats[18..30]),
            background_color: ColorSlotGpu::from_floats(&floats[30..42]),
            highlight_color: ColorSlotGpu::from_floats(&floats[42..54]),
            pupil_color: ColorSlotGpu::from_floats(&floats[54..66]),
            skin_color,
            floats: [floats[0], floats[1], floats[2], floats[3]],
            floats2: [floats[4], floats[5], floats[6], ctx.time],
            floats3: [floats[7], floats[8], floats[9], floats[10]],
            view_proj,
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("oracle uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("oracle bind group"),
            layout: &gpu.bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        let depth_view = gpu.depth.get(ctx.device, ctx.width, ctx.height);
        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("oracle encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("oracle pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    // Not black: the frame *is* the skin. `ColorSlotGpu::grad4`
                    // is the same walk the shader does, sampled at the same
                    // `SKIN_T`, so the lid and the background are one colour
                    // and a closed eye has nothing to show against.
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(skin_color.clear_at(SKIN_T)),
                        store: wgpu::StoreOp::Store,
                    },
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
            pass.draw_indexed(0..gpu.index_count, 0, 0..n_eyes);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}
