//! P-B — `text.source` on the GPU: the `gpu` painter's override of
//! [`crate::paint::Paint::text`].
//!
//! Layout and the `fx.text.*` effects are [`crate::text::plan`] — the same
//! data the CPU rasterizer ([`crate::text::render`], the `Stand`/wasm path and
//! the reference this is diffed against) draws. Only the pixels move:
//!
//! 1. **glyphs** — every placed glyph is a glyphon custom glyph (position,
//!    size and colour per glyph is what the effects need; glyphon's own
//!    buffer path places glyphs where cosmic-text laid them and nowhere else),
//!    rasterized once by swash into glyphon's atlas and drawn into a
//!    transparent `Rgba16Float` layer. glyphon blends `ALPHA_BLENDING`, so a
//!    pixel of the layer holds `rgb·m, m²` for coverage `m`.
//! 2. **stroke** — a separable max filter (dilate) over the layer's coverage,
//!    two passes into `R16Float`, the CPU's square structuring element.
//! 3. **compose** — one full-frame pass: background, the border box as an SDF
//!    rounded rect, the stroke ring (`dilated − fill`), the glyph layer; each a
//!    straight-alpha source-over, as `text::blend` does per pixel.
//! 4. **readback** into the RGBA the painters hand around.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use cosmic_text::{CacheKey, FontSystem, SwashCache, SwashContent};
use glyphon::{
    Cache, Color, ColorMode, ContentType, CustomGlyph, RasterizeCustomGlyphRequest, RasterizedCustomGlyph,
    Resolution, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};

use crate::text::{plan, to_rgba, TextConfig};

const LAYER: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const COVER: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
const OUT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const SHADER: &str = r#"
struct Dil { r: i32, dx: i32, dy: i32, from_layer: i32 };
struct Comp {
    bg: vec4<f32>,
    rect: vec4<f32>,
    border: vec4<f32>,
    stroke: vec4<f32>,
    // radius, has_border, has_stroke, fill_alpha
    k: vec4<f32>,
};
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> dil: Dil;
@group(1) @binding(0) var layer: texture_2d<f32>;
@group(1) @binding(1) var ring: texture_2d<f32>;
@group(1) @binding(2) var<uniform> comp: Comp;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn cover_at(p: vec2<i32>) -> f32 {
    let s = textureLoad(src, p, 0);
    if (dil.from_layer != 0) { return sqrt(max(s.a, 0.0)); }
    return s.r;
}

@fragment
fn fs_dilate(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(src));
    let p = vec2<i32>(pos.xy);
    let d = vec2<i32>(dil.dx, dil.dy);
    var m = 0.0;
    for (var i = -dil.r; i <= dil.r; i = i + 1) {
        let q = p + d * i;
        if (q.x >= 0 && q.y >= 0 && q.x < size.x && q.y < size.y) {
            m = max(m, cover_at(q));
        }
    }
    return vec4<f32>(m, 0.0, 0.0, 1.0);
}

fn over(dst: vec4<f32>, rgb: vec3<f32>, a: f32) -> vec4<f32> {
    return vec4<f32>(rgb * a + dst.rgb * (1.0 - a), a + dst.a * (1.0 - a));
}

@fragment
fn fs_compose(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    var col = comp.bg;
    if (comp.k.y > 0.5) {
        let r = comp.k.x;
        let c = (comp.rect.xy + comp.rect.zw) * 0.5;
        let hs = (comp.rect.zw - comp.rect.xy) * 0.5 - vec2<f32>(r, r);
        let q = abs(pos.xy - c) - hs;
        let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
        let cov = clamp(0.5 - d, 0.0, 1.0);
        col = over(col, comp.border.rgb, comp.border.a * cov);
    }
    let l = textureLoad(layer, p, 0);
    let m = sqrt(max(l.a, 0.0));
    if (comp.k.z > 0.5) {
        let a = max(textureLoad(ring, p, 0).r - m, 0.0) * comp.stroke.a;
        col = over(col, comp.stroke.rgb, a);
    }
    if (m > 0.0) {
        col = over(col, clamp(l.rgb / m, vec3<f32>(0.0), vec3<f32>(1.0)), clamp(m * comp.k.w, 0.0, 1.0));
    }
    return col;
}
"#;

struct Targets {
    w: u32,
    h: u32,
    layer: wgpu::TextureView,
    d1: wgpu::TextureView,
    d2: wgpu::TextureView,
    out: wgpu::Texture,
    out_view: wgpu::TextureView,
    readback: wgpu::Buffer,
    dil_h: wgpu::BindGroup,
    dil_v: wgpu::BindGroup,
    compose: wgpu::BindGroup,
}

/// Wall-clock split of the last [`TextGpu::render`], for the measure test.
#[derive(Debug, Clone, Copy, Default)]
pub struct Timing {
    pub draw: Duration,
    pub readback: Duration,
}

/// The GPU text painter: one per `paint::Gpu`, on the renderer's own device.
pub struct TextGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    fonts: FontSystem,
    swash: SwashCache,
    atlas: TextAtlas,
    glyphs: TextRenderer,
    viewport: Viewport,
    empty: cosmic_text::Buffer,
    /// Raster key → custom glyph id, and each id's swash image.
    ids: HashMap<(CacheKey, u32), u16>,
    images: Vec<(Vec<u8>, u32, u32, ContentType)>,
    dilate: wgpu::RenderPipeline,
    compose: wgpu::RenderPipeline,
    dil_layout: wgpu::BindGroupLayout,
    comp_layout: wgpu::BindGroupLayout,
    dil_h: wgpu::Buffer,
    dil_v: wgpu::Buffer,
    comp: wgpu::Buffer,
    empty_bind: wgpu::BindGroup,
    targets: Option<Targets>,
    pub last: Timing,
}

fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn uni_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl TextGpu {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> TextGpu {
        let cache = Cache::new(&device);
        let mut atlas = TextAtlas::with_color_mode(&device, &queue, &cache, LAYER, ColorMode::Web);
        let glyphs = TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        let viewport = Viewport::new(&device, &cache);
        let mut fonts = FontSystem::new();
        let empty = cosmic_text::Buffer::new(&mut fonts, cosmic_text::Metrics::new(1.0, 1.0));

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("text_gpu"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let dil_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("text dilate"),
            entries: &[tex_entry(0), uni_entry(1)],
        });
        let comp_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("text compose"),
            entries: &[tex_entry(0), tex_entry(1), uni_entry(2)],
        });
        let empty_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[] });
        let pipe = |label: &str, entry: &str, layouts: &[Option<&wgpu::BindGroupLayout>], format| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: layouts,
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
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
            })
        };
        let dilate = pipe("text dilate", "fs_dilate", &[Some(&dil_layout)], COVER);
        let compose = pipe("text compose", "fs_compose", &[Some(&empty_layout), Some(&comp_layout)], OUT);
        let uniform = |size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("text uniform"),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let (dil_h, dil_v, comp) = (uniform(16), uniform(16), uniform(80));
        let empty_bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &empty_layout, entries: &[] });
        TextGpu {
            device,
            queue,
            fonts,
            swash: SwashCache::new(),
            atlas,
            glyphs,
            viewport,
            empty,
            ids: HashMap::new(),
            images: Vec::new(),
            dilate,
            compose,
            dil_layout,
            comp_layout,
            dil_h,
            dil_v,
            comp,
            empty_bind,
            targets: None,
            last: Timing::default(),
        }
    }

    fn targets(&mut self, w: u32, h: u32) -> &Targets {
        if self.targets.as_ref().is_some_and(|t| t.w == w && t.h == h) {
            return self.targets.as_ref().unwrap();
        }
        let d = &self.device;
        let tex = |label: &str, format, extra: wgpu::TextureUsages| {
            d.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | extra,
                view_formats: &[],
            })
        };
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        let layer = view(&tex("text layer", LAYER, wgpu::TextureUsages::TEXTURE_BINDING));
        let d1 = view(&tex("text d1", COVER, wgpu::TextureUsages::TEXTURE_BINDING));
        let d2 = view(&tex("text d2", COVER, wgpu::TextureUsages::TEXTURE_BINDING));
        let out = tex("text out", OUT, wgpu::TextureUsages::COPY_SRC);
        let out_view = view(&out);
        let readback = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("text readback"),
            size: ((w * 4).next_multiple_of(256) * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let dil = |src: &wgpu::TextureView, u: &wgpu::Buffer| {
            d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.dil_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                    wgpu::BindGroupEntry { binding: 1, resource: u.as_entire_binding() },
                ],
            })
        };
        let dil_h = dil(&layer, &self.dil_h);
        let dil_v = dil(&d1, &self.dil_v);
        let compose = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.comp_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&layer) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&d2) },
                wgpu::BindGroupEntry { binding: 2, resource: self.comp.as_entire_binding() },
            ],
        });
        self.targets = Some(Targets { w, h, layer, d1, d2, out, out_view, readback, dil_h, dil_v, compose });
        self.targets.as_ref().unwrap()
    }

    /// The custom glyph id for a raster key at a scale, rasterizing it with
    /// swash on first sight. `None` for an empty glyph (a space).
    fn glyph_id(&mut self, key: CacheKey, scale: f32) -> Option<(u16, i32, i32, u32, u32)> {
        let image = self.swash.get_image(&mut self.fonts, key).as_ref()?;
        let (left, top) = (image.placement.left, image.placement.top);
        let (gw, gh) = (image.placement.width, image.placement.height);
        if gw == 0 || gh == 0 {
            return None;
        }
        let tag = scale.to_bits();
        if let Some(&id) = self.ids.get(&(key, tag)) {
            let (_, w, h, _) = &self.images[id as usize];
            return Some((id, left, top, *w, *h));
        }
        if self.images.len() >= u16::MAX as usize {
            // An id space this full means a long animated scale: start over.
            self.ids.clear();
            self.images.clear();
        }
        let (content, bpp) = match image.content {
            SwashContent::Mask => (ContentType::Mask, 1usize),
            _ => (ContentType::Color, 4usize),
        };
        let s = scale.clamp(0.05, 8.0);
        let (sw, sh) = (((gw as f32 * s).round() as u32).max(1), ((gh as f32 * s).round() as u32).max(1));
        // Nearest resample to the scaled footprint (identity at scale 1).
        let mut data = vec![0u8; sw as usize * sh as usize * bpp];
        for y in 0..sh as usize {
            let sy = ((y as f32 + 0.5) * gh as f32 / sh as f32) as usize;
            for x in 0..sw as usize {
                let sx = ((x as f32 + 0.5) * gw as f32 / sw as f32) as usize;
                let si = (sy.min(gh as usize - 1) * gw as usize + sx.min(gw as usize - 1)) * bpp;
                let di = (y * sw as usize + x) * bpp;
                if let Some(src) = image.data.get(si..si + bpp) {
                    data[di..di + bpp].copy_from_slice(src);
                }
            }
        }
        let id = self.images.len() as u16;
        self.images.push((data, sw, sh, content));
        self.ids.insert((key, tag), id);
        Some((id, left, top, sw, sh))
    }

    /// `config` on a `w × h` RGBA8 frame — [`crate::text::render`]'s output,
    /// drawn on the GPU.
    pub fn render(&mut self, config: &TextConfig, row: &[f32], time: f32, w: u32, h: u32) -> (Vec<u8>, u32, u32) {
        let (w, h) = (w.max(1), h.max(1));
        let t0 = Instant::now();
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("text") });
        if !self.draw_into(&mut enc, config, row, time, w, h, None) {
            return crate::text::render(config, row, time, w, h);
        }
        let t = self.targets.as_ref().unwrap();
        let bpr = (w * 4).next_multiple_of(256);
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &t.out,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &t.readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(enc.finish()));
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let t1 = Instant::now();

        let t = self.targets.as_ref().unwrap();
        let slice = t.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let ok = matches!(rx.recv(), Ok(Ok(())));
        let mut out = Vec::new();
        if ok {
            let row = (w * 4) as usize;
            out.reserve_exact(row * h as usize);
            if let Ok(data) = slice.get_mapped_range() {
                for y in 0..h as usize {
                    let s = y * bpr as usize;
                    out.extend_from_slice(&data[s..s + row]);
                }
            }
            t.readback.unmap();
        }
        self.atlas.trim();
        self.last = Timing { draw: t1 - t0, readback: t1.elapsed() };
        if out.len() != (w * h * 4) as usize {
            return crate::text::render(config, row, time, w, h);
        }
        (out, w, h)
    }

    /// G2: the text frame drawn straight into `target` (an `Rgba8Unorm` view of
    /// a `w x h` [`crate::gpu_compose::GpuFrame`]), recorded into `enc`: no
    /// readback. `false` when the glyphs could not be prepared (the caller
    /// then takes the CPU frame). Every pixel is written (the background is
    /// the pass's clear).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_into(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        config: &TextConfig,
        row: &[f32],
        time: f32,
        w: u32,
        h: u32,
        target: Option<&wgpu::TextureView>,
    ) -> bool {
        let plan = plan(config, row, time, w, h, &mut self.fonts);

        // Glyphs → custom glyphs, effects applied.
        let mut custom = Vec::new();
        if let Some(plan) = &plan {
            for g in &plan.glyphs {
                let Some((id, left, top, sw, sh)) = self.glyph_id(g.key, g.scale) else { continue };
                let (gw, gh) = {
                    let i = self.swash.get_image(&mut self.fonts, g.key).as_ref().unwrap();
                    (i.placement.width as f32, i.placement.height as f32)
                };
                let (x0, y0) = ((g.gx + left) as f32, (g.gy - top) as f32);
                let (x, y) = if (g.scale - 1.0).abs() < 0.001 {
                    (x0, y0)
                } else {
                    (x0 + gw * 0.5 - sw as f32 * 0.5, y0 + gh * 0.5 - sh as f32 * 0.5)
                };
                let a = (g.fx_alpha * 255.0).round() as u8;
                custom.push(CustomGlyph {
                    id,
                    left: x,
                    top: y,
                    width: sw as f32,
                    height: sh as f32,
                    color: Some(Color::rgba(g.rgb[0], g.rgb[1], g.rgb[2], a)),
                    snap_to_physical_pixel: true,
                    metadata: 0,
                });
            }
        }

        let (stroke_r, stroke_c) = plan.as_ref().and_then(|p| p.stroke).map_or((0, [0; 4]), |(r, c)| (r, c));
        let border = plan.as_ref().and_then(|p| p.border);
        let fill_alpha = plan.as_ref().map_or(0.0, |p| p.fill_alpha);
        let f = |c: [u8; 4]| c.map(|v| v as f32 / 255.0);
        let (rect, bc, radius) = border.map_or(([0.0; 4], [0.0; 4], 0.0), |(x0, y0, x1, y1, r, c)| {
            let r = r.min((x1 - x0) * 0.5).min((y1 - y0) * 0.5).max(0.0);
            ([x0, y0, x1, y1], f(c), r)
        });
        let mut comp = Vec::with_capacity(20);
        comp.extend(f(to_rgba(config.background)));
        comp.extend(rect);
        comp.extend(bc);
        comp.extend(f(stroke_c));
        comp.extend([radius, border.is_some() as u8 as f32, (stroke_r > 0) as u8 as f32, fill_alpha]);
        let words = |v: [i32; 4]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        self.queue.write_buffer(&self.comp, 0, &comp.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>());
        self.queue.write_buffer(&self.dil_h, 0, &words([stroke_r, 1, 0, 1]));
        self.queue.write_buffer(&self.dil_v, 0, &words([stroke_r, 0, 1, 0]));

        self.viewport.update(&self.queue, Resolution { width: w, height: h });
        let images = &self.images;
        let area = TextArea {
            buffer: &self.empty,
            left: 0.0,
            top: 0.0,
            scale: 1.0,
            bounds: TextBounds { left: 0, top: 0, right: w as i32, bottom: h as i32 },
            default_color: Color::rgb(255, 255, 255),
            custom_glyphs: &custom,
        };
        let prepared = self.glyphs.prepare_with_custom(
            &self.device,
            &self.queue,
            &mut self.fonts,
            &mut self.atlas,
            &self.viewport,
            [area],
            &mut self.swash,
            |req: RasterizeCustomGlyphRequest| {
                let (data, iw, ih, content) = images.get(req.id as usize)?;
                (*iw == req.width as u32 && *ih == req.height as u32)
                    .then(|| RasterizedCustomGlyph { data: data.clone(), content_type: *content })
            },
        );
        if let Err(e) = prepared {
            eprintln!("text_gpu: glyph prepare failed ({e}); CPU text this frame");
            return false;
        }

        self.targets(w, h);
        let t = self.targets.as_ref().unwrap();
        let out_view = target.unwrap_or(&t.out_view);
        // Every pass is clipped to what the text can touch: the glyphs' box
        // grown by the stroke, plus the border box. Outside it the frame is
        // the background, which the out pass's clear already is.
        let bbox = {
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for g in &custom {
                x0 = x0.min(g.left);
                y0 = y0.min(g.top);
                x1 = x1.max(g.left + g.width);
                y1 = y1.max(g.top + g.height);
            }
            let grow = stroke_r.max(0) as f32 + 2.0;
            let (mut x0, mut y0, mut x1, mut y1) = (x0 - grow, y0 - grow, x1 + grow, y1 + grow);
            if border.is_some() {
                x0 = x0.min(rect[0] - 1.0);
                y0 = y0.min(rect[1] - 1.0);
                x1 = x1.max(rect[2] + 1.0);
                y1 = y1.max(rect[3] + 1.0);
            }
            let cx = |v: f32, m: u32| (v.floor().max(0.0) as u32).min(m);
            let (bx0, by0, bx1, by1) = (cx(x0, w), cx(y0, h), cx(x1.ceil(), w), cx(y1.ceil(), h));
            (bx1 > bx0 && by1 > by0).then_some((bx0, by0, bx1 - bx0, by1 - by0))
        };
        let bg = to_rgba(config.background).map(|v| v as f64 / 255.0);
        let bg = wgpu::Color { r: bg[0], g: bg[1], b: bg[2], a: bg[3] };
        let pass = |enc: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, clear: wgpu::Color| {
            enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("text"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime()
        };
        {
            let mut p = pass(enc, &t.layer, wgpu::Color::TRANSPARENT);
            if let Some((x, y, bw, bh)) = bbox {
                p.set_scissor_rect(x, y, bw, bh);
            }
            let _ = self.glyphs.render(&self.atlas, &self.viewport, &mut p);
        }
        if stroke_r > 0 && bbox.is_some() {
            let (x, y, bw, bh) = bbox.unwrap();
            for (view, bind) in [(&t.d1, &t.dil_h), (&t.d2, &t.dil_v)] {
                let mut p = pass(enc, view, wgpu::Color::TRANSPARENT);
                p.set_scissor_rect(x, y, bw, bh);
                p.set_pipeline(&self.dilate);
                p.set_bind_group(0, bind, &[]);
                p.draw(0..3, 0..1);
            }
        }
        {
            let mut p = pass(enc, out_view, bg);
            if let Some((x, y, bw, bh)) = bbox {
                p.set_scissor_rect(x, y, bw, bh);
                p.set_pipeline(&self.compose);
                p.set_bind_group(0, &self.empty_bind, &[]);
                p.set_bind_group(1, &t.compose, &[]);
                p.draw(0..3, 0..1);
            }
        }
        self.atlas.trim();
        true
    }
}

/// The text frame as a [`GpuPass`]: no inputs, one frame out.
pub struct TextPass<'a> {
    pub text: &'a mut TextGpu,
    pub config: &'a TextConfig,
    pub row: &'a [f32],
    pub time: f32,
    pub ok: bool,
}

impl crate::gpu_compose::GpuPass for TextPass<'_> {
    fn name(&self) -> &'static str {
        "text"
    }
    fn encode(&mut self, _cx: &crate::gpu_compose::GpuCx, enc: &mut wgpu::CommandEncoder, _inputs: &[&crate::gpu_compose::GpuFrame], output: &crate::gpu_compose::GpuFrame) {
        self.ok = self.text.draw_into(enc, self.config, self.row, self.time, output.width(), output.height(), Some(output.view()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Border, Stroke, TextEffectStep};

    fn painter() -> (TextGpu, crate::gpu_compose::GpuCx) {
        let gpu = metal_render::gpu::Gpu::new_headless();
        let cx = crate::gpu_compose::GpuCx::new(gpu.device.clone(), gpu.queue.clone());
        (TextGpu::new(gpu.device, gpu.queue), cx)
    }

    /// The text frame as a texture (no readback until the test reads it).
    fn texture(
        gpu: &mut TextGpu,
        cx: &crate::gpu_compose::GpuCx,
        cfg: &TextConfig,
        row: &[f32],
        time: f32,
        w: u32,
        h: u32,
    ) -> Option<crate::gpu_compose::GpuFrame> {
        let mut pass = TextPass { text: gpu, config: cfg, row, time, ok: false };
        let f = cx.run(&mut pass, &[], w, h);
        pass.ok.then_some(f)
    }

    /// Mean absolute channel difference and the share of pixels any channel
    /// of which differs by more than 24.
    fn diff(a: &[u8], b: &[u8]) -> (f64, f64) {
        assert_eq!(a.len(), b.len());
        let sum: u64 = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u64).sum();
        let bad = a
            .chunks_exact(4)
            .zip(b.chunks_exact(4))
            .filter(|(p, q)| p.iter().zip(q.iter()).any(|(x, y)| x.abs_diff(*y) > 24))
            .count();
        (sum as f64 / a.len() as f64, bad as f64 / (a.len() / 4) as f64)
    }

    fn cases() -> Vec<(&'static str, TextConfig)> {
        let base = TextConfig { content: "METAL VJ\nlive".into(), ..TextConfig::default() };
        vec![
            ("static", base.clone()),
            (
                "bg+center",
                TextConfig {
                    background: [0.1, 0.2, 0.3, 1.0],
                    color: [1.0, 0.5, 0.2, 1.0],
                    align: crate::text::Align::Center,
                    valign: crate::text::VAlign::Center,
                    ..base.clone()
                },
            ),
            (
                "wave+cycle",
                TextConfig {
                    effects: vec![
                        TextEffectStep { kind: "fx.text.wave".into(), amount: 1.0 },
                        TextEffectStep { kind: "fx.text.color_cycle".into(), amount: 0.5 },
                    ],
                    ..base.clone()
                },
            ),
            (
                "stroke+border",
                TextConfig {
                    stroke: Stroke { width: 0.06, color: [0.0, 0.0, 0.0, 1.0] },
                    border: Border { padding: 0.02, color: [0.1, 0.1, 0.1, 0.8], radius: 0.02 },
                    color: [1.0, 1.0, 0.0, 1.0],
                    ..base.clone()
                },
            ),
        ]
    }

    /// The GPU frame is the CPU reference within a blend-rounding tolerance.
    #[test]
    fn gpu_text_matches_the_cpu_reference() {
        let (mut gpu, cx) = painter();
        for (name, cfg) in cases() {
            let (w, h) = (480, 270);
            let cpu = crate::text::render(&cfg, &[], 0.3, w, h);
            let got = gpu.render(&cfg, &[], 0.3, w, h);
            assert_eq!((got.1, got.2), (w, h));
            // The texture pass (G2) draws the same frame the readback path does.
            let tex = texture(&mut gpu, &cx, &cfg, &[], 0.3, w, h).expect("glyphs prepared");
            let (tmean, _) = diff(&cx.readback(&tex), &got.0);
            assert!(tmean < 0.01, "{name}: texture vs readback path mean {tmean:.3}");
            let (mean, bad) = diff(&cpu.0, &got.0);
            println!("{name}: mean {mean:.3}, off {:.3}%", bad * 100.0);
            assert!(mean < 1.5 && bad < 0.01, "{name}: mean {mean:.3}, off {:.3}%", bad * 100.0);
            // Something was drawn: not a frame of background alone.
            assert!(got.0.chunks_exact(4).filter(|p| p[3] > 50).count() > 100, "{name}: nothing drawn");
        }
    }

    /// `cargo test -p metal-perform --features gpu --release --lib text_gpu::tests::measure -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn measure() {
        let (mut gpu, cx) = painter();
        for (name, cfg) in cases() {
            for (w, h) in [(1920u32, 1080u32), (3840, 2160)] {
                let _ = gpu.render(&cfg, &[], 0.0, w, h);
                let n = 30;
                let t0 = Instant::now();
                for i in 0..n {
                    std::hint::black_box(gpu.render(&cfg, &[], i as f32 / 60.0, w, h));
                }
                let before = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
                let t0 = Instant::now();
                for i in 0..n {
                    std::hint::black_box(texture(&mut gpu, &cx, &cfg, &[], i as f32 / 60.0, w, h));
                }
                let _ = cx.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
                let after = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
                println!("MEASURE text {name} {w}x{h}: before (readback) {before:.2} ms, after (texture) {after:.2} ms");
            }
        }
    }
}
