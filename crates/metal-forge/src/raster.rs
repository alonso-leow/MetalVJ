//! `raster`'s two hard budgets — H.2's third non-negotiable — plus the
//! actual draw-and-readback.

use std::sync::mpsc;
use std::time::Duration;

use crate::Pipeline;

/// Hard wall-clock budget for one `raster` call. The plan asks for "a few
/// seconds" without pinning a number; 5s is the judgment call made here —
/// generous enough that a legitimately expensive but finite shader (heavy
/// raymarching at 4K) has a real chance, short enough that the agent loop's
/// own `sleep` tool (built for exactly this wait) does not itself time out
/// waiting on it. Revisit if either assumption turns out wrong in practice.
pub const RASTER_TIMEOUT: Duration = Duration::from_secs(5);

/// Hard resolution cap in each dimension. 4096 is the plan's own suggested
/// number ("a resolution cap... for example 4096x4096"); kept as
/// suggested rather than picked independently.
pub const MAX_DIMENSION: u32 = 4096;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RasterError {
    #[error("requested resolution {w}x{h} exceeds the {MAX_DIMENSION}x{MAX_DIMENSION} cap")]
    ResolutionTooLarge { w: u32, h: u32 },
    #[error("resolution must be nonzero, got {w}x{h}")]
    ZeroResolution { w: u32, h: u32 },
    #[error("floats slice has {got} entries, the shader's uniform block only holds {capacity}")]
    TooManyFloats { got: usize, capacity: usize },
    #[error("raster exceeded the {0:?} timeout")]
    Timeout(Duration),
    #[error("raster GPU work panicked: {0}")]
    Panicked(String),
}

/// Compiles-and-validates output already checked into a `Pipeline` by
/// [`crate::compile`]; `raster` turns it into a PNG for one frame. `floats`
/// is written into the shader's `floatsN` uniform members in declaration
/// order, 4 at a time, zero-padded if it does not fill the whole block.
///
/// Bounded on both axes per H.2's third non-negotiable: `w`/`h` are checked
/// against [`MAX_DIMENSION`] before any GPU work happens, and the GPU
/// submission + readback runs on a worker thread joined with
/// [`RASTER_TIMEOUT`] — a shader that never finishes (an infinite loop in
/// `fs_main`, a driver hang) fails this call instead of hanging the caller,
/// though the worker thread itself is intentionally leaked rather than
/// force-killed: wgpu has no API to cancel a submitted command buffer
/// mid-flight, and killing the thread would leave the forge device's
/// internal state (encoders, mapped buffers) in an unknown condition for
/// the *next* call. A raster that times out is rare enough, and the forge
/// device isolated enough (H.2's second non-negotiable), that leaking one
/// worker is the safer failure than resetting shared GPU state from outside
/// the thread that owns it.
pub fn raster(pipeline: &Pipeline, floats: &[f32], w: u32, h: u32) -> Result<Vec<u8>, RasterError> {
    if w == 0 || h == 0 {
        return Err(RasterError::ZeroResolution { w, h });
    }
    if w > MAX_DIMENSION || h > MAX_DIMENSION {
        return Err(RasterError::ResolutionTooLarge { w, h });
    }
    if floats.len() > pipeline.float_capacity {
        return Err(RasterError::TooManyFloats { got: floats.len(), capacity: pipeline.float_capacity });
    }

    // The worker thread gets only owned, cloned handles — `wgpu::Device`,
    // `wgpu::RenderPipeline`, `wgpu::BindGroupLayout` are cheap `Clone`
    // wrappers onto driver objects — never a borrow of `pipeline` itself.
    // That is what makes it safe to abandon the thread (never join it) on
    // timeout: it does not reference anything on `raster`'s stack.
    let device = pipeline.device.clone();
    let uniform_bytes = crate::uniform_bytes(pipeline.uniform_size, &pipeline.float_offsets, floats);
    let effect = pipeline.effect;
    let bind_group_layout = pipeline.bind_group_layout.clone();
    let render_pipeline = pipeline.pipeline.clone();

    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("metal-forge-raster".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                render_and_read(&device, &render_pipeline, &bind_group_layout, &uniform_bytes, effect, w, h)
            }));
            let _ = tx.send(result);
        })
        .expect("metal-forge: failed to spawn raster worker thread");

    match rx.recv_timeout(RASTER_TIMEOUT) {
        Ok(Ok(pixels)) => encode_png(&pixels, w, h),
        Ok(Err(panic_payload)) => {
            let msg = panic_payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            Err(RasterError::Panicked(msg))
        }
        Err(mpsc::RecvTimeoutError::Timeout) => Err(RasterError::Timeout(RASTER_TIMEOUT)),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(RasterError::Panicked("raster worker dropped without a result".into()))
        }
    }
}

fn render_and_read(
    device: &crate::ForgeDevice,
    pipeline: &wgpu::RenderPipeline,
    bind_group_layout: &wgpu::BindGroupLayout,
    uniform_bytes: &[u8],
    effect: bool,
    w: u32,
    h: u32,
) -> Vec<u8> {
    use wgpu::util::DeviceExt;

    let texture = device.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("metal-forge raster target"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: crate::TARGET_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let ubuf = device.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("metal-forge raster uniform"),
        contents: uniform_bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    // An effect is rastered over a test picture — a checker with a colour
    // ramp across it — so what it does to edges, flat areas and colour is
    // all visible in one PNG.
    let input = effect.then(|| test_picture(device, w, h));
    let sampler = device.device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("metal-forge raster sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }];
    if let Some(view) = &input {
        entries.push(wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) });
        entries.push(wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) });
    }
    let bind_group = device.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("metal-forge raster bind group"),
        layout: bind_group_layout,
        entries: &entries,
    });

    // Rows in a `COPY_SRC` -> buffer readback have to be padded to a
    // multiple of `COPY_BYTES_PER_ROW_ALIGNMENT` (256) — the same reason
    // every headless wgpu screenshot path in the ecosystem carries this
    // exact padding dance.
    const ALIGN: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let unpadded_bytes_per_row = w * 4;
    let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(ALIGN) * ALIGN;

    let output_buffer = device.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("metal-forge raster readback"),
        size: (padded_bytes_per_row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("metal-forge raster encoder") });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("metal-forge raster pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &output_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    device.queue.submit(Some(encoder.finish()));

    let slice = output_buffer.slice(..);
    let (map_tx, map_rx) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = map_tx.send(r);
    });
    device
        .device
        .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
        .expect("metal-forge: device poll failed");
    map_rx
        .recv()
        .expect("metal-forge: map_async callback never fired")
        .expect("metal-forge: buffer map failed");

    let data = slice.get_mapped_range().expect("metal-forge: get_mapped_range failed after a successful map");
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h {
        let start = (row * padded_bytes_per_row) as usize;
        pixels.extend_from_slice(&data[start..start + unpadded_bytes_per_row as usize]);
    }
    drop(data);
    output_buffer.unmap();
    pixels
}

/// The picture an effect is rastered over: an 8×8 checker of black and a
/// hue ramp, so an effect's work shows on edges, on flat colour and across
/// hues at once.
fn test_picture(device: &crate::ForgeDevice, w: u32, h: u32) -> wgpu::TextureView {
    use wgpu::util::DeviceExt;
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let on = ((x * 8 / w.max(1)) + (y * 8 / h.max(1))) % 2 == 0;
            let t = x as f32 / w.max(1) as f32;
            let rgb = [
                (0.5 + 0.5 * (6.283 * t).cos()) * 255.0,
                (0.5 + 0.5 * (6.283 * (t - 0.333)).cos()) * 255.0,
                (0.5 + 0.5 * (6.283 * (t - 0.667)).cos()) * 255.0,
            ];
            if on {
                px.extend_from_slice(&[rgb[0] as u8, rgb[1] as u8, rgb[2] as u8, 255]);
            } else {
                px.extend_from_slice(&[16, 16, 16, 255]);
            }
        }
    }
    let texture = device.device.create_texture_with_data(
        &device.queue,
        &wgpu::TextureDescriptor {
            label: Some("metal-forge test picture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::TARGET_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &px,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

fn encode_png(pixels: &[u8], w: u32, h: u32) -> Result<Vec<u8>, RasterError> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .unwrap_or_else(|e| panic!("metal-forge: png header write failed: {e}"));
        writer
            .write_image_data(pixels)
            .unwrap_or_else(|e| panic!("metal-forge: png data write failed: {e}"));
    }
    Ok(out)
}
