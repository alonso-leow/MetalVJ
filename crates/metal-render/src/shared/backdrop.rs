//! The background every model that draws *on* something now shares.
//!
//! Eleven models opened by clearing their frame to `wgpu::Color::BLACK` and
//! drawing objects over it: strings, spirals, hairs, figures, panels, glyphs.
//! That black was a literal in eleven places and the one large area of the
//! frame an operator could not touch — in a suite whose whole colour story is
//! that every part of a model is a slot.
//!
//! This is that literal, made into one pass. A model builds a [`Backdrop`]
//! alongside its own pipeline, calls [`Backdrop::draw`] first, and switches
//! its own pass from `Clear` to `Load`. Slots start solid black
//! (`metal_schema::fondo`), so a model that gains one looks exactly as it did.
//!
//! It fills with a *gradient*, not a flat colour, and that is the reason this
//! is a pass rather than a `clear_at` on the colour slot: `LoadOp::Clear`
//! takes one colour, and the largest area in the frame is the last place a
//! four-stop slot should collapse to its first stop. Models whose own field
//! already covers the frame (Fractals, Kaleido, Turing, Domain, Vaporgrid's
//! sky, Oracle's skin) do not use this — see `metal_schema::fondo`.

use wgpu::util::DeviceExt;

use super::ColorSlotGpu;
use crate::model::Ctx;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    color: ColorSlotGpu,
    misc: [f32; 4],
}

pub struct Backdrop {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

impl Backdrop {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Backdrop {
        let shader = super::build_shader(
            device,
            "backdrop",
            include_str!("../../shaders/backdrop.wgsl"),
        );

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("backdrop bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("backdrop pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("backdrop pipeline"),
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
                    blend: None,
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

        Backdrop { pipeline, bind_group_layout }
    }

    /// Fills `target` with `color`, walked along `angle`.
    ///
    /// `angle` is in radians, measured the way the models measure their own
    /// rotations: 0 walks the slot top to bottom, and a model that turns its
    /// content — Escape's `giro` — passes the same angle here so the
    /// background turns with it rather than staying pinned to the frame.
    ///
    /// This is the pass that clears, so it must run before the model's own,
    /// and the model's own must then `Load`.
    pub fn draw(
        &self,
        ctx: &Ctx,
        target: &wgpu::TextureView,
        color: ColorSlotGpu,
        angle: f32,
        alpha: bool,
    ) {
        let (sin, cos) = angle.sin_cos();
        // misc.w: was unused padding; now 1.0 means "write alpha 0 instead
        // of 1 here" — the fondo-as-alpha switch, see the design notes PARTE IV §17.3.
        let uniform =
            Uniform { color, misc: [sin, -cos, ctx.aspect(), if alpha { 1.0 } else { 0.0 }] };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("backdrop uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("backdrop bind group"),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("backdrop encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("backdrop pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}
