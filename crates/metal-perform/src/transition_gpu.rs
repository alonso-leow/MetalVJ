//! P-E / G2 — the transition as a [`GpuPass`]: the `gpu` painter's override of
//! [`crate::paint::Paint::transition`], now texture in, texture out.
//!
//! One fragment pass draws every kind but blur (the same per-pixel formulas
//! and the same integer hash as [`crate::transition::apply`], the CPU
//! reference); blur is a separable box (horizontal into a scratch frame from
//! [`GpuCx::alloc`], vertical into the output). The tile never leaves the GPU;
//! [`TransitionGpu::apply`] is the CPU-bytes convenience (upload, pass,
//! readback) for a tile that only exists on the CPU.

use std::time::{Duration, Instant};

use wgpu::util::DeviceExt;

use crate::gpu_compose::{GpuCx, GpuFrame, GpuPass};
use crate::transition::{blur_radius, Dir, Kind, Spec};

const FMT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const SHADER: &str = r#"
struct U { kind: i32, dir: i32, seed: u32, pass_: i32, v: f32, radius: i32, _a: i32, _b: i32 };
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> u: U;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn hash(x: u32, y: u32, seed: u32) -> f32 {
    var h = (x * 0x8da6b343u) ^ (y * 0xd8163841u) ^ (seed * 0xcb1ab31fu);
    h = h ^ (h >> 13u);
    h = h * 0x5bd1e995u;
    h = h ^ (h >> 15u);
    return f32(h & 0xffffu) / 65535.0;
}

fn at(p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(src));
    if (p.x < 0 || p.y < 0 || p.x >= size.x || p.y >= size.y) { return vec4<f32>(0.0); }
    return textureLoad(src, p, 0);
}

fn clamped(p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(src));
    return textureLoad(src, clamp(p, vec2<i32>(0), size - 1), 0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(src));
    let p = vec2<i32>(pos.xy);
    let x = u32(p.x);
    let y = u32(p.y);
    let f = vec2<f32>(p) + 0.5;
    let c = size * 0.5;
    let v = u.v;
    let w = 1.0 - v;
    // Blur, separable: pass 1 horizontal, pass 2 vertical (+ alpha).
    if (u.pass_ > 0) {
        let d = select(vec2<i32>(0, 1), vec2<i32>(1, 0), u.pass_ == 1);
        var acc = vec4<f32>(0.0);
        // Strided taps (at most 9 per pass), as the CPU's downsampled box.
        let step = max(u.radius / 4, 1);
        var n = 0.0;
        for (var i = -u.radius; i <= u.radius; i = i + step) {
            acc = acc + clamped(p + d * i);
            n = n + 1.0;
        }
        var o = acc / n;
        if (u.pass_ == 2) { o.a = o.a * v; }
        return o;
    }
    var px = at(p);
    var alpha = 1.0;
    switch u.kind {
        case 0: { alpha = v; }
        case 1: { if (hash(x, y, 7u) >= v) { alpha = 0.0; } }
        case 2: {
            var t = f.x / size.x;
            if (u.dir == 1) { t = 1.0 - f.x / size.x; }
            if (u.dir == 2) { t = f.y / size.y; }
            if (u.dir == 3) { t = 1.0 - f.y / size.y; }
            if (t > v) { alpha = 0.0; }
        }
        case 3: {
            let lim = v * length(c);
            let d = f - c;
            if (dot(d, d) > lim * lim) { alpha = 0.0; }
        }
        case 4, 5: {
            var s = max(v, 0.001);
            if (u.kind == 5) { s = 1.0 + w * 2.0; }
            px = at(vec2<i32>(floor(c + (f - c) / s)));
            alpha = v;
        }
        case 6: { alpha = v; }
        case 7: {
            var shift = 0;
            if (hash(0u, y / 8u, u.seed) < w) { shift = i32((hash(1u, y / 8u, u.seed) - 0.5) * w * size.x * 0.4); }
            let r = at(vec2<i32>(p.x + shift + i32(w * 12.0), p.y));
            px = at(vec2<i32>(p.x + shift, p.y));
            px.r = r.r;
            if (hash(x / 16u, y / 16u, u.seed) < w * w) { alpha = 0.0; }
        }
        case 8: {
            let b = u32(max(round(1.0 + w * 48.0), 1.0));
            let sz = vec2<u32>(size);
            let bx = min((x / b) * b + b / 2u, sz.x - 1u);
            let by = min((y / b) * b + b / 2u, sz.y - 1u);
            px = at(vec2<i32>(i32(bx), i32(by)));
            alpha = sqrt(v);
        }
        case 9: {
            var d = vec2<f32>(0.0);
            if (u.dir == 0) { d.x = -(w * size.x); }
            if (u.dir == 1) { d.x = w * size.x; }
            if (u.dir == 2) { d.y = -(w * size.y); }
            if (u.dir == 3) { d.y = w * size.y; }
            px = at(vec2<i32>(floor(f - d)));
        }
        default: { px = px + (vec4<f32>(1.0) - px) * w; }
    }
    px.a = px.a * alpha;
    return px;
}
"#;

/// Wall-clock split of the last [`TransitionGpu::apply`], for the measure test.
#[derive(Debug, Clone, Copy, Default)]
pub struct Timing {
    pub upload: Duration,
    pub pass: Duration,
    pub readback: Duration,
}

/// The transition pass: one per `paint::Gpu`, on the shared device. Set the
/// look with [`TransitionGpu::set`], then run it as a [`GpuPass`] over one
/// input frame.
pub struct TransitionGpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    look: Option<(Spec, f32, u32)>,
    pub last: Timing,
}

fn kind_index(k: Kind) -> i32 {
    match k {
        Kind::Fade => 0,
        Kind::Dissolve => 1,
        Kind::Wipe => 2,
        Kind::Iris => 3,
        Kind::ZoomIn => 4,
        Kind::ZoomOut => 5,
        Kind::Blur => 6,
        Kind::Glitch => 7,
        Kind::Pixelate => 8,
        Kind::Slide => 9,
        Kind::Flash => 10,
    }
}

fn dir_index(d: Dir) -> i32 {
    match d {
        Dir::Left => 0,
        Dir::Right => 1,
        Dir::Up => 2,
        Dir::Down => 3,
    }
}

impl TransitionGpu {
    pub fn new(cx: &GpuCx) -> TransitionGpu {
        let device = &cx.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("transition_gpu"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("transition"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("transition"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("transition"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FMT,
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
        TransitionGpu { pipeline, layout, look: None, last: Timing::default() }
    }

    /// What the next [`GpuPass::encode`] draws: `spec` at visibility `v`.
    pub fn set(&mut self, spec: &Spec, v: f32, seed: u32) {
        self.look = Some((spec.clone(), v.clamp(0.0, 1.0), seed));
    }

    /// The transition on a texture tile: allocate, encode, submit.
    pub fn run(&mut self, cx: &GpuCx, spec: &Spec, v: f32, input: &GpuFrame, seed: u32) -> GpuFrame {
        self.set(spec, v, seed);
        cx.run(self, &[input], input.width(), input.height())
    }

    /// [`crate::transition::apply`] on CPU bytes: upload, the pass, readback.
    /// Only for a tile that exists on the CPU (a spanning layer's crop).
    pub fn apply(&mut self, cx: &GpuCx, spec: &Spec, v: f32, frame: &mut [u8], w: u32, h: u32, seed: u32) {
        let v = v.clamp(0.0, 1.0);
        if v >= 1.0 || w == 0 || h == 0 || frame.len() < (w * h * 4) as usize {
            return;
        }
        let t0 = Instant::now();
        let input = cx.upload(frame, w, h);
        let t1 = Instant::now();
        let out = self.run(cx, spec, v, &input, seed);
        let _ = cx.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let t2 = Instant::now();
        let bytes = cx.readback(&out);
        if bytes.len() == (w * h * 4) as usize {
            frame[..bytes.len()].copy_from_slice(&bytes);
        } else {
            crate::transition::apply(spec, v, frame, w, h, seed);
        }
        self.last = Timing { upload: t1 - t0, pass: t2 - t1, readback: t2.elapsed() };
    }
}

impl GpuPass for TransitionGpu {
    fn name(&self) -> &'static str {
        "transition"
    }

    fn encode(&mut self, cx: &GpuCx, enc: &mut wgpu::CommandEncoder, inputs: &[&GpuFrame], output: &GpuFrame) {
        let input = inputs[0];
        let (w, h) = (output.width(), output.height());
        let Some((spec, v, seed)) = self.look.clone() else {
            enc.copy_texture_to_texture(
                input.texture().as_image_copy(),
                output.texture().as_image_copy(),
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            return;
        };
        if v >= 1.0 {
            enc.copy_texture_to_texture(
                input.texture().as_image_copy(),
                output.texture().as_image_copy(),
                wgpu::Extent3d { width: w.min(input.width()), height: h.min(input.height()), depth_or_array_layers: 1 },
            );
            return;
        }
        let radius = blur_radius(v) as i32;
        let blur = spec.kind == Kind::Blur && radius > 0;
        let words = |pass: i32| -> Vec<u8> {
            let mut b = Vec::with_capacity(32);
            b.extend(kind_index(spec.kind).to_le_bytes());
            b.extend(dir_index(spec.dir).to_le_bytes());
            b.extend(seed.to_le_bytes());
            b.extend(pass.to_le_bytes());
            b.extend(v.to_le_bytes());
            b.extend(radius.to_le_bytes());
            b.extend([0u8; 8]);
            b
        };
        let d = &cx.device;
        let bind = |view: &wgpu::TextureView, pass: i32| {
            let u = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("transition uniform"),
                contents: &words(pass),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                    wgpu::BindGroupEntry { binding: 1, resource: u.as_entire_binding() },
                ],
            })
        };
        let draw = |enc: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, bind: &wgpu::BindGroup| {
            let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("transition"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            p.set_pipeline(&self.pipeline);
            p.set_bind_group(0, bind, &[]);
            p.draw(0..3, 0..1);
        };
        if blur {
            let mid = cx.alloc(w, h);
            draw(enc, mid.view(), &bind(input.view(), 1));
            draw(enc, output.view(), &bind(mid.view(), 2));
        } else {
            draw(enc, output.view(), &bind(input.view(), 0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transition::{Ease, Unit};

    fn cx() -> GpuCx {
        let gpu = metal_render::gpu::Gpu::new_headless();
        GpuCx::new(gpu.device, gpu.queue)
    }

    fn spec(kind: Kind) -> Spec {
        Spec { kind, duration: 1.0, unit: Unit::Seconds, ease: Ease::Linear, dir: Dir::Left }
    }

    fn picture(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                [(x * 255 / w) as u8, (y * 255 / h) as u8, ((x + y) % 256) as u8, 255]
            })
            .collect()
    }

    /// Every kind as a texture-to-texture pass is the CPU reference: exact up
    /// to rounding for the point kinds, a close box for blur (the CPU blurs
    /// downsampled).
    #[test]
    fn gpu_transition_matches_the_cpu_reference() {
        let cx = cx();
        let mut gpu = TransitionGpu::new(&cx);
        let (w, h) = (160u32, 90u32);
        let src = picture(w, h);
        let tile = cx.upload(&src, w, h);
        for k in Kind::ALL {
            for v in [0.25f32, 0.6] {
                let mut cpu = src.clone();
                crate::transition::apply(&spec(k), v, &mut cpu, w, h, 5);
                let got = cx.readback(&gpu.run(&cx, &spec(k), v, &tile, 5));
                let sum: u64 = cpu.iter().zip(&got).map(|(a, b)| a.abs_diff(*b) as u64).sum();
                let mean = sum as f64 / cpu.len() as f64;
                let lim = if k == Kind::Blur { 6.0 } else { 1.0 };
                assert!(mean < lim, "{k:?} at {v}: mean diff {mean:.3}");
                assert_ne!(got, src, "{k:?} at {v} changed nothing");
            }
        }
        // The CPU-bytes convenience agrees with the texture path.
        let mut bytes = src.clone();
        gpu.apply(&cx, &spec(Kind::Wipe), 0.4, &mut bytes, w, h, 5);
        assert_eq!(bytes, cx.readback(&gpu.run(&cx, &spec(Kind::Wipe), 0.4, &tile, 5)));
    }

    /// `cargo test -p metal-perform --features gpu --release --lib transition_gpu::tests::measure -- --ignored --nocapture`
    ///
    /// Before = bytes in, bytes out (`apply`: upload + pass + readback, what
    /// every transition cost before G2); after = a texture tile through the
    /// pass, GPU work waited, nothing read back.
    #[test]
    #[ignore]
    fn measure() {
        let cx = cx();
        let mut gpu = TransitionGpu::new(&cx);
        for (w, h) in [(1920u32, 1080u32), (3840, 2160)] {
            let src = picture(w, h);
            let tile = cx.upload(&src, w, h);
            for k in [Kind::Fade, Kind::Wipe, Kind::Blur, Kind::Glitch] {
                let n = 20;
                let mut f = src.clone();
                gpu.apply(&cx, &spec(k), 0.5, &mut f, w, h, 0);
                let t0 = Instant::now();
                for i in 0..n {
                    let mut f = src.clone();
                    gpu.apply(&cx, &spec(k), 0.5, &mut f, w, h, i);
                }
                let before = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
                let t0 = Instant::now();
                for i in 0..n {
                    let _ = gpu.run(&cx, &spec(k), 0.5, &tile, i);
                }
                let _ = cx.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
                let after = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
                println!("MEASURE transition {k:?} {w}x{h}: before (bytes) {before:.2} ms, after (texture) {after:.2} ms");
            }
        }
    }
}
