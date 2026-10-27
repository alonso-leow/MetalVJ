//! A ping-pong pair of render targets for the grid simulations (cellular
//! automata, fluid, FDTD, Euler): the state lives in a texture, a fullscreen
//! pass reads the previous state and writes the next, then the two swap.
//!
//! Built on [`Fullscreen`](super::fullscreen::Fullscreen): the *step* shader is
//! one `Fullscreen` (with one texture pair: the previous state), the *display*
//! shader is another that samples the current state. State format is
//! `Rgba16Float` so it is renderable and filterable on every backend, so step
//! passes are drawn with that format, display passes with the caller's.

use super::fullscreen::Fullscreen;
use crate::model::Ctx;

pub const STATE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub struct PingPong {
    views: [wgpu::TextureView; 2],
    _textures: [wgpu::Texture; 2],
    sampler: wgpu::Sampler,
    pub width: u32,
    pub height: u32,
    /// Index of the texture holding the current state.
    cur: usize,
    /// True until the first step; the step shader may read `uniform` flags to
    /// seed itself, and the caller asks `fresh()` to decide.
    fresh: bool,
}

impl PingPong {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> PingPong {
        let make = || {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sim state"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: STATE_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let (a, b) = (make(), make());
        let views = [
            a.create_view(&wgpu::TextureViewDescriptor::default()),
            b.create_view(&wgpu::TextureViewDescriptor::default()),
        ];
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        PingPong { views, _textures: [a, b], sampler, width, height, cur: 0, fresh: true }
    }

    /// True before the first [`step`](Self::step) since creation or [`reset`](Self::reset).
    pub fn fresh(&self) -> bool {
        self.fresh
    }

    /// Forget the state; the next step is a fresh seed.
    pub fn reset(&mut self) {
        self.fresh = true;
    }

    /// The current state, for the display pass: `(view, sampler)`.
    pub fn current(&self) -> (&wgpu::TextureView, &wgpu::Sampler) {
        (&self.views[self.cur], &self.sampler)
    }

    /// The texture holding the current state (for tests that read it back).
    pub fn current_texture(&self) -> &wgpu::Texture {
        &self._textures[self.cur]
    }

    /// Run `pass` once: it reads the current state as texture pair 0 and writes
    /// the other texture, then they swap. `pass` must be built with one texture.
    pub fn step<U: bytemuck::Pod>(&mut self, ctx: &Ctx, pass: &mut Fullscreen, uniform: &U) {
        let next = 1 - self.cur;
        pass.draw_textured(
            ctx,
            &self.views[next],
            STATE_FORMAT,
            uniform,
            &[(&self.views[self.cur], &self.sampler)],
        );
        self.cur = next;
        self.fresh = false;
    }
}
