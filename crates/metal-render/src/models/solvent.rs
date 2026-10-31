//! Model 10 — Solvent. A field of soft drops, blurred into one another and
//! grained, read through two four-stop palettes.
//!
//! # Why this was rewritten
//!
//! The first Solvent advanced *fronts*: for each of up to four liquids, a
//! distance from one edge of the frame compared against a noise-perturbed
//! reach, so the liquid arrived across the picture like a tide. It was a
//! reasonable reading of "physics of liquid in 2D, seen from above" and it
//! produced a bad image, for three reasons that were all the same reason —
//! **nothing in it had a shape**.
//!
//! - A front is a *boundary*, and a boundary drawn by `smoothstep` over a
//!   noise field has no interior. The frame was two flat washes meeting along
//!   a wobble.
//! - The liquids combined with `max` and composited with `+`. Neither is
//!   mixing. Two liquids overlapping got brighter, not blended, and three of
//!   them clipped to white.
//! - The field was 256×256 read with `textureLoad`, which is nearest
//!   neighbour, stretched across a 1920-wide output. Every edge in the frame
//!   was a staircase of 7-pixel steps.
//!
//! The rework keeps the subject — a combination of materials — and changes
//! what the picture is *made of*: **drops at coordinates**. A drop is a place,
//! a size and a lifetime, and a body of liquid is what you get when enough of
//! them overlap. That gives an interior for free (the sum of the weights),
//! gives mixing for free (the weights are a partition), and it is the thing
//! the ask asked for.
//!
//! # The three layers
//!
//! 1. **Drops** ([`drops`], on the CPU). Everything about a drop is closed
//!    form in `(floats, time)` — see the note on `SLOT_PERIOD` for the one
//!    part of that which took thought. Solving them here rather than in the
//!    shader costs a 2 KB upload and buys the whole system a unit test.
//! 2. **The field** (`cs_accumulate`, then two blur passes). Per texel and per
//!    material, the sum of the drop weights and the sum of weight×tone. The
//!    quotient is the place along that material's gradient, weighted by how
//!    much of each drop is here — so where two drops of one material overlap,
//!    the colour is *between* their two colours rather than one of them.
//!    Blurring is linear and the quotient is taken afterwards, so blurring the
//!    two sums separately blurs the average correctly.
//! 3. **The frame** (`fs_main`). Resolve the two tones, mix the materials by
//!    weight, composite `fondo`, grain.
//!
//! # Grain is not decoration here
//!
//! `granulado` is in the ask, and it also fixes something. A wide smooth
//! gradient quantised to 8 bits bands — the eye finds the 1/255 steps in a
//! ramp that crosses the frame, and this model is nothing *but* such ramps.
//! Noise at ±1 LSB destroys the banding completely, because it turns a step
//! into a dither. So the grain floor is not zero even at `granulado = 0`: a
//! sixth of a code value, which is invisible as texture and sufficient as
//! dither. Above that it is the ask's film grain.

use wgpu::util::DeviceExt;

use crate::model::{color_at, background, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::ColorSlotGpu;

/// Drop slots. The uniform carries this many whatever `gotas` says, and the
/// shader loops over all of them — a dead slot has zero weight and costs one
/// multiply. Raising it means raising the array in `solvent.wgsl` too.
pub const MAX_DROPS: usize = 64;

/// How long one slot's cycle is, in seconds, before its own jitter.
///
/// **This constant is why `duracion` is a fraction and not a number of
/// seconds**, which is worth the paragraph because the obvious design is
/// wrong. If a drop's lifetime *were* its cycle, then `floor(time / lifetime)`
/// is which drop the slot is currently holding, and changing the lifetime
/// changes that index — every drop in the frame is replaced by a different
/// one, at a different place, in a single frame. Under a slider that is a
/// jarring shuffle; under the digest, which moves floats every frame, it is a
/// permanent seizure.
///
/// So the *period* is fixed per slot and `duracion` is how much of it the drop
/// is present for. Turning it up and down fades drops in and out where they
/// already are, which is what the operator meant, and no value of it changes
/// which drop is which.
const SLOT_PERIOD: f32 = 9.0;

/// The field the drops are accumulated into, per side. Square, sampled over
/// the frame, and deliberately far below output resolution: it is upsampled
/// with a linear sampler, which is the cheapest correct blur in the pipeline,
/// and everything drawn into it is soft enough that the interpolation has
/// nothing to reveal.
const FIELD_SIZE: u32 = 256;
const FIELD_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Half-width of the blur kernel, in field texels. Fixed, so the loop is a
/// constant trip count; `difuminado` moves the gaussian's sigma inside it.
/// Sixteen taps a pass, two passes, over 65 k texels — under a fiftieth of the
/// work the same blur would cost at output resolution.
const BLUR_TAPS: i32 = 8;

const FLOATS: &[FloatSpec] = &[
    // What appears. The defaults are deliberately crowded: this model reads as
    // liquid when the drops overlap and as a lava lamp when they do not, and
    // the first pass at them opened on twelve separate circles on black.
    spec("drops", 1.0, MAX_DROPS as f32, 40.0),
    spec("size", 0.03, 1.00, 0.55),
    // How much of its slot's period a drop is present for. Not seconds — see
    // `SLOT_PERIOD`. At 1.0 the drops never leave and the field only drifts.
    spec("duration", 0.05, 1.0, 0.80),
    // Where. The ask's *"they appear according to coordinates"*: one coordinate
    // the liquid wells up at, and how far from it the drops scatter. Driving
    // the centre with the music moves the source of the liquid around the
    // frame, which is the most musical thing in the model.
    spec("center_x", -1.0, 1.0, 0.0),
    spec("center_y", -1.0, 1.0, 0.0),
    spec("dispersion", 0.0, 1.0, 0.90),
    // How it moves. `deriva` is per drop and incoherent — drops wander apart.
    // `agitacion` warps the whole field at once — the body of liquid sloshes.
    // Two floats because those are two different pictures.
    spec("drift", 0.0, 1.0, 0.30),
    spec("agitation", 0.0, 1.0, 0.30),
    spec("hit", 0.0, 1.0, 0.0),
    // How it looks. Viscosity is the contrast on the coverage: thin liquid
    // fades out over a wide band and reads as haze, thick liquid holds a
    // definite surface and reads as a lava lamp.
    spec("viscosity", 0.0, 1.0, 0.35),
    spec("smudge", 0.0, 1.0, 0.55),
    spec("granular", 0.0, 1.0, 0.35),
    // Which material a drop is dealt. 0 is all A, 1 is all B, and the
    // interesting values are in between — this is the ask's *combination*.
    spec_held("mix", 0.0, 1.0, 0.5, 0.6),
];

/// Two materials that are different from each other on the first frame.
///
/// Four white stops in both slots would draw the whole model in one colour,
/// and a model whose subject is *how two materials combine* cannot open unable
/// to show the combination. A cold one and a warm one, each a gradient in
/// itself so a single drop already has depth.
///
/// Saturated the whole way along, including the light end. A stop near white
/// looks like a highlight in a swatch and like nothing at all here: drops
/// average, so a pale stop pulls every overlap it touches toward grey, and the
/// first version of these — which ended in a near-white — produced a frame
/// that was pastel everywhere the liquid was thick.
const LIQUID_A: [f32; 12] = [
    0.02, 0.16, 0.55, //
    0.05, 0.45, 0.92, //
    0.10, 0.80, 0.95, //
    0.45, 0.98, 0.82,
];
const LIQUID_B: [f32; 12] = [
    0.52, 0.02, 0.30, //
    0.95, 0.10, 0.34, //
    1.00, 0.44, 0.10, //
    1.00, 0.82, 0.18,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("liquid_a", &LIQUID_A),
        color_at("liquid_b", &LIQUID_B),
        background(),
    crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
    crate::model::EFFECTS_COLOR_BLOCK,
],
};

// ---------------------------------------------------------------------------
// the drops
// ---------------------------------------------------------------------------

fn hash(x: u32) -> f32 {
    let mut h = x.wrapping_add(0x9E37_79B9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    (h >> 8) as f32 / 16_777_216.0
}

/// A hashed value in ±1.
fn signed(x: u32) -> f32 {
    hash(x) * 2.0 - 1.0
}

/// One drop of liquid at one instant: where it is in aspect-corrected space,
/// how wide, how much it counts for, which material it is and where in that
/// material's gradient it sits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drop {
    pub at: [f32; 2],
    pub radius: f32,
    /// 0 when the drop is absent, 1 at the middle of its life. Both the life
    /// envelope and the `gotas` fade land here, because to the field there is
    /// no difference between a drop that is ending and a drop that is being
    /// turned off.
    pub weight: f32,
    /// 0..1 along its material's four stops.
    pub tone: f32,
    /// 0 for `liquido_a`, 1 for `liquido_b`.
    pub material: f32,
}

/// The float row, read once.
#[derive(Clone, Copy)]
struct Setup {
    count: f32,
    size: f32,
    duration: f32,
    centre: [f32; 2],
    spread: f32,
    drift: f32,
    mix: f32,
}

impl Setup {
    fn read(f: &[f32]) -> Setup {
        Setup {
            count: f[0].clamp(1.0, MAX_DROPS as f32),
            size: f[1].clamp(0.03, 1.00),
            duration: f[2].clamp(0.05, 1.0),
            centre: [f[3].clamp(-1.0, 1.0), f[4].clamp(-1.0, 1.0)],
            spread: f[5].clamp(0.0, 1.0),
            drift: f[6].clamp(0.0, 1.0),
            mix: f[12].clamp(0.0, 1.0),
        }
    }
}

/// How present a drop is, `0..1`, given how far through its presence it is.
///
/// Not a triangle and not a step: a drop that appears at full size is a pop,
/// and one that fades linearly ends with a visible corner. `sin²` is zero with
/// zero slope at both ends, which is the shape of something dissolving.
fn envelope(u: f32) -> f32 {
    if !(0.0..=1.0).contains(&u) {
        return 0.0;
    }
    let s = (std::f32::consts::PI * u).sin();
    s * s
}

/// Every drop in the frame at `time`.
///
/// Slot `i` runs on a fixed period and a fixed phase, so which drop the slot
/// is holding depends on the clock alone. Everything the floats touch is
/// *inside* one cycle: where the drop is dealt, how big, how long it stays.
/// That is what makes the whole model scrub, park and resume — there is no
/// state to lose and no parameter whose movement re-deals the frame.
///
/// `aspect` is width over height. Positions come back in the space the shader
/// works in: x in ±aspect, y in ±1, y up.
pub fn drops(floats: &[f32], time: f32, aspect: f32) -> Vec<Drop> {
    let s = Setup::read(floats);
    let aspect = aspect.max(0.01);
    let mut out = Vec::with_capacity(MAX_DROPS);

    // A fractional count fades the last slot rather than switching it on, so
    // `gotas` is continuous and the digest can swell the number of drops
    // without the frame flickering on the way.
    let whole = s.count.floor() as usize;
    let tail = s.count - whole as f32;

    for i in 0..MAX_DROPS {
        let slot = i as u32;
        let presence = if i < whole {
            1.0
        } else if i == whole {
            tail
        } else {
            0.0
        };
        if presence <= 0.0 {
            out.push(Drop { at: [0.0, 0.0], radius: 0.0, weight: 0.0, tone: 0.0, material: 0.0 });
            continue;
        }

        // Fixed per slot, and never touched by a float.
        let period = SLOT_PERIOD * (0.55 + 0.9 * hash(slot * 7 + 1));
        let phase = hash(slot * 13 + 2);
        let turn = time / period + phase;
        let cycle = turn.floor();
        let through = turn - cycle;

        // Which drop this is. Two slots that happen to share a cycle number
        // must not share an identity, hence the slot in the key.
        let id = (cycle as i64 as u32).wrapping_mul(0x9E37_79B9) ^ slot.wrapping_mul(0x85EB_CA6B);

        // Present for the first `duracion` of the period, then the slot is
        // empty until it comes round again. An empty stretch is not dead time:
        // it is what makes drops arrive rather than merely exist.
        let live = envelope(through / s.duration);
        if live <= 0.0 {
            out.push(Drop { at: [0.0, 0.0], radius: 0.0, weight: 0.0, tone: 0.0, material: 0.0 });
            continue;
        }

        // Where it was dealt. `dispersion` at 0 stacks every drop on the
        // centre, which is not a degenerate case but a useful one — one large
        // pulsing body instead of a field.
        let reach = s.spread;
        let dealt = [
            s.centre[0] * aspect + signed(id ^ 0x1111) * aspect * reach,
            s.centre[1] + signed(id ^ 0x2222) * reach,
        ];

        // Drift is measured from the start of the cycle, so a drop travels
        // once across its life rather than being teleported by the clock.
        let age = through * period;
        let angle = hash(id ^ 0x3333) * std::f32::consts::TAU;
        let speed = s.drift * (0.25 + 0.75 * hash(id ^ 0x4444)) * 0.10;
        let at = [dealt[0] + angle.cos() * speed * age, dealt[1] + angle.sin() * speed * age];

        // Radius grows a little across the life on top of the envelope, so a
        // drop spreads as it dissolves the way a drop of ink does.
        let radius = s.size * (0.45 + 0.85 * hash(id ^ 0x5555)) * (0.75 + 0.45 * through);

        // Which material. Hashed rather than alternating, because alternating
        // slots plus hashed positions still leaves the two materials
        // interleaved in a way the eye reads as a texture rather than a mix.
        let material = if hash(id ^ 0x6666) < s.mix { 1.0 } else { 0.0 };

        out.push(Drop {
            at,
            radius: radius.max(1e-4),
            weight: live * presence,
            tone: hash(id ^ 0x7777),
            material,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// the GPU side
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DropGpu {
    /// x, y, radius, weight
    place: [f32; 4],
    /// tone, material, unused, unused
    tint: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    liquid_a: ColorSlotGpu,
    liquid_b: ColorSlotGpu,
    /// The background the liquid sits on.
    ///
    /// In the uniform rather than under a `shared::backdrop` pass because this
    /// model's own quad covers every pixel and writes with
    /// `BlendState::REPLACE` — a pass underneath would be drawn and then
    /// erased. Compositing it here is also the more honest of the two: the
    /// background and the liquid end up in the same colour arithmetic, and
    /// with the liquid now *mixed* over the background rather than added to
    /// it, that arithmetic is where the two actually meet.
    background: ColorSlotGpu,
    /// aspect, time, field size, blur sigma in texels
    misc: [f32; 4],
    /// agitacion, golpe, viscosidad, granulado
    look: [f32; 4],
    /// The one thing the two blur dispatches disagree about: which way to walk
    /// (x, y) and what to scale the sigma by on that axis.
    sweep: [f32; 4],
    drops: [DropGpu; MAX_DROPS],
}

/// How the field is built. Compute where the device has it; otherwise the same
/// `accumulate_value` / `blur_value` drawn by fragment passes (WebGL2).
enum Stages {
    Compute {
        accumulate: wgpu::ComputePipeline,
        blur: wgpu::ComputePipeline,
        bgl: wgpu::BindGroupLayout,
    },
    PingPong {
        accumulate: wgpu::RenderPipeline,
        blur: wgpu::RenderPipeline,
    },
}

struct Gpu {
    stages: Stages,
    render_pipeline: wgpu::RenderPipeline,
    render_bgl: wgpu::BindGroupLayout,
    /// Ping and pong. `field` is what the render pass reads; `swap` is where
    /// the accumulation lands and where the horizontal blur reads from. A
    /// separable gaussian needs somewhere to put the half-blurred image, and
    /// the alternative — one texture read and written in the same dispatch —
    /// is not a thing a storage texture can do.
    field: wgpu::Texture,
    swap: wgpu::Texture,
    field_store: wgpu::TextureView,
    field_read: wgpu::TextureView,
    swap_store: wgpu::TextureView,
    swap_read: wgpu::TextureView,
    sampler: wgpu::Sampler,
}

pub struct Solvent {
    gpu: Built<Gpu>,
    /// Build the field with fragment passes even where compute exists (the
    /// parity test runs both on one adapter).
    force_pingpong: bool,
}

impl Solvent {
    pub fn new() -> Solvent {
        Solvent { gpu: Built::default(), force_pingpong: false }
    }

    #[cfg(test)]
    fn pingpong() -> Solvent {
        Solvent { force_pingpong: true, ..Solvent::new() }
    }

    fn build(device: &wgpu::Device, format: wgpu::TextureFormat, pingpong: bool) -> Gpu {
        use crate::shared::sim::{with_compute, without_compute};
        let src = include_str!("../../shaders/solvent.wgsl");
        let src = if pingpong {
            format!("{}\n{}", without_compute(src), include_str!("../../shaders/solvent_pp.wgsl"))
        } else {
            with_compute(src)
        };
        let shader = crate::shared::build_shader(device, "solvent", &src);

        let compute = (!pingpong).then(|| {
            // One layout for all three compute dispatches: uniform, a texture to
            // read, a texture to write. `cs_accumulate` ignores the one it reads.
            let compute_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("solvent compute bgl"),
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
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: FIELD_FORMAT,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                ],
            });
            let compute_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("solvent compute layout"),
                bind_group_layouts: &[Some(&compute_bgl)],
                immediate_size: 0,
            });
            let stage = |entry: &str, label: &str| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(&compute_layout),
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                })
            };
            let accumulate = stage("cs_accumulate", "solvent accumulate");
            let blur = stage("cs_blur", "solvent blur");
            Stages::Compute { accumulate, blur, bgl: compute_bgl }
        });

        let render_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("solvent render bgl"),
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
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let render_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("solvent render layout"),
            bind_group_layouts: &[Some(&render_bgl)],
            immediate_size: 0,
        });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("solvent render pipeline"),
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

        let stages = compute.unwrap_or_else(|| {
            let pass = |entry: &str| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&render_layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_main"),
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
            Stages::PingPong { accumulate: pass("fs_accumulate"), blur: pass("fs_blur") }
        });

        let make = |label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
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
        let field = make("solvent field");
        let swap = make("solvent swap");
        let field_store = field.create_view(&wgpu::TextureViewDescriptor::default());
        let field_read = field.create_view(&wgpu::TextureViewDescriptor::default());
        let swap_store = swap.create_view(&wgpu::TextureViewDescriptor::default());
        let swap_read = swap.create_view(&wgpu::TextureViewDescriptor::default());

        // Linear, and clamped rather than repeating: a drop near the edge must
        // not bleed round to the other side of the frame.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("solvent sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        Gpu {
            stages,
            render_pipeline,
            render_bgl,
            field,
            swap,
            field_store,
            field_read,
            swap_store,
            swap_read,
            sampler,
        }
    }
}

impl Model for Solvent {
    fn id(&self) -> &'static str {
        "solvent"
    }

    fn name(&self) -> &'static str {
        "Solvent"
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
        let aspect = ctx.aspect().max(0.01);
        let live = drops(floats, ctx.time, aspect);

        let mut table = [DropGpu { place: [0.0; 4], tint: [0.0; 4] }; MAX_DROPS];
        for (slot, d) in table.iter_mut().zip(live.iter()) {
            slot.place = [d.at[0], d.at[1], d.radius, d.weight];
            slot.tint = [d.tone, d.material, 0.0, 0.0];
        }

        // The kernel is a fixed sixteen taps a side; `difuminado` is the sigma
        // inside it. A sigma of a third of the window is where a gaussian has
        // spent its weight, so that is the top of the range.
        let sigma = 0.35 + floats[10].clamp(0.0, 1.0) * (BLUR_TAPS as f32 / 3.0);
        let base = Uniform {
            liquid_a: ColorSlotGpu::from_floats(&floats[13..25]),
            liquid_b: ColorSlotGpu::from_floats(&floats[25..37]),
            background: ColorSlotGpu::from_floats(&floats[37..49]),
            misc: [aspect, ctx.time, FIELD_SIZE as f32, sigma],
            look: [
                floats[7].clamp(0.0, 1.0),
                floats[8].clamp(0.0, 1.0),
                floats[9].clamp(0.0, 1.0),
                floats[11].clamp(0.0, 1.0),
            ],
            sweep: [0.0; 4],
            drops: table,
        };

        let pingpong = self.force_pingpong || !crate::gpu::device_has_compute(ctx.device);
        let gpu = self.gpu.get(format, || Self::build(ctx.device, format, pingpong));
        let _ = (&gpu.field, &gpu.swap); // keeps the textures alive alongside their views

        // Three uniforms rather than one, because the blur passes differ in
        // exactly one vec4 and a dynamic offset would be more plumbing than
        // 3 KB of scratch is worth.
        let upload = |sweep: [f32; 4], label: &'static str| {
            let mut u = base;
            u.sweep = sweep;
            ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::bytes_of(&u),
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        let u_accumulate = upload([0.0, 0.0, 0.0, 0.0], "solvent uniform");
        // The across pass walks x and shrinks its sigma by the aspect. A field
        // texel is `aspect` times wider on screen than it is tall, so a kernel
        // that is round in texels is an ellipse in the frame — the picture
        // comes out smeared sideways on any output that is not square.
        let u_across = upload([1.0, 0.0, 1.0 / aspect, 0.0], "solvent blur across");
        let u_down = upload([0.0, 1.0, 1.0, 0.0], "solvent blur down");

        let bind = |uniform: &wgpu::Buffer, read: &wgpu::TextureView, write: &wgpu::TextureView| match &gpu.stages {
            // The fragment passes read `read` and are drawn into `write`
            // outside the bind group; the layout is the render one.
            Stages::PingPong { .. } => ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("solvent pass bind group"),
                layout: &gpu.render_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(read) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&gpu.sampler) },
                ],
            }),
            Stages::Compute { bgl, .. } => {
            ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("solvent compute bind group"),
                layout: bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(read),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(write),
                    },
                ],
            })
            }
        };
        // swap ← drops, field ← blur across swap, swap ← blur down field, and
        // the render pass reads swap. Every dispatch reads one texture and
        // writes the other, which is the only arrangement a write-only storage
        // binding permits.
        let deal = bind(&u_accumulate, &gpu.field_read, &gpu.swap_store);
        let across = bind(&u_across, &gpu.swap_read, &gpu.field_store);
        let down = bind(&u_down, &gpu.field_read, &gpu.swap_store);

        let render_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("solvent render bind group"),
            layout: &gpu.render_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: u_accumulate.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&gpu.swap_read),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&gpu.sampler),
                },
            ],
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("solvent encoder"),
        });
        let groups = FIELD_SIZE.div_ceil(8);
        // Separate passes rather than three dispatches in one, so the
        // read-after-write between them is a pass boundary and not a question
        // about how a usage scope is tracked. `write` is only used by the
        // fragment path (a compute bind group already holds it).
        for (label, accumulate, group, write) in [
            ("solvent accumulate pass", true, &deal, &gpu.swap_store),
            ("solvent blur across pass", false, &across, &gpu.field_store),
            ("solvent blur down pass", false, &down, &gpu.swap_store),
        ] {
            match &gpu.stages {
                Stages::Compute { accumulate: ap, blur: bp, .. } => {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some(label),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(if accumulate { ap } else { bp });
                    pass.set_bind_group(0, group, &[]);
                    pass.dispatch_workgroups(groups, groups, 1);
                }
                Stages::PingPong { accumulate: ap, blur: bp } => {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some(label),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: write,
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
                    pass.set_pipeline(if accumulate { ap } else { bp });
                    pass.set_bind_group(0, group, &[]);
                    pass.draw(0..3, 0..1);
                }
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("solvent render pass"),
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
            pass.set_bind_group(0, &render_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> Vec<f32> {
        SCHEMA.defaults()
    }

    /// Every output in this suite is 16:9 or close enough that a test written
    /// against another number is testing something no projector shows.
    const WIDE: f32 = 16.0 / 9.0;

    fn alive(floats: &[f32], time: f32) -> Vec<Drop> {
        drops(floats, time, WIDE).into_iter().filter(|d| d.weight > 0.0).collect()
    }

    #[test]
    fn the_schema_and_the_slices_render_reads_are_the_same_width() {
        // 13 floats + three colour slots. `render` slices `[13..25]`,
        // `[25..37]` and `[37..49]` by hand.
        assert_eq!(SCHEMA.width(), 49 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 49 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    #[test]
    fn a_frame_is_a_pure_function_of_the_floats_and_the_clock() {
        // The property the whole suite is built on, and the one automix breaks
        // when a model integrates: parking this for thirty seconds and coming
        // back must land on the frame it would have landed on anyway.
        let f = row();
        for t in [0.0f32, 3.5, 41.0, 900.0] {
            assert_eq!(drops(&f, t, WIDE), drops(&f, t, WIDE), "t={t} did not repeat");
        }
    }

    #[test]
    fn changing_duration_never_moves_a_drop() {
        // Why `SLOT_PERIOD` is a constant. `duracion` may fade drops in and
        // out; if it also re-dealt them, every frame the digest touched it
        // would be a different picture.
        let mut short = row();
        short[2] = 0.15;
        let mut long = row();
        long[2] = 0.95;
        for t in [1.0f32, 7.3, 22.0, 61.5] {
            let a = drops(&short, t, WIDE);
            let b = drops(&long, t, WIDE);
            for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
                if x.weight > 0.0 && y.weight > 0.0 {
                    assert_eq!(x.at, y.at, "slot {i} moved when duracion changed, t={t}");
                    assert_eq!(x.tone, y.tone, "slot {i} recoloured when duracion changed");
                }
            }
        }
    }

    #[test]
    fn a_fractional_count_fades_the_last_slot_in_rather_than_switching_it_on() {
        // `gotas` has to be continuous for the same reason `duracion` does:
        // the digest moves it every frame, and a drop that switches on at full
        // weight is a pop in the frame on every transient.
        let mut f = row();
        f[0] = 9.0;
        // Slot 8 is only present for part of its period, so find a moment it
        // is actually in the frame rather than assuming one.
        let when = (0..400)
            .map(|k| k as f32 * 0.13)
            .find(|&t| drops(&f, t, WIDE)[8].weight > 0.05)
            .expect("slot 8 was never alive in 52 seconds");
        let full = drops(&f, when, WIDE)[8].weight;

        for tenth in 0..=10 {
            let part = tenth as f32 / 10.0;
            f[0] = 8.0 + part;
            let all = drops(&f, when, WIDE);
            assert!(
                (all[8].weight - full * part).abs() < 1e-6,
                "at gotas={} slot 8 weighed {} and should have weighed {}",
                f[0],
                all[8].weight,
                full * part
            );
            assert_eq!(all[9].weight, 0.0, "slot 9 is past the count and must be off");
        }
    }

    #[test]
    fn dispersion_at_zero_stacks_every_drop_on_the_coordinate() {
        // The ask's "according to coordinates": the centre is a real place, and
        // the spread is measured from it. At zero spread that is one body.
        let mut f = row();
        f[3] = 0.4;
        f[4] = -0.6;
        f[5] = 0.0;
        f[6] = 0.0; // no drift, or the drops walk away from the coordinate
        for d in alive(&f, 5.0) {
            assert!(
                (d.at[0] - 0.4 * WIDE).abs() < 1e-5 && (d.at[1] + 0.6).abs() < 1e-5,
                "a drop was dealt at {:?}, not on the coordinate",
                d.at
            );
        }
    }

    #[test]
    fn no_drop_is_ever_dealt_outside_a_frame_and_a_half() {
        // Drift is unbounded in principle — it is a speed times an age — so
        // the bound is the thing to check, across every slider that feeds it.
        for spread in [0.0f32, 0.5, 1.0] {
            for drift in [0.0f32, 0.5, 1.0] {
                let mut f = row();
                f[5] = spread;
                f[6] = drift;
                f[3] = 1.0;
                f[4] = 1.0;
                let mut t = 0.0;
                while t < 120.0 {
                    for d in alive(&f, t) {
                        assert!(
                            d.at[0].abs() < 4.0 && d.at[1].abs() < 3.0,
                            "a drop reached {:?} at t={t} (dispersion {spread}, deriva {drift})",
                            d.at
                        );
                    }
                    t += 1.7;
                }
            }
        }
    }

    #[test]
    fn mix_deals_the_materials_it_says_it_does() {
        // 0 is one material, 1 is the other, and the middle is a mix. Counted
        // over a long stretch, because a single frame holds 26 drops and a
        // hash is not a quota.
        let sample = |mix: f32| {
            let mut f = row();
            f[12] = mix;
            f[0] = MAX_DROPS as f32;
            let (mut b, mut all) = (0.0f32, 0.0f32);
            let mut t = 0.0;
            while t < 200.0 {
                for d in alive(&f, t) {
                    all += 1.0;
                    b += d.material;
                }
                t += 2.3;
            }
            b / all.max(1.0)
        };
        assert_eq!(sample(0.0), 0.0, "mezcla 0 dealt material B");
        assert_eq!(sample(1.0), 1.0, "mezcla 1 dealt material A");
        let half = sample(0.5);
        assert!((0.4..0.6).contains(&half), "mezcla 0.5 dealt {half} of material B");
    }

    #[test]
    fn a_drop_is_never_more_present_than_it_is_allowed_to_be() {
        // Weight is a coverage, and the shader treats it as one. A weight over
        // 1 would blow past the palette and clip.
        let mut f = row();
        f[0] = MAX_DROPS as f32;
        f[2] = 1.0;
        let mut t = 0.0;
        while t < 90.0 {
            for d in drops(&f, t, WIDE) {
                assert!(
                    (0.0..=1.0).contains(&d.weight) && d.weight.is_finite(),
                    "weight {} at t={t}",
                    d.weight
                );
                assert!(d.radius >= 0.0 && d.radius.is_finite(), "radius {} at t={t}", d.radius);
                assert!((0.0..=1.0).contains(&d.tone), "tone {} at t={t}", d.tone);
            }
            t += 0.41;
        }
    }

    #[test]
    fn a_drop_arrives_and_leaves_instead_of_appearing_and_vanishing() {
        // The envelope is the difference between drops and a slideshow. Both
        // ends must be zero, and the middle must not be. `sin(pi)` in f32 is
        // 1e-15 rather than 0, which is a fact about the library and not about
        // the drop — hence a tolerance at the ends the maths reaches by limit,
        // and exact zeroes outside the life, which are a branch.
        assert!(envelope(0.0).abs() < 1e-12);
        assert!(envelope(1.0).abs() < 1e-12);
        assert!(envelope(0.5) > 0.99);
        assert_eq!(envelope(-0.1), 0.0, "before it exists");
        assert_eq!(envelope(1.4), 0.0, "after it is gone");
        // …and no corner at either end: the slope has to vanish too, or the
        // drop ends with a visible kink.
        let slope = (envelope(0.02) - envelope(0.0)) / 0.02;
        assert!(slope < 0.35, "the envelope opens with a corner (slope {slope})");
    }

    #[test]
    fn the_field_is_never_empty_at_the_defaults() {
        // A model that opens on a blank frame is the failure nobody reports,
        // because it looks like it is still loading.
        let f = row();
        let mut t = 0.0;
        while t < 60.0 {
            let live = alive(&f, t);
            let coverage: f32 = live.iter().map(|d| d.weight * d.radius * d.radius).sum();
            assert!(coverage > 0.02, "only {coverage} of liquid in the frame at t={t}");
            t += 0.37;
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
        let a = frame(&gpu, &mut Solvent::new(), &f, &times);
        let b = frame(&gpu, &mut Solvent::pingpong(), &f, &times);
        let diff: Vec<u8> = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).collect();
        let mean = diff.iter().map(|d| *d as f32).sum::<f32>() / diff.len() as f32;
        let max = diff.iter().copied().max().unwrap_or(0);
        println!("solvent parity: mean {mean:.4} max {max}");
        assert!(a.iter().any(|v| *v > 8), "the frame is not black");
        assert!(mean < 0.5, "ping-pong drifted from compute: mean {mean:.3} max {max}");
    }
}
