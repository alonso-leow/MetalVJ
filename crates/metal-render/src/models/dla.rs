//! Model 36 — Dla. Diffusion-limited aggregation: the video's algorithm
//! exactly — points appear on a ring at random angles, random-walk inwards,
//! and the ones that touch the structure freeze where they landed while the
//! rest are discarded and re-enter at a new angle. The aggregate that survives
//! is the picture: a coral that branches because its tips catch walkers
//! first, one rule with no data structure behind it.
//!
//! **The second model that keeps state, on purpose — Turing's argument, not
//! a copy of its plumbing.** An aggregate cannot be re-derived from t=0 any
//! more than a reaction can: its whole content is the accumulation. So it
//! follows the precedent `models::turing` set rather than the suite's pure
//! `(floats, time)` rule: the simulation advances when time advances, the
//! same instant asked for twice steps nothing and returns the same image
//! (the render gate still passes), a fresh instance reseeds, and a rewind or
//! a seek beyond [`SEEK_GAP`] starts a new coral rather than integrating
//! backwards into nonsense.
//!
//! Where it deliberately *differs* from Turing: the GPU state — occupancy
//! grid, cell metadata, walkers — lives in the model, not inside the
//! per-format cache, so a second consumer at another target format (the
//! preview server's `Rgba8UnormSrgb` beside the window's `Rgba16Float`)
//! rebuilds pipelines and keeps the coral. Turing's field lives in its
//! `Built` and pays a reseed for that switch; there was no reason to inherit
//! the bill with the pattern.
//!
//! # What is honest about re-running
//!
//! Within one frame everything is deterministic — the render gate compares
//! two renders of the same instant, the second advances nothing, and gets
//! the same bytes. Across *runs*, two walkers can reach the same cell in the
//! same step, and which one's jitter lands is up to the scheduler. A preset
//! re-run grows a cousin of the same coral — same seed, same rules, same
//! character, not the same fingerprint. The shape was never going to
//! reproduce byte-for-byte without serializing the walkers, and the point
//! of the model is that it grows live, not that it replays.

use wgpu::util::DeviceExt;

use crate::model::{color_at, background, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::{build_shader, ColorSlotGpu};

/// One triangle covering the frame, handing `fs_main` a 0..1 `uv` — the same
/// vertex half every fullscreen model uses, prepended ahead of this model's
/// WGSL at build time.
use crate::shared::fullscreen::VERTEX;

const FLOATS: &[FloatSpec] = &[
    // Walker steps per 1/60 s. 0 freezes the coral where it is without
    // freezing the sparks on top of it. Driven: growth rate is the model's
    // most expressive axis — the music literally feeds the walk.
    spec("step", 0.0, 40.0, 14.0),
    // What fraction of the walker ring is alive, 0..1 of MAX_WALKERS. A
    // driven float and a safe one: fewer walkers is a sparser, hungrier
    // coral, never a broken one.
    spec("walkers", 0.0, 1.0, 0.5),
    // A steady push on every step, so a signal can bend the whole growth
    // sideways while the walk stays random underneath.
    spec("drift", 0.0, 1.0, 0.0),
    // Which way the push points, 0..1 as a full turn.
    spec("drift_direction", 0.0, 1.0, 0.0),
    // A swirl instead of a push: tangential bias proportional to how far
    // off-axis a walker is, which curls the coral into a spiral. Negative
    // winds the other way.
    spec("turn", -1.0, 1.0, 0.0),
    // Step length in cells. Held: it sets how fine-grained the walk is
    // against the grid, and a signal sweeping it jitters the branch spacing
    // for no expressive gain.
    spec_held("stride", 0.5, 2.0, 1.0, 0.25),
    // The spawn ring's radius in world units, and the kill ring 15% beyond
    // it. Held, not pinned: it is structural (how far the coral has to grow
    // before it is done), but a show that wants the structure big has to be
    // able to reach for it.
    spec_held("radius", 0.9, 2.0, 1.35, 0.3),
    // Bead size in cells. Driven: the most beat-friendly float in the row —
    // the whole coral pulses thick and thin with the digest.
    spec("size", 0.3, 2.0, 1.0),
    // How bright the beads are.
    spec("light", 0.0, 1.0, 0.6),
    // How far the bloom reaches around each bead, so tips read as glowing
    // without any per-tip bookkeeping.
    spec("shine", 0.0, 1.0, 0.35),
    // How bright the walkers themselves show — the points flying in and
    // dying on the ring. 0 turns the model into the aggregate alone.
    spec("spark", 0.0, 1.0, 0.55),
    // The whole world spins, slowly, render-side only — the aggregate never
    // moves in its own frame, so a preset stays a preset however it turns.
    spec("rotation", 0.0, 1.0, 0.05),
    // Changing this reseeds: a different coral, same rules. Pinned, for
    // Turing's reason exactly — a driven reseed spends its life in burn-in
    // and never shows a structure at all.
    spec_held("seed", 0.0, 1.0, 0.0, 0.0),
];

/// Ice on glass: a near-black navy core rising up through blue and cyan to
/// a white-hot tip, which is the ramp that makes fresh growth read as the
/// lit part of the picture. Linear values — dark on paper, right on a
/// target that encodes sRGB on write (see Stained Glass's `GLASS` for the same
/// calibration note).
const CORAL: [f32; 12] = [
    0.010, 0.012, 0.050, //
    0.020, 0.060, 0.30, //
    0.10, 0.45, 0.85, //
    0.85, 0.95, 1.00,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("coral", &CORAL),
        background(),
        crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

// -- constants, mirrored in shaders/dla.wgsl ---------------------------------

/// Grid resolution. 256 is where a cell is one bead of the coral at frame
/// scale — finer wastes occupancy on empty space, coarser and the branches
/// read as chunky pixels.
const GRID: u32 = 256;
const CELLS: u32 = GRID * GRID;
/// The walker ring's budget. 1024 makes the sparks a cloud rather than a
/// queue while one dispatch stays a single-figure workgroup count.
const MAX_WALKERS: u32 = 1024;
const WALKER_TEX_W: u32 = 64;
const WALKER_TEX_H: u32 = 16;
/// Half-size of the world square the grid covers: it must swallow the kill
/// ring (`radio`'s 2.0 top times the 1.15 kill factor = 2.3) whole, or a
/// dying walker would spend its last steps clamped onto the edge cells.
/// 2.4 covers it with margin — see [`Self::the_kill_ring_stays_on_the_grid`].
const FIELD: f32 = 2.4;
/// Steps run at seed time so the first frame already has a coral rather
/// than a lone dot. A walker needs ~R²/step² steps to diffuse from the ring
/// to the middle — about 5,000 at the default `radio` — so 4,000 burn-in
/// steps land a few dozen beads: a structure that reads as DLA from frame
/// one, not so much that the show starts over. Measured, not guessed: at
/// 900 the first frame was a 9-pixel speck.
const BURN_IN: u32 = 4000;
/// Cap on steps in any one frame — a dropped frame arrives as a slowdown,
/// not a visible jump (Turing's rule, same reason).
const MAX_STEPS_PER_FRAME: u32 = 40;
/// A forward jump larger than this is a seek and reseeds. A minute, again
/// for the router: automix rotates a model off air for three switches and
/// back, and the coral that returns must be the one that left.
const SEEK_GAP: f32 = 60.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    coral: ColorSlotGpu,
    background: ColorSlotGpu,
    // aspect, time, rotacion, tam
    f0: [f32; 4],
    // luz, brillo, chispa, active walkers
    f1: [f32; 4],
    // age_base, deriva, deriva_dir, giro
    f2: [f32; 4],
    // zancada, radio, cell_world, seed_key
    f3: [f32; 4],
}

/// Every GPU resource that does not depend on the target format: the
/// simulation buffers, the compute pipelines, and the bind group layouts.
/// Built once, on the first frame, and kept for the model's life — which is
/// the whole point of it not living inside [`Built`].
struct Core {
    occupancy: wgpu::Buffer,
    meta: wgpu::Buffer,
    walkers: wgpu::Buffer,
    walker_tex: wgpu::Texture,
    walker_storage_view: wgpu::TextureView,
    walker_sampled_view: wgpu::TextureView,
    sim_bgl: wgpu::BindGroupLayout,
    bake_bgl: wgpu::BindGroupLayout,
    field_bgl: wgpu::BindGroupLayout,
    sparks_bgl: wgpu::BindGroupLayout,
    seed_cells: wgpu::ComputePipeline,
    seed_walkers: wgpu::ComputePipeline,
    step: wgpu::ComputePipeline,
    bake: wgpu::ComputePipeline,
}

/// The two render pipelines, keyed on target format by [`Built`].
struct Gpu {
    field_pipeline: wgpu::RenderPipeline,
    sparks_pipeline: wgpu::RenderPipeline,
}

#[derive(Default)]
struct Sim {
    /// The time of the last call. `None` means nothing has been seeded yet.
    wall: Option<f32>,
    /// Fractional steps owed from previous frames, so a slow `paso` still
    /// advances instead of rounding to zero for ever.
    carry: f32,
    seed_key: u32,
    /// The global step counter — the aggregate's clock, and the age written
    /// into a cell when a walker sticks in it.
    steps_done: u32,
}

pub struct Dla {
    /// `None` until the first frame; building needs a device and there is
    /// no GPU at build time (`metal-dash`'s stand build names models without
    /// rendering them).
    core: Option<Core>,
    render: Built<Gpu>,
    sim: Sim,
}

impl Dla {
    pub fn new() -> Dla {
        Dla { core: None, render: Built::default(), sim: Sim::default() }
    }

    fn build_core(device: &wgpu::Device) -> Core {
        let shader = build_shader(
            device,
            "dla",
            &format!("{VERTEX}\n{}", include_str!("../../shaders/dla.wgsl")),
        );

        let storage = wgpu::BufferUsages::STORAGE;
        let occupancy = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dla occupancy"),
            size: (CELLS * 4) as u64,
            usage: storage,
            mapped_at_creation: false,
        });
        let meta = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dla meta"),
            size: (CELLS * 16) as u64,
            usage: storage,
            mapped_at_creation: false,
        });
        // Walkers are seeded on the GPU before anything reads them — the
        // first `render` call always reseeds (`Sim::wall` starts `None`), so
        // the undefined contents of a fresh storage buffer are never drawn.
        let walkers = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dla walkers"),
            size: (MAX_WALKERS * 16) as u64,
            usage: storage,
            mapped_at_creation: false,
        });

        let walker_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("dla walker tex"),
            size: wgpu::Extent3d { width: WALKER_TEX_W, height: WALKER_TEX_H, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let walker_storage_view = walker_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let walker_sampled_view = walker_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let uniform_entry = |binding: u32, visibility: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage_entry = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };

        let sim_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dla sim bgl"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::COMPUTE),
                storage_entry(1, false),
                storage_entry(2, false),
                storage_entry(3, false),
            ],
        });
        let bake_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dla bake bgl"),
            entries: &[
                storage_entry(5, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let field_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dla field bgl"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let sparks_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dla sparks bgl"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let compute = |layout: &wgpu::PipelineLayout, entry: &str, label: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let sim_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dla sim layout"),
            bind_group_layouts: &[Some(&sim_bgl)],
            immediate_size: 0,
        });
        let bake_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dla bake layout"),
            bind_group_layouts: &[Some(&bake_bgl)],
            immediate_size: 0,
        });

        Core {
            occupancy,
            meta,
            walkers,
            walker_tex,
            walker_storage_view,
            walker_sampled_view,
            sim_bgl,
            bake_bgl,
            field_bgl,
            sparks_bgl,
            seed_cells: compute(&sim_layout, "cs_seed_cells", "dla seed cells"),
            seed_walkers: compute(&sim_layout, "cs_seed_walkers", "dla seed walkers"),
            step: compute(&sim_layout, "cs_step", "dla step"),
            bake: compute(&bake_layout, "cs_bake_walkers", "dla bake walkers"),
        }
    }

    fn build_render(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        core: &Core,
    ) -> Gpu {
        let shader = build_shader(
            device,
            "dla",
            &format!("{VERTEX}\n{}", include_str!("../../shaders/dla.wgsl")),
        );
        let field_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dla field layout"),
            bind_group_layouts: &[Some(&core.field_bgl)],
            immediate_size: 0,
        });
        let field_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dla field pipeline"),
            layout: Some(&field_layout),
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

        let sparks_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dla sparks layout"),
            bind_group_layouts: &[Some(&core.sparks_bgl)],
            immediate_size: 0,
        });
        let sparks_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dla sparks pipeline"),
            layout: Some(&sparks_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sparks"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sparks"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // Additive: sparks land on the coral as light, and a
                    // spark that crosses a bright bead adds to it rather
                    // than punching a hole through it.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
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

        Gpu { field_pipeline, sparks_pipeline }
    }

    /// Whether to reseed, and how many steps to run — Turing's schedule,
    /// verbatim in shape: reseed on a new seed, on a rewind, or on a seek
    /// beyond [`SEEK_GAP`]; otherwise advance by elapsed time with the
    /// remainder carried, clamped so a stall arrives as a slowdown.
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
            self.sim.steps_done = 0;
            return (true, BURN_IN);
        }

        self.sim.carry += rate * dt * 60.0;
        let whole = self.sim.carry.floor();
        self.sim.carry -= whole;
        (false, (whole.max(0.0) as u32).min(MAX_STEPS_PER_FRAME))
    }
}

impl Model for Dla {
    fn needs_compute(&self) -> bool {
        true
    }

    fn id(&self) -> &'static str {
        "dla"
    }

    fn name(&self) -> &'static str {
        "Dla"
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
        let rate = floats[0].max(0.0);
        let seed_key = (floats[12].clamp(0.0, 1.0) * 1000.0).round() as u32;
        let (reseed, steps) = self.schedule(ctx.time, rate, seed_key);
        let active = (floats[1].clamp(0.0, 1.0) * MAX_WALKERS as f32).round();
        let cell_world = 2.0 * FIELD / GRID as f32;

        let uniform = Uniform {
            coral: ColorSlotGpu::from_floats(&floats[13..25]),
            background: ColorSlotGpu::from_floats(&floats[25..37]),
            f0: [ctx.aspect(), ctx.time, floats[11], floats[7]],
            f1: [floats[8], floats[9], floats[10], active],
            f2: [self.sim.steps_done as f32 + 1.0, floats[2], floats[3], floats[4]],
            f3: [floats[5], floats[6], cell_world, seed_key as f32],
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dla uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let core = self.core.get_or_insert_with(|| Self::build_core(ctx.device));
        let gpu = self.render.get(format, || Self::build_render(ctx.device, format, core));

        let sim_bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dla sim bind group"),
            layout: &core.sim_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: core.occupancy.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: core.meta.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: core.walkers.as_entire_binding() },
            ],
        });
        let bake_bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dla bake bind group"),
            layout: &core.bake_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 5, resource: core.walkers.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&core.walker_storage_view),
                },
            ],
        });
        let field_bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dla field bind group"),
            layout: &core.field_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: core.occupancy.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 8, resource: core.meta.as_entire_binding() },
            ],
        });
        let sparks_bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dla sparks bind group"),
            layout: &core.sparks_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(&core.walker_sampled_view),
                },
            ],
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("dla encoder"),
        });

        // One pass per dispatch, Turing's caution exactly: a fresh pass is
        // the plain way to say "the writes from the last one are visible
        // now", and at 40 steps the extra encodes are noise.
        let mut dispatch = |pipeline: &wgpu::ComputePipeline, bind: &wgpu::BindGroup, groups: u32| {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("dla compute pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.dispatch_workgroups(groups, 1, 1);
        };

        if reseed {
            dispatch(&core.seed_cells, &sim_bg, CELLS.div_ceil(64));
            dispatch(&core.seed_walkers, &sim_bg, MAX_WALKERS.div_ceil(64));
        }
        for _ in 0..steps {
            dispatch(&core.step, &sim_bg, MAX_WALKERS.div_ceil(64));
        }
        self.sim.steps_done += steps;
        // The bake runs every frame, steps or not: the sparks are a readout
        // of walker state, so a frame that advanced nothing draws the same
        // sparks and the render gate's determinism check holds.
        dispatch(&core.bake, &bake_bg, MAX_WALKERS.div_ceil(64));

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("dla render pass"),
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
            pass.set_pipeline(&gpu.field_pipeline);
            pass.set_bind_group(0, &field_bg, &[]);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&gpu.sparks_pipeline);
            pass.set_bind_group(0, &sparks_bg, &[]);
            pass.draw(0..6, 0..MAX_WALKERS);
        }
        ctx.queue.submit(Some(encoder.finish()));
        let _ = &core.walker_tex; // the views borrow from this; keep it named
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_and_the_slices_render_reads_are_the_same_width() {
        // 13 floats + coral + fondo. `render` slices `[13..25]` and
        // `[25..37]` by hand, and indexes `floats[0]`..`floats[12]`.
        assert_eq!(FLOATS.len(), 13);
        assert_eq!(
            SCHEMA.width(),
            37 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH
        );
        assert_eq!(
            SCHEMA.defaults().len(),
            37 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH
        );
    }

    #[test]
    fn the_kill_ring_stays_on_the_grid() {
        // The furthest a walker may wander before being discarded is
        // `radio`'s top times the shader's 1.15 kill factor, and the grid
        // has to cover that whole circle or a dying walker would clamp onto
        // the edge cells and could stick to the frame itself. Checked at
        // the worst corner of the range, not at the default.
        assert!(2.0 * 1.15 < FIELD, "the kill ring escapes the field the grid covers");
    }
}
