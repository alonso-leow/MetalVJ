//! Compile and rasterize WGSL written at runtime — by a human pasting a
//! shader or by the agent loop (`metal-agent`, H.3) writing one on the fly.
//! `the design notes` H.2 is the spec; three things there are
//! non-negotiable and shape every choice below:
//!
//! 1. `naga` validates before `wgpu` ever sees the source. A broken shader
//!    is a normal [`CompileError`] result, carrying line/column, never a
//!    panic.
//! 2. Compilation and rasterization run on a device this crate owns and
//!    nobody else touches — see [`device`] — so a hung or malicious shader
//!    here cannot stall or crash whatever device the live show is drawing
//!    with.
//! 3. `raster` enforces a hard [`RASTER_TIMEOUT`] and a hard
//!    [`MAX_DIMENSION`], so the worst case is bounded no matter what the
//!    shader does.
//!
//! # The float-uniform contract
//!
//! Every model in `metal-render` (see `crates/metal-render/src/models/*.rs`, e.g.
//! `chamber.rs`) gets its animated parameters the same way: a `#[repr(C)]`
//! Rust struct is written into a `wgpu::Buffer` with `BufferUsages::UNIFORM`,
//! bound at `@group(0) @binding(0)`, and the WGSL side declares a matching
//! struct whose fields are named `floats`, `floats2`, `floats3`, … — each a
//! `vec4<f32>` — read straight out of `render()`'s `&[f32]` slice four at a
//! time (`chamber.wgsl`'s `MeshUniform` is the clearest example: `floats:
//! vec4<f32>`, `floats2: vec4<f32>`).
//!
//! A shader compiled by [`compile`] has to declare the same shape: a
//! `var<uniform>` at `@group(0) @binding(0)` whose type is a struct with at
//! least one `floatsN: vec4<f32>` member. `the design notes` is explicit
//! about why this is enforced rather than merely conventional — "the soul of
//! every animation here is its variability... a shader with baked-in
//! constants is a painting, not a model" — so a shader with no such member
//! fails [`compile`] with [`CompileError::NoFloatBlock`] rather than
//! compiling into something that always draws the same frame.
//!
//! The fragment entry point is expected to be named `fs_main`, following the
//! same convention `metal-render`'s own `shared::fullscreen` helper uses for
//! every model that is just a field over the frame. `compile` supplies the
//! vertex stage itself (one oversized triangle, `uv` in `@location(0)`) —
//! the caller's WGSL only needs the uniform struct and `fs_main`.

#[cfg(not(target_arch = "wasm32"))]
mod device;
mod error;
#[cfg(not(target_arch = "wasm32"))]
pub mod gate;
#[cfg(not(target_arch = "wasm32"))]
mod raster;
mod uniform;
pub mod vocab;

#[cfg(not(target_arch = "wasm32"))]
pub use device::ForgeDevice;
pub use error::CompileError;
#[cfg(not(target_arch = "wasm32"))]
pub use raster::{raster, RasterError, MAX_DIMENSION, RASTER_TIMEOUT};

#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

/// The vertex half every custom shader gets for free — verbatim what
/// `metal-render`'s `shared::fullscreen::VERTEX` does, so a shader authored
/// against one behaves the same against the other.
pub const VERTEX_PRELUDE: &str = r#"
struct FsVOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> FsVOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    let xy = p[vi];
    var out: FsVOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return out;
}
"#;

/// How many lines [`VERTEX_PRELUDE`] adds ahead of the caller's own source —
/// used to translate naga's line numbers (which are against the
/// concatenated source) back to line numbers in the WGSL the caller
/// actually wrote.
fn prelude_line_count() -> u32 {
    // Plus one: the source is `{VERTEX_PRELUDE}\n{wgsl}`, and the prelude
    // already ends in a newline, so the separator is a blank line of its own.
    // Without it every reported line was one past the caller's — naga said
    // "line 9" for a missing `;` on the caller's line 7.
    VERTEX_PRELUDE.lines().count() as u32 + 1
}

/// The color target every raster targets. Fixed rather than parameterized:
/// `raster` always produces a PNG, and sRGB-encoded 8-bit is what a PNG
/// holds anyway.
pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// A validated, GPU-ready shader: the output of [`compile`] and the input to
/// [`raster`].
#[cfg(not(target_arch = "wasm32"))]
pub struct Pipeline {
    pub(crate) pipeline: wgpu::RenderPipeline,
    pub(crate) bind_group_layout: wgpu::BindGroupLayout,
    /// Byte size of the uniform buffer the WGSL's own struct declares —
    /// computed from the `floatsN` members' offsets, not guessed.
    pub(crate) uniform_size: u64,
    /// How many `f32`s the declared `floatsN` members can hold in total
    /// (four per member) — `raster` accepts at most this many.
    pub(crate) float_capacity: usize,
    /// Byte offset of each `floatsN` member within the uniform struct, in
    /// `floats`/`floats2`/`floats3`/... order — where a fresh buffer of
    /// `uniform_size` gets each group of 4 floats written. Not assumed to
    /// be contiguous from 0: a caller's struct may declare other members
    /// (a colour, a view matrix) before or between the `floatsN` ones.
    pub(crate) float_offsets: Vec<u32>,
    /// Whether the shader is an **effect**: it samples a picture at
    /// `@group(0) @binding(1)` (a `texture_2d<f32>`) through a sampler at
    /// `@binding(2)` — see [`Checked::effect`].
    pub(crate) effect: bool,
    pub(crate) device: Arc<ForgeDevice>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Pipeline {
    pub fn is_effect(&self) -> bool {
        self.effect
    }
}

/// A shader that parsed, validated and declares its float block — what
/// [`check`] hands back without touching a GPU. The live renderer
/// (`metal-render`'s `forge` model and `fx.forge` pass) builds its own pipeline
/// from `source` on its own device; [`compile`] builds one on the forge's.
#[derive(Debug, Clone)]
pub struct Checked {
    /// The full source a pipeline is built from: [`VERTEX_PRELUDE`] and the
    /// caller's WGSL, exactly what was validated.
    pub source: String,
    pub uniform_size: u64,
    pub float_capacity: usize,
    pub float_offsets: Vec<u32>,
    /// An effect shader samples its input: `@group(0) @binding(1) var src:
    /// texture_2d<f32>;` and `@group(0) @binding(2) var samp: sampler;`. A
    /// generator declares neither. Declaring one without the other is a
    /// [`CompileError::EffectBindings`].
    pub effect: bool,
}

/// Parses and validates `wgsl` and finds its float block — [`compile`]'s
/// first half, with no GPU. The one gate every custom shader passes through
/// whichever device ends up drawing it.
pub fn check(wgsl: &str) -> Result<Checked, CompileError> {
    let source = format!("{VERTEX_PRELUDE}\n{wgsl}");

    let module = naga::front::wgsl::parse_str(&source).map_err(|e| {
        let loc = e.location(&source);
        CompileError::from_parse(e.message().to_string(), loc, prelude_line_count())
    })?;

    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    );
    validator.validate(&module).map_err(|e| {
        let loc = e.location(&source);
        CompileError::from_validation(e.to_string(), loc, prelude_line_count())
    })?;

    let float_block = uniform::find_float_block(&module).ok_or(CompileError::NoFloatBlock)?;
    let (texture, sampler) = uniform::effect_bindings(&module);
    if texture != sampler {
        return Err(CompileError::EffectBindings);
    }
    Ok(Checked {
        source,
        uniform_size: float_block.struct_size,
        float_capacity: float_block.float_capacity,
        float_offsets: float_block.member_offsets,
        effect: texture,
    })
}

/// The bind group layout entries a checked shader needs: the uniform, and
/// for an effect the input texture and its sampler.
pub fn layout_entries(effect: bool) -> Vec<wgpu::BindGroupLayoutEntry> {
    let mut entries = vec![wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }];
    if effect {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
    }
    entries
}

/// The uniform buffer's bytes for `floats`, written four at a time at each
/// `floatsN` member's offset — the one packing rule [`raster`] and the live
/// renderer share.
pub fn uniform_bytes(checked_size: u64, offsets: &[u32], floats: &[f32]) -> Vec<u8> {
    let mut bytes = vec![0u8; checked_size as usize];
    for (i, chunk) in floats.chunks(4).enumerate() {
        let Some(&offset) = offsets.get(i) else { break };
        let mut quad = [0f32; 4];
        quad[..chunk.len()].copy_from_slice(chunk);
        let offset = offset as usize;
        bytes[offset..offset + 16].copy_from_slice(bytemuck::bytes_of(&quad));
    }
    bytes
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Debug for Pipeline {
    /// Only exists so `Result<Pipeline, CompileError>::expect_err` reads
    /// nicely in tests — none of the GPU handles inside are meaningfully
    /// printable, so this deliberately shows none of them.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline").field("float_capacity", &self.float_capacity).finish_non_exhaustive()
    }
}

/// Parses `wgsl` with `naga`, validates it, and checks it declares the
/// float-uniform block every model in the suite uses (see the module docs).
/// Returns line/column-annotated [`CompileError`]s instead of panicking —
/// per H.2, a broken shader from the agent loop is a normal tool result.
#[cfg(not(target_arch = "wasm32"))]
pub fn compile(wgsl: &str) -> Result<Pipeline, CompileError> {
    let checked = check(wgsl)?;

    let device = device::forge_device();
    let shader = device.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("metal-forge custom shader"),
        source: wgpu::ShaderSource::Wgsl(checked.source.clone().into()),
    });

    let bind_group_layout = device.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("metal-forge bgl"),
        entries: &layout_entries(checked.effect),
    });

    let pipeline_layout = device.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("metal-forge pipeline layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let pipeline = device.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("metal-forge pipeline"),
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
                format: TARGET_FORMAT,
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

    Ok(Pipeline {
        pipeline,
        bind_group_layout,
        uniform_size: checked.uniform_size,
        float_capacity: checked.float_capacity,
        float_offsets: checked.float_offsets,
        effect: checked.effect,
        device: device.clone(),
    })
}
