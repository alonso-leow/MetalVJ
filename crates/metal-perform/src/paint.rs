//! Where the pictures come from.
//!
//! The app asks for frames; something draws them. Until now that something was
//! `metal_render::headless::Renderer` and the app named it directly. It is now a
//! trait with two implementations, for the same reason `digest::Source` is one:
//!
//! > *"`digest::Simulated` stands where microphone and loopback capture
//! > will go […] When the real digester lands it implements `digest::Source`
//! > and one struct is deleted."*
//!
//! Same shape, different axis. [`Gpu`] is the real thing — the wgpu models,
//! unchanged, however many `metal_render::registry` currently holds.
//! [`crate::stand::Stand`] is a stand-in that draws on the CPU, for hosts with
//! no adapter.
//!
//! # Why a host with no adapter matters
//!
//! Because the operator's surface has to be reviewable by somebody who is not
//! sitting at the machine that renders. The overlay system this trait was
//! introduced alongside is a *layout* feature — sectors aimed at doors,
//! stacking order, opacity, which of two crossing rectangles wins — and all of
//! that is judged by looking at the app on a phone, at a laptop, at whatever
//! screen the person reviewing it has. Requiring a GPU to look at the app would
//! mean the one thing that needs the widest review has the narrowest.
//!
//! **What is a stand-in and what is not**, precisely, because the difference is
//! the whole honesty of this:
//!
//! | | on a venue laptop | on a host with no GPU |
//! |---|---|---|
//! | the digest | simulated | simulated |
//! | the mapping (digest → floats) | real | **real** |
//! | the router | real | **real** |
//! | the Curator and the Colourist | real | **real** |
//! | indexing a folder of footage | real | **real** |
//! | the compositor: opacity, z, blend, sectors | real | **real** |
//! | the contours (`apply_masks`) | real | **real** |
//! | the renderers themselves | real | stand-in |
//!
//! One row of that table is a stand-in. Everything the overlay system *is*
//! runs for real, on the same code, in both. The page says which it is looking
//! at, in the same place and the same words it already says the digest is not
//! real — see `/state.paint`.
//!
//! **A transmission is the one model where that row bites harder**, and it is
//! worth being exact about. On a host with no adapter the library still
//! indexes, the Curator still chooses a passage, the Colourist still writes the
//! row and the page still reports all three — but [`Paint::reel`] hands back
//! `None`, so no footage reaches a renderer and what is drawn is a stand-in
//! figure. Everything about *deciding* is real; the picture is not, and
//! `real()` already says so.

use std::collections::{HashMap, HashSet};

use metal_schema::Schema;

/// How a surface picture is laid on a part — the plain twin of
/// `metal_render::model::Mapping`, here so the trait stays nameable in the
/// no-GPU build (see [`Paint::set_surfaces`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SurfaceMapping {
    /// Triplanar in object space: the picture follows the mesh.
    Object,
    /// Screen-space: the picture is projected onto the part.
    Projector,
}

/// One part's picture for [`Paint::set_surfaces`]: RGBA8 `pixels` of
/// `width x height`, the `mix` (0..1), `mapping` and `scale` the consumer's
/// `config.surfaces.<part>` asked for.
#[derive(Clone, Copy, Debug)]
pub struct SurfacePicture<'a> {
    pub pixels: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub mix: f32,
    pub mapping: SurfaceMapping,
    pub scale: f32,
}

/// One part's material for [`Paint::set_surfaces`], the plain twin of
/// `metal_render::model::MaterialSpec` (no-GPU build). The row's `material`
/// group resolved for the part, in `metal_schema::MATERIAL_FLOATS` order.
#[derive(Clone, Copy, Debug, Default)]
pub struct SurfaceMaterial {
    pub kind: f32,
    pub mix: f32,
    pub scale: f32,
    pub rough: f32,
    pub metal: f32,
    pub bump: f32,
    pub env: f32,
    pub tint: f32,
    pub tint_hue: f32,
    pub glitch: f32,
    pub glitch_rate: f32,
    pub glitch_seed: f32,
}

/// The two things a part can carry: an optional picture and an optional
/// material, `None`/`None` for nothing. The plain twin of
/// `metal_render::model::SurfaceSpec`, the plan's one widening of the surface
/// argument so it can carry a material too.
#[derive(Clone, Copy, Debug, Default)]
pub struct SurfaceSpec<'a> {
    pub image: Option<SurfacePicture<'a>>,
    pub material: Option<SurfaceMaterial>,
}

/// What one call to [`Paint::run`] produced.
///
/// **`frames` only holds buffers a sink actually reads.** A generator whose
/// output only ever feeds an `fx.*` node that nothing downstream consumes
/// (an orphaned chain) is still rendered — `run` cannot know that is dead
/// weight without re-deriving the whole graph — but it is not kept past the
/// end of the walk. `readbacks` is the number this crate's B1 test counts:
/// how many times this call actually asked a painter for pixels, which is
/// the number `Paint::solo` used to be called once per source per frame and
/// this is meant to shrink.
pub struct RunOutput {
    /// Rendered pixels, keyed by the plan's buffer index, for every buffer a
    /// sink node (a node whose own [`metal_graph::Op::buffer`] is `None` —
    /// `output.device`, `export.video`, `mask`, `router`, `cam.track`) reads.
    pub frames: HashMap<usize, (Vec<u8>, u32, u32)>,
    /// How many times a renderer was actually asked for pixels this run.
    pub readbacks: usize,
    /// Every `gen.*` node's resolved `fx.*` chain — `the design notes`
    /// §3.2/§10 Set E, and the whole point of `crate::resolved_chain`.
    ///
    /// Each entry is `metal_schema::Post::resolved_chain()` for that node: the
    /// operator's `fx.*` nodes, in graph order, with `fijo` amounts baked in
    /// and `reactivo` ones left as `drive::row` (or raw ingest) computed
    /// them. It is handed straight to [`Paint::solo_chain`] at the call site
    /// below, which is what makes dragging an `fx.*` node change the picture
    /// and not just the diagram.
    ///
    /// Still published on `RunOutput` rather than kept local, because it is
    /// the only place a caller can see what order a generator actually
    /// resolved to — the graph says what the operator drew, this says what
    /// was rendered, and a test that wants to assert the two agree needs
    /// both.
    pub chains: HashMap<metal_graph::NodeId, Vec<metal_schema::EffectStep>>,
    /// **Material tiles** (23-sept-2026, addendum D7): the rendered pixels of
    /// every chain wired to a *material* input port, keyed by
    /// `(consuming node, port name)` — for the first one, `(mask_node,
    /// "stroke")`.
    ///
    /// A material source is a **secondary sink**: it is a picture of its own
    /// that a node samples (the mask's outline) rather than a layer of the
    /// device, so it never reaches an `output.device` and would be dropped by
    /// the `frames` filter below. Keeping it here is the entire point of the
    /// port being declared material — the consumer
    /// (`crate::output::apply_mask_strokes`) reads its tile out of this map
    /// instead of from `frames`.
    ///
    /// Keyed by the *consumer*, not the source, because that is what the
    /// consumer knows: a mask node asks "what is wired to my `stroke`", and
    /// answers with one lookup, no walk of the graph at paint time.
    pub materials: HashMap<(metal_graph::NodeId, String), (Vec<u8>, u32, u32)>,
    /// G1: generator buffers that stayed textures (no readback) because
    /// every consumer is a compose/output sink. Same keys as `frames`, never
    /// both for one buffer. Empty unless the run was asked for textures
    /// ([`Paint::run_full`]) on a [`crate::backend::Backend::Gpu`] painter.
    pub textures: HashMap<usize, crate::gpu_frame::GpuFrame>,
}

/// S.W: the `fx.text.*` nodes downstream of a `text.source`, in graph order,
/// as the renderer's effect steps. Walks forward over `image` edges only — a
/// `control` edge is not what carries the glyphs.
/// The image fx a GPU pass can run over a texture (G2): the forge effect
/// contract and the built-in passes `fx_on_texture` dispatches.
fn gpu_fx_kind(k: &str) -> bool {
    metal_graph::parity::is_parity_effect(k)
        || k == crate::forge::FX_KIND
        || k == crate::motion::FX_MOVE_KIND
        || k == crate::fx::particles::FX_PARTICLES_KIND
        || k == crate::fx::flash::FX_FLASH_KIND
        || k == crate::fx::image_pop::FX_IMAGE_POP_KIND
        || k == crate::fx::warp::FX_WARP_KIND
}

/// An `fx.*` node the generator's own shader bakes (G5): on the CPU the fx
/// branch of `run_full` has no code for it, so its picture is its input's.
fn baked_fx_kind(k: &str) -> bool {
    k.starts_with("fx.") && !gpu_fx_kind(k)
}

/// Whether every consumer of `node`'s picture can take it as a texture: a
/// sink, a GPU image pass, or a baked effect whose own consumers can (G5).
/// Any other consumer reads the picture back, so the producer must too.
///
/// **The device's `venue` output is a sink, not a pass.** It is the wire
/// after an `output.device` (2026-10-22): the finished frame offered to a
/// `venue.surface` as a view, rendered by nothing — the device already has no
/// buffer (`metal_graph::compile`), so a `venue` cable off it cannot make a
/// producer read back. Ignoring it here means the extra cable never changes
/// whether the picture stays a texture, which is what keeps the frame
/// byte-identical with and without it.
fn tex_consumers_ok(graph: &metal_graph::Graph, node: &str, depth: usize) -> bool {
    if depth > 64 {
        return false;
    }
    let mut any = false;
    for e in graph.edges.iter().filter(|e| e.from.node == node) {
        if e.from.port == "venue" {
            any = true;
            continue;
        }
        let Some(to) = graph.node(&e.to.node) else { return false };
        let sink = to.kind.starts_with("output.") || metal_graph::is_compose_kind(&to.kind);
        let pass = e.to.port == "image"
            && (gpu_fx_kind(&to.kind)
                || (baked_fx_kind(&to.kind) && tex_consumers_ok(graph, &e.to.node, depth + 1))
                // A `mat.*` node passes its picture through, so it is a texture
                // consumer exactly when whatever follows it is.
                || (to.kind.starts_with(metal_graph::materials::PREFIX)
                    && tex_consumers_ok(graph, &e.to.node, depth + 1)));
        if !(sink || pass) {
            return false;
        }
        any = true;
    }
    any
}

fn text_effects_for(
    graph: &metal_graph::Graph,
    source: &metal_graph::NodeId,
) -> Vec<crate::text::TextEffectStep> {
    let mut out = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut queue: Vec<&str> = vec![source.as_str()];
    while let Some(id) = queue.pop() {
        if !seen.insert(id) {
            continue;
        }
        for edge in graph.edges.iter().filter(|e| e.from.node == id && e.from.port == "image") {
            let Some(next) = graph.node(&edge.to.node) else { continue };
            if let Some(name) = next.kind.strip_prefix("fx.text.") {
                let amount = serde_json::from_str::<serde_json::Value>(&next.config.to_json_str())
                    .ok()
                    .and_then(|v| v.get("amount").and_then(|a| a.as_f64()))
                    .map(|a| a as f32)
                    .unwrap_or(1.0);
                out.push(crate::text::TextEffectStep { kind: name.to_string(), amount });
            }
            queue.push(edge.to.node.as_str());
        }
    }
    out
}

/// Something that can draw the suite.
///
/// Deliberately the shape `Renderer` already had, so the GPU implementation is
/// a forwarding struct and not a rewrite. The two extra methods are the ones a
/// caller needs to be honest about what it is showing.
/// `Send` on native, nothing on the browser (see `metal_render::model::MaybeSend`:
/// the same bound, the same reason).
#[cfg(not(target_arch = "wasm32"))]
pub trait PaintBound: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> PaintBound for T {}
#[cfg(target_arch = "wasm32")]
pub trait PaintBound {}
#[cfg(target_arch = "wasm32")]
impl<T> PaintBound for T {}

/// `Send` is a supertrait for the same reason `metal_render::model::Model`
/// gained one in `S.U7b`: `Shared` (metal-dash's shared handle) holds a `Box<dyn Paint>`
/// and needs to be `Send` so it can live behind an `Arc<Mutex<…>>` shared
/// between the winit thread and the HTTP listener thread. [`Gpu`] is
/// `Send` once `Model: Send` makes its `metal_render::headless::Renderer`
/// field `Send`; [`crate::stand::Stand`] is plain CPU data and always was.
pub trait Paint: PaintBound {
    fn model_count(&self) -> usize;
    fn ids(&self) -> Vec<&'static str>;
    fn names(&self) -> Vec<&'static str>;
    fn schemas(&self) -> Vec<&'static Schema>;

    /// One model, at the size asked for. Returns RGBA8 plus its size.
    ///
    /// `background_alpha` is the source's `Layer::fondo_alpha` —
    /// whether the model's `fondo` slot (for the twelve models that use the
    /// shared `Backdrop` pass) should render as alpha instead of its own
    /// colour. Meaningless, and ignored, for every other model.
    fn solo(
        &mut self,
        index: usize,
        floats: &[f32],
        time: f32,
        w: u32,
        h: u32,
        background_alpha: bool,
    ) -> (Vec<u8>, u32, u32);

    /// The same, with the effect chain the graph resolved for this generator.
    ///
    /// Defaulted to `solo` rather than made required, because an empty chain
    /// and a missing chain mean the same thing downstream — `resolved_chain`
    /// rebuilds the legacy declaration order out of the row's own `Post` when
    /// it is handed nothing. So a `Paint` that does not model order (the test
    /// double, anything drawing a single model with no graph behind it) stays
    /// correct by doing nothing, instead of being forced to carry a parameter
    /// it has no way to honour.
    fn solo_chain(
        &mut self,
        index: usize,
        floats: &[f32],
        time: f32,
        w: u32,
        h: u32,
        background_alpha: bool,
        _chain: &[metal_schema::EffectStep],
    ) -> (Vec<u8>, u32, u32) {
        self.solo(index, floats, time, w, h, background_alpha)
    }

    /// A `forge` source: `wgsl` (the shader `key` names) drawn with this
    /// row. Defaulted to the model's ordinary draw, which is what the CPU
    /// stand-in can honestly do — it has no shader compiler, so a forge
    /// layer there is its placeholder figure like every other model.
    #[allow(clippy::too_many_arguments)]
    fn forge(
        &mut self,
        index: usize,
        _key: &str,
        _wgsl: &str,
        floats: &[f32],
        time: f32,
        w: u32,
        h: u32,
        background_alpha: bool,
        chain: &[metal_schema::EffectStep],
    ) -> (Vec<u8>, u32, u32) {
        self.solo_chain(index, floats, time, w, h, background_alpha, chain)
    }

    /// An `fx.forge` node: a custom effect shader over a finished frame.
    /// `None` passes the frame through untouched — the stand-in's answer,
    /// and the GPU's for a shader that does not check (logged, not fatal: a
    /// broken effect must not black out the source it sits on).
    fn forge_fx(
        &mut self,
        _key: &str,
        _wgsl: &str,
        _frame: &(Vec<u8>, u32, u32),
        _time: f32,
        _ingest: &[f32],
    ) -> Option<Vec<u8>> {
        None
    }

    /// An `fx.warp` node (loops T12): `frame` displaced by `warp`'s mesh
    /// field, moshed against `key`'s history. The default is the CPU path
    /// (`crate::fx::warp::apply_cpu`), which the stand-in uses as is; the GPU
    /// painter runs `metal_render`'s pass and falls back here on an error.
    fn warp_fx(&mut self, key: &str, warp: &metal_schema::warp::Warp, frame: &(Vec<u8>, u32, u32)) -> Vec<u8> {
        crate::fx::warp::apply_cpu(key, warp, frame)
    }

    /// P-E: a source's finished frame inside its in/out transition, drawn at
    /// visibility `v`. Defaulted to the CPU pass ([`crate::transition::apply`],
    /// the `Stand`/wasm path and the reference); the GPU painter overrides it.
    fn transition(&mut self, spec: &crate::transition::Spec, v: f32, frame: &mut [u8], w: u32, h: u32, seed: u32) {
        crate::transition::apply(spec, v, frame, w, h, seed)
    }

    /// G1: which half composes. Chosen once at startup (see
    /// [`crate::backend`]); the CPU stand-in is always [`Backend::Cpu`].
    fn backend(&self) -> crate::backend::Backend {
        crate::backend::Backend::Cpu
    }

    /// G1: the shared device context a [`crate::gpu_compose::GpuPass`] runs
    /// with. `None` on a CPU backend.
    #[cfg(feature = "gpu")]
    fn gpu_cx(&mut self) -> Option<crate::gpu_compose::GpuCx> {
        None
    }

    /// G3: a handle to the device this painter composes on, for a window that
    /// presents the compose texture directly. `None` on a CPU backend (the
    /// window then keeps fetching JPEGs).
    #[cfg(feature = "gpu")]
    fn shared_gpu(&mut self) -> Option<metal_render::gpu::Gpu> {
        None
    }

    /// G1: [`Paint::solo_chain`] left as a texture on the shared device.
    /// `None` means "not on this painter": the caller reads pixels instead.
    #[allow(clippy::too_many_arguments)]
    fn solo_texture(
        &mut self,
        _index: usize,
        _floats: &[f32],
        _time: f32,
        _w: u32,
        _h: u32,
        _background_alpha: bool,
        _chain: &[metal_schema::EffectStep],
    ) -> Option<crate::gpu_frame::GpuFrame> {
        None
    }

    /// G1: [`Paint::forge`] as a texture.
    #[allow(clippy::too_many_arguments)]
    fn forge_texture(
        &mut self,
        _index: usize,
        _key: &str,
        _wgsl: &str,
        _floats: &[f32],
        _time: f32,
        _w: u32,
        _h: u32,
        _background_alpha: bool,
        _chain: &[metal_schema::EffectStep],
    ) -> Option<crate::gpu_frame::GpuFrame> {
        None
    }

    /// G1: the layer stack composed on the GPU. `None` on a CPU backend.
    fn compose_gpu(
        &mut self,
        _w: u32,
        _h: u32,
        _mode: crate::layers::Composition,
        _tiles: &[crate::gpu_frame::GpuTile],
    ) -> Option<crate::gpu_frame::GpuFrame> {
        None
    }

    /// G3: a device-sized coverage map for `key`, built by `make` (row-major
    /// `w * h` bytes) on a miss and cached while the key repeats. `None` on a
    /// CPU backend.
    fn cover_gpu(
        &mut self,
        _key: u64,
        _w: u32,
        _h: u32,
        _make: &mut dyn FnMut() -> Vec<u8>,
    ) -> Option<crate::gpu_frame::CoverRef> {
        None
    }

    /// G3: `frame` through one whole-frame [`Shade`] pass, still a texture.
    fn shade_gpu(
        &mut self,
        _frame: &crate::gpu_frame::GpuFrame,
        _shade: &crate::gpu_frame::Shade,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        None
    }

    /// G3: queue a copy of `frame` now and get back a closure that waits for
    /// it (on any thread) and returns the RGBA8 bytes - for a sink that needs
    /// exactly this frame (a PNG export) without holding the painter's lock.
    fn readback_deferred(&mut self, _frame: &crate::gpu_frame::GpuFrame) -> Option<Box<dyn FnOnce() -> Vec<u8> + Send>> {
        None
    }

    /// G3: the sink's non-blocking read. Enqueues a copy of `frame` under the
    /// sink's `key` and returns the newest copy that has finished, a frame or
    /// two old, or `None` while none has. Never waits for the GPU.
    fn readback_async(&mut self, _key: &str, _frame: &crate::gpu_frame::GpuFrame) -> Option<(Vec<u8>, u32, u32)> {
        None
    }

    /// G3: the largest compose size this painter can hold (adapter texture
    /// limit, memory pressure); the engine renders there and upscales.
    fn fit_size(&mut self, w: u32, h: u32) -> (u32, u32) {
        (w, h)
    }

    /// G3: `frame` multiplied by the edge-blend `map` on the GPU.
    fn edge_blend_gpu(
        &mut self,
        _frame: &crate::gpu_frame::GpuFrame,
        _map: &std::sync::Arc<crate::edge_blend::BlendMap>,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        None
    }

    /// G3: `frame` resized (nearest) to `w x h` on the GPU.
    fn upscale_gpu(&mut self, _frame: &crate::gpu_frame::GpuFrame, _w: u32, _h: u32) -> Option<crate::gpu_frame::GpuFrame> {
        None
    }

    /// G1: a frame's bytes, for a sink that needs CPU pixels. Blocks; the
    /// frame loop uses [`crate::gpu_compose::AsyncReadback`] where it can.
    fn readback(&mut self, _frame: &crate::gpu_frame::GpuFrame) -> Vec<u8> {
        Vec::new()
    }

    /// G2: [`Paint::transition`] over a texture tile, result a new texture.
    /// `None` means "not on this painter": the caller reads pixels instead.
    fn transition_texture(
        &mut self,
        _spec: &crate::transition::Spec,
        _v: f32,
        _input: &crate::gpu_frame::GpuFrame,
        _seed: u32,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        None
    }

    /// G2: [`Paint::text`] drawn straight into a texture (no readback). `None`
    /// on a CPU backend or when the glyphs could not be prepared.
    fn text_texture(
        &mut self,
        _config: &crate::text::TextConfig,
        _row: &[f32],
        _time: f32,
        _w: u32,
        _h: u32,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        None
    }

    /// G2: [`Paint::forge_fx`] over a texture. `srgb_views` keeps the legacy
    /// linear-light behaviour of `fx.forge`-style shaders; `false` is the
    /// display-space maths of the built-in parity shaders. `None`: not here.
    #[allow(clippy::too_many_arguments)]
    fn forge_fx_texture(
        &mut self,
        _key: &str,
        _wgsl: &str,
        _input: &crate::gpu_frame::GpuFrame,
        _time: f32,
        _ingest: &[f32],
        _srgb_views: bool,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        None
    }

    /// G2: the parity nodes that need more than the forge effect contract
    /// binds: `fx.lookup` (`input` is the picture) and `gen.ramp` (`input` is
    /// `None`, `w x h` is the size). `None`: not here.
    fn parity_texture(
        &mut self,
        _kind: &str,
        _cfg: &serde_json::Value,
        _root: &std::path::Path,
        _input: Option<&crate::gpu_frame::GpuFrame>,
        _w: u32,
        _h: u32,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        None
    }

    /// G2: the pixel window `win` (a layer `span`) of a texture as its own
    /// texture, a rectangle copy. `None` when the window is the whole frame or
    /// this painter has no textures.
    fn crop_texture(&mut self, _frame: &crate::gpu_frame::GpuFrame, _win: &crate::layers::Sector) -> Option<crate::gpu_frame::GpuFrame> {
        None
    }

    /// G2: `fx.flash` / `fx.image-pop` / `fx.warp` over a texture (`None`: not
    /// here, the caller reads back and uses the CPU code). `id` keys the warp's
    /// mosh history.
    fn special_fx_texture(
        &mut self,
        _node: &metal_graph::Node,
        _id: &str,
        _input: &crate::gpu_frame::GpuFrame,
        _clock: f32,
        _root: &std::path::Path,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        None
    }

    /// G2: a CPU picture kept as a texture under `key`, re-uploaded only when
    /// `version` changes (a `still`'s config, a `capture`'s frame number).
    /// `make` produces `(rgba, w, h)` and runs only on a miss. `None`: not here
    /// or nothing to show.
    fn cached_texture(
        &mut self,
        _key: &str,
        _version: u64,
        _make: &mut dyn FnMut() -> Option<(Vec<u8>, u32, u32)>,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        None
    }

    /// G2: runs the image fx `node` over a texture, or `None` when it has no
    /// pass here (the caller then reads the texture back and uses the CPU
    /// code). Only kinds [`Paint::run_full`] lists as texture-capable arrive.
    #[allow(clippy::too_many_arguments)]
    fn fx_on_texture(
        &mut self,
        graph: &metal_graph::Graph,
        node: &metal_graph::Node,
        id: &str,
        input: &crate::gpu_frame::GpuFrame,
        clock: f32,
        raw_digest: &[f32],
        root: &std::path::Path,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        let kind = node.kind.as_str();
        let (shader, ingest, srgb) = if kind == crate::motion::FX_MOVE_KIND {
            let (sid, ingest) = crate::motion::move_node(graph, id)?;
            (sid, ingest.to_vec(), true)
        } else if kind == crate::fx::particles::FX_PARTICLES_KIND {
            let (sid, ingest) = crate::fx::particles::particles_node(graph, id)?;
            (sid, ingest.to_vec(), true)
        } else if kind == crate::forge::FX_KIND {
            (crate::forge::node_shader(node)?, crate::forge::fx_ingest(node, raw_digest), true)
        } else if kind == metal_graph::parity::LOOKUP_KIND {
            return self.parity_texture(kind, &crate::fx::parity::config(node), root, Some(input), input.width(), input.height());
        } else if metal_graph::parity::is_parity_effect(kind) {
            let cfg = crate::fx::parity::config(node);
            (crate::fx::parity::shader_of(kind)?.to_string(), crate::fx::parity::ingest(kind, &cfg), false)
        } else {
            return self.special_fx_texture(node, id, input, clock, root);
        };
        let wgsl = match crate::forge::source(root, &shader) {
            Ok(w) => w,
            Err(e) => {
                crate::forge::report(&shader, &e);
                return None;
            }
        };
        self.forge_fx_texture(&shader, &wgsl, input, clock, &ingest, srgb)
    }

    /// S.W: one `text.source` node's frame.
    ///
    /// Defaulted to the shared CPU rasterizer, deliberately: both painters
    /// already hand frames around as raw RGBA, so there is no GPU text
    /// pipeline to keep in step with a CPU one. The config arrives parsed
    /// ([`crate::text::TextConfig`]) and the node's own `fx.text.*` effects
    /// are already folded into it by [`Paint::run`].
    fn text(
        &mut self,
        config: &crate::text::TextConfig,
        row: &[f32],
        time: f32,
        w: u32,
        h: u32,
    ) -> (Vec<u8>, u32, u32) {
        crate::text::render(config, row, time, w, h)
    }

    /// Renders a compiled [`metal_graph::Plan`] once, keeping intermediates as
    /// in-memory buffers rather than reading each one back from the GPU —
    /// `the design notes` §10 Set B1.
    ///
    /// **Why this is a provided method and not one each painter overrides.**
    /// It is written once, on top of [`Paint::solo`], so [`crate::stand::Stand`]
    /// gets it for free and cannot drift from the GPU path — the CPU stand-in
    /// has to keep up with whatever this does, per B1's own warning, and the
    /// cheapest way to guarantee that is for there to be only one
    /// implementation.
    ///
    /// **What actually reads back pixels today, and why it is not yet every
    /// node kind.** Only `gen.*` nodes call [`Paint::solo`] — one readback
    /// each, however many sinks their output fans out to, because
    /// `plan.ops` already deduplicated that (`metal-graph`'s `compile`, A5).
    /// `fx.*` nodes forward their input buffer untouched, and that is not a
    /// stub — it is where the work moved to. Their *identity and order* are
    /// collected into [`RunOutput::chains`] and handed to the generator's own
    /// post pass via [`Paint::solo_chain`], so the chain composes inside the
    /// one pass `solo` was already running rather than costing a texture
    /// round trip per node (§3.2, §11 point 2 — `apply_node_post`, per-layer
    /// rather than the single `apply_master_post` this used to name,
    /// the design notes PART IV §17.4 Set E). So a generator with
    /// two chained `fx.*` nodes still costs exactly one readback, which is
    /// what B1's gate asks for — and unlike before, the two nodes now
    /// genuinely compose in the order they were drawn.
    ///
    /// The disclosed exception, unchanged: `blur`/`sharpen`/`crt` read their
    /// neighbour taps from the pre-chain texture, because chaining those
    /// needs a real multi-pass ping-pong that this single pass does not do.
    /// The centre tap does reflect the chain. `router` / `mask` /
    /// `output.device` / `export.video` / `audio.*` / `cam.track` / `sensor.*`
    /// produce no pixels of their own here — ordering, opacity, blend and
    /// clip are still `compose.rs`'s job on top of what this returns.
    ///
    /// `background_alpha` mirrors `rows`: one entry per generator node, carrying
    /// its source's `Layer::fondo_alpha` through to the
    /// [`Paint::solo`] call this makes on its behalf. A node missing from the
    /// map (there is no live caller that leaves one out today, but `Stand`'s
    /// own tests do) draws with its backdrop opaque, same as before this
    /// field existed.
    /// Hands model `index` the surfaces of its named parts (plan: surface
    /// sources, materials M3). `parts` lists **every** part the model declares,
    /// with `None` for one that carries nothing, so a model instance shared by
    /// two nodes never carries one node's picture or material into the other's
    /// frame. Defaulted to a no-op: the CPU stand-in draws its placeholder and
    /// has no surfaces.
    fn set_surfaces(&mut self, _index: usize, _parts: &[(&str, Option<SurfaceSpec>)]) {}

    fn run(
        &mut self,
        graph: &metal_graph::Graph,
        plan: &metal_graph::Plan,
        rows: &HashMap<metal_graph::NodeId, Vec<f32>>,
        background_alpha: &HashMap<metal_graph::NodeId, bool>,
        raw_digest: &[f32],
        time: f32,
        w: u32,
        h: u32,
    ) -> RunOutput {
        self.run_with_clocks(
            graph,
            plan,
            rows,
            background_alpha,
            raw_digest,
            time,
            w,
            h,
            &HashMap::new(),
            &HashMap::new(),
        )
    }

    /// [`Paint::run`] with a per-generator clock (K, `fx.speed`).
    ///
    /// `gen_time` is what each `gen.*` node draws at this frame — the engine's
    /// accumulated, speed-scaled time from [`crate::motion::Clocks`]. A node
    /// missing from the map draws at the flat show `time`, so every existing
    /// caller (`run` above, the tests, anything with no graph clock) is
    /// unchanged: the empty map *is* the old behaviour.
    ///
    /// The signature grew a map rather than the engine wrapping `run` because
    /// the time reaches the renderers here, one `solo_chain`/`forge` call per
    /// generator, and splitting them across two walks is the drift this one
    /// method exists to prevent.
    #[allow(clippy::too_many_arguments)]
    fn run_with_clocks(
        &mut self,
        graph: &metal_graph::Graph,
        plan: &metal_graph::Plan,
        rows: &HashMap<metal_graph::NodeId, Vec<f32>>,
        background_alpha: &HashMap<metal_graph::NodeId, bool>,
        raw_digest: &[f32],
        time: f32,
        w: u32,
        h: u32,
        gen_time: &HashMap<metal_graph::NodeId, f32>,
        gen_size: &HashMap<metal_graph::NodeId, (u32, u32)>,
    ) -> RunOutput {
        self.run_full(graph, plan, rows, background_alpha, raw_digest, time, w, h, gen_time, gen_size, false)
    }

    /// [`Paint::run_with_clocks`] that may keep a generator's picture as a
    /// texture (G1): with `textures` on a GPU-backend painter, a `gen.*`
    /// buffer whose every consumer is an `output.*` or `compose.*` node lands
    /// in [`RunOutput::textures`] and costs no readback. Everything else
    /// (anything an `fx.*` node, a mask or a material reads) is still a CPU
    /// buffer in `frames` until wave 2 moves those nodes onto passes.
    #[allow(clippy::too_many_arguments)]
    fn run_full(
        &mut self,
        graph: &metal_graph::Graph,
        plan: &metal_graph::Plan,
        rows: &HashMap<metal_graph::NodeId, Vec<f32>>,
        background_alpha: &HashMap<metal_graph::NodeId, bool>,
        // The single digest this frame shares (§6 invariant 1), unshaped —
        // `digest::Frame::raw`. Only consulted for an effect
        // `metal_graph::effects::ControlSource::RawIngest` names — axis 2b, see
        // `crate::resolved_chain`. An empty slice (a caller with no live
        // digest at all — the asset grid, most of this trait's own tests)
        // just leaves every effect's row-computed value in place.
        raw_digest: &[f32],
        time: f32,
        w: u32,
        h: u32,
        gen_time: &HashMap<metal_graph::NodeId, f32>,
        // P2: the size one generator renders at, when it is not the device's.
        // A source pinned to a `venue.surface` renders at the projected
        // contour's bounding box × 1.5 (`engine::surface_place`), so a patch
        // of wall does not cost a device-sized buffer. A node missing from
        // the map draws at the flat `(w, h)`, so every existing caller is
        // unchanged.
        gen_size: &HashMap<metal_graph::NodeId, (u32, u32)>,
        textures: bool,
    ) -> RunOutput {
        // A buffer the composition step actually reads: any input of a node
        // whose own output is not itself consumed further (`buffer.is_none()`),
        // **or of a `compose.*` mixer** (LAY.7c). The mixer's members are its
        // `image` inputs and the mixed frame is produced where the per-layer
        // blend rule lives — `render_device`, which owns the `Layer`s — so the
        // members' buffers have to survive this filter exactly as a sink's
        // inputs do. Without the second clause a `gen → compose →
        // output.device` graph would hand back no member frames and the device
        // would compose from nothing: a black screen, in range, silent.
        let mut sink_inputs: HashSet<usize> = plan
            .ops
            .iter()
            .filter(|op| {
                op.buffer.is_none()
                    || graph
                        .node(&op.node)
                        .map(|n| metal_graph::is_compose_kind(&n.kind))
                        .unwrap_or(false)
            })
            .flat_map(|op| op.inputs.iter().copied())
            .collect();

        // Node -> buffer, so the `fx.*` branch below can find the specific
        // edge it needs (its `image` predecessor) instead of trusting
        // `op.inputs.first()` — that stopped being safe to trust the moment
        // an `fx.*` node could carry a *second* incoming edge (`control`,
        // for `reactivo`): `Op.inputs` is sorted by `(to.port, from.node)`
        // (`metal_graph::compile`), and `"control" < "image"` lexicographically,
        // so `.first()` would silently hand back the control source's
        // buffer — which nothing ever renders into — instead of the image
        // predecessor's. A generator's `router` output (axis 2a) makes the
        // same trap reachable one hop earlier. Node identity sidesteps the
        // sort order entirely.
        let buffer_of: HashMap<&str, usize> = plan
            .ops
            .iter()
            .filter_map(|op| op.buffer.map(|b| (op.node.as_str(), b)))
            .collect();

        // Material edges (D7): `(consumer node, port) -> source node`, in
        // graph order. Read once here so the sink filter and the `materials`
        // map at the end cannot disagree on which chains are material — a
        // second walk is the drift this port exists to remove.
        //
        // **A material source joins the kept set.** A chain wired only to a
        // `stroke` port has no `output.device` ahead of it, so without this
        // its tile would be rendered (every producing node is walked below)
        // and then thrown away — "no frame to sample", the exact bug this
        // lot closes. Defensive rather than load-bearing today: a `mask` node
        // usually has no consumer of its own `image`, so its inputs are
        // already kept by the `buffer.is_none()` clause; it stops being free
        // the moment one is chained after it.
        let material_edges: Vec<(metal_graph::NodeId, String, &str)> = graph
            .edges
            .iter()
            .filter_map(|e| {
                let kind = graph.node(&e.to.node)?.kind.as_str();
                metal_graph::is_material_port(kind, &e.to.port)
                    .then(|| (e.to.node.clone(), e.to.port.clone(), e.from.node.as_str()))
            })
            .collect();
        for (_, _, source) in &material_edges {
            if let Some(b) = buffer_of.get(*source) {
                sink_inputs.insert(*b);
            }
        }

        // G1: buffers whose every consumer is a compose/output sink — the only
        // ones that can stay textures while `fx.*`, masks and materials still
        // read pixels.
        // G2: ... or an image fx that runs as a pass over a texture (the forge
        // effect contract, `fx.lookup`), so a chain of them never leaves the GPU.
        let texture_ok = |node: &str| tex_consumers_ok(graph, node, 0);
        let textures = textures && self.backend() == crate::backend::Backend::Gpu;
        let mut tex_out: HashMap<usize, crate::gpu_frame::GpuFrame> = HashMap::new();
        let ids = self.ids();
        let mut produced: HashMap<usize, (Vec<u8>, u32, u32)> = HashMap::new();
        let mut chains: HashMap<metal_graph::NodeId, Vec<metal_schema::EffectStep>> = HashMap::new();
        let mut readbacks = 0usize;

        for op in &plan.ops {
            let Some(node) = graph.node(&op.node) else {
                continue;
            };
            let Some(buf) = op.buffer else { continue };
            // P2: a generator pinned to a surface renders at its own size
            // (`engine::surface_place`); every other node draws at the device.
            let (w, h) = gen_size.get(&op.node).copied().unwrap_or((w, h));

            if node.kind == metal_graph::RAMP_KIND {
                // Parity P-A: the gradient source — not a model (rule 1),
                // rasterised here on every painter.
                let cfg = crate::fx::parity::config(node);
                let root = crate::output::data_root();
                if textures && texture_ok(&op.node) {
                    if let Some(t) = self.parity_texture(metal_graph::RAMP_KIND, &cfg, &root, None, w, h) {
                        tex_out.insert(buf, t);
                        continue;
                    }
                }
                let px = crate::fx::parity::ramp(&cfg, w, h, &root);
                produced.insert(buf, (px, w, h));
            } else if let Some(model_id) = node.kind.strip_prefix("gen.") {
                let index = ids.iter().position(|id| *id == model_id).unwrap_or(0);
                let mut row = rows.get(&op.node).cloned().unwrap_or_default();
                let alpha = background_alpha.get(&op.node).copied().unwrap_or(false);
                // K: `fx.speed` draws this generator at its own accumulated
                // clock rather than the show clock — see `run_with_clocks`.
                let node_time = gen_time.get(&op.node).copied().unwrap_or(time);

                // §3.2/B5: resolve this generator's `fx.*` chain from the
                // graph, in graph order — see `crate::chain_for_row`.
                let resolved = crate::chain_for_row(graph, &op.node, &mut row, raw_digest);
                chains.insert(op.node.clone(), resolved);

                // Surfaces (plan: surface sources, materials M3): every part the
                // model declares is handed over, before the draw. The picture of
                // a part is the `tex.<part>` source's own tile, read out of
                // `produced` (the source renders earlier in the plan); its
                // material is the row's `material` group. A part carries a
                // `None` slot only when it has neither, so a part with a
                // material and no picture is not cleared.
                let parts = surface_parts(model_id);
                if !parts.is_empty() {
                    let material = surface_material(&row);
                    let mut specs: Vec<(&str, Option<SurfaceSpec>)> = parts
                        .iter()
                        .map(|p| {
                            (*p, material.map(|m| SurfaceSpec { image: None, material: Some(m) }))
                        })
                        .collect();
                    for (consumer, port, source) in &material_edges {
                        let Some(part) = port.strip_prefix("tex.") else { continue };
                        if consumer != &op.node {
                            continue;
                        }
                        let Some(slot) = specs.iter_mut().find(|(p, _)| *p == part) else { continue };
                        let Some(frame) = buffer_of.get(*source).and_then(|b| produced.get(b)) else {
                            continue;
                        };
                        let cfg = surface_config(node, part);
                        let spec = slot.1.get_or_insert(SurfaceSpec::default());
                        spec.image = Some(SurfacePicture {
                            pixels: &frame.0,
                            width: frame.1,
                            height: frame.2,
                            mix: cfg.mix,
                            mapping: cfg.mapping,
                            scale: cfg.scale,
                        });
                    }
                    self.set_surfaces(index, &specs);
                }

                // The chain resolved just above is what the operator arranged
                // in the graph, and handing it here is what makes dragging an
                // `fx.*` node change the picture rather than just the diagram.
                // A generator with no `fx.*` nodes after it resolves to an
                // empty chain, which `solo_chain` reads as "the fixed order
                // this shader always used" — so the un-graphed path renders
                // exactly as it did before any of this existed.
                let chain = chains.get(&op.node).cloned().unwrap_or_default();
                if textures && texture_ok(&op.node) {
                    let t = match (model_id, crate::forge::node_shader(node)) {
                        ("forge", Some(id)) => {
                            let wgsl = crate::forge::source(&crate::output::data_root(), &id).unwrap_or_else(|e| {
                                crate::forge::report(&id, &e);
                                String::new()
                            });
                            self.forge_texture(index, &id, &wgsl, &row, node_time, w, h, alpha, &chain)
                        }
                        _ => self.solo_texture(index, &row, node_time, w, h, alpha, &chain),
                    };
                    if let Some(t) = t {
                        tex_out.insert(buf, t);
                        continue;
                    }
                }
                let frame = match (model_id, crate::forge::node_shader(node)) {
                    // A `forge` source draws the WGSL its layer names. A
                    // shader that cannot be read is the slate, with the
                    // reason on stderr — never a silent black.
                    ("forge", Some(id)) => match crate::forge::source(&crate::output::data_root(), &id) {
                        Ok(wgsl) => self.forge(index, &id, &wgsl, &row, node_time, w, h, alpha, &chain),
                        Err(e) => {
                            crate::forge::report(&id, &e);
                            self.forge(index, &id, "", &row, node_time, w, h, alpha, &chain)
                        }
                    },
                    _ => self.solo_chain(index, &row, node_time, w, h, alpha, &chain),
                };
                readbacks += 1;
                produced.insert(buf, frame);
            } else if node.kind == "capture" {
                // 2026-10-01: a camera, screen or window, live — the newest
                // frame the source's own pipeline decoded, black until the
                // first one lands (a camera takes a second to open).
                let source = node.config.to_json_str();
                let source: serde_json::Value = serde_json::from_str(&source).unwrap_or_default();
                // G2: the newest frame is uploaded once per new frame and
                // stays a texture (compose scales it); `ndi:` and a source
                // with no frame yet take the CPU path below.
                if textures && texture_ok(&op.node) {
                    if let Some(src) = source["source"].as_str() {
                        if let Some((seq, raw)) = crate::capture::latest_raw(src) {
                            let key = format!("capture:{}", op.node);
                            let t = self.cached_texture(&key, seq, &mut || {
                                Some((raw.as_ref().clone(), crate::capture::W, crate::capture::H))
                            });
                            if let Some(t) = t {
                                tex_out.insert(buf, t);
                                continue;
                            }
                        }
                    }
                }
                let px = source["source"]
                    .as_str()
                    .and_then(|s| crate::capture::frame(s, w, h))
                    .unwrap_or_else(|| vec![0u8; (w * h * 4) as usize]);
                produced.insert(buf, (px, w, h));
            } else if node.kind == "still" || node.kind == "reel.clip" {
                // P-E: a picture or one library clip — black until the file
                // has pixels (an unreadable image, a clip with no HAP proxy).
                if node.kind == "still" && textures && texture_ok(&op.node) {
                    // G2: a picture changes only when its config or the draw
                    // size does: one upload, then the cached texture.
                    let mut h64 = std::collections::hash_map::DefaultHasher::new();
                    std::hash::Hash::hash(&(node.config.to_json_str(), w, h), &mut h64);
                    let key = format!("still:{}", op.node);
                    let root = crate::output::data_root();
                    let t = self.cached_texture(&key, std::hash::Hasher::finish(&h64), &mut || {
                        let px = crate::reel_batch::source_frame(&root, node, time, w, h)
                            .unwrap_or_else(|| vec![0u8; (w * h * 4) as usize]);
                        Some((px, w, h))
                    });
                    if let Some(t) = t {
                        tex_out.insert(buf, t);
                        continue;
                    }
                }
                let px = crate::reel_batch::source_frame(&crate::output::data_root(), node, time, w, h)
                    .unwrap_or_else(|| vec![0u8; (w * h * 4) as usize]);
                produced.insert(buf, (px, w, h));
            } else if node.kind == crate::reel_batch::REEL_BATCH_KIND {
                // 2026-10-01: the multi-source reel — whatever its list shows
                // at this tick (`crate::reel_batch`). An empty list (every
                // batch saved before the list existed) draws nothing, as
                // before; an item with no pixels yet is black.
                let root = crate::output::data_root();
                let favorites = crate::scenes::live(&root);
                if let Some((entry, local)) = crate::reel_batch::now(node, &favorites, time) {
                    let px = crate::reel_batch::frame(&root, &entry, local, w, h)
                        .unwrap_or_else(|| vec![0u8; (w * h * 4) as usize]);
                    produced.insert(buf, (px, w, h));
                }
            } else if node.kind == metal_graph::TEXT_SOURCE_KIND {
                // S.W: the string renders here, then its `fx.text.*`
                // descendants are folded into the config before the glyphs
                // are set — per-glyph, not a pixel post pass.
                let mut config = crate::text::TextConfig::from_json(&node.config.to_json_str());
                let steps = text_effects_for(graph, &op.node);
                if !steps.is_empty() {
                    config.effects = steps;
                }
                let row = rows.get(&op.node).cloned().unwrap_or_default();
                if textures && texture_ok(&op.node) {
                    if let Some(t) = self.text_texture(&config, &row, time, w, h) {
                        tex_out.insert(buf, t);
                        continue;
                    }
                }
                let frame = self.text(&config, &row, time, w, h);
                readbacks += 1;
                produced.insert(buf, frame);
            } else if node.kind.starts_with("fx.") {
                // Forward whatever feeds this node's `image` input — found
                // by edge identity, not position, for the reason `buffer_of`
                // exists (see above).
                let src = graph
                    .edges
                    .iter()
                    .find(|e| e.to.node == op.node && e.to.port == "image")
                    .and_then(|e| buffer_of.get(e.from.node.as_str()).copied());
                if let Some(src) = src {
                    // G2: the picture is still a texture: run the fx as a pass.
                    // Any miss reads it back once and takes the CPU code below.
                    if let Some(tin) = tex_out.get(&src).cloned() {
                        // G5: an effect the generator's shader already bakes
                        // (`fx.grade`, `fx.shake`, ...) does nothing to the
                        // picture here on the CPU either, so the texture
                        // passes through with no pass and no readback.
                        if textures && baked_fx_kind(&node.kind) {
                            tex_out.insert(buf, tin);
                            continue;
                        }
                        let clock = metal_graph::effects::generators_of(graph, &op.node)
                            .first()
                            .and_then(|g| gen_time.get(g).copied())
                            .unwrap_or(time);
                        let root = crate::output::data_root();
                        let out = self.fx_on_texture(graph, node, &op.node, &tin, clock, raw_digest, &root);
                        if let Some(t) = out {
                            if texture_ok(&op.node) {
                                tex_out.insert(buf, t);
                            } else {
                                let px = self.readback(&t);
                                readbacks += 1;
                                produced.insert(buf, (px, t.width(), t.height()));
                            }
                            continue;
                        }
                        if !produced.contains_key(&src) {
                            let px = self.readback(&tin);
                            readbacks += 1;
                            produced.insert(src, (px, tin.width(), tin.height()));
                        }
                    }
                    if let Some(mut frame) = produced.get(&src).cloned() {
                        // K: `fx.move` is an effect that works here rather
                        // than inside the generator's post pass — it
                        // translates the picture, and the mask lives
                        // downstream. It draws the built-in `move.wgsl` with
                        // its own five parameters in the ingest, at the
                        // generator's own accumulated clock so a per-beat
                        // drift advances on the music.
                        let clock = metal_graph::effects::generators_of(graph, &op.node)
                            .first()
                            .and_then(|g| gen_time.get(g).copied())
                            .unwrap_or(time);
                        if node.kind == crate::motion::FX_MOVE_KIND {
                            if let Some((id, ingest)) = crate::motion::move_node(graph, &op.node) {
                                match crate::forge::source(&crate::output::data_root(), &id) {
                                    Ok(wgsl) => {
                                        if let Some(px) = self.forge_fx(&id, &wgsl, &frame, clock, &ingest) {
                                            frame.0 = px;
                                        }
                                    }
                                    Err(e) => crate::forge::report(&id, &e),
                                }
                            }
                        } else if node.kind == crate::fx::particles::FX_PARTICLES_KIND {
                            // B: `fx.particles` — stateless dust. On a host with
                            // an adapter `particles.wgsl` hashes the points; the
                            // CPU `dust` is the stand-in's honest version, the
                            // same pairing `fx.move`'s `translate` has.
                            if let Some((id, ingest)) =
                                crate::fx::particles::particles_node(graph, &op.node)
                            {
                                let tint = crate::fx::particles::dust_tint(node);
                                match crate::forge::source(&crate::output::data_root(), &id) {
                                    Ok(wgsl) => match self.forge_fx(&id, &wgsl, &frame, clock, &ingest) {
                                        Some(px) => frame.0 = px,
                                        None => {
                                            frame = crate::fx::particles::dust(&frame, &ingest, clock, tint)
                                        }
                                    },
                                    Err(e) => {
                                        crate::forge::report(&id, &e);
                                        frame = crate::fx::particles::dust(&frame, &ingest, clock, tint);
                                    }
                                }
                            }
                        } else if node.kind == crate::fx::image_pop::FX_IMAGE_POP_KIND {
                            // B: `fx.image-pop` — reel media, composited on the
                            // CPU. The forge effect contract hands a shader one
                            // texture (the picture), with no binding an atlas
                            // could arrive on, so both painters share
                            // `image_pop::compose`; see `crate::fx`.
                            if let Some(reel) = crate::fx::image_pop::reel_of(node) {
                                if let Some(atlas) = crate::fx::image_pop::load_atlas(
                                    &crate::output::data_root(),
                                    &reel,
                                ) {
                                    let ingest = crate::fx::image_pop::image_pop_ingest(node);
                                    crate::fx::image_pop::compose(&mut frame, &atlas, &ingest, clock, None);
                                }
                            }
                        } else if node.kind == crate::fx::flash::FX_FLASH_KIND {
                            // Loops T13: the diffuse white flash, per pixel
                            // on the CPU on every painter (a lookup table).
                            let (opacity, diffuse) = crate::fx::flash::flash_of(node);
                            crate::fx::flash::apply(&mut frame.0, opacity, diffuse);
                        } else if metal_graph::parity::is_parity_effect(&node.kind) {
                            // Parity P-A: chroma key, level, lookup, math,
                            // projection. A built-in shader on a GPU painter
                            // (the forge effect pass), the same math on the
                            // CPU otherwise or if the shader cannot draw.
                            let cfg = crate::fx::parity::config(node);
                            let root = crate::output::data_root();
                            let gpu = crate::fx::parity::shader_of(&node.kind).and_then(|id| {
                                match crate::forge::source(&root, id) {
                                    Ok(wgsl) => {
                                        let ingest = crate::fx::parity::ingest(&node.kind, &cfg);
                                        #[cfg(feature = "gpu")]
                                        let via_texture = self.gpu_cx().and_then(|cx| {
                                            let input = cx.upload(&frame.0, frame.1, frame.2);
                                            let t = self.forge_fx_texture(id, &wgsl, &input, clock, &ingest, false)?;
                                            Some(self.readback(&t))
                                        });
                                        #[cfg(not(feature = "gpu"))]
                                        let via_texture: Option<Vec<u8>> = None;
                                        via_texture.or_else(|| self.forge_fx(id, &wgsl, &frame, clock, &ingest))
                                    }
                                    Err(e) => {
                                        crate::forge::report(id, &e);
                                        None
                                    }
                                }
                            });
                            match gpu {
                                Some(px) => frame.0 = px,
                                None => crate::fx::parity::apply_cpu(&node.kind, &cfg, &mut frame, &root),
                            }
                        } else if node.kind == crate::fx::warp::FX_WARP_KIND {
                            // Loops T12: the coordinate-mesh warp, with its
                            // own per-node (and per-size) mosh history.
                            let warp = crate::fx::warp::warp_of(node);
                            let key = crate::fx::warp::history_key(&op.node, frame.1, frame.2);
                            frame.0 = self.warp_fx(&key, &warp, &frame);
                        } else if node.kind == crate::forge::FX_KIND {
                            // `fx.forge`: its shader is runtime WGSL, run over
                            // the frame it is handed.
                            if let Some(id) = crate::forge::node_shader(node) {
                                match crate::forge::source(&crate::output::data_root(), &id) {
                                    Ok(wgsl) => {
                                        let ingest = crate::forge::fx_ingest(node, raw_digest);
                                        if let Some(px) = self.forge_fx(&id, &wgsl, &frame, clock, &ingest) {
                                            frame.0 = px;
                                        }
                                    }
                                    Err(e) => crate::forge::report(&id, &e),
                                }
                            }
                        }
                        produced.insert(buf, frame);
                    }
                }
            } else if node.kind.starts_with(metal_graph::materials::PREFIX) {
                // A `mat.*` node passes its picture through unchanged: the
                // material acts on the 3D model upstream, whose own draw
                // resolved the chain into the row (see `crate::chain_for_row`,
                // rule 4). Forward it like any pass-through so a downstream
                // `fx.*`/compose still sees it.
                let src = graph
                    .edges
                    .iter()
                    .find(|e| e.to.node == op.node && e.to.port == "image")
                    .and_then(|e| buffer_of.get(e.from.node.as_str()).copied());
                if let Some(src) = src {
                    if let Some(t) = tex_out.get(&src).cloned() {
                        if textures && texture_ok(&op.node) {
                            tex_out.insert(buf, t);
                        } else {
                            let px = self.readback(&t);
                            readbacks += 1;
                            produced.insert(buf, (px, t.width(), t.height()));
                        }
                        continue;
                    }
                    if let Some(frame) = produced.get(&src).cloned() {
                        produced.insert(buf, frame);
                    }
                }
            }
        }

        let frames: HashMap<usize, (Vec<u8>, u32, u32)> = produced
            .into_iter()
            .filter(|(b, _)| sink_inputs.contains(b))
            .collect();
        let textures: HashMap<usize, crate::gpu_frame::GpuFrame> =
            tex_out.into_iter().filter(|(b, _)| sink_inputs.contains(b)).collect();
        // The material tiles, keyed by their *consumer* — see
        // `RunOutput::materials`. Pulled from `frames` rather than from the
        // `produced` map before the filter, deliberately: a material whose
        // pixels somehow did not survive the filter is absent here too, so
        // the consumer's "no material wired" and "material rendered nothing"
        // read the same way instead of disagreeing.
        let materials: HashMap<(metal_graph::NodeId, String), (Vec<u8>, u32, u32)> = material_edges
            .iter()
            .filter_map(|(node, port, source)| {
                let b = buffer_of.get(*source)?;
                frames
                    .get(b)
                    .map(|f| ((node.clone(), port.clone()), f.clone()))
            })
            .collect();
        RunOutput {
            frames,
            readbacks,
            chains,
            materials,
            textures,
        }
    }
    /// Which models are **transmissions** — they draw over footage rather than
    /// generating a picture, and cannot show anything until the operator has
    /// pointed them at a folder. Same order as [`Paint::ids`].
    ///
    /// On the trait rather than behind [`Paint::reel`] because the answer has
    /// to be the same on both painters: the stand-in has no renderers to ask
    /// and reads it out of `data/schema.json`, and the surface that says "this
    /// one needs a library" is drawn on both.
    fn footage(&self) -> Vec<bool>;

    /// Which models draw their background via the shared `Backdrop` pass in
    /// `metal-render`, rather than covering the frame with their own field —
    /// see `metal_render::model::Model::backdrop`. Same order as [`Paint::ids`],
    /// same reason as [`Paint::footage`] for being on the trait: the page's
    /// "background as alpha" toggle (`the design notes PART IV` §17.3) has to know which
    /// models it does anything on, and the stand-in painter reads it out of
    /// `data/schema.json` the same way it reads `footage`.
    fn backdrop(&self) -> Vec<bool>;

    /// The footage side of model `index`, for attaching a library and pushing
    /// the Curator's cue.
    ///
    /// `None` from every generative model, and `None` from *every* model on the
    /// stand-in painter — which is honest rather than a gap: there is no
    /// renderer there to hand footage to, and `real()` is already false.
    fn reel(&mut self, index: usize) -> Option<&mut dyn metal_reel::Reel>;

    /// Whether these are the real renderers. `false` puts a warning on the
    /// page, and it must never be a comforting lie.
    fn real(&self) -> bool;

    /// One line for the log and the page.
    fn describe(&self) -> String;

    /// Which painter draws model `index`, for the page: the device's backend
    /// when the model runs on it, the CPU stand-in when it cannot.
    fn painter_of(&self, _index: usize) -> String {
        "CPU stand-in".to_string()
    }
}

/// The surface parts a generator's model declares, from the manifest the
/// graph is built on (`data/schema.json`), so the names here are the same
/// names as the `tex.<part>` inputs in the catalog. Empty for a model with none.
fn surface_parts(model_id: &str) -> Vec<&'static str> {
    crate::engine::shared_manifest()
        .models
        .iter()
        .find(|m| m.id == model_id)
        .map(|m| m.surfaces.iter().map(String::as_str).collect())
        .unwrap_or_default()
}

/// One part's settings from the consumer node: `config.surfaces.<part>` with
/// `mix` (default 1), `mapping` (`"object"` | `"projector"`, default object)
/// and `scale` (default 1).
struct SurfaceConfig {
    mix: f32,
    mapping: SurfaceMapping,
    scale: f32,
}

fn surface_config(node: &metal_graph::Node, part: &str) -> SurfaceConfig {
    let at = node.config.get("surfaces").and_then(|s| s.get(part));
    let num = |key: &str, default: f64| {
        at.and_then(|v| v.get(key)).and_then(serde_json::Value::as_f64).unwrap_or(default) as f32
    };
    let mapping = match at.and_then(|v| v.get("mapping")).and_then(serde_json::Value::as_str) {
        Some("projector") => SurfaceMapping::Projector,
        _ => SurfaceMapping::Object,
    };
    SurfaceConfig { mix: num("mix", 1.0).clamp(0.0, 1.0), mapping, scale: num("scale", 1.0) }
}

/// The `material` group of a row resolved into a [`SurfaceMaterial`], or `None`
/// when the row carries no group or it is off (`mat_mix` 0 — every default row
/// and every row saved before the group existed). Read from the row itself, so
/// a material is a float group like an effect, not a second state (rule 4).
fn surface_material(row: &[f32]) -> Option<SurfaceMaterial> {
    let m = metal_schema::read_material(row)?;
    if m[1] <= 0.0 {
        return None;
    }
    Some(SurfaceMaterial {
        kind: m[0],
        mix: m[1],
        scale: m[2],
        rough: m[3],
        metal: m[4],
        bump: m[5],
        env: m[6],
        tint: m[7],
        tint_hue: m[8],
        glitch: m[9],
        glitch_rate: m[10],
        glitch_seed: m[11],
    })
}

/// Whichever painter this build has.
///
/// The one place the choice is made. Everything downstream — the routes, the
/// compositor, the monitor, the tests — takes a `&mut dyn Paint` and never asks
/// which one it got; the only code that is allowed to care is the line in
/// `main` that prints what it is.
pub fn painter() -> Box<dyn Paint> {
    // `Gpu::new()` reaches a real wgpu adapter. Concurrent adapter
    // creation segfaults on some drivers (seen in the test suite: two
    // parallel real-door tests SIGSEGV'd the whole process), so every
    // creation is serialized here — the lock guards only the build,
    // not the drawn frames afterwards.
    static ADAPTER: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _adapter = ADAPTER
        .get_or_init(std::sync::Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    #[cfg(feature = "gpu")]
    {
        Box::new(Gpu::new())
    }
    #[cfg(not(feature = "gpu"))]
    {
        crate::backend::set_active(crate::backend::Backend::Cpu);
        Box::new(crate::stand::Stand::new())
    }
}

/// RGBA straight to a JPEG. Quality 82 because the models are mostly smooth
/// gradients, where the difference from 95 is invisible and the difference in
/// bytes is not.
///
/// Lives here rather than in `metal-render` now that a build without `metal-render`
/// still has to answer `/preview.jpg`. Byte-identical to the one it replaced.
pub fn encode_jpeg(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    encode_jpeg_q(rgba, width, height, 82)
}

/// [`encode_jpeg`] at a chosen quality (1–100), for `GET /render`'s `q=`.
pub fn encode_jpeg_q(rgba: &[u8], width: u32, height: u32, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut out, quality);
    encoder
        .encode(
            rgba,
            width as u16,
            height as u16,
            jpeg_encoder::ColorType::Rgba,
        )
        .expect("jpeg encode failed");
    out
}

// ---------------------------------------------------------------------------
// the real one
// ---------------------------------------------------------------------------

/// The wgpu models. A forwarding wrapper and nothing else — every
/// decision about how a model draws is still in `metal-render`.
///
/// The second field is the CPU stand-in, which draws any model the device
/// cannot run (a WebGL2 device has no compute shaders, so dla, solvent, turing
/// and terrario fall to it one by one). On a device with compute every model
/// is the renderer's own and the stand-in is never asked.
#[cfg(feature = "gpu")]
pub struct Gpu(
    pub metal_render::headless::Renderer,
    pub crate::stand::Stand,
    /// P-B: GPU text on the renderer's device, built on the first text frame.
    pub Option<crate::text_gpu::TextGpu>,
    /// P-E: the transition pass on the renderer's device, built on first use.
    pub Option<crate::transition_gpu::TransitionGpu>,
    /// G1: where this painter composes, decided once in [`Gpu::from_renderer`].
    pub crate::backend::Backend,
    /// G1: the shared device context and the layer-stack compositor.
    pub GpuState,
);

/// G1: the GPU-centric half of [`Gpu`], built with it.
#[cfg(feature = "gpu")]
pub struct GpuState {
    pub cx: crate::gpu_compose::GpuCx,
    pub compose: Option<crate::gpu_compose::GpuCompose>,
    pub edge: Vec<crate::gpu_edge::EdgeBlendPass>,
    pub scale: Option<crate::gpu_edge::ScalePass>,
    /// G2: the lookup/ramp passes, built on first use.
    pub fx: crate::gpu_fx::FxPasses,
    /// G3: cached coverage maps (contour clips, mask cuts, stroke bands).
    pub covers: crate::gpu_cover::CoverCache,
    /// G3: the whole-frame shade pass, built on first use.
    pub shade: Option<crate::gpu_cover::ShadePass>,
    /// G3: one non-blocking readback ring per sink.
    pub readbacks: std::collections::HashMap<String, crate::gpu_compose::AsyncReadback>,
    /// G2: `still`/`capture` pictures kept as textures: key -> (version, frame).
    pub sources: std::collections::HashMap<String, (u64, crate::gpu_compose::GpuFrame)>,
}

#[cfg(feature = "gpu")]
impl Gpu {
    /// Builds the renderers.
    ///
    /// Built before the listener binds, on purpose and unchanged: a machine
    /// with no usable adapter must fail with a message rather than serve a page
    /// of broken images at a venue.
    pub fn new() -> Gpu {
        Gpu::from_renderer(metal_render::headless::Renderer::new())
    }

    /// The painter over a renderer the caller already built on its device.
    /// The compose backend is decided here, once: `METALVJ_BACKEND` wins,
    /// otherwise an adapter exists (this renderer is on one), so [`Backend::Gpu`].
    pub fn from_renderer(renderer: metal_render::headless::Renderer) -> Gpu {
        let backend = crate::backend::Backend::choose(crate::backend::Backend::from_env(), true);
        Gpu::with_backend(renderer, backend)
    }

    /// [`Gpu::from_renderer`] with the backend forced (tests, measurements).
    pub fn with_backend(renderer: metal_render::headless::Renderer, backend: crate::backend::Backend) -> Gpu {
        let (device, queue) = renderer.device_queue();
        crate::backend::set_active(backend);
        Gpu(
            renderer,
            crate::stand::Stand::new(),
            None,
            None,
            backend,
            GpuState { cx: crate::gpu_compose::GpuCx::new(device, queue), compose: None, edge: Vec::new(), scale: None, fx: Default::default(), covers: Default::default(), shade: None, readbacks: Default::default(), sources: Default::default() },
        )
    }
}

#[cfg(feature = "gpu")]
impl Paint for Gpu {
    fn backend(&self) -> crate::backend::Backend {
        self.4
    }
    fn gpu_cx(&mut self) -> Option<crate::gpu_compose::GpuCx> {
        (self.4 == crate::backend::Backend::Gpu).then(|| self.5.cx.clone())
    }
    fn solo_texture(
        &mut self,
        index: usize,
        floats: &[f32],
        time: f32,
        w: u32,
        h: u32,
        background_alpha: bool,
        chain: &[metal_schema::EffectStep],
    ) -> Option<crate::gpu_frame::GpuFrame> {
        if self.4 != crate::backend::Backend::Gpu || !self.0.runs_here(index) {
            return None;
        }
        let target = self.5.cx.alloc(w, h);
        self.0.solo_chain_into(
            index,
            floats,
            time,
            background_alpha,
            chain,
            &metal_render::shared::post::EstelaParams::default(),
            target.texture(),
        );
        Some(target)
    }
    fn forge_texture(
        &mut self,
        index: usize,
        key: &str,
        wgsl: &str,
        floats: &[f32],
        time: f32,
        w: u32,
        h: u32,
        background_alpha: bool,
        chain: &[metal_schema::EffectStep],
    ) -> Option<crate::gpu_frame::GpuFrame> {
        if let Err(e) = self.0.forge_select(key, wgsl) {
            crate::forge::report(key, &e);
        }
        self.solo_texture(index, floats, time, w, h, background_alpha, chain)
    }
    fn compose_gpu(
        &mut self,
        w: u32,
        h: u32,
        mode: crate::layers::Composition,
        tiles: &[crate::gpu_frame::GpuTile],
    ) -> Option<crate::gpu_frame::GpuFrame> {
        if self.4 != crate::backend::Backend::Gpu {
            return None;
        }
        let cx = self.5.cx.clone();
        let c = self.5.compose.get_or_insert_with(|| crate::gpu_compose::GpuCompose::new(&cx));
        Some(c.compose(w, h, mode, tiles))
    }
    fn readback(&mut self, frame: &crate::gpu_frame::GpuFrame) -> Vec<u8> {
        self.5.cx.readback(frame)
    }
    fn shared_gpu(&mut self) -> Option<metal_render::gpu::Gpu> {
        (self.4 == crate::backend::Backend::Gpu).then(|| self.0.shared_gpu())
    }
    fn cover_gpu(
        &mut self,
        key: u64,
        w: u32,
        h: u32,
        make: &mut dyn FnMut() -> Vec<u8>,
    ) -> Option<crate::gpu_frame::CoverRef> {
        if self.4 != crate::backend::Backend::Gpu {
            return None;
        }
        let cx = self.5.cx.clone();
        Some(self.5.covers.get_or_build(&cx, key, w, h, make))
    }
    fn shade_gpu(
        &mut self,
        frame: &crate::gpu_frame::GpuFrame,
        shade: &crate::gpu_frame::Shade,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        if self.4 != crate::backend::Backend::Gpu {
            return None;
        }
        let cx = self.5.cx.clone();
        let p = self.5.shade.get_or_insert_with(|| crate::gpu_cover::ShadePass::new(&cx));
        Some(p.apply(&cx, frame, shade))
    }
    fn readback_deferred(&mut self, frame: &crate::gpu_frame::GpuFrame) -> Option<Box<dyn FnOnce() -> Vec<u8> + Send>> {
        Some(self.5.cx.readback_deferred(frame))
    }
    fn readback_async(&mut self, key: &str, frame: &crate::gpu_frame::GpuFrame) -> Option<(Vec<u8>, u32, u32)> {
        let cx = self.5.cx.clone();
        let rb = self.5.readbacks.entry(key.to_string()).or_insert_with(|| crate::gpu_compose::AsyncReadback::new(&cx));
        let _ = rb.submit(&cx, frame);
        rb.poll(&cx)
    }
    fn fit_size(&mut self, w: u32, h: u32) -> (u32, u32) {
        if self.4 != crate::backend::Backend::Gpu {
            return (w, h);
        }
        let max = self.5.cx.device.limits().max_texture_dimension_2d;
        crate::gpu_edge::fit_size(w, h, max, crate::gpu_edge::memory_budget())
    }
    fn edge_blend_gpu(
        &mut self,
        frame: &crate::gpu_frame::GpuFrame,
        map: &std::sync::Arc<crate::edge_blend::BlendMap>,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        if self.4 != crate::backend::Backend::Gpu || (map.width, map.height) != (frame.width(), frame.height()) {
            return None;
        }
        let cx = self.5.cx.clone();
        let i = match self.5.edge.iter().position(|p| std::sync::Arc::ptr_eq(&p.source, map)) {
            Some(i) => i,
            None => {
                if self.5.edge.len() >= 8 {
                    self.5.edge.remove(0);
                }
                self.5.edge.push(crate::gpu_edge::EdgeBlendPass::new(&cx, map));
                self.5.edge.len() - 1
            }
        };
        Some(cx.run(&mut self.5.edge[i], &[frame], frame.width(), frame.height()))
    }
    fn upscale_gpu(&mut self, frame: &crate::gpu_frame::GpuFrame, w: u32, h: u32) -> Option<crate::gpu_frame::GpuFrame> {
        if self.4 != crate::backend::Backend::Gpu {
            return None;
        }
        let cx = self.5.cx.clone();
        let p = self.5.scale.get_or_insert_with(|| crate::gpu_edge::ScalePass::new(&cx));
        Some(cx.run(p, &[frame], w, h))
    }

    /// P-E: the GPU pass on the Gpu backend, the CPU reference on the Cpu one.
    /// This is the CPU-bytes entry (upload, pass, readback), for a tile that
    /// only exists on the CPU; textures go through [`Paint::transition_texture`].
    fn transition(&mut self, spec: &crate::transition::Spec, v: f32, frame: &mut [u8], w: u32, h: u32, seed: u32) {
        if self.4 == crate::backend::Backend::Gpu {
            let cx = self.5.cx.clone();
            self.3
                .get_or_insert_with(|| crate::transition_gpu::TransitionGpu::new(&cx))
                .apply(&cx, spec, v, frame, w, h, seed);
        } else {
            crate::transition::apply(spec, v, frame, w, h, seed);
        }
    }
    fn transition_texture(
        &mut self,
        spec: &crate::transition::Spec,
        v: f32,
        input: &crate::gpu_frame::GpuFrame,
        seed: u32,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        if self.4 != crate::backend::Backend::Gpu {
            return None;
        }
        let cx = self.5.cx.clone();
        let t = self.3.get_or_insert_with(|| crate::transition_gpu::TransitionGpu::new(&cx));
        Some(t.run(&cx, spec, v, input, seed))
    }
    fn text_texture(
        &mut self,
        config: &crate::text::TextConfig,
        row: &[f32],
        time: f32,
        w: u32,
        h: u32,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        if self.4 != crate::backend::Backend::Gpu {
            return None;
        }
        let cx = self.5.cx.clone();
        let text = self.2.get_or_insert_with(|| {
            let (device, queue) = (cx.device.clone(), cx.queue.clone());
            crate::text_gpu::TextGpu::new(device, queue)
        });
        let mut pass = crate::text_gpu::TextPass { text, config, row, time, ok: false };
        let f = cx.run(&mut pass, &[], w.max(1), h.max(1));
        pass.ok.then_some(f)
    }
    fn forge_fx_texture(
        &mut self,
        key: &str,
        wgsl: &str,
        input: &crate::gpu_frame::GpuFrame,
        time: f32,
        ingest: &[f32],
        srgb_views: bool,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        if self.4 != crate::backend::Backend::Gpu {
            return None;
        }
        let cx = self.5.cx.clone();
        let mut pass = crate::gpu_fx::ForgeFxPass { renderer: &mut self.0, key, wgsl, time, ingest, srgb_views, error: None };
        let f = cx.run(&mut pass, &[input], input.width(), input.height());
        if let Some(e) = pass.error {
            crate::forge::report(key, &e);
            return None;
        }
        Some(f)
    }
    fn crop_texture(&mut self, frame: &crate::gpu_frame::GpuFrame, win: &crate::layers::Sector) -> Option<crate::gpu_frame::GpuFrame> {
        if self.4 != crate::backend::Backend::Gpu {
            return None;
        }
        let (x, y, cw, ch) = crate::engine::crop_rect(frame.width(), frame.height(), win)?;
        Some(self.5.cx.crop(frame, x, y, cw, ch))
    }
    fn special_fx_texture(
        &mut self,
        node: &metal_graph::Node,
        id: &str,
        input: &crate::gpu_frame::GpuFrame,
        clock: f32,
        root: &std::path::Path,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        if self.4 != crate::backend::Backend::Gpu {
            return None;
        }
        let cx = self.5.cx.clone();
        let kind = node.kind.as_str();
        if kind == crate::fx::flash::FX_FLASH_KIND {
            let (opacity, diffuse) = crate::fx::flash::flash_of(node);
            Some(self.5.fx.flash(&cx, opacity, diffuse, input))
        } else if kind == crate::fx::image_pop::FX_IMAGE_POP_KIND {
            let atlas = crate::fx::image_pop::reel_of(node).and_then(|r| crate::fx::image_pop::load_atlas(root, &r));
            Some(match atlas {
                Some(atlas) => {
                    let ingest = crate::fx::image_pop::image_pop_ingest(node);
                    self.5.fx.image_pop(&cx, &atlas, &ingest, clock, input)
                }
                None => input.clone(),
            })
        } else if kind == crate::fx::warp::FX_WARP_KIND {
            let warp = crate::fx::warp::warp_of(node);
            let key = crate::fx::warp::history_key(id, input.width(), input.height());
            Some(self.5.fx.warp(&cx, &mut self.0, &key, &warp, input))
        } else {
            None
        }
    }
    fn cached_texture(
        &mut self,
        key: &str,
        version: u64,
        make: &mut dyn FnMut() -> Option<(Vec<u8>, u32, u32)>,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        if self.4 != crate::backend::Backend::Gpu {
            return None;
        }
        if let Some((v, f)) = self.5.sources.get(key) {
            if *v == version {
                return Some(f.clone());
            }
        }
        let (px, w, h) = make()?;
        let f = self.5.cx.upload(&px, w, h);
        self.5.sources.insert(key.to_string(), (version, f.clone()));
        Some(f)
    }
    fn parity_texture(
        &mut self,
        kind: &str,
        cfg: &serde_json::Value,
        root: &std::path::Path,
        input: Option<&crate::gpu_frame::GpuFrame>,
        w: u32,
        h: u32,
    ) -> Option<crate::gpu_frame::GpuFrame> {
        if self.4 != crate::backend::Backend::Gpu {
            return None;
        }
        let cx = self.5.cx.clone();
        match (kind, input) {
            (metal_graph::parity::LOOKUP_KIND, Some(i)) => self.5.fx.lookup(&cx, cfg, root, i),
            (metal_graph::RAMP_KIND, None) => self.5.fx.ramp(&cx, cfg, root, w, h),
            _ => None,
        }
    }
    /// P-B: glyphon on the renderer's own device on the Gpu backend;
    /// [`crate::text::render`] is the Cpu backend and the reference.
    fn text(
        &mut self,
        config: &crate::text::TextConfig,
        row: &[f32],
        time: f32,
        w: u32,
        h: u32,
    ) -> (Vec<u8>, u32, u32) {
        if self.4 == crate::backend::Backend::Gpu || config.stroke.width > 0.0 {
            let (device, queue) = self.0.device_queue();
            self.2
                .get_or_insert_with(|| crate::text_gpu::TextGpu::new(device, queue))
                .render(config, row, time, w, h)
        } else {
            crate::text::render(config, row, time, w, h)
        }
    }
    fn set_surfaces(&mut self, index: usize, parts: &[(&str, Option<SurfaceSpec>)]) {
        use metal_render::model::{
            Mapping, MaterialSpec, SurfaceImage, SurfaceSpec as RenderSpec,
        };
        for (part, spec) in parts {
            let spec = spec.map(|s| RenderSpec {
                image: s.image.map(|p| SurfaceImage {
                    pixels: p.pixels,
                    width: p.width,
                    height: p.height,
                    mix: p.mix,
                    mapping: match p.mapping {
                        SurfaceMapping::Object => Mapping::Object,
                        SurfaceMapping::Projector => Mapping::Projector,
                    },
                    scale: p.scale,
                }),
                material: s.material.map(|m| MaterialSpec {
                    kind: m.kind,
                    mix: m.mix,
                    scale: m.scale,
                    rough: m.rough,
                    metal: m.metal,
                    bump: m.bump,
                    env: m.env,
                    tint: m.tint,
                    tint_hue: m.tint_hue,
                    glitch: m.glitch,
                    glitch_rate: m.glitch_rate,
                    glitch_seed: m.glitch_seed,
                }),
            });
            self.0.set_surface(index, part, spec);
        }
    }
    fn model_count(&self) -> usize {
        self.0.model_count()
    }
    fn ids(&self) -> Vec<&'static str> {
        self.0.ids()
    }
    fn names(&self) -> Vec<&'static str> {
        self.0.names()
    }
    fn schemas(&self) -> Vec<&'static Schema> {
        self.0.schemas()
    }
    fn solo(
        &mut self,
        index: usize,
        floats: &[f32],
        time: f32,
        w: u32,
        h: u32,
        background_alpha: bool,
    ) -> (Vec<u8>, u32, u32) {
        if !self.0.runs_here(index) {
            return self.1.solo(index, floats, time, w, h, background_alpha);
        }
        self.0.solo(index, floats, time, w, h, background_alpha)
    }
    fn solo_chain(
        &mut self,
        index: usize,
        floats: &[f32],
        time: f32,
        w: u32,
        h: u32,
        background_alpha: bool,
        chain: &[metal_schema::EffectStep],
    ) -> (Vec<u8>, u32, u32) {
        // `Paint::solo_chain`'s own signature stays exactly what it was —
        // `Stand` (the CPU stand-in, `crate::stand`) relies on the trait's
        // default forwarding to `solo`, and growing this signature would
        // have to grow that default too. So the Estela look an
        // `fx.estela` step in `chain` grades with is not
        // threaded in from here; it is `EstelaParams::
        // default` (the model's shipped look) every time — see
        // `metal_render::shared::post::EstelaParams`'s own doc comment for why
        // v1 has exactly one look to offer regardless of caller.
        if !self.0.runs_here(index) {
            return self.1.solo_chain(index, floats, time, w, h, background_alpha, chain);
        }
        self.0.solo_chain(
            index,
            floats,
            time,
            w,
            h,
            background_alpha,
            chain,
            &metal_render::shared::post::EstelaParams::default(),
        )
    }
    fn forge(
        &mut self,
        index: usize,
        key: &str,
        wgsl: &str,
        floats: &[f32],
        time: f32,
        w: u32,
        h: u32,
        background_alpha: bool,
        chain: &[metal_schema::EffectStep],
    ) -> (Vec<u8>, u32, u32) {
        if let Err(e) = self.0.forge_select(key, wgsl) {
            crate::forge::report(key, &e);
        }
        self.solo_chain(index, floats, time, w, h, background_alpha, chain)
    }
    fn forge_fx(
        &mut self,
        key: &str,
        wgsl: &str,
        frame: &(Vec<u8>, u32, u32),
        time: f32,
        ingest: &[f32],
    ) -> Option<Vec<u8>> {
        match self.0.forge_fx(key, wgsl, &frame.0, frame.1, frame.2, time, ingest) {
            Ok(px) => Some(px),
            Err(e) => {
                crate::forge::report(key, &e);
                None
            }
        }
    }
    fn warp_fx(&mut self, key: &str, warp: &metal_schema::warp::Warp, frame: &(Vec<u8>, u32, u32)) -> Vec<u8> {
        match self.0.warp_fx(key, warp, &frame.0, frame.1, frame.2) {
            Ok(px) => px,
            Err(e) => {
                eprintln!("fx.warp {key}: {e}");
                crate::fx::warp::apply_cpu(key, warp, frame)
            }
        }
    }
    fn footage(&self) -> Vec<bool> {
        self.0.footage()
    }
    fn backdrop(&self) -> Vec<bool> {
        self.0.backdrop()
    }
    fn reel(&mut self, index: usize) -> Option<&mut dyn metal_reel::Reel> {
        self.0.reel(index)
    }
    fn real(&self) -> bool {
        true
    }
    fn describe(&self) -> String {
        let cpu = (0..self.0.model_count()).filter(|i| !self.0.runs_here(*i)).count();
        format!(
            "{} renderers on {}, {} of them on the CPU stand-in (no compute shaders)",
            self.0.model_count(),
            self.0.backend_name(),
            cpu
        )
    }
    fn painter_of(&self, index: usize) -> String {
        if self.0.runs_here(index) {
            self.0.backend_name().to_string()
        } else {
            "CPU stand-in (no compute shaders on this device)".to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// B1 — one readback per generator, however many `fx.*` nodes are chained
// ---------------------------------------------------------------------------

// Runs against `Stand` rather than `Gpu`: this crate is tested on hosts with
// no adapter (see the module doc), and the readback count `run` produces
// does not depend on which painter counts it — that is the whole point of
// `run` being a provided method on the trait.
#[cfg(all(test, not(feature = "gpu")))]
mod run_tests {
    use super::*;
    use metal_graph::{Domain, Endpoint, Graph};

    /// A generator feeding two chained `fx.*` nodes into an `output.device`
    /// must cost exactly one readback — the number `render_device` used to
    /// spend once per source *plus one per effect chained after it*, before
    /// this method existed.
    #[test]
    fn a_generator_with_two_chained_effects_is_one_readback() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let fx1 = g.add_node("fx", "fx.shake", [1.0, 0.0]);
        let fx2 = g.add_node("fx", "fx.crt", [2.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&fx1, "image"));
        g.add_edge(Endpoint::new(&fx1, "image"), Endpoint::new(&fx2, "image"));
        g.add_edge(Endpoint::new(&fx2, "image"), Endpoint::new(&out, "image"));
        let plan = g.compile();

        let mut stand = crate::stand::Stand::new();
        let rows: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
            [(gen.clone(), stand.schemas()[0].defaults())]
                .into_iter()
                .collect();

        let result = stand.run(&g, &plan, &rows, &HashMap::new(), &[], 1.0, 16, 9);
        assert_eq!(
            result.readbacks, 1,
            "a generator + two chained effects must be one readback"
        );
        assert_eq!(
            result.frames.len(),
            1,
            "only the buffer the sink reads should be kept"
        );
    }

    /// S.W: a `text.source` renders in one readback and picks up its
    /// `fx.text.*` descendants as the renderer's per-glyph steps.
    #[test]
    fn a_text_source_collects_its_text_effects_in_one_readback() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let text = g.add_node("t", metal_graph::TEXT_SOURCE_KIND, [0.0, 0.0]);
        let wave = g.add_node("w", "fx.text.wave", [1.0, 0.0]);
        let out = g.add_node("out", "output.device", [2.0, 0.0]);
        if let Some(n) = g.node_mut(&wave) {
            n.config = metal_graph::Config::from_json_str("{\"amount\":0.7}").unwrap();
        }
        g.add_edge(Endpoint::new(&text, "image"), Endpoint::new(&wave, "image"));
        g.add_edge(Endpoint::new(&wave, "image"), Endpoint::new(&out, "image"));
        let plan = g.compile();

        let steps = text_effects_for(&g, &text);
        assert_eq!(steps.len(), 1, "the downstream fx.text node is collected");
        assert_eq!(steps[0].kind, "wave");
        assert!((steps[0].amount - 0.7).abs() < 1e-6);

        let mut stand = crate::stand::Stand::new();
        let result = stand.run(&g, &plan, &HashMap::new(), &HashMap::new(), &[], 1.0, 16, 9);
        assert_eq!(result.readbacks, 1, "text.source is one readback");
        assert_eq!(result.frames.len(), 1);
    }

    /// The same generator feeding three sinks directly — A5's dedup case —
    /// is also one readback, not three: `Graph::compile` already collapsed
    /// the fan-out to one op, and `run` must not re-render per consumer.
    #[test]
    fn a_generator_feeding_three_sinks_is_still_one_readback() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let m1 = g.add_node("mask", "mask", [1.0, 0.0]);
        let m2 = g.add_node("mask", "mask", [1.0, 1.0]);
        let m3 = g.add_node("mask", "mask", [1.0, 2.0]);
        for m in [&m1, &m2, &m3] {
            g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(m, "image"));
        }
        let plan = g.compile();

        let mut stand = crate::stand::Stand::new();
        let rows: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
            [(gen.clone(), stand.schemas()[0].defaults())]
                .into_iter()
                .collect();

        let result = stand.run(&g, &plan, &rows, &HashMap::new(), &[], 1.0, 16, 9);
        assert_eq!(result.readbacks, 1);
    }

    /// B5/§3.2, end to end: a `fijo` `fx.*` node chained after a generator
    /// must show up in `RunOutput.chains` with its constant amount, in
    /// chain order — the integration point `solo_chain` will read from,
    /// proved through the same `Paint::run` path a real frame takes rather
    /// than only at the lower `metal_graph::effects`/`metal_perform` layers.
    #[test]
    fn a_fijo_effect_chained_after_a_generator_resolves_in_run_output() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "vignette",
            metal_schema::EffectState::Fixed(0.65),
            metal_graph::effects::ControlSource::RawIngest,
        )
        .expect("materializes");
        let plan = g.compile();

        let mut stand = crate::stand::Stand::new();
        let rows: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
            [(gen.clone(), stand.schemas()[0].defaults())]
                .into_iter()
                .collect();

        let result = stand.run(&g, &plan, &rows, &HashMap::new(), &[], 1.0, 16, 9);
        let chain = result
            .chains
            .get(&gen)
            .expect("this generator has a resolved chain");
        let idx = metal_schema::effect_index("vignette").unwrap() as u32;
        assert_eq!(chain.len(), 1);
        assert_eq!(
            chain[0],
            metal_schema::EffectStep {
                effect: idx,
                amount: 0.65,
                slot: 0
            }
        );
    }

    /// Reordering two `fx.*` nodes directly on the graph — the way a drag
    /// in the canvas would — must change the order `RunOutput.chains`
    /// reports, with no separate step to tell `run` to "recompute" anything.
    #[test]
    fn reordering_fx_nodes_on_the_graph_changes_the_resolved_chain_order() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "shake",
            metal_schema::EffectState::Fixed(0.1),
            metal_graph::effects::ControlSource::RawIngest,
        )
        .unwrap();
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "crt",
            metal_schema::EffectState::Fixed(0.2),
            metal_graph::effects::ControlSource::RawIngest,
        )
        .unwrap();

        let mut stand = crate::stand::Stand::new();
        let rows: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
            [(gen.clone(), stand.schemas()[0].defaults())]
                .into_iter()
                .collect();
        let plan = g.compile();
        let before = stand.run(&g, &plan, &rows, &HashMap::new(), &[], 1.0, 16, 9);
        let order_before: Vec<u32> = before
            .chains
            .get(&gen)
            .unwrap()
            .iter()
            .map(|s| s.effect)
            .collect();
        assert_eq!(
            order_before[0],
            metal_schema::effect_index("shake").unwrap() as u32
        );

        // Cut every edge and rewire gen -> crt -> shake -> out, swapping
        // the order directly on the graph — nothing about the fx nodes'
        // own config changed.
        let edge_ids: Vec<String> = g.edges.iter().map(|e| e.id.clone()).collect();
        for id in edge_ids {
            g.remove_edge(&id);
        }
        let shake = g
            .nodes
            .iter()
            .find(|n| n.kind == "fx.shake")
            .unwrap()
            .id
            .clone();
        let crt = g
            .nodes
            .iter()
            .find(|n| n.kind == "fx.crt")
            .unwrap()
            .id
            .clone();
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&crt, "image"));
        g.add_edge(Endpoint::new(&crt, "image"), Endpoint::new(&shake, "image"));
        g.add_edge(Endpoint::new(&shake, "image"), Endpoint::new(&out, "image"));

        // The plan is a compiled snapshot (`the design notes` §6: "el
        // grafo se compila, no se interpreta") — recompiling after rewiring
        // is the caller's job, same as a live edit in the editor would
        // trigger, not something `run` does on the caller's behalf.
        let plan = g.compile();
        let after = stand.run(&g, &plan, &rows, &HashMap::new(), &[], 1.0, 16, 9);
        let order_after: Vec<u32> = after
            .chains
            .get(&gen)
            .unwrap()
            .iter()
            .map(|s| s.effect)
            .collect();
        assert_eq!(
            order_after[0],
            metal_schema::effect_index("crt").unwrap() as u32,
            "the resolved chain must follow the graph as it now is"
        );
        assert_ne!(order_before, order_after);
    }

    /// `fx.estela` chained after a generator must not panic,
    /// and — now that Set F has wired `metal_schema::footage_chain_kind`
    /// into `crate::resolved_chain` — must actually show up in
    /// `RunOutput.chains`, the same as any other `fx.*` node the operator
    /// gave a fixed amount.
    #[test]
    fn estela_chained_after_a_generator_resolves_to_its_own_chain_kind_and_does_not_panic() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "wake",
            metal_schema::EffectState::Fixed(0.5),
            metal_graph::effects::ControlSource::RawIngest,
        )
        .expect("materializes");
        let plan = g.compile();

        let mut stand = crate::stand::Stand::new();
        let rows: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
            [(gen.clone(), stand.schemas()[0].defaults())]
                .into_iter()
                .collect();

        let result = stand.run(&g, &plan, &rows, &HashMap::new(), &[], 1.0, 16, 9);
        assert_eq!(
            result.chains.get(&gen),
            Some(&vec![metal_schema::EffectStep {
                effect: metal_schema::CHAIN_KIND_ESTELA,
                amount: 0.5,
                slot: 0
            }]),
            "estela now resolves to its own chain kind rather than to no step at all"
        );
        // And the image itself still forwards through the fx branch — this
        // must not have regressed while fixing the input-ordering bug B5
        // was about.
        assert_eq!(result.readbacks, 1);
        assert_eq!(result.frames.len(), 1);
    }

    /// The whole advanced bake path, end to end through `Paint::run`: a
    /// `fijo` `fx.grade` node's own config keys must reach the rendered CPU
    /// frame (not just `RunOutput.chains`), and a neutral grade node must be
    /// byte-identical to no grade node at all.
    #[test]
    fn a_fijo_grade_node_changes_the_rendered_cpu_frame() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        metal_graph::effects::set_effect(
            &mut g,
            &gen,
            "grade",
            metal_schema::EffectState::Fixed(0.0),
            metal_graph::effects::ControlSource::Row,
        )
        .expect("materializes");
        let grade = metal_graph::effects::chain(&g, &gen)
            .into_iter()
            .find(|id| metal_graph::effects::effect_name(&g, id).as_deref() == Some("grade"))
            .expect("the grade node");

        let mut stand = crate::stand::Stand::new();
        let rows: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
            [(gen.clone(), stand.schemas()[0].defaults())]
                .into_iter()
                .collect();
        let neutral = stand.run(&g, &g.compile(), &rows, &HashMap::new(), &[], 1.0, 32, 18);
        let neutral = neutral.frames.values().next().expect("a frame").0.clone();

        // Drive one grade parameter the way the node's panel does, then render
        // the very same graph/row/time again.
        crate::bind::set_config_number(&mut g, &grade, "grade_exposure", 60.0);
        let graded = stand.run(&g, &g.compile(), &rows, &HashMap::new(), &[], 1.0, 32, 18);
        let graded = graded.frames.values().next().expect("a frame").0.clone();

        assert_ne!(
            neutral, graded,
            "a fijo fx.grade node must change the CPU frame, not just the chain"
        );
        let mean = |px: &[u8]| -> f64 {
            px.chunks_exact(4).map(|p| p[0] as u64).sum::<u64>() as f64 / (px.len() / 4) as f64
        };
        assert!(mean(&graded) > mean(&neutral), "exposure + must brighten");
    }

    // -----------------------------------------------------------------------
    // K — a per-generator clock and `fx.move`
    // -----------------------------------------------------------------------

    /// `run_with_clocks` draws a generator at the clock it is handed, not at
    /// the flat show `time`: a node given a future clock must render exactly
    /// what `solo` draws at that time. This is the seam `fx.speed` reaches the
    /// renderers through.
    #[test]
    fn a_generator_draws_at_its_own_clock() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        let plan = g.compile();

        let mut stand = crate::stand::Stand::new();
        let index = stand.ids().iter().position(|id| *id == "turing").unwrap();
        let row = stand.schemas()[index].defaults();
        let rows: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
            [(gen.clone(), row.clone())].into_iter().collect();

        let clocks: std::collections::HashMap<metal_graph::NodeId, f32> =
            [(gen.clone(), 9.0)].into_iter().collect();
        let out_frame =
            stand.run_with_clocks(&g, &plan, &rows, &HashMap::new(), &[], 1.0, 32, 18, &clocks, &HashMap::new());
        let at_nine = stand.solo(index, &row, 9.0, 32, 18, false);

        let got = out_frame.frames.values().next().expect("one sink frame");
        assert_eq!(got.0, at_nine.0, "the generator was not drawn at its own clock");
    }

    /// With no clock for the generator, `run_with_clocks` is `run`: drawn at
    /// the show time — the identity every existing caller relies on.
    #[test]
    fn a_missing_clock_falls_back_to_the_show_time() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        let plan = g.compile();

        let mut stand = crate::stand::Stand::new();
        let index = stand.ids().iter().position(|id| *id == "turing").unwrap();
        let row = stand.schemas()[index].defaults();
        let rows: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
            [(gen.clone(), row.clone())].into_iter().collect();
        let with_empty =
            stand.run_with_clocks(&g, &plan, &rows, &HashMap::new(), &[], 2.5, 32, 18, &HashMap::new(), &HashMap::new());
        let plain = stand.run(&g, &plan, &rows, &HashMap::new(), &[], 2.5, 32, 18);
        assert_eq!(with_empty.frames.values().next().unwrap().0, plain.frames.values().next().unwrap().0);
    }

    /// `fx.move` offsets the picture on the stand-in (its `forge_fx` runs the
    /// move honestly on the CPU) — a generator through a move node is not the
    /// same frame as the generator alone.
    #[test]
    fn fx_move_offsets_the_picture_on_the_stand_in() {
        let plain = {
            let mut g = Graph::new("g", "t", Domain::Live);
            let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
            let out = g.add_node("out", "output.device", [3.0, 0.0]);
            g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
            g
        };
        let moved = {
            let mut g = Graph::new("g", "t", Domain::Live);
            let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
            let mv = g.add_node("fx", crate::motion::FX_MOVE_KIND, [1.0, 0.0]);
            let out = g.add_node("out", "output.device", [3.0, 0.0]);
            g.node_mut(&mv).unwrap().config =
                metal_graph::Config::from_json_str(r#"{"x":0.3,"y":0.0,"vx":0.0,"vy":0.0,"wrap":1}"#).unwrap();
            g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&mv, "image"));
            g.add_edge(Endpoint::new(&mv, "image"), Endpoint::new(&out, "image"));
            g
        };
        let mut stand = crate::stand::Stand::new();
        let index = stand.ids().iter().position(|id| *id == "turing").unwrap();
        let row = stand.schemas()[index].defaults();
        let gen_of = |g: &metal_graph::Graph| g.nodes.iter().find(|n| n.kind == "gen.turing").unwrap().id.clone();
        let rows_plain: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
            [(gen_of(&plain), row.clone())].into_iter().collect();
        let rows_moved: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
            [(gen_of(&moved), row.clone())].into_iter().collect();
        let a = stand.run(&plain, &plain.compile(), &rows_plain, &HashMap::new(), &[], 3.0, 32, 18);
        let b = stand.run(&moved, &moved.compile(), &rows_moved, &HashMap::new(), &[], 3.0, 32, 18);
        assert_ne!(
            a.frames.values().next().unwrap().0,
            b.frames.values().next().unwrap().0,
            "fx.move did not shift the picture"
        );
        assert_eq!(b.readbacks, 1, "the move runs in the same one readback");
    }

    // -----------------------------------------------------------------------
    // D7 — material ports: a chain wired only to a `stroke` is rendered and kept
    // -----------------------------------------------------------------------

    /// The bug this lot closes, at its lowest layer: a generator wired *only*
    /// to a `mask` node's `stroke` port — never reaching an `output.device` —
    /// is still rendered and handed back, keyed by the consuming node and
    /// port. Before `RunOutput::materials` existed the tile had nowhere to
    /// come from, and `apply_mask_strokes` had nothing to sample.
    #[test]
    fn a_generator_wired_only_to_a_masks_stroke_is_kept_as_a_material() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let mask = g.add_node("m", "mask", [0.0, 1.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&mask, "stroke"));
        let plan = g.compile();

        let mut stand = crate::stand::Stand::new();
        let rows: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
            [(gen.clone(), stand.schemas()[0].defaults())]
                .into_iter()
                .collect();
        let result = stand.run(&g, &plan, &rows, &HashMap::new(), &[], 1.0, 16, 9);

        let tile = result
            .materials
            .get(&(mask.clone(), "stroke".to_string()))
            .expect("the stroke material must be kept, keyed by (mask, stroke)");
        assert_eq!(tile.1, 16);
        assert_eq!(tile.2, 9);
        assert_eq!(result.readbacks, 1, "the material is one readback, like any generator");
    }

    /// With no material edge at all the map is empty — so the consumer's
    /// `materials.get(mask.id)` is `None` and the flat colour paints, the
    /// exact behaviour every stroked mask had before this lot.
    #[test]
    fn a_graph_with_no_material_edge_has_no_materials() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        let plan = g.compile();

        let mut stand = crate::stand::Stand::new();
        let rows: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
            [(gen.clone(), stand.schemas()[0].defaults())]
                .into_iter()
                .collect();
        let result = stand.run(&g, &plan, &rows, &HashMap::new(), &[], 1.0, 16, 9);
        assert!(result.materials.is_empty());
    }

    /// A material chain *with an `fx.*` node after the generator* is still
    /// keyed by the consumer: the `fx.*` branch forwards the frame along the
    /// chain, and the last node's buffer is the one the `stroke` edge names.
    #[test]
    fn an_fx_chain_before_a_stroke_port_is_kept_as_the_material() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let fx = g.add_node("fx", "fx.shake", [1.0, 0.0]);
        let mask = g.add_node("m", "mask", [0.0, 1.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&fx, "image"));
        g.add_edge(Endpoint::new(&fx, "image"), Endpoint::new(&mask, "stroke"));
        let plan = g.compile();

        let mut stand = crate::stand::Stand::new();
        let rows: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
            [(gen.clone(), stand.schemas()[0].defaults())]
                .into_iter()
                .collect();
        let result = stand.run(&g, &plan, &rows, &HashMap::new(), &[], 1.0, 16, 9);
        assert!(
            result.materials.contains_key(&(mask.clone(), "stroke".to_string())),
            "the tail of the fx chain wired to `stroke` is the material"
        );
    }
}

#[cfg(test)]
mod surface_tests {
    use super::*;
    use metal_graph::{Config, Domain, Endpoint, Graph};

    /// A stand-in painter that records every `set_surfaces` call and otherwise
    /// draws exactly as the CPU stand-in does.
    struct Recorder {
        inner: crate::stand::Stand,
        /// Per call: `(model index, [(part, Some((image, material kind)) | None)])`
        /// where `image` is `Some((w, h, mix, mapping, scale))`.
        calls: Vec<(
            usize,
            Vec<(
                String,
                Option<(Option<(u32, u32, f32, SurfaceMapping, f32)>, Option<f32>)>,
            )>,
        )>,
        /// Number of `solo_chain` draws, to count renders.
        draws: usize,
    }

    impl Paint for Recorder {
        fn model_count(&self) -> usize {
            self.inner.model_count()
        }
        fn ids(&self) -> Vec<&'static str> {
            self.inner.ids()
        }
        fn names(&self) -> Vec<&'static str> {
            self.inner.names()
        }
        fn schemas(&self) -> Vec<&'static Schema> {
            self.inner.schemas()
        }
        fn solo(
            &mut self,
            index: usize,
            floats: &[f32],
            time: f32,
            w: u32,
            h: u32,
            background_alpha: bool,
        ) -> (Vec<u8>, u32, u32) {
            self.draws += 1;
            self.inner.solo(index, floats, time, w, h, background_alpha)
        }
        fn set_surfaces(&mut self, index: usize, parts: &[(&str, Option<SurfaceSpec>)]) {
            let parts = parts
                .iter()
                .map(|(part, spec)| {
                    let spec = spec.map(|s| {
                        let image = s
                            .image
                            .map(|p| (p.width, p.height, p.mix, p.mapping, p.scale));
                        (image, s.material.map(|m| m.kind))
                    });
                    (part.to_string(), spec)
                })
                .collect();
            self.calls.push((index, parts));
        }
        fn footage(&self) -> Vec<bool> {
            self.inner.footage()
        }
        fn backdrop(&self) -> Vec<bool> {
            self.inner.backdrop()
        }
        fn reel(&mut self, index: usize) -> Option<&mut dyn metal_reel::Reel> {
            self.inner.reel(index)
        }
        fn real(&self) -> bool {
            self.inner.real()
        }
        fn describe(&self) -> String {
            self.inner.describe()
        }
    }

    fn recorder() -> Recorder {
        Recorder { inner: crate::stand::Stand::new(), calls: Vec::new(), draws: 0 }
    }

    /// `gen.turing` feeding `gen.monstruo`'s `tex.skin`, and monstruo feeding
    /// the device. Turing reaches an output only through the surface.
    fn surface_graph(config: &str) -> (Graph, String, String) {
        let mut g = Graph::new("g", "t", Domain::Live);
        let turing = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let monstruo = g.add_node("gen", "gen.monster", [1.0, 0.0]);
        let out = g.add_node("out", "output.device", [2.0, 0.0]);
        g.add_edge(Endpoint::new(&monstruo, "image"), Endpoint::new(&out, "image"));
        g.add_edge(Endpoint::new(&turing, "image"), Endpoint::new(&monstruo, "tex.skin"));
        g.node_mut(&monstruo).expect("monster").config = Config::from_json_str(config).expect("config parses");
        (g, turing, monstruo)
    }

    /// A surface source renders though nothing downstream of it is an output,
    /// and its picture is kept as the consumer's material tile.
    #[test]
    fn a_surface_source_renders_and_is_kept_as_the_consumers_material() {
        let (g, turing, monstruo) = surface_graph("{}");
        let plan = g.compile();
        let mut painter = crate::stand::Stand::new();
        let out = painter.run(&g, &plan, &HashMap::new(), &HashMap::new(), &[], 0.0, 64, 48);
        assert!(out.chains.contains_key(&turing), "turing was rendered");
        let (px, w, h) = out
            .materials
            .get(&(monstruo.clone(), "tex.skin".to_string()))
            .expect("the turing picture is recorded as monstruo's tex.skin material");
        assert_eq!((*w, *h), (64, 48));
        assert_eq!(px.len(), 64 * 48 * 4);
    }

    /// Every declared part is handed to the painter before the draw: the wired
    /// one with its picture and the consumer's `surfaces.<part>` settings, the
    /// unwired one as `None`. A second run without the cable clears the part.
    #[test]
    fn the_consumer_hands_its_wired_picture_and_its_settings_to_the_painter() {
        let config = r#"{"surfaces":{"skin":{"mix":0.5,"mapping":"projector","scale":2.0}}}"#;
        let (g, _, monstruo) = surface_graph(config);
        let plan = g.compile();
        let mut painter = recorder();
        let index = painter.ids().iter().position(|id| *id == "monster").expect("monstruo is registered");
        let _ = painter.run(&g, &plan, &HashMap::new(), &HashMap::new(), &[], 0.0, 64, 48);

        let (at, parts) = painter.calls.last().expect("set_surfaces was called").clone();
        assert_eq!(at, index);
        assert_eq!(parts.len(), 2, "every declared part is passed");
        let skin = parts.iter().find(|(p, _)| p == "skin").expect("skin");
        let (image, material) = skin.1.as_ref().expect("skin carries a spec");
        assert_eq!(*image, Some((64, 48, 0.5, SurfaceMapping::Projector, 2.0)));
        assert!(material.is_none(), "an all-zero material row carries no material");
        let eyes = parts.iter().find(|(p, _)| p == "eyes").expect("eyes");
        assert_eq!(eyes.1, None, "an unwired part is cleared");

        // The same consumer without the cable: nothing is left on skin.
        let mut g2 = g.clone();
        let edge = g2.edges.iter().find(|e| e.to.port == "tex.skin").expect("the cable").id.clone();
        g2.remove_edge(&edge);
        let plan2 = g2.compile();
        let _ = painter.run(&g2, &plan2, &HashMap::new(), &HashMap::new(), &[], 0.0, 64, 48);
        let (_, parts) = painter.calls.last().expect("called again").clone();
        assert!(parts.iter().all(|(_, p)| p.is_none()), "nothing leaks into the next run");
        let _ = monstruo;
    }

    /// A row with a live `material` group hands the same material to every
    /// declared part, with or without a picture on it; a mix-0 group (the
    /// migrated default) hands nothing.
    #[test]
    fn a_material_row_reaches_every_part_and_a_mix_zero_row_reaches_none() {
        let (g, _, monstruo) = surface_graph("{}");
        let plan = g.compile();
        let mut painter = recorder();
        let index = painter.ids().iter().position(|id| *id == "monster").expect("monster");
        let width = painter.schemas()[index].width();

        let mut row = vec![0.0f32; width];
        let at = width - metal_schema::EFFECTS_WIDTH + metal_schema::CHAIN_FLOATS + metal_schema::N_ADVANCED;
        row[at] = 3.0; // mat_kind: leather
        row[at + 1] = 1.0; // mat_mix: on
        let rows: HashMap<metal_graph::NodeId, Vec<f32>> =
            [(monstruo.clone(), row)].into_iter().collect();
        let _ = painter.run(&g, &plan, &rows, &HashMap::new(), &[], 0.0, 64, 48);

        let (_, parts) = painter.calls.last().expect("set_surfaces was called").clone();
        for (part, spec) in &parts {
            let (_, material) = spec.as_ref().unwrap_or_else(|| panic!("{part} carries a spec"));
            assert_eq!(*material, Some(3.0), "{part} wears the row's leather");
        }

        // The same row with `mat_mix` back to 0 (the migrated default) is off.
        let mut row = vec![0.0f32; width];
        row[at] = 3.0;
        let rows: HashMap<metal_graph::NodeId, Vec<f32>> =
            [(monstruo, row)].into_iter().collect();
        let _ = painter.run(&g, &plan, &rows, &HashMap::new(), &[], 0.0, 64, 48);
        let (_, parts) = painter.calls.last().expect("called again").clone();
        assert!(
            parts
                .iter()
                .all(|(_, p)| p.as_ref().map_or(true, |(_, m)| m.is_none())),
            "a mix-0 material row is nothing, like the default"
        );
    }
}
