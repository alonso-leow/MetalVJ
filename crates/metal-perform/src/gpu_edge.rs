//! G3 — edge blend and the compose-size upscale as [`GpuPass`]es.
//!
//! [`EdgeBlendPass`] is [`crate::edge_blend::BlendMap::apply`] in a shader: the
//! map's `(gain, lift)` goes up once as an `Rgba32Float` texture (read with
//! `textureLoad`, so no filtering feature is needed on a downlevel adapter) and
//! `out = lift + in * gain * (1 - lift)` runs on the encoded bytes, alpha
//! untouched. [`ScalePass`] is the nearest-neighbour upscale a clamped compose
//! size needs to reach the device size without a CPU round trip.

use std::sync::Arc;

use crate::edge_blend::BlendMap;
use crate::gpu_compose::{GpuCx, GpuFrame, GpuPass, VIEW_FORMAT};

const SHADER: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var aux: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_blend(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let d = vec2<i32>(pos.xy);
    let c = textureLoad(src, d, 0);
    let m = textureLoad(aux, d, 0);
    let rgb = clamp(vec3<f32>(m.y) + c.rgb * m.x * (1.0 - m.y), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb, c.a);
}

@fragment
fn fs_scale(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let sd = vec2<f32>(textureDimensions(src));
    let od = textureLoad(aux, vec2<i32>(0, 0), 0).xy;
    let s = min(vec2<i32>(pos.xy * sd / od), vec2<i32>(sd) - vec2<i32>(1));
    return textureLoad(src, s, 0);
}
"#;

fn pipeline(cx: &GpuCx, fs: &str) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let device = &cx.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("gpu_edge"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let tex = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("gpu_edge"),
        entries: &[tex(0), tex(1)],
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("gpu_edge"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("gpu_edge"),
        layout: Some(&pl),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some(fs),
            targets: &[Some(wgpu::ColorTargetState {
                format: VIEW_FORMAT,
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
    (p, layout)
}

fn draw(
    cx: &GpuCx,
    enc: &mut wgpu::CommandEncoder,
    p: &wgpu::RenderPipeline,
    layout: &wgpu::BindGroupLayout,
    a: &wgpu::TextureView,
    b: &wgpu::TextureView,
    out: &GpuFrame,
) {
    let bg = cx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("gpu_edge"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(a) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(b) },
        ],
    });
    let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("gpu_edge"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: out.view(),
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    rp.set_pipeline(p);
    rp.set_bind_group(0, &bg, &[]);
    rp.draw(0..3, 0..1);
}

/// One device's edge blend. The map texture is uploaded on construction.
pub struct EdgeBlendPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    map: wgpu::TextureView,
    pub size: (u32, u32),
    /// Keeps the source map alive so its address stays a unique cache key.
    pub source: Arc<BlendMap>,
}

impl EdgeBlendPass {
    pub fn new(cx: &GpuCx, map: &Arc<BlendMap>) -> EdgeBlendPass {
        let (pipeline, layout) = pipeline(cx, "fs_blend");
        let (w, h) = (map.width.max(1), map.height.max(1));
        let mut texels: Vec<f32> = Vec::with_capacity((w * h * 4) as usize);
        for i in 0..(w * h) as usize {
            texels.extend_from_slice(&[
                map.gain.get(i).copied().unwrap_or(1.0),
                map.lift.get(i).copied().unwrap_or(0.0),
                0.0,
                0.0,
            ]);
        }
        let tex = cx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("edge blend map"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        cx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck_cast(&texels),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 16), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        EdgeBlendPass { pipeline, layout, map: tex.create_view(&Default::default()), size: (w, h), source: map.clone() }
    }
}

fn bytemuck_cast(v: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding and u8 has alignment 1.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

impl GpuPass for EdgeBlendPass {
    fn name(&self) -> &'static str {
        "edge_blend"
    }
    fn encode(&mut self, cx: &GpuCx, enc: &mut wgpu::CommandEncoder, inputs: &[&GpuFrame], output: &GpuFrame) {
        draw(cx, enc, &self.pipeline, &self.layout, inputs[0].view(), &self.map, output);
    }
}

/// Nearest-neighbour resize of `inputs[0]` to the output's size.
pub struct ScalePass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl ScalePass {
    pub fn new(cx: &GpuCx) -> ScalePass {
        let (pipeline, layout) = pipeline(cx, "fs_scale");
        ScalePass { pipeline, layout }
    }
}

impl GpuPass for ScalePass {
    fn name(&self) -> &'static str {
        "scale"
    }
    fn encode(&mut self, cx: &GpuCx, enc: &mut wgpu::CommandEncoder, inputs: &[&GpuFrame], output: &GpuFrame) {
        // The output size rides in a 1x1 float texture (the output itself
        // cannot be bound while it is the render target).
        let tex = cx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scale dims"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let dims = [output.width() as f32, output.height() as f32, 0.0, 0.0];
        cx.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytemuck_cast(&dims),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(16), rows_per_image: Some(1) },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        draw(cx, enc, &self.pipeline, &self.layout, inputs[0].view(), &tex.create_view(&Default::default()), output);
    }
}

/// The largest compose size `(w, h)` that fits `max_dim` per side and a memory
/// budget of `budget_bytes` for a working set of `FRAMES` RGBA8 frames, keeping
/// the aspect ratio. Returns the input unchanged when it already fits.
pub fn fit_size(w: u32, h: u32, max_dim: u32, budget_bytes: Option<u64>) -> (u32, u32) {
    const FRAMES: f64 = 24.0;
    let (w, h) = (w.max(1), h.max(1));
    let mut k = 1.0_f64;
    let m = max_dim.max(1) as f64;
    k = k.min(m / w as f64).min(m / h as f64);
    if let Some(b) = budget_bytes {
        let need = w as f64 * h as f64 * 4.0 * FRAMES;
        if need > b as f64 {
            k = k.min((b as f64 / need).sqrt());
        }
    }
    if k >= 1.0 {
        return (w, h);
    }
    (((w as f64 * k) as u32).max(1), ((h as f64 * k) as u32).max(1))
}

/// A quarter of the memory the OS reports available (Linux), cached for 2 s;
/// `None` where it cannot be read, which means "no pressure".
pub fn memory_budget() -> Option<u64> {
    use std::sync::Mutex;
    static CACHE: Mutex<Option<(web_time::Instant, Option<u64>)>> = Mutex::new(None);
    let mut c = CACHE.lock().ok()?;
    if let Some((t, v)) = *c {
        if t.elapsed().as_secs() < 2 {
            return v;
        }
    }
    #[cfg(target_os = "linux")]
    let v = std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| {
        let kb: u64 = s.lines().find(|l| l.starts_with("MemAvailable:"))?.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024 / 4)
    });
    #[cfg(not(target_os = "linux"))]
    let v = None;
    *c = Some((web_time::Instant::now(), v));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_compose::tests::picture;

    fn cx() -> GpuCx {
        let g = metal_render::gpu::Gpu::new_headless();
        GpuCx::new(g.device, g.queue)
    }

    #[test]
    fn fit_size_clamps_to_the_limit_and_the_budget() {
        assert_eq!(fit_size(1920, 1080, 8192, None), (1920, 1080));
        assert_eq!(fit_size(3840, 2160, 2048, None), (2048, 1152));
        let (w, h) = fit_size(3840, 2160, 16384, Some(100_000_000));
        assert!(w < 3840 && (w as f64 / h as f64 - 16.0 / 9.0).abs() < 0.01);
        assert!((w as u64) * (h as u64) * 4 * 24 <= 100_000_000 + 1_000_000);
    }

    #[test]
    fn gpu_edge_blend_matches_the_cpu_map() {
        let (w, h) = (96u32, 54u32);
        let n = (w * h) as usize;
        let map = Arc::new(BlendMap {
            width: w,
            height: h,
            gain: (0..n).map(|i| if i % 7 == 0 { 1.0 } else { ((i % w as usize) as f32 / w as f32).max(0.05) }).collect(),
            lift: (0..n).map(|i| if i % 5 == 0 { 0.08 } else { 0.0 }).collect(),
        });
        let px = picture(w, h, 3);
        let mut want = px.clone();
        map.apply(&mut want);
        let cx = cx();
        let src = cx.upload(&px, w, h);
        let got = cx.readback(&cx.run(&mut EdgeBlendPass::new(&cx, &map), &[&src], w, h));
        let max = got.iter().zip(&want).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(max <= 1, "max diff {max}");
    }

    #[test]
    fn gpu_scale_upscales_nearest() {
        let cx = cx();
        let px = picture(8, 8, 1);
        let src = cx.upload(&px, 8, 8);
        let got = cx.readback(&cx.run(&mut ScalePass::new(&cx), &[&src], 16, 16));
        for y in 0..16usize {
            for x in 0..16usize {
                assert_eq!(&got[(y * 16 + x) * 4..][..4], &px[((y / 2) * 8 + x / 2) * 4..][..4]);
            }
        }
    }

    #[test]
    #[ignore]
    fn measure_edge_blend() {
        for (w, h) in [(1920u32, 1080u32), (3840, 2160)] {
            let n = (w * h) as usize;
            let map = Arc::new(BlendMap { width: w, height: h, gain: vec![0.7; n], lift: vec![0.02; n] });
            let px = picture(w, h, 1);
            let t = std::time::Instant::now();
            let mut c = px.clone();
            map.apply(&mut c);
            let cpu = t.elapsed();
            let cx = cx();
            let src = cx.upload(&px, w, h);
            let mut pass = EdgeBlendPass::new(&cx, &map);
            let _ = cx.run(&mut pass, &[&src], w, h);
            let _ = cx.device.poll(wgpu::PollType::wait_indefinitely());
            let t = std::time::Instant::now();
            for _ in 0..10 {
                let _ = cx.run(&mut pass, &[&src], w, h);
            }
            let _ = cx.device.poll(wgpu::PollType::wait_indefinitely());
            println!("{w}x{h}: cpu apply {:?}, gpu pass {:?}", cpu, t.elapsed() / 10);
        }
    }
}
