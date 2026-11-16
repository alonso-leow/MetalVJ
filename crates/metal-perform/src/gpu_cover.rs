//! G3 — coverage maps and whole-frame shade passes.
//!
//! A contour, a mask cut, a stroke band or an overlay's per-mask colour is
//! geometry, not pixels: the CPU walks the polygon (the one [`Scanline`]
//! the CPU compose uses, so edges agree to the pixel) into a single-channel
//! map, which uploads as an `R8Unorm` texture and is cached while the masks do
//! not change. The frame itself never leaves the GPU: [`ShadePass`] multiplies
//! it by a map ([`Shade::Cut`]), tints it through one ([`Shade::Tint`]) or lays
//! a painted RGBA layer over it ([`Shade::Over`]), and `gpu_compose` reads the
//! same maps as per-tile contour clips.
//!
//! [`Scanline`]: crate::output::Scanline

use std::collections::HashMap;
use std::sync::Arc;

use crate::gpu_compose::{GpuCx, GpuFrame, GpuPass, VIEW_FORMAT};
use crate::gpu_frame::Shade;

/// A device-sized `R8Unorm` coverage map.
pub struct CoverTex {
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    w: u32,
    h: u32,
}

/// A shared handle to a [`CoverTex`] (the cache keeps one, tiles borrow it).
pub type CoverRef = Arc<CoverTex>;

impl CoverTex {
    /// `bytes` is `w * h` coverage values, row-major.
    pub fn new(cx: &GpuCx, bytes: &[u8], w: u32, h: u32) -> CoverRef {
        let (w, h) = (w.max(1), h.max(1));
        let tex = cx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gpu cover"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let n = (w as usize) * (h as usize);
        let mut padded;
        let data = if bytes.len() >= n {
            &bytes[..n]
        } else {
            padded = bytes.to_vec();
            padded.resize(n, 0);
            &padded[..]
        };
        cx.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        Arc::new(CoverTex { tex, view, w, h })
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
    pub fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }
    pub fn texture(&self) -> &wgpu::Texture {
        &self.tex
    }
}

/// Cached coverage maps keyed by what they were built from. A handful live at
/// a time (a device's clips and cuts); past the cap the lot is dropped and
/// rebuilt on demand, which costs one frame of polygon walking.
#[derive(Default)]
pub struct CoverCache {
    maps: HashMap<u64, CoverRef>,
}

const CACHE_CAP: usize = 24;

impl CoverCache {
    pub fn get_or_build(&mut self, cx: &GpuCx, key: u64, w: u32, h: u32, make: &mut dyn FnMut() -> Vec<u8>) -> CoverRef {
        if let Some(hit) = self.maps.get(&key) {
            if hit.size() == (w.max(1), h.max(1)) {
                return hit.clone();
            }
        }
        if self.maps.len() >= CACHE_CAP {
            self.maps.clear();
        }
        let built = CoverTex::new(cx, &make(), w, h);
        self.maps.insert(key, built.clone());
        built
    }
}

const SHADER: &str = r#"
struct U {
    k: vec4<f32>,      // colour rgb, k
    mode: vec4<i32>,   // mode (0 cut, 1 tint, 2 over), has cover
};
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var aux: texture_2d<f32>;
@group(0) @binding(2) var<uniform> u: U;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let d = vec2<i32>(pos.xy);
    let s = textureLoad(src, d, 0);
    var rgb = s.rgb;
    if (u.mode.x == 0) {
        rgb = s.rgb * textureLoad(aux, d, 0).r;
    } else if (u.mode.x == 1) {
        var k = u.k.w;
        if (u.mode.y != 0) { k = k * textureLoad(aux, d, 0).r; }
        rgb = mix(s.rgb, u.k.rgb, clamp(k, 0.0, 1.0));
    } else {
        let o = textureLoad(aux, d, 0);
        rgb = mix(s.rgb, o.rgb, o.a);
    }
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), s.a);
}
"#;

/// The shade pipeline, built on first use.
pub struct ShadePass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    dummy: CoverRef,
}

struct Job<'a> {
    pass: &'a ShadePass,
    shade: &'a Shade<'a>,
}

impl ShadePass {
    pub fn new(cx: &GpuCx) -> ShadePass {
        let device = &cx.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gpu_shade"),
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
            label: Some("gpu_shade"),
            entries: &[
                tex(0),
                tex(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(32),
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpu_shade"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gpu_shade"),
            layout: Some(&pl),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"), buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
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
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_shade uniforms"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ShadePass { pipeline, layout, uniforms, dummy: CoverTex::new(cx, &[255], 1, 1) }
    }

    /// `frame` shaded into a fresh frame of its size.
    pub fn apply(&mut self, cx: &GpuCx, frame: &GpuFrame, shade: &Shade) -> GpuFrame {
        let mut job = Job { pass: self, shade };
        cx.run(&mut job, &[frame], frame.width(), frame.height())
    }
}

impl GpuPass for Job<'_> {
    fn name(&self) -> &'static str {
        "shade"
    }

    fn encode(&mut self, cx: &GpuCx, enc: &mut wgpu::CommandEncoder, inputs: &[&GpuFrame], output: &GpuFrame) {
        let p = self.pass;
        let (mode, has_cover, k, aux_cover, aux_frame): (i32, i32, [f32; 4], Option<&CoverRef>, Option<GpuFrame>) = match self.shade {
            Shade::Cut(c) => (0, 1, [0.0; 4], Some(*c), None),
            Shade::Tint { rgb, k, cover } => (1, cover.is_some() as i32, [rgb[0], rgb[1], rgb[2], *k], *cover, None),
            Shade::Over { rgba } => {
                let (w, h) = (output.width(), output.height());
                (2, 0, [0.0; 4], None, Some(cx.upload(rgba, w, h)))
            }
        };
        let mut u = Vec::with_capacity(32);
        for v in k {
            u.extend(v.to_le_bytes());
        }
        for v in [mode, has_cover, 0, 0] {
            u.extend(v.to_le_bytes());
        }
        cx.queue.write_buffer(&p.uniforms, 0, &u);
        let aux_view = match (&aux_frame, aux_cover) {
            (Some(f), _) => f.view(),
            (None, Some(c)) => c.view(),
            (None, None) => p.dummy.view(),
        };
        let bind = cx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &p.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(inputs[0].view()) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(aux_view) },
                wgpu::BindGroupEntry { binding: 2, resource: p.uniforms.as_entire_binding() },
            ],
        });
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gpu_shade"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output.view(),
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&p.pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
        drop(pass);
        // `aux_frame` (an uploaded layer) must outlive the recorded commands:
        // the pool keeps a dropped frame's texture alive for ordering, as the
        // other passes rely on.
        drop(aux_frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx() -> GpuCx {
        let g = metal_render::gpu::Gpu::new_headless();
        GpuCx::new(g.device, g.queue)
    }

    fn picture(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                [(x * 255 / w) as u8, (y * 255 / h) as u8, ((x * 7 + y * 3) % 256) as u8, 255]
            })
            .collect()
    }

    /// The shade shader against the same arithmetic on the CPU, byte for byte
    /// within one step of rounding: cut, tint with and without a cover, and
    /// the RGBA layer over.
    #[test]
    fn shade_matches_the_cpu_reference() {
        let (w, h) = (64u32, 36u32);
        let cx = cx();
        let px = picture(w, h);
        let cover: Vec<u8> = (0..w * h).map(|i| ((i * 37) % 256) as u8).collect();
        let layer: Vec<u8> = (0..w * h).flat_map(|i| [200u8, 40, 90, ((i * 11) % 256) as u8]).collect();
        let src = cx.upload(&px, w, h);
        let cov = CoverTex::new(&cx, &cover, w, h);
        let mut pass = ShadePass::new(&cx);
        let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let to8 = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;

        let got = cx.readback(&pass.apply(&cx, &src, &Shade::Cut(&cov)));
        let want: Vec<u8> = px
            .chunks_exact(4)
            .zip(&cover)
            .flat_map(|(p, c)| {
                let t = *c as f32 / 255.0;
                [to8(p[0] as f32 / 255.0 * t), to8(p[1] as f32 / 255.0 * t), to8(p[2] as f32 / 255.0 * t), p[3]]
            })
            .collect();
        assert_eq!(got.len(), want.len());
        let worst = got.iter().zip(&want).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(worst <= 1, "cut: max diff {worst}");

        let rgb = [1.0f32, 0.25, 0.0];
        let k = 0.5f32;
        let got = cx.readback(&pass.apply(&cx, &src, &Shade::Tint { rgb, k, cover: Some(&cov) }));
        let want: Vec<u8> = px
            .chunks_exact(4)
            .zip(&cover)
            .flat_map(|(p, c)| {
                let t = (k * *c as f32 / 255.0).clamp(0.0, 1.0);
                let ch = |i: usize| to8(mix(p[i] as f32 / 255.0, rgb[i], t));
                [ch(0), ch(1), ch(2), p[3]]
            })
            .collect();
        let worst = got.iter().zip(&want).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(worst <= 1, "tint with cover: max diff {worst}");

        let got = cx.readback(&pass.apply(&cx, &src, &Shade::Tint { rgb, k, cover: None }));
        let want: Vec<u8> = px
            .chunks_exact(4)
            .flat_map(|p| {
                let ch = |i: usize| to8(mix(p[i] as f32 / 255.0, rgb[i], k));
                [ch(0), ch(1), ch(2), p[3]]
            })
            .collect();
        let worst = got.iter().zip(&want).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(worst <= 1, "tint whole frame: max diff {worst}");

        let got = cx.readback(&pass.apply(&cx, &src, &Shade::Over { rgba: &layer }));
        let want: Vec<u8> = px
            .chunks_exact(4)
            .zip(layer.chunks_exact(4))
            .flat_map(|(p, o)| {
                let a = o[3] as f32 / 255.0;
                let ch = |i: usize| to8(mix(p[i] as f32 / 255.0, o[i] as f32 / 255.0, a));
                [ch(0), ch(1), ch(2), p[3]]
            })
            .collect();
        let worst = got.iter().zip(&want).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(worst <= 1, "over: max diff {worst}");
    }
}
