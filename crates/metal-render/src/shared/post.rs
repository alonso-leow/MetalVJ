//! Light leaks + camera shake, applied once after whichever model just drew
//! to the offscreen texture, followed by the ten common effects (`crt`
//! through `grain`) and — Set F ("B7") — Wake (`wake`), in whatever
//! order the caller names. See `shaders/post.wgsl` for the actual
//! pass; this is just the pipeline/uniform plumbing around it.

use wgpu::util::DeviceExt;

use super::trail::ESTELA_RING_TAPS;
use super::ColorSlotGpu;
use metal_schema::{EffectStep, MAX_CHAIN};

/// How much larger the offscreen texture is than the final output, in each
/// axis. Shake samples within that margin instead of leaving black edges.
pub const OVERSCAN: f32 = 1.06;

/// The params block a `fx.estela` chain step reads — Wake's retirement as
/// a model (`models::estela`'s own module doc has the plan), one shipped
/// look, not one per node, sourced from
/// `models::estela::SCHEMA.defaults()`.
///
/// **Why this excludes `avance`/`tartamudeo`/`retencion`/`salto`.** Those
/// four are `models::estela::Stutter` — which position in a *footage clip*
/// the playhead holds or jumps to, resolved on the CPU against a
/// `Cue`/`Library` a chain step does not have (`shaders/post.wgsl`'s own
/// block comment above `effect_crt` has the full argument for why a chain
    /// step reads a ring of the generator's own recent frames instead of
    /// footage). The block-mosh proper never touches `Stutter` or `playhead` at
    /// all — `estela.wgsl`'s `fs_main` computes which tap a frozen block shows
/// from `congelado` (frozen)/`refresco` (refresh)/the wall clock/region hashing alone, using
/// each tap's own *pixels* only after the CPU already resolved them — so
/// that half of the model ports over to a ring of the generator's own past
/// frames unchanged, and the twelve floats below are exactly the ones that
/// half actually reads.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EstelaParams {
    pub bleed_color: ColorSlotGpu,
    /// bloque (blocks across), congelado (frozen), refresco (refresh, seconds), arrastre (drag)
    pub a: [f32; 4],
    /// desgarro (tear), sangrado (bleed), exposicion (exposure), contraste (contrast)
    pub b: [f32; 4],
    /// saturacion (saturation), r, g, b
    pub c: [f32; 4],
}

impl EstelaParams {
    /// Reads a params block out of a row shaped like `models::estela::SCHEMA`
    /// — this model's own sixteen floats (only twelve of which mean anything
    /// here — see this type's own doc comment for why the other four,
    /// `Stutter`'s, are not among them) followed by its `sangre` (bleed) colour
    /// slot, the same slice `models::estela::Estela::render`'s own uniform
    /// build already reads for the standalone model.
    ///
    /// Non-finite input falls back to `models::estela::SCHEMA.defaults()`'s
    /// value for that field: a step with nowhere sensible to read a look from
    /// should grade with the shipped look, not with black or NaN. Finite
    /// values are clamped to the
    /// same range the model itself defends — `bloque`, `congelado`,
    /// `refresco` and `sangrado` — and passed through as authored otherwise,
    /// matching exactly how much defence `Estela::render`'s own uniform
    /// build applies to each of these before it ever reaches a shader.
    pub fn from_row(floats: &[f32]) -> EstelaParams {
        use crate::models::estela::{MAX_BLOCKS, MAX_REFRESH, MIN_BLOCKS, MIN_REFRESH};
        let fallback = crate::models::estela::SCHEMA.defaults();
        let at = |i: usize| {
            let v = floats.get(i).copied().unwrap_or(fallback[i]);
            if v.is_finite() { v } else { fallback[i] }
        };
        let clamped = |i: usize, min: f32, max: f32| at(i).clamp(min, max);

        let mut bleed_color = [1.0f32; 12];
        match floats.get(16..28) {
            Some(slice) if slice.iter().all(|v| v.is_finite()) => bleed_color.copy_from_slice(slice),
            _ => bleed_color.copy_from_slice(&fallback[16..28]),
        }

        EstelaParams {
            bleed_color: ColorSlotGpu::from_floats(&bleed_color),
            a: [
                clamped(4, MIN_BLOCKS, MAX_BLOCKS),
                clamped(5, 0.0, 1.0),
                clamped(6, MIN_REFRESH, MAX_REFRESH),
                at(7),
            ],
            b: [at(8), clamped(9, 0.0, 1.0), at(10), at(11)],
            c: [at(12), at(13), at(14), at(15)],
        }
    }
}

impl Default for EstelaParams {
    /// Wake's own shipped look — see this type's own doc comment for why
    /// v1 has exactly one of these rather than an operator-authored one.
    fn default() -> EstelaParams {
        EstelaParams::from_row(&crate::models::estela::SCHEMA.defaults())
    }
}

/// How many `grade_*` floats the colour grade carries — `metal_schema::GRADE_FLOATS`
/// is the one declaration, so this cannot drift from the CPU mirror in
/// `metal-perform::grade`.
pub const GRADE_FLOATS_LEN: usize = metal_schema::GRADE_FLOATS.len();

/// The advanced effects block as the GPU post pass sees it, laid out in
/// uniform-friendly 16-byte groups. This is the Rust twin of `GradeUniform` in
/// `shaders/post.wgsl` (same field names and order), so a change here has to
/// be made there too.
///
/// The grade field order is exactly `metal_schema::GRADE_FLOATS`' own order,
/// regrouped; the linear `GRADE_FLOATS` index of each component is written
/// beside it so the two cannot silently drift:
///
/// | field      | components                                                       |
/// |------------|------------------------------------------------------------------|
/// | `tone_a`   | 0 `temperature`, 1 `tint`, 2 `exposure`, 3 `contrast`           |
/// | `tone_b`   | 4 `highlights`, 5 `shadows`, 6 `whites`, 7 `blacks`             |
/// | `color_a`  | 8 `vibrance`, 9 `saturation`, 10 `curve_highlights`, 11 `curve_lights` |
/// | `curve_a`  | 12 `curve_darks`, 13 `curve_shadows`, 14 `split_shadows`, 15 `split_mid` |
/// | `mix_a`    | 16 `split_highlights`, 17 `refine_saturation`, 54 `blending`, 55 `balance` |
/// | `mixer[0..8]` | 18..41, band `b`: 18+3b `hue`, `+1` `sat`, `+2` `lum`, 4th spare |
/// | `grading[0..4]` | 42..53, wheel `z`: 42+3z `hue`, `+1` `sat`, `+2` `lum`, 4th spare |
/// | `fx`       | advanced 56 `fx_texture`, 57 `fx_clarity`, 58 `fx_dehaze`, 4th = active gate |
///
/// `fx.w` is the **active gate**: `1.0` when any grade float differs from its
/// declared default, `0.0` otherwise. The shader runs `effect_grade` only when
/// it is set, so a post with no advanced block (every caller before this
/// landed) leaves `fs_main`'s output byte-identical rather than round-tripping
/// through HSL at the neutral grade.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GradeUniform {
    pub tone_a: [f32; 4],
    pub tone_b: [f32; 4],
    pub color_a: [f32; 4],
    pub curve_a: [f32; 4],
    pub mix_a: [f32; 4],
    pub mixer: [[f32; 4]; 8],
    pub grading: [[f32; 4]; 4],
    pub fx: [f32; 4],
}

impl GradeUniform {
    /// Builds the group from `Post.advanced`, tolerating a short or missing
    /// slice exactly the way `metal-perform::grade::GradeParams::from_floats`
    /// does: a missing or non-finite float reads as the declared default and
    /// every finite one is clamped into its declared range. An empty slice is
    /// therefore the neutral grade.
    pub fn from_advanced(floats: &[f32]) -> GradeUniform {
        let g = |i: usize| -> f32 {
            let spec = &metal_schema::GRADE_FLOATS[i];
            let v = floats.get(i).copied().unwrap_or(spec.default);
            if v.is_finite() { v.clamp(spec.min, spec.max) } else { spec.default }
        };
        let fx_at = |id: &str| -> f32 {
            match metal_schema::advanced_effect_span(id) {
                Some(span) if !span.is_empty() => {
                    let spec = &metal_schema::EFFECTS_FLOATS[span.start];
                    let idx = span.start - metal_schema::CHAIN_FLOATS;
                    let v = floats.get(idx).copied().unwrap_or(spec.default);
                    if v.is_finite() { v.clamp(spec.min, spec.max) } else { spec.default }
                }
                _ => 0.0,
            }
        };

        let mut mixer = [[0.0f32; 4]; 8];
        for (b, band) in mixer.iter_mut().enumerate() {
            *band = [g(18 + b * 3), g(18 + b * 3 + 1), g(18 + b * 3 + 2), 0.0];
        }
        let mut grading = [[0.0f32; 4]; 4];
        for (z, wheel) in grading.iter_mut().enumerate() {
            *wheel = [g(42 + z * 3), g(42 + z * 3 + 1), g(42 + z * 3 + 2), 0.0];
        }
        let grade_active = (0..GRADE_FLOATS_LEN)
            .any(|i| g(i) != metal_schema::GRADE_FLOATS[i].default);

        GradeUniform {
            tone_a: [g(0), g(1), g(2), g(3)],
            tone_b: [g(4), g(5), g(6), g(7)],
            color_a: [g(8), g(9), g(10), g(11)],
            curve_a: [g(12), g(13), g(14), g(15)],
            mix_a: [g(16), g(17), g(54), g(55)],
            mixer,
            grading,
            fx: [
                fx_at("texture"),
                fx_at("clarity"),
                fx_at("dehaze"),
                if grade_active { 1.0 } else { 0.0 },
            ],
        }
    }
}

impl Default for GradeUniform {
    /// The neutral grade — the same `Post::default().advanced` the rest of the
    /// suite treats as "no advanced effects on this row".
    fn default() -> GradeUniform {
        GradeUniform::from_advanced(&[])
    }
}

/// The extra floats of the multi-float chain steps, the Rust twin of
/// `ChainFx` in `shaders/post.wgsl` (same names and order). An effect whose
/// first float is the chain step's amount keeps its remaining floats here, in
/// 16-byte groups; one block per effect, appended at the end when an effect is
/// added. Built from `Post.advanced`, tolerating a short or missing slice like
/// [`GradeUniform::from_advanced`]: a missing or non-finite float reads as its
/// declared default and every finite one is clamped to its declared range.
///
/// | field  | components |
/// |--------|------------|
/// | `fm_a` | `portadora_freq`, `moduladora_freq`, `indice`, `fase` |
/// | `fm_b` | `angulo` (radians), `forma_onda`, `x_cant`, `y_cant` |
/// | `fm_c` | `color_cant`, `realim`, spare, spare |
/// | `wavewarp_a` | `tipo`, `largo`, `alto`, `angulo` (radians) |
/// | `wavewarp_b` | `vel`, `fase` (radians), `pin`, `pin_x` |
/// | `wavewarp_c` | `pin_y`, `asimetria`, `capa2`, `capa2_largo` |
/// | `wavewarp_d` | `capa2_angulo` (radians), `cruce`, `borde`, spare |
/// | `ascii_a` | `celda` (cell), `rampa` (ramp), `contraste` (contrast), `color_modo` (colour mode) |
/// | `ascii_b` | `color_mezcla`, `fondo`, `invierte`, `ruido` |
/// | `ascii_c` | `gamma`, `parpadeo`, `proporcion`, `tinte` |
/// | `ascii_d` | `bordes`, `deriva`, `grosor`, spare |
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChainFx {
    pub fm_a: [f32; 4],
    pub fm_b: [f32; 4],
    pub fm_c: [f32; 4],
    pub wavewarp_a: [f32; 4],
    pub wavewarp_b: [f32; 4],
    pub wavewarp_c: [f32; 4],
    pub wavewarp_d: [f32; 4],
    pub ascii_a: [f32; 4],
    pub ascii_b: [f32; 4],
    pub ascii_c: [f32; 4],
    pub ascii_d: [f32; 4],
    pub mosaic_a: [f32; 4],
    pub faceted_a: [f32; 4],
    pub thermograph_a: [f32; 4],
    pub depthfield_a: [f32; 4],
    pub refract_a: [f32; 4],
    pub relamp_a: [f32; 4],
    pub letterform_a: [f32; 4],
    pub datafield_a: [f32; 4],
}

/// One advanced group's floats out of `Post.advanced`, defaults and clamps
/// applied (the helper every `ChainFx` block is built with).
pub(crate) fn group_floats(advanced: &[f32], id: &str) -> Vec<f32> {
    let (Some(specs), Some(span)) = (metal_schema::advanced_effect_floats(id), metal_schema::advanced_effect_span(id))
    else {
        return Vec::new();
    };
    let base = span.start - metal_schema::CHAIN_FLOATS;
    specs
        .iter()
        .enumerate()
        .map(|(i, spec)| {
            let v = advanced.get(base + i).copied().unwrap_or(spec.default);
            if v.is_finite() { v.clamp(spec.min, spec.max) } else { spec.default }
        })
        .collect()
}

/// The three params after a group's amount, packed as one vec4.
fn pack4(advanced: &[f32], id: &str) -> [f32; 4] {
    let f = group_floats(advanced, id);
    let at = |i: usize| f.get(i).copied().unwrap_or(0.0);
    [at(1), at(2), at(3), 0.0]
}

impl ChainFx {
    pub fn from_advanced(advanced: &[f32]) -> ChainFx {
        // Order = `metal_schema::FM_FLOATS`; index 0 (`cant`) is the step amount.
        let f = group_floats(advanced, "fm");
        let at = |i: usize| f.get(i).copied().unwrap_or(0.0);
        let w = group_floats(advanced, "wavewarp");
        let wt = |i: usize| w.get(i).copied().unwrap_or(0.0);
        // `ascii` follows `metal_schema::ASCII_FLOATS` the same way.
        let a = group_floats(advanced, "ascii");
        let an = |i: usize| a.get(i).copied().unwrap_or(0.0);
        ChainFx {
            wavewarp_a: [wt(1), wt(2), wt(3), wt(4).to_radians()],
            wavewarp_b: [wt(5), wt(6).to_radians(), wt(7), wt(8)],
            wavewarp_c: [wt(9), wt(10), wt(11), wt(12)],
            wavewarp_d: [wt(13).to_radians(), wt(14), wt(15), 0.0],
            fm_a: [at(1), at(2), at(3), at(4)],
            fm_b: [at(5).to_radians(), at(6), at(7), at(8)],
            fm_c: [at(9), at(10), 0.0, 0.0],
            ascii_a: [an(1), an(2), an(3), an(4)],
            ascii_b: [an(5), an(6), an(7), an(8)],
            ascii_c: [an(9), an(10), an(11), an(12)],
            ascii_d: [an(13), an(14), an(15), 0.0],
            mosaic_a: pack4(advanced, "mosaic"),
            faceted_a: pack4(advanced, "faceted"),
            thermograph_a: pack4(advanced, "thermograph"),
            depthfield_a: pack4(advanced, "depthfield"),
            refract_a: pack4(advanced, "refract"),
            relamp_a: pack4(advanced, "relamp"),
            letterform_a: pack4(advanced, "letterform"),
            datafield_a: pack4(advanced, "datafield"),
        }
    }
}

impl Default for ChainFx {
    fn default() -> ChainFx {
        ChainFx::from_advanced(&[])
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    params: [f32; 4],
    crop: [f32; 4],
    extra: [f32; 4],
    chain_len: [u32; 4],
    chain: [[f32; 4]; MAX_CHAIN],
    leak_color: ColorSlotGpu,
    wake: EstelaParams,
    /// The advanced colour/texture/clarity/dehaze block. Appended after
    /// `wake` (byte offset 496, a multiple of the 16-byte uniform
    /// alignment) so every field above keeps the offset it always had.
    grade: GradeUniform,
    /// Multi-float chain steps' params; appended last so nothing above moves.
    chain_fx: ChainFx,
}

pub struct PostChain {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

pub struct PostParams {
    pub shake: f32,
    pub light_leak: f32,
    pub leak_color: ColorSlotGpu,
    pub time: f32,
    /// Mirrors to fold the frame into; below 2 is off. Stays a flat field
    /// here — unlike the ten effects `apply_tile`'s `chain` argument now
    /// carries, `kaleido` is not a chain step: it folds the coordinate the
    /// chain starts from, before `chain_uv` even exists, and a node graph
    /// never gets to place it relative to them. See the block comment above
    /// `effect_crt` in `shaders/post.wgsl`.
    pub kaleido: f32,
    /// The one params block a `fx.estela` chain step (`metal_schema::
    /// CHAIN_KIND_ESTELA`) reads — see [`EstelaParams`]'s own doc comment
    /// for why there is exactly one. Unread, and harmless, when `chain`
    /// carries no such step — most callers pass [`EstelaParams::default`].
    pub wake: EstelaParams,
    /// The advanced block (`Post.advanced`): the colour grade plus the
    /// separate texture/clarity/dehaze amounts, packed for the uniform. See
    /// [`GradeUniform`] for the exact layout and for why its `fx.w` gate
    /// keeps a post with no advanced block byte-identical.
    pub grade: GradeUniform,
    /// The extra floats of the multi-float chain steps (`fx.fm`, …), built
    /// with [`ChainFx::from_advanced`]`(&post.advanced)`; [`ChainFx::default`]
    /// when no step needs them.
    pub chain_fx: ChainFx,
}

impl PostChain {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> PostChain {
        let shader = super::build_shader(
            device,
            "post-chain",
            include_str!("../../shaders/post.wgsl"),
        );

        // Bindings 0-2 are the uniform, the pre-chain source and its sampler,
        // unchanged since before Wake. 3..3+N are `fx.estela`'s ring —
        // `ESTELA_RING_TAPS` separate `texture_2d<f32>` bindings rather than
        // one `texture_2d_array` (the shape `shared::footage::Frames` uses
        // for its own eight taps) because that is the plumbing
        // `shaders/post.wgsl`'s own note specced out: N bindings this bind
        // group grows by, not one binding of N layers. See
        // `shared::trail::EstelaRing` for where those N textures live.
        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };

        let mut entries = vec![
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
            texture_entry(1),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ];
        for i in 0..ESTELA_RING_TAPS as u32 {
            entries.push(texture_entry(3 + i));
        }
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post-chain bind group layout"),
            entries: &entries,
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post-chain pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("post-chain pipeline"),
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

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post-chain sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        PostChain { pipeline, bind_group_layout, sampler }
    }

    /// Reads the (overscanned) `src` texture and writes the cropped,
    /// shaken, leak-tinted, chained result into `dst`, sized `final_w x
    /// final_h`. `chain` is the order to run the ten common effects and Wake
    /// in — see [`Self::apply_tile`], which this is a
    /// one-tile shorthand for, including for `estela_ring`.
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        final_w: u32,
        final_h: u32,
        params: &PostParams,
        chain: &[EffectStep],
        estela_ring: &[&wgpu::TextureView; ESTELA_RING_TAPS],
    ) {
        self.apply_tile(
            device,
            queue,
            src,
            dst,
            (0, 0, final_w, final_h),
            true,
            params,
            chain,
            estela_ring,
        );
    }

    /// The same pass into a sub-rectangle of `dst`, optionally without
    /// clearing what is already there. This is what lets the preview page
    /// composite all eleven models into one image: each model draws its own
    /// tile of a shared texture, so a whole grid is one readback rather than
    /// eleven.
    ///
    /// `chain` is the order to run the ten common effects and Wake in —
    /// `crt` through `grain` at `metal-schema::EFFECTS_FLOATS`
    /// positions 4..=13, plus `metal_schema::CHAIN_KIND_ESTELA` (15) — each
    /// paired with how much of it
    /// to apply, most often `metal_schema::Post::resolved_chain`'s own output.
    /// A position this build's shader does not handle (0..3, or >= 16) is
    /// not rejected here; it rides along into the uniform and the shader's
    /// `switch` silently ignores it — see the block comment above
    /// `effect_crt` in `shaders/post.wgsl` for why those positions are not
    /// chain steps. Truncated to [`MAX_CHAIN`], the GPU array's fixed size,
    /// the same truncation `resolved_chain` already applies — belt and
    /// suspenders, since every caller in this codebase already goes through
    /// that function, but this is the boundary where an oversized slice
    /// would otherwise index past the uniform.
    ///
    /// `estela_ring` is the [`shared::trail::ESTELA_RING_TAPS`](super::trail::ESTELA_RING_TAPS)
    /// views a `fx.estela` step samples, newest first — a fixed-size array
    /// rather than a slice so a caller cannot pass the wrong count and have
    /// it silently truncate or leave a binding unset. Unread, and harmless,
    /// when `chain` carries no such step: most callers with no history of
    /// their own pass [`shared::trail::fallback_ring`](super::trail::fallback_ring)
    /// (the current frame, repeated) — see that function's own doc comment.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_tile(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        rect: (u32, u32, u32, u32),
        clear: bool,
        params: &PostParams,
        chain: &[EffectStep],
        estela_ring: &[&wgpu::TextureView; ESTELA_RING_TAPS],
    ) {
        let (rx, ry, final_w, final_h) = rect;
        let crop = 1.0 / OVERSCAN;
        // The shake offset is in UV, and UV is not isotropic: on a 16:9 output
        // the same UV offset moves 1.78x as many pixels sideways as it does
        // vertically, which reads as a horizontal wobble rather than a shake.
        // The shader divides the x component by this to even it back out.
        let aspect = final_w.max(1) as f32 / final_h.max(1) as f32;

        // `chain_gpu.iter_mut().zip(chain.iter())` stops at the shorter of
        // the two on its own, so this both truncates an oversized `chain`
        // and leaves any slot past `chain.len()` at its zeroed default —
        // harmless, since `chain_len` below tells the shader to never read
        // that far.
        //
        // `step.slot` (which params block a footage-kind step reads) is not
        // packed into the third/fourth component here: v1 has exactly one
        // slot per footage kind (see `EstelaParams`'s own doc comment), so
        // the shader has only ever the one block to read regardless of what
        // a step's `slot` says. Nothing is lost — a build that adds a second
        // slot will need a shader-side change to consume it anyway, and this
        // is where that would start reading it.
        let mut chain_gpu = [[0.0f32; 4]; MAX_CHAIN];
        for (gpu_step, step) in chain_gpu.iter_mut().zip(chain.iter()) {
            *gpu_step = [step.effect as f32, step.amount, 0.0, 0.0];
        }
        let chain_len = chain.len().min(MAX_CHAIN) as u32;

        let uniform = Uniform {
            params: [params.shake, params.light_leak, params.time, aspect],
            crop: [crop, crop, 0.0, 0.0],
            extra: [params.kaleido, 0.0, 0.0, 0.0],
            chain_len: [chain_len, 0, 0, 0],
            chain: chain_gpu,
            leak_color: params.leak_color,
            wake: params.wake,
            grade: params.grade,
            chain_fx: params.chain_fx,
        };
        let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("post-chain uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        // Bindings 0-2 as always, then `estela_ring` at 3..3+N — see the
        // block comment above the layout in `PostChain::new` for why N
        // separate bindings rather than one texture array.
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(src) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
        ];
        for (i, view) in estela_ring.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 3 + i as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post-chain bind group"),
            layout: &self.bind_group_layout,
            entries: &entries,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("post-chain encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post-chain pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dst,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: if clear {
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            // The viewport maps the fullscreen triangle onto the tile; the
            // scissor is what stops it spilling over the rest of the target,
            // since a viewport transforms but does not clip.
            pass.set_viewport(rx as f32, ry as f32, final_w as f32, final_h as f32, 0.0, 1.0);
            pass.set_scissor_rect(rx, ry, final_w, final_h);
            pass.draw(0..3, 0..1);
        }
        queue.submit(Some(encoder.finish()));
    }
}

// ---------------------------------------------------------------------------
// LAY.5 — the declarative table and the drawn dispatch may not disagree
// ---------------------------------------------------------------------------
//
// WGSL has no function pointers, so the `switch` in `fs_main` cannot be
// generated from `EFFECT_SPECS` the way `EFFECTS_FLOATS` and
// `effect_column_map` are — the case bodies differ in their call shape
// (zoom/lorenz relocate `chain_uv`; estela reads its own params
// block), and a generated uniform call would be wrong for exactly those.
// What *can* be enforced is that the two halves agree: every row with
// `chains: true` has a case and a function, every case names a declared
// chain step, and the loop is bounded by the uniform's own length. Adding an
// effect is then one row in `metal-schema` plus one WGSL function — forget the
// case and this test names the line you are missing.
#[cfg(test)]
mod chain_conformance_tests {
    use metal_schema::{advanced_chain_kind, ADVANCED_CHAIN_EFFECTS, CHAIN_KIND_ESTELA, EFFECT_SPECS};

    const SHADER: &str = include_str!("../../shaders/post.wgsl");

    /// Advanced chain effects applied outside the post shader.
    const STATEFUL_CHAIN_EFFECTS: &[&str] = &[
        "brush",
        "hardstutter",
    ];

    fn dispatch_cases() -> Vec<u32> {
        SHADER
            .lines()
            .filter_map(|line| {
                let rest = line.trim().strip_prefix("case ")?;
                let token = rest.split(':').next()?;
                token.trim().strip_suffix('u')?.parse::<u32>().ok()
            })
            .collect()
    }

    #[test]
    fn every_chain_step_in_the_table_has_a_case_and_a_function() {
        let cases = dispatch_cases();
        for (i, spec) in EFFECT_SPECS.iter().enumerate() {
            if !spec.chains {
                continue;
            }
            assert!(
                cases.contains(&(i as u32)),
                "EFFECT_SPECS[{i}] = {} declares chains: true but post.wgsl has no `case {i}u:`",
                spec.spec.name
            );
            assert!(
                SHADER.contains(&format!("fn {}(", spec.wgsl)),
                "{} declares wgsl = {:?} but post.wgsl has no such function",
                spec.spec.name,
                spec.wgsl
            );
        }
    }

    #[test]
    fn the_dispatch_cases_are_exactly_the_chain_steps_the_table_declares() {
        let mut expected: Vec<u32> = EFFECT_SPECS
            .iter()
            .enumerate()
            .filter(|(_, s)| s.chains)
            .map(|(i, _)| i as u32)
            .chain([CHAIN_KIND_ESTELA])
            .chain(ADVANCED_CHAIN_EFFECTS.iter().filter_map(|id| advanced_chain_kind(id)))
            .collect();
        expected.sort_unstable();
        let mut got = dispatch_cases();
        got.sort_unstable();
        assert_eq!(
            got, expected,
            "the shader's dispatch cases must be exactly the chain steps EFFECT_SPECS declares"
        );
    }

    /// Every advanced chain effect has its WGSL pair (`effect_<id>` and the
    /// coordinate half `<id>_uv` if it relocates) and a `ChainFx` block.
    #[test]
    fn every_advanced_chain_effect_has_a_function_and_a_uniform_block() {
        for id in ADVANCED_CHAIN_EFFECTS {
            // Stateful effects run before the post pass (`shared::brush`);
            // their `case` here is a no-op and they have no function or block.
            if STATEFUL_CHAIN_EFFECTS.contains(id) {
                continue;
            }
            assert!(SHADER.contains(&format!("fn effect_{id}(")), "post.wgsl has no fn effect_{id}(");
            assert!(SHADER.contains(&format!("{id}_a: vec4<f32>")), "ChainFx has no {id}_a block in post.wgsl");
        }
    }

    /// `fx.ascii` embeds the `ascii` model's ramps in `post.wgsl`; the table
    /// and the ramp lengths it hard-codes must equal the Rust source.
    #[test]
    fn the_embedded_ascii_font_equals_the_models_ramps() {
        let ramps = crate::models::ascii::packed_ramps();
        assert_eq!(crate::models::ascii::ramp_lengths(), vec![10, 9, 8, 7], "post.wgsl hard-codes these lengths");
        assert!(SHADER.contains(&format!("array<vec2<u32>, {}>(", ramps.len())));
        let table: String = ramps.iter().map(|[x, y]| format!("    vec2<u32>({x}u, {y}u)")).collect::<Vec<_>>().join(",\n");
        assert!(SHADER.contains(&table), "ASCII_GLYPHS in post.wgsl differs from models/ascii.rs");
    }

    #[test]
    fn the_loop_is_bounded_by_the_uniforms_own_chain_length() {
        assert!(
            SHADER.contains("u.chain[i]"),
            "fs_main must read the chain from the uniform, not a fixed order"
        );
        assert!(
            SHADER.contains("i < u.chain_len.x"),
            "fs_main's loop must be bounded by the uniform's own length"
        );
    }

    /// The GPU twin of the advanced block exists, is applied from `fs_main`,
    /// and keeps its gate — the tests above only police the 14 common effects.
    #[test]
    fn the_grade_and_its_helpers_are_wired_into_fs_main() {
        assert!(SHADER.contains("fn effect_grade(color: vec3<f32>, p: GradeUniform) -> vec3<f32>"));
        for f in ["effect_texture", "effect_clarity", "effect_dehaze"] {
            assert!(SHADER.contains(&format!("fn {f}(")), "post.wgsl has no fn {f}(");
        }
        assert!(
            SHADER.contains("if (u.grade.fx.w > 0.5)"),
            "effect_grade must stay behind the active gate"
        );
    }
}

#[cfg(test)]
mod grade_uniform_tests {
    use super::{GradeUniform, GRADE_FLOATS_LEN};

    #[test]
    fn an_empty_advanced_slice_is_the_declared_neutral_default() {
        let g = GradeUniform::default();
        // The declared defaults, not zeros, are the neutral grade.
        assert_eq!(g.curve_a[2], 25.0, "split_shadows default");
        assert_eq!(g.curve_a[3], 50.0, "split_mid default");
        assert_eq!(g.mix_a[0], 75.0, "split_highlights default");
        assert_eq!(g.mix_a[1], 100.0, "refine_saturation default");
        assert_eq!(g.mix_a[2], 50.0, "blending default");
        assert_eq!(g.fx[3], 0.0, "a neutral grade must leave the gate off");
    }

    #[test]
    fn grade_floats_land_at_the_documented_components() {
        let mut advanced = vec![0.0f32; metal_schema::Post::default().advanced.len()];
        // 0 temperature, 9 saturation, 18 mixer red hue, 42 grading shadows
        // hue, 54 blending, 55 balance.
        advanced[0] = 40.0;
        advanced[9] = -30.0;
        advanced[18] = 12.0;
        advanced[42] = 180.0;
        advanced[54] = 70.0;
        advanced[55] = -20.0;
        let g = GradeUniform::from_advanced(&advanced);
        assert_eq!(g.tone_a[0], 40.0, "grade_temperature");
        assert_eq!(g.color_a[1], -30.0, "grade_saturation");
        assert_eq!(g.mixer[0][0], 12.0, "grade_mixer_red_hue");
        assert_eq!(g.grading[0][0], 180.0, "grade_grading_shadows_hue");
        assert_eq!(g.mix_a[2], 70.0, "grade_blending");
        assert_eq!(g.mix_a[3], -20.0, "grade_balance");
        assert_eq!(g.fx[3], 1.0, "an off-default grade must open the gate");
    }

    #[test]
    fn texture_clarity_dehaze_land_in_fx_with_their_declared_default_when_missing() {
        let mut advanced = metal_schema::Post::default().advanced;
        let base = metal_schema::advanced_effect_span("texture").unwrap().start
            - metal_schema::CHAIN_FLOATS;
        advanced[base] = 55.0; // fx_texture
        advanced[base + 1] = -40.0; // fx_clarity
        advanced[base + 2] = 15.0; // fx_dehaze
        let g = GradeUniform::from_advanced(&advanced);
        assert_eq!(&g.fx[0..3], &[55.0, -40.0, 15.0]);
        assert_eq!(g.fx[3], 0.0, "a texture-only edit must not open the grade gate");
        // A slice shorter than the advanced block still fills the grade from
        // its declared defaults rather than reading as zero.
        let short = GradeUniform::from_advanced(&advanced[..2]);
        assert_eq!(short.curve_a[2], 25.0, "split_shadows default from a short slice");
        assert_eq!(short.fx[0], 0.0, "texture missing reads neutral");
        assert!(GRADE_FLOATS_LEN >= 56, "the grade group is at least the 56 documented floats");
    }
}
