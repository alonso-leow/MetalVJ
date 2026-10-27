//! Model 22 — Lazer Bounce. The DVD logo, if it were a beam: something enters
//! from an edge at an angle, reflects off the frame however many times the
//! parameters allow, and drags a trail behind it. Each border hit throws a
//! short flash, so the bounces are readable on the beat.
//!
//! # The reflection is a reflection
//!
//! *"if the launch angle is perpendicular to an edge, the bounce should be
//! symmetric (straight) — I mean the realism of the bounce physics."*
//!
//! So `angulo` is measured **from the edge's inward normal**, not from the
//! frame's x axis, and it is what makes zero mean something: at
//! `angulo = 0, dispersion = 0` a beam enters at right angles, and
//! `v - 2(v·n)n` against an axis-aligned wall returns exactly `-v` — bit for
//! bit, not approximately — so it retraces its own path forever. Every test
//! for that behaviour is in this file, because it is the one property of the
//! model an audience can check by eye.
//!
//! An angle measured from the x axis would have made the perpendicular case
//! four different numbers depending on which edge the beam came in on, and
//! three of them would have been wrong by a hair.
//!
//! # A scene
//!
//! The launch layout is hashed per scene and every laser restarts when the
//! scene does, so the rearrangement is a visible cut rather than a slow lie.
//!
//! Inside a scene each beam gets [`LIVES_PER_SCENE`] **lives**: it runs out of
//! bounces, its trail drains, and it comes back from a different edge. Without
//! that the frame emptied out a few seconds into every scene and stayed empty
//! until the cut. The lives divide the scene exactly, so no flight is ever
//! halfway through when the scene cuts.
//!
//! Everything else follows the suite's rule: a frame is a pure function of
//! `(floats, time)`, so there is nothing to lose when automix parks the model.

use wgpu::util::DeviceExt;

use crate::model::{color_at, background, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::backdrop::Backdrop;
use crate::shared::ColorSlotGpu;

pub const MAX_LAZERS: usize = 16;
/// The most segments one trail may be cut into. A trail long enough to wrap
/// several bounces is legal; one that would need a hundred quads is a
/// parameter combination, not a picture.
const MAX_TRAIL_SEGMENTS: usize = 40;
/// How far a beam is traced before the tracer gives up, in frame widths.
const MAX_PATH: f32 = 80.0;
/// Pushed off the surface after a bounce, so the next intersection test cannot
/// find the surface it just left.
const EPS: f32 = 1e-4;
/// How long a border-hit flash lasts, in seconds.
const FLASH_SECONDS: f32 = 0.45;
/// How many times each beam relaunches inside one scene. A divisor of the
/// scene, so no flight is ever cut in half by the obstacles rearranging.
const LIVES_PER_SCENE: u32 = 3;

const FLOATS: &[FloatSpec] = &[
    spec_held("lasers", 1.0, MAX_LAZERS as f32, 5.0, 0.25),
    // From the edge's inward normal, in quarter turns. 0 is perpendicular; the
    // range stops short of ±1 because exactly ±1 is a beam running along the
    // edge it launched from, which never enters the frame.
    spec("angle", -0.92, 0.92, 0.34),
    spec("dispersion", 0.0, 1.0, 0.45),
    spec("speed", 0.3, 8.0, 1.9),
    spec("wake", 0.05, 6.0, 2.4),
    // Structural: how many reflections a beam is allowed. It sets how long a
    // path is, and a path that changed length on every transient would make
    // the trails flicker in and out rather than travel.
    spec_held("bounces", 1.0, 24.0, 12.0, 0.35),
    spec("thickness", 0.002, 0.05, 0.016),
    spec("shine", 0.0, 1.0, 0.7),
    // How far apart the beams' colours are picked from the slot. At 0 they are
    // all the same beam in different places.
    spec("variety", 0.0, 1.0, 0.6),
    spec_held("scene", 4.0, 40.0, 14.0, 0.1),
    // Appended, never inserted: how bright the flash thrown at each border hit
    // is, and how far the beams' halo reaches.
    spec("flash", 0.0, 1.0, 0.6),
    spec("glow", 0.0, 1.0, 0.375),
];

const LASER: [f32; 12] = [
    0.30, 0.95, 1.00, //
    0.55, 0.45, 1.00, //
    1.00, 0.30, 0.45, //
    1.00, 0.92, 0.45,
];
pub static SCHEMA: Schema = Schema {
    blocks: &[Block::Floats(FLOATS), color_at("laser", &LASER), background(), crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK, crate::model::EFFECTS_COLOR_BLOCK],
};

// ---------------------------------------------------------------------------
// geometry
// ---------------------------------------------------------------------------

type V2 = [f32; 2];

fn sub(a: V2, b: V2) -> V2 {
    [a[0] - b[0], a[1] - b[1]]
}

fn add(a: V2, b: V2) -> V2 {
    [a[0] + b[0], a[1] + b[1]]
}

fn scale(a: V2, k: f32) -> V2 {
    [a[0] * k, a[1] * k]
}

fn dot(a: V2, b: V2) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn norm(a: V2) -> V2 {
    let l = dot(a, a).sqrt();
    if l < 1e-9 {
        [1.0, 0.0]
    } else {
        [a[0] / l, a[1] / l]
    }
}

/// `v` reflected in the surface whose normal is `n`. The one line the model is
/// judged on; `n` must be a unit vector.
fn reflect(v: V2, n: V2) -> V2 {
    let k = 2.0 * dot(v, n);
    [v[0] - k * n[0], v[1] - k * n[1]]
}

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
    hash(a.wrapping_mul(0x9E37_79B9) ^ b.wrapping_mul(0x85EB_CA6B).wrapping_add(3))
}

struct Hit {
    t: f32,
    normal: V2,
}

/// The nearest wall of the frame in front of `p` along `d`.
fn nearest(p: V2, d: V2, aspect: f32) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    let mut consider = |t: f32, normal: V2| {
        if t <= EPS || !t.is_finite() {
            return;
        }
        if best.as_ref().is_none_or(|h| t < h.t) {
            best = Some(Hit { t, normal });
        }
    };

    // The frame. Its normals point inward, which is what makes a perpendicular
    // launch reflect to exactly its own reverse.
    if d[0] > 1e-9 {
        consider((aspect - p[0]) / d[0], [-1.0, 0.0]);
    } else if d[0] < -1e-9 {
        consider((-aspect - p[0]) / d[0], [1.0, 0.0]);
    }
    if d[1] > 1e-9 {
        consider((1.0 - p[1]) / d[1], [0.0, -1.0]);
    } else if d[1] < -1e-9 {
        consider((-1.0 - p[1]) / d[1], [0.0, 1.0]);
    }

    best
}

/// The whole path a beam takes, as points with the arc length at each one.
pub fn trace(
    start: V2,
    dir: V2,
    bounces: usize,
    aspect: f32,
) -> (Vec<V2>, Vec<f32>) {
    let mut points = vec![start];
    let mut lengths = vec![0.0f32];
    let mut p = start;
    let mut d = norm(dir);
    let mut travelled = 0.0f32;

    for _ in 0..=bounces {
        let Some(hit) = nearest(p, d, aspect) else { break };
        let next = add(p, scale(d, hit.t));
        travelled += hit.t;
        points.push(next);
        lengths.push(travelled);
        if travelled >= MAX_PATH {
            break;
        }
        d = norm(reflect(d, hit.normal));
        // Off the surface, so the next test cannot re-find it at t≈0.
        p = add(next, scale(d, EPS));
    }
    (points, lengths)
}

/// Where a laser is launched from: a point on one of the four edges, and a
/// direction turned `turn` quarter-turns off that edge's inward normal.
pub fn launch(edge: usize, along: f32, turn: f32, aspect: f32) -> (V2, V2) {
    // Kept a hair inside, so the launch point is not simultaneously on the
    // wall it would immediately be reflected by.
    let inset = EPS * 4.0;
    let (p, n) = match edge % 4 {
        0 => ([-aspect + inset, (along - 0.5) * 1.8], [1.0f32, 0.0f32]),
        1 => ([aspect - inset, (along - 0.5) * 1.8], [-1.0, 0.0]),
        2 => ([(along - 0.5) * 1.8 * aspect, -1.0 + inset], [0.0, 1.0]),
        _ => ([(along - 0.5) * 1.8 * aspect, 1.0 - inset], [0.0, -1.0]),
    };
    let a = turn * std::f32::consts::FRAC_PI_2;
    let (sin, cos) = a.sin_cos();
    // Rotating the *normal* is what makes `turn == 0` exactly perpendicular on
    // all four edges rather than on one of them.
    let d = [n[0] * cos - n[1] * sin, n[0] * sin + n[1] * cos];
    (p, d)
}

// ---------------------------------------------------------------------------
// the frame
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, Debug, PartialEq)]
pub struct Beam {
    pub seg: [f32; 4],
    pub shape: [f32; 4],
    pub ca: [f32; 4],
    pub cb: [f32; 4],
}

fn scene_at(scene_seconds: f32, time: f32) -> (u32, f32) {
    let s = scene_seconds.max(1.0);
    let t = time.max(0.0);
    let index = (t / s).floor().clamp(0.0, 2.0e9);
    (index as u32, t - index * s)
}

/// Every capsule in the frame: the figures' outlines first, then whatever part
/// of each trail is currently visible.
pub fn beams(floats: &[f32], time: f32, aspect: f32) -> Vec<Beam> {
    let n = (floats[0].round() as usize).clamp(1, MAX_LAZERS);
    let angle = floats[1];
    let spread = floats[2].clamp(0.0, 1.0);
    let speed = floats[3].max(0.05);
    let tail_len = floats[4].max(0.02);
    let bounces = (floats[5].round() as usize).clamp(1, 24);
    let width = floats[6].clamp(0.001, 0.05);
    let variety = floats[8].clamp(0.0, 1.0);
    let scene_seconds = floats[9].clamp(4.0, 40.0);
    let flash = floats[10].clamp(0.0, 1.0);

    let laser_slot = ColorSlotGpu::from_floats(&floats[12..24]);

    let (scene, local) = scene_at(scene_seconds, time);
    let seed = scene.wrapping_mul(0x9E37_79B9).wrapping_add(29);

    let mut out: Vec<Beam> = Vec::with_capacity(n * 8);

    // A beam that has run out of path and drained its trail would otherwise be
    // off until the scene cut, and a scene is fifteen seconds — long enough for
    // the frame to empty out and stay that way. So each beam gets several
    // *lives* inside one scene, relaunching from somewhere else each time. The
    // lives divide the scene exactly, so nothing is ever mid-flight across a
    // cut, which is what would make the obstacles appear to teleport under it.
    let life_len = scene_seconds / LIVES_PER_SCENE as f32;

    for i in 0..n {
        let id = i as u32;
        // Staggered inside a life, so they do not all leave at once.
        let stagger = h2(seed, id.wrapping_add(40)) * life_len * 0.55;
        if local < stagger {
            continue;
        }
        let life = ((local - stagger) / life_len).floor().max(0.0) as u32;
        let start_at = life as f32 * life_len + stagger;
        // Keyed on the life as well as the beam, so a relaunch is a new beam
        // from a new edge rather than the same one played again.
        let key = id.wrapping_mul(101).wrapping_add(life);

        let edge = (h2(seed, key.wrapping_add(10)) * 4.0) as usize;
        let along = h2(seed, key.wrapping_add(20));
        let turn = (angle + (h2(seed, key.wrapping_add(30)) - 0.5) * spread).clamp(-0.96, 0.96);
        let (p0, d0) = launch(edge, along, turn, aspect);

        let (points, lengths) = trace(p0, d0, bounces, aspect);
        let total = *lengths.last().unwrap_or(&0.0);
        if points.len() < 2 || total <= 0.0 {
            continue;
        }

        let travelled = (local - start_at) * speed;
        let head = travelled.min(total);
        let tail = travelled - tail_len;
        if tail >= total {
            // Run out of path and the trail has drained. It is off until the
            // scene cuts.
            continue;
        }

        // Its two colours, out of the one slot. `variedad` at 0 puts every
        // beam at the same place in it, which is what "all the same colour"
        // has to mean when there is one palette.
        let base = (0.5 + (h2(seed, id.wrapping_add(60)) - 0.5) * variety).clamp(0.0, 1.0);
        let tail_c = laser_slot.grad4(base);
        let head_c = laser_slot.grad4((base + 0.25).min(1.0));

        let mut emitted = 0usize;
        for k in 0..points.len() - 1 {
            if emitted >= MAX_TRAIL_SEGMENTS {
                break;
            }
            let (s0, s1) = (lengths[k], lengths[k + 1]);
            let from = s0.max(tail);
            let to = s1.min(head);
            if to <= from {
                continue;
            }
            let span = (s1 - s0).max(1e-6);
            let lerp = |u: f32| {
                let f = ((u - s0) / span).clamp(0.0, 1.0);
                [
                    points[k][0] + (points[k + 1][0] - points[k][0]) * f,
                    points[k][1] + (points[k + 1][1] - points[k][1]) * f,
                ]
            };
            let a = lerp(from);
            let b = lerp(to);
            // 0 at the tail of the trail, 1 at the head, so a beam has a
            // direction you can see without watching it move.
            let fade = |u: f32| (((u - tail) / tail_len).clamp(0.0, 1.0)).powf(0.7);
            let (fa, fb) = (fade(from), fade(to));
            out.push(Beam {
                seg: [a[0], a[1], b[0], b[1]],
                shape: [width, fa, fb, 1.0],
                ca: [
                    tail_c[0] + (head_c[0] - tail_c[0]) * fa,
                    tail_c[1] + (head_c[1] - tail_c[1]) * fa,
                    tail_c[2] + (head_c[2] - tail_c[2]) * fa,
                    1.0,
                ],
                cb: [
                    tail_c[0] + (head_c[0] - tail_c[0]) * fb,
                    tail_c[1] + (head_c[1] - tail_c[1]) * fb,
                    tail_c[2] + (head_c[2] - tail_c[2]) * fb,
                    1.0,
                ],
            });
            emitted += 1;
        }

        // A flash where the head has struck a border, fading over ~0.45 s.
        // Drawn as a tiny capsule fattened by the flash, so it shares the
        // beam's halo and its additive blend.
        if flash > 0.001 {
            for k in 1..points.len() {
                let age = (travelled - lengths[k]) / speed;
                if !(0.0..FLASH_SECONDS).contains(&age) || lengths[k] > head + 1e-4 {
                    continue;
                }
                let f = (1.0 - age / FLASH_SECONDS).powi(2) * flash;
                let q = points[k];
                out.push(Beam {
                    seg: [q[0], q[1], q[0] + 1e-3, q[1]],
                    shape: [width * (1.0 + 2.5 * f), f, f, 1.0],
                    ca: [head_c[0], head_c[1], head_c[2], 1.0],
                    cb: [head_c[0], head_c[1], head_c[2], 1.0],
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// the GPU side
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    misc: [f32; 4],
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    backdrop: Backdrop,
}

pub struct Lazer {
    gpu: Built<Gpu>,
}

impl Lazer {
    pub fn new() -> Lazer {
        Lazer { gpu: Built::default() }
    }

    fn build(device: &wgpu::Device, format: wgpu::TextureFormat) -> Gpu {
        let shader =
            crate::shared::build_shader(device, "laser", include_str!("../../shaders/lazer.wgsl"));

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("laser bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("laser layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Beam>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("laser pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(instance_layout)],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // Additive. Beams cross each other constantly and a beam
                    // that occluded another would read as one of them ending.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::OVER,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Gpu { pipeline, bind_group_layout, backdrop: Backdrop::new(device, format) }
    }
}

impl Model for Lazer {
    fn id(&self) -> &'static str {
        "laser"
    }

    fn name(&self) -> &'static str {
        "Lazer Bounce"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn backdrop(&self) -> bool {
        true
    }

    fn render(
        &mut self,
        ctx: &Ctx,
        floats: &[f32],
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        let gpu = self.gpu.get(format, || Self::build(ctx.device, format));
        gpu.backdrop.draw(
            ctx,
            target,
            ColorSlotGpu::from_floats(&floats[24..36]),
            0.0,
            ctx.fondo_alpha,
        );

        let instances = beams(floats, ctx.time, ctx.aspect());
        if instances.is_empty() {
            return;
        }

        let uniform = Uniform { misc: [ctx.aspect(), floats[7].clamp(0.0, 1.0), floats[11].clamp(0.0, 1.0), 0.0] };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("laser uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let ibuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("laser instances"),
            contents: bytemuck::cast_slice(&instances),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("laser bind group"),
            layout: &gpu.bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("laser encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("laser pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, ibuf.slice(..));
            pass.draw(0..4, 0..instances.len() as u32);
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
        assert_eq!(SCHEMA.width(), 36 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 36 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    /// The one property an audience checks by eye, on all four edges. A beam
    /// launched at right angles must come back along the line it went out on —
    /// not close to it.
    #[test]
    fn a_perpendicular_launch_bounces_straight_back_on_every_edge() {
        for edge in 0..4 {
            let (p, d) = launch(edge, 0.5, 0.0, A);
            let (points, _) = trace(p, d, 3, A);
            assert!(points.len() >= 3, "edge {edge} did not bounce at all");
            // Out and back along one line: every point is on the launch line.
            let n = [-d[1], d[0]];
            for (i, q) in points.iter().enumerate() {
                let off = dot(sub(*q, p), n).abs();
                assert!(off < 1e-3, "edge {edge} point {i} drifted {off} off the launch line");
            }
            // And the second point is not the first: it actually crossed.
            let span = sub(points[1], points[0]);
            assert!(dot(span, span).sqrt() > 1.0, "edge {edge} barely moved before bouncing");
        }
    }

    #[test]
    fn a_perpendicular_launch_retraces_itself_forever() {
        // Out, back, out, back. If the reflection were approximate this walks
        // off the line after a few bounces, which is exactly how a nearly-right
        // reflection looks at a venue after thirty seconds.
        let (p, d) = launch(0, 0.37, 0.0, A);
        let (points, _) = trace(p, d, 9, A);
        for (i, q) in points.iter().enumerate() {
            assert!(
                (q[1] - p[1]).abs() < 1e-4,
                "bounce {i} left the launch row by {}",
                (q[1] - p[1]).abs()
            );
        }
        // It alternates between the two walls rather than creeping.
        for pair in points.windows(2) {
            assert!((pair[0][0] - pair[1][0]).abs() > A, "a bounce did not cross the frame");
        }
    }

    #[test]
    fn reflection_is_the_textbook_one() {
        // 45° into a vertical wall comes out at 45°, mirrored in x only.
        let v = norm([1.0, 1.0]);
        let out = reflect(v, [-1.0, 0.0]);
        assert!((out[0] + v[0]).abs() < 1e-6, "x did not reverse");
        assert!((out[1] - v[1]).abs() < 1e-6, "y should be untouched");
        // Speed is conserved, which is what stops a long path drifting.
        assert!((dot(out, out).sqrt() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_beam_never_leaves_the_frame() {
        // The failure this model has: an intersection missed at a corner, after
        // which the beam is outside and the trail is a line across the void.
        for angle in [-0.92f32, -0.5, 0.0, 0.34, 0.92] {
            for edge in 0..4 {
                let (p, d) = launch(edge, 0.42, angle, A);
                let (points, _) = trace(p, d, 24, A);
                for (i, q) in points.iter().enumerate() {
                    assert!(
                        q[0].abs() <= A + 1e-2 && q[1].abs() <= 1.0 + 1e-2,
                        "edge {edge} angle {angle}: point {i} at {q:?} left the frame"
                    );
                }
            }
        }
    }

    #[test]
    fn a_traced_path_only_ever_gets_longer() {
        let (p, d) = launch(2, 0.3, 0.4, A);
        let (_, lengths) = trace(p, d, 20, A);
        for pair in lengths.windows(2) {
            assert!(pair[1] > pair[0], "arc length went backwards: {pair:?}");
        }
    }

    #[test]
    fn a_border_hit_throws_a_flash_and_flash_zero_throws_none() {
        // One slow beam at a perpendicular launch: the head reaches the far
        // wall after about 2*aspect/speed seconds.
        let mut f = row();
        f[0] = 1.0;
        f[2] = 0.0;
        f[1] = 0.0;
        f[3] = 1.0;
        let t = (0..400)
            .map(|i| i as f32 * 0.05)
            .find(|&t| {
                let mut on = f.clone();
                on[10] = 1.0;
                let mut off = f.clone();
                off[10] = 0.0;
                beams(&on, t, A).len() > beams(&off, t, A).len()
            });
        assert!(t.is_some(), "no flash was ever thrown at a border");
    }

    #[test]
    fn a_trail_is_drawn_head_bright_and_tail_dim() {
        let f = row();
        let all = beams(&f, 9.0, A);
        let trail: Vec<&Beam> = all.iter().filter(|b| b.shape[1] < 0.99 || b.shape[2] > 0.0).collect();
        assert!(!trail.is_empty(), "nothing was drawn at all");
        for b in &all {
            assert!(b.shape[1] >= 0.0 && b.shape[1] <= 1.0);
            assert!(b.shape[2] >= 0.0 && b.shape[2] <= 1.0);
            assert!(b.seg.iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn more_lazers_is_more_lazers() {
        let mut few = row();
        few[0] = 1.0;
        let mut many = row();
        many[0] = 16.0;
        // Late enough in the scene that everything has launched.
        let t = few[9] * 0.9;
        assert!(
            beams(&many, t, A).len() > beams(&few, t, A).len(),
            "asking for sixteen beams drew no more than asking for one"
        );
    }

    #[test]
    fn asking_for_the_same_instant_twice_draws_the_same_beams() {
        let f = row();
        assert_eq!(beams(&f, 5.5, A), beams(&f, 5.5, A));
    }

    #[test]
    fn a_scene_cuts_and_relaunches_the_beams() {
        let f = row();
        let a = beams(&f, 5.0, A);
        let b = beams(&f, 5.0 + f[9], A);
        assert_ne!(a, b, "two scenes drew the same beams");
    }
}
