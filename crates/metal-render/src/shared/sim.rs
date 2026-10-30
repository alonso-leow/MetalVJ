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

/// `wgsl` without the spans between `//@compute` and `//@end-compute` lines:
/// the module a device with no compute shaders can build (a storage-texture
/// binding or a `@compute` entry point in the module fails there even when
/// unused). The marked span holds the compute-only declarations; the shared
/// functions around it are the code both paths run.
pub fn without_compute(wgsl: &str) -> String {
    let mut out = String::with_capacity(wgsl.len());
    let mut skipping = false;
    for line in wgsl.lines() {
        match line.trim() {
            "//@compute" => skipping = true,
            "//@end-compute" => skipping = false,
            _ if skipping => {}
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// The same module with the marker comments dropped and the compute spans kept.
pub fn with_compute(wgsl: &str) -> String {
    wgsl.lines()
        .filter(|l| !matches!(l.trim(), "//@compute" | "//@end-compute"))
        .collect::<Vec<_>>()
        .join("\n")
}

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

/// Simulation ticks per second: the CPU sims step at this fixed rate, so a
/// 60 fps draw loop runs every other draw and reuses the last state.
pub const SIM_HZ: f32 = 30.0;
/// Most ticks one draw may run; a longer gap drops the rest (slowdown, not a
/// backlog that stalls the next frames).
pub const MAX_TICKS: u32 = 3;

/// A fixed-rate accumulator over the frame clock: [`tick`](Self::tick) turns a
/// frame's elapsed time into the whole number of sim ticks to run now, and
/// carries the fraction to the next draw.
#[derive(Clone, Copy, Debug)]
pub struct FixedStep {
    hz: f32,
    max: u32,
    acc: f32,
}

impl FixedStep {
    pub const fn new(hz: f32, max: u32) -> FixedStep {
        FixedStep { hz, max, acc: 0.0 }
    }

    /// Ticks to run for `dt` seconds of elapsed time (the caller clamps it
    /// against seeks). Never more than `max`.
    pub fn tick(&mut self, dt: f32) -> u32 {
        self.acc += dt.max(0.0) * self.hz;
        let n = (self.acc.floor() as u32).min(self.max);
        // A capped backlog is dropped, not carried.
        self.acc = if n == self.max { 0.0 } else { self.acc - n as f32 };
        n
    }

    /// Forget the carried fraction; the next tick starts a fresh interval.
    pub fn reset(&mut self) {
        self.acc = 0.0;
    }
}

impl Default for FixedStep {
    fn default() -> Self {
        FixedStep::new(SIM_HZ, MAX_TICKS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixty_fps_runs_about_every_other_draw_at_thirty_hz() {
        let mut s = FixedStep::default();
        let ticks: Vec<u32> = (0..60).map(|_| s.tick(1.0 / 60.0)).collect();
        let total: u32 = ticks.iter().sum();
        assert!((29..=30).contains(&total), "one second is 30 ticks: {total}");
        assert!(ticks.iter().all(|&n| n <= 1), "no draw runs two ticks at 60 fps");
    }

    #[test]
    fn a_long_gap_is_capped_and_the_backlog_dropped() {
        let mut s = FixedStep::new(30.0, 3);
        assert_eq!(s.tick(1.0), 3, "a long gap is capped");
        assert_eq!(s.tick(0.0), 0, "the capped backlog is not carried");
        assert_eq!(s.tick(-5.0), 0, "a rewound clock ticks nothing");
    }

    #[test]
    fn a_fraction_carries_until_it_is_a_whole_tick() {
        let mut s = FixedStep::new(30.0, 3);
        assert_eq!(s.tick(0.02), 0, "0.6 of a tick is carried");
        assert_eq!(s.tick(0.02), 1, "0.6 + 0.6 crosses one tick");
        assert_eq!(s.tick(0.02), 0, "0.2 + 0.6 is still short");
    }

    #[test]
    fn reset_forgets_the_fraction() {
        let mut s = FixedStep::new(30.0, 3);
        assert_eq!(s.tick(0.02), 0);
        s.reset();
        assert_eq!(s.tick(0.02), 0, "without the reset this would tick");
        assert_eq!(s.tick(0.02), 1);
    }
}
