//! G2 — the image-fx nodes as [`GpuPass`]es over [`GpuFrame`]s, so a chain of
//! them keeps the picture a texture from the generator to the compositor.
//!
//! * [`ForgeFxPass`] — the forge effect contract (`data/shaders/<id>.wgsl`:
//!   chroma key, level, math, projection, and every `fx.forge`/`fx.move`/
//!   `fx.particles` shader) over a texture, through
//!   [`metal_render::headless::Renderer::forge_fx_encode`].
//! * [`LookupPass`] — `fx.lookup`: a palette gradient or a `.cube` 3D LUT.
//! * [`RampPass`] — the `gen.ramp` source.
//!
//! The CPU code in [`crate::fx::parity`] is the reference each is diffed
//! against (display-space byte maths; the built-in shaders therefore sample and
//! write through the frame's `Rgba8Unorm` view, as the contract says).

use std::collections::HashMap;
use std::sync::Arc;

use metal_graph::parity as p;

use crate::fx::image_pop::Atlas as ImageAtlas;
use crate::fx::parity::{self, Cube};
use crate::gpu_compose::{GpuCx, GpuFrame, GpuPass, FRAME_FORMAT, VIEW_FORMAT};

/// Most colour stops a lookup or ramp holds: they live in a `n x 1` float
/// texture (no storage buffer, so GL ES / WebGL2 work), bounded by the
/// adapter's 2D size, 2048 on the smallest.
const MAX_STOPS: usize = 2048;

/// A forge effect shader over one input frame.
pub struct ForgeFxPass<'a> {
    pub renderer: &'a mut metal_render::headless::Renderer,
    pub key: &'a str,
    pub wgsl: &'a str,
    pub time: f32,
    pub ingest: &'a [f32],
    /// `true`: sample and write through the frame's sRGB view (the legacy
    /// `fx.forge` behaviour, shaders work in linear light); `false`: the
    /// `Rgba8Unorm` view (display-space maths, what the built-in P-A shaders and
    /// their CPU reference do).
    pub srgb_views: bool,
    /// Why the shader could not draw, when it could not (output undefined).
    pub error: Option<String>,
}

impl GpuPass for ForgeFxPass<'_> {
    fn name(&self) -> &'static str {
        "forge_fx"
    }

    fn encode(&mut self, _cx: &GpuCx, enc: &mut wgpu::CommandEncoder, inputs: &[&GpuFrame], output: &GpuFrame) {
        let input = inputs[0];
        let aspect = output.width() as f32 / output.height().max(1) as f32;
        let r = if self.srgb_views {
            let (i, o) = (input.texture().create_view(&Default::default()), output.texture().create_view(&Default::default()));
            self.renderer.forge_fx_encode(enc, FRAME_FORMAT, self.key, self.wgsl, &i, &o, aspect, self.time, self.ingest)
        } else {
            self.renderer.forge_fx_encode(
                enc,
                VIEW_FORMAT,
                self.key,
                self.wgsl,
                input.view(),
                output.view(),
                aspect,
                self.time,
                self.ingest,
            )
        };
        self.error = r.err();
    }
}

const VS: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const GRADIENT: &str = r#"
fn stop(i: i32) -> vec3<f32> { return textureLoad(stops, vec2<i32>(i, 0), 0).xyz; }
fn gradient(t: f32, n: i32) -> vec3<f32> {
    if (n <= 0) { return vec3<f32>(t); }
    if (n == 1) { return stop(0); }
    let x = clamp(t, 0.0, 1.0) * f32(n - 1);
    let i = min(i32(floor(x)), n - 2);
    let f = x - f32(i);
    return mix(stop(i), stop(i + 1), f);
}
"#;

fn lookup_shader() -> String {
    format!(
        r#"
struct U {{
    a: vec4<f32>,      // mode (0 palette, 1 cube), channel, mix, stop count
    dmin: vec4<f32>,   // cube domain min, cube size
    dmax: vec4<f32>,   // cube domain max
}};
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> u: U;
@group(0) @binding(2) var cube: texture_3d<f32>;
@group(0) @binding(3) var stops: texture_2d<f32>;
{VS}
{}
fn luma(c: vec3<f32>) -> f32 {{ return 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b; }}
fn cube_at(p: vec3<i32>) -> vec3<f32> {{ return textureLoad(cube, p, 0).rgb; }}
fn cube_apply(c: vec3<f32>) -> vec3<f32> {{
    let n = i32(u.dmin.w);
    let g = clamp((c - u.dmin.xyz) / max(u.dmax.xyz - u.dmin.xyz, vec3<f32>(1e-6)), vec3<f32>(0.0), vec3<f32>(1.0)) * f32(n - 1);
    let i0 = vec3<i32>(floor(g));
    let i1 = min(i0 + vec3<i32>(1), vec3<i32>(n - 1));
    let f = g - vec3<f32>(i0);
    let c00 = mix(cube_at(vec3<i32>(i0.x, i0.y, i0.z)), cube_at(vec3<i32>(i1.x, i0.y, i0.z)), f.x);
    let c10 = mix(cube_at(vec3<i32>(i0.x, i1.y, i0.z)), cube_at(vec3<i32>(i1.x, i1.y, i0.z)), f.x);
    let c01 = mix(cube_at(vec3<i32>(i0.x, i0.y, i1.z)), cube_at(vec3<i32>(i1.x, i0.y, i1.z)), f.x);
    let c11 = mix(cube_at(vec3<i32>(i0.x, i1.y, i1.z)), cube_at(vec3<i32>(i1.x, i1.y, i1.z)), f.x);
    return mix(mix(c00, c10, f.y), mix(c01, c11, f.y), f.z);
}}
@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {{
    let c = textureLoad(src, vec2<i32>(pos.xy), 0);
    var m = vec3<f32>(0.0);
    if (u.a.x > 0.5) {{
        m = cube_apply(c.rgb);
    }} else {{
        let ch = i32(u.a.y);
        var t = luma(c.rgb);
        if (ch > 0) {{ t = c[min(ch - 1, 3)]; }}
        m = gradient(t, i32(u.a.w));
    }}
    return vec4<f32>(c.rgb + (m - c.rgb) * u.a.z, c.a);
}}
"#,
        GRADIENT
    )
}

fn ramp_shader() -> String {
    format!(
        r#"
struct U {{
    a: vec4<f32>,   // shape, angle (deg), centre x, centre y
    b: vec4<f32>,   // phase, repeat, aspect, stop count
    c: vec4<f32>,   // width, height
}};
@group(0) @binding(1) var<uniform> u: U;
@group(0) @binding(2) var stops: texture_2d<f32>;
{VS}
{}
const TAU: f32 = 6.283185307;
fn ramp_t(shape: i32, uv: vec2<f32>) -> f32 {{
    let d = vec2<f32>((uv.x - u.a.z) * u.b.z, uv.y - u.a.w);
    let a = radians(u.a.y);
    if (shape == 0) {{
        let s = sin(a);
        let c = cos(a);
        let ext = max(u.b.z * 0.5 * abs(c) + 0.5 * abs(s), 1e-4);
        return (d.x * c + d.y * s) / (2.0 * ext) + 0.5;
    }}
    if (shape == 1) {{ return length(d) * 2.0; }}
    if (shape == 2) {{
        let x = atan2(d.y, d.x) - a;
        return (x - TAU * floor(x / TAU)) / TAU;
    }}
    return (abs(d.x) + abs(d.y)) * 2.0;
}}
@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {{
    let uv = pos.xy / u.c.xy;
    var t = ramp_t(i32(round(u.a.x)), uv) * u.b.y + u.b.x;
    if (!(t >= 0.0 && t <= 1.0 && u.b.y <= 1.0)) {{ t = t - floor(t); }}
    return vec4<f32>(gradient(t, i32(u.b.w)), 1.0);
}}
"#,
        GRADIENT
    )
}

fn pipeline(
    cx: &GpuCx,
    label: &str,
    wgsl: &str,
    entries: &[wgpu::BindGroupLayoutEntry],
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let d = &cx.device;
    let module = d.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    });
    let layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries });
    let pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipe = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
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
    (pipe, layout)
}

fn entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty, count: None }
}

fn tex2d(binding: u32) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
    )
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
    )
}

fn bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn draw(enc: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, pipe: &wgpu::RenderPipeline, bind: &wgpu::BindGroup) {
    let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("gpu_fx"),
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
    p.set_pipeline(pipe);
    p.set_bind_group(0, bind, &[]);
    p.draw(0..3, 0..1);
}

/// The stops as an `n x 1` `Rgba32Float` texture (a dummy texel for none).
fn stops_view(cx: &GpuCx, stops: &[[f32; 3]]) -> wgpu::TextureView {
    let n = stops.len().clamp(1, MAX_STOPS);
    let t = cx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("stops"),
        size: wgpu::Extent3d { width: n as u32, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut flat: Vec<f32> = stops.iter().take(n).flat_map(|s| [s[0], s[1], s[2], 1.0]).collect();
    flat.resize(n * 4, 0.0);
    cx.queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &bytes(&flat),
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(n as u32 * 16), rows_per_image: Some(1) },
        wgpu::Extent3d { width: n as u32, height: 1, depth_or_array_layers: 1 },
    );
    t.create_view(&Default::default())
}

/// `fx.lookup`: a palette gradient (a channel through the stops) or a `.cube`.
pub struct LookupPass {
    pipe: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    dummy: wgpu::TextureView,
    uniform: Vec<f32>,
    cube: Option<wgpu::TextureView>,
    stops: Option<wgpu::TextureView>,
}

impl LookupPass {
    fn new(cx: &GpuCx) -> LookupPass {
        let entries = [
            tex2d(0),
            uniform_entry(1),
            entry(
                2,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                },
            ),
            tex2d(3),
        ];
        let (pipe, layout) = pipeline(cx, "fx.lookup", &lookup_shader(), &entries);
        let dummy = cube_texture(cx, 1, &[[0.0; 3]]).create_view(&Default::default());
        LookupPass { pipe, layout, dummy, uniform: Vec::new(), cube: None, stops: None }
    }
}

fn cube_texture(cx: &GpuCx, n: u32, data: &[[f32; 3]]) -> wgpu::Texture {
    let t = cx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("lut"),
        size: wgpu::Extent3d { width: n, height: n, depth_or_array_layers: n },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let flat: Vec<f32> = data.iter().flat_map(|c| [c[0], c[1], c[2], 1.0]).collect();
    cx.queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &bytes(&flat),
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(n * 16), rows_per_image: Some(n) },
        wgpu::Extent3d { width: n, height: n, depth_or_array_layers: n },
    );
    t
}

impl GpuPass for LookupPass {
    fn name(&self) -> &'static str {
        "lookup"
    }

    fn encode(&mut self, cx: &GpuCx, enc: &mut wgpu::CommandEncoder, inputs: &[&GpuFrame], output: &GpuFrame) {
        use wgpu::util::DeviceExt;
        let stops = self.stops.take().unwrap_or_else(|| stops_view(cx, &[]));
        let u = cx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("lookup"),
            contents: &bytes(&self.uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind = cx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(inputs[0].view()) },
                wgpu::BindGroupEntry { binding: 1, resource: u.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(self.cube.as_ref().unwrap_or(&self.dummy)),
                },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&stops) },
            ],
        });
        draw(enc, output.view(), &self.pipe, &bind);
    }
}

/// `gen.ramp`: the gradient source, no input.
pub struct RampPass {
    pipe: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: Vec<f32>,
    stops: Option<wgpu::TextureView>,
}

impl RampPass {
    fn new(cx: &GpuCx) -> RampPass {
        let (pipe, layout) = pipeline(cx, "gen.ramp", &ramp_shader(), &[uniform_entry(1), tex2d(2)]);
        RampPass { pipe, layout, uniform: Vec::new(), stops: None }
    }
}

impl GpuPass for RampPass {
    fn name(&self) -> &'static str {
        "ramp"
    }

    fn encode(&mut self, cx: &GpuCx, enc: &mut wgpu::CommandEncoder, _inputs: &[&GpuFrame], output: &GpuFrame) {
        use wgpu::util::DeviceExt;
        let stops = self.stops.take().unwrap_or_else(|| stops_view(cx, &[]));
        let u = cx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ramp"),
            contents: &bytes(&self.uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind = cx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 1, resource: u.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&stops) },
            ],
        });
        draw(enc, output.view(), &self.pipe, &bind);
    }
}


// ---------------------------------------------------------------------------
// fx.flash
// ---------------------------------------------------------------------------

const FLASH: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> u: vec4<f32>; // opacity, diffuse
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}
@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(src, vec2<i32>(pos.xy), 0);
    let target_ = vec3<f32>(1.0) + (sqrt(c.rgb) - vec3<f32>(1.0)) * u.y;
    return vec4<f32>(c.rgb + (target_ - c.rgb) * u.x, c.a);
}
"#;

struct FlashPass<'a> {
    pipe: &'a wgpu::RenderPipeline,
    layout: &'a wgpu::BindGroupLayout,
    opacity: f32,
    diffuse: f32,
}

impl GpuPass for FlashPass<'_> {
    fn name(&self) -> &'static str {
        "flash"
    }
    fn encode(&mut self, cx: &GpuCx, enc: &mut wgpu::CommandEncoder, inputs: &[&GpuFrame], output: &GpuFrame) {
        use wgpu::util::DeviceExt;
        let u = cx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("flash"),
            contents: &bytes(&[self.opacity, self.diffuse, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind = cx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(inputs[0].view()) },
                wgpu::BindGroupEntry { binding: 1, resource: u.as_entire_binding() },
            ],
        });
        draw(enc, output.view(), self.pipe, &bind);
    }
}

// ---------------------------------------------------------------------------
// fx.image-pop
// ---------------------------------------------------------------------------

const POP: &str = r#"
struct U {
    rect: vec4<f32>,   // left, top, width, height of the pop in pixels
    uv: vec4<f32>,     // su0, sv0, su1, sv1 (the centred crop)
    m: vec4<f32>,      // alpha, media width, media height
};
@group(0) @binding(0) var media: texture_2d<f32>;
@group(0) @binding(1) var<uniform> u: U;
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}
fn at(p: vec2<i32>) -> vec3<f32> { return textureLoad(media, p, 0).rgb; }
@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let px = floor(pos.xy);
    let uu = u.uv.x + ((px.x - u.rect.x) / u.rect.z) * (u.uv.z - u.uv.x);
    let vv = u.uv.y + ((px.y - u.rect.y) / u.rect.w) * (u.uv.w - u.uv.y);
    let w = u.m.y; let h = u.m.z;
    var rgb = vec3<f32>(0.0);
    if (w > 1.0 && h > 1.0) {
        let x = clamp(clamp(uu, 0.0, 1.0) * (w - 1.0), 0.0, w - 1.0);
        let y = clamp(clamp(vv, 0.0, 1.0) * (h - 1.0), 0.0, h - 1.0);
        let x0 = i32(floor(x)); let y0 = i32(floor(y));
        let x1 = min(x0 + 1, i32(w) - 1); let y1 = min(y0 + 1, i32(h) - 1);
        let tx = x - f32(x0); let ty = y - f32(y0);
        let top = at(vec2<i32>(x0, y0)) * (1.0 - tx) + at(vec2<i32>(x1, y0)) * tx;
        let bot = at(vec2<i32>(x0, y1)) * (1.0 - tx) + at(vec2<i32>(x1, y1)) * tx;
        rgb = top * (1.0 - ty) + bot * ty;
    }
    return vec4<f32>(rgb * u.m.x, u.m.x);
}
"#;

/// One pipeline per blend mode (normal, add, screen); the hardware blend does
/// `dst` mixing so the pass never reads its own target.
struct PopPipes {
    pipes: [wgpu::RenderPipeline; 3],
    layout: wgpu::BindGroupLayout,
}

impl PopPipes {
    fn new(cx: &GpuCx) -> PopPipes {
        let d = &cx.device;
        let module = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("image-pop"),
            source: wgpu::ShaderSource::Wgsl(POP.into()),
        });
        let layout =
            d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("image-pop"), entries: &[tex2d(0), uniform_entry(1)] });
        let pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("image-pop"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        use wgpu::BlendFactor as F;
        let make = |dst: F| {
            let comp = wgpu::BlendComponent { src_factor: F::One, dst_factor: dst, operation: wgpu::BlendOperation::Add };
            d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("image-pop"),
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
                        format: VIEW_FORMAT,
                        blend: Some(wgpu::BlendState { color: comp, alpha: comp }),
                        write_mask: wgpu::ColorWrites::COLOR,
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
        PopPipes { pipes: [make(F::OneMinusSrcAlpha), make(F::One), make(F::OneMinusSrc)], layout }
    }
}

fn atlas_views(cx: &GpuCx, atlas: &ImageAtlas) -> Vec<(wgpu::TextureView, u32, u32)> {
    atlas
        .frames
        .iter()
        .map(|m| {
            let (w, h) = (m.width.max(1), m.height.max(1));
            let mut rgba: Vec<u8> = m.px.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect();
            rgba.resize((w * h * 4) as usize, 0);
            let t = cx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("image-pop media"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: VIEW_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            cx.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            (t.create_view(&Default::default()), m.width, m.height)
        })
        .collect()
}

struct PopPass<'a> {
    pipes: &'a PopPipes,
    media: &'a [(wgpu::TextureView, u32, u32)],
    ingest: &'a [f32; 12],
    time: f32,
}

impl GpuPass for PopPass<'_> {
    fn name(&self) -> &'static str {
        "image_pop"
    }
    fn encode(&mut self, cx: &GpuCx, enc: &mut wgpu::CommandEncoder, inputs: &[&GpuFrame], output: &GpuFrame) {
        use wgpu::util::DeviceExt;
        let input = inputs[0];
        let (w, h) = (output.width(), output.height());
        enc.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: input.texture(), mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo { texture: output.texture(), mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let mode = self.ingest[10].clamp(0.0, 1.0);
        let crop = self.ingest[11].clamp(0.0, 1.0);
        let pipe = &self.pipes.pipes[if mode < 0.25 { 0 } else if mode < 0.75 { 1 } else { 2 }];
        // Same geometry as `image_pop::compose`, pop by pop (back to front: the
        // list is newest first, the CPU draws it in that order too).
        let mut draws = Vec::new();
        for pop in crate::fx::image_pop::pops(self.ingest, self.time, None, self.media.len()) {
            let (view, mw, mh) = &self.media[pop.frame];
            if *mw == 0 || *mh == 0 {
                continue;
            }
            let (su0, sv0) = (crop * 0.5, crop * 0.5);
            let (su1, sv1) = (1.0 - crop * 0.5, 1.0 - crop * 0.5);
            let src_w = (su1 - su0) * *mw as f32;
            let src_h = (sv1 - sv0) * *mh as f32;
            let dw = pop.scale * w as f32;
            let dh = dw * (src_h / src_w.max(1e-3));
            if dw < 1.0 || dh < 1.0 {
                continue;
            }
            let (cx_, cy_) = (pop.center[0] * w as f32, pop.center[1] * h as f32);
            let x0 = (cx_ - dw * 0.5).floor().max(0.0) as i32;
            let x1 = (cx_ + dw * 0.5).ceil().min(w as f32 - 1.0) as i32;
            let y0 = (cy_ - dh * 0.5).floor().max(0.0) as i32;
            let y1 = (cy_ + dh * 0.5).ceil().min(h as f32 - 1.0) as i32;
            if x1 < x0 || y1 < y0 {
                continue;
            }
            let u = cx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("image-pop"),
                contents: &bytes(&[
                    cx_ - dw * 0.5, cy_ - dh * 0.5, dw, dh, su0, sv0, su1, sv1, pop.alpha, *mw as f32, *mh as f32, 0.0,
                ]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind = cx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.pipes.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                    wgpu::BindGroupEntry { binding: 1, resource: u.as_entire_binding() },
                ],
            });
            draws.push((bind, [x0 as u32, y0 as u32, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32]));
        }
        if draws.is_empty() {
            return;
        }
        let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("image-pop"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output.view(),
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        p.set_pipeline(pipe);
        for (bind, r) in &draws {
            p.set_bind_group(0, bind, &[]);
            p.set_scissor_rect(r[0], r[1], r[2], r[3]);
            p.draw(0..3, 0..1);
        }
    }
}

// ---------------------------------------------------------------------------
// fx.warp
// ---------------------------------------------------------------------------

struct WarpPass<'a> {
    renderer: &'a mut metal_render::headless::Renderer,
    warp: &'a metal_schema::warp::Warp,
    prev: Option<&'a GpuFrame>,
}

impl GpuPass for WarpPass<'_> {
    fn name(&self) -> &'static str {
        "warp"
    }
    fn encode(&mut self, _cx: &GpuCx, enc: &mut wgpu::CommandEncoder, inputs: &[&GpuFrame], output: &GpuFrame) {
        // The sRGB views: the CPU-bytes path uploads the frame as `FORMAT`.
        let v = |f: &GpuFrame| f.texture().create_view(&Default::default());
        let (cur, out) = (v(inputs[0]), v(output));
        let prev = self.prev.map(v);
        self.renderer.warp_fx_encode(enc, FRAME_FORMAT, self.warp, &cur, prev.as_ref(), &out, output.width(), output.height());
    }
}

/// The lazily built lookup/ramp passes and the uploaded LUTs, one per painter.
#[derive(Default)]
pub struct FxPasses {
    lookup: Option<LookupPass>,
    ramp: Option<RampPass>,
    /// Uploaded `.cube` textures, by the parsed LUT's address (the parse cache
    /// hands the same `Arc` back for the same file).
    cubes: HashMap<usize, (Arc<Cube>, wgpu::Texture)>,
    flash: Option<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
    pop: Option<PopPipes>,
    /// Uploaded `image-pop` atlases, by the cached atlas's address (held so it
    /// stays valid): uploaded once, drawn every frame.
    atlases: HashMap<usize, (Arc<ImageAtlas>, Vec<(wgpu::TextureView, u32, u32)>)>,
    /// Each `fx.warp` node's previous output (the mosh history), by key.
    warp_hist: HashMap<String, GpuFrame>,
}

impl FxPasses {
    /// `fx.lookup` over a texture. `None` when it cannot run here (more stops
    /// than the uniform holds): the caller reads back and uses the CPU.
    pub fn lookup(
        &mut self,
        cx: &GpuCx,
        cfg: &serde_json::Value,
        root: &std::path::Path,
        input: &GpuFrame,
    ) -> Option<GpuFrame> {
        let f = parity::floats(p::LOOKUP_KIND, cfg);
        let (source, channel, mix) = (f[0].round(), f[1].round(), f[2]);
        let mut uniform = vec![source, channel, mix, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let mut cube_view = None;
        let mut stops_tex = None;
        if source as u8 == 1 {
            // Nothing to look up is the identity (the CPU returns early too).
            let Some(cube) = cfg.get("cube").and_then(serde_json::Value::as_str).and_then(|c| parity::load_cube(root, c))
            else {
                return Some(input.clone());
            };
            let key = Arc::as_ptr(&cube) as usize;
            let entry = self.cubes.entry(key).or_insert_with(|| (cube.clone(), cube_texture(cx, cube.size as u32, &cube.data)));
            cube_view = Some(entry.1.create_view(&Default::default()));
            uniform[4..8].copy_from_slice(&[cube.min[0], cube.min[1], cube.min[2], cube.size as f32]);
            uniform[8..12].copy_from_slice(&[cube.max[0], cube.max[1], cube.max[2], 1.0]);
        } else {
            let stops = parity::stops(cfg, root);
            if stops.len() > MAX_STOPS {
                return None;
            }
            uniform[3] = stops.len() as f32;
            stops_tex = Some(stops_view(cx, &stops));
        }
        let pass = self.lookup.get_or_insert_with(|| LookupPass::new(cx));
        pass.uniform = uniform;
        pass.cube = cube_view;
        pass.stops = stops_tex;
        Some(cx.run(pass, &[input], input.width(), input.height()))
    }


    /// `fx.flash` over a texture (opacity 0 is the identity).
    pub fn flash(&mut self, cx: &GpuCx, opacity: f32, diffuse: f32, input: &GpuFrame) -> GpuFrame {
        if opacity <= 0.0 {
            return input.clone();
        }
        let (pipe, layout) = self.flash.get_or_insert_with(|| pipeline(cx, "fx.flash", FLASH, &[tex2d(0), uniform_entry(1)]));
        let mut pass = FlashPass { pipe, layout, opacity, diffuse };
        cx.run(&mut pass, &[input], input.width(), input.height())
    }

    /// `fx.image-pop` over a texture; the atlas is uploaded the first time it is
    /// seen. A missing/empty atlas leaves the picture alone, as on the CPU.
    pub fn image_pop(&mut self, cx: &GpuCx, atlas: &Arc<ImageAtlas>, ingest: &[f32; 12], time: f32, input: &GpuFrame) -> GpuFrame {
        if atlas.frames.is_empty() {
            return input.clone();
        }
        let pipes = self.pop.get_or_insert_with(|| PopPipes::new(cx));
        if self.atlases.len() > 8 {
            self.atlases.clear();
        }
        let key = Arc::as_ptr(atlas) as usize;
        let entry = self.atlases.entry(key).or_insert_with(|| (atlas.clone(), atlas_views(cx, atlas)));
        let mut pass = PopPass { pipes, media: &entry.1, ingest, time };
        cx.run(&mut pass, &[input], input.width(), input.height())
    }

    /// `fx.warp` over a texture, moshing against this node's previous output.
    pub fn warp(
        &mut self,
        cx: &GpuCx,
        renderer: &mut metal_render::headless::Renderer,
        key: &str,
        warp: &metal_schema::warp::Warp,
        input: &GpuFrame,
    ) -> GpuFrame {
        let prev = self.warp_hist.get(key).filter(|p| (p.width(), p.height()) == (input.width(), input.height())).cloned();
        let out = if warp.amount <= 0.0 && (warp.mosh <= 0.0 || prev.is_none()) {
            input.clone()
        } else {
            let mut pass = WarpPass { renderer, warp, prev: prev.as_ref() };
            cx.run(&mut pass, &[input], input.width(), input.height())
        };
        self.warp_hist.insert(key.to_string(), out.clone());
        out
    }

    /// `gen.ramp` at `w x h`; `None` with more stops than the stop texture holds.
    pub fn ramp(&mut self, cx: &GpuCx, cfg: &serde_json::Value, root: &std::path::Path, w: u32, h: u32) -> Option<GpuFrame> {
        let f = parity::floats(p::RAMP_KIND, cfg);
        let stops = parity::stops(cfg, root);
        if stops.len() > MAX_STOPS {
            return None;
        }
        let aspect = w as f32 / h.max(1) as f32;
        let uniform = vec![
            f[0].round(),
            f[1],
            f[2],
            f[3],
            f[4],
            f[5].max(1.0),
            aspect,
            stops.len() as f32,
            w as f32,
            h as f32,
            0.0,
            0.0,
        ];
        let pass = self.ramp.get_or_insert_with(|| RampPass::new(cx));
        pass.uniform = uniform;
        pass.stops = Some(stops_view(cx, &stops));
        Some(cx.run(pass, &[], w, h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cx() -> GpuCx {
        let gpu = metal_render::gpu::Gpu::new_headless();
        GpuCx::new(gpu.device, gpu.queue)
    }

    pub(crate) fn picture(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                [(x * 255 / w) as u8, (y * 255 / h) as u8, ((x * 7 + y * 3) % 256) as u8, (200 + (x + y) % 56) as u8]
            })
            .collect()
    }

    /// Mean absolute channel difference and the largest.
    fn diff(a: &[u8], b: &[u8]) -> (f64, u8) {
        assert_eq!(a.len(), b.len());
        let sum: u64 = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u64).sum();
        (sum as f64 / a.len() as f64, a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0))
    }

    /// A built-in effect shader as a texture pass against `apply_cpu`.
    fn forge_vs_cpu(kind: &str, cfg: serde_json::Value, mean_lim: f64) {
        let mut renderer = metal_render::headless::Renderer::new();
        let cx = {
            let (d, q) = renderer.device_queue();
            GpuCx::new(d, q)
        };
        let (w, h) = (96u32, 54u32);
        let src = picture(w, h);
        let id = parity::shader_of(kind).expect("a GPU effect");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let wgsl = crate::forge::source(&root, id).expect("built-in shader");
        let ingest = parity::ingest(kind, &cfg);
        let input = cx.upload(&src, w, h);
        let mut pass = ForgeFxPass {
            renderer: &mut renderer,
            key: id,
            wgsl: &wgsl,
            time: 0.0,
            ingest: &ingest,
            srgb_views: false,
            error: None,
        };
        let out = cx.run(&mut pass, &[&input], w, h);
        assert!(pass.error.is_none(), "{kind}: {:?}", pass.error);
        let mut cpu = (src.clone(), w, h);
        parity::apply_cpu(kind, &cfg, &mut cpu, &root);
        let (mean, max) = diff(&cpu.0, &cx.readback(&out));
        println!("{kind}: mean {mean:.3} max {max}");
        assert!(mean < mean_lim, "{kind}: mean diff {mean:.3} (max {max})");
        assert_ne!(cpu.0, src, "{kind} changed nothing on the CPU either");
    }

    #[test]
    fn chroma_key_level_math_projection_match_the_cpu() {
        forge_vs_cpu(p::CHROMA_KEY_KIND, json!({"key": "#336633", "tolerance": 0.5, "softness": 0.2}), 1.0);
        forge_vs_cpu(p::LEVEL_KIND, json!({"gamma": 1.8, "black_in": 0.1, "contrast": 1.2, "invert": 1.0}), 1.0);
        forge_vs_cpu(p::MATH_KIND, json!({"swizzle": "bgr1", "mul_r": 0.8, "add_g": 0.1}), 1.0);
        forge_vs_cpu(p::PROJECTION_KIND, json!({"from": 0.0, "to": 2.0, "yaw": 20.0, "fov": 180.0}), 2.5);
    }

    #[test]
    fn lookup_palette_and_cube_match_the_cpu() {
        let cx = cx();
        let mut fx = FxPasses::default();
        let (w, h) = (96u32, 54u32);
        let src = picture(w, h);
        let input = cx.upload(&src, w, h);
        let dir = std::env::temp_dir().join(format!("mvj-g2-lut-{}", std::process::id()));
        metal_vfs::create_dir_all(dir.join("luts")).unwrap();
        let mut text = String::from("LUT_3D_SIZE 4\n");
        for b in 0..4 {
            for g in 0..4 {
                for r in 0..4 {
                    text += &format!("{} {} {}\n", 1.0 - r as f32 / 3.0, g as f32 / 3.0 * 0.5, (b as f32 / 3.0).powf(0.5));
                }
            }
        }
        metal_vfs::write(dir.join("luts/t.cube"), &text).unwrap();
        let many: Vec<[f32; 3]> = (0..100)
            .map(|i| {
                let t = i as f32 / 99.0;
                [t, (t * 6.0).sin().abs(), 1.0 - t]
            })
            .collect();
        for cfg in [
            json!({"source": 0.0, "channel": 0.0, "mix": 1.0, "stops": many}),
            json!({"source": 0.0, "channel": 0.0, "mix": 1.0, "stops": [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]}),
            json!({"source": 0.0, "channel": 2.0, "mix": 0.5, "stops": [[1.0, 0.5, 0.0], [0.0, 0.2, 1.0]]}),
            json!({"source": 1.0, "mix": 1.0, "cube": "luts/t.cube"}),
        ] {
            let got = fx.lookup(&cx, &cfg, &dir, &input).expect("runs");
            let mut cpu = src.clone();
            parity::lookup(&mut cpu, &cfg, &parity::floats(p::LOOKUP_KIND, &cfg), &dir);
            let (mean, max) = diff(&cpu, &cx.readback(&got));
            println!("lookup {cfg}: mean {mean:.3} max {max}");
            assert!(mean < 0.8 && max <= 3, "lookup {cfg}: mean {mean:.3} max {max}");
            assert_ne!(cpu, src);
        }
        // A cube that is not there is the identity, as on the CPU.
        let same = fx.lookup(&cx, &json!({"source": 1.0, "cube": "nope.cube"}), &dir, &input).unwrap();
        assert_eq!(cx.readback(&same), src);
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn every_ramp_shape_matches_the_cpu() {
        let cx = cx();
        let mut fx = FxPasses::default();
        let root = std::path::Path::new("/nonexistent");
        for shape in 0..4 {
            for extra in [json!({}), json!({"phase": 0.3, "repeat": 3.0, "angle": 40.0, "center_x": 0.3, "center_y": 0.6})] {
                let mut cfg = extra;
                cfg["shape"] = json!(shape);
                cfg["stops"] = json!([[1.0, 0.0, 0.2], [0.1, 0.9, 0.3], [0.0, 0.2, 1.0]]);
                let (w, h) = (96u32, 54u32);
                let got = cx.readback(&fx.ramp(&cx, &cfg, root, w, h).expect("runs"));
                let cpu = parity::ramp(&cfg, w, h, root);
                let off = cpu.iter().zip(&got).filter(|(a, b)| a.abs_diff(**b) > 4).count();
                let (mean, max) = diff(&cpu, &got);
                println!("ramp shape {shape} {cfg}: mean {mean:.3} max {max} off {off}");
                // Seam pixels (t exactly 1.0 on the diamond lattice) may land on the other side of a repeat/atan2 edge.
                assert!(mean < 1.5 && off * 100 < cpu.len() * 3, "ramp {shape} {cfg}: mean {mean:.3} off {off}");
            }
        }
    }

    fn renderer_cx() -> (metal_render::headless::Renderer, GpuCx) {
        let mut renderer = metal_render::headless::Renderer::new();
        let cx = {
            let (d, q) = renderer.device_queue();
            GpuCx::new(d, q)
        };
        (renderer, cx)
    }

    #[test]
    fn flash_matches_the_cpu() {
        let cx = cx();
        let mut fx = FxPasses::default();
        let (w, h) = (96u32, 54u32);
        let src = picture(w, h);
        let input = cx.upload(&src, w, h);
        for (o, d) in [(1.0, 1.0), (0.6, 0.0), (0.35, 0.5)] {
            let got = cx.readback(&fx.flash(&cx, o, d, &input));
            let mut cpu = src.clone();
            crate::fx::flash::apply(&mut cpu, o, d);
            let (mean, max) = diff(&cpu, &got);
            println!("flash {o} {d}: mean {mean:.3} max {max}");
            assert!(mean < 0.5 && max <= 2, "flash {o} {d}: mean {mean:.3} max {max}");
            assert_ne!(cpu, src);
        }
        assert_eq!(cx.readback(&fx.flash(&cx, 0.0, 0.5, &input)), src);
    }

    #[test]
    fn image_pop_matches_the_cpu() {
        use crate::fx::image_pop::{compose, image_pop_ingest, Atlas};
        let cx = cx();
        let mut fx = FxPasses::default();
        let media = |seed: u8| metal_reel::read::Rgb {
            width: 16,
            height: 12,
            px: (0..16 * 12).flat_map(|i| [(i as u8).wrapping_mul(7).wrapping_add(seed), (i / 16 * 20) as u8, 255 - seed]).collect(),
        };
        let atlas = Arc::new(Atlas { frames: vec![media(10), media(90), media(170)] });
        let (w, h) = (128u32, 72u32);
        let src = picture(w, h);
        let input = cx.upload(&src, w, h);
        for blend in [0.0, 0.5, 1.0] {
            let mut n = metal_graph::Node::new("p", crate::fx::image_pop::FX_IMAGE_POP_KIND, [0.0, 0.0]);
            n.config = metal_graph::Config::from_json_str(&format!(
                r#"{{"rate":8,"life":4,"concurrent":10,"scale":0.4,"spread":1.0,"seed":3,"blend":{blend},"crop":0.2}}"#
            ))
            .unwrap();
            let ingest = image_pop_ingest(&n);
            let got = cx.readback(&fx.image_pop(&cx, &atlas, &ingest, 1.3, &input));
            let mut cpu = (src.clone(), w, h);
            compose(&mut cpu, &atlas, &ingest, 1.3, None);
            let (mean, max) = diff(&cpu.0, &got);
            println!("image-pop blend {blend}: mean {mean:.3} max {max}");
            assert!(mean < 0.8, "image-pop blend {blend}: mean {mean:.3} max {max}");
            assert_ne!(cpu.0, src, "the pops drew something");
        }
    }

    #[test]
    fn warp_matches_the_cpu_with_mosh_history() {
        let (mut renderer, cx) = renderer_cx();
        let mut fx = FxPasses::default();
        let (w, h) = (96u32, 54u32);
        let a = picture(w, h);
        let b: Vec<u8> = a.iter().enumerate().map(|(i, v)| v.wrapping_add((i / 4 % 37) as u8)).collect();
        let warp = metal_schema::warp::Warp::from_config(&json!({"amount": 0.4, "mode": 0.7, "mosh": 0.35}));
        let (mut gpu_out, mut cpu_out) = (Vec::new(), Vec::new());
        for (i, f) in [&a, &b, &a].into_iter().enumerate() {
            let input = cx.upload(f, w, h);
            gpu_out = cx.readback(&fx.warp(&cx, &mut renderer, "n", &warp, &input));
            cpu_out = crate::fx::warp::apply_cpu(&format!("warp-test-{}", std::process::id()), &warp, &(f.clone(), w, h));
            let old = renderer.warp_fx("old", &warp, f, w, h).unwrap();
            let (m0, x0) = diff(&old, &gpu_out);
            let (mean, max) = diff(&cpu_out, &gpu_out);
            println!("warp frame {i}: vs cpu mean {mean:.3} max {max}; vs bytes path mean {m0:.3} max {x0}");
            assert!(m0 < 0.3, "texture pass differs from the bytes pass");
            assert!(mean < 6.0, "warp frame {i}: mean {mean:.3} max {max}");
        }
        assert_ne!(gpu_out, a);
        let _ = cpu_out;
        // amount 0 and no mosh: the identity
        let still = metal_schema::warp::Warp::default();
        let input = cx.upload(&a, w, h);
        assert_eq!(cx.readback(&fx.warp(&cx, &mut renderer, "z", &still, &input)), a);
    }

    #[test]
    fn crop_is_the_cpu_crop() {
        let cx = cx();
        let (w, h) = (96u32, 54u32);
        let src = picture(w, h);
        let input = cx.upload(&src, w, h);
        let win = crate::layers::Sector { x: 0.25, y: 0.1, w: 0.5, h: 0.6 };
        let (x, y, cw, ch) = crate::engine::crop_rect(w, h, &win).unwrap();
        let got = cx.readback(&cx.crop(&input, x, y, cw, ch));
        let (cpu, ..) = crate::engine::crop_window(&src, w, h, &win).unwrap();
        assert_eq!(got, cpu);
    }

    /// `cargo test -p metal-perform --features gpu --release --lib gpu_fx::tests::measure -- --ignored --nocapture`
    ///
    /// Before = bytes in, bytes out (`Renderer::forge_fx`: upload, pass,
    /// readback); after = texture through the pass, GPU work waited.
    #[test]
    #[ignore]
    fn measure() {
        use std::time::Instant;
        let mut renderer = metal_render::headless::Renderer::new();
        let cx = {
            let (d, q) = renderer.device_queue();
            GpuCx::new(d, q)
        };
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let mut fx = FxPasses::default();
        for (w, h) in [(1920u32, 1080u32), (3840, 2160)] {
            let src = picture(w, h);
            let input = cx.upload(&src, w, h);
            for kind in [p::LEVEL_KIND, p::CHROMA_KEY_KIND] {
                let id = parity::shader_of(kind).unwrap();
                let wgsl = crate::forge::source(&root, id).unwrap();
                let ingest = parity::ingest(kind, &json!({}));
                let n = 20;
                let _ = renderer.forge_fx(id, &wgsl, &src, w, h, 0.0, &ingest);
                let t0 = Instant::now();
                for _ in 0..n {
                    std::hint::black_box(renderer.forge_fx(id, &wgsl, &src, w, h, 0.0, &ingest).unwrap());
                }
                let before = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
                let t0 = Instant::now();
                for _ in 0..n {
                    let mut pass = ForgeFxPass { renderer: &mut renderer, key: id, wgsl: &wgsl, time: 0.0, ingest: &ingest, srgb_views: false, error: None };
                    std::hint::black_box(cx.run(&mut pass, &[&input], w, h));
                }
                let _ = cx.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
                let after = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
                println!("MEASURE fx {kind} {w}x{h}: before (bytes) {before:.2} ms, after (texture) {after:.2} ms");
            }
            let cfg = json!({"stops": [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]});
            let t0 = Instant::now();
            for _ in 0..10 {
                let mut px = src.clone();
                parity::lookup(&mut px, &cfg, &parity::floats(p::LOOKUP_KIND, &cfg), &root);
            }
            let before = t0.elapsed().as_secs_f64() * 100.0;
            let t0 = Instant::now();
            for _ in 0..10 {
                std::hint::black_box(fx.lookup(&cx, &cfg, &root, &input));
            }
            let _ = cx.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
            let after = t0.elapsed().as_secs_f64() * 100.0;
            println!("MEASURE fx lookup {w}x{h}: before (CPU) {before:.2} ms, after (texture) {after:.2} ms");
            let t0 = Instant::now();
            for _ in 0..5 {
                std::hint::black_box(parity::ramp(&cfg, w, h, &root));
            }
            let before = t0.elapsed().as_secs_f64() * 200.0;
            let t0 = Instant::now();
            for _ in 0..5 {
                std::hint::black_box(fx.ramp(&cx, &cfg, &root, w, h));
            }
            let _ = cx.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
            let after = t0.elapsed().as_secs_f64() * 200.0;
            println!("MEASURE gen.ramp {w}x{h}: before (CPU) {before:.2} ms, after (texture) {after:.2} ms");
        }
    }

    /// `cargo test -p metal-perform --features gpu --release --lib gpu_fx::tests::measure_g2a -- --ignored --nocapture --test-threads=1`
    ///
    /// Before = the CPU/bytes path, after = texture through the pass (GPU work waited).
    #[test]
    #[ignore]
    fn measure_g2a() {
        use std::time::Instant;
        let (mut renderer, cx) = renderer_cx();
        let wait = || {
            let _ = cx.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        };
        let mut fx = FxPasses::default();
        let media = || metal_reel::read::Rgb { width: 256, height: 144, px: (0..256 * 144 * 3).map(|i| (i % 251) as u8).collect() };
        let atlas = Arc::new(crate::fx::image_pop::Atlas { frames: vec![media(), media()] });
        let mut n = metal_graph::Node::new("p", crate::fx::image_pop::FX_IMAGE_POP_KIND, [0.0, 0.0]);
        n.config = metal_graph::Config::from_json_str(r#"{"rate":8,"life":4,"concurrent":16,"scale":0.3,"spread":1.0,"seed":3}"#).unwrap();
        let ingest = crate::fx::image_pop::image_pop_ingest(&n);
        let warp = metal_schema::warp::Warp::from_config(&json!({"amount": 0.4, "mode": 0.7, "mosh": 0.3}));
        let many: Vec<[f32; 3]> = (0..100).map(|i| [i as f32 / 99.0, 0.5, 1.0 - i as f32 / 99.0]).collect();
        let lut = json!({"source": 0.0, "channel": 0.0, "mix": 1.0, "stops": many});
        let root = std::path::Path::new("/nonexistent");
        let win = crate::layers::Sector { x: 0.0, y: 0.0, w: 0.5, h: 1.0 };
        for (w, h) in [(1920u32, 1080u32), (3840, 2160)] {
            let src = picture(w, h);
            let input = cx.upload(&src, w, h);
            let n_it = 10;
            let mut time = |name: &str, before: &mut dyn FnMut(), after: &mut dyn FnMut()| {
                before();
                after();
                wait();
                let t = Instant::now();
                for _ in 0..n_it {
                    before();
                }
                let b = t.elapsed().as_secs_f64() * 1000.0 / n_it as f64;
                let t = Instant::now();
                for _ in 0..n_it {
                    after();
                }
                wait();
                let a = t.elapsed().as_secs_f64() * 1000.0 / n_it as f64;
                println!("{w}x{h} {name}: {b:.1} -> {a:.1} ms");
            };
            time("flash", &mut || { let mut p = src.clone(); crate::fx::flash::apply(&mut p, 0.6, 0.5); }, &mut || { let _ = fx.flash(&cx, 0.6, 0.5, &input); });
            time("image-pop", &mut || { let mut f = (src.clone(), w, h); crate::fx::image_pop::compose(&mut f, &atlas, &ingest, 1.3, None); }, &mut || { let _ = fx.image_pop(&cx, &atlas, &ingest, 1.3, &input); });
            time("lookup 100 stops", &mut || { let mut p = src.clone(); parity::lookup(&mut p, &lut, &parity::floats(p::LOOKUP_KIND, &lut), root); }, &mut || { let _ = fx.lookup(&cx, &lut, root, &input); });
            time("span crop", &mut || { let _ = crate::engine::crop_window(&src, w, h, &win); }, &mut || { let (x, y, cw, ch) = crate::engine::crop_rect(w, h, &win).unwrap(); let _ = cx.crop(&input, x, y, cw, ch); });
            time("still/capture re-upload vs cached", &mut || { let _ = cx.upload(&src, w, h); }, &mut || { let _ = input.clone(); });
        }
        // warp needs the renderer for both sides
        for (w, h) in [(1920u32, 1080u32), (3840, 2160)] {
            let src = picture(w, h);
            let input = cx.upload(&src, w, h);
            let t = Instant::now();
            for _ in 0..5 {
                let _ = renderer.warp_fx("m", &warp, &src, w, h);
            }
            let b = t.elapsed().as_secs_f64() * 200.0;
            let t = Instant::now();
            for _ in 0..5 {
                let _ = fx.warp(&cx, &mut renderer, "m2", &warp, &input);
            }
            wait();
            println!("{w}x{h} warp: {b:.1} -> {:.1} ms", t.elapsed().as_secs_f64() * 200.0);
        }
    }
}
