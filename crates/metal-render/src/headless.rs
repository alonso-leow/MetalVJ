//! Headless rendering: one GPU with no window, all the models, and two ways to
//! ask for pixels — the whole grid at once, or one model large.
//!
//! Lives here rather than in either app because both need it: the studio
//! previews every model side by side while you author, and the app renders
//! whichever model is on air. One headless path, two consumers.
//!
//! Everything here runs on the request thread. There is no render loop and no
//! background worker: the browser asks for a frame, this draws it, encodes it
//! and hands it back. The page requesting the next frame only when the last
//! one arrived is what paces it, so a slow model slows the preview down
//! instead of piling up work nobody is waiting for any more.

use crate::gpu::{offscreen_target, read_rgba8, Gpu};
use crate::model::{read_post, Ctx, Model};
use crate::registry::all_models;
use crate::shared::post::{ChainFx, EstelaParams, GradeUniform, PostChain, PostParams, OVERSCAN};
use crate::shared::brush::{BrushParams, Brushes};
use crate::shared::stutter::{Stutter, StutterParams};
use crate::shared::trail::{EstelaRings, TrailChain, Trails};
use crate::shared::ColorSlotGpu;
use metal_ingest::export::{self as ingest_export, EncodeSettings, Encoder, FrameSource};
use metal_schema::{EffectStep, CHAIN_KIND_ESTELA};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// The pixel count a tile is judged against when it asks for less work (1080p).
const DETAIL_REFERENCE_PIXELS: f32 = 1920.0 * 1080.0;
/// The floor on `Ctx::detail`: a tile never gets less than a quarter of the authored counts.
const MIN_DETAIL: f32 = 0.25;
/// The floor on a model's render scale once the global quality is applied.
const MIN_RENDER_SCALE: f32 = 0.1;
/// Scratch sizes kept alive at once. Models at different scales alternate in a
/// grid, so one slot would reallocate every tile; a handful covers any mix.
const SCRATCH_CACHE_MAX: usize = 8;

/// The overscanned size a `w x h` tile is drawn at when its model runs at
/// `scale` of the output's linear size. `scale` of `1.0` is the authored size.
pub fn scratch_size(w: u32, h: u32, scale: f32) -> (u32, u32) {
    let ow = ((w as f32) * OVERSCAN * scale) as u32;
    let oh = ((h as f32) * OVERSCAN * scale) as u32;
    (ow.max(1), oh.max(1))
}

/// `Ctx::detail` for a `w x h` tile: its pixel count over 1920x1080, clamped to
/// `MIN_DETAIL..=1.0`, so a tile at 1080p or above keeps the authored counts.
pub fn detail_factor(w: u32, h: u32) -> f32 {
    ((w as f32) * (h as f32) / DETAIL_REFERENCE_PIXELS).clamp(MIN_DETAIL, 1.0)
}

/// 8-bit sRGB, unlike the window's `Rgba16Float`: this path ends in a JPEG,
/// so the sRGB encode the hardware does on write is exactly the conversion
/// that would otherwise have to happen by hand on the way out.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// The amount of the `fx.brush` step in `steps`, if it is on.
fn brush_amount(steps: &[EffectStep]) -> Option<f32> {
    let kind = metal_schema::advanced_chain_kind("brush")?;
    steps.iter().find(|s| s.effect == kind && s.amount > 0.0).map(|s| s.amount)
}

pub struct Renderer {
    gpu: Gpu,
    post: PostChain,
    trail: TrailChain,
    models: Vec<Box<dyn Model>>,
    /// The overscanned textures each model draws into before the post chain
    /// crops it down, keyed by their size (a model's render scale sets it).
    scratch: HashMap<(u32, u32), (wgpu::Texture, wgpu::TextureView)>,
    /// The app's global render quality, `0.25..=1.0`, multiplied into every
    /// model's own `render_scale`. See [`Renderer::set_quality`].
    quality: f32,
    /// The composite the tiles are assembled into, then read back once.
    canvas: Option<(wgpu::Texture, wgpu::TextureView, u32, u32)>,
    /// One frame history **per model**, for ghosting. Per model rather than
    /// per renderer because there is only one scratch texture and every tile
    /// of the grid draws into it in turn — a single shared history would smear
    /// each model's frame into the next one's tile.
    trails: Trails,
    /// The same idea, one level over, for `fx.estela`'s chain step: a ring
    /// of undecayed past frames per model, instead of ghost's one decayed
    /// one — see `shared::trail`'s own module doc for why this is a second
    /// type rather than `trails` widened.
    estela_rings: EstelaRings,
    /// `fx.brush`: one persistent canvas per model, applied between the model
    /// and the post pass (`shared::brush` has the design).
    brushes: Brushes,
    /// `fx.forge`: custom effect passes, one pipeline per shader key.
    forge_fx: crate::models::forge::ForgeFx,
    /// `fx.warp` (loops T12): the mesh pass and each node's history.
    warp_fx: crate::shared::warp::WarpFx,
    /// `fx.hardstutter`: the compose pass and each model's held frames (see
    /// `shared::stutter` for why this is its own texture and not a lookup).
    stutter: Stutter,
    /// CPU time the last `draw_tile` spent inside `Model::render` alone (the
    /// model's own encode + submit, not the post chain) — for `render-bench`.
    pub last_model_cpu: std::time::Duration,
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer::with_gpu(Gpu::new_headless())
    }

    /// The same renderer on a device the caller already has. The browser
    /// cannot block on an adapter (`pollster` would spin on a promise the
    /// page only resolves when this thread yields), so its device comes from
    /// [`Gpu::web`] and is handed in here.
    pub fn with_gpu(gpu: Gpu) -> Renderer {
        let post = PostChain::new(&gpu.device, FORMAT);
        let trail = TrailChain::new(&gpu.device, FORMAT);
        let stutter = Stutter::new(&gpu.device, FORMAT);
        Renderer {
            gpu,
            post,
            trail,
            models: all_models(),
            scratch: HashMap::new(),
            quality: 1.0,
            canvas: None,
            trails: Trails::default(),
            estela_rings: EstelaRings::default(),
            brushes: Brushes::default(),
            forge_fx: crate::models::forge::ForgeFx::default(),
            warp_fx: crate::shared::warp::WarpFx::default(),
            stutter,
            last_model_cpu: std::time::Duration::ZERO,
        }
    }

    /// The device and queue this renderer draws on (cheap handle clones), for
    /// passes that live outside this crate — `metal-perform`'s GPU text.
    pub fn device_queue(&self) -> (wgpu::Device, wgpu::Queue) {
        (self.gpu.device.clone(), self.gpu.queue.clone())
    }

    /// A handle to the device this renderer draws on (see
    /// [`Gpu::share`]).
    pub fn shared_gpu(&self) -> Gpu {
        self.gpu.share()
    }

    /// How many models hold an `fx.brush` canvas right now (0 whenever no row
    /// has the effect on). For tests and "is this costing anything".
    pub fn brush_canvases(&self) -> usize {
        self.brushes.allocated()
    }

    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    /// Whether model `index` can draw on this device: every model can where the
    /// adapter runs compute shaders, and only the ones that do not need them
    /// where it does not (WebGL2). A caller that gets `false` draws the model
    /// some other way; this renderer must not be asked to.
    pub fn runs_here(&self, index: usize) -> bool {
        self.gpu.compute || !self.models.get(index).is_some_and(|m| m.needs_compute())
    }

    /// Whether the adapter is real hardware (not a software rasterizer).
    pub fn real_adapter(&self) -> bool {
        self.gpu.real_adapter
    }

    /// The backend the device is on, as the page names it.
    pub fn backend_name(&self) -> &'static str {
        match self.gpu.backend {
            wgpu::Backend::Vulkan => "Vulkan",
            wgpu::Backend::Metal => "Metal",
            wgpu::Backend::Dx12 => "DirectX 12",
            wgpu::Backend::Gl => "WebGL2",
            wgpu::Backend::BrowserWebGpu => "WebGPU",
            wgpu::Backend::Noop => "no-op",
        }
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.models.iter().map(|m| m.name()).collect()
    }

    /// The schema ids, in the same order as everything else here. This is
    /// what a `/live` push names a model by.
    pub fn ids(&self) -> Vec<&'static str> {
        self.models.iter().map(|m| m.id()).collect()
    }

    pub fn schemas(&self) -> Vec<&'static crate::model::Schema> {
        self.models.iter().map(|m| m.schema()).collect()
    }

    /// Which models are transmissions — they draw over footage rather than
    /// generating a picture, and cannot be selected until a library is
    /// attached. Same order as everything else here.
    pub fn footage(&self) -> Vec<bool> {
        self.models.iter().map(|m| m.footage()).collect()
    }

    /// Which models draw their background via the shared `Backdrop` pass —
    /// see [`crate::model::Model::backdrop`]. Same order as everything else
    /// here.
    pub fn backdrop(&self) -> Vec<bool> {
        self.models.iter().map(|m| m.backdrop()).collect()
    }

    /// The footage side of model `index`, if it has one.
    ///
    /// The app attaches a library and pushes the Curator's cue through here.
    /// It is a lookup by index rather than by id for the same reason `/select`
    /// is: an operator's on-air choice is a number into this list.
    pub fn reel(&mut self, index: usize) -> Option<&mut dyn crate::model::Reel> {
        self.models.get_mut(index).and_then(|m| m.reel())
    }

    /// Hands model `index` one part's surfaces (a picture and/or a material),
    /// or takes them away with `None` (see `Model::set_surface`). Unknown
    /// index: no-op.
    pub fn set_surface(&mut self, index: usize, part: &str, spec: Option<crate::model::SurfaceSpec>) {
        if let Some(m) = self.models.get_mut(index) {
            m.set_surface(part, spec);
        }
    }

    /// Global render quality: `1.0` is the authored look, lower renders every
    /// model at a smaller scale and lets the post pass upscale it. Clamped to
    /// `0.25..=1.0`; the app lowers it on weak GPUs.
    pub fn set_quality(&mut self, quality: f32) {
        let quality = if quality.is_finite() { quality } else { 1.0 };
        self.quality = quality.clamp(0.25, 1.0);
    }

    /// The render scale model `index` draws at this frame: its own scale times
    /// the global quality.
    fn render_scale_for(&self, index: usize) -> f32 {
        let model = self.models.get(index).map_or(1.0, |m| m.render_scale());
        (model * self.quality).clamp(MIN_RENDER_SCALE, 1.0)
    }

    /// Makes sure a scratch texture of `size` exists. Dropping the whole cache
    /// when it is full is the simple bound: the next tiles just reallocate.
    fn scratch_for(&mut self, size: (u32, u32)) {
        if !self.scratch.contains_key(&size) {
            if self.scratch.len() >= SCRATCH_CACHE_MAX {
                self.scratch.clear();
            }
            let (t, v) = offscreen_target(&self.gpu.device, size.0, size.1, FORMAT);
            self.scratch.insert(size, (t, v));
        }
    }

    /// Ensures the composite exists at this size. Returns nothing on
    /// purpose: handing back a `&Texture` borrowed from `&mut self` would
    /// keep the whole renderer borrowed while the readback needs the device.
    fn ensure_canvas(&mut self, w: u32, h: u32) {
        let stale = !matches!(&self.canvas, Some((_, _, cw, ch)) if *cw == w && *ch == h);
        if stale {
            let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("headless canvas"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.canvas = Some((texture, view, w, h));
        }
    }

    /// Draws model `index` into `rect` of the canvas. The model draws into the
    /// scratch texture first, at its render scale of the tile (full tile size
    /// unless the model or quality lowers it); the post chain is what places
    /// it, so shake and the leaks land per tile instead of smearing across the
    /// whole grid.
    ///
    /// The effect *amounts* are read out of `floats` itself — `read_post`,
    /// the mirror of every model's schema being widened by
    /// `EFFECTS_FLOATS_BLOCK` and `EFFECTS_COLOR_BLOCK` — rather than taken
    /// as a separate argument: they are keyframed the same as every other
    /// float now, so each tile's own row is the only place they can
    /// honestly come from. The *order* the ten common effects run in is a
    /// separate question `floats` structurally cannot answer — `read_post`
    /// always hands back an empty `chain`, see its doc comment — so it is
    /// this function's own `chain` parameter: non-empty means an operator
    /// arranged these as nodes and this is that arrangement; empty means no
    /// graph is driving this row, so the order this row's own flat amounts
    /// imply is what `metal_schema::Post::resolved_chain` derives below.
    ///
    /// `wake` is a third question `floats` cannot
    /// answer at all, not even when `chain` is non-empty: a
    /// `fx.estela` step names *that* a chain step reads that kind's look,
    /// not *which* look, because there is no row field for that kind's own
    /// floats to ride on once it is chained after a different generator —
    /// see [`EstelaParams`]'s own doc comment. Every caller
    /// today passes its `::default` (that model's shipped look); the
    /// parameter exists at this boundary rather than being hardcoded inside
    /// so that stays this function's caller's decision, not this function's.
    #[allow(clippy::too_many_arguments)]
    fn draw_tile(
        &mut self,
        index: usize,
        floats: &[f32],
        time: f32,
        rect: (u32, u32, u32, u32),
        clear: bool,
        fondo_alpha: bool,
        chain: &[EffectStep],
        wake: &EstelaParams,
        canvas_view: &wgpu::TextureView,
    ) {
        let padded = full_row(floats);
        let floats: &[f32] = &padded;
        let post = read_post(floats);
        // Empty means no graph is driving this row — `floats` cannot carry
        // an order (see `read_post`'s doc comment), so the legacy order this
        // row's own flat amounts imply is what `resolved_chain` derives.
        // Resolved up front rather than only just before `apply_tile`,
        // because the estela-ring gate just below needs to know whether
        // *this* row's chain actually names `fx.estela` too.
        let legacy;
        let steps: &[EffectStep] = if chain.is_empty() {
            legacy = post.resolved_chain();
            &legacy
        } else {
            chain
        };
        let estela_active =
            steps.iter().any(|s| s.effect == CHAIN_KIND_ESTELA && s.amount > 0.0);

        // `fx.hardstutter` is stateful and freezes the final picture, so when
        // its step is on the post chain draws into an intermediate and the
        // stutter pass composes that with the held frame into the target.
        let stutter_kind = metal_schema::advanced_chain_kind("hardstutter");
        let stutter_cant = steps
            .iter()
            .find(|s| Some(s.effect) == stutter_kind && s.amount > 0.0)
            .map(|s| s.amount);
        if stutter_cant.is_none() {
            self.stutter.release(index);
        }

        let (_, _, w, h) = rect;
        // The model draws at its render scale; the post pass samples the
        // scratch in UV, so a smaller scratch is upscaled back to the tile.
        let (ow, oh) = scratch_size(w, h, self.render_scale_for(index));
        // Split the borrow: `scratch_for` needs `&mut self`, everything after
        // it needs the view plus other fields at the same time.
        self.scratch_for((ow, oh));
        let Some((scratch_tex, scratch_view)) = self.scratch.get(&(ow, oh)) else { return };
        let ctx = Ctx {
            device: &self.gpu.device,
            queue: &self.gpu.queue,
            time,
            width: ow,
            height: oh,
            fondo_alpha,
            detail: detail_factor(w, h),
        };
        // The row's palette motion (`col_*`) moves its colour slots around the
        // slots' own values before the model reads them.
        let moved = metal_schema::apply_palette_motion(self.models[index].schema(), floats);
        let started = Instant::now();
        self.models[index].render(&ctx, moved.as_deref().unwrap_or(floats), scratch_view, FORMAT);
        self.last_model_cpu = started.elapsed();

        // Ghosting sits **between** the model and the post chain, so the trail
        // is of the model's own frame: put it after, and a light leak smears
        // across the next second of output along with everything else.
        //
        // Off means off. At `ghost == 0` this branch is not taken, no history
        // is allocated and no pass runs, which is what keeps a cold renderer
        // asked for one frame byte-identical to what it drew before ghosting
        // existed — the property both of the studio's render gates rest on.
        //
        // `fx.brush` repaints the model's frame first of all (it is the one
        // step that is not applied in chain order: see `shared::brush`), so
        // ghost trails the painting and the post pass grades it.
        let painted: &wgpu::TextureView = match brush_amount(steps) {
            Some(amount) => {
                let mut bp = BrushParams::from_advanced(&post.advanced);
                bp.cant = amount;
                self.brushes
                    .advance(&self.gpu.device, &self.gpu.queue, self.gpu.compute, index, scratch_view, (ow, oh), time, &bp)
                    .unwrap_or(scratch_view)
            }
            None => scratch_view,
        };
        let source: &wgpu::TextureView = if post.ghost > 0.0 {
            self.trails
                .advance(
                    &self.trail,
                    &self.gpu.device,
                    &self.gpu.queue,
                    FORMAT,
                    index,
                    painted,
                    (ow, oh),
                    time,
                    post.ghost_tau(),
                )
                .unwrap_or(painted)
        } else {
            painted
        };

        // The estela ring, the same "off means off, nothing allocated, no
        // pass runs" posture as ghost just above — `EstelaRings::advance`'s
        // own cost is one `copy_texture_to_texture` at most, but that is
        // still one more than a chain that never names `fx.estela` should
        // pay. Reads the model's own frame (`scratch_tex`/`scratch_view`),
        // same as ghost, and *before* ghost's own decay — a block-mosh on
        // top of a ghost trail is a stranger effect than either alone, and
        // nothing asks for it; keeping the ring keyed to the model's raw
        // output is the simpler, honest choice.
        let estela_ring = if estela_active {
            self.estela_rings.advance(
                &self.gpu.device,
                &self.gpu.queue,
                FORMAT,
                index,
                scratch_tex,
                scratch_view,
                (ow, oh),
                time,
            )
        } else {
            crate::shared::trail::fallback_ring(scratch_view)
        };

        let mut leak = [1.0f32; 12];
        for (dst, src) in leak.iter_mut().zip(post.leak_color.iter()) {
            *dst = *src;
        }
        let (post_dst, post_rect, post_clear) = match stutter_cant {
            Some(_) => (self.stutter.live_view(&self.gpu.device, w, h), (0, 0, w, h), true),
            None => (canvas_view.clone(), rect, clear),
        };
        self.post.apply_tile(
            &self.gpu.device,
            &self.gpu.queue,
            source,
            &post_dst,
            post_rect,
            post_clear,
            &PostParams {
                shake: post.shake,
                light_leak: post.light_leak,
                leak_color: ColorSlotGpu::from_floats(&leak),
                time,
                kaleido: post.kaleido,
                wake: *wake,
                grade: GradeUniform::from_advanced(&post.advanced),
                chain_fx: ChainFx::from_advanced(&post.advanced),
            },
            steps,
            &estela_ring,
        );
        if let Some(cant) = stutter_cant {
            self.stutter.compose(
                &self.gpu.device,
                &self.gpu.queue,
                index,
                &StutterParams::from_advanced(&post.advanced),
                cant,
                time,
                canvas_view,
                rect,
                clear,
            );
        }
    }

    /// Throws away every model's trail if ghosting is off everywhere.
    ///
    /// Called at the top of both entry points rather than when the slider
    /// moves, because the renderer never hears about a slider directly — it
    /// reads `ghost` back out of whichever rows it was just handed. Without
    /// this, turning the effect off and on again resumes a trail from
    /// wherever the clock was the last time it was on. `grid` has many rows
    /// and one trail pool, so this releases only when *none* of them want a
    /// ghost — any single row still ghosting is reason enough to keep it.
    fn release_trails_if_off(&mut self, any_ghost: bool) {
        if !any_ghost && self.trails.allocated() > 0 {
            self.trails.clear();
        }
    }

    /// Throws every brush canvas away when no row wants `fx.brush` any more
    /// ([`Self::release_trails_if_off`]'s reasoning).
    fn release_brushes_if_off(&mut self, any_brush: bool) {
        if !any_brush && self.brushes.allocated() > 0 {
            self.brushes.clear();
        }
    }

    /// [`Self::release_trails_if_off`]'s own reasoning, for the estela ring:
    /// turning a `fx.estela` step off (or dragging it out of a chain
    /// entirely) releases whatever ring memory was allocated for it, so
    /// switching it back on starts from the live frame instead of resuming
    /// a ring left over from whenever it was last active.
    fn release_estela_rings_if_off(&mut self, any_estela: bool) {
        if !any_estela && self.estela_rings.allocated() > 0 {
            self.estela_rings.clear();
        }
    }

    /// Every model at once, laid out in `cols` columns of `tile_w x tile_h`.
    /// Returns the composite as raw RGBA plus its size.
    ///
    /// `cells` is how many cells the grid must have, which is **not** always the
    /// number of models: the studio's grid has one more, for the rig, which it
    /// draws into itself on a canvas of its own. Anything past the models is
    /// left black — the first tile clears the whole attachment, so an unused
    /// cell is a cell nothing was drawn into rather than one that has to be
    /// cleared.
    ///
    /// **This parameter is the fix for a bug that was live and invisible.** The
    /// studio's page laid its click overlay out in `ceil(entries / cols)` rows
    /// over an image the server had laid out in `ceil(models / cols)` — two
    /// counts, in two languages, that have to produce the same number of rows.
    /// They agreed at eleven models and twelve entries by coincidence, and they
    /// agree at seventeen and eighteen by coincidence again; they stop agreeing
    /// the moment the model count is a multiple of the column count, and then
    /// every tile below the first row selects the wrong animation with nothing
    /// anywhere reporting it. That is `CORRECTIONS.md` §44 bug 3, in the app it
    /// was not found in. The caller now says how many cells it wants and the
    /// image is the authority — one count, not two that must match.
    #[allow(clippy::too_many_arguments)]
    pub fn grid(
        &mut self,
        rows: &[Vec<f32>],
        time: f32,
        tile_w: u32,
        tile_h: u32,
        cols: u32,
        cells: u32,
        skip: u32,
    ) -> (Vec<u8>, u32, u32) {
        let (texture, cw, ch) = self.grid_canvas(rows, time, tile_w, tile_h, cols, cells, skip);
        let pixels = read_rgba8(&self.gpu.device, &self.gpu.queue, &texture, cw, ch);
        (pixels, cw, ch)
    }

    /// [`Renderer::grid`] left on the device (G5): the tiles are drawn into the
    /// canvas and the canvas texture comes back with its size, nothing read
    /// back. The handle is the renderer's own canvas, so a caller copies it out
    /// before the next draw (the copy is queued in order on the same queue).
    #[allow(clippy::too_many_arguments)]
    pub fn grid_canvas(
        &mut self,
        rows: &[Vec<f32>],
        time: f32,
        tile_w: u32,
        tile_h: u32,
        cols: u32,
        cells: u32,
        skip: u32,
    ) -> (wgpu::Texture, u32, u32) {
        // A retired model's row is empty (a tombstone), so it has no trailing
        // effects block to read; skip it rather than underflow `read_post`.
        self.release_trails_if_off(
            rows.iter().any(|r| r.len() >= metal_schema::EFFECTS_WIDTH && read_post(r).ghost > 0.0),
        );
        // Every row here draws in its own legacy order (see the comment on
        // `EstelaParams::default` below), which can never resolve to a
        // `fx.estela` step either — so this is always a release, never a
        // keep-alive.
        self.release_estela_rings_if_off(false);
        // Rows draw in their legacy order here, which includes `fx.brush`
        // whenever a row's own `cant` is up.
        self.release_brushes_if_off(rows.iter().any(|r| {
            r.len() >= metal_schema::EFFECTS_WIDTH
                && metal_schema::advanced_chain_kind("brush").is_some_and(|k| read_post(r).amount(k as usize) > 0.0)
        }));
        let n = self.models.len() as u32;
        // Never fewer cells than there are models plus whatever is being left
        // for something else: a caller asking for less would get an image that
        // silently drops the last of them.
        let cells = cells.max(n + skip).max(1);
        let cols = cols.clamp(1, cells);
        let grid_rows = cells.div_ceil(cols);
        let (cw, ch) = (cols * tile_w, grid_rows * tile_h);
        self.ensure_canvas(cw, ch);

        let canvas_view = self.canvas.as_ref().expect("ensured above").1.clone();
        for i in 0..n as usize {
            // A tombstone keeps its grid cell (so the page's positional layout
            // stays aligned with `/select`) but draws nothing: the cell stays
            // whatever the first tile's clear left, black.
            if self.models[i].retired() {
                continue;
            }
            let at = i as u32 + skip;
            let x = (at % cols) * tile_w;
            let y = (at / cols) * tile_h;
            // The first tile clears; every later one loads, so the cells that
            // no model covers stay black instead of wiping their neighbours.
            // A review grid, not the live composition path — no layer here
            // has a `fondo_alpha` to honour, and no graph feeds it a chain
            // either — every row draws in its own legacy order, which can
            // never include a Wake chain step (see
            // `resolved_chain`'s own doc comment on why an empty chain never
            // resolves to one), so `EstelaParams::default`
            // here is inert, not merely a stand-in.
            self.draw_tile(
                i,
                &rows[i],
                time,
                (x, y, tile_w, tile_h),
                i == 0,
                false,
                &[],
                &EstelaParams::default(),
                &canvas_view,
            );
        }

        let texture = self.canvas.as_ref().expect("ensured above").0.clone();
        (texture, cw, ch)
    }

    /// One model, large, compositing the ten common effects in an explicit
    /// order instead of the legacy one.
    ///
    /// `floats` still carries every effect's *amount*, the same as `solo`;
    /// only the *order* can differ, because a flat row has nowhere to keep
    /// one — see `read_post`'s doc comment. `chain` is the resolved order a
    /// node graph asked for, most often `metal_schema::Post::resolved_chain`'s
    /// own output for whichever `Post` the graph attached to this row. An
    /// empty slice is not special-cased here: it falls straight through to
    /// `draw_tile`, which derives the legacy order from `floats` itself, so
    /// this and `solo` share the one code path all the way down.
    ///
    /// `wake` is the look a `fx.estela` step in
    /// `chain` grades with — see [`EstelaParams`]'s own doc
    /// comment for why this is a parameter at all rather than something
    /// `draw_tile` could work out from `floats`. Added alongside `solo`'s
    /// existing signature rather than folded into it, the same additive
    /// discipline [`Renderer::solo`] itself keeps — see its own doc comment.
    #[allow(clippy::too_many_arguments)]
    pub fn solo_chain(
        &mut self,
        index: usize,
        floats: &[f32],
        time: f32,
        w: u32,
        h: u32,
        fondo_alpha: bool,
        chain: &[EffectStep],
        wake: &EstelaParams,
    ) -> (Vec<u8>, u32, u32) {
        let padded = full_row(floats);
        let floats: &[f32] = &padded;
        let post = read_post(floats);
        self.release_trails_if_off(post.ghost > 0.0);
        // The same "no graph, no history to keep" resolution `draw_tile`
        // itself repeats just below — cheap (at most `MAX_CHAIN` steps) and
        // kept separate so the release can happen before this row's own
        // ring is (possibly) allocated, not after.
        let legacy;
        let steps: &[EffectStep] = if chain.is_empty() {
            legacy = post.resolved_chain();
            &legacy
        } else {
            chain
        };
        self.release_estela_rings_if_off(
            steps.iter().any(|s| s.effect == CHAIN_KIND_ESTELA && s.amount > 0.0),
        );
        self.release_brushes_if_off(brush_amount(steps).is_some());
        self.ensure_canvas(w, h);
        let canvas_view = self.canvas.as_ref().expect("ensured above").1.clone();
        self.draw_tile(index, floats, time, (0, 0, w, h), true, fondo_alpha, chain, wake, &canvas_view);
        let texture = &self.canvas.as_ref().expect("ensured above").0;
        let pixels = read_rgba8(&self.gpu.device, &self.gpu.queue, texture, w, h);
        (pixels, w, h)
    }

    /// [`Renderer::solo_chain`] without the readback: the model (post chain,
    /// ghost, estela and all) is drawn straight into `target` (from
    /// [`Renderer::new_target`], `w x h`) and stays a texture on the
    /// renderer's device. G1: the model-to-compose handoff. `target` must be an
    /// `Rgba8UnormSrgb` render attachment (`metal-perform`'s `GpuCx::alloc`).
    #[allow(clippy::too_many_arguments)]
    pub fn solo_chain_into(
        &mut self,
        index: usize,
        floats: &[f32],
        time: f32,
        fondo_alpha: bool,
        chain: &[EffectStep],
        wake: &EstelaParams,
        target: &wgpu::Texture,
    ) {
        let (w, h) = (target.width(), target.height());
        let padded = full_row(floats);
        let floats: &[f32] = &padded;
        let post = read_post(floats);
        self.release_trails_if_off(post.ghost > 0.0);
        let legacy;
        let steps: &[EffectStep] = if chain.is_empty() {
            legacy = post.resolved_chain();
            &legacy
        } else {
            chain
        };
        self.release_estela_rings_if_off(
            steps.iter().any(|s| s.effect == CHAIN_KIND_ESTELA && s.amount > 0.0),
        );
        self.release_brushes_if_off(brush_amount(steps).is_some());
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        self.draw_tile(index, floats, time, (0, 0, w, h), true, fondo_alpha, chain, wake, &view);
    }

    /// Points the `forge` model at the WGSL its next frame draws (`key` is
    /// the shader's id). `Err` carries naga's diagnosis; the model then
    /// draws its slate until a shader that checks is handed over.
    pub fn forge_select(&mut self, key: &str, wgsl: &str) -> Result<(), String> {
        let model = self
            .models
            .iter_mut()
            .find(|m| m.id() == "forge")
            .ok_or("this build has no forge model")?;
        model.set_source(key, wgsl)
    }

    /// `fx.forge`: runs a custom effect shader over a finished RGBA frame.
    #[allow(clippy::too_many_arguments)]
    pub fn forge_fx(
        &mut self,
        key: &str,
        wgsl: &str,
        input: &[u8],
        w: u32,
        h: u32,
        time: f32,
        ingest: &[f32],
    ) -> Result<Vec<u8>, String> {
        self.forge_fx
            .apply(&self.gpu.device, &self.gpu.queue, FORMAT, key, wgsl, input, w, h, time, ingest)
    }

    /// [`Renderer::forge_fx`] over textures, recorded into `enc` (G2): `input`
    /// and `output` are views in `format`. No readback, no submit.
    #[allow(clippy::too_many_arguments)]
    pub fn forge_fx_encode(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        format: wgpu::TextureFormat,
        key: &str,
        wgsl: &str,
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
        aspect: f32,
        time: f32,
        ingest: &[f32],
    ) -> Result<(), String> {
        self.forge_fx.encode(&self.gpu.device, enc, format, key, wgsl, input, output, aspect, time, ingest)
    }

    /// `fx.warp`: displaces a finished RGBA frame by `warp`'s mesh field,
    /// moshed against `key`'s previous output (`crate::shared::warp`).
    pub fn warp_fx(
        &mut self,
        key: &str,
        warp: &metal_schema::warp::Warp,
        input: &[u8],
        w: u32,
        h: u32,
    ) -> Result<Vec<u8>, String> {
        self.warp_fx.apply(&self.gpu.device, &self.gpu.queue, FORMAT, key, warp, input, w, h)
    }

    /// `fx.warp` over textures, recorded into `enc` (G2). Views are in `format`
    /// (use the frame's sRGB view: the CPU path uploads as `FORMAT`).
    #[allow(clippy::too_many_arguments)]
    pub fn warp_fx_encode(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        format: wgpu::TextureFormat,
        warp: &metal_schema::warp::Warp,
        cur: &wgpu::TextureView,
        prev: Option<&wgpu::TextureView>,
        out: &wgpu::TextureView,
        w: u32,
        h: u32,
    ) {
        self.warp_fx.encode(&self.gpu.device, enc, format, warp, cur, prev, out, w, h)
    }

    /// One model, large. Always the legacy per-row effect order; see
    /// [`Renderer::solo_chain`] for the graph-ordered version this is a thin
    /// wrapper over — kept as its own name, with its own signature
    /// unchanged, so the callers with no graph to ask (there are about ten,
    /// all in `metal-dash`) do not have to thread a `&[]` and a Wake
    /// look nothing in their call ever uses through every call site.
    pub fn solo(
        &mut self,
        index: usize,
        floats: &[f32],
        time: f32,
        w: u32,
        h: u32,
        fondo_alpha: bool,
    ) -> (Vec<u8>, u32, u32) {
        self.solo_chain(
            index,
            floats,
            time,
            w,
            h,
            fondo_alpha,
            &[],
            &EstelaParams::default(),
        )
    }
}

/// RGBA straight from the texture to a JPEG. Quality 82 because the grid is
/// mostly smooth gradients, where the difference from 95 is invisible and the
/// difference in bytes is not.
pub fn encode_jpeg(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut out, 82);
    encoder
        .encode(rgba, width as u16, height as u16, jpeg_encoder::ColorType::Rgba)
        .expect("jpeg encode failed");
    out
}

/// A row long enough for [`read_post`]: a retired model's (tombstone,
/// registry rule 1) schema has no floats, so its row arrives empty; padded
/// with zeros (no effects) it draws the tombstone's nothing instead of
/// underflowing `read_post`.
fn full_row(floats: &[f32]) -> std::borrow::Cow<'_, [f32]> {
    if floats.len() >= metal_schema::EFFECTS_WIDTH {
        std::borrow::Cow::Borrowed(floats)
    } else {
        let mut v = floats.to_vec();
        v.resize(metal_schema::EFFECTS_WIDTH, 0.0);
        std::borrow::Cow::Owned(v)
    }
}

// ---------------------------------------------------------------------------
// export.video: the stepped, resumable exporter (plan §9.1 step E1)
// ---------------------------------------------------------------------------

/// Frames per write group. Fixed, not `#[derive]`-tunable beyond the settings:
/// the determinism guarantee holds across runs of the *same* settings, and a
/// group is only a batching of [`Encoder::push`] calls, so the group size
/// changes how work is overlapped, never which frames land in the file.
pub const EXPORT_BATCH: u64 = 16;

/// Everything an export needs beyond its [`FrameSource`].
#[derive(Clone, Debug)]
pub struct ExportSettings {
    pub path: std::path::PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub codec: String,
    pub bitrate_kbps: Option<u32>,
    /// An optional audio track to mux beside the video (plan §9.1 E3): a path
    /// the exporter passes straight to the encoder as a second input. `None`
    /// is the video-only export and keeps the bytes E1 produced.
    pub audio: Option<std::path::PathBuf>,
    /// Frames rendered and pushed per [`Exporter::step`] (the overlap unit).
    pub batch: u64,
}

impl ExportSettings {
    pub fn new(path: impl Into<std::path::PathBuf>, width: u32, height: u32, fps: f64) -> ExportSettings {
        ExportSettings {
            path: path.into(),
            width,
            height,
            fps,
            codec: "h264".to_string(),
            bitrate_kbps: None,
            audio: None,
            batch: EXPORT_BATCH,
        }
    }
}

/// What one [`Exporter::step`] did. A background job polls the exporter
/// instead of blocking a thread
/// on a whole clip, so it can be cancelled and reported on mid-export.
#[derive(Debug)]
pub enum Step {
    /// `Progress(frames_done)` — a group was encoded; call again.
    Progress(u64),
    /// Media is not ready: nothing was encoded, call again later.
    Pending,
    /// The file is complete.
    Done(Report),
}

/// What a finished export came to.
#[derive(Debug, Clone)]
pub struct Report {
    pub path: std::path::PathBuf,
    pub frames: u64,
    pub seconds: f64,
    pub bytes: u64,
}

#[derive(Debug)]
pub enum ExportError {
    Encode(metal_ingest::video::VideoError),
    /// The caller's cancel flag was set; the partial file is left for the job
    /// to remove.
    Cancelled,
    Io(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Encode(e) => write!(f, "{e}"),
            ExportError::Cancelled => write!(f, "export cancelled"),
            ExportError::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ExportError {}

/// An offline export advanced one group at a time. Owns its own headless
/// renderer (so it can run on a background thread with no window and no
/// painter), the live [`Encoder`] and the [`FrameSource`].
///
/// **Overlap.** A group of frames is rendered, pushed to the encoder's pipe,
/// and then the *next* group is rendered while ffmpeg encodes the one just
/// pushed — ffmpeg runs in its own process, so the GPU work genuinely
/// overlaps it instead of interleaving on one timeline. The rendered-but-not-
/// yet-pushed group lives in `ahead` (one group, fixed size, so memory is
/// bounded by [`ExportSettings::batch`], not by the clip).
///
/// **Determinism.** Every frame goes through the same `solo_chain` draw at the
/// same time, and the encoder's flags are pinned (see `metal-ingest::export`),
/// so exporting the same [`FrameSource`] twice is byte-identical. The write
/// grouping is fixed at construction; nothing about the schedule depends on
/// wall-clock timing.
pub struct Exporter {
    renderer: Renderer,
    encoder: Option<Box<dyn Encoder>>,
    source: Box<dyn FrameSource>,
    settings: ExportSettings,
    total: u64,
    next: u64,
    done: u64,
    ahead: Option<(u64, Vec<Vec<u8>>)>,
    started: Instant,
}

impl Exporter {
    pub fn new(source: Box<dyn FrameSource>, settings: ExportSettings) -> Result<Exporter, ExportError> {
        let settings = ExportSettings {
            batch: settings.batch.max(1),
            ..settings
        };
        let total = source.total();
        let encoder = ingest_export::open(&EncodeSettings {
            path: settings.path.clone(),
            width: settings.width,
            height: settings.height,
            fps: settings.fps,
            codec: settings.codec.clone(),
            bitrate_kbps: settings.bitrate_kbps,
            audio: settings.audio.clone(),
        })
        .map_err(ExportError::Encode)?;
        Ok(Exporter {
            renderer: Renderer::new(),
            encoder: Some(encoder),
            source,
            settings,
            total,
            next: 0,
            done: 0,
            ahead: None,
            started: Instant::now(),
        })
    }

    /// Frames in the range being exported.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Frames pushed to the encoder so far — what a `GET /export` reports.
    pub fn done(&self) -> u64 {
        self.done
    }

    /// Renders frames `start..end` (inclusive start, exclusive end) into RGBA
    /// buffers. `None` when the source is not ready for one of them, which is
    /// [`Step::Pending`] for the caller.
    fn range(&mut self, start: u64, end: u64) -> Result<Option<Vec<Vec<u8>>>, ExportError> {
        let mut frames = Vec::with_capacity((end - start) as usize);
        for k in start..end {
            let Some(job) = self.source.job(k) else {
                return Ok(None);
            };
            let (pixels, _, _) = self.renderer.solo_chain(
                job.index,
                &job.floats,
                job.time,
                self.settings.width,
                self.settings.height,
                job.fondo_alpha,
                &job.chain,
                &EstelaParams::default(),
            );
            frames.push(pixels);
        }
        Ok(Some(frames))
    }

    /// Renders, encodes and muxes the next group, or finishes the file.
    ///
    /// `cancel` is the job's flag; set between groups, it stops the export
    /// cleanly with [`ExportError::Cancelled`] rather than mid-frame.
    pub fn step(&mut self, cancel: &AtomicBool) -> Result<Step, ExportError> {
        if cancel.load(Ordering::Relaxed) {
            self.ahead = None;
            return Err(ExportError::Cancelled);
        }
        if self.next >= self.total {
            return self.finish();
        }
        let (start, frames) = match self.ahead.take() {
            Some((s, f)) if s == self.next => (s, f),
            _ => {
                let end = (self.next + self.settings.batch).min(self.total);
                match self.range(self.next, end)? {
                    Some(f) => (self.next, f),
                    None => return Ok(Step::Pending),
                }
            }
        };
        if frames.is_empty() {
            return self.finish();
        }
        let end = start + frames.len() as u64;
        {
            let encoder = self
                .encoder
                .as_mut()
                .ok_or_else(|| ExportError::Io("the encoder was already finished".into()))?;
            for frame in &frames {
                encoder.push(frame).map_err(ExportError::Encode)?;
            }
        }
        self.next = end;
        self.done = end;
        // Overlap: the next group renders while ffmpeg encodes the one just
        // pushed. A source not ready for it just leaves `ahead` empty and the
        // next step renders it then.
        if self.next < self.total {
            let look_end = (self.next + self.settings.batch).min(self.total);
            if let Some(f) = self.range(self.next, look_end)? {
                self.ahead = Some((self.next, f));
            }
        }
        Ok(Step::Progress(self.done))
    }

    fn finish(&mut self) -> Result<Step, ExportError> {
        self.ahead = None;
        let encoder = self
            .encoder
            .take()
            .ok_or_else(|| ExportError::Io("the encoder was already finished".into()))?;
        let bytes = encoder.finish().map_err(ExportError::Encode)?;
        self.done = self.total;
        Ok(Step::Done(Report {
            path: self.settings.path.clone(),
            frames: self.total,
            seconds: self.started.elapsed().as_secs_f64(),
            bytes,
        }))
    }
}

#[cfg(test)]
mod export_tests {
    use super::*;
    use metal_ingest::export::FrameJob;

    /// A deterministic frame source: model 0, a zero row (no effects), one
    /// frame per step. Not random and not a still either — every frame draws
    /// at its own time, so a byte-for-byte comparison of two exports means
    /// something.
    struct Scripted {
        n: u64,
        fps: f64,
    }

    impl FrameSource for Scripted {
        fn total(&self) -> u64 {
            self.n
        }
        fn job(&mut self, k: u64) -> Option<FrameJob> {
            Some(FrameJob {
                index: 0,
                floats: vec![0.0; metal_schema::EFFECTS_WIDTH],
                time: k as f32 / self.fps as f32,
                fondo_alpha: false,
                chain: Vec::new(),
            })
        }
    }

    fn settings(dir: &std::path::Path, name: &str) -> ExportSettings {
        let mut s = ExportSettings::new(dir.join(name), 320, 180, 8.0);
        s.batch = 2;
        s
    }

    /// Runs an export to completion, returning the report and every step it
    /// reported. Skips (returns `None`) when ffmpeg is unavailable, the same
    /// pattern `metal-ingest::video`'s tests use.
    fn run(dir: &std::path::Path, name: &str, n: u64) -> Option<(Report, Vec<u64>)> {
        let mut exporter = Exporter::new(
            Box::new(Scripted { n, fps: 8.0 }),
            settings(dir, name),
        )
        .ok()?;
        let cancel = AtomicBool::new(false);
        let mut progress = Vec::new();
        for _ in 0..(n * 2 + 8) {
            match exporter.step(&cancel) {
                Ok(Step::Progress(done)) => progress.push(done),
                Ok(Step::Pending) => continue,
                Ok(Step::Done(report)) => return Some((report, progress)),
                Err(ExportError::Encode(metal_ingest::video::VideoError::Spawn(_))) => {
                    eprintln!("skipping: ffmpeg unavailable");
                    return None;
                }
                Err(e) => panic!("export failed: {e}"),
            }
        }
        panic!("the export never finished");
    }

    #[test]
    fn an_export_renders_a_range_to_a_real_file_and_reports_progress() {
        let dir = std::env::temp_dir().join(format!("fw-exp-job-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let Some((report, progress)) = run(&dir, "out.mp4", 5) else { return };
        assert_eq!(report.frames, 5);
        assert!(report.bytes > 0, "a finished export leaves bytes: {report:?}");
        assert!(report.path.exists(), "the file is where the report says");
        assert_eq!(progress.last().copied(), Some(5), "progress reached the total");
        assert!(progress.windows(2).all(|w| w[0] <= w[1]), "progress is monotonic: {progress:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_exports_of_the_same_frames_are_byte_identical() {
        let dir = std::env::temp_dir().join(format!("fw-exp-det-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let Some((ra, _)) = run(&dir, "a.mp4", 5) else { return };
        std::thread::sleep(std::time::Duration::from_millis(1100));
        // A second Exporter, a second Renderer, the same source.
        let Some((rb, _)) = run(&dir, "b.mp4", 5) else { return };

        let a = std::fs::read(&ra.path).unwrap();
        let b = std::fs::read(&rb.path).unwrap();
        assert_eq!(
            a, b,
            "two exports of the same frames differ ({} vs {} bytes)",
            a.len(),
            b.len()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_cancelled_export_stops_instead_of_finishing() {
        let dir = std::env::temp_dir().join(format!("fw-exp-cancel-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut exporter = match Exporter::new(
            Box::new(Scripted { n: 1000, fps: 8.0 }),
            settings(&dir, "cancel.mp4"),
        ) {
            Ok(e) => e,
            Err(ExportError::Encode(metal_ingest::video::VideoError::Spawn(_))) => {
                eprintln!("skipping: ffmpeg unavailable");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        let cancel = AtomicBool::new(true);
        let err = exporter.step(&cancel).unwrap_err();
        assert!(matches!(err, ExportError::Cancelled), "{err:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `FrameSource` with nothing to draw finishes on the first step with an
    /// empty file rather than hanging on a zero-frame loop.
    #[test]
    fn an_empty_range_finishes_immediately() {
        let dir = std::env::temp_dir().join(format!("fw-exp-empty-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut exporter = match Exporter::new(Box::new(Scripted { n: 0, fps: 8.0 }), settings(&dir, "empty.mp4")) {
            Ok(e) => e,
            Err(ExportError::Encode(metal_ingest::video::VideoError::Spawn(_))) => {
                eprintln!("skipping: ffmpeg unavailable");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        let cancel = AtomicBool::new(false);
        match exporter.step(&cancel) {
            Ok(Step::Done(report)) => assert_eq!(report.frames, 0),
            other => panic!("expected Done, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;

    #[test]
    fn scratch_size_is_the_overscan_at_full_scale() {
        assert_eq!(scratch_size(1920, 1080, 1.0), (2035, 1144));
    }

    #[test]
    fn scratch_size_shrinks_with_the_render_scale() {
        assert_eq!(scratch_size(1920, 1080, 0.6), (1221, 686));
        assert_eq!(scratch_size(0, 0, 0.1), (1, 1));
    }

    #[test]
    fn detail_is_one_at_1080p_and_above_and_floored_below() {
        assert_eq!(detail_factor(1920, 1080), 1.0);
        assert_eq!(detail_factor(3840, 2160), 1.0);
        assert_eq!(detail_factor(960, 540), 0.25);
        assert_eq!(detail_factor(320, 180), 0.25);
        assert!((detail_factor(1280, 720) - 4.0 / 9.0).abs() < 1e-5);
    }
}
