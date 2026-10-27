//! The `fx.warp` GPU pass (the loops design notes T12): a frame displaced by a mesh
//! field, optionally moshed against this warp's own previous output.
//!
//! The notation — how the mesh is written, generated from the params and
//! interpolated — is `metal_schema::warp`'s module doc; this pass uploads the
//! mesh [`metal_schema::warp::Warp::mesh`] builds and `shaders/warp.wgsl` does
//! the same Catmull-Rom sum per pixel that `metal_schema::warp::field` does on
//! the CPU. Like `fx.forge`, it runs on an RGBA frame after readback: one
//! upload (two with history), one pass, one readback. The history is a
//! per-key copy of the last output, kept here (see the notation doc for why
//! it is not the post chain's trail).

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use metal_schema::warp::{Warp, N};

const SHADER: &str = include_str!("../../shaders/warp.wgsl");

/// Uniform size: one vec4 of flags, then 145 vec4 of mesh.
const MESH_VEC4: usize = 145;

struct Built {
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

/// The pass plus each warp node's previous output, by key.
#[derive(Default)]
pub struct WarpFx {
    built: Option<Built>,
    history: HashMap<String, (u32, u32, Vec<u8>)>,
}

/// The uniform's floats: `[mosh, has_prev, N, 0]` then the mesh, two points
/// per vec4.
pub fn uniform_values(warp: &Warp, aspect: f32, has_prev: bool) -> Vec<f32> {
    let mut v = vec![warp.mosh, if has_prev { 1.0 } else { 0.0 }, N as f32, 0.0];
    for d in warp.mesh(aspect) {
        v.extend_from_slice(&d);
    }
    v.resize(4 + MESH_VEC4 * 4, 0.0);
    v
}

fn build(device: &wgpu::Device, format: wgpu::TextureFormat) -> Built {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("warp"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let tex = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("warp"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            tex(1),
            tex(2),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("warp"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("warp"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
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
    });
    Built { format, pipeline, layout }
}

fn upload(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, px: &[u8], w: u32, h: u32) -> wgpu::Texture {
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("warp input"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        px,
    )
}

impl WarpFx {
    /// [`WarpFx::apply`] over textures, recorded into `enc` (G2 `GpuPass`): `cur`,
    /// `prev` (the node's previous output, when it has one) and `out` are views
    /// in `format`. The caller keeps the history. No readback, no submit.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        enc: &mut wgpu::CommandEncoder,
        format: wgpu::TextureFormat,
        warp: &Warp,
        cur: &wgpu::TextureView,
        prev: Option<&wgpu::TextureView>,
        out: &wgpu::TextureView,
        w: u32,
        h: u32,
    ) {
        if self.built.as_ref().is_none_or(|b| b.format != format) {
            self.built = Some(build(device, format));
        }
        let built = self.built.as_ref().expect("built above");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("warp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let values = uniform_values(warp, w as f32 / h.max(1) as f32, prev.is_some());
        let ubuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("warp uniform"),
            contents: bytemuck::cast_slice(&values),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("warp"),
            layout: &built.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(cur) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(prev.unwrap_or(cur)) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("warp"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: out,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&built.pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Warps `input` (RGBA8, `w × h`) by `warp`. `key` names the node whose
    /// history mosh reads and which this frame's output becomes. Amount 0
    /// and mosh 0 hand `input` back untouched (and still record it).
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        key: &str,
        warp: &Warp,
        input: &[u8],
        w: u32,
        h: u32,
    ) -> Result<Vec<u8>, String> {
        if w == 0 || h == 0 || input.len() != (w * h * 4) as usize {
            return Err(format!("{key}: the input is not a {w}x{h} RGBA frame"));
        }
        let prev = self.history.get(key).filter(|(pw, ph, _)| (*pw, *ph) == (w, h)).map(|(_, _, p)| p.clone());
        let out = if warp.amount <= 0.0 && (warp.mosh <= 0.0 || prev.is_none()) {
            input.to_vec()
        } else {
            if self.built.as_ref().is_none_or(|b| b.format != format) {
                self.built = Some(build(device, format));
            }
            let built = self.built.as_ref().expect("built above");
            let cur_tex = upload(device, queue, format, input, w, h);
            let prev_tex = upload(device, queue, format, prev.as_deref().unwrap_or(input), w, h);
            let cur_view = cur_tex.create_view(&Default::default());
            let prev_view = prev_tex.create_view(&Default::default());
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("warp"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            let values = uniform_values(warp, w as f32 / h as f32, prev.is_some());
            let ubuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("warp uniform"),
                contents: bytemuck::cast_slice(&values),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("warp"),
                layout: &built.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&cur_view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&prev_view) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&sampler) },
                ],
            });
            let (target, target_view) = crate::gpu::offscreen_target(device, w, h, format);
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("warp") });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("warp"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&built.pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..3, 0..1);
            }
            queue.submit(Some(encoder.finish()));
            crate::gpu::read_rgba8(device, queue, &target, w, h)
        };
        self.history.insert(key.to_string(), (w, h, out.clone()));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uniform_is_flags_then_the_mesh() {
        let w = Warp { amount: 0.5, mosh: 0.25, ..Warp::default() };
        let v = uniform_values(&w, 1.0, true);
        assert_eq!(v.len(), 4 + MESH_VEC4 * 4);
        assert_eq!(&v[..3], &[0.25, 1.0, N as f32]);
        assert!(N * N <= MESH_VEC4 * 2, "the mesh fits the uniform array");
        let mesh = w.mesh(1.0);
        assert_eq!(&v[4..6], &mesh[0]);
        assert_eq!(&v[4 + 2 * (N * N - 1)..4 + 2 * N * N], &mesh[N * N - 1]);
    }
}
