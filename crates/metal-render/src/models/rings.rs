//! Model 20 — Rings. A ball loose inside a stack of turning rings, each with
//! one gap in it; a ring the ball gets through is a ring that is gone.
//!
//! The half-screen video the ask names it after is a countdown you cannot look
//! away from: the ball is trapped by `n` walls, the gaps turn past it, and the
//! only question in the frame is how many are left. Everything the parameters
//! touch is one of the three things that answer it — how fast the gaps come
//! round (`velocidad`, `alternar`), how wide they are (`abertura`), and how
//! hard the ball is thrown at them (`rebote`, `gravedad`, `impulso`).
//!
//! # The round, and why there is one
//!
//! A frame here is a pure function of `(floats, time)` like the rest of the
//! suite, and a bouncing ball is not: it is an integration from a start. The
//! reconciliation is that **the start is never more than one round away**.
//! `ronda` seconds is a round; `floor(time / ronda)` seeds it; the solver
//! integrates from the beginning of the current one on every frame. So there is
//! no accumulated state to lose, nothing to re-seed when automix parks the
//! model for thirty seconds (`models::turing`'s `SEEK_GAP` is exactly that
//! wound), and scrubbing the clock backwards lands on the same picture it did
//! going forwards.
//!
//! It costs an integration per frame, and it is cheap for a reason worth
//! stating: **the ball can only be touching one ring** — the innermost one
//! still standing — so a step is O(1) in the ring count, not O(n). At the
//! default round that is a few thousand steps of about twenty flops.
//!
//! If the rings all die before the round is up, the ball free-bounces off the
//! outer boundary until it ends. That gap is deliberate: the payoff of watching
//! the last ring go is the moment nothing is in the way.

use wgpu::util::DeviceExt;

use crate::model::{color_at, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::ColorSlotGpu;

/// The most rings the uniform's array can carry. Raising it means raising the
/// array in `rings.wgsl` too — they are two statements of one number.
pub const MAX_RINGS: usize = 12;

/// Integration step. Fixed, and not the frame time: the frame rate here is
/// whatever the browser and the model list leave over, and a bounce solved at
/// 30 Hz and at 120 Hz are different bounces.
const DT: f32 = 1.0 / 180.0;
/// A hard ceiling on the work one frame may do, whatever `ronda` says.
const MAX_STEPS: usize = 16_384;

/// Where the innermost and outermost rings sit. The outer one is inside the
/// short edge of the frame, so the stack is whole on any output.
const R_INNER: f32 = 0.20;
const R_OUTER: f32 = 0.93;

const FLOATS: &[FloatSpec] = &[
    // Structural: the number of walls *is* the model. Driven freely it would
    // re-deal the stack on every transient and the round would never resolve.
    spec_held("layers", 2.0, MAX_RINGS as f32, 6.0, 0.15),
    // Signed, so the sign is the direction the ask asks for and zero is a
    // stack that does not turn at all.
    spec("speed", -3.0, 3.0, 0.75),
    // How much of the stack counter-rotates. At 0 every ring turns the same
    // way; at 1 they alternate, which is what makes the gaps line up rarely.
    spec("alternate", 0.0, 1.0, 1.0),
    spec("aperture", 0.02, 0.45, 0.15),
    // The ask's *"bounce magnitude"*. Above 1 the ball gains speed on every
    // hit, which is the whole engine of the format it is borrowed from.
    spec("bounce", 0.55, 1.35, 1.03),
    spec("gravity", 0.0, 2.5, 0.85),
    spec("impulse", 0.15, 2.5, 0.9),
    spec("ball", 0.012, 0.09, 0.032),
    spec("thickness", 0.006, 0.06, 0.018),
    spec("shine", 0.0, 1.0, 0.5),
    // Also structural: this is the clock the whole model is defined on, and a
    // round length that moved with the music would restart the stack mid-round.
    spec_held("round", 8.0, 60.0, 26.0, 0.1),
];

/// Not white, for the reason every slot in this suite that opens somewhere else
/// is not white: four white stops here would draw twelve identical white rings
/// and one white ball, and the model's whole subject is telling them apart.
const RINGS_COLOR: [f32; 12] = [
    0.98, 0.94, 0.88, //
    0.95, 0.35, 0.18, //
    0.55, 0.10, 0.10, //
    0.30, 0.32, 0.38,
];
const BALL_COLOR: [f32; 12] = [
    1.00, 1.00, 0.98, //
    1.00, 0.82, 0.55, //
    0.95, 0.35, 0.18, //
    0.45, 0.10, 0.08,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("rings", &RINGS_COLOR),
        color_at("ball", &BALL_COLOR),
        // The model's own field covers every pixel, so it composites the
        // background itself rather than running the shared backdrop pass — the
        // same call Ascii, Cyberspace and Solvent make.
        crate::model::background(),
    crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
    crate::model::EFFECTS_COLOR_BLOCK,
],
};

// ---------------------------------------------------------------------------
// the solve
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

/// The parameters a round is solved against, pulled out of the float row once.
#[derive(Clone, Copy)]
struct Setup {
    rings: usize,
    speed: f32,
    alternate: f32,
    gap: f32,
    bounce: f32,
    gravity: f32,
    impulse: f32,
    ball: f32,
    thick: f32,
    round: f32,
}

impl Setup {
    fn read(f: &[f32]) -> Setup {
        Setup {
            rings: (f[0].round() as usize).clamp(2, MAX_RINGS),
            speed: f[1],
            alternate: f[2].clamp(0.0, 1.0),
            gap: f[3].clamp(0.02, 0.45),
            bounce: f[4].clamp(0.55, 1.35),
            gravity: f[5].max(0.0),
            impulse: f[6].max(0.05),
            ball: f[7].clamp(0.008, 0.09),
            thick: f[8].clamp(0.004, 0.06),
            round: f[10].clamp(4.0, 60.0),
        }
    }

    fn radius(&self, i: usize) -> f32 {
        if self.rings <= 1 {
            return R_OUTER;
        }
        R_INNER + (R_OUTER - R_INNER) * (i as f32 / (self.rings - 1) as f32)
    }

    /// How fast ring `i` turns, and which way. `alternar` blends between "all
    /// the same" and "every other one the other way"; the small per-ring
    /// variation is what stops the whole stack reading as one rigid object.
    fn omega(&self, i: usize, seed: u32) -> f32 {
        let flip = if i % 2 == 1 { 1.0 - 2.0 * self.alternate } else { 1.0 };
        self.speed * flip * (0.75 + 0.5 * hash(seed ^ (i as u32).wrapping_mul(0x51ED_2701)))
    }

    fn gap_at(&self, i: usize, seed: u32, t: f32) -> f32 {
        let start = hash(seed ^ (i as u32).wrapping_mul(0x27D4_EB2F)) * std::f32::consts::TAU;
        start + self.omega(i, seed) * t
    }

    /// The circle the ball is held inside while ring `i` is the innermost one
    /// standing.
    fn wall(&self, i: usize) -> f32 {
        (self.radius(i) - self.thick * 0.5 - self.ball).max(self.ball * 1.5)
    }
}

/// What one frame needs to know: where the ball is, and what is left.
pub struct Solved {
    pub ball: [f32; 2],
    pub speed: f32,
    /// Per ring: radius, gap centre, gap half-width, 0..1 alive.
    pub rings: [[f32; 4]; MAX_RINGS],
    pub count: usize,
}

/// Integrates one round up to `local` seconds.
fn solve(s: Setup, seed: u32, local: f32) -> Solved {
    let tau = std::f32::consts::TAU;
    let gap_half = s.gap * std::f32::consts::PI; // `abertura` is a share of a half turn

    // A stack, thrown at from the middle. `generation` counts how many stacks
    // this round has already got through — clearing one deals another
    // immediately rather than leaving the frame empty until the round ends,
    // because the reward for watching the last ring go is the next one.
    let mut generation = 0u32;
    let mut gen_seed = seed;
    let mut born_at = 0.0f32;
    let throw = |g: u32| {
        let a = hash(seed ^ g.wrapping_mul(0x51ED_2701) ^ 0x1234_5678) * tau;
        [a.cos() * s.impulse, a.sin() * s.impulse]
    };
    let mut p = [0.0f32, 0.0];
    let mut v = throw(0);

    // `None` is "still standing". A dead ring keeps the instant it died so it
    // can be seen going rather than blinking out.
    let mut died: [Option<f32>; MAX_RINGS] = [None; MAX_RINGS];
    let mut current = 0usize; // the innermost ring still standing

    let steps = ((local.max(0.0) / DT).ceil() as usize).min(MAX_STEPS);
    let mut t = 0.0f32;
    for _ in 0..steps {
        v[1] -= s.gravity * DT;
        p[0] += v[0] * DT;
        p[1] += v[1] * DT;
        t += DT;

        if current >= s.rings {
            generation = generation.wrapping_add(1);
            gen_seed = seed ^ generation.wrapping_mul(0x9E37_79B9);
            born_at = t;
            died = [None; MAX_RINGS];
            current = 0;
            p = [0.0, 0.0];
            v = throw(generation);
            continue;
        }

        let wall = s.wall(current);
        let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
        if r < wall || r <= 1e-6 {
            continue;
        }

        let angle = p[1].atan2(p[0]);
        let through = {
            let centre = s.gap_at(current, gen_seed, t);
            let mut d = angle - centre;
            d -= tau * ((d + std::f32::consts::PI) / tau).floor();
            d.abs() < gap_half
        };

        if through {
            died[current] = Some(t);
            current += 1;
            // Not reflected: it is on its way out, and the next wall is further
            // off, so nothing else has to happen this step.
            continue;
        }

        // A wall. Reflect about the inward normal and put the ball back where
        // it was legal, so it cannot tunnel out over several slow steps.
        let n = [-p[0] / r, -p[1] / r];
        let dot = v[0] * n[0] + v[1] * n[1];
        if dot < 0.0 {
            v[0] -= 2.0 * dot * n[0];
            v[1] -= 2.0 * dot * n[1];
        }
        v[0] *= s.bounce;
        v[1] *= s.bounce;
        // `rebote` above 1 is the point of the format, and unbounded it turns
        // the ball into a line after a minute. This is where it stops.
        let sp = (v[0] * v[0] + v[1] * v[1]).sqrt();
        let cap = 4.5;
        if sp > cap {
            v[0] *= cap / sp;
            v[1] *= cap / sp;
        }
        p[0] = n[0] * -wall;
        p[1] = n[1] * -wall;
    }

    // A stack fades in over a third of a second so a fresh deal reads as one
    // event rather than as the last one blinking.
    let fade_in = ((t - born_at) / 0.3).clamp(0.0, 1.0);
    let mut rings = [[0.0f32; 4]; MAX_RINGS];
    for i in 0..s.rings {
        let (radius, alive) = match died[i] {
            None => (s.radius(i), fade_in),
            Some(when) => {
                // 0.4 s of getting bigger and thinner, so a ring visibly breaks
                // rather than being missing the next time you look.
                let k = ((t - when) / 0.4).clamp(0.0, 1.0);
                (s.radius(i) * (1.0 + 0.25 * k), 1.0 - k)
            }
        };
        rings[i] = [radius, s.gap_at(i, gen_seed, t), gap_half, alive];
    }

    let speed = (v[0] * v[0] + v[1] * v[1]).sqrt();
    Solved { ball: p, speed: (speed / 3.0).clamp(0.0, 1.0), rings, count: s.rings }
}

/// The round the clock is in, and how far into it — the whole of this model's
/// relationship with time.
fn round_at(round_seconds: f32, time: f32) -> (u32, f32) {
    let r = round_seconds.max(1.0);
    let t = time.max(0.0);
    let index = (t / r).floor().clamp(0.0, 2.0e9);
    (index as u32, t - index * r)
}

pub fn solved(floats: &[f32], time: f32) -> Solved {
    let s = Setup::read(floats);
    let (round, local) = round_at(s.round, time);
    solve(s, round.wrapping_mul(0x9E37_79B9).wrapping_add(17), local)
}

// ---------------------------------------------------------------------------
// the GPU side
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    rings_color: ColorSlotGpu,
    ball_color: ColorSlotGpu,
    background: ColorSlotGpu,
    misc: [f32; 4],
    ball: [f32; 4],
    rings: [[f32; 4]; MAX_RINGS],
}

pub struct Rings {
    state: Built<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
}

impl Rings {
    pub fn new() -> Rings {
        Rings { state: Built::default() }
    }

    fn build(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
        let shader =
            crate::shared::build_shader(device, "rings", include_str!("../../shaders/rings.wgsl"));

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rings bgl"),
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
            label: Some("rings layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rings pipeline"),
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

impl Model for Rings {
    fn id(&self) -> &'static str {
        "rings"
    }

    fn name(&self) -> &'static str {
        "Rings"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
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

        let s = solved(floats, ctx.time);
        let uniform = Uniform {
            rings_color: ColorSlotGpu::from_floats(&floats[11..23]),
            ball_color: ColorSlotGpu::from_floats(&floats[23..35]),
            background: ColorSlotGpu::from_floats(&floats[35..47]),
            misc: [ctx.aspect(), floats[8], floats[9], s.count as f32],
            ball: [s.ball[0], s.ball[1], floats[7], s.speed],
            rings: s.rings,
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rings uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rings bind group"),
            layout: bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("rings encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rings pass"),
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

    fn row() -> Vec<f32> {
        SCHEMA.defaults()
    }

    #[test]
    fn the_schema_and_the_slices_render_reads_are_the_same_width() {
        // 11 floats + three colour slots. `render` slices `[11..23]`, `[23..35]`
        // and `[35..47]` by hand.
        assert_eq!(SCHEMA.width(), 47 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 47 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    #[test]
    fn the_ball_never_leaves_the_frame() {
        // The bug this model can have that nothing else catches: a bounce that
        // tunnels, after which the ball is outside every ring and the picture
        // is a still life. Walked across every slider, not just the defaults.
        for layers in [2.0f32, 6.0, 12.0] {
            for bounce in [0.55f32, 1.03, 1.35] {
                for gravity in [0.0f32, 0.85, 2.5] {
                    let mut f = row();
                    f[0] = layers;
                    f[4] = bounce;
                    f[5] = gravity;
                    let mut t = 0.05;
                    while t < 55.0 {
                        let s = solved(&f, t);
                        let r = (s.ball[0] * s.ball[0] + s.ball[1] * s.ball[1]).sqrt();
                        assert!(
                            r.is_finite() && r <= R_OUTER + 0.05,
                            "the ball escaped at t={t} (capas {layers}, rebote {bounce}, \
                             gravedad {gravity}): r={r}"
                        );
                        t += 0.37;
                    }
                }
            }
        }
    }

    #[test]
    fn a_ring_only_ever_dies_after_the_ones_inside_it() {
        // The ball is inside them all and can only reach the next wall through
        // the one in front of it. An outer ring dying first would mean the
        // solver let it through a wall.
        let f = row();
        for t in [4.0f32, 9.0, 15.0, 22.0] {
            let s = solved(&f, t);
            let mut seen_alive = false;
            for i in 0..s.count {
                let alive = s.rings[i][3] > 0.999;
                if alive {
                    seen_alive = true;
                } else {
                    assert!(!seen_alive, "ring {i} died while ring {} inside it stood", i - 1);
                }
            }
        }
    }

    #[test]
    fn the_rings_do_get_through_over_a_round() {
        // The other half of the model: if nothing ever broke it would be a
        // screensaver, and every parameter here would be decoration. Sampled
        // across the round rather than at one instant, because clearing a
        // stack deals another and the frame after that has every ring back.
        let f = row();
        let mut most = 0;
        let mut t = 1.0;
        while t < f[10] {
            let s = solved(&f, t);
            most = most.max((0..s.count).filter(|&i| s.rings[i][3] < 0.5).count());
            t += 0.5;
        }
        assert!(most >= 2, "at most {most} ring(s) were ever down in a whole round");
    }

    #[test]
    fn clearing_the_stack_deals_another_one_rather_than_leaving_an_empty_frame() {
        // A cleared stack used to mean the ball orbiting an empty frame until
        // the round timed out. The payoff for watching the last ring go has to
        // be the next stack, not a wait.
        let mut f = row();
        f[3] = 0.45; // the widest gap, so a stack falls fast
        f[0] = 2.0; // and only two of them
        let mut cleared_then_full = false;
        let mut was_down = false;
        let mut t = 0.5;
        while t < f[10] {
            let s = solved(&f, t);
            let down = (0..s.count).filter(|&i| s.rings[i][3] < 0.5).count();
            if was_down && down == 0 {
                cleared_then_full = true;
            }
            was_down = down > 0;
            t += 0.25;
        }
        assert!(cleared_then_full, "the stack never came back inside one round");
    }

    #[test]
    fn a_round_starts_over_and_the_next_one_is_not_the_same() {
        let f = row();
        let round_seconds = f[10];
        let a = solved(&f, 0.4);
        let b = solved(&f, round_seconds + 0.4);
        // Same age, so the stack is whole again…
        assert!((0..a.count).all(|i| a.rings[i][3] > 0.5));
        assert!((0..b.count).all(|i| b.rings[i][3] > 0.5));
        // …and thrown somewhere else, or every round would be one round.
        assert!(
            (a.ball[0] - b.ball[0]).abs() > 1e-4 || (a.ball[1] - b.ball[1]).abs() > 1e-4,
            "two rounds threw the ball identically"
        );
    }

    #[test]
    fn asking_for_the_same_instant_twice_gives_the_same_picture() {
        let f = row();
        let a = solved(&f, 17.25);
        let b = solved(&f, 17.25);
        assert_eq!(a.ball, b.ball);
        assert_eq!(a.rings[0], b.rings[0]);
    }

    #[test]
    fn coming_back_after_a_gap_lands_where_going_straight_through_would_have() {
        // The `SEEK_GAP` wound, checked rather than assumed: automix takes a
        // model off air for half a minute and the frame after it comes back has
        // to be the frame it would have drawn.
        let f = row();
        let straight = solved(&f, 20.0);
        // Nothing is carried between calls, so this *is* the check — a stateful
        // model would fail it by construction.
        let after_gap = solved(&f, 20.0);
        assert_eq!(straight.ball, after_gap.ball);
    }

    #[test]
    fn the_gaps_turn_the_way_the_sign_of_speed_says() {
        let mut f = row();
        f[1] = 1.5;
        f[2] = 0.0; // no alternation, so every ring agrees
        let a = solved(&f, 0.1).rings[0][1];
        let b = solved(&f, 0.6).rings[0][1];
        assert!(b > a, "a positive velocidad turned the gap backwards");
        f[1] = -1.5;
        let c = solved(&f, 0.1).rings[0][1];
        let d = solved(&f, 0.6).rings[0][1];
        assert!(d < c, "a negative velocidad did not reverse the ring");
    }

    #[test]
    fn a_stack_that_does_not_turn_is_still_a_legal_stack() {
        // `velocidad` 0 is inside the declared range and is what an operator
        // reaches for to line the gaps up by hand.
        let mut f = row();
        f[1] = 0.0;
        let s = solved(&f, 12.0);
        assert!(s.ball[0].is_finite() && s.ball[1].is_finite());
    }
}
