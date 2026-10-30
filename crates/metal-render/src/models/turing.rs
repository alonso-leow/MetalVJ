//! Model 12 — Turing. Reaction-diffusion (Gray-Scott) on a torus, advected by
//! a flow field, displacing a 3D sheet so the chemical ridges arrive as
//! travelling creases. `shaders/turing.wgsl` holds the reaction and the
//! surface; this is the ping-pong plumbing and the stepping rule.
//!
//! **The one model that keeps state.** The other eleven are pure functions of
//! (floats, time) and that is deliberate — it is what makes a preset
//! reproduce and the render gate deterministic. A reaction cannot be: its
//! whole content is the accumulation, and re-deriving it from t=0 every frame
//! would mean thousands of dispatches per frame instead of a dozen. So it
//! keeps the field between frames, and keeps the property that actually
//! mattered instead: *the simulation advances when time advances*. Asking for
//! the same instant twice steps nothing and returns the same image, so the
//! render gate's determinism check still holds, and a fresh instance always
//! starts from the same seed and the same burn-in.

use wgpu::util::DeviceExt;

use crate::model::{color, camera, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::mesh::{self as m3, DepthCache};
use crate::shared::{ColorSlotGpu, CameraGpu};

const FLOATS: &[FloatSpec] = &[
    // The two Gray-Scott parameters. Where you land in this square is which
    // pattern you get — spots, stripes, mitosis, coral — so these two are the
    // model's main "which pattern" control. The default sits in the labyrinth
    // regime, which grows long continuous ridges: those are the ones that read
    // as creases travelling across cloth. Coral (around 0.037 / 0.060) grows
    // isolated towers instead, which is a different and much harder look to
    // get waves out of.
    //
    // Held against live drive (`FloatSpec::drive`) for exactly that reason:
    // these two say *which* pattern, not how much of it, and a signal free to
    // sweep them walks the reaction out of the pattern-forming regime
    // altogether — the mesh then renders as a flat blank sheet, with nothing
    // out of range and nothing logged. A slider may still go anywhere.
    spec_held("feed", 0.010, 0.090, 0.030, 0.10),
    spec_held("shrub", 0.030, 0.075, 0.057, 0.10),
    // Diffusion sets the feature size. Capped where the explicit step stays
    // stable: with the nine-point stencil at dt = 1 the field diverges past
    // about 1.25, and a diverged field is a black frame. Also held: the top of
    // the range is a stability limit, and a limit is not somewhere to be
    // driven towards eight times a second.
    spec_held("diffusion_u", 0.20, 1.15, 1.00, 0.20),
    spec_held("diffusion_v", 0.05, 0.60, 0.50, 0.20),
    // How fast the reaction runs, in steps per 1/60 s. 0 freezes the pattern
    // where it is without freezing the sheet under it.
    spec("step", 0.0, 40.0, 12.0),
    // How many times the field tiles across the sheet. Raised with the
    // sheet: the sheet grew from a 3.2-unit square to a 16-unit one so it
    // could run off every edge of the frame instead of sitting in the middle
    // of it, and at the old tiling that would have stretched one pattern over
    // five times the area. 5 across 16 units is the density 1 had across 3.2.
    spec("scale", 1.0, 12.0, 5.0),
    // The movement: how hard, which way, and how much it swirls on the way.
    spec("flow_force", 0.0, 1.0, 0.25),
    spec("flow_direction", 0.0, 1.0, 0.12),
    spec("flow_turn", 0.0, 1.0, 0.35),
    // The surface: how deep the creases cut and how sharp their edges are.
    spec("fold", 0.0, 1.0, 0.55),
    spec("sharp", 0.0, 1.0, 0.25),
    // The swell the creases ride on.
    spec("wave_high", 0.0, 1.0, 0.25),
    spec("wave_freq", 0.2, 6.0, 1.60),
    spec("wave_speed", 0.0, 2.0, 0.60),
    spec("light", 0.0, 1.0, 0.60),
    spec("shine", 0.0, 1.0, 0.40),
    // Changing this reseeds: a different starting pattern, same rules.
    //
    // Pinned against live drive, not merely damped. A reseed throws away the
    // pattern and pays BURN_IN steps to grow a new one; driven by a signal
    // that changes every frame this model would spend its whole life in
    // burn-in and never show a pattern at all.
    spec_held("seed", 0.0, 1.0, 0.0, 0.0),
];

/// Reserved for `cam.track` (`the design notes` §13.2) — Turing is the
/// pilot generator for `camera_track`. Appended after the model's own camera
/// block and before the shared effects tail, never in the middle: nothing
/// above this point shifts, and `EFFECTS_WIDTH`-from-the-end arithmetic
/// (`metal_schema::read_post`, `apply_node_post`) keeps finding the right place
/// because the effects blocks are still the trailing two.
///
/// Each defaults to 0.0 and ranges wide enough to cover a generous
/// `cam.track` sensitivity without immediately leaving the sheet — a
/// judgment call sized against this model's own camera-to-subject distance
/// (`camera(...)` below sits at z≈2.2 looking at z≈0.86), not a measured
/// limit. Without a `cam.track` connected these stay at their default and
/// `render()` below adds exactly nothing to the camera it already had.
const CAMERA_TRACK_FLOATS: &[FloatSpec] = &[
    spec("cam_x", -3.0, 3.0, 0.0),
    spec("cam_y", -3.0, 3.0, 0.0),
    spec("cam_z", -3.0, 3.0, 0.0),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color("patron"),
        color("pliegue_color"),
        // Looking down at the sheet steeply enough that its far edge is above
        // the top of the frame. A ground plane seen from a low angle always
        // has a horizon, and a horizon in this suite is a band of black sky —
        // the model reading as an object sitting in a frame. 48° down with a
        // 74° lens puts the whole frame on the sheet.
        camera([0.0, 2.80, 2.20], [0.0, 1.31, 0.86], 74.0),
        Block::Floats(CAMERA_TRACK_FLOATS),
    crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
    crate::model::EFFECTS_COLOR_BLOCK,
],
};

/// Deliberately coarser than the mesh. A field as fine as the mesh puts a
/// whole crease inside one or two vertices, which is a spike, not a fold —
/// at half the mesh resolution each ridge spans several vertices and the
/// interpolation between them is the smooth flank.
const FIELD_SIZE: u32 = 128;
const MESH_N: u32 = 256;
const FIELD_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Steps run at seed time so the first frame you see already has a pattern
/// rather than the noise it started from.
const BURN_IN: u32 = 600;
/// Cap on steps in any one frame. Without it a dropped frame turns into a
/// visible jump, and a paused window into a multi-second stall on resume.
const MAX_STEPS_PER_FRAME: u32 = 40;

/// A forward jump larger than this is a seek and reseeds. Smaller ones keep
/// the field.
///
/// **It is a minute because of the router.** It used to be one second, which
/// was right when the only thing that could jump the clock was a seek. Automix
/// rotates a model off air for the length of three switches — half a minute at
/// a slow tempo — and this model is the only one in the suite with a field to
/// lose. At one second, every return burnt in a brand new labyrinth: a
/// different pattern each time and a 600-step hitch on the frame it happened,
/// which is not what "the same animation came back round" should look like.
/// `docs/NEXT.md` §1 left this open; the router is what settles it.
const SEEK_GAP: f32 = 60.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    pattern_color: ColorSlotGpu,
    crease_color: ColorSlotGpu,
    view_proj: [[f32; 4]; 4],
    f0: [f32; 4],
    f1: [f32; 4],
    f2: [f32; 4],
    f3: [f32; 4],
    f4: [f32; 4],
}

/// How the field is stepped. Compute where the device has it; otherwise the
/// same `seed_value` / `step_value` drawn into the other half of the pair by
/// a fragment pass (WebGL2).
enum Stepper {
    Compute {
        seed: wgpu::ComputePipeline,
        step: wgpu::ComputePipeline,
        bgl: wgpu::BindGroupLayout,
    },
    PingPong {
        seed: wgpu::RenderPipeline,
        step: wgpu::RenderPipeline,
    },
}

struct Gpu {
    stepper: Stepper,
    render_pipeline: wgpu::RenderPipeline,
    render_bgl: wgpu::BindGroupLayout,
    /// Ping-pong pair. Each is written as a storage texture on one step and
    /// read as a sampled one on the next; a single texture cannot be both
    /// inside one dispatch.
    field: [wgpu::Texture; 2],
    storage_view: [wgpu::TextureView; 2],
    sampled_view: [wgpu::TextureView; 2],
    index_buffer: wgpu::Buffer,
    index_count: u32,
    depth: DepthCache,
}

#[derive(Default)]
struct Sim {
    /// Which of the pair currently holds the field.
    cur: usize,
    /// The time of the last call. `None` means nothing has been seeded yet.
    wall: Option<f32>,
    /// Fractional steps owed from previous frames, so a slow rate still
    /// advances instead of rounding to zero every frame for ever.
    carry: f32,
    seed_key: u32,
}

pub struct Turing {
    gpu: Built<Gpu>,
    sim: Sim,
    /// Step with the fragment ping-pong even where compute exists (the parity
    /// test runs both on one adapter).
    force_pingpong: bool,
}

impl Turing {
    pub fn new() -> Turing {
        Turing { gpu: Built::default(), sim: Sim::default(), force_pingpong: false }
    }

    #[cfg(test)]
    fn pingpong() -> Turing {
        Turing { force_pingpong: true, ..Turing::new() }
    }

    fn build(device: &wgpu::Device, format: wgpu::TextureFormat, pingpong: bool) -> Gpu {
        use crate::shared::sim::{with_compute, without_compute};
        let src = include_str!("../../shaders/turing.wgsl");
        let src = if pingpong {
            format!("{}\n{}", without_compute(src), include_str!("../../shaders/turing_pp.wgsl"))
        } else {
            with_compute(src)
        };
        let shader = crate::shared::build_shader(device, "turing", &src);

        let compute = (!pingpong).then(|| {
            let compute_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("turing compute bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: FIELD_FORMAT,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            });
            let compute_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("turing compute layout"),
                bind_group_layouts: &[Some(&compute_bgl)],
                immediate_size: 0,
            });
            let compute_pipeline = |entry: &str, label: &str| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(&compute_layout),
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                })
            };
            let seed_pipeline = compute_pipeline("cs_seed", "turing seed pipeline");
            let step_pipeline = compute_pipeline("cs_step", "turing step pipeline");
            Stepper::Compute { seed: seed_pipeline, step: step_pipeline, bgl: compute_bgl }
        });

        // The mesh reads the field in the vertex stage — the height, the
        // normal and both colour ramps all come from the same texel, so
        // computing them per vertex and interpolating is both cheaper and
        // smoother than re-fetching per fragment.
        let render_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("turing render bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let render_pipeline = m3::build_pipeline(
            device,
            format,
            &shader,
            &render_bgl,
            "turing render pipeline",
            wgpu::PrimitiveTopology::TriangleList,
            None,
        );

        let stepper = compute.unwrap_or_else(|| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("turing step layout"),
                bind_group_layouts: &[Some(&render_bgl)],
                immediate_size: 0,
            });
            let pass = |entry: &str| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_full"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(entry),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: FIELD_FORMAT,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                })
            };
            Stepper::PingPong { seed: pass("fs_seed"), step: pass("fs_step") }
        });

        let make_field = || {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("turing field"),
                size: wgpu::Extent3d {
                    width: FIELD_SIZE,
                    height: FIELD_SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FIELD_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | if pingpong {
                        wgpu::TextureUsages::RENDER_ATTACHMENT
                    } else {
                        wgpu::TextureUsages::STORAGE_BINDING
                    },
                view_formats: &[],
            })
        };
        let field = [make_field(), make_field()];
        let storage_view = [
            field[0].create_view(&wgpu::TextureViewDescriptor::default()),
            field[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];
        let sampled_view = [
            field[0].create_view(&wgpu::TextureViewDescriptor::default()),
            field[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];

        let (index_buffer, index_count) = Self::build_indices(device);

        Gpu {
            stepper,
            render_pipeline,
            render_bgl,
            field,
            storage_view,
            sampled_view,
            index_buffer,
            index_count,
            depth: DepthCache::default(),
        }
    }

    /// Indices only: the vertex positions come from `vertex_index` in the
    /// shader, since a regular grid is entirely described by its size.
    fn build_indices(device: &wgpu::Device) -> (wgpu::Buffer, u32) {
        let n = MESH_N;
        let mut indices: Vec<u32> = Vec::with_capacity(((n - 1) * (n - 1) * 6) as usize);
        for z in 0..n - 1 {
            for x in 0..n - 1 {
                let a = z * n + x;
                let b = a + 1;
                let c = a + n;
                let d = c + 1;
                indices.extend([a, c, b, b, c, d]);
            }
        }
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("turing indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        (buffer, indices.len() as u32)
    }

    /// Whether to reseed, and how many steps to run.
    ///
    /// Reseeds when the seed float changed, when time went backwards, or when
    /// it jumped further than [`SEEK_GAP`] — the three cases where continuing
    /// from the field on hand would be a lie about what `time` means.
    ///
    /// Otherwise the count comes from *elapsed time*, not from the fact that
    /// a frame happened. That is the whole invariant: `paso` is steps per
    /// 1/60 s, so dt = 0 is zero steps and the same instant asked for twice
    /// returns the same field. The elapsed time is clamped before it is used
    /// so a stall arrives as a slowdown rather than a burst of catch-up.
    fn schedule(&mut self, time: f32, rate: f32, seed_key: u32) -> (bool, u32) {
        let reseed = match self.sim.wall {
            None => true,
            Some(prev) => {
                let dt = time - prev;
                seed_key != self.sim.seed_key || dt < 0.0 || dt > SEEK_GAP
            }
        };
        let dt = self.sim.wall.map_or(0.0, |prev| (time - prev).clamp(0.0, 0.1));
        self.sim.wall = Some(time);
        self.sim.seed_key = seed_key;

        if reseed {
            self.sim.carry = 0.0;
            return (true, BURN_IN);
        }

        // Fractional rates still advance: the remainder is carried instead of
        // rounded away, so a slow `paso` steps every few frames rather than
        // never.
        self.sim.carry += rate * dt * 60.0;
        let whole = self.sim.carry.floor();
        self.sim.carry -= whole;
        (false, (whole.max(0.0) as u32).min(MAX_STEPS_PER_FRAME))
    }
}

impl Model for Turing {
    fn id(&self) -> &'static str {
        "turing"
    }

    fn name(&self) -> &'static str {
        "Turing"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    /// Turing is the pilot for `the design notes` §13.2 — it already had
    /// a real 3D `camera(...)` block and a `view_proj` in `render()` below,
    /// so wiring `cam.track` in costs three floats and an offset add rather
    /// than inventing a viewpoint from nothing.
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
        let rate = floats[4].max(0.0);
        let seed_key = (floats[16].clamp(0.0, 1.0) * 1000.0).round() as u32;
        let (reseed, steps) = self.schedule(ctx.time, rate, seed_key);

        // camera_track (§13.2): cam_x/cam_y/cam_z are a view-offset a
        // `cam.track` node may drive, added to this model's own resting
        // `pos` — never to `target` or `fov`, so a `cam.track` pans and
        // dollies the viewpoint without ever changing what it is aimed at.
        // At the schema default `[0,0,0]` this is `cam[0..3] + 0.0`,
        // byte-identical to the camera before this capability existed.
        let mut cam = [0.0f32; 7];
        cam.copy_from_slice(&floats[41..48]);
        let track = &floats[48..51];
        cam[0] += track[0];
        cam[1] += track[1];
        cam[2] += track[2];
        let uniform = Uniform {
            pattern_color: ColorSlotGpu::from_floats(&floats[17..29]),
            crease_color: ColorSlotGpu::from_floats(&floats[29..41]),
            view_proj: CameraGpu::from_floats(&cam).view_proj(ctx.aspect()).to_cols_array_2d(),
            f0: [floats[0], floats[1], floats[2], floats[3]],
            f1: [floats[6], floats[7], floats[8], floats[5]],
            f2: [floats[9], floats[10], floats[11], floats[12]],
            f3: [floats[13], floats[14], floats[15], ctx.time],
            f4: [floats[16], FIELD_SIZE as f32, MESH_N as f32, 0.0],
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("turing uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let pingpong = self.force_pingpong || !crate::gpu::device_has_compute(ctx.device);
        let gpu = self.gpu.get(format, || Self::build(ctx.device, format, pingpong));

        // One step bind group per direction of the ping-pong: reading half
        // `from`, writing the other (a storage view for compute, a render
        // target for the fragment path).
        let step_bg: Vec<wgpu::BindGroup> = (0..2)
            .map(|from| match &gpu.stepper {
                Stepper::PingPong { .. } => ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("turing step bind group"),
                    layout: &gpu.render_bgl,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&gpu.sampled_view[from]),
                        },
                    ],
                }),
                Stepper::Compute { bgl, .. } => {
                ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("turing compute bind group"),
                    layout: bgl,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(
                                &gpu.storage_view[1 - from],
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&gpu.sampled_view[from]),
                        },
                    ],
                })
                }
            })
            .collect();

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("turing encoder"),
        });
        let groups = FIELD_SIZE.div_ceil(8);

        // One pass per step rather than one pass with many dispatches: the
        // pair swaps roles between steps, and a fresh pass is the plain way
        // to say "the writes from the last one are visible now". `from` is
        // the half read; the other is written.
        let run = |encoder: &mut wgpu::CommandEncoder, seed: bool, from: usize| match &gpu.stepper {
            Stepper::Compute { seed: sp, step: tp, .. } => {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("turing compute pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(if seed { sp } else { tp });
                pass.set_bind_group(0, &step_bg[from], &[]);
                pass.dispatch_workgroups(groups, groups, 1);
            }
            Stepper::PingPong { seed: sp, step: tp } => {
                let target = gpu.sampled_view[1 - from].clone();
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("turing step pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target,
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
                pass.set_pipeline(if seed { sp } else { tp });
                pass.set_bind_group(0, &step_bg[from], &[]);
                pass.draw(0..3, 0..1);
            }
        };

        if reseed {
            // Seeding writes field 0, so it runs on the direction whose
            // destination is field 0 — the one that reads field 1, which it
            // does not touch.
            run(&mut encoder, true, 1);
            self.sim.cur = 0;
        }
        for _ in 0..steps {
            run(&mut encoder, false, self.sim.cur);
            self.sim.cur = 1 - self.sim.cur;
        }

        let render_bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("turing render bind group"),
            layout: &gpu.render_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&gpu.sampled_view[self.sim.cur]),
                },
            ],
        });
        let depth_view = gpu.depth.get(ctx.device, ctx.width, ctx.height);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("turing render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
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
            pass.set_pipeline(&gpu.render_pipeline);
            pass.set_bind_group(0, &render_bg, &[]);
            pass.set_index_buffer(gpu.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..gpu.index_count, 0, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
        let _ = &gpu.field; // the views borrow from these; keep them named
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::scaffold::testing::frame;

    /// The fragment ping-pong (what a WebGL2 device runs) draws the same
    /// frame as the compute path on the same adapter: same seed, same burn-in,
    /// same steps, both through `seed_value` / `step_value`.
    #[test]
    fn the_pingpong_path_matches_the_compute_frame() {
        let gpu = crate::gpu::Gpu::new_headless();
        if !crate::gpu::device_has_compute(&gpu.device) {
            eprintln!("no compute here: only the ping-pong path exists on this adapter");
            return;
        }
        let f = Turing::new().schema().defaults();
        let times = [0.3, 0.8, 1.4];
        let a = frame(&gpu, &mut Turing::new(), &f, &times);
        let b = frame(&gpu, &mut Turing::pingpong(), &f, &times);
        let diff: Vec<u8> = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).collect();
        let mean = diff.iter().map(|d| *d as f32).sum::<f32>() / diff.len() as f32;
        let max = diff.iter().copied().max().unwrap_or(0);
        println!("turing parity: mean {mean:.4} max {max}");
        assert!(a.iter().any(|v| *v > 8), "the frame is not black");
        assert!(mean < 1.0 && max < 48, "ping-pong drifted from compute: mean {mean:.3} max {max}");
    }

    /// The ping-pong path is a pure function of its time like the compute one:
    /// the same instant twice is the same frame.
    #[test]
    fn the_pingpong_path_is_deterministic() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = Turing::new().schema().defaults();
        let a = frame(&gpu, &mut Turing::pingpong(), &f, &[0.3, 0.8]);
        let b = frame(&gpu, &mut Turing::pingpong(), &f, &[0.3, 0.8]);
        assert_eq!(a, b);
    }
}
