//! Model 29 — Terrarium (`terrarium`). A glass box being filled from above by up
//! to ten mouths of coloured sand, with the sand piling up in it.
//!
//! *"A terrarium that fills with sand from above, with gravity physics so the
//! piles grow and interact between streams of different colours; the same
//! things Sandstorm defines, but the source points down from above and in
//! smaller mouths, to make patterns with colours, and up to 10 sources raining
//! at once."*
//!
//! # Why this is not Sandstorm with a floor
//!
//! Sandstorm has no CPU side and no state: its grains are a closed form, a
//! pixel's height inverted into an age. That works because nothing in it ever
//! *lands*. A pile cannot be written that way — where a grain comes to rest is
//! a function of every grain that arrived before it, and no amount of algebra
//! turns a history into a pixel's coordinates. So the two models share a
//! vocabulary and share nothing else: this one keeps a grid of cells between
//! frames and steps it, which makes it the second model in the suite with
//! state, after Turing.
//!
//! It keeps Turing's invariant along with the state, because that invariant is
//! what a preset, the render gate and the automix router all depend on: **the
//! simulation advances when time advances**. Zero elapsed time is zero steps,
//! so asking for the same instant twice gives the same frame; a jump larger
//! than [`SEEK_GAP`] is a seek and empties the box.
//!
//! # Ten mouths, not ten mouths' worth of sliders
//!
//! Ten sources with a position, a width, a rate and a colour each would be
//! forty floats, forty sliders on the dash, and forty things a live signal
//! could point at. Nobody would use them and nothing could drive them. So the
//! sources are *derived*: `fuentes` says how many, `reparto` how far apart
//! they stand, `origen_x` where the array is centred, `ancho` how wide each
//! mouth is, `vaiven`/`vaiven_vel`/`desfase` how they sweep, and `matiz` how
//! far apart their colours sit in the `arena` gradient. Eight floats do what
//! forty would have done worse — and every one of them means something when a
//! band moves it, which forty per-source floats would not.
//!
//! # What the CPU does here
//!
//! Almost nothing, and deliberately: the scheduler, the wind of the moment,
//! and [`settle`] — the block rule, written a second time so it can be tested.
//! `shaders/terrario.wgsl` has the argument for the rule itself, which is the
//! only interesting thing in the model.

use wgpu::util::DeviceExt;

use crate::model::{color_at, background, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema, SurfaceSpec};
use crate::shared::surface::{MaterialU, PartSet};
use crate::shared::ColorSlotGpu;

/// The one part a material can be laid on: the sand surface in the box.
const SURFACES: &[&str] = &["body"];

const FLOATS: &[FloatSpec] = &[
    // How many mouths are pouring. Pinned against live drive: a source
    // appearing and disappearing eight times a second is not ten streams of
    // sand, it is a stream that stutters, and the operator chose the number.
    spec_held("sources", 1.0, 10.0, 5.0, 0.0),
    // Where the array of mouths is centred, and how far it spreads. Both in
    // the box's own width, which runs -1..1 whatever the wall's aspect is.
    spec("origin_x", -1.0, 1.0, 0.0),
    spec("distribution", 0.0, 1.9, 1.35),
    // One mouth. Small on purpose — the ask is *smaller mouths*, and it is
    // what separates this from Sandstorm's curtain: a narrow mouth builds a
    // cone you can see the shape of, a wide one builds a flat layer.
    spec("width", 0.004, 0.30, 0.035),
    // How much of each mouth's opening is actually pouring, per step. At 1 the
    // stream is solid; low values give a scatter of individual grains.
    spec("flow", 0.0, 1.0, 0.65),
    // The mouths sweeping side to side. `desfase` is how much of a cycle apart
    // consecutive mouths are: at 0 all ten swing as one bar, which draws
    // stripes; away from 0 they cross each other and draw weaves.
    spec("swing", 0.0, 1.0, 0.45),
    spec("swing_speed", 0.0, 3.0, 0.30),
    spec("shift", 0.0, 1.0, 0.35),
    // Gravity, in cells fallen per second — which is the same thing as steps
    // of the automaton per second, since a step moves a falling grain one
    // cell. 0 freezes the box without emptying it.
    spec("gravity", 0.0, 600.0, 240.0),
    // Sideways, in the same two terms Sandstorm uses: a steady lean and a
    // gust that comes and goes. Both bias which way a grain slides off a pile,
    // so the piles lean too rather than only the falling sand.
    spec("drift", -1.0, 1.0, 0.0),
    spec("wind", 0.0, 1.0, 0.0),
    // The angle of repose, as a probability — see the shader. This is the
    // float that decides whether the box fills with towers or with dunes, and
    // it is the one worth putting a band on.
    spec("rest", 0.0, 1.0, 0.45),
    // Shaking the box. Old piles slump instead of standing for ever.
    spec("vibration", 0.0, 1.0, 0.25),
    // The hole in the floor. At 0 the box fills and the picture stops
    // changing, which is a legal thing to want and a terrible default: with a
    // drain the model reaches a steady state it can hold all night.
    spec("drain", 0.0, 1.0, 0.55),
    // How much touching grains bleed each other's colour. The seam between two
    // streams is the whole point of pouring two colours.
    spec("mix", 0.0, 1.0, 0.20),
    // Per-grain tonal jitter, so a pile has a grain to it rather than reading
    // as a flat region of one colour.
    spec("vein", 0.0, 1.0, 0.35),
    // How far apart the mouths' colours sit in `arena`. At 0 every mouth
    // pours the same colour and the box is monochrome; at 1 the mouths span
    // the whole gradient.
    spec("hue", 0.0, 1.0, 1.0),
    // The box in the frame, and how present its glass is.
    spec("box", 0.30, 1.0, 0.92),
    spec("frame", 0.0, 1.0, 0.45),
    // Lights the sand that is moving and leaves the sand that has settled
    // alone, which is what makes a stream read as a stream.
    spec("shine", 0.0, 1.0, 0.22),
    // Pegs standing in the box for the streams to break over. Structural: they
    // are where the sand goes, so a signal sweeping them would rearrange the
    // furniture several times a second.
    spec_held("obstacles", 0.0, 1.0, 0.35, 0.10),
    // Changing it re-deals the pegs and empties the box. Pinned for Turing's
    // reason: driven live, the model would spend its life in burn-in.
    spec_held("seed", 0.0, 1.0, 0.0, 0.0),
];

/// Not white, and not Sandstorm's desert either. Four stops that are four
/// clearly different sands is what makes ten mouths read as ten colours — the
/// gradient is being used as a palette here, not as a shading ramp, because
/// `matiz` samples it at ten separated points.
const SAND: [f32; 12] = [
    0.96, 0.72, 0.12, //
    0.88, 0.19, 0.16, //
    0.13, 0.42, 0.92, //
    0.24, 0.78, 0.34,
];

/// Glass, seen edge on: nearly nothing at the bottom, a cold sheen at the top.
const GLASS: [f32; 12] = [
    0.40, 0.62, 0.68, //
    0.55, 0.78, 0.82, //
    0.70, 0.88, 0.92, //
    0.86, 0.96, 1.00,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("sand", &SAND),
        color_at("crystal", &GLASS),
        background(),
    crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
    crate::model::EFFECTS_COLOR_BLOCK,
],
};

/// The grid's cell budget (the old fixed 512x288). Its shape follows the
/// viewport, see [`grid_dims`]; its area stays here so a tall or a wide preview
/// costs what a 16:9 one does.
const GRID_CELLS: f32 = 512.0 * 288.0;
const GRID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The grid for a viewport of this aspect: the same number of cells, shaped
/// like the viewport so the box fills it with no letterbox. The aspect is
/// snapped to a ~6% ladder (log steps) so dragging a window edge does not
/// reseed the sand on every pixel; both sides are even so the 2x2 blocks tile.
pub fn grid_dims(aspect: f32) -> (u32, u32) {
    let a = aspect.clamp(0.2, 6.0);
    let snapped = ((a.ln() / 0.06).round() * 0.06).exp();
    let even = |v: f32| ((v / 2.0).round() as u32).max(8) * 2;
    (even((GRID_CELLS * snapped).sqrt()), even((GRID_CELLS / snapped).sqrt()))
}

/// Steps run when the box is emptied, so the first frame an operator sees is
/// already a box with sand falling into a pile rather than an empty box that
/// takes five seconds to become interesting. Slightly more than the grid's
/// height, which is what it costs for the first grain to reach the floor.
const BURN_IN: u32 = 420;

/// Cap on steps in one frame. A dropped frame becomes a slowdown rather than a
/// jump, and a window that was paused for a minute does not resume by running
/// fifteen thousand steps in one go.
const MAX_STEPS_PER_FRAME: u32 = 24;

/// A forward jump larger than this is a seek and empties the box. A minute for
/// the reason Turing's is a minute: automix parks a model for the length of
/// three switches, and a box that emptied every time it came back on air would
/// never be the same box twice.
const SEEK_GAP: f32 = 60.0;

// ---------------------------------------------------------------------------
// the block rule, on the CPU too
// ---------------------------------------------------------------------------

/// One 2x2 Margolus block, resolved. Returns `map`, where `map[k]` is which of
/// the block's four cells ends up in slot `k` — slots being `0 = TL, 1 = TR,
/// 2 = BL, 3 = BR`.
///
/// It is here as well as in the shader because it is the piece of this model
/// that can be wrong while everything stays in range: a rule that is not a
/// permutation duplicates or destroys sand, and sand appearing out of nothing
/// looks exactly like a pour rate that is slightly too high. Nothing
/// downstream can tell those apart — not the render gate, which only asks for
/// "not black, no NaN, deterministic", and not an operator, who has never seen
/// the correct version. The tests at the bottom of this file are the only
/// place that difference is ever checked.
///
/// `bias` is the wind, -1 (left) to 1 (right). `r` is three independent
/// 0..1 numbers; the shader passes a hash of the block's own coordinates and
/// the step index, which is what makes the sand grainy rather than crystalline.
pub fn settle(
    occ: [bool; 4],
    sun: [bool; 4],
    repose: f32,
    bias: f32,
    vibration: f32,
    r: [f32; 3],
) -> [usize; 4] {
    let mut map = [0usize, 1, 2, 3];
    let mut o = occ;

    // 1. Straight down.
    for c in 0..2 {
        let (top, bot) = (c, c + 2);
        if o[top] && !o[bot] && !sun[top] && !sun[bot] {
            map.swap(top, bot);
            o[top] = false;
            o[bot] = true;
        }
    }

    // 2. Off the shoulder of the pile.
    let slide = (repose * 0.9 + vibration * 0.3).clamp(0.0, 1.0);
    let right = (slide * (0.5 + 0.5 * bias)).clamp(0.0, 1.0);
    let left = (slide * (0.5 - 0.5 * bias)).clamp(0.0, 1.0);
    if o[0] && o[2] && !o[3] && !sun[0] && !sun[3] && r[0] < right {
        map.swap(0, 3);
        o[0] = false;
        o[3] = true;
    }
    if o[1] && o[3] && !o[2] && !sun[1] && !sun[2] && r[1] < left {
        map.swap(1, 2);
        o[1] = false;
        o[2] = true;
    }

    // 3. Shuffling along the floor.
    let shake = (vibration * 0.25 + bias.abs() * 0.18).clamp(0.0, 1.0);
    if !sun[2] && !sun[3] {
        if o[2] && !o[3] && r[2] < shake * (0.5 + 0.5 * bias) {
            map.swap(2, 3);
        } else if o[3] && !o[2] && (1.0 - r[2]) < shake * (0.5 - 0.5 * bias) {
            map.swap(3, 2);
        }
    }

    map
}

/// The wind of the moment: the steady lean plus a gust that comes and goes.
///
/// Two sines rather than noise so it is a closed form of the clock — the
/// grid's history depends on this, and a value that came from anywhere else
/// would be one more thing a seek could not reproduce.
pub fn wind(time: f32, drift: f32, wind_amount: f32) -> f32 {
    let gust = (time * 0.37).sin() * 0.62 + (time * 0.131 + 1.7).sin() * 0.38;
    (drift + wind_amount * gust).clamp(-1.0, 1.0)
}

// ---------------------------------------------------------------------------
// the GPU side
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    sand: ColorSlotGpu,
    glass: ColorSlotGpu,
    background: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    e: [f32; 4],
    f: [f32; 4],
    /// `body`'s picture and `mat_*`, see `material.wgsl`.
    mat_body: MaterialU,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct StepUniform {
    s: [f32; 4],
}

/// Compute where the device has it; otherwise the same `make_block` /
/// `cell_out` drawn by a fragment pass per cell (WebGL2).
enum Stepper {
    Compute {
        seed: wgpu::ComputePipeline,
        step: wgpu::ComputePipeline,
        bgl: wgpu::BindGroupLayout,
    },
    PingPong {
        seed: wgpu::RenderPipeline,
        step: wgpu::RenderPipeline,
        bgl: wgpu::BindGroupLayout,
    },
}

struct Gpu {
    stepper: Stepper,
    render_pipeline: wgpu::RenderPipeline,
    render_bgl: wgpu::BindGroupLayout,
    /// Ping-pong pair, for the reason Turing has one: a step reads every cell
    /// of the grid and writes every cell of the grid, and one texture cannot
    /// be both inside a dispatch.
    grid: [wgpu::Texture; 2],
    dims: (u32, u32),
    storage_view: [wgpu::TextureView; 2],
    sampled_view: [wgpu::TextureView; 2],
}

#[derive(Default)]
struct Sim {
    /// Which of the pair currently holds the grid.
    cur: usize,
    /// The time of the last call. `None` means nothing has been seeded.
    wall: Option<f32>,
    /// Fractional steps owed from earlier frames, so a slow `gravedad` still
    /// advances instead of rounding to zero for ever.
    carry: f32,
    /// Counts every step ever run. It is the hash salt, so it is also what
    /// stops two steps of the same frame pouring the same grains.
    steps: u32,
    seed_key: u32,
}

pub struct Terrario {
    gpu: Built<Gpu>,
    sim: Sim,
    surfaces: PartSet<1>,
    /// Step with the fragment ping-pong even where compute exists (the parity
    /// test runs both on one adapter).
    force_pingpong: bool,
}

impl Terrario {
    pub fn new() -> Terrario {
        Terrario {
            gpu: Built::default(),
            sim: Sim::default(),
            surfaces: PartSet::new(SURFACES),
            force_pingpong: false,
        }
    }

    #[cfg(test)]
    fn pingpong() -> Terrario {
        Terrario { force_pingpong: true, ..Terrario::new() }
    }

    fn build(device: &wgpu::Device, format: wgpu::TextureFormat, pingpong: bool) -> Gpu {
        use crate::shared::sim::{with_compute, without_compute};
        let body = include_str!("../../shaders/terrario.wgsl");
        let body = if pingpong {
            format!("{}\n{}", without_compute(body), include_str!("../../shaders/terrario_pp.wgsl"))
        } else {
            with_compute(body)
        };
        let module_wgsl = format!(
            "{}\n{}\n{}",
            include_str!("../../shaders/surface.wgsl"),
            include_str!("../../shaders/material.wgsl"),
            body,
        );
        let shader = crate::shared::build_shader(device, "terrarium", &module_wgsl);

        let compute = (!pingpong).then(|| {
            let compute_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("terrario compute bgl"),
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
                            format: GRID_FORMAT,
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
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
            let compute_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("terrario compute layout"),
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
            let seed_pipeline = compute_pipeline("cs_seed", "terrario seed pipeline");
            let step_pipeline = compute_pipeline("cs_step", "terrario step pipeline");
            Stepper::Compute { seed: seed_pipeline, step: step_pipeline, bgl: compute_bgl }
        });

        // The frame is one triangle reading the grid with `textureLoad`, not a
        // mesh and not a sampler: a grain has to land on whole cells, and
        // anything filtered would smear the box into fog at exactly the moment
        // the pile shape became visible.
        let render_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrario render bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // The `body` part's picture, at 4/5 so it does not collide with
                // the compute pass's bindings (0..=3), which share this module.
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let render_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrario render layout"),
            bind_group_layouts: &[Some(&render_bgl)],
            immediate_size: 0,
        });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrario render pipeline"),
            layout: Some(&render_layout),
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

        let stepper = compute.unwrap_or_else(|| {
            let entry = |binding: u32, ty: wgpu::BindingType| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty,
                count: None,
            };
            let uniform = wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            };
            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("terrario step bgl"),
                entries: &[
                    entry(0, uniform),
                    entry(
                        2,
                        wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                    ),
                    entry(3, uniform),
                ],
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("terrario step layout"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });
            let pass = |fs: &str| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(fs),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_main"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(fs),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: GRID_FORMAT,
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
            Stepper::PingPong { seed: pass("fs_seed"), step: pass("fs_step"), bgl }
        });

        let dims = grid_dims(16.0 / 9.0);
        let (grid, storage_view, sampled_view) = Self::make_grids(device, dims, pingpong);

        Gpu {
            stepper,
            render_pipeline,
            render_bgl,
            grid,
            dims,
            storage_view,
            sampled_view,
        }
    }

    fn make_grids(
        device: &wgpu::Device,
        (w, h): (u32, u32),
        pingpong: bool,
    ) -> ([wgpu::Texture; 2], [wgpu::TextureView; 2], [wgpu::TextureView; 2]) {
        let make_grid = || {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("terrario grid"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: GRID_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | if pingpong {
                        wgpu::TextureUsages::RENDER_ATTACHMENT
                    } else {
                        wgpu::TextureUsages::STORAGE_BINDING
                    },
                view_formats: &[],
            })
        };
        let grid = [make_grid(), make_grid()];
        let views = || {
            [
                grid[0].create_view(&wgpu::TextureViewDescriptor::default()),
                grid[1].create_view(&wgpu::TextureViewDescriptor::default()),
            ]
        };
        let (storage, sampled) = (views(), views());
        (grid, storage, sampled)
    }

    /// Whether to empty the box, and how many steps to run — Turing's
    /// scheduler, and the same argument holds for it here. The count comes
    /// from *elapsed time*, never from the fact that a frame happened, which
    /// is the whole reason this stateful model still satisfies the render
    /// gate's determinism check.
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

        self.sim.carry += rate * dt;
        let whole = self.sim.carry.floor();
        self.sim.carry -= whole;
        (false, (whole.max(0.0) as u32).min(MAX_STEPS_PER_FRAME))
    }
}

impl Model for Terrario {
    fn id(&self) -> &'static str {
        "terrarium"
    }

    fn name(&self) -> &'static str {
        "Terrarium"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn is_3d(&self) -> bool {
        true
    }

    fn surfaces(&self) -> &'static [&'static str] {
        self.surfaces.surfaces()
    }

    fn set_surface(&mut self, part: &str, spec: Option<SurfaceSpec>) {
        self.surfaces.set(part, spec);
    }

    fn render(
        &mut self,
        ctx: &Ctx,
        floats: &[f32],
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        let rate = floats[8].max(0.0);
        // The pegs are part of the box's shape, so changing them is a reseed
        // in the same way the seed is: leaving the old sand where a new
        // obstacle now stands would bury grains that can never move again.
        let seed_key = (floats[21].clamp(0.0, 1.0) * 1000.0).round() as u32 * 2003
            + (floats[20].clamp(0.0, 1.0) * 100.0).round() as u32;
        // The grid follows the viewport's aspect; a new shape is a new box.
        let dims = grid_dims(ctx.aspect());
        let pingpong = self.force_pingpong || !crate::gpu::device_has_compute(ctx.device);
        {
            let gpu = self.gpu.get(format, || Self::build(ctx.device, format, pingpong));
            if gpu.dims != dims {
                let (grid, storage_view, sampled_view) = Self::make_grids(ctx.device, dims, pingpong);
                gpu.grid = grid;
                gpu.storage_view = storage_view;
                gpu.sampled_view = sampled_view;
                gpu.dims = dims;
                self.sim.wall = None;
            }
        }
        let (grid_w, grid_h) = dims;
        let (reseed, steps) = self.schedule(ctx.time, rate, seed_key);
        let bias = wind(ctx.time, floats[9], floats[10]);

        self.surfaces.sync(ctx.device, ctx.queue);
        let uniform = Uniform {
            sand: ColorSlotGpu::from_floats(&floats[22..34]),
            glass: ColorSlotGpu::from_floats(&floats[34..46]),
            background: ColorSlotGpu::from_floats(&floats[46..58]),
            a: [ctx.aspect(), ctx.time, grid_w as f32, grid_h as f32],
            b: [floats[0], floats[1], floats[2], floats[3]],
            c: [floats[4], floats[5], floats[6], floats[7]],
            d: [bias, floats[11], floats[12], floats[13]],
            e: [floats[14], floats[15], floats[16], floats[17]],
            f: [floats[18], floats[19], floats[20], floats[21]],
            mat_body: self.surfaces.uniform(0),
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrario uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let gpu = self.gpu.get(format, || Self::build(ctx.device, format, pingpong));

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("terrario encoder"),
        });

        // One bind group per step rather than two for the frame, because each
        // step carries its own clock and its own salt — see `StepUniform` in
        // the shader. They are eight tiny allocations at the cap and the
        // alternative is every step of a frame pouring identical grains.
        let mut bind_groups = Vec::with_capacity(steps as usize + 1);
        let make_bg = |from: usize, s: StepUniform| {
            let sbuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("terrario step uniform"),
                contents: bytemuck::bytes_of(&s),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let (layout, pp) = match &gpu.stepper {
                Stepper::Compute { bgl, .. } => (bgl, false),
                Stepper::PingPong { bgl, .. } => (bgl, true),
            };
            if pp {
                return ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("terrario step bind group"),
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&gpu.sampled_view[from]),
                        },
                        wgpu::BindGroupEntry { binding: 3, resource: sbuf.as_entire_binding() },
                    ],
                });
            }
            ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("terrario compute bind group"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&gpu.storage_view[1 - from]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&gpu.sampled_view[from]),
                    },
                    wgpu::BindGroupEntry { binding: 3, resource: sbuf.as_entire_binding() },
                ],
            })
        };

        if reseed {
            self.sim.steps = 0;
            // Seeding writes grid 0, so it runs on the bind group whose
            // destination is grid 0 — the one that reads grid 1 and does not
            // touch it.
            bind_groups.push(make_bg(1, StepUniform { s: [ctx.time, 0.0, 0.0, 0.0] }));
        }
        // The burn-in's steps are dated backwards from now at the current rate,
        // so the sand it pours has swept where it would have swept had the
        // model been running all along — rather than every one of the 420
        // steps pouring from the same instant, which is one static stripe.
        let dt_step = 1.0 / rate.max(1.0);
        let mut cur = if reseed { 0 } else { self.sim.cur };
        for i in 0..steps {
            let age = (steps - i) as f32 * dt_step;
            let s = StepUniform {
                s: [
                    ctx.time - age,
                    (self.sim.steps.wrapping_add(i) % 4096) as f32,
                    ((self.sim.steps.wrapping_add(i)) % 2) as f32,
                    0.0,
                ],
            };
            bind_groups.push(make_bg(cur, s));
            cur = 1 - cur;
        }

        {
            let groups_x = (grid_w / 2 + 1).div_ceil(8);
            let groups_y = (grid_h / 2 + 1).div_ceil(8);
            let seed_groups_x = grid_w.div_ceil(8);
            let seed_groups_y = grid_h.div_ceil(8);
            let mut next = 0usize;
            // One pass per step rather than one pass with many dispatches: the
            // pair swaps roles between steps, and a fresh pass is the plain way
            // to say the previous one's writes are visible now. `to` is the
            // half written (only the fragment path needs it here).
            let mut dispatch = |seed: bool, to: usize, x: u32, y: u32| {
                match &gpu.stepper {
                    Stepper::Compute { seed: sp, step: tp, .. } => {
                        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                            label: Some("terrario compute pass"),
                            timestamp_writes: None,
                        });
                        pass.set_pipeline(if seed { sp } else { tp });
                        pass.set_bind_group(0, &bind_groups[next], &[]);
                        pass.dispatch_workgroups(x, y, 1);
                    }
                    Stepper::PingPong { seed: sp, step: tp, .. } => {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("terrario step pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &gpu.sampled_view[to],
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
                        pass.set_bind_group(0, &bind_groups[next], &[]);
                        pass.draw(0..3, 0..1);
                    }
                }
                next += 1;
            };
            if reseed {
                dispatch(true, 0, seed_groups_x, seed_groups_y);
                self.sim.cur = 0;
            }
            for _ in 0..steps {
                dispatch(false, 1 - self.sim.cur, groups_x, groups_y);
                self.sim.cur = 1 - self.sim.cur;
            }
        }
        self.sim.steps = self.sim.steps.wrapping_add(steps);

        let render_bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrario render bind group"),
            layout: &gpu.render_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&gpu.sampled_view[self.sim.cur]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(self.surfaces.slot(0).view()),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(self.surfaces.slot(0).sampler()),
                },
            ],
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terrario render pass"),
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
            pass.set_pipeline(&gpu.render_pipeline);
            pass.set_bind_group(0, &render_bg, &[]);
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
        let _ = &gpu.grid; // the views borrow from these; keep them named
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> Vec<f32> {
        SCHEMA.defaults()
    }

    /// Every occupancy of a block, every wall arrangement, and both ends of
    /// each random draw. 16 x 16 x 8 = 2048 cases, which is the whole input
    /// space of the rule up to the granularity of `r`.
    fn every_case(mut check: impl FnMut([bool; 4], [bool; 4], [f32; 3], [usize; 4])) {
        let bits = |m: usize| [m & 1 != 0, m & 2 != 0, m & 4 != 0, m & 8 != 0];
        for occ_bits in 0..16 {
            for sol_bits in 0..16 {
                let sun = bits(sol_bits);
                // A wall never holds a grain: the shader reads solid cells as
                // empty before it calls this.
                let occ: [bool; 4] = {
                    let o = bits(occ_bits);
                    [o[0] && !sun[0], o[1] && !sun[1], o[2] && !sun[2], o[3] && !sun[3]]
                };
                for rr in 0..8 {
                    let r = [
                        if rr & 1 != 0 { 0.99 } else { 0.01 },
                        if rr & 2 != 0 { 0.99 } else { 0.01 },
                        if rr & 4 != 0 { 0.99 } else { 0.01 },
                    ];
                    check(occ, sun, r, settle(occ, sun, 1.0, 0.0, 1.0, r));
                }
            }
        }
    }

    #[test]
    fn the_schema_and_the_slices_render_reads_are_the_same_width() {
        // 22 floats + three colour slots. `render` slices `[22..34]`,
        // `[34..46]` and `[46..58]` by hand.
        assert_eq!(SCHEMA.width(), 58 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 58 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    #[test]
    fn a_step_never_creates_or_destroys_a_grain() {
        // The failure the whole block rule exists to make impossible, and the
        // one nothing downstream can see: a rule that is not a permutation
        // duplicates sand, and duplicated sand just looks like more sand.
        every_case(|occ, sun, r, map| {
            let mut seen = [false; 4];
            for k in 0..4 {
                assert!(!seen[map[k]], "slot {k} is a second copy of cell {} ({occ:?} {sun:?} {r:?})", map[k]);
                seen[map[k]] = true;
            }
        });
    }

    #[test]
    fn a_wall_is_never_moved_and_never_moved_into() {
        // A grain permuted into a wall is a grain that has left the terrarium,
        // and one permuted out of a wall is sand pouring out of the glass.
        every_case(|occ, sun, r, map| {
            for k in 0..4 {
                if sun[k] {
                    assert_eq!(map[k], k, "wall at {k} was permuted ({sun:?} {r:?})");
                }
                if occ[map[k]] && sun[k] {
                    assert!(false, "a grain landed in the wall at {k} ({occ:?} {sun:?})");
                }
            }
        });
    }

    #[test]
    fn no_grain_ever_rises() {
        // There is no upward rule in `settle`, and there must not be: gravity
        // in this model is entirely the absence of one. A sign flipped in the
        // slide rules would give sand climbing its own pile, which is in range,
        // deterministic, and unmistakably wrong only to a human watching it.
        every_case(|occ, _sol, r, map| {
            for slot in 0..4 {
                if !occ[map[slot]] {
                    continue;
                }
                let from_row = map[slot] / 2;
                let to_row = slot / 2;
                assert!(to_row >= from_row, "a grain rose from row {from_row} to {to_row} ({occ:?} {r:?})");
            }
        });
    }

    #[test]
    fn a_grain_with_a_hole_under_it_falls_into_it() {
        // The other direction: the rule has to actually do something. Checked
        // with the slides turned off, so this is the vertical rule alone.
        let none = [false; 4];
        let map = settle([true, false, false, false], none, 0.0, 0.0, 0.0, [0.99; 3]);
        assert_eq!(map[2], 0, "the grain in TL did not fall to BL");
        assert_eq!(map[0], 2, "the hole in BL did not rise to TL");
    }

    #[test]
    fn a_grain_on_a_pile_slides_off_it_and_the_wind_picks_the_side() {
        // TL is a grain resting on the grain in BL, with BR empty. At full
        // repose it slides; at zero repose it stays. That pair is the angle of
        // repose, and it is the only thing giving a pile a slope.
        let none = [false; 4];
        let piled = [true, false, true, false];
        assert_eq!(settle(piled, none, 1.0, 0.0, 0.0, [0.01, 0.01, 0.99])[3], 0, "no slide at reposo 1");
        assert_eq!(settle(piled, none, 0.0, 0.0, 0.0, [0.01, 0.01, 0.99])[3], 3, "slid at reposo 0");
        // Wind blowing left must not push a grain to the right.
        assert_eq!(
            settle(piled, none, 1.0, -1.0, 0.0, [0.4, 0.4, 0.99])[3],
            3,
            "a leftward wind slid a grain rightwards"
        );
    }

    #[test]
    fn the_floor_holds_the_sand_up() {
        // The floor is "the cells below the grid are solid". If a full bottom
        // row ever permuted downward the box would drain through its own base.
        let floor = [false, false, true, true];
        let sitting = [true, true, false, false];
        // Bottom row is wall, so nothing may move at all.
        assert_eq!(settle(sitting, floor, 1.0, 0.0, 1.0, [0.01; 3]), [0, 1, 2, 3]);
    }

    #[test]
    fn the_grid_takes_the_viewports_shape_with_the_same_cell_budget() {
        for a in [16.0 / 9.0, 4.0 / 3.0, 1.0, 9.0 / 16.0, 21.0 / 9.0] {
            let (w, h) = grid_dims(a);
            assert!(w % 2 == 0 && h % 2 == 0, "{a}: odd grid {w}x{h}");
            let shape = w as f32 / h as f32;
            assert!((shape / a - 1.0).abs() < 0.08, "{a}: grid {w}x{h} is shaped {shape}");
            let cells = (w * h) as f32;
            assert!((cells / GRID_CELLS - 1.0).abs() < 0.06, "{a}: {cells} cells");
        }
    }

    #[test]
    fn the_default_box_reaches_a_steady_state_instead_of_filling_or_emptying() {
        // Not a rule of the automaton, a rule of the defaults, and the one
        // that decides whether this model can be left on air for an hour.
        //
        // The drain only removes grains that are *there*, so it runs at its
        // full rate only once the floor is covered. That gives the two ends
        // this checks. If the pour beats a covered floor's drain, the box
        // fills to the brim and the picture stops moving; if the pour is a
        // trickle next to it, the floor never covers and there is nothing to
        // make a pattern out of. Both render, both are deterministic, and both
        // are a model nobody would put on a wall.
        //
        // The pour lays down `fuentes` mouths of `ancho` at `caudal`, in cells
        // of the grid's width; a covered floor drains `drenaje * 0.14` of the
        // bottom row. Both are per step, so `gravedad` cancels.
        let f = row();
        let poured = f[0] * (f[3] * 0.5 * 512.0_f32) * f[4];
        let drained = 512.0_f32 * f[13] * 0.14;
        // 1.25 rather than something rounder: the equilibrium coverage of the
        // floor is `poured / drained`, so this is "the floor settles at four
        // fifths covered or less". Nearer to 1 than that and the box is full
        // in every way that matters to someone looking at it.
        assert!(
            drained > poured * 1.25,
            "the default box fills up: it pours {poured:.1} cells a step and a \
             covered floor only drains {drained:.1}"
        );
        assert!(
            poured > drained * 0.25,
            "the default box never covers its floor: {poured:.1} cells a step \
             against a drain of {drained:.1}"
        );
    }

    #[test]
    fn the_wind_stays_inside_the_slide_probabilities() {
        // `settle` multiplies the slide chance by `0.5 +- 0.5 * bias`. A bias
        // outside -1..1 would make one side's probability negative and the
        // other's exceed the slide chance — sand that slides one way at a rate
        // no float in the schema asked for.
        for i in 0..400 {
            let t = i as f32 * 0.37;
            for &(d, v) in &[(-1.0f32, 1.0f32), (1.0, 1.0), (0.0, 1.0), (-1.0, 0.0)] {
                let b = wind(t, d, v);
                assert!(b.is_finite() && (-1.0..=1.0).contains(&b), "wind({t}, {d}, {v}) = {b}");
            }
        }
    }

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

    /// Every kind is a surface: at `mat_mix` 1 it changes the frame, and
    /// clearing `body` restores the frame byte for byte. The clock is held at
    /// one instant, so the box is the same grid for every draw.
    #[test]
    fn a_body_material_changes_the_frame_and_clearing_it_restores_it() {
        use crate::model::{MaterialSpec, SurfaceSpec};
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (128u32, 128u32);
        let floats = SCHEMA.defaults();
        let (texture, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let mut model = Terrario::new();
        assert_eq!(model.surfaces(), &["body"]);
        let draw = |model: &mut Terrario| {
            let ctx = Ctx {
                device: &gpu.device,
                queue: &gpu.queue,
                time: 1.0,
                width: w,
                height: h,
                fondo_alpha: false, detail: 1.0,
            };
            model.render(&ctx, &floats, &view, FORMAT);
            crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &texture, w, h)
        };
        // The first draw burns in; every later draw at time 1.0 steps nothing.
        let _ = draw(&mut model);
        let plain = draw(&mut model);
        let kinds = ["matte", "concrete", "wood", "leather", "metal", "glass", "matrix"];
        for (kind, name) in kinds.iter().enumerate() {
            model.set_surface(
                "body",
                Some(SurfaceSpec {
                    image: None,
                    material: Some(MaterialSpec {
                        kind: kind as f32,
                        mix: 1.0,
                        scale: 1.0,
                        ..Default::default()
                    }),
                }),
            );
            let painted = draw(&mut model);
            assert_ne!(plain, painted, "{name} did not change the frame");
            model.set_surface("body", None);
            let restored = draw(&mut model);
            assert_eq!(plain, restored, "clearing {name} did not restore the frame");
        }
    }

    /// The fragment ping-pong (what a WebGL2 device runs) draws the same frame
    /// as the compute path on the same adapter.
    #[test]
    fn the_pingpong_path_matches_the_compute_frame() {
        use crate::shared::scaffold::testing::frame;
        let gpu = crate::gpu::Gpu::new_headless();
        if !crate::gpu::device_has_compute(&gpu.device) {
            eprintln!("no compute here: only the ping-pong path exists on this adapter");
            return;
        }
        let f = row();
        let times = [0.3, 0.8, 1.4];
        let a = frame(&gpu, &mut Terrario::new(), &f, &times);
        let b = frame(&gpu, &mut Terrario::pingpong(), &f, &times);
        let diff: Vec<u8> = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).collect();
        let mean = diff.iter().map(|d| *d as f32).sum::<f32>() / diff.len() as f32;
        let max = diff.iter().copied().max().unwrap_or(0);
        println!("terrario parity: mean {mean:.4} max {max}");
        assert!(a.iter().any(|v| *v > 8), "the frame is not black");
        assert!(mean < 0.5, "ping-pong drifted from compute: mean {mean:.3} max {max}");
    }
}
