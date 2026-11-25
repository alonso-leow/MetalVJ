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
    // Appended 2026-11-25: how fast the camera travels along the set's
    // outline (loops per minute). The zoom into the outline is implicit.
    spec("trace_speed", 0.0, 4.0, 1.5),
];

/// The outline tracer (2026-11-25). The camera follows the boundary of the
/// current Julia set and zooms into it on its own, so the frame is always on
/// the newest generation of detail; pixels are only computed inside that
/// window, never for the rest of the set.
mod trace {
    const N: usize = 160;
    const ITER: u32 = 96;

    fn escapes(x: f32, y: f32, c: (f32, f32)) -> bool {
        let (mut a, mut b) = (x, y);
        for _ in 0..ITER {
            let (a2, b2) = (a * a, b * b);
            if a2 + b2 > 4.0 {
                return true;
            }
            b = 2.0 * a * b + c.1;
            a = a2 - b2 + c.0;
        }
        false
    }

    /// Boundary cells of the set on a coarse grid, ordered into one loop by
    /// their angle around the set's centroid and smoothed.
    pub fn outline(c: (f32, f32)) -> Vec<(f32, f32)> {
        let span = 1.9f32;
        let at = |i: usize| -span + 2.0 * span * (i as f32 + 0.5) / N as f32;
        let mut inside = vec![false; N * N];
        for j in 0..N {
            for i in 0..N {
                inside[j * N + i] = !escapes(at(i), at(j), c);
            }
        }
        let mut pts = Vec::new();
        for j in 1..N - 1 {
            for i in 1..N - 1 {
                let k = j * N + i;
                if inside[k] && (!inside[k - 1] || !inside[k + 1] || !inside[k - N] || !inside[k + N]) {
                    pts.push((at(i), at(j)));
                }
            }
        }
        if pts.len() < 8 {
            // A dust set: circle the origin instead.
            return (0..64)
                .map(|k| {
                    let a = k as f32 / 64.0 * std::f32::consts::TAU;
                    (0.6 * a.cos(), 0.6 * a.sin())
                })
                .collect();
        }
        let n = pts.len() as f32;
        let (cx, cy) = pts.iter().fold((0.0, 0.0), |s, p| (s.0 + p.0 / n, s.1 + p.1 / n));
        pts.sort_by(|p, q| (p.1 - cy).atan2(p.0 - cx).total_cmp(&(q.1 - cy).atan2(q.0 - cx)));
        let k = (pts.len() / 48).max(1);
        let m = pts.len();
        (0..m)
            .step_by(k)
            .map(|i| {
                let mut s = (0.0, 0.0);
                for d in 0..k {
                    let p = pts[(i + d) % m];
                    s.0 += p.0 / k as f32;
                    s.1 += p.1 / k as f32;
                }
                s
            })
            .collect()
    }

    /// Camera centre and implicit zoom multiplier at `time`.
    pub fn camera(path: &[(f32, f32)], time: f32, speed: f32) -> (f32, f32, f32) {
        let m = path.len() as f32;
        let s = (time * speed / 60.0).rem_euclid(1.0) * m;
        let i = s.floor() as usize % path.len();
        let f = s - s.floor();
        let (a, b) = (path[i], path[(i + 1) % path.len()]);
        let ease = f * f * (3.0 - 2.0 * f);
        // Breathes from the whole set (x1) into the outline (x48) every 16 s.
        let zm = (48f32.ln() * (0.5 - 0.5 * (time * std::f32::consts::TAU / 16.0).cos())).exp();
        (a.0 + (b.0 - a.0) * ease, a.1 + (b.1 - a.1) * ease, zm)
    }
}

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
    /// camera centre on the outline, implicit zoom multiplier
    centre: [f32; 4],
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
            color: ColorSlotGpu::from_floats(&floats[7..19]),
            floats: [floats[0], floats[1], floats[2], floats[3]],
            // The aspect rides in the uniform: a fullscreen fragment pass has
            // no vertex stage to correct in, and without it the set is an
            // ellipse on every output this suite actually draws to.
            floats2: [floats[4], floats[5], ctx.time, ctx.aspect()],
            centre: {
                // The constant the shader iterates (with its drift wander).
                let w = floats[4] * 0.12;
                let c = (floats[1] + w * (ctx.time * 0.21).cos(), floats[2] + w * (ctx.time * 0.17).sin());
                let (x, y, zm) = trace::camera(&trace::outline(c), ctx.time, floats[6]);
                // Eased in from the centre, so the zoomed-out frame is the whole set.
                let k = ((zm - 1.0) / 6.0).clamp(0.0, 1.0);
                [x * k, y * k, zm, 0.0]
            },
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
