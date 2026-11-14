//! Ghosting — the one effect in this project that has to remember something.
//!
//! The post chain is stateless by construction and every other effect is a
//! function of `(frame, time)`. A trail is not: it is a function of every frame
//! before it. So this module owns the state and keeps it away from the post
//! chain, and the three traps `docs/NEXT.md` §4 measured are each answered
//! somewhere specific:
//!
//! 1. **One scratch texture, every model draws into it.** `Renderer::scratch_for`
//!    caches a single offscreen target by size and each tile of the grid draws
//!    into it in turn. A history hung off that would smear Geoterrain's spirals
//!    into Domain's scanlines. So a [`Trail`] is owned *per model*, keyed by
//!    size, and [`Trails`] is the map.
//! 2. **Decay per second, not per frame.** `Post::ghost_tau` and the `dt` this
//!    module tracks, not a fixed fraction. Both apps render on demand.
//! 3. **Determinism.** Asking for the same instant twice must advance nothing
//!    and return the same image — the property Turing had to keep for exactly
//!    the same reason, and the one both render gates rest on. `dt <= 0` returns
//!    the history untouched, and the caller skips this whole path when the
//!    effect is off, so a cold renderer at `ghost = 0` is byte-for-byte what it
//!    was before ghosting existed.
//!
//! # A second history, added alongside rather than folded in
//!
//! [`FrameRing`]/[`EstelaRings`] (2026-09-04) are `fx.estela`'s chain-step
//! history — a ring of [`ESTELA_RING_TAPS`] *undecayed* past frames, so a
//! frozen block can reveal exactly the moment it froze on, the way
//! `models::estela`'s own block-mosh does. **This is not [`Trail`] widened
//! into a bigger ring**, even though the note in `shaders/post.wgsl` that
//! specced this out talks about generalising `Trail` that way: [`Trail`]'s
//! two textures hold a running exponential *blend* (a decayed accumulator,
//! folding every past frame into one), and a block-mosh needs a *specific*
//! past frame, undecayed — sharing one struct between the two would mean
//! either blending Wake's taps (the wrong effect — see `models::estela`'s
//! own doc on why per-block selection, not blending, is the whole
//! difference between a ghost and a datamosh) or feeding ghost's decay raw
//! frames it was never built to accept. Keeping them separate is also what
//! keeps ghost's own "off is byte-identical to before ghosting existed"
//! guarantee free of any risk from this change: there is no shared code path
//! for the new ring to disturb it through.

use wgpu::util::DeviceExt;

use crate::gpu::offscreen_target;

/// A jump larger than this is a seek, not a frame: the trail is thrown away and
/// restarted from the current frame rather than blended across the gap.
///
/// One second is the same threshold Turing reseeds on, and for the same reason
/// — the alternative is a smear left by wherever the clock was before.
///
/// Shared with [`FrameRing`] below, deliberately not a second constant: "this
/// is not continuous playback any more" is one fact about the wall clock, not
/// a fact about which history mechanism is asking.
const SEEK: f32 = 1.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    params: [f32; 4],
}

/// The pipeline, built once and shared by every model's trail.
pub struct TrailChain {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

/// One model's history: two textures it ping-pongs between, and when it was
/// last advanced.
struct Trail {
    textures: [(wgpu::Texture, wgpu::TextureView); 2],
    /// Which of the two currently holds the history.
    front: usize,
    size: (u32, u32),
    last_time: Option<f32>,
}

/// Every model's history, allocated on demand.
#[derive(Default)]
pub struct Trails {
    per_model: Vec<Option<Trail>>,
}

impl TrailChain {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> TrailChain {
        let shader = super::build_shader(
            device,
            "trail",
            include_str!("../../shaders/trail.wgsl"),
        );

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

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trail bind group layout"),
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
                texture_entry(1),
                texture_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trail pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("trail pipeline"),
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
            label: Some("trail sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        TrailChain { pipeline, bind_group_layout, sampler }
    }
}

impl Trails {
    /// Advances model `index`'s trail with the frame in `current`, and returns
    /// the view the post chain should read instead of `current`.
    ///
    /// `None` means "nothing to do — read `current` yourself": either the size
    /// was degenerate, or the same instant was asked for twice with no history
    /// yet. Every path through here is a function of `time` and the frames that
    /// came before it, and asking twice at one `time` never advances anything.
    #[allow(clippy::too_many_arguments)]
    pub fn advance<'a>(
        &'a mut self,
        chain: &TrailChain,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        index: usize,
        current: &wgpu::TextureView,
        size: (u32, u32),
        time: f32,
        tau: f32,
    ) -> Option<&'a wgpu::TextureView> {
        let (w, h) = size;
        if w == 0 || h == 0 {
            return None;
        }
        if self.per_model.len() <= index {
            self.per_model.resize_with(index + 1, || None);
        }

        // Reallocate on a size change rather than scaling: a trail sampled from
        // a differently-sized history is a trail that stretches whenever the
        // resolution control moves.
        let stale = !matches!(&self.per_model[index], Some(t) if t.size == (w, h));
        if stale {
            let a = offscreen_target(device, w, h, format);
            let b = offscreen_target(device, w, h, format);
            self.per_model[index] =
                Some(Trail { textures: [a, b], front: 0, size: (w, h), last_time: None });
        }
        let trail = self.per_model[index].as_mut().expect("just built above");

        let dt = trail.last_time.map(|last| time - last);
        let decay = match dt {
            // The same instant, again. Return what is already there: advancing
            // would fade the trail once per request, so a paused preview would
            // dissolve while nothing about it had changed.
            Some(0.0) => return Some(&trail.textures[trail.front].1),
            // A seek, a rewind, or the first frame: no history to blend.
            None => 0.0,
            Some(d) if !(0.0..=SEEK).contains(&d) => 0.0,
            Some(d) => (-d / tau.max(1e-4)).exp(),
        };

        let back = 1 - trail.front;
        let uniform = Uniform { params: [decay, 0.0, 0.0, 0.0] };
        let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("trail uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trail bind group"),
            layout: &chain.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(current),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&trail.textures[trail.front].1),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&chain.sampler),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("trail encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("trail pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &trail.textures[back].1,
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
            pass.set_pipeline(&chain.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit(Some(encoder.finish()));

        trail.front = back;
        trail.last_time = Some(time);
        Some(&trail.textures[trail.front].1)
    }

    /// Throws every history away. Called when the effect is turned off, so
    /// turning it back on starts from the live frame instead of resuming a
    /// trail left over from whenever it was last on.
    pub fn clear(&mut self) {
        self.per_model.clear();
    }

    /// How many models currently hold a history. Only for tests and for the
    /// question "is this costing anything right now".
    pub fn allocated(&self) -> usize {
        self.per_model.iter().filter(|t| t.is_some()).count()
    }
}

// ---------------------------------------------------------------------------
// `fx.estela`'s own history — see this module's own doc for why it is not
// `Trail` widened.

/// How many past frames [`FrameRing`] keeps.
///
/// Eight, matching `shared::footage::MAX_TAPS` — the same "how far back a
/// freeze reaches" the model family already committed to, not a second
/// number invented for the chain step: `models::estela::MAX_HOLD` (0.55s) at
/// `models::estela::TAP_DT` (1/12s) is about 6.6 taps, and the model's own
/// `generation` clamps to `MAX_TAPS - 1` (7) for exactly that reach — past
/// it, a frozen region pins to the oldest tap rather than reaching further
/// back. This is that same ceiling, reused rather than re-derived.
pub const ESTELA_RING_TAPS: usize = 8;

/// The view every ring slot gets when nothing real is behind it: the current
/// frame, repeated. Two callers need exactly this: a size-degenerate request
/// inside [`EstelaRings::advance`] (nothing was allocated, so there is
/// nothing else to offer), and `app.rs`'s window path, which by its own
/// long-standing design keeps no per-model history of any kind (see that
/// module's doc) and so has nothing *but* the current frame to bind for
/// every slot.
///
/// Filling every slot with *this* frame, rather than leaving them at
/// whatever an uninitialised texture would show, is the ring's whole answer
/// to "what does a frozen block look like before there is history to freeze
/// on": nothing is stale yet, so nothing reads as stale — every tap agrees
/// with tap 0 until real history exists to disagree with it.
pub fn fallback_ring(view: &wgpu::TextureView) -> [&wgpu::TextureView; ESTELA_RING_TAPS] {
    [view; ESTELA_RING_TAPS]
}

/// A ring slot: `TEXTURE_BINDING` so `post.wgsl` can sample it, `COPY_DST` so
/// [`FrameRing`] can write into it with a plain `copy_texture_to_texture` —
/// no `RENDER_ATTACHMENT`, unlike [`crate::gpu::offscreen_target`]: nothing
/// ever draws into a ring slot with a render pass, only copies land here,
/// which is the entire cost `shaders/post.wgsl`'s own note budgets for this
/// ("the ring itself needs no new pass").
fn ring_slot(
    device: &wgpu::Device,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("frame ring slot"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn copy_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &'static str,
    src: &wgpu::Texture,
    dst: &wgpu::Texture,
    w: u32,
    h: u32,
) {
    let origin = wgpu::Origin3d { x: 0, y: 0, z: 0 };
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo { texture: src, mip_level: 0, origin, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyTextureInfo { texture: dst, mip_level: 0, origin, aspect: wgpu::TextureAspect::All },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit(Some(encoder.finish()));
}

/// One model's last [`ESTELA_RING_TAPS`] frames, undecayed — see this
/// module's own doc for why this is a second type rather than [`Trail`]
/// widened.
struct FrameRing {
    textures: [(wgpu::Texture, wgpu::TextureView); ESTELA_RING_TAPS],
    /// Index of the most recently written slot — "0 taps back".
    cursor: usize,
    size: (u32, u32),
    last_time: Option<f32>,
}

impl FrameRing {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        w: u32,
        h: u32,
        format: wgpu::TextureFormat,
        current: &wgpu::Texture,
        time: f32,
    ) -> FrameRing {
        let textures = std::array::from_fn(|_| ring_slot(device, w, h, format));
        let mut ring = FrameRing { textures, cursor: 0, size: (w, h), last_time: Some(time) };
        ring.reset(device, queue, current);
        ring
    }

    /// Copies `current` into every slot and rewinds the cursor to 0 — the
    /// answer both to "before the ring has filled" (called once, from
    /// [`FrameRing::new`]) and to a seek (called again, later, by
    /// [`EstelaRings::advance`]): either way there is no real history yet,
    /// on the far side of a cut or before the first frame, so every tap
    /// shows the live frame until new history accumulates.
    fn reset(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, current: &wgpu::Texture) {
        let (w, h) = self.size;
        for (tex, _) in &self.textures {
            copy_frame(device, queue, "frame ring reset", current, tex, w, h);
        }
        self.cursor = 0;
    }

    /// Writes `current` into the next slot, ageing every existing tap by one.
    fn advance_one(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, current: &wgpu::Texture, time: f32) {
        let (w, h) = self.size;
        let next = (self.cursor + 1) % ESTELA_RING_TAPS;
        copy_frame(device, queue, "frame ring advance", current, &self.textures[next].0, w, h);
        self.cursor = next;
        self.last_time = Some(time);
    }

    /// The [`ESTELA_RING_TAPS`] views, newest first — `views()[0]` is "0
    /// taps back" (this frame's own copy), `views()[ESTELA_RING_TAPS - 1]`
    /// the oldest the ring remembers. Resolved here, on the CPU, so
    /// `post.wgsl` can be handed one fixed slot per tap instead of doing
    /// modulo arithmetic on `cursor` inside the shader.
    fn views(&self) -> [&wgpu::TextureView; ESTELA_RING_TAPS] {
        std::array::from_fn(|j| &self.textures[(self.cursor + ESTELA_RING_TAPS - j) % ESTELA_RING_TAPS].1)
    }
}

/// Every model's [`FrameRing`], allocated on demand — [`Trails`]'s shape,
/// one level over: per model index, because [`crate::headless::Renderer`]
/// has one scratch texture that every tile of a grid draws into in turn, so
/// keying this any other way would mosh one model's blocks against the next
/// model's frames.
#[derive(Default)]
pub struct EstelaRings {
    per_model: Vec<Option<FrameRing>>,
}

impl EstelaRings {
    /// Advances model `index`'s ring with the frame in `current` and hands
    /// back the [`ESTELA_RING_TAPS`] views `effect_estela` reads, newest
    /// first. Always returns something sane and never panics — a
    /// size-degenerate request gets [`fallback_ring`], the same as a caller
    /// with no ring at all — so nothing downstream needs its own branch for
    /// "the ring doesn't exist yet".
    ///
    /// `current_tex`/`current_view` are the same texture, handed in twice:
    /// the copy this performs needs the `Texture`, the fallback path needs
    /// the `TextureView` it would otherwise bind instead.
    #[allow(clippy::too_many_arguments)]
    pub fn advance<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        index: usize,
        current_tex: &wgpu::Texture,
        current_view: &'a wgpu::TextureView,
        size: (u32, u32),
        time: f32,
    ) -> [&'a wgpu::TextureView; ESTELA_RING_TAPS] {
        let (w, h) = size;
        if w == 0 || h == 0 {
            return fallback_ring(current_view);
        }
        if self.per_model.len() <= index {
            self.per_model.resize_with(index + 1, || None);
        }

        let stale = !matches!(&self.per_model[index], Some(r) if r.size == (w, h));
        if stale {
            self.per_model[index] = Some(FrameRing::new(device, queue, w, h, format, current_tex, time));
            return self.per_model[index].as_ref().expect("just built above").views();
        }
        let ring = self.per_model[index].as_mut().expect("checked not stale above");

        let dt = ring.last_time.map(|last| time - last);
        match dt {
            // The same instant, again — determinism: advancing would age the
            // ring once per request, so a paused preview would mosh further
            // while nothing about it had changed.
            Some(d) if d == 0.0 => {}
            // Cannot happen once `stale` has just built one above, but keeps
            // this match total without reaching for an `unwrap`.
            None => {}
            // A seek, a rewind, or a gap big enough that "one tap back" would
            // not be a real historical frame of *this* playback — `SEEK`,
            // reused from ghost's own threshold (this module's doc says why
            // it is one constant, not two).
            Some(d) if !(0.0..=SEEK).contains(&d) => {
                ring.reset(device, queue, current_tex);
                ring.last_time = Some(time);
            }
            // Not a full tap's worth of wall-clock time yet.
            // `models::estela::TAP_DT` is a property of the *effect* — how
            // fast the tube decays — not of how often this renderer happens
            // to be asked for a frame (rendered on demand, not on a fixed
            // loop — this crate's own long-standing shape). Unlike the
            // model, which can evaluate an exact historical instant by
            // arithmetic (`playhead(t - jΔ)` against footage that already
            // exists), this ring can only advance on frames it is actually
            // asked to draw — so below one tap, nothing new has happened
            // yet. `last_time` is left untouched so the *next* call's `dt`
            // is measured from the same instant, rather than resetting the
            // accumulator on every sub-tap call and never reaching a whole
            // one. The disclosed cost: a render cadence coarser than
            // `TAP_DT` (a slow preview poll, say) advances one slot every
            // call regardless, so the ring's reach in wall-clock seconds
            // runs *deeper* than the model's own in that case — an honest
            // consequence of turning "frames a generator actually drew"
            // into history, not something worth a second mechanism to hide.
            Some(d) if d < crate::models::estela::TAP_DT => {}
            Some(_) => ring.advance_one(device, queue, current_tex, time),
        }
        ring.views()
    }

    /// Throws every ring away. Called when nothing's chain wants `fx.estela`
    /// any more, the same reason [`Trails::clear`] exists for ghost.
    pub fn clear(&mut self) {
        self.per_model.clear();
    }

    /// How many models currently hold a ring. Tests, and "is this costing
    /// anything right now" — [`Trails::allocated`]'s reason.
    pub fn allocated(&self) -> usize {
        self.per_model.iter().filter(|r| r.is_some()).count()
    }
}
