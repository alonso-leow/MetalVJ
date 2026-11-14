//! `fx.hardstutter` — a beat-locked gate that hides or holds frames.
//!
//! # The design choice: a persistent texture, not a lookup on the clock
//!
//! `shared::footage` can make its trail "a lookup, not a memory" because a
//! transmission's past frames are rungs of a ladder at known addresses: the
//! frame at `t - d` is one evaluation of the same position function. A
//! stutter on a *generated* picture cannot do that. To hold the frame of
//! `t_hold` the renderer would have to run the model, the ghost trail, the
//! estela ring and the whole post chain again at `t_hold`, and several of
//! those are histories themselves (turing, ghost, the rings), so "the picture
//! at an earlier time" is not a function of the time. Option (a) is therefore
//! out and this is option (b): **the effect owns the frames it holds**, in
//! textures that persist between draws, exactly like `shared::trail`'s
//! [`Trail`](super::trail) and its wake ring.
//!
//! **This breaks the fx recipe's "no previous-frame texture" limit** on
//! purpose, and the way it does so is narrow:
//!
//! * the post pass is untouched. It still has no memory; the chain step's
//!   `case` in `post.wgsl` is a no-op (like `fx.brush`'s, and listed in
//!   `STATEFUL_CHAIN_EFFECTS` in the conformance test), there only so the
//!   step is a declared chain kind. The group has no `ChainFx` block:
//!   [`StutterParams`] reads its floats from `Post.advanced` directly;
//! * when a `fx.hardstutter` step is on, `Renderer::draw_tile` sends the post
//!   chain into an intermediate texture and runs [`Stutter::compose`] over it
//!   into the real target. So the stutter freezes the **final** picture
//!   (leaks, grade, every other step), wherever the step sits in the chain —
//!   "last in the chain" is by construction, not by ordering;
//! * off means off: with `cant == 0` (or no step) no intermediate, no pass and
//!   no allocation exist, and the frame is bit-identical to before this file.
//!
//! # State, kept per model index (like ghost and the estela ring)
//!
//! A ring of up to [`MAX_LOOP`] slots of the final picture. While the gate is
//! open the live frame is copied in; while it is shut the compose pass shows
//! a slot instead: the newest (`repite_n` 0 or 1, a freeze) or a cycle through
//! the newest `repite_n` (a loop of frames, [`LOOP_FPS`] per second). Which
//! slot is a function of the time since the gate shut, not a counter, so asking
//! for the same instant twice changes nothing. A gap larger than
//! [`SEEK`] seconds, or time running backwards, empties the ring (no history
//! to hold): the next frame is shown live and becomes the held one.
//!
//! # Beat lock
//!
//! The renderer has no beat clock (see the note above `stand_in_pulse` in
//! `post.wgsl`), so the gate is locked to `time` at the `tempo` float (BPM) and
//! slid by `retardo`; a song of constant tempo whose downbeat is at time 0 is
//! locked by setting `tempo` to the song's. The gate maths is [`gate`], pure
//! CPU, tested without a GPU.
//!
//! # Limits
//!
//! * State is per model index. Two layers drawing the same model share one
//!   hold (the same limit ghost and the wake ring have).
//! * `app.rs`'s window path keeps no per-model history of any kind, so there
//!   the step is the identity.

use wgpu::util::DeviceExt;

use crate::gpu::offscreen_target;

use super::post::group_floats;

/// Gaps beyond this (and any step backwards) are a seek: nothing held survives.
/// The same one second as `shared::trail`.
const SEEK: f32 = 1.0;

/// Most frames a loop can hold (`repite_n`'s ceiling in the schema).
pub const MAX_LOOP: usize = 8;

/// How fast a `repite_n` loop plays, frames per second, and so how often the
/// ring takes a new frame while the gate is open.
pub const LOOP_FPS: f32 = 24.0;

/// Steps per beat for `ritmo`: 1/4, 1/8, 1/8T, 1/16, 1/16T, 1/32.
pub const STEPS_PER_BEAT: [f32; 6] = [1.0, 2.0, 3.0, 4.0, 6.0, 8.0];

/// The group's floats (everything but `cant`, which is the step's amount),
/// clamped to their declared ranges, in a shape the gate reads by name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StutterParams {
    pub rhythm: usize,
    pub duty: f32,
    pub congela: f32,
    pub repite_n: usize,
    pub retardo: f32,
    pub jitter: f32,
    pub patron: u32,
    pub invert: bool,
    pub tempo: f32,
    pub fade: f32,
    pub tint: f32,
    pub tinte_tono: f32,
}

impl StutterParams {
    /// From `Post.advanced`, tolerating a short or missing slice like
    /// [`ChainFx::from_advanced`](super::post::ChainFx::from_advanced).
    pub fn from_advanced(advanced: &[f32]) -> StutterParams {
        let f = group_floats(advanced, "hardstutter");
        let at = |i: usize| f.get(i).copied().unwrap_or(0.0);
        StutterParams {
            rhythm: at(1).round() as usize,
            duty: at(2),
            congela: at(3),
            repite_n: at(4).round() as usize,
            retardo: at(5),
            jitter: at(6),
            patron: at(7).round() as u32,
            invert: at(8).round() >= 1.0,
            tempo: at(9),
            fade: at(10),
            tint: at(11),
            tinte_tono: at(12),
        }
    }
}

/// What the gate says about one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gate {
    /// The gate is shut: the picture is not the live one (fully or partly).
    pub hidden: bool,
    /// How much of the live picture the hidden look has replaced, 0..1
    /// (`desvanece` ramps it in; 1 is a hard cut). 0 while visible.
    pub blend: f32,
    /// Seconds since the gate shut (0 while visible).
    pub held_secs: f32,
}

/// A deterministic hash of a step number to 0..1.
fn hash01(step: i64, salt: u32) -> f32 {
    let mut x = (step as u32).wrapping_mul(0x9E37_79B1) ^ salt;
    x ^= x >> 15;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^= x >> 16;
    (x >> 8) as f32 / 16_777_216.0
}

/// Whether the pattern gates step `step` (before `invierte_ritmo`).
fn pattern(kind: u32, step: i64) -> bool {
    let r = step.rem_euclid(8);
    match kind {
        0 => true,
        1 => step.rem_euclid(2) == 1,
        2 => (r * 3) % 8 < 3,
        3 => (r * 5) % 8 < 5,
        _ => hash01(step, 0x51ED_270B) > 0.5,
    }
}

/// The gate at `time`: pure arithmetic on the clock, no state.
pub fn gate(p: &StutterParams, time: f32) -> Gate {
    let visible = Gate { hidden: false, blend: 0.0, held_secs: 0.0 };
    let spb = STEPS_PER_BEAT[p.rhythm.min(STEPS_PER_BEAT.len() - 1)] as f64;
    let step_secs = 60.0 / (p.tempo.max(1.0) as f64 * spb);
    let pos = time as f64 / step_secs - p.retardo as f64;
    let step = pos.floor();
    let frac = (pos - step) as f32;
    let step = step as i64;
    if pattern(p.patron, step) == p.invert {
        return visible;
    }
    let duty = (p.duty + (hash01(step, 0xA3C5_9AC3) - 0.5) * p.jitter).clamp(0.02, 0.98);
    if frac < duty {
        return visible;
    }
    let since = frac - duty;
    let blend = if p.fade <= 0.0 {
        1.0
    } else {
        let t = (since / (p.fade * 0.5)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    Gate { hidden: true, blend, held_secs: since * step_secs as f32 }
}

/// Which ring slot a hidden frame shows: the newest for a freeze, a cycle
/// through the newest `n` (oldest first) for a loop. `cursor` is the newest
/// slot, `filled` how many hold real frames, `ring` the ring's length.
fn slot_for(held_secs: f32, n: usize, cursor: usize, filled: usize, ring: usize) -> usize {
    let count = filled.min(n).max(1);
    if n <= 1 || count == 1 {
        return cursor;
    }
    let k = (held_secs * LOOP_FPS).floor().max(0.0) as usize;
    let back = (count - 1) - (k % count);
    (cursor + ring - back) % ring
}

/// `h` in 0..1 to a saturated RGB (the tint's colour).
fn hue_rgb(h: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let f = |n: f32| ((h + n).rem_euclid(6.0) - 3.0).abs().clamp(1.0, 2.0) - 1.0;
    [f(0.0), f(4.0), f(2.0)]
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    a: [f32; 4],
    tint: [f32; 4],
}

/// One model's held frames.
struct Hold {
    slots: Vec<(wgpu::Texture, wgpu::TextureView)>,
    /// Newest written slot.
    cursor: usize,
    /// Slots holding a real frame.
    filled: usize,
    size: (u32, u32),
    /// The ring length the slots were filled under (`repite_n`, at least 1).
    ring: usize,
    last_time: Option<f32>,
    last_push: Option<f32>,
}

/// The pass, the intermediate the post chain draws into, and every model's
/// [`Hold`].
pub struct Stutter {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    live: Option<(wgpu::Texture, wgpu::TextureView, u32, u32)>,
    holds: Vec<Option<Hold>>,
    format: wgpu::TextureFormat,
}

impl Stutter {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Stutter {
        let shader = super::build_shader(device, "stutter", include_str!("../../shaders/stutter.wgsl"));
        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
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
            label: Some("stutter bind group layout"),
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
                texture_entry(1),
                texture_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("stutter pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("stutter pipeline"),
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("stutter sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Stutter { pipeline, layout, sampler, live: None, holds: Vec::new(), format }
    }

    /// The `w x h` texture the post chain draws into when a step is on
    /// (rebuilt on a size change).
    pub fn live_view(&mut self, device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
        if !matches!(&self.live, Some((_, _, lw, lh)) if (*lw, *lh) == (w, h)) {
            let (tex, view) = offscreen_target(device, w, h, self.format);
            self.live = Some((tex, view, w, h));
        }
        self.live.as_ref().expect("built above").1.clone()
    }

    /// Model `index` has no step on: free what it held (and the intermediate
    /// once nothing holds anything).
    pub fn release(&mut self, index: usize) {
        if let Some(slot) = self.holds.get_mut(index) {
            *slot = None;
        }
        if self.holds.iter().all(|h| h.is_none()) {
            self.holds.clear();
            self.live = None;
        }
    }

    /// How many models hold frames right now (tests).
    pub fn allocated(&self) -> usize {
        self.holds.iter().filter(|h| h.is_some()).count()
    }

    /// Composes the post chain's output (in the texture [`Stutter::live_view`]
    /// returned) with the held frame into `dst`'s `rect`, then updates the
    /// ring. `amount` is the step's amount.
    #[allow(clippy::too_many_arguments)]
    pub fn compose(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        index: usize,
        p: &StutterParams,
        amount: f32,
        time: f32,
        dst: &wgpu::TextureView,
        rect: (u32, u32, u32, u32),
        clear: bool,
    ) {
        let Some((live_tex, live_view, w, h)) = &self.live else { return };
        let (w, h) = (*w, *h);
        if self.holds.len() <= index {
            self.holds.resize_with(index + 1, || None);
        }
        let n = p.repite_n.clamp(1, MAX_LOOP);
        let format = self.format;
        let stale = !matches!(&self.holds[index], Some(hd) if hd.size == (w, h));
        if stale {
            self.holds[index] = Some(Hold {
                slots: Vec::new(),
                cursor: 0,
                filled: 0,
                size: (w, h),
                ring: n,
                last_time: None,
                last_push: None,
            });
        }
        let hold = self.holds[index].as_mut().expect("built above");
        while hold.slots.len() < n {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("stutter slot"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            hold.slots.push((texture, view));
        }
        // Changing `repite_n` changes the ring's modulus, so what was held no
        // longer lines up: start the hold over.
        let ring = n;
        if hold.ring != ring {
            hold.ring = ring;
            hold.cursor = 0;
            hold.filled = 0;
        }

        // A seek, a rewind or the first frame: there is nothing to hold yet.
        let continuous = matches!(hold.last_time, Some(l) if (0.0..=SEEK).contains(&(time - l)));
        if !continuous {
            hold.filled = 0;
            hold.last_push = None;
        }
        hold.last_time = Some(time);

        let g = gate(p, time);
        let have = hold.filled > 0;
        let showing_held = g.hidden && have;
        let (blend, shown) = if showing_held {
            (g.blend * amount.clamp(0.0, 1.0), slot_for(g.held_secs, n, hold.cursor, hold.filled, ring))
        } else {
            (0.0, hold.cursor)
        };

        let rgb = hue_rgb(p.tinte_tono);
        let uniform = Uniform {
            a: [blend, p.congela, p.tint, 0.0],
            tint: [rgb[0], rgb[1], rgb[2], 0.0],
        };
        let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("stutter uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        // Before the first frame is held the slot is unwritten; the shader
        // never reads it (`blend` is 0) but a view must still be bound.
        let held_view = if hold.slots.is_empty() { live_view } else { &hold.slots[shown].1 };
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stutter bind group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(live_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(held_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        let (rx, ry, rw, rh) = rect;
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("stutter encoder") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("stutter pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dst,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: if clear { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load },
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
            pass.set_viewport(rx as f32, ry as f32, rw as f32, rh as f32, 0.0, 1.0);
            pass.set_scissor_rect(rx, ry, rw, rh);
            pass.draw(0..3, 0..1);
        }
        queue.submit(Some(encoder.finish()));

        // The ring takes the live frame while the gate is open (or has
        // nothing to show yet). A loop takes one per `1 / LOOP_FPS`; a freeze
        // takes every distinct instant, so it is always the newest visible.
        if !showing_held {
            let gap = if n > 1 { 1.0 / LOOP_FPS } else { f32::MIN_POSITIVE };
            let due = hold.filled == 0 || hold.last_push.is_none_or(|l| time - l >= gap);
            if due {
                let next = if hold.filled == 0 { 0 } else { (hold.cursor + 1) % ring };
                let origin = wgpu::Origin3d { x: 0, y: 0, z: 0 };
                let mut enc = device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("stutter hold") });
                enc.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: live_tex,
                        mip_level: 0,
                        origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &hold.slots[next].0,
                        mip_level: 0,
                        origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                );
                queue.submit(Some(enc.finish()));
                hold.cursor = next;
                hold.filled = (hold.filled + 1).min(ring);
                hold.last_push = Some(time);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> StutterParams {
        StutterParams::from_advanced(&[])
    }

    #[test]
    fn the_defaults_are_the_declared_ones() {
        let p = params();
        assert_eq!(p.rhythm, 3);
        assert_eq!(p.tempo, 128.0);
        assert_eq!(p.duty, 0.5);
        assert_eq!(p.repite_n, 0);
    }

    #[test]
    fn the_gate_is_open_for_duty_of_each_step_and_shut_after() {
        let p = StutterParams { tempo: 120.0, rhythm: 3, duty: 0.25, ..params() };
        // 1/16 at 120 bpm = 0.125 s per step.
        assert!(!gate(&p, 0.01).hidden);
        assert!(gate(&p, 0.04).hidden);
        assert!(!gate(&p, 0.126).hidden, "the next step opens again");
        let g = gate(&p, 0.0625);
        assert!(g.hidden && g.blend == 1.0 && (g.held_secs - 0.03125).abs() < 1e-4);
    }

    #[test]
    fn ritmo_sets_the_step_length() {
        let mut p = StutterParams { tempo: 120.0, duty: 0.5, ..params() };
        let shut_at = |p: &StutterParams| {
            (0..2000).map(|i| i as f32 * 0.0005).find(|t| gate(p, *t).hidden).unwrap()
        };
        p.rhythm = 0; // 1/4: 0.5 s step, shut from 0.25
        assert!((shut_at(&p) - 0.25).abs() < 0.002);
        p.rhythm = 5; // 1/32: 0.0625 s step, shut from 0.03125
        assert!((shut_at(&p) - 0.03125).abs() < 0.002);
    }

    #[test]
    fn patterns_gate_the_steps_they_say_and_invert_swaps_them() {
        let steps = |patron, invert| -> Vec<bool> {
            let p = StutterParams { patron, invert, tempo: 120.0, rhythm: 3, duty: 0.1, ..params() };
            // The middle of each step's hidden span.
            (0..8).map(|s| gate(&p, (s as f32 + 0.6) * 0.125).hidden).collect()
        };
        assert_eq!(steps(0, false), vec![true; 8]);
        assert_eq!(steps(1, false), vec![false, true, false, true, false, true, false, true]);
        assert_eq!(steps(2, false).iter().filter(|b| **b).count(), 3);
        assert_eq!(steps(3, false).iter().filter(|b| **b).count(), 5);
        for patron in 0..=4 {
            let a = steps(patron, false);
            let b = steps(patron, true);
            assert!(a.iter().zip(&b).all(|(x, y)| x != y), "invierte_ritmo swaps pattern {patron}");
        }
        let r = steps(4, false);
        assert!(r.iter().any(|b| *b) && r.iter().any(|b| !*b), "random is neither all nor none");
    }

    #[test]
    fn retardo_slides_the_grid_by_a_share_of_a_step() {
        let p = StutterParams { tempo: 120.0, rhythm: 3, duty: 0.5, retardo: 0.0, ..params() };
        let q = StutterParams { retardo: 0.5, ..p };
        // 0.09 s is inside the shut half of step 0 only when the grid is slid.
        assert!(gate(&p, 0.09).hidden);
        assert!(!gate(&q, 0.09).hidden);
    }

    #[test]
    fn jitter_moves_the_edges_and_is_deterministic() {
        let p = StutterParams { tempo: 120.0, rhythm: 3, duty: 0.5, ..params() };
        let j = StutterParams { jitter: 1.0, ..p };
        let times: Vec<f32> = (0..400).map(|i| i as f32 * 0.0025).collect();
        assert_ne!(
            times.iter().map(|t| gate(&p, *t).hidden).collect::<Vec<_>>(),
            times.iter().map(|t| gate(&j, *t).hidden).collect::<Vec<_>>()
        );
        assert_eq!(gate(&j, 0.3), gate(&j, 0.3));
    }

    #[test]
    fn desvanece_eases_the_cut_in() {
        let p = StutterParams { tempo: 120.0, rhythm: 3, duty: 0.5, fade: 0.5, ..params() };
        // Shut span starts at 0.0625 s; the ramp is a quarter step (0.03125 s).
        assert!(gate(&p, 0.0626).blend < 0.05);
        let mid = gate(&p, 0.0625 + 0.015625).blend;
        assert!(mid > 0.3 && mid < 0.7, "{mid}");
        assert_eq!(gate(&p, 0.1).blend, 1.0);
    }

    #[test]
    fn a_loop_cycles_the_newest_n_frames_oldest_first() {
        // Ring of 4, newest at slot 2, 3 real frames (slots 0, 1, 2).
        let at = |secs| slot_for(secs, 3, 2, 3, 4);
        let frame = 1.0 / LOOP_FPS;
        assert_eq!(at(0.0), 0);
        assert_eq!(at(frame * 1.5), 1);
        assert_eq!(at(frame * 2.5), 2);
        assert_eq!(at(frame * 3.5), 0, "and around again");
        assert_eq!(slot_for(5.0, 1, 2, 3, 4), 2, "a freeze is the newest");
    }

    #[test]
    fn the_tint_hue_is_a_saturated_colour() {
        assert_eq!(hue_rgb(0.0), [1.0, 0.0, 0.0]);
        let g = hue_rgb(1.0 / 3.0);
        assert!(g[1] > 0.99 && g[0] < 0.01 && g[2] < 0.01);
    }
}
