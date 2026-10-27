//! Model 9 — Membrane. No CPU geometry at all: every hair is derived
//! from its instance index in `shaders/membrane.wgsl`, so this file is
//! only the pipeline/uniform plumbing plus the one instance count draw.

use glam::Vec3;
use wgpu::util::DeviceExt;

use crate::model::{color, camera, background, spec, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::backdrop::Backdrop;
use crate::shared::mesh::{self as m3, DepthCache};
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    spec("long", 0.02, 0.35, 0.12),
    spec("growth", 0.0, 1.0, 0.8),
    spec("density", 32.0, 12000.0, 5200.0),
    spec("wind_direction_x", -1.0, 1.0, 0.6),
    spec("wind_direction_y", -1.0, 1.0, 0.2),
    spec("wind_force", 0.0, 1.0, 0.35),
    spec("luminescence", 0.0, 1.0, 0.5),
    // The living skin (appended; the names above keep their meaning).
    spec("relief", 0.0, 1.0, 0.7),
    spec("bulge", 0.0, 1.0, 0.85),
    spec("wobble_speed", 0.0, 3.0, 1.0),
    spec("wrinkle", 0.0, 1.0, 0.6),
    spec("tension", 0.0, 1.0, 0.4),
    spec("pulse", 0.0, 1.0, 0.5),
    spec("impact", 0.0, 1.0, 0.0),
    spec("impact_age", 0.0, 1.0, 0.0),
    spec("impact_x", -1.0, 1.0, 0.0),
    spec("impact_y", -1.0, 1.0, 0.0),
    spec("pinch", 0.0, 1.0, 0.5),
    spec("translucency", 0.0, 1.0, 0.7),
    spec("rim", 0.0, 1.0, 0.7),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        // Low and near, so the hairs are seen across the plane rather than
        // down onto it — the angle a skin texture reads at — and close enough
        // that the field's own edges are outside the frame.
        camera([0.0, -1.35, 0.95], [0.0, 0.25, 0.05], 62.0),
        color("raiz"),
        color("tip"),
        background(),
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

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    root_color: ColorSlotGpu,
    tip_color: ColorSlotGpu,
    floats: [f32; 4],
    floats2: [f32; 4],
    view_proj: [[f32; 4]; 4],
    f3: [f32; 4],
    f4: [f32; 4],
    f5: [f32; 4],
    f6: [f32; 4],
    cam: [f32; 4],
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    depth: DepthCache,
}

pub struct Membrane {
    gpu: Built<Gpu>,
    backdrop: Built<Backdrop>,
}

impl Membrane {
    pub fn new() -> Membrane {
        Membrane { gpu: Built::default(), backdrop: Built::default() }
    }

    fn build(device: &wgpu::Device, format: wgpu::TextureFormat) -> Gpu {
        let shader = crate::shared::build_shader(
            device,
            "membrane",
            include_str!("../../shaders/membrane.wgsl"),
        );
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("membrane bgl"),
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
            "membrane pipeline",
            wgpu::PrimitiveTopology::TriangleList,
            None,
        );
        Gpu { pipeline, bind_group_layout, depth: DepthCache::default() }
    }
}

impl Model for Membrane {
    fn id(&self) -> &'static str {
        "membrane"
    }

    fn name(&self) -> &'static str {
        "Membrane"
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

        let background = ColorSlotGpu::from_floats(&floats[51..63]);
        let cam = &floats[20..27];
        let track = crate::model::camera_track_offset(floats);
        let view_proj = crate::model::tracked_view_proj(
            Vec3::new(cam[0], cam[1], cam[2]),
            Vec3::new(cam[3], cam[4], cam[5]),
            cam[6],
            ctx.aspect(),
            track,
        );

        let n_hairs = (floats[2].round() as u32).max(1);
        let uniform = Uniform {
            root_color: ColorSlotGpu::from_floats(&floats[27..39]),
            tip_color: ColorSlotGpu::from_floats(&floats[39..51]),
            floats: [floats[0], floats[1], floats[2], floats[3]],
            floats2: [floats[4], floats[5], floats[6], ctx.time],
            view_proj,
            f3: [floats[7], floats[8], floats[9], floats[10]],
            f4: [floats[11], floats[12], floats[13], floats[14]],
            f5: [floats[15], floats[16], floats[17], floats[18]],
            f6: [floats[19], 0.0, 0.0, 0.0],
            cam: [cam[0] + track[0], cam[1] + track[1], cam[2] + track[2], 0.0],
        };
        let mk = |mode: f32, label: &str| {
            let mut u = uniform;
            u.f6[1] = mode;
            let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::bytes_of(&u),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &gpu.bind_group_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
            })
        };
        let surface_bg = mk(0.0, "membrane surface");
        let hair_bg = mk(1.0, "membrane hair");

        let depth_view = gpu.depth.get(ctx.device, ctx.width, ctx.height);
        // The background, first and as its own pass — it is what clears the
        // frame now, and the pass below loads over it. See `shared::backdrop`
        // for why this is a gradient rather than a clear colour.
        self.backdrop
            .get(format, || Backdrop::new(ctx.device, format))
            .draw(ctx, target, background, 0.0, ctx.fondo_alpha);

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("membrane encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("membrane pass"),
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
            pass.set_bind_group(0, &surface_bg, &[]);
            pass.draw(0..(200 * 150 * 6), 0..1);
            pass.set_bind_group(0, &hair_bg, &[]);
            pass.draw(0..3, 0..n_hairs);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}
