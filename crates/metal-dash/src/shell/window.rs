//! The platform layer under the shell: a `winit` window and one of two
//! paint backends — `wgpu` (`egui-wgpu`) or `egui_glow` (`glutin`) — that
//! turn [`Shell::ui`] into pixels on it. `O.12.b` decided this is done by
//! hand, no `eframe`, so the whole binary has one platform layer; the
//! `wgpu` half's pattern — and most of its frame — is
//! `metal-render/src/app.rs`, copied as a pattern and not as a file:
//! `resumed` creates the window and the surface, `window_event` feeds egui
//! and handles resize/close/redraw, `about_to_wait` asks for the next
//! frame, and `redraw` runs egui, tessellates, uploads and presents.
//!
//! **`S.U15`**: a second painter, `egui_glow`, behind [`Backend::Glow`] —
//! `metal_render::gpu::probe`'s reason to exist (`S.U3`) and the answer to
//! "`--no-default-features` would have no client once the HTML dashboard is
//! gone" (`plan-sonnet.md` § v0.3 table). One `Painter` enum inside this
//! file dispatches the two — `resumed`/`window_event`/`redraw` each match
//! on it once, not two copies of the event loop. `Backend` itself is local
//! to this file, not `metal_render::gpu::Backend`: a `--no-default-features
//! --features glow` build has no `metal-render` in its tree at all (that crate
//! carries `wgpu` unconditionally in its own `Cargo.toml`, with no feature
//! to shed it — see `Cargo.toml`'s comment on the `glow` feature), and this
//! file still needs a `Backend` to dispatch on in that build. [`resolve`]
//! is the only place the two meet, cfg-gated on which half is compiled in.
//!
//! What this file itself calls: nothing beyond building a [`Door`] into the
//! `Shell` it owns and handing it over — `S.U7b` is what made that possible
//! at all (`Shared: Send`, see that struct's doc comment in `main.rs`), and
//! it is `main_windowed`, not this file, that builds the `Arc<Mutex<Shared>>`
//! / `Arc<Author>` pair a `Door` is made from. This file is under the three
//! gates of `gates.rs` like the rest of `shell/`: every colour it needs
//! comes off the theme (`Theme::ground_wgpu`/`Theme::ground_glow` for the
//! clear), and the only thing it names off `crate::call` is `Door` itself —
//! gate 1's door surface, not a submodule.
//!
//! **No model under egui in this lote.** The pass below clears to the
//! theme's ground and paints egui; the `LoadOp::Load` pass `app.rs` draws
//! a model with is exactly what `O.12.b` kept the frame under our control
//! for, and it arrives when a panel has something to show. The `glow`
//! painter has the same gap: nothing draws to the framebuffer between
//! `clear` and egui's own meshes.

#[cfg(target_os = "android")]
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use web_time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::call::{Call, Door};

use super::classes::Theme;
use super::frames::set_software_gpu;
use super::Shell;

/// The window title.
pub const TITLE: &str = "MetalVJ";

/// The window's first size, in logical pixels — `super::DESKTOP_VIEWPORT`:
/// the desktop viewport `check-ui.mjs` section A measures first, the
/// `screen_rect` the restyle gate paints under, and the size the `S.U7`
/// kittest harness runs at, so what opens is what the tests see; the
/// compositor clamps it on a smaller screen. Not a spacing decision — it is
/// how big the *window* is, and the theme has no say in that.
const INITIAL_SIZE: (f64, f64) = super::DESKTOP_VIEWPORT;

/// The client's refresh pace: 60 Hz, one frame every 16.6 ms.
///
/// `ControlFlow::Poll` with `about_to_wait` asking for the next frame made
/// the client redraw as fast as the compositor would present — 165 Hz on a
/// venue panel — which is GPU and battery spent on frames nobody asked for.
/// The client and the previews share one cap (settings → performance, 60 by
/// default, up to 300): `frame_cap::gap`.


/// How often a settled UI still wakes: nothing animates, so egui asks for no
/// frame, and background state (worker results, sync) shows within this.
/// 10 Hz, against the 60 Hz the client used to repaint at while idle.
const IDLE_REFRESH: Duration = Duration::from_millis(100);

/// A software adapter, by its reported type or by the names the CPU
/// rasterisers go by. `name` is the adapter's own name.
#[cfg(feature = "gpu")]
pub fn is_software_adapter(name: &str, kind: wgpu::DeviceType) -> bool {
    if kind == wgpu::DeviceType::Cpu {
        return true;
    }
    let n = name.to_ascii_lowercase();
    ["swiftshader", "llvmpipe", "lavapipe"].iter().any(|k| n.contains(k))
}

/// The minimum time between two painted frames for the current GPU.
fn frame_gap() -> Duration {
    crate::frame_cap::gap()
}

/// When the next frame is wanted, as the last one left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wake {
    /// egui asked for a repaint now (input, an animation): paced by the gap.
    Now,
    /// egui asked for one by this instant (`request_repaint_after`).
    At(Instant),
    /// egui asked for nothing: the slow heartbeat, [`IDLE_REFRESH`].
    Idle,
}

/// Count of frames painted, for the periodic perf line on Android.
#[cfg(target_os = "android")]
static FRAMES_PAINTED: AtomicU32 = AtomicU32::new(0);

/// Which paint backend a window is opened with. Local to this file — see
/// the module doc comment for why it is not `metal_render::gpu::Backend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    #[cfg(feature = "gpu")]
    Wgpu,
    #[cfg(feature = "glow")]
    Glow,
}

/// Which route a preview picture takes to the screen, decided once at startup
/// from the real window surface (`the design notes` G, step 2). The three cases are the
/// plan's: (a) the window shares the painter's device, (b) the painter's
/// adapter cannot present so the window owns a surface-capable adapter and
/// previews cross as a raw RGBA readback, (c) there is no painter device — the
/// `Cpu` backend — so previews travel as JPEG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentPath {
    Shared,
    Readback,
    Jpeg,
}

impl PresentPath {
    /// The name `GET /perf` reports (see `crate::perf::set_preview_path`).
    pub fn as_str(self) -> &'static str {
        match self {
            PresentPath::Shared => "a_shared",
            PresentPath::Readback => "b_readback",
            PresentPath::Jpeg => "c_jpeg",
        }
    }
}

impl Backend {
    /// Parses `--backend`/`FW_BACKEND`'s value. `None` for anything that is
    /// not a name this build actually compiled in — a single-backend build
    /// rejects the other backend's name rather than silently ignoring it,
    /// so a typo (or asking for `glow` on a `--no-default-features` build
    /// that only has `gpu`) is a startup error in `main.rs`, not a quiet
    /// no-op.
    pub fn parse(s: &str) -> Option<Backend> {
        match s {
            #[cfg(feature = "gpu")]
            "wgpu" => Some(Backend::Wgpu),
            #[cfg(feature = "glow")]
            "glow" => Some(Backend::Glow),
            _ => None,
        }
    }
}

/// Which backend [`run`] should actually open with: `forced` (from
/// `--backend`/`FW_BACKEND`, `main.rs::resolve_forced_backend`) wins when
/// given; otherwise, on a build with both backends compiled in,
/// `metal_render::gpu::probe` decides — `Glow` when it finds no adapter,
/// `Wgpu` otherwise, exactly what `S.U3` built `probe` for. On a
/// single-backend build there is nothing to probe: that one backend is what
/// opens, unconditionally.
pub fn resolve(forced: Option<Backend>) -> Backend {
    if let Some(b) = forced {
        return b;
    }
    #[cfg(all(feature = "gpu", feature = "glow"))]
    {
        match metal_render::gpu::probe(wgpu::Backends::PRIMARY) {
            metal_render::gpu::Backend::Wgpu => Backend::Wgpu,
            metal_render::gpu::Backend::Glow => Backend::Glow,
        }
    }
    #[cfg(all(feature = "gpu", not(feature = "glow")))]
    {
        Backend::Wgpu
    }
    #[cfg(all(feature = "glow", not(feature = "gpu")))]
    {
        Backend::Glow
    }
}

/// Why [`run`] came back.
#[derive(Debug)]
pub enum Outcome {
    /// The operator closed the window.
    Closed,
    /// No window could be opened — no display, no adapter compatible with
    /// the surface, or a surface the adapter cannot configure — with the
    /// reason, so the caller can say it and keep serving headless.
    NoWindow(String),
}

/// Open the window and run its event loop on **this** thread until the
/// window closes or cannot be opened, painting through `backend`. winit
/// wants the main thread on every platform that matters (macOS and iOS
/// refuse any other), which is why the HTTP listener is the one that moved
/// to a background thread and not this.
///
/// `door` (`S.U7b`) is the real `call::Door` `main_windowed` built from the
/// same `Arc<Mutex<Shared>>`/`Arc<Author>` the listener thread serves from —
/// see that function and `Shared`'s own doc comment in `main.rs` for why
/// this could not exist before `S.U7b`. Handed to [`Shell::new`] as-is; no
/// panel calls it yet through anything backend-specific — the door is the
/// same object under either painter.
/// Android only: the `AndroidApp` `android_main` was handed. winit's event
/// loop cannot be built on Android without it, and the native activity hands
/// it over exactly once, before anything here runs.
#[cfg(target_os = "android")]
static ANDROID_APP: std::sync::OnceLock<winit::platform::android::activity::AndroidApp> =
    std::sync::OnceLock::new();

#[cfg(target_os = "android")]
pub fn set_android_app(app: winit::platform::android::activity::AndroidApp) {
    let _ = ANDROID_APP.set(app);
}

/// Android only: the NativeActivity's `jobject`, for JNI calls that need the
/// Activity itself (`shell/insets.rs`). Null before `android_main` ran.
#[cfg(target_os = "android")]
pub fn android_activity_ptr() -> *mut std::ffi::c_void {
    ANDROID_APP
        .get()
        .map_or(std::ptr::null_mut(), |app| app.activity_as_ptr())
}

/// The event loop for this platform: `EventLoop::new()` everywhere but
/// Android, where the activity has to be passed in.
fn event_loop() -> Result<EventLoop<()>, String> {
    #[cfg(target_os = "android")]
    {
        use winit::platform::android::EventLoopBuilderExtAndroid;
        let app = ANDROID_APP.get().cloned().ok_or("no android activity")?;
        return EventLoop::builder()
            .with_android_app(app)
            .build()
            .map_err(|e| e.to_string());
    }
    #[cfg(not(target_os = "android"))]
    EventLoop::new().map_err(|e| e.to_string())
}

pub fn run(theme: Theme, door: Door, backend: Backend) -> Outcome {
    let event_loop = match event_loop() {
        Ok(el) => el,
        Err(e) => return Outcome::NoWindow(format!("no event loop: {e}")),
    };
    // The preview pump's worker: a second handle to the same door (`Door` is
    // two `Arc`s), moved to a thread so the grid/solo renders never run on
    // this one. Built here and not in `Shell::new` because this is the only
    // caller that has a real door to give it — the kittest harness keeps the
    // inline path (`frames::Frame::tick`'s own `None` branch).
    let presenter = super::present::Presenter::default();
    let fetcher = super::frames::Fetcher::spawn_presenting(door.clone(), presenter.clone());
    // G3 + the design notes G step 3: a second handle to the door, so `open_wgpu` can
    // read the painter's device *after* the window and its surface exist and
    // decide the present case from the real surface, not before it.
    #[cfg(feature = "gpu")]
    let painter_door = door.clone();
    // The pairing POSTs get the same treatment: a `metal-sync` dial can wait 20 s
    // and must not hold a frame (`panels::pairing::PairWorker`).
    let pair_worker = super::panels::pairing::PairWorker::spawn(door.clone());
    // And the shell's reads go through the short-lived cache: the worker's
    // renders hold the studio's lock for tens of milliseconds, and a panel
    // reading fresh every frame would otherwise queue behind every one of
    // them. Writes clear what they can have changed, and an expired read is
    // answered stale while the "metal-dash reads" thread refreshes it — see
    // `reads.rs` for the full rule.
    let cached = super::reads::CachedDoor::with_background(Box::new(door.clone()), door.clone());
    // The lanes worker writes and reads through a second handle to the door;
    // its POSTs clear the cache the same way the cached door's own do.
    let lane_worker = super::lane_worker::LaneWorker::spawn(door.clone(), cached.write_hook());
    // The graph writes the panels fire and forget run on their own thread too
    // (`graph_writer`); the cached door routes `post_async` through it.
    let graph_writer = super::graph_writer::GraphWriter::spawn(door.clone(), cached.write_hook(), cached.graph_put());
    let cached = match &graph_writer {
        Some(writer) => cached.with_writes(writer.clone()),
        None => cached,
    };
    let door: Box<dyn Call> = Box::new(cached);
    let mut shell = Shell::new(theme, door);
    // The layout the operator last left (panel sizes, canvas view) comes back
    // from the main config file; a missing or unreadable one is nothing
    // remembered, never a failed start.
    let file = super::layout::config_file();
    let remembered = metal_config::Config::load(&file).map(|c| c.layout).unwrap_or_default();
    shell.remember_layout(file, remembered);
    shell.set_fetcher(fetcher);
    shell.set_pair_worker(pair_worker);
    if let Some(worker) = lane_worker {
        shell.set_lane_worker(worker);
    }
    if let Some(writer) = graph_writer {
        shell.set_graph_writer(writer);
    }
    let mut app = App {
        backend,
        rendering: None,
        presenter,
        #[cfg(feature = "gpu")]
        painter_door,
        shell,
        last_frame: None,
        wake: Wake::Now,
        failure: None,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        return Outcome::NoWindow(format!("event loop: {e}"));
    }
    match app.failure.take() {
        Some(why) => Outcome::NoWindow(why),
        None => Outcome::Closed,
    }
}

/// G3: registers the compose textures that landed since the last paint with
/// egui, on the device the painter shares, replacing the ones they succeed.
#[cfg(feature = "gpu")]
pub(crate) fn present_pending(
    presenter: &super::present::Presenter,
    renderer: &mut egui_wgpu::Renderer,
    device: &wgpu::Device,
    hist: Option<&mut metal_perform::gpu_hist::Histogrammer<String>>,
) {
    if let Some(hist) = hist {
        presenter.histograms_pending(hist);
    }
    presenter.register_pending(|frame, id| {
        // The frame's sRGB view: egui samples user textures as linear, the way
        // it does its own `Rgba8UnormSrgb` images.
        let view = frame.texture().create_view(&wgpu::TextureViewDescriptor::default());
        match id {
            Some(id) => {
                renderer.update_egui_texture_from_wgpu_texture(device, &view, wgpu::FilterMode::Linear, id);
                id
            }
            None => renderer.register_native_texture(device, &view, wgpu::FilterMode::Linear),
        }
    });
}

/// Case (b) of the design notes G step 2: the painter's adapter cannot present, so a
/// landed compose texture is read back raw (RGBA8) on the painter's device and
/// uploaded to the window's device with `queue.write_texture` — never a JPEG.
/// Registers the uploaded texture with egui exactly as [`present_pending`]
/// does the shared one.
#[cfg(feature = "gpu")]
pub(crate) fn present_pending_readback(
    presenter: &super::present::Presenter,
    renderer: &mut egui_wgpu::Renderer,
    window: &metal_render::gpu::Gpu,
    painter: &metal_render::gpu::Gpu,
    hist: Option<&mut metal_perform::gpu_hist::Histogrammer<String>>,
) {
    if let Some(hist) = hist {
        presenter.histograms_pending(hist);
    }
    let cx = metal_perform::gpu_compose::GpuCx::new(painter.device.clone(), painter.queue.clone());
    presenter.register_pending(|frame, id| {
        let (w, h) = (frame.width(), frame.height());
        let rgba = cx.readback(frame);
        let texture = window.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("metal-dash preview readback"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        window.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        match id {
            Some(id) => {
                renderer.update_egui_texture_from_wgpu_texture(&window.device, &view, wgpu::FilterMode::Linear, id);
                id
            }
            None => renderer.register_native_texture(&window.device, &view, wgpu::FilterMode::Linear),
        }
    });
}

/// The present mode the window asks for, uncapped first: Immediate, then
/// Mailbox, then FifoRelaxed, else AutoVsync (FIFO). `modes` is what the surface reports it supports.
#[cfg(feature = "gpu")]
pub fn pick_present_mode(modes: &[wgpu::PresentMode]) -> wgpu::PresentMode {
    [wgpu::PresentMode::Immediate, wgpu::PresentMode::Mailbox, wgpu::PresentMode::FifoRelaxed]
        .into_iter()
        .find(|m| modes.contains(m))
        .unwrap_or(wgpu::PresentMode::AutoVsync)
}

/// The `wgpu` half of [`Painter`]. Unchanged from `S.U6`/`S.U3` besides the
/// rename (it used to be all of `Rendering`).
#[cfg(feature = "gpu")]
struct WgpuPainter {
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    gpu: metal_render::gpu::Gpu,
    renderer: egui_wgpu::Renderer,
    /// G5: the grade histogram pass, on the device the pictures are on — the
    /// shared device in case (a), the painter's in case (b).
    hist: Option<metal_perform::gpu_hist::Histogrammer<String>>,
    /// Plan G step 2: the present case, decided once from the surface.
    path: PresentPath,
    /// Case (b) only: the painter's device, whose compose textures are read
    /// back raw and uploaded on this window's device — never a JPEG.
    preview_src: Option<metal_render::gpu::Gpu>,
    /// Set when a resize's `configure` failed (the GPU was still busy, e.g.
    /// right after a monitor move); the next frame retries it.
    needs_configure: bool,
}

#[cfg(feature = "gpu")]
impl WgpuPainter {
    /// Reconfigures the surface without letting a wgpu validation error
    /// panic the window. Moving the window to another monitor can make
    /// `configure` fail with `GpuWaitTimeout` ("Failed to wait for GPU to
    /// come idle") while frames are still in flight; the default handler
    /// turns that into a panic. Returns `false` on failure so the caller
    /// retries on the next frame.
    fn configure_surface(&self) -> bool {
        let scope = self.gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        self.surface.configure(&self.gpu.device, &self.surface_config);
        pollster::block_on(scope.pop()).is_none()
    }
}

/// The `egui_glow` half of [`Painter`]: the GL context/surface `glutin`
/// (via `glutin-winit`) creates against the same window, plus the
/// `egui_glow::Painter` that turns tessellated shapes into GL calls. The
/// shape here — display, context, surface kept alongside the painter — is
/// `egui_glow`'s own `pure_glow.rs` example (`GlutinWindowContext`), read
/// as a pattern the same way `app.rs` is for the `wgpu` half; nothing here
/// is copied from a file that ships.
#[cfg(feature = "glow")]
struct GlowPainter {
    gl_context: glutin::context::PossiblyCurrentContext,
    gl_surface: glutin::surface::Surface<glutin::surface::WindowSurface>,
    painter: egui_glow::Painter,
}

/// One of the two paint backends, alive for as long as the window is.
enum Painter {
    #[cfg(feature = "gpu")]
    Wgpu(WgpuPainter),
    #[cfg(feature = "glow")]
    Glow(GlowPainter),
}

/// Everything that exists only once the window does. Separate from
/// [`App`]'s `shell` for the same borrow reason `app.rs` splits
/// `Rendering` from `UiState`: `redraw` needs `&mut` of this at the same
/// time the `run_ui` closure needs `&mut Shell`.
struct Rendering {
    window: Arc<Window>,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    painter: Painter,
}

struct App {
    backend: Backend,
    /// The mailbox compose textures reach the window through.
    presenter: super::present::Presenter,
    /// A handle to the door, so the painter's device is read inside
    /// [`App::open_wgpu`] once the window and its surface exist (the design notes G
    /// step 3). `None` on a build with no `gpu` feature.
    #[cfg(feature = "gpu")]
    painter_door: Door,
    rendering: Option<Rendering>,
    shell: Shell,
    /// When the last frame was actually painted — the frame gap is measured
    /// from here.
    last_frame: Option<Instant>,
    /// What the last frame's egui output asked for next.
    wake: Wake,
    /// Set by `resumed` when the window could not be brought up, read by
    /// [`run`] after the loop exits.
    failure: Option<String>,
}

impl App {
    /// Bring the window, the chosen backend's context/surface and egui up.
    /// `Err` is the reason no window will open; the caller turns it into
    /// [`Outcome::NoWindow`] rather than a panic, because a venue box with
    /// no compositor still has a dashboard to serve.
    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<Rendering, String> {
        match self.backend {
            #[cfg(feature = "gpu")]
            Backend::Wgpu => self.open_wgpu(event_loop),
            #[cfg(feature = "glow")]
            Backend::Glow => self.open_glow(event_loop),
        }
    }

    #[cfg(feature = "gpu")]
    fn open_wgpu(&mut self, event_loop: &ActiveEventLoop) -> Result<Rendering, String> {
        let attrs = Window::default_attributes()
            .with_title(TITLE)
            .with_inner_size(winit::dpi::LogicalSize::new(INITIAL_SIZE.0, INITIAL_SIZE.1));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| format!("create_window: {e}"))?,
        );

        // G3 + the design notes G steps 2/3: the window and its surface exist first, so
        // the present case is decided from the real surface. The painter's
        // device (when it has one) is read *now*, after the window exists, so
        // case (a) is not guessed before a surface is there to check against.
        let painter = self.painter_door.shared_gpu();
        let instance = metal_render::gpu::shared_instance();
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("create_surface: {e}"))?;

        // (a) the painter's adapter can present: egui renders on the painter's
        // own device and a compose texture goes to the screen as it is.
        // Otherwise the window owns a surface-capable adapter — (b) alongside a
        // painter device (a hybrid laptop), so previews cross as a raw RGBA
        // readback, or (c) with none (the CPU backend), previews as JPEG.
        let shares = painter
            .as_ref()
            .is_some_and(|g| g.adapter.is_surface_supported(&surface));
        let (adapter, gpu, preview_src, path) = if shares {
            let g = painter.as_ref().expect("checked above");
            (g.adapter.clone(), g.share(), None, PresentPath::Shared)
        } else {
            // `Gpu::new_windowed` uses `pick_adapter` now: an explicitly ranked
            // adapter (discrete > integrated > virtual > software) that can
            // present, and `None` — never a panic — once ranking *and* the
            // LowPower/force_fallback ladder are exhausted.
            let (adapter, gpu) = pollster::block_on(metal_render::gpu::Gpu::new_windowed(
                instance, &surface,
            ))
            .ok_or_else(|| {
                "no wgpu adapter compatible with the window, even after ranking and LowPower/force_fallback retries"
                    .to_string()
            })?;
            let path = if painter.is_some() { PresentPath::Readback } else { PresentPath::Jpeg };
            (adapter, gpu, painter, path)
        };
        let info = adapter.get_info();
        let soft = is_software_adapter(&info.name, info.device_type);
        set_software_gpu(soft);
        if soft {
            eprintln!("  metal-dash: software GPU ({}): reduced preview", info.name);
        }
        // Plan G step 2: the case, and the adapters it names, on stderr and in
        // `GET /perf`.
        crate::perf::set_preview_path(path.as_str());
        let painter_name = preview_src.as_ref().map(|g| g.adapter.get_info().name);
        eprintln!(
            "  metal-dash: preview path {} — {}",
            path.as_str(),
            match path {
                PresentPath::Shared => format!("window and painter share {}", info.name),
                PresentPath::Readback => format!(
                    "painter {} cannot present; window on {} (raw RGBA readback)",
                    painter_name.as_deref().unwrap_or("?"),
                    info.name
                ),
                PresentPath::Jpeg => format!("no painter device; window on {} (JPEG)", info.name),
            }
        );

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| "the adapter cannot configure this surface".to_string())?;
        // Mailbox or FifoRelaxed when the surface offers them: present does
        // not block a frame behind vsync, and a late frame is not held for a
        // whole refresh. AutoVsync (FIFO) is the fallback. Two frames in
        // flight is wgpu's own default, set explicitly.
        let modes = surface.get_capabilities(&adapter).present_modes;
        config.present_mode = pick_present_mode(&modes);
        config.desired_maximum_frame_latency = 2;
        surface.configure(&gpu.device, &config);
        {
            let info = adapter.get_info();
            crate::perf::set_present_mode(format!("{:?}", config.present_mode));
            eprintln!(
                "  metal-dash: present mode {:?} (supported {:?}); adapter {} on {:?}",
                config.present_mode,
                modes,
                info.name,
                info.backend,
            );
        }

        let egui_ctx = egui::Context::default();
        crate::shell::components::install_icon_font(&egui_ctx);
        self.shell.theme().apply(&egui_ctx);
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            config.format,
            egui_wgpu::RendererOptions::default(),
        );

        // The histogram runs on whatever device the pictures are on: the
        // shared device in case (a), the painter's in case (b).
        let hist = match path {
            PresentPath::Shared => {
                metal_perform::gpu_hist::Histogrammer::new(&gpu.device, &gpu.queue, gpu.compute)
            }
            PresentPath::Readback => {
                let src = preview_src.as_ref().expect("readback path has the painter's device");
                metal_perform::gpu_hist::Histogrammer::new(&src.device, &src.queue, src.compute)
            }
            PresentPath::Jpeg => None,
        };
        if path != PresentPath::Jpeg {
            self.presenter.enable(egui_ctx.clone());
            if hist.is_some() {
                self.presenter.enable_histograms();
            }
        }
        Ok(Rendering {
            window,
            egui_ctx,
            egui_state,
            painter: Painter::Wgpu(WgpuPainter {
                surface,
                surface_config: config,
                gpu,
                renderer,
                hist,
                path,
                preview_src,
                needs_configure: false,
            }),
        })
    }

    /// Builds the window and the GL context together via `glutin-winit`'s
    /// `DisplayBuilder`: unlike `wgpu`, a GL context needs a pixel format
    /// chosen *before* (or at) window creation, not after, so this cannot
    /// reuse `open_wgpu`'s "create window, then attach a surface" order.
    #[cfg(feature = "glow")]
    fn open_glow(&mut self, event_loop: &ActiveEventLoop) -> Result<Rendering, String> {
        use glutin::context::NotCurrentGlContext as _;
        use glutin::display::{GetGlDisplay as _, GlDisplay as _};
        
        use raw_window_handle::HasWindowHandle as _;

        let attrs = Window::default_attributes()
            .with_title(TITLE)
            .with_inner_size(winit::dpi::LogicalSize::new(INITIAL_SIZE.0, INITIAL_SIZE.1));
        let template = glutin::config::ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(None)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(false);

        let (window, gl_config) = glutin_winit::DisplayBuilder::new()
            .with_preference(glutin_winit::ApiPreference::FallbackEgl)
            .with_window_attributes(Some(attrs.clone()))
            .build(event_loop, template, |mut configs| {
                configs
                    .next()
                    .expect("glutin returned zero matching GL configs")
            })
            .map_err(|e| format!("glutin display/config: {e}"))?;
        let gl_display = gl_config.display();

        let raw_window_handle = window
            .as_ref()
            .and_then(|w| w.window_handle().ok())
            .map(|h| h.as_raw());
        let context_attributes =
            glutin::context::ContextAttributesBuilder::new().build(raw_window_handle);
        let fallback_attributes = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::Gles(None))
            .build(raw_window_handle);
        let not_current = unsafe { gl_display.create_context(&gl_config, &context_attributes) }
            .or_else(|_| unsafe { gl_display.create_context(&gl_config, &fallback_attributes) })
            .map_err(|e| {
                format!("glutin create_context (core and GLES fallback both failed): {e}")
            })?;

        let window = match window {
            Some(w) => w,
            None => glutin_winit::finalize_window(event_loop, attrs, &gl_config)
                .map_err(|e| format!("glutin finalize_window: {e}"))?,
        };
        let window = Arc::new(window);

        let (width, height): (u32, u32) = window.inner_size().into();
        let width = std::num::NonZeroU32::new(width).unwrap_or(std::num::NonZeroU32::MIN);
        let height = std::num::NonZeroU32::new(height).unwrap_or(std::num::NonZeroU32::MIN);
        let window_handle = window
            .window_handle()
            .map_err(|e| format!("window_handle: {e}"))?
            .as_raw();
        let surface_attrs =
            glutin::surface::SurfaceAttributesBuilder::<glutin::surface::WindowSurface>::new()
                .build(window_handle, width, height);
        let gl_surface = unsafe { gl_display.create_window_surface(&gl_config, &surface_attrs) }
            .map_err(|e| format!("glutin create_window_surface: {e}"))?;
        let gl_context = not_current
            .make_current(&gl_surface)
            .map_err(|e| format!("glutin make_current: {e}"))?;

        let gl = unsafe {
            egui_glow::glow::Context::from_loader_function(|s| {
                let s = std::ffi::CString::new(s).expect("proc name is not a valid C string");
                gl_display.get_proc_address(&s)
            })
        };
        let gl = Arc::new(gl);
        let egui_painter = egui_glow::Painter::new(gl, "", None, false)
            .map_err(|e| format!("egui_glow::Painter::new: {e}"))?;

        let egui_ctx = egui::Context::default();
        crate::shell::components::install_icon_font(&egui_ctx);
        self.shell.theme().apply(&egui_ctx);
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            None,
        );

        Ok(Rendering {
            window,
            egui_ctx,
            egui_state,
            painter: Painter::Glow(GlowPainter {
                gl_context,
                gl_surface,
                painter: egui_painter,
            }),
        })
    }

    fn redraw(&mut self) {
        let App {
            rendering,
            shell,
            last_frame,
            wake,
            ..
        } = self;
        let Some(r) = rendering else { return };

        // The performance monitor's frame (`crate::perf`): the shell's pass,
        // then tessellate + paint, each timed.
        crate::perf::frame_begin();
        let ui_started = Instant::now();
        #[cfg(feature = "gpu")]
        if let Painter::Wgpu(p) = &mut r.painter {
            match p.path {
                PresentPath::Shared => crate::perf::span("frame.present_pending", || {
                    present_pending(&self.presenter, &mut p.renderer, &p.gpu.device, p.hist.as_mut())
                }),
                PresentPath::Readback => {
                    let src = p.preview_src.as_ref().expect("readback path has the painter's device");
                    present_pending_readback(&self.presenter, &mut p.renderer, &p.gpu, src, p.hist.as_mut());
                }
                PresentPath::Jpeg => {}
            }
            // The eyedropper's pixel: off the compose texture's own device —
            // the painter's on the readback path, the shared one otherwise.
            let cx = match (p.path, p.preview_src.as_ref()) {
                (PresentPath::Readback, Some(src)) => {
                    metal_perform::gpu_compose::GpuCx::new(src.device.clone(), src.queue.clone())
                }
                _ => metal_perform::gpu_compose::GpuCx::new(p.gpu.device.clone(), p.gpu.queue.clone()),
            };
            if let Some(rgb) = self.presenter.picks_pending(&cx) {
                super::preview::deliver_picked(&r.egui_ctx, rgb);
            }
        }
        let raw_input = r.egui_state.take_egui_input(&r.window);
        // The size egui lays this frame out at. It can differ from the live
        // `inner_size()` read at paint time when the window resizes in
        // between, and the scissor rects egui emits follow this one.
        let laid_out = raw_input.screen_rect;
        let mut full_output = crate::perf::span("frame.run_ui", || {
            r.egui_ctx.clone().run_ui(raw_input, |ui| shell.ui(ui))
        });
        let ui_took = ui_started.elapsed();
        // egui's own answer to "when next?": zero for an animation or input
        // still in flight, `MAX` for nothing, a delay for a timed repaint.
        let delay = full_output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(Duration::ZERO, |v| v.repaint_delay);
        if super::frames::tracing() {
            let focused = r.egui_ctx.memory(|m| m.focused());
            eprintln!("[repaint] frame: egui asks {delay:?}, focused {focused:?}");
        }
        *wake = if delay.is_zero() {
            Wake::Now
        } else if delay == Duration::MAX {
            Wake::Idle
        } else {
            Wake::At(Instant::now() + delay)
        };
        let paint_started = Instant::now();
        r.egui_state
            .handle_platform_output(&r.window, full_output.platform_output);

        // A software GPU's idle redraw: egui asked for nothing, so the picture
        // already on the surface is this one. Skip the tessellation and the
        // present (hundreds of milliseconds each there), leave the textures
        // delta for the next real frame, and say the frame was not painted.
        if delay == Duration::MAX && last_frame.is_some() && super::frames::software_gpu() {
            crate::perf::frame_end(ui_took, Duration::ZERO);
            return;
        }
        *last_frame = Some(Instant::now());

        let pixels_per_point = full_output.pixels_per_point;
        let paint_jobs = crate::perf::span("frame.tessellate", || {
            r.egui_ctx.tessellate(full_output.shapes, pixels_per_point)
        });
        let live = r.window.inner_size();
        let frame = laid_out.map_or(live, |rect| {
            let px = rect.size() * pixels_per_point;
            winit::dpi::PhysicalSize::new(px.x.round().max(1.0) as u32, px.y.round().max(1.0) as u32)
        });

        match &mut r.painter {
            #[cfg(feature = "gpu")]
            Painter::Wgpu(p) => Self::redraw_wgpu(
                p,
                shell,
                &paint_jobs,
                &mut full_output.textures_delta,
                live,
                frame,
                pixels_per_point,
            ),
            #[cfg(feature = "glow")]
            Painter::Glow(p) => Self::redraw_glow(
                p,
                &r.window,
                shell,
                &paint_jobs,
                &mut full_output.textures_delta,
                frame,
                pixels_per_point,
            ),
        }

        // `TexturesDelta::drop` asserts it is empty — walking `.set`/`.free`
        // in each backend's redraw is not "handled" in its own bookkeeping;
        // `clear()` is.
        full_output.textures_delta.clear();
        crate::perf::frame_end(ui_took, paint_started.elapsed());
        #[cfg(target_os = "android")]
        if FRAMES_PAINTED.fetch_add(1, Ordering::Relaxed) % 120 == 119 {
            let s = crate::perf::snapshot();
            eprintln!(
                "perf: fps {:.1} frame p50 {:.1} p95 {:.1} max {:.1} ui {:.1} paint {:.1} slow {} (software {})",
                s["fps"].as_f64().unwrap_or(0.0),
                s["frame_ms"]["p50"].as_f64().unwrap_or(0.0),
                s["frame_ms"]["p95"].as_f64().unwrap_or(0.0),
                s["frame_ms"]["max"].as_f64().unwrap_or(0.0),
                s["ui_ms"].as_f64().unwrap_or(0.0),
                s["paint_ms"].as_f64().unwrap_or(0.0),
                s["slow_frames"],
                super::frames::software_gpu(),
            );
        }
    }

    #[cfg(feature = "gpu")]
    fn redraw_wgpu(
        p: &mut WgpuPainter,
        shell: &Shell,
        paint_jobs: &[egui::ClippedPrimitive],
        textures_delta: &mut egui::TexturesDelta,
        live: winit::dpi::PhysicalSize<u32>,
        size: winit::dpi::PhysicalSize<u32>,
        pixels_per_point: f32,
    ) {
        let surface_started = Instant::now();
        if p.needs_configure {
            p.needs_configure = !p.configure_surface();
        }
        // Texture uploads happen even when the frame is skipped below: the
        // delta is consumed by the caller, so a dropped upload would leave
        // later frames drawing textures that were never created.
        let textures_started = Instant::now();
        for (id, deltas) in &textures_delta.set {
            for delta in deltas {
                p.renderer
                    .update_texture(&p.gpu.device, &p.gpu.queue, *id, delta);
            }
        }
        let surface_texture = match p.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                for id in &textures_delta.free {
                    p.renderer.free_texture(id);
                }
                return;
            }
        };
        // The surface can still be at its old size right after a resize or a
        // monitor move; drawing at the size egui laid out then trips a scissor
        // validation panic. Reconfigure to the live window and skip this frame.
        let tex_size = surface_texture.texture.size();
        if tex_size.width != size.width || tex_size.height != size.height {
            p.surface_config.width = live.width.max(1);
            p.surface_config.height = live.height.max(1);
            p.needs_configure = true;
            for id in &textures_delta.free {
                p.renderer.free_texture(id);
            }
            return;
        }
        crate::perf::span_done("frame.surface", surface_started.elapsed());
        let surface_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [size.width, size.height],
            pixels_per_point,
        };
        crate::perf::span_done("frame.textures", textures_started.elapsed());
        let buffers_started = Instant::now();
        let mut encoder = p
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("shell encoder"),
            });
        let cmd_bufs = p.renderer.update_buffers(
            &p.gpu.device,
            &p.gpu.queue,
            &mut encoder,
            paint_jobs,
            &screen_descriptor,
        );
        crate::perf::span_done("frame.buffers", buffers_started.elapsed());
        let pass_started = Instant::now();
        {
            // The ground comes off the theme, converted for the surface
            // format we actually got — see `Theme::ground_wgpu`.
            let ground = shell.theme().ground_wgpu(p.surface_config.format.is_srgb());
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shell pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(ground),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            p.renderer.render(&mut pass, paint_jobs, &screen_descriptor);
        }
        crate::perf::span_done("frame.pass", pass_started.elapsed());
        let submit_started = Instant::now();
        p.gpu
            .queue
            .submit(cmd_bufs.into_iter().chain(Some(encoder.finish())));
        crate::perf::span_done("frame.submit", submit_started.elapsed());
        // Freed only after the submit: this frame's draws still use them.
        for id in &textures_delta.free {
            p.renderer.free_texture(id);
        }
        let present_started = Instant::now();
        p.gpu.queue.present(surface_texture);
        crate::perf::span_done("frame.present", present_started.elapsed());
    }

    /// The `egui_glow` twin of `redraw_wgpu`: clear to `Theme::ground_glow`,
    /// hand the tessellated shapes to `egui_glow::Painter`, swap buffers.
    /// `srgb_surface` never enters this path — the GL config above never
    /// asks for an sRGB-capable surface (`with_transparency(false)` and no
    /// `ARB_framebuffer_sRGB` request), so `ground_glow` always does the
    /// non-sRGB half of the conversion `ground_wgpu` also has, and there is
    /// only one branch to take.
    #[cfg(feature = "glow")]
    fn redraw_glow(
        p: &mut GlowPainter,
        window: &Window,
        shell: &Shell,
        paint_jobs: &[egui::ClippedPrimitive],
        textures_delta: &mut egui::TexturesDelta,
        size: winit::dpi::PhysicalSize<u32>,
        pixels_per_point: f32,
    ) {
        use glutin::surface::GlSurface as _;

        p.painter
            .clear([size.width, size.height], shell.theme().ground_glow());
        p.painter.paint_and_update_textures(
            [size.width, size.height],
            pixels_per_point,
            paint_jobs,
            textures_delta,
        );
        let _ = window;
        if let Err(e) = p.gl_surface.swap_buffers(&p.gl_context) {
            eprintln!("  glow swap_buffers failed: {e}");
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.rendering.is_some() {
            return;
        }
        match self.open(event_loop) {
            Ok(r) => {
                self.rendering = Some(r);
                self.wake = Wake::Now;
                // The pace comes from `pace` in `about_to_wait`, one frame per
                // `frame_gap` while egui wants one and a heartbeat otherwise —
                // not `ControlFlow::Poll`, which redrew at whatever the
                // monitor's refresh is.
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Err(why) => {
                self.failure = Some(why);
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(r) = &mut self.rendering else { return };

        let response = r.egui_state.on_window_event(&r.window, &event);
        // egui-winit answers `repaint` for `RedrawRequested` too. That is the
        // paint itself, not a request for the next one: waking on it would
        // redraw again at once, and an idle frame that skips its paint never
        // moves the frame gap on.
        if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
            self.wake = Wake::Now;
            pace(event_loop, &r.window, self.wake, self.last_frame, self.presenter.live());
        }
        if response.consumed {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    match &mut r.painter {
                        #[cfg(feature = "gpu")]
                        Painter::Wgpu(p) => {
                            p.surface_config.width = size.width;
                            p.surface_config.height = size.height;
                            p.needs_configure = !p.configure_surface();
                        }
                        #[cfg(feature = "glow")]
                        Painter::Glow(p) => {
                            use glutin::surface::GlSurface as _;
                            if let (Some(w), Some(h)) = (
                                std::num::NonZeroU32::new(size.width),
                                std::num::NonZeroU32::new(size.height),
                            ) {
                                p.gl_surface.resize(&p.gl_context, w, h);
                            }
                        }
                    }
                    // `S.U13`'s arrangement follows the resize with no
                    // further plumbing here: `redraw`'s `take_egui_input`
                    // reads `window.inner_size()` live every frame
                    // (`egui-winit`'s own `update_viewport_info`), so
                    // `Shell::ui`'s `ui.input(|i| i.viewport_rect())` always
                    // sees the real current size, not the one this event
                    // reports — this branch only exists to keep the surface
                    // (either backend's) in step. Verified against
                    // `egui-winit` 0.36.1's source (`window.inner_size()` at
                    // the call site), not by dragging a real edge on this
                    // box: niri's scrollable-tiling layout resizes columns,
                    // not window edges, so there is no drag gesture here to
                    // record — `niri msg action set-column-width` (this
                    // lote's own verification) exercises the same `Resized`
                    // event a drag would.
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(r) = &self.rendering {
            pace(event_loop, &r.window, self.wake, self.last_frame, self.presenter.live());
        }
    }
}

/// Decide when the next frame happens — the one place the client's pace is
/// applied. Each frame is at least [`frame_gap`] after the last. A
/// [`Wake::Now`] repaint goes as soon as that allows; a [`Wake::At`] waits
/// for its instant; [`Wake::Idle`] waits out [`IDLE_REFRESH`]. Not yet due
/// parks the loop until the deadline instead of spinning.
fn pace(
    event_loop: &ActiveEventLoop,
    window: &Window,
    wake: Wake,
    last_frame: Option<Instant>,
    live: bool,
) {
    let now = Instant::now();
    let floor = last_frame.map_or(now, |last| last + frame_gap());
    let due = match wake {
        Wake::Now => floor,
        Wake::At(at) => at.max(floor),
        // Nothing asked for a frame. On a software GPU a paint takes hundreds
        // of milliseconds, so the 10 Hz heartbeat would only keep the phone
        // painting for nothing: park until input or a request. Desktop keeps
        // the heartbeat, because its live readouts are drawn from it.
        Wake::Idle if super::frames::software_gpu() => {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        // A GPU preview on screen keeps the client at the 60 Hz pace: the
        // picture is what the operator is watching, and it must not wait for
        // the 10 Hz heartbeat.
        Wake::Idle if live => floor,
        Wake::Idle => last_frame.map_or(now, |last| last + IDLE_REFRESH).max(floor),
    };
    if due <= now {
        window.request_redraw();
    } else {
        event_loop.set_control_flow(ControlFlow::WaitUntil(due));
    }
}
