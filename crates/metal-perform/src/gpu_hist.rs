//! G5 - the grade histogram on the GPU: one compute pass over a picture's
//! texture counts 256 bins each of red, green, blue and luma (atomics into a
//! storage buffer), and the 4 KiB result comes back through a double-buffered
//! async readback. The CPU reduction (`histogram_of` in `metal-dash`) stays
//! the reference: same byte values, same luma weights, same pixel stride, so
//! the two agree bin for bin.
//!
//! The reduction reads the `Rgba8Unorm` view ([`crate::gpu_frame::GpuFrame::view`]),
//! not the sRGB texture, so every channel is the encoded byte, as the CPU sees it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::gpu_compose::GpuFrame;

/// 256 bins each of red, green, blue and luma, the shape `metal-dash` paints.
pub type Bins = [[u32; 256]; 4];

const BINS_BYTES: u64 = (4 * 256 * 4) as u64;

const SHADER: &str = r#"
struct Params { w: u32, h: u32, stride: u32, pad: u32 }

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> bins: array<atomic<u32>, 1024>;
@group(0) @binding(2) var<uniform> p: Params;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.w || id.y >= p.h) {
        return;
    }
    let idx = id.y * p.w + id.x;
    if (idx % p.stride != 0u) {
        return;
    }
    let c = vec3<u32>(round(textureLoad(src, vec2<i32>(i32(id.x), i32(id.y)), 0).rgb * 255.0));
    atomicAdd(&bins[c.r], 1u);
    atomicAdd(&bins[256u + c.g], 1u);
    atomicAdd(&bins[512u + c.b], 1u);
    let y = min((c.r * 54u + c.g * 183u + c.b * 19u) >> 8u, 255u);
    atomicAdd(&bins[768u + y], 1u);
}
"#;

struct Staging {
    buf: wgpu::Buffer,
    /// Set by the map callback once the copy is readable.
    done: Option<Arc<AtomicBool>>,
}

/// The histogram pass and its readback. `T` is whatever the caller uses to
/// tell its pictures apart (the key of the texture it asked about).
pub struct Histogrammer<T> {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    bins: wgpu::Buffer,
    params: wgpu::Buffer,
    slots: [Staging; 2],
    next: usize,
    /// Submission order, oldest first: the slot each one copied into and its tag.
    queued: VecDeque<(usize, T)>,
}

impl<T> Histogrammer<T> {
    /// `None` when the device cannot run the pass (no compute shaders, as on
    /// WebGL2, or no storage buffers): the caller reads the picture back and
    /// reduces it on the CPU instead.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, compute: bool) -> Option<Histogrammer<T>> {
        if !compute || device.limits().max_storage_buffers_per_shader_stage < 1 {
            return None;
        }
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grade histogram"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grade histogram layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grade histogram"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("grade histogram"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let bins = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grade histogram bins"),
            size: BINS_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grade histogram params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let staging = || Staging {
            buf: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("grade histogram readback"),
                size: BINS_BYTES,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            done: None,
        };
        Some(Histogrammer {
            device: device.clone(),
            queue: queue.clone(),
            pipeline,
            layout,
            bins,
            params,
            slots: [staging(), staging()],
            next: 0,
            queued: VecDeque::new(),
        })
    }

    /// Queues the count of `frame` (every `stride`-th pixel, row-major, as the
    /// CPU reduction samples them) and the copy of the result. Returns `false`
    /// and does nothing when both readback slots are still in flight: the
    /// caller tries again on a later frame instead of waiting. Never blocks.
    pub fn submit(&mut self, frame: &GpuFrame, stride: u32, tag: T) -> bool {
        let i = self.next;
        if self.slots[i].done.is_some() {
            return false;
        }
        let (w, h) = (frame.width(), frame.height());
        let params = [w, h, stride.max(1), 0u32];
        let mut bytes = [0u8; 16];
        for (i, word) in params.iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        self.queue.write_buffer(&self.params, 0, &bytes);
        let view = frame.view();
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grade histogram"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: self.bins.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.params.as_entire_binding() },
            ],
        });
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("grade histogram") });
        enc.clear_buffer(&self.bins, 0, None);
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("grade histogram"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(w.div_ceil(16), h.div_ceil(16), 1);
        }
        enc.copy_buffer_to_buffer(&self.bins, 0, &self.slots[i].buf, 0, BINS_BYTES);
        self.queue.submit(Some(enc.finish()));
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        self.slots[i].buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            if r.is_ok() {
                flag.store(true, Ordering::Release);
            }
        });
        self.slots[i].done = Some(done);
        self.queued.push_back((i, tag));
        self.next = 1 - i;
        true
    }

    /// The oldest finished histogram, if any; never waits on the GPU.
    pub fn poll(&mut self) -> Option<(T, Bins)> {
        let _ = self.device.poll(wgpu::PollType::Poll);
        let (i, _) = self.queued.front()?;
        let i = *i;
        let done = self.slots[i].done.as_ref()?;
        if !done.load(Ordering::Acquire) {
            return None;
        }
        let (_, tag) = self.queued.pop_front()?;
        let bins = match self.slots[i].buf.slice(..).get_mapped_range() {
            Ok(data) => read_bins(&data),
            Err(_) => [[0u32; 256]; 4],
        };
        self.slots[i].buf.unmap();
        self.slots[i].done = None;
        Some((tag, bins))
    }

    /// Waits until every queued histogram has landed, then returns them in
    /// order. For tests and the measure only: the window never waits.
    pub fn drain(&mut self) -> Vec<(T, Bins)> {
        let mut out = Vec::new();
        while !self.queued.is_empty() {
            let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
            match self.poll() {
                Some(one) => out.push(one),
                None => break,
            }
        }
        out
    }
}

/// The 1024 little-endian counters of a mapped readback.
fn read_bins(data: &[u8]) -> Bins {
    let mut out = [[0u32; 256]; 4];
    for (c, channel) in out.iter_mut().enumerate() {
        for (b, bin) in channel.iter_mut().enumerate() {
            let at = (c * 256 + b) * 4;
            *bin = u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        }
    }
    out
}

/// The CPU reduction this module must match: every `stride`-th pixel of an
/// RGBA8 picture, counted per channel and by luma (`metal-dash`'s
/// `histogram_of` reads an RGB buffer the same way).
pub fn histogram_cpu(rgba: &[u8], stride: u32) -> Bins {
    let mut h = [[0u32; 256]; 4];
    let stride = stride.max(1) as usize;
    for (idx, px) in rgba.chunks_exact(4).enumerate() {
        if idx % stride != 0 {
            continue;
        }
        let (r, g, b) = (px[0], px[1], px[2]);
        h[0][r as usize] += 1;
        h[1][g as usize] += 1;
        h[2][b as usize] += 1;
        let y = (u32::from(r) * 54 + u32::from(g) * 183 + u32::from(b) * 19) >> 8;
        h[3][y.min(255) as usize] += 1;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_compose::GpuCx;

    fn cx() -> GpuCx {
        let g = metal_render::gpu::Gpu::new_headless();
        GpuCx::new(g.device, g.queue)
    }

    /// A picture with every byte value in every channel, so each bin is hit.
    fn picture(w: u32, h: u32) -> Vec<u8> {
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        let mut s: u32 = 0x9e37_79b9;
        for _ in 0..w * h {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            px.extend_from_slice(&[(s >> 24) as u8, (s >> 16) as u8, (s >> 8) as u8, 255]);
        }
        px
    }

    /// The GPU reduction matches the CPU one bin for bin, at the preview's
    /// stride and on a size that is not a multiple of the workgroup.
    #[test]
    fn gpu_histogram_matches_the_cpu_reduction() {
        let cx = cx();
        let mut hist = match Histogrammer::<u32>::new(&cx.device, &cx.queue, true) {
            Some(h) => h,
            None => {
                eprintln!("skipped: this adapter has no compute shaders");
                return;
            }
        };
        let (w, h) = (37u32, 23u32);
        let px = picture(w, h);
        let f = cx.upload(&px, w, h);
        assert!(hist.submit(&f, 4, 7));
        let got = hist.drain();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, 7);
        assert_eq!(got[0].1, histogram_cpu(&px, 4));
        let total: u32 = got[0].1[0].iter().sum();
        assert_eq!(total, (w * h).div_ceil(4));
    }

    /// Rule 11: the cost of a grade's histogram per frame, the CPU way (read
    /// the picture back, reduce it) against the GPU way (one pass, the 4 KiB
    /// result read asynchronously). `cargo test -p metal-perform --features gpu
    /// --release --lib gpu_hist::tests::measure -- --ignored --nocapture`
    #[test]
    #[ignore = "prints the G5 histogram numbers"]
    fn measure() {
        let cx = cx();
        let mut hist = Histogrammer::<u32>::new(&cx.device, &cx.queue, true).expect("compute adapter");
        for (w, h) in [(960u32, 540u32), (1920, 1080)] {
            let px = picture(w, h);
            let f = cx.upload(&px, w, h);
            let n = 20;
            let t = std::time::Instant::now();
            for _ in 0..n {
                let back = cx.readback(&f);
                std::hint::black_box(histogram_cpu(&back, 4));
            }
            let cpu = t.elapsed().as_secs_f64() * 1e3 / n as f64;
            let t = std::time::Instant::now();
            for i in 0..n {
                while !hist.submit(&f, 4, i) {
                    let _ = hist.poll();
                }
            }
            let submit = t.elapsed().as_secs_f64() * 1e3 / n as f64;
            let got = hist.drain();
            let all = t.elapsed().as_secs_f64() * 1e3 / n as f64;
            std::hint::black_box(got.len());
            println!(
                "MEASURE grade histogram {w}x{h}: CPU readback+reduce {cpu:.2} ms/frame; \
                 GPU submit {submit:.3} ms/frame (the window never waits), GPU done {all:.2} ms/frame"
            );
        }
    }
}
