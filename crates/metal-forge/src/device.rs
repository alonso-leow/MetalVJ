//! The device this whole crate draws with — deliberately never the one the
//! live show/session uses. `the design notes` H.2: "It runs on a
//! separate device... a shader that hangs the GPU cannot take down the show
//! that is projecting on a wall." One process can hold more than one
//! `wgpu::Device` against the same adapter (or a different one); this
//! module is the only place `metal-forge` touches `wgpu::Instance`.

use std::sync::{Arc, OnceLock};

pub struct ForgeDevice {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

static FORGE_DEVICE: OnceLock<Arc<ForgeDevice>> = OnceLock::new();

/// Lazily creates — once — the device `compile`/`raster` share. Never the
/// device passed to `metal_render::model::Ctx`; nothing in this crate accepts
/// one from a caller, on purpose, so there is no way to accidentally wire
/// forge onto the live rendering path.
pub fn forge_device() -> &'static Arc<ForgeDevice> {
    FORGE_DEVICE.get_or_init(|| Arc::new(pollster::block_on(create())))
}

async fn create() -> ForgeDevice {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::PRIMARY;
    let instance = wgpu::Instance::new(desc);
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        })
        .await
        .expect("metal-forge: no wgpu adapter available for its own device");
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("metal-forge device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            trace: wgpu::Trace::Off,
        })
        .await
        .expect("metal-forge: adapter refused to hand out a device");
    ForgeDevice { device, queue }
}
