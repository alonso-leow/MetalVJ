//! Model 14 — Vaporgrid. A ground plane of lines receding to a horizon, a sun
//! behind it, and infinite forward scroll.
//!
//! Asked for on 2026-08-17 as a child of Entrance ("super zoom") and built on
//! Chamber's shape instead: a displaced height field drawn as lines. Entrance
//! is instanced billboards deriving every particle from `instance_index`, and
//! a vapourwave grid has no particles in it — `docs/NEXT.md` §5 has the whole
//! argument. What survives from Entrance is the *feeling*: things arrive out
//! of the horizon and never stop arriving.
//!
//! Two passes: the sky and the sun as a fullscreen triangle with no depth,
//! then the grid over it. The geometry is rebuilt only when the line counts
//! change; the scroll is a uniform offset with a wrap, so nothing is recycled.

use glam::Vec3;
use wgpu::util::DeviceExt;

use crate::model::{camera, color_at, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::mesh::{self as m3, DepthCache, Vertex};
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    spec("speed", 0.0, 6.0, 1.6),
    // The two line counts. Structural: they are what the grid *is*, and a
    // count swung by the music rebuilds the mesh on every beat — which is both
    // the most expensive thing this model can do and the one that reads as a
    // fault. Held near what the operator set.
    spec_held("lines_x", 4.0, 64.0, 24.0, 0.2),
    spec_held("lines_z", 4.0, 64.0, 26.0, 0.2),
    spec("perturbation", 0.0, 1.0, 0.35),
    spec("glitch", 0.0, 1.0, 0.12),
    spec("sun", 0.0, 1.0, 0.6),
    spec_held("ceiling", 0.0, 1.0, 1.0, 0.0),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        // Low, near the ground, looking at the horizon — a driver's eye. From
        // above, a grid of lines is a plan drawing rather than a receding
        // plane, and the whole model is the recession.
        camera([0.0, 1.15, 2.6], [0.0, 1.05, -12.0], 62.0),
        color_at(
            "grid",
            &[
                0.20, 0.05, 0.35, //
                0.85, 0.15, 0.75, //
                1.00, 0.35, 0.85, //
                0.55, 0.95, 1.00,
            ],
        ),
        color_at(
            "horizon",
            &[
                0.04, 0.02, 0.10, //
                0.28, 0.05, 0.35, //
                0.85, 0.20, 0.45, //
                1.00, 0.55, 0.30,
            ],
        ),
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

/// How far the plane reaches sideways, and the two ends of it in depth. Fixed
/// rather than floats: they are the frame the camera default was chosen
/// against, and a plane that can be made shorter than the view is a plane with
/// a visible far edge across the middle of the picture.
const HALF_WIDTH: f32 = 9.0;
const Z_FAR: f32 = -34.0;
const Z_NEAR: f32 = 3.0;

/// Points along one line. Enough that a line bends smoothly through the height
/// field rather than reading as a chain of chords.
const SEG: usize = 56;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    grid: ColorSlotGpu,
    horizon: ColorSlotGpu,
    floats: [f32; 4],
    floats2: [f32; 4],
    floats3: [f32; 4],
    view_proj: [[f32; 4]; 4],
}

struct Gpu {
    background: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    depth: DepthCache,
    /// The line mesh and the counts it was built for.
    mesh: Option<(wgpu::Buffer, u32, u32, u32)>,
}

pub struct Vaporgrid {
    gpu: Built<Gpu>,
}

impl Vaporgrid {
    pub fn new() -> Vaporgrid {
        Vaporgrid { gpu: Built::default() }
    }

    /// The grid as a line list: `nx` rails running to the horizon and `nz + 1`
    /// crossing lines.
    ///
    /// One extra crossing line, on purpose. The scroll slides the whole set by
    /// up to one spacing; without the spare, the far end of the grid would open
    /// a gap the width of a cell every time the offset wrapped.
    fn build_mesh(device: &wgpu::Device, nx: u32, nz: u32) -> (wgpu::Buffer, u32) {
        let mut v: Vec<Vertex> = Vec::new();
        let spacing = (Z_NEAR - Z_FAR) / nz as f32;

        // Rails: constant x, running the whole depth.
        for i in 0..nx {
            let x = if nx == 1 {
                0.0
            } else {
                (i as f32 / (nx - 1) as f32 - 0.5) * 2.0 * HALF_WIDTH
            };
            let tag = Vec3::new(i as f32, 0.0, 0.0);
            let mut previous = None;
            for k in 0..SEG {
                let z = Z_FAR + (Z_NEAR - Z_FAR) * (k as f32 / (SEG - 1) as f32);
                let point = Vertex::new(Vec3::new(x, 0.0, z), tag, 0.0);
                if let Some(p) = previous {
                    v.push(p);
                    v.push(point);
                }
                previous = Some(point);
            }
        }

        // Crossing lines: constant z, spanning the width. These are the ones
        // that scroll, so they are the ones a glitch can pick — `tag.y = 1`.
        for k in 0..=nz {
            let z = Z_FAR - spacing + k as f32 * spacing;
            let tag = Vec3::new(k as f32 + 0.5, 1.0, 0.0);
            let mut previous = None;
            for i in 0..SEG {
                let x = (i as f32 / (SEG - 1) as f32 - 0.5) * 2.0 * HALF_WIDTH;
                let point = Vertex::new(Vec3::new(x, 0.0, z), tag, 0.0);
                if let Some(p) = previous {
                    v.push(p);
                    v.push(point);
                }
                previous = Some(point);
            }
        }

        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vaporgrid lines"),
            contents: bytemuck::cast_slice(&v),
            usage: wgpu::BufferUsages::VERTEX,
        });
        (buffer, v.len() as u32)
    }

    fn build(device: &wgpu::Device, format: wgpu::TextureFormat) -> Gpu {
        let shader = crate::shared::build_shader(
            device,
            "vaporgrid",
            include_str!("../../shaders/vaporgrid.wgsl"),
        );
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vaporgrid bgl"),
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

        // The sky has no depth attachment and so cannot use the shared mesh
        // pipeline builder, which always declares one.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vaporgrid background layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let background = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vaporgrid background"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_bg"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_bg"),
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

        let lines = m3::build_pipeline(
            device,
            format,
            &shader,
            &bind_group_layout,
            "vaporgrid lines",
            wgpu::PrimitiveTopology::LineList,
            Some(m3::VERTEX_LAYOUT),
        );

        Gpu {
            background,
            lines,
            bind_group_layout,
            depth: DepthCache::default(),
            mesh: None,
        }
    }
}

impl Model for Vaporgrid {
    fn id(&self) -> &'static str {
        "vaporgrid"
    }

    fn name(&self) -> &'static str {
        "Vaporgrid"
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

        let nx = (floats[1].round() as u32).clamp(2, 64);
        let nz = (floats[2].round() as u32).clamp(2, 64);
        let stale = !matches!(&gpu.mesh, Some((_, _, mx, mz)) if *mx == nx && *mz == nz);
        if stale {
            let (buffer, count) = Self::build_mesh(ctx.device, nx, nz);
            gpu.mesh = Some((buffer, count, nx, nz));
        }
        let Some((vbuf, vertex_count, _, _)) = &gpu.mesh else { return };

        let cam = &floats[7..14];
        let view_proj = crate::model::tracked_view_proj(
            Vec3::new(cam[0], cam[1], cam[2]),
            Vec3::new(cam[3], cam[4], cam[5]),
            cam[6],
            ctx.aspect(),
            crate::model::camera_track_offset(floats),
        );
        let spacing = (Z_NEAR - Z_FAR) / nz as f32;

        let uniform = Uniform {
            grid: ColorSlotGpu::from_floats(&floats[14..26]),
            horizon: ColorSlotGpu::from_floats(&floats[26..38]),
            floats: [floats[0], spacing, floats[3], floats[4]],
            floats2: [floats[5], floats[6], ctx.time, HALF_WIDTH],
            floats3: [Z_FAR, Z_NEAR, ctx.aspect(), 0.0],
            view_proj,
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vaporgrid uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vaporgrid bind group"),
            layout: &gpu.bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        let depth_view = gpu.depth.get(ctx.device, ctx.width, ctx.height);
        let planes = if floats[6] > 0.5 { 2 } else { 1 };

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vaporgrid encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("vaporgrid sky"),
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
            pass.set_pipeline(&gpu.background);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("vaporgrid pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    // Loads: the sky was just laid down and the grid goes over
                    // it. Clearing here would be a grid on black.
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&gpu.lines);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, vbuf.slice(..));
            pass.draw(0..*vertex_count, 0..planes);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}
