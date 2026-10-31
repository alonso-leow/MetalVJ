//! Device/queue setup, shared between the windowed app and the headless
//! test gates. The two paths differ only in whether an adapter is asked to
//! be compatible with a surface. `request_adapter` itself is called from
//! exactly one place in this crate: [`request_adapter_ladder`], below —
//! `new_headless` and `new_windowed` both go through it, and so does
//! [`probe`].

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Whether the adapter runs compute shaders. WebGL2 does not, so a model
    /// that dispatches compute cannot draw on it; see [`crate::model::Model::needs_compute`].
    pub compute: bool,
    /// Which backend the adapter is (Vulkan, Metal, GL, browser WebGPU, …).
    pub backend: wgpu::Backend,
    /// `false` for a software rasterizer (llvmpipe, WARP, SwiftShader): the
    /// adapter works, but compose is faster on the CPU there. G1's startup
    /// backend choice reads it.
    pub real_adapter: bool,
    /// The adapter the device came from: a window surface is checked against
    /// it and configured from it when the window shares this device.
    pub adapter: wgpu::Adapter,
    /// The canvas surface a WebGL2 context was created on. Held, not drawn to:
    /// the models render offscreen. Dropping it could take the context with it.
    #[cfg(target_arch = "wasm32")]
    pub surface: Option<wgpu::Surface<'static>>,
}

/// `METALVJ_ADAPTER`: a case-insensitive substring of the adapter name
/// (`Intel`, `NVIDIA`). When set and an adapter matches (and can present to
/// `compatible_surface`, if one is given), that adapter is used instead of the
/// ladder. No match falls through to the ladder.
async fn request_adapter_override(
    instance: &wgpu::Instance,
    compatible_surface: Option<&wgpu::Surface<'_>>,
) -> Option<wgpu::Adapter> {
    let wanted = std::env::var("METALVJ_ADAPTER").ok()?.to_lowercase();
    if wanted.trim().is_empty() {
        return None;
    }
    let found = instance
        .enumerate_adapters(wgpu::Backends::PRIMARY)
        .await
        .into_iter()
        .find(|a| {
            a.get_info().name.to_lowercase().contains(wanted.trim())
                && compatible_surface.is_none_or(|s| a.is_surface_supported(s))
        });
    match &found {
        Some(a) => eprintln!("metal-render: METALVJ_ADAPTER={wanted} -> {}", a.get_info().name),
        None => eprintln!("metal-render: METALVJ_ADAPTER={wanted} matched no adapter; using the default ladder"),
    }
    found
}

/// The adapter class in the binding ranking (`the design notes` G): higher is better,
/// and the order is explicit rather than `power_preference`. A discrete GPU
/// beats an integrated one, which beats a virtual/paravirtual one, which beats
/// a software rasterizer. `Other` (an unknown type) sits just above software
/// and below every known GPU.
fn adapter_class(device_type: wgpu::DeviceType) -> u8 {
    match device_type {
        wgpu::DeviceType::DiscreteGpu => 4,
        wgpu::DeviceType::IntegratedGpu => 3,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 1,
        wgpu::DeviceType::Cpu => 0,
    }
}

/// The class rank of an adapter, highest wins (`the design notes` G: `Discrete` >
/// `Integrated` > `Virtual` > `Cpu`). Pure and hardware-free, so the ranking
/// is unit-tested on synthetic [`wgpu::AdapterInfo`] values. Ties within one
/// class are broken by `max_buffer_size` — see [`outranks`].
pub fn rank(info: &wgpu::AdapterInfo) -> u8 {
    adapter_class(info.device_type)
}

/// Whether class `ra` at size `sa` outranks class `rb` at size `sb`: the
/// class first, then the larger `max_buffer_size` (a proxy for VRAM) as the
/// tie-break. `rank` alone cannot express the tie-break, so this is the pure
/// half of the comparison [`pick_adapter`] uses.
pub fn outranks(ra: u8, sa: u64, rb: u8, sb: u64) -> bool {
    (ra, sa) > (rb, sb)
}

/// The best adapter to compose on: [`rank`] over every adapter
/// `Backends::PRIMARY` exposes, optionally restricted to ones that can
/// present to `surface`. `METALVJ_ADAPTER` still overrides; the
/// [`request_adapter_ladder`] stays as the fallback for a backend that
/// enumerates nothing. A discrete GPU wins even when the surface is wired to
/// an integrated one (the caller keeps this adapter to paint on and gives the
/// window a surface-capable one — see `shell::window::open_wgpu`).
pub async fn pick_adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
) -> Option<wgpu::Adapter> {
    if let Some(adapter) = request_adapter_override(instance, surface).await {
        return Some(adapter);
    }
    let mut best: Option<(u8, u64, wgpu::Adapter)> = None;
    for adapter in instance.enumerate_adapters(wgpu::Backends::PRIMARY).await {
        if let Some(s) = surface {
            if !adapter.is_surface_supported(s) {
                continue;
            }
        }
        let key = (rank(&adapter.get_info()), adapter.limits().max_buffer_size);
        if best.as_ref().is_none_or(|(r, sz, _)| outranks(key.0, key.1, *r, *sz)) {
            best = Some((key.0, key.1, adapter));
        }
    }
    match best {
        Some((_, size, adapter)) => {
            let info = adapter.get_info();
            eprintln!(
                "metal-render: ranked adapter {} ({:?}, {} MiB max buffer)",
                info.name,
                info.device_type,
                size >> 20
            );
            Some(adapter)
        }
        None => request_adapter_ladder(instance, surface).await,
    }
}

/// Tries, in order, the caller's `power_preference`, then `LowPower`, then
/// `LowPower` with `force_fallback_adapter: true` (a software rasterizer on
/// backends that ship one). Returns the first adapter any rung produces;
/// `None` only once all three have come back empty — it is the caller's
/// call whether that is a panic (today, everywhere but `probe`) or a
/// `Backend::Glow` decision (`probe`, and eventually `S.U15`'s fallback
/// paint path). `METALVJ_ADAPTER` (see [`request_adapter_override`]) is
/// consulted first.
async fn request_adapter_ladder(
    instance: &wgpu::Instance,
    compatible_surface: Option<&wgpu::Surface<'_>>,
) -> Option<wgpu::Adapter> {
    if let Some(adapter) = request_adapter_override(instance, compatible_surface).await {
        return Some(adapter);
    }
    for (power_preference, force_fallback_adapter) in [
        (wgpu::PowerPreference::HighPerformance, false),
        (wgpu::PowerPreference::LowPower, false),
        (wgpu::PowerPreference::LowPower, true),
    ] {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference,
                compatible_surface,
                force_fallback_adapter,
                apply_limit_buckets: false,
            })
            .await;
        if let Ok(adapter) = adapter {
            return Some(adapter);
        }
    }
    None
}

/// The process-wide instance (native). The headless renderers and the window's
/// surface come from this one instance, so the window can present the very
/// textures the renderers draw on: one device, no copy through the CPU.
#[cfg(not(target_arch = "wasm32"))]
pub fn shared_instance() -> &'static wgpu::Instance {
    static INSTANCE: std::sync::OnceLock<wgpu::Instance> = std::sync::OnceLock::new();
    INSTANCE.get_or_init(|| {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::PRIMARY;
        wgpu::Instance::new(desc)
    })
}

fn headless_instance(backends: wgpu::Backends) -> wgpu::Instance {
    #[cfg(not(target_arch = "wasm32"))]
    if backends == wgpu::Backends::PRIMARY {
        return shared_instance().clone();
    }
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = backends;
    // A browser cannot block on a GL fence, so its `poll(Wait)` would answer
    // `Timeout` for every readback. `AutoFinish` makes the fences answer at
    // once; the readback's own `getBufferSubData` is what waits, on the GPU.
    #[cfg(target_arch = "wasm32")]
    {
        desc.backend_options.gl.fence_behavior = wgpu::GlFenceBehavior::AutoFinish;
    }
    wgpu::Instance::new(desc)
}

/// Serializes wgpu adapter acquisition process-wide. `request_adapter` runs
/// on the driver, and two concurrent acquisitions (tests spawning parallel
/// real-door/Author builds, the window at startup) crash some drivers with
/// SIGSEGV in the adapter probe. The lock guards only the acquisition, not
/// the draw commands afterwards.
pub(crate) static ADAPTER: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

/// Acquire the adapter-creation lock, healing a poisoned one (a poisoned
/// lock carries no invariant to lose).
pub(crate) fn adapter_lock() -> std::sync::MutexGuard<'static, ()> {
    ADAPTER.get_or_init(std::sync::Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Gpu {
    async fn from_adapter(instance: &wgpu::Instance, adapter: &wgpu::Adapter) -> Gpu {
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("metal-render device"),
                // 2026-10-01: HAP footage uploads its BC blocks untouched
                // where the adapter samples them (`shared::footage`); a device
                // without them decodes on the CPU instead.
                required_features: adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC,
                // WebGL2 has no compute limits at all (the workgroup counts are
                // zero), so its device asks for the WebGL2 defaults, at the
                // adapter's own resolution; every other backend asks for the
                // defaults it always did.
                required_limits: if adapter.get_info().backend == wgpu::Backend::Gl {
                    wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())
                } else {
                    wgpu::Limits::default()
                },
                memory_hints: wgpu::MemoryHints::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                trace: wgpu::Trace::Off,
            })
            .await
            .expect("request_device failed");
        let _ = instance;
        let compute = adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS);
        Gpu {
            device,
            queue,
            compute,
            backend: adapter.get_info().backend,
            real_adapter: adapter.get_info().device_type != wgpu::DeviceType::Cpu,
            adapter: adapter.clone(),
            #[cfg(target_arch = "wasm32")]
            surface: None,
        }
    }

    /// Another handle to the same device, queue and adapter (they are
    /// reference-counted): what the window renders egui on when it shares the
    /// painter's device.
    pub fn share(&self) -> Gpu {
        Gpu {
            device: self.device.clone(),
            queue: self.queue.clone(),
            compute: self.compute,
            backend: self.backend,
            real_adapter: self.real_adapter,
            adapter: self.adapter.clone(),
            #[cfg(target_arch = "wasm32")]
            surface: None,
        }
    }

    /// No surface: used by the headless render gates in `tests/render.rs`
    /// and by `metal-dash`'s model painter (`metal_render::headless::Renderer`).
    /// Behaviour preserved from before `S.U3`: still panics on total
    /// failure, still `PRIMARY` backends — the ladder only adds the
    /// `LowPower`/`force_fallback_adapter` rungs before giving up.
    pub fn new_headless() -> Gpu {
        pollster::block_on(async {
            let instance = headless_instance(wgpu::Backends::PRIMARY);
            let adapter = pick_adapter(&instance, None)
                .await
                .expect("no wgpu adapter (headless), even after ranking and LowPower/force_fallback retries");
            Gpu::from_adapter(&instance, &adapter).await
        })
    }

    /// Compatible with `surface`: used by the real app window. Returns the
    /// adapter alongside the `Gpu` — the caller still needs it for
    /// `surface.get_default_config` — and `None` (never a panic) once the
    /// ladder is exhausted, so a caller can choose a fallback instead of
    /// aborting. `metal-render` itself does not implement that fallback yet
    /// (`S.U15` does, via [`probe`]); today's caller (`app.rs`) still
    /// `.expect()`s this `Option`, which is why "the window opens exactly
    /// as before" holds.
    pub async fn new_windowed(
        instance: &wgpu::Instance,
        surface: &wgpu::Surface<'_>,
    ) -> Option<(wgpu::Adapter, Gpu)> {
        let _adapter = adapter_lock();
        let adapter = pick_adapter(instance, Some(surface)).await?;
        let gpu = Gpu::from_adapter(instance, &adapter).await;
        Some((adapter, gpu))
    }
}

/// The browser's device. An `await` on the adapter and the device, never
/// `pollster`: a blocked future on wasm never returns to the page that would
/// resolve it.
///
/// WebGPU where the page has it (`navigator.gpu`). Otherwise WebGL2: a GL
/// adapter only exists once a context does, and wgpu creates that context on a
/// canvas surface, so a detached `<canvas>` is made for it here. The models
/// that need compute shaders then report `compute == false` and the painter
/// draws them on the CPU (see [`Gpu::compute`]). `None` when the browser has
/// neither.
#[cfg(target_arch = "wasm32")]
pub async fn web() -> Option<Gpu> {
    let instance = headless_instance(wgpu::Backends::BROWSER_WEBGPU);
    if let Some(adapter) = request_adapter_ladder(&instance, None).await {
        return Some(Gpu::from_adapter(&instance, &adapter).await);
    }

    use wasm_bindgen::JsCast;
    let canvas = web_sys::window()?
        .document()?
        .create_element("canvas")
        .ok()?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .ok()?;
    let instance = headless_instance(wgpu::Backends::GL);
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .ok()?;
    let adapter = request_adapter_ladder(&instance, Some(&surface)).await?;
    let mut gpu = Gpu::from_adapter(&instance, &adapter).await;
    gpu.surface = Some(surface);
    Some(gpu)
}

/// [`web`] inside a Web Worker, drawing on an `OffscreenCanvas` the page
/// transferred (`canvas.transferControlToOffscreen()`): there is no `window`
/// or `document` in a worker, so the GL fallback's surface is made on the
/// transferred canvas. `None` when the worker has neither WebGPU nor WebGL2.
#[cfg(target_arch = "wasm32")]
pub async fn web_worker(canvas: web_sys::OffscreenCanvas) -> Option<Gpu> {
    let instance = headless_instance(wgpu::Backends::BROWSER_WEBGPU);
    if let Some(adapter) = request_adapter_ladder(&instance, None).await {
        return Some(Gpu::from_adapter(&instance, &adapter).await);
    }
    let instance = headless_instance(wgpu::Backends::GL);
    let surface = instance.create_surface(wgpu::SurfaceTarget::OffscreenCanvas(canvas)).ok()?;
    let adapter = request_adapter_ladder(&instance, Some(&surface)).await?;
    let mut gpu = Gpu::from_adapter(&instance, &adapter).await;
    gpu.surface = Some(surface);
    Some(gpu)
}

/// Which paint backend a window should use. `Glow` is a decision point
/// only as of `S.U3` — no `egui_glow` painter exists yet in this crate.
/// `S.U15` is the batch that plugs one in behind this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Wgpu,
    Glow,
}

/// Runs [`request_adapter_ladder`] against a headless instance (no window
/// needed) restricted to `backends`, and reports `Wgpu` if any rung of the
/// ladder finds an adapter, `Glow` if all three come back empty.
///
/// This only ever probes adapter *presence*. It does not attempt
/// `request_device`, so it cannot and does not fold "adapter found, device
/// creation failed" into `Glow` — that is a different failure, still
/// surfaced as a panic in [`Gpu::from_adapter`], and no batch has decided
/// what it should do yet.
///
/// `backends` is the seam a test uses to force a deterministic no-adapter
/// result: `wgpu::Backends::empty()` guarantees `request_adapter` returns
/// `None` on every rung, regardless of what hardware this machine has.
/// Ordinary callers pass `wgpu::Backends::PRIMARY`.
pub fn probe(backends: wgpu::Backends) -> Backend {
    pollster::block_on(async {
        let instance = headless_instance(backends);
        match request_adapter_ladder(&instance, None).await {
            Some(_) => Backend::Wgpu,
            None => Backend::Glow,
        }
    })
}

/// Pulls an 8-bit RGBA texture back to the CPU, dropping the row padding the
/// copy requires. This is how a renderer that only knows how to fill a
/// texture ends up in a browser: `fw-preview` renders the grid, reads
/// it back here, and encodes it.
pub fn read_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let bytes_per_row = (width * 4).next_multiple_of(256);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback") });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit(Some(encoder.finish()));

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
        .expect("device poll failed");
    rx.recv().expect("readback never completed").expect("buffer map failed");

    let data = slice.get_mapped_range().expect("get_mapped_range failed");
    let row = (width * 4) as usize;
    let mut out = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        let start = y * bytes_per_row as usize;
        out.extend_from_slice(&data[start..start + row]);
    }
    drop(data);
    buffer.unmap();
    out
}

/// Allocates an offscreen render target every model draws into. Shared by
/// the app (one per frame, then blit-with-shake to the surface) and the
/// test gates (rendered once, then read back).
pub fn offscreen_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("metal-render offscreen"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::DeviceType;

    /// A synthetic adapter of a given class; no hardware is touched.
    fn info(kind: DeviceType) -> wgpu::AdapterInfo {
        wgpu::AdapterInfo {
            name: format!("{kind:?}"),
            device_type: kind,
            ..wgpu::AdapterInfo::new(kind, wgpu::Backend::Vulkan)
        }
    }

    #[test]
    fn a_discrete_adapter_beats_an_integrated_one() {
        assert!(rank(&info(DeviceType::DiscreteGpu)) > rank(&info(DeviceType::IntegratedGpu)));
    }

    #[test]
    fn an_apu_picks_the_integrated_adapter_over_virtual_and_software() {
        assert!(rank(&info(DeviceType::IntegratedGpu)) > rank(&info(DeviceType::VirtualGpu)));
        assert!(rank(&info(DeviceType::IntegratedGpu)) > rank(&info(DeviceType::Cpu)));
    }

    #[test]
    fn a_software_adapter_ranks_last() {
        for better in [DeviceType::DiscreteGpu, DeviceType::IntegratedGpu, DeviceType::VirtualGpu] {
            assert!(rank(&info(better)) > rank(&info(DeviceType::Cpu)), "{better:?} must beat software");
        }
    }

    #[test]
    fn a_larger_max_buffer_size_breaks_a_tie_but_never_a_class() {
        assert!(outranks(3, 2 << 30, 3, 1 << 30), "same class, more memory wins");
        assert!(!outranks(3, 1 << 30, 3, 2 << 30), "same class, less memory loses");
        assert!(outranks(4, 1, 3, u64::MAX), "the class dominates the size");
    }
}
