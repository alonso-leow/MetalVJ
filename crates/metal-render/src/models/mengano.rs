//! Model 19 — Mengano. A ball with a face, on a plane it can either cross or
//! stand still on while the plane goes past instead.
//!
//! # Two ways of arriving, and one slider between them
//!
//! *"moves … to specific positions using params of coordinates in two ways
//! a. by using the intermediate positions or b. by teleporting"*.
//!
//! Both are the same expression. A move from one waypoint to the next is eased
//! over a **window** of the beat it lives in, and `teletransporte` is how wide
//! that window is: the whole beat (it travels, and you watch it cross) down to
//! four per cent of it (it is simply somewhere else). Nothing branches, so the
//! float can be driven and a half-teleport is a legal, sensible picture — a
//! very fast dash — rather than an undefined state between two modes.
//!
//! The squash comes free from that. It is computed from the ball's actual speed
//! rather than scripted, so a teleport squashes hard on landing because a
//! teleport *is* a very fast arrival, and nothing had to know which of the two
//! modes it was in.
//!
//! # The travelling shot
//!
//! *"make other type of transition where is the background that moves,
//! generating like super travelling, to a new background with new params"*.
//!
//! `travelling` pins the ball in the middle and moves the world under it
//! instead, along the direction the coordinate params point — the **same two
//! params**, meaning the same thing in both modes: where this is going. The
//! background is a procedural field seeded per scene, and the world offset
//! crossing a scene boundary crossfades from one seed to the next, so a new
//! background arrives while the old one is still leaving rather than cutting.
//!
//! It is a straight line, deliberately. A curved path would need the offset to
//! be an integral over every scene since the start of the set, which is
//! unbounded work per frame for a wobble nobody asked for.
//!
//! # The eyes are Oracle's
//!
//! Its saccade — snap to a hashed target on a tick, then hold — is lifted into
//! `mengano.wgsl` unchanged, and so is the one rule that makes a blink read:
//! the lid is drawn in the surrounding skin's colour. What is not lifted is the
//! geometry, because Oracle's eye is a sphere seen close up and this one is a
//! pair of discs on a face.

use wgpu::util::DeviceExt;

use crate::model::{color_at, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema, SurfaceSpec};
use crate::shared::surface::{MaterialU, PartSet};
use crate::shared::ColorSlotGpu;

/// How far the world travels before it is somewhere else, in frame heights.
/// Roughly three frame widths at 16:9 — long enough that a scene is a place and
/// short enough that a set sees several of them.
const SCENE_SPAN: f32 = 6.0;

/// The one part a material can be laid on: the ball (the face's body).
const SURFACES: &[&str] = &["body"];

const FLOATS: &[FloatSpec] = &[
    // Where it is going. In travelling mode, the direction the world goes.
    spec("target_x", 0.0, 1.0, 0.5),
    spec("target_y", 0.0, 1.0, 0.5),
    spec("rhythm", 0.1, 4.0, 0.85),
    // The window a move is eased over — see the module header. 0 travels, 1
    // teleports, and everything between is a legal dash.
    spec("teleport", 0.0, 1.0, 0.0),
    // How far it strays from the coordinates it was given. At 0 it sits on
    // them, which is what makes them a control rather than a suggestion.
    spec("wander", 0.0, 1.0, 0.6),
    spec("size", 0.05, 0.5, 0.2),
    spec("elasticity", 0.0, 1.0, 0.7),
    // A mode, blended: at 1 the ball is pinned and the world moves. Held,
    // because a transient flipping the whole frame between the two reads as a
    // fault rather than as an effect.
    spec_held("travelling", 0.0, 1.0, 0.0, 0.3),
    spec("speed_background", 0.0, 3.0, 0.9),
    spec("scale_background", 0.4, 8.0, 2.2),
    // How much the eyes follow where the body is heading, against Oracle's
    // scan of the room.
    spec("gaze", 0.0, 1.0, 0.8),
    spec("speed_scan", 0.05, 4.0, 0.9),
    spec("blink", 0.0, 1.0, 0.5),
    spec("eyes", 0.1, 1.0, 0.55),
];

/// A body, lit. Not white: four white stops would draw a white circle with no
/// shading at all, since the stops are walked by how lit the surface is.
/// Where the personality block starts in a row: right after the three colour
/// slots, before the signal floats (the precedent `rio`'s camera floats set).
const P: usize = 50;

/// Personality: dance styles (blendable), squash and stretch, secondary
/// motion, groove, the world-space light, radiated light and the background's
/// own controls. All default to the look the model had before, except that
/// the lighting no longer turns with the body.
const PERSONALITY: &[FloatSpec] = &[
    // Beats per second the dance keeps.
    spec("dance_tempo", 0.5, 6.0, 2.0),
    spec("dance_bounce", 0.0, 1.0, 0.0),
    spec("dance_sway", 0.0, 1.0, 0.0),
    spec("dance_wobble", 0.0, 1.0, 0.0),
    spec("dance_shimmy", 0.0, 1.0, 0.0),
    spec("dance_stomp", 0.0, 1.0, 0.0),
    spec("dance_spin", 0.0, 1.0, 0.0),
    // How much the dance's vertical speed stretches the body and its landings
    // squash it (area kept).
    spec("squash_stretch", 0.0, 1.0, 0.0),
    // Secondary motion: a jelly ripple, ringing after each beat, eyes lagging.
    spec("jelly", 0.0, 1.0, 0.0),
    // The dip before a jump.
    spec("anticipation", 0.0, 1.0, 0.0),
    // Swing: the off beat arrives late.
    spec("groove", 0.0, 1.0, 0.0),
    // World-space key light direction, in turns (0.325 = up and to the left).
    spec("light_angle", 0.0, 1.0, LIGHT_TURNS),
    spec("glow_aura", 0.0, 1.0, 0.0),
    spec("glow_halo", 0.0, 1.0, 0.0),
    spec("glow_rim", 0.0, 1.0, 0.0),
    spec("glow_emissive", 0.0, 1.0, 0.0),
    spec("glow_rays", 0.0, 1.0, 0.0),
    spec("glow_hue", 0.0, 1.0, 0.08),
    spec("glow_sat", 0.0, 1.0, 0.7),
    // 0 keeps the glow steady, 1 flashes it on each beat.
    spec("glow_pulse", 0.0, 1.0, 0.0),
    spec("bg_bright", 0.0, 2.0, 1.0),
    spec("bg_tint", 0.0, 1.0, 0.0),
    spec("bg_tint_hue", 0.0, 1.0, 0.0),
    spec("bg_gradient", -1.0, 1.0, 0.0),
    spec("bg_vignette", 0.0, 1.0, 0.0),
    spec("bg_pulse", 0.0, 1.0, 0.0),
];
/// The old fixed light, (-0.45, 0.89), as a fraction of a turn.
const LIGHT_TURNS: f32 = 0.3251;

const BODY: [f32; 12] = [
    1.00, 0.90, 0.78, //
    0.99, 0.52, 0.38, //
    0.74, 0.16, 0.22, //
    0.26, 0.05, 0.14,
];
/// Sclera at the near end, pupil at the far one — the shader samples 0.06 and
/// 0.94. White here would be an eye with no pupil, which is the exact trap
/// `metal_schema::color_at` was written for.
const EYE: [f32; 12] = [
    1.00, 1.00, 1.00, //
    0.94, 0.95, 0.98, //
    0.10, 0.10, 0.16, //
    0.02, 0.02, 0.04,
];
const BACKGROUND: [f32; 12] = [
    0.04, 0.05, 0.10, //
    0.15, 0.10, 0.29, //
    0.56, 0.24, 0.42, //
    0.98, 0.66, 0.42,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("body", &BODY),
        color_at("eye", &EYE),
        // The model's own field covers every pixel, so it composites the
        // background itself instead of running the shared backdrop pass.
        color_at("background", &BACKGROUND),
        Block::Floats(PERSONALITY),
    crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
    crate::model::EFFECTS_COLOR_BLOCK,
],
};

// ---------------------------------------------------------------------------
// the motion
// ---------------------------------------------------------------------------

fn hash(x: u32) -> f32 {
    let mut h = x.wrapping_add(0x9E37_79B9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    (h >> 8) as f32 / 16_777_216.0
}

fn h2(a: u32, b: u32) -> f32 {
    hash(a.wrapping_mul(0x9E37_79B9) ^ b.wrapping_mul(0x85EB_CA6B).wrapping_add(11))
}

/// Everything one frame needs about the ball.
#[derive(Clone, Copy, Debug)]
pub struct Motion {
    pub pos: [f32; 2],
    pub vel: [f32; 2],
    /// Along the direction of travel, and across it. Their product is 1, so the
    /// ball keeps its area however hard it is squashed.
    pub stretch: f32,
    pub squash: f32,
    pub axis: [f32; 2],
    /// 1 the instant it lands, decaying. Drives the rim light.
    pub pulse: f32,
    pub radius: f32,
    /// Where the world has got to, and which two scenes are being crossfaded.
    pub world: [f32; 2],
    pub scene_a: f32,
    pub scene_b: f32,
    pub blend: f32,
    pub gaze: [f32; 2],
    /// 1 fully open, 0 shut.
    pub open: f32,
    pub dance: Dance,
}

/// The dance, one frame of it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Dance {
    /// Offset added to the ball's position.
    pub off: [f32; 2],
    /// Face/body tilt in radians.
    pub tilt: f32,
    /// World-axis scale; `sx * sy == 1`.
    pub sx: f32,
    pub sy: f32,
    pub jelly: f32,
    /// Eye lag, world space.
    pub lag: [f32; 2],
    /// 1 on the beat, decaying.
    pub beat: f32,
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Beat index and phase 0..1 with swing: even beats long, odd beats short.
fn beat_phase(t: f32, tempo: f32, groove: f32) -> (i64, f32) {
    let b = (t.max(0.0) * tempo) as f64;
    let pair = (b / 2.0).floor();
    let u = ((b / 2.0 - pair) * 2.0) as f32; // 0..2
    let s = groove.clamp(0.0, 1.0) * 0.33;
    let w = if u < 1.0 + s { u / (1.0 + s) } else { 1.0 + (u - 1.0 - s) / (1.0 - s) };
    let k = w.floor().min(1.0);
    ((pair as i64) * 2 + k as i64, (w - k).clamp(0.0, 1.0))
}

/// Position and tilt of the dance at `t`, before any stretching.
fn pose(f: &[f32], t: f32, r: f32) -> ([f32; 2], f32, i64, f32) {
    let tempo = f[P].max(0.1);
    let (idx, ph) = beat_phase(t, tempo, f[P + 10]);
    let tau = std::f32::consts::TAU;
    let b = idx as f32 + ph;
    let (bounce, sway, wob, shim, stomp, spin) =
        (f[P + 1], f[P + 2], f[P + 3], f[P + 4], f[P + 5], f[P + 6]);
    let hop = 4.0 * ph * (1.0 - ph);
    let mut x = 0.0;
    let mut y = 0.0;
    let mut tilt = 0.0;
    y += bounce * r * 1.2 * hop;
    y += stomp * r * 0.5 * hop * hop;
    x += stomp * r * 0.5 * (std::f32::consts::PI * (idx as f32 + smooth(ph))).cos();
    x += sway * r * (std::f32::consts::PI * b * 0.5).sin();
    tilt += sway * 0.25 * (std::f32::consts::PI * b * 0.5 - 0.6).sin();
    tilt += wob * 0.5 * (tau * b * 0.75).sin();
    y += wob * r * 0.1 * (tau * b * 1.5).sin();
    x += shim * r * 0.12 * (tau * b * 3.0).sin();
    tilt += shim * 0.08 * (tau * b * 3.0 + 1.0).sin();
    let turns = (b / 4.0).floor();
    tilt += spin * tau * (turns + smooth(b / 4.0 - turns));
    // The dip before the jump.
    let ant = f[P + 9].clamp(0.0, 1.0);
    y -= ant * r * 0.3 * (std::f32::consts::PI * ((ph - 0.7) / 0.3).clamp(0.0, 1.0)).sin();
    ([x, y], tilt % tau, idx, ph)
}

pub fn dance(f: &[f32], t: f32, radius: f32) -> Dance {
    let (off, tilt, _idx, ph) = pose(f, t, radius);
    let dt = 1.0 / 120.0;
    let (before, _, _, _) = pose(f, t - dt, radius);
    let v = [(off[0] - before[0]) / dt, (off[1] - before[1]) / dt];
    let tempo = f[P].max(0.1);
    let sqs = f[P + 7].clamp(0.0, 1.0);
    let jelly = f[P + 8].clamp(0.0, 1.0);
    let heavy = (f[P + 1] + f[P + 5]).min(1.0);
    let land = (-ph * 10.0).exp() * heavy * 0.5;
    let since = ph / tempo;
    let ring = (-since * 6.0).exp() * (since * 28.0).sin();
    let mut sy = 1.0 + sqs * (v[1] * 0.16).clamp(-0.4, 0.5) - sqs * 0.3 * land;
    sy *= 1.0 + jelly * 0.25 * ring * (f[P + 1] + f[P + 5] + f[P + 3]).min(1.0);
    let sy = sy.clamp(0.5, 1.8);
    let beat = (-ph * 5.0).exp();
    Dance {
        off,
        tilt,
        sx: 1.0 / sy,
        sy,
        jelly: jelly * (0.3 + (v[0].hypot(v[1]) * 0.15).min(1.0)),
        lag: [-v[0] * jelly * 0.04, -v[1] * jelly * 0.04],
        beat,
    }
}

/// The waypoint for step `k`: the coordinates the operator set, strayed from by
/// `vagar`.
fn waypoint(k: i64, target: [f32; 2], stray: f32, aspect: f32) -> [f32; 2] {
    let key = (k.rem_euclid(1 << 30)) as u32;
    let wx = (h2(key, 1) - 0.5) * 2.0 * aspect * 0.78;
    let wy = (h2(key, 2) - 0.5) * 2.0 * 0.78;
    [target[0] + (wx - target[0]) * stray, target[1] + (wy - target[1]) * stray]
}

/// Where the ball is at `t`, ignoring the travelling mode.
fn walk(t: f32, rhythm: f32, teleport: f32, target: [f32; 2], stray: f32, aspect: f32) -> [f32; 2] {
    let c = (t.max(0.0) * rhythm) as f64;
    let k = c.floor();
    let frac = (c - k) as f32;
    let k = k.clamp(0.0, 2.0e9) as i64;
    // The window the move is eased over. This is the whole of "travels" versus
    // "teleports", and it is one number.
    let window = 1.0 - teleport.clamp(0.0, 1.0) * 0.96;
    let e = (frac / window.max(1e-3)).clamp(0.0, 1.0);
    let e = e * e * (3.0 - 2.0 * e);
    let from = waypoint(k - 1, target, stray, aspect);
    let to = waypoint(k, target, stray, aspect);
    [from[0] + (to[0] - from[0]) * e, from[1] + (to[1] - from[1]) * e]
}

pub fn motion(floats: &[f32], time: f32, aspect: f32) -> Motion {
    let target = [
        (floats[0] - 0.5) * 2.0 * aspect * 0.78,
        (floats[1] - 0.5) * 2.0 * 0.78,
    ];
    let rhythm = floats[2].max(0.02);
    let teleport = floats[3].clamp(0.0, 1.0);
    let stray = floats[4].clamp(0.0, 1.0);
    let radius = floats[5].clamp(0.02, 0.5);
    let elast = floats[6].clamp(0.0, 1.0);
    let travelling = floats[7].clamp(0.0, 1.0);
    let world_speed = floats[8].max(0.0);
    let gaze_amount = floats[10].clamp(0.0, 1.0);
    let blink_rate = floats[12].clamp(0.0, 1.0) * 0.9;

    let free = walk(time, rhythm, teleport, target, stray, aspect);
    // Differenced rather than differentiated: the ease is piecewise and its
    // derivative is discontinuous at every step boundary, which would make the
    // squash flick rather than settle.
    let dt = 1.0 / 120.0;
    let before = walk(time - dt, rhythm, teleport, target, stray, aspect);
    let mut vel = [(free[0] - before[0]) / dt, (free[1] - before[1]) / dt];

    // Pinned, blended. At `travelling` 1 the ball does not move and the world
    // does; halfway, both do a bit, which is what a driven float has to mean.
    let pos = [free[0] * (1.0 - travelling), free[1] * (1.0 - travelling)];
    vel[0] *= 1.0 - travelling;
    vel[1] *= 1.0 - travelling;

    // Where the world is going: the same coordinates, read as a direction.
    let mut dir = [target[0], target[1]];
    let len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
    if len < 1e-3 {
        dir = [0.988, 0.156];
    } else {
        dir = [dir[0] / len, dir[1] / len];
    }
    let travelled = travelling * world_speed * time.max(0.0);
    let world = [dir[0] * travelled, dir[1] * travelled];
    let scene = (travelled / SCENE_SPAN).floor().clamp(0.0, 2.0e9);
    let within = travelled / SCENE_SPAN - scene;
    // Crossfaded over the last third of a scene, so the new one is arriving
    // while the old one is still on screen.
    let blend = ((within - 0.62) / 0.38).clamp(0.0, 1.0);

    // The squash. `stretch * squash == 1`, so the ball keeps its area and a
    // fast pass looks like a fast pass rather than like a bigger ball.
    let speed = (vel[0] * vel[0] + vel[1] * vel[1]).sqrt();
    let axis = if speed > 1e-4 { [vel[0] / speed, vel[1] / speed] } else { [1.0, 0.0] };
    // Seconds since the move finished, for the wobble.
    let window = 1.0 - teleport * 0.96;
    let c = (time.max(0.0) * rhythm) as f64;
    let frac = (c - c.floor()) as f32;
    let since = ((frac - window).max(0.0) / rhythm).max(0.0);
    let wobble = (-since * 6.0).exp() * (since * 30.0).sin();
    let stretch = (1.0 + (speed * 0.22).min(0.75) * elast + wobble * elast * 0.28).clamp(0.45, 2.2);
    let squash = 1.0 / stretch;
    let pulse = ((-since * 7.0).exp() * elast).clamp(0.0, 1.0);

    // The eyes look where the body is going, and Oracle's scan does the rest.
    let gaze = if speed > 1e-3 {
        [axis[0] * gaze_amount * 0.9, axis[1] * gaze_amount * 0.9]
    } else {
        [0.0, 0.0]
    };

    // A triangle blink: shut and open again inside five per cent of the cycle.
    let open = if blink_rate < 1e-3 {
        1.0
    } else {
        let phase = (time.max(0.0) * blink_rate + 0.37).fract();
        let w = 0.05;
        if phase < w {
            ((phase / w) * 2.0 - 1.0).abs()
        } else {
            1.0
        }
    };

    let dn = dance(floats, time, radius);
    let pos = [pos[0] + dn.off[0], pos[1] + dn.off[1]];

    Motion {
        dance: dn,
        pos,
        vel,
        stretch,
        squash,
        axis,
        pulse,
        radius,
        world,
        scene_a: scene,
        scene_b: scene + 1.0,
        blend,
        gaze,
        open,
    }
}

// ---------------------------------------------------------------------------
// the GPU side
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    body: ColorSlotGpu,
    eye: ColorSlotGpu,
    background: ColorSlotGpu,
    misc: [f32; 4],
    ball: [f32; 4],
    squish: [f32; 4],
    eyes: [f32; 4],
    world: [f32; 4],
    misc2: [f32; 4],
    dance: [f32; 4],
    dance2: [f32; 4],
    glow1: [f32; 4],
    glow2: [f32; 4],
    bg1: [f32; 4],
    bg2: [f32; 4],
    /// `body`'s picture and `mat_*`, see `material.wgsl`.
    mat_body: MaterialU,
}

pub struct Mengano {
    state: Built<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
    surfaces: PartSet<1>,
}

impl Mengano {
    pub fn new() -> Mengano {
        Mengano { state: Built::default(), surfaces: PartSet::new(SURFACES) }
    }

    fn build(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
        let module_wgsl = format!(
            "{}\n{}\n{}",
            include_str!("../../shaders/surface.wgsl"),
            include_str!("../../shaders/material.wgsl"),
            include_str!("../../shaders/mengano.wgsl"),
        );
        let shader = crate::shared::build_shader(device, "someone", &module_wgsl);

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mengano bgl"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mengano layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mengano pipeline"),
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

        (pipeline, bind_group_layout)
    }
}

impl Model for Mengano {
    fn id(&self) -> &'static str {
        "someone"
    }

    fn name(&self) -> &'static str {
        "Mengano"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn is_3d(&self) -> bool {
        true
    }

    fn surfaces(&self) -> &'static [&'static str] {
        self.surfaces.surfaces()
    }

    fn set_surface(&mut self, part: &str, spec: Option<SurfaceSpec>) {
        self.surfaces.set(part, spec);
    }

    fn render(
        &mut self,
        ctx: &Ctx,
        floats: &[f32],
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        let (pipeline, bind_group_layout) =
            self.state.get(format, || Self::build(ctx.device, format));

        self.surfaces.sync(ctx.device, ctx.queue);
        let m = motion(floats, ctx.time, ctx.aspect());
        let uniform = Uniform {
            body: ColorSlotGpu::from_floats(&floats[14..26]),
            eye: ColorSlotGpu::from_floats(&floats[26..38]),
            background: ColorSlotGpu::from_floats(&floats[38..50]),
            misc: [ctx.aspect(), ctx.time, m.blend, floats[9]],
            ball: [m.pos[0], m.pos[1], m.radius, m.pulse],
            squish: [m.axis[0], m.axis[1], m.stretch, m.squash],
            eyes: [m.gaze[0], m.gaze[1], m.open, floats[13]],
            world: [m.world[0], m.world[1], m.scene_a, m.scene_b],
            misc2: [floats[11], 0.0, 0.0, 0.0],
            dance: [m.dance.sx, m.dance.sy, m.dance.tilt, m.dance.jelly],
            dance2: [m.dance.lag[0], m.dance.lag[1], 0.0, floats[P + 11] * std::f32::consts::TAU],
            glow1: [floats[P + 12], floats[P + 13], floats[P + 14], floats[P + 15]],
            glow2: [
                floats[P + 16],
                floats[P + 17],
                floats[P + 18],
                1.0 + (m.dance.beat - 1.0) * floats[P + 19].clamp(0.0, 1.0),
            ],
            bg1: [floats[P + 20], floats[P + 21], floats[P + 22], floats[P + 23]],
            bg2: [floats[P + 24], floats[P + 25] * m.dance.beat * 0.5, 0.0, 0.0],
            mat_body: self.surfaces.uniform(0),
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mengano uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mengano bind group"),
            layout: bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(self.surfaces.slot(0).view()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(self.surfaces.slot(0).sampler()),
                },
            ],
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("mengano encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mengano pass"),
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
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: f32 = 16.0 / 9.0;

    fn row() -> Vec<f32> {
        SCHEMA.defaults()
    }

    #[test]
    fn the_schema_and_the_slices_render_reads_are_the_same_width() {
        assert_eq!(SCHEMA.width(), 50 + PERSONALITY.len() + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 50 + PERSONALITY.len() + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    #[test]
    fn it_stays_in_the_frame() {
        for teleport in [0.0f32, 0.5, 1.0] {
            for wander in [0.0f32, 0.5, 1.0] {
                let mut f = row();
                f[3] = teleport;
                f[4] = wander;
                let mut t = 0.0;
                while t < 40.0 {
                    let m = motion(&f, t, A);
                    assert!(
                        m.pos[0].abs() < A && m.pos[1].abs() < 1.0,
                        "left the frame at t={t}: {:?}",
                        m.pos
                    );
                    assert!(m.stretch.is_finite() && m.squash.is_finite());
                    t += 0.093;
                }
            }
        }
    }

    /// *"a. by using the intermediate positions or b. by teleporting"* — the
    /// two behaviours the ask names, told apart by the one thing that
    /// distinguishes them: whether the ball is ever anywhere in between.
    #[test]
    fn travelling_visits_the_middle_and_teleporting_does_not() {
        let mut travel = row();
        travel[3] = 0.0;
        travel[2] = 1.0; // one move a second, so a step is [k, k+1)
        travel[4] = 1.0; // and the same straying `walk` is called with below
        let mut jump = travel.clone();
        jump[3] = 1.0;

        let from = walk(3.0, 1.0, 0.0, [0.0, 0.0], 1.0, A);
        let to = walk(3.99, 1.0, 0.0, [0.0, 0.0], 1.0, A);
        let span = ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2)).sqrt();
        assert!(span > 0.2, "the two waypoints are too close to tell anything from");

        let half_travel = motion(&travel, 3.5, A).pos;
        let half_jump = motion(&jump, 3.5, A).pos;
        let off = |p: [f32; 2], q: [f32; 2]| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt();
        assert!(
            off(half_travel, from) > span * 0.2 && off(half_travel, to) > span * 0.2,
            "travelling did not pass through the middle"
        );
        assert!(
            off(half_jump, to) < span * 0.02,
            "teleporting was still on its way at the halfway point"
        );
    }

    #[test]
    fn the_squash_keeps_the_balls_area() {
        // A ball that got bigger when it moved would read as coming toward the
        // camera, which is a different animation.
        let mut f = row();
        f[6] = 1.0;
        let mut t = 0.0;
        while t < 20.0 {
            let m = motion(&f, t, A);
            assert!(
                (m.stretch * m.squash - 1.0).abs() < 1e-4,
                "stretch x squash was {} at t={t}",
                m.stretch * m.squash
            );
            t += 0.05;
        }
    }

    #[test]
    fn no_elasticity_is_a_round_ball() {
        let mut f = row();
        f[6] = 0.0;
        for t in [1.0f32, 2.3, 7.7] {
            let m = motion(&f, t, A);
            assert!((m.stretch - 1.0).abs() < 1e-5, "a rigid ball squashed anyway");
        }
    }

    #[test]
    fn the_coordinates_are_a_control_and_not_a_suggestion() {
        // `vagar` at 0 has to put it exactly where it was told, or the two
        // coordinate params are decoration.
        let mut f = row();
        f[4] = 0.0;
        f[0] = 0.8;
        f[1] = 0.25;
        let want = [(0.8 - 0.5) * 2.0 * A * 0.78, (0.25 - 0.5) * 2.0 * 0.78];
        let m = motion(&f, 9.3, A);
        assert!((m.pos[0] - want[0]).abs() < 1e-4 && (m.pos[1] - want[1]).abs() < 1e-4);
    }

    /// The travelling shot: the ball stops and the world starts.
    #[test]
    fn travelling_pins_the_ball_and_moves_the_world() {
        let mut f = row();
        f[7] = 1.0;
        f[8] = 1.5;
        let early = motion(&f, 1.0, A);
        let late = motion(&f, 9.0, A);
        assert_eq!(early.pos, [0.0, 0.0], "the ball should be pinned while travelling");
        assert_eq!(late.pos, [0.0, 0.0]);
        let moved = ((late.world[0] - early.world[0]).powi(2)
            + (late.world[1] - early.world[1]).powi(2))
        .sqrt();
        assert!(moved > 8.0, "the world barely moved: {moved}");
    }

    #[test]
    fn the_world_stands_still_when_travelling_is_off() {
        let mut f = row();
        f[7] = 0.0;
        assert_eq!(motion(&f, 30.0, A).world, [0.0, 0.0]);
    }

    #[test]
    fn travelling_far_enough_reaches_a_new_background() {
        // *"to a new background with new params"* — the seeds have to actually
        // advance, and the crossfade has to be somewhere in between.
        let mut f = row();
        f[7] = 1.0;
        f[8] = 3.0;
        let first = motion(&f, 0.5, A);
        let later = motion(&f, 30.0, A);
        assert!(later.scene_a > first.scene_a, "the scene never changed");
        let mut saw_blend = false;
        let mut t = 0.0;
        while t < 30.0 {
            let b = motion(&f, t, A).blend;
            assert!((0.0..=1.0).contains(&b));
            if b > 0.05 && b < 0.95 {
                saw_blend = true;
            }
            t += 0.05;
        }
        assert!(saw_blend, "the backgrounds cut instead of crossfading");
    }

    #[test]
    fn it_blinks_and_it_stops_blinking_when_told_to() {
        let mut f = row();
        f[12] = 1.0;
        let mut shut = false;
        let mut t = 0.0;
        while t < 20.0 {
            let m = motion(&f, t, A);
            assert!((0.0..=1.0).contains(&m.open));
            if m.open < 0.2 {
                shut = true;
            }
            t += 0.004;
        }
        assert!(shut, "never blinked");

        f[12] = 0.0;
        let mut t = 0.0;
        while t < 20.0 {
            assert_eq!(motion(&f, t, A).open, 1.0, "blinked with the blink param at zero");
            t += 0.01;
        }
    }

    #[test]
    fn asking_for_the_same_instant_twice_gives_the_same_ball() {
        let f = row();
        let a = motion(&f, 6.125, A);
        let b = motion(&f, 6.125, A);
        assert_eq!(a.pos, b.pos);
        assert_eq!(a.stretch, b.stretch);
        assert_eq!(a.open, b.open);
    }

    #[test]
    fn the_eyes_look_where_it_is_going() {
        let mut f = row();
        f[10] = 1.0;
        f[4] = 1.0;
        let mut agreed = 0;
        let mut tested = 0;
        let mut t = 0.1;
        while t < 20.0 {
            let m = motion(&f, t, A);
            let speed = (m.vel[0] * m.vel[0] + m.vel[1] * m.vel[1]).sqrt();
            if speed > 0.4 {
                tested += 1;
                if m.gaze[0] * m.vel[0] + m.gaze[1] * m.vel[1] > 0.0 {
                    agreed += 1;
                }
            }
            t += 0.05;
        }
        assert!(tested > 10, "the ball hardly moved, so nothing was tested");
        assert_eq!(agreed, tested, "the gaze pointed away from the direction of travel");
    }

    #[test]
    fn personality_floats_sit_where_the_code_reads_them() {
        let names: Vec<&str> = PERSONALITY.iter().map(|f| f.name).collect();
        assert_eq!(names[0], "dance_tempo");
        assert_eq!(names[10], "groove");
        assert_eq!(names[11], "light_angle");
        assert_eq!(names[19], "glow_pulse");
        assert_eq!(names[25], "bg_pulse");
        assert_eq!(PERSONALITY.len(), 26);
        let (a, b) = (-0.45f32, 0.89f32);
        assert!((b.atan2(a) / std::f32::consts::TAU - LIGHT_TURNS).abs() < 1e-3);
    }

    #[test]
    fn no_dance_means_no_dance() {
        let f = row();
        for t in [0.0f32, 1.3, 9.9] {
            let d = dance(&f, t, 0.2);
            assert_eq!(d.off, [0.0, 0.0]);
            assert!(d.tilt.abs() < 1e-6 && (d.sy - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn each_style_moves_the_figure_and_squash_keeps_area() {
        for (i, name) in ["bounce", "sway", "wobble", "shimmy", "stomp", "spin"].iter().enumerate() {
            let mut f = row();
            f[P + 1 + i] = 1.0;
            f[P + 7] = 1.0;
            f[P + 8] = 1.0;
            let (mut moved, mut t) = (0.0f32, 0.0);
            while t < 8.0 {
                let d = dance(&f, t, 0.2);
                moved = moved.max(d.off[0].abs() + d.off[1].abs() + d.tilt.abs());
                assert!((d.sx * d.sy - 1.0).abs() < 1e-4 && d.sy.is_finite());
                t += 0.037;
            }
            assert!(moved > 0.02, "{name} did not move anything");
        }
    }

    #[test]
    fn groove_delays_the_off_beat_and_stays_monotonic() {
        let (_, p0) = beat_phase(0.5 * 0.9, 1.0, 0.0);
        let (_, p1) = beat_phase(0.5 * 0.9, 1.0, 1.0);
        assert!(p1 < p0 + 1e-6 || p1 != p0);
        let mut last = -1.0f32;
        let mut t = 0.0;
        while t < 4.0 {
            let (i, p) = beat_phase(t, 1.0, 1.0);
            let w = i as f32 + p;
            assert!(w >= last - 1e-4, "swing ran backwards at {t}");
            last = w;
            t += 0.01;
        }
    }

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

    /// Every kind is a surface: at `mat_mix` 1 it changes the frame, and
    /// clearing `body` restores the frame byte for byte.
    #[test]
    fn a_body_material_changes_the_frame_and_clearing_it_restores_it() {
        use crate::model::{MaterialSpec, SurfaceSpec};
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (128u32, 128u32);
        let floats = SCHEMA.defaults();
        let (texture, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let mut model = Mengano::new();
        assert_eq!(model.surfaces(), &["body"]);
        let draw = |model: &mut Mengano| {
            let ctx = Ctx {
                device: &gpu.device,
                queue: &gpu.queue,
                time: 1.0,
                width: w,
                height: h,
                fondo_alpha: false,
            };
            model.render(&ctx, &floats, &view, FORMAT);
            crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &texture, w, h)
        };
        let plain = draw(&mut model);
        let kinds = ["matte", "concrete", "wood", "leather", "metal", "glass", "matrix"];
        for (kind, name) in kinds.iter().enumerate() {
            model.set_surface(
                "body",
                Some(SurfaceSpec {
                    image: None,
                    material: Some(MaterialSpec {
                        kind: kind as f32,
                        mix: 1.0,
                        scale: 1.0,
                        ..Default::default()
                    }),
                }),
            );
            let painted = draw(&mut model);
            assert_ne!(plain, painted, "{name} did not change the frame");
            model.set_surface("body", None);
            let restored = draw(&mut model);
            assert_eq!(plain, restored, "clearing {name} did not restore the frame");
        }
    }
}
