//! Model 21 — Scratch-off. A surface that comes away where something has been
//! dragged over it, and a cycle that does not end until all of it has.
//!
//! # Why there is a closer
//!
//! The hook the ask names — *"wait until everything is cleared"* — is a promise
//! the model has to keep, and a scatter of elements at hashed angles does not
//! keep it: bands laid down at random leave slivers between them for as long as
//! you care to wait, and a viewer who has been promised a finish and does not
//! get one stops watching. So the **last element of every cycle is wider than
//! the frame**. It arrives at 80% of the cycle and takes everything left,
//! including whatever the grain was protecting.
//!
//! That is a deliberate loss of realism for a gain in the thing the model is
//! for. A card being scratched by a coin never gets a guaranteed final pass;
//! this is not a card, it is a loop that has to resolve before it repeats.
//!
//! # Remnants are a threshold, not an accumulation
//!
//! Every pixel is given a toughness out of a noise field (`resto`), and an
//! element passing over it only takes it if it scratched harder than that. The
//! leftovers are therefore *where the surface is stubborn*, which is where they
//! are on a real card, rather than *where nothing has been yet* — and they read
//! as flecks clinging on rather than as geometry the elements missed.
//!
//! # No history buffer
//!
//! Coverage is a `max` over swept regions and a swept region is a closed form,
//! so the whole picture is a function of `(floats, time)`. See `scratch.wgsl`.
//! This is the model that most obviously *wants* an accumulation texture and
//! most obviously must not have one: automix parks an animation for thirty
//! seconds, and a scratch card that carries on from where it was when it went
//! off air would come back either finished or, worse, half finished from a
//! cycle nobody saw.

use wgpu::util::DeviceExt;

use crate::model::{color_at, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::ColorSlotGpu;

/// The elements the uniform's arrays can carry. Two statements of one number:
/// raising it means raising both arrays in `scratch.wgsl`.
pub const MAX_ELEMENTS: usize = 24;

/// Corner to corner of the frame, near enough. Everything an element does is
/// measured in these, so a pass crosses the whole picture whatever its angle.
const DIAG: f32 = 2.6;

const FLOATS: &[FloatSpec] = &[
    // Structural: the count sets how the cycle is paced, and re-dealing it on
    // a transient would restart the clearing rather than intensify it.
    spec_held("elements", 2.0, MAX_ELEMENTS as f32, 9.0, 0.2),
    // The direction they come from, as a whole turn. *"from different
    // directions"* is this plus the spread below.
    spec("angle", 0.0, 1.0, 0.18),
    // How far the individual angles wander off it. At 0 every element crosses
    // the same way, which is a wipe; at 1 they come from everywhere.
    spec("dispersion", 0.0, 1.0, 0.55),
    spec("width", 0.02, 0.5, 0.13),
    spec("speed", 0.15, 4.0, 1.0),
    // How much of the surface refuses to come off on the first pass.
    spec("rest", 0.0, 1.0, 0.5),
    spec("grain", 1.0, 24.0, 7.0),
    spec("edge", 0.0, 1.0, 0.6),
    spec("shine", 0.0, 1.0, 0.55),
    // The clock the whole model is defined on — see `round_at`. Held, because
    // a cycle length that moved with the music would restart the card mid-clear.
    spec_held("cycle", 4.0, 60.0, 18.0, 0.1),
];

/// The surface being taken off: a brushed metallic grey with the suite's red
/// in it. Not white — four white stops would scratch white off white.
const COATING: [f32; 12] = [
    0.62, 0.63, 0.66, //
    0.34, 0.35, 0.39, //
    0.55, 0.16, 0.13, //
    0.20, 0.20, 0.23,
];
/// What is under it. Dark, so the clearing reads as opening a hole rather than
/// as changing shade.
const REVEALED: [f32; 12] = [
    0.03, 0.03, 0.05, //
    0.10, 0.02, 0.02, //
    0.32, 0.05, 0.04, //
    0.95, 0.35, 0.18,
];
const ELEMENT: [f32; 12] = [
    0.80, 0.80, 0.84, //
    0.98, 0.94, 0.88, //
    1.00, 0.72, 0.42, //
    0.95, 0.35, 0.18,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("scratchable", &COATING),
        color_at("develop", &REVEALED),
        color_at("element", &ELEMENT),
    crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
    crate::model::EFFECTS_COLOR_BLOCK,
],
};

// ---------------------------------------------------------------------------
// the elements
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
    hash(a.wrapping_mul(0x9E37_79B9) ^ b.wrapping_mul(0x85EB_CA6B).wrapping_add(7))
}

/// One element, in the two `vec4`s the shader reads.
#[derive(Clone, Copy, Debug)]
pub struct Element {
    /// Where its line starts, off-frame.
    pub origin: [f32; 2],
    pub dir: [f32; 2],
    /// How far along the line its head has got. Zero or less is "not yet".
    pub reach: f32,
    pub half_width: f32,
    pub seed: f32,
    /// 0 before it launches, 1 after.
    pub live: f32,
}

/// Where the cycle is. The only relationship this model has with the clock.
fn round_at(cycle: f32, time: f32) -> (u32, f32) {
    let c = cycle.max(1.0);
    let t = time.max(0.0);
    let index = (t / c).floor().clamp(0.0, 2.0e9);
    (index as u32, t - index * c)
}

/// Everything crossing the frame at `time`, and how far through the cycle it is.
pub fn elements(floats: &[f32], time: f32) -> (Vec<Element>, f32) {
    let n = (floats[0].round() as usize).clamp(2, MAX_ELEMENTS);
    let base = floats[1] * std::f32::consts::TAU;
    let spread = floats[2].clamp(0.0, 1.0);
    let width = floats[3].clamp(0.02, 0.5);
    let speed = floats[4].max(0.05);
    let cycle = floats[9].clamp(4.0, 60.0);

    let (round, local) = round_at(cycle, time);
    let seed = round.wrapping_mul(0x9E37_79B9).wrapping_add(101);

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let id = i as u32;
        // The last one is the closer: wider than the frame, late in the cycle,
        // and the reason the card is guaranteed to finish. See the module note.
        let closer = i + 1 == n;

        let angle = if closer {
            // Across whatever the others left, rather than with them.
            base + std::f32::consts::FRAC_PI_2
        } else {
            base + (h2(seed, id) - 0.5) * spread * std::f32::consts::TAU
        };
        let (sin, cos) = angle.sin_cos();
        let dir = [cos, sin];
        let normal = [-sin, cos];

        // The line is offset sideways so the passes are spread over the frame
        // instead of all going through the middle.
        let lateral = if closer { 0.0 } else { (h2(seed, id.wrapping_add(400)) - 0.5) * DIAG };
        let origin = [
            normal[0] * lateral - dir[0] * DIAG * 0.5,
            normal[1] * lateral - dir[1] * DIAG * 0.5,
        ];

        // Staggered across the cycle, so the card is taken off in passes rather
        // than in one instant.
        let launch = if closer {
            cycle * 0.80
        } else {
            cycle * 0.75 * (i as f32 / (n - 1).max(1) as f32)
                + h2(seed, id.wrapping_add(800)) * cycle * 0.06
        };
        let travelled = (local - launch) * speed * DIAG * 0.55;

        let half_width = if closer {
            // Wider than the frame's diagonal, so "behind the head" is the only
            // thing that decides, and everything ends up behind it.
            DIAG * 2.0
        } else {
            width * (0.55 + 0.9 * h2(seed, id.wrapping_add(1200)))
        };

        out.push(Element {
            origin,
            dir,
            reach: travelled,
            half_width,
            seed: h2(seed, id.wrapping_add(1600)) * 100.0,
            live: if local >= launch { 1.0 } else { 0.0 },
        });
    }
    (out, local / cycle)
}

// ---------------------------------------------------------------------------
// the GPU side
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    coating: ColorSlotGpu,
    revealed: ColorSlotGpu,
    element: ColorSlotGpu,
    misc: [f32; 4],
    misc2: [f32; 4],
    paths: [[f32; 4]; MAX_ELEMENTS],
    heads: [[f32; 4]; MAX_ELEMENTS],
}

pub struct Scratch {
    state: Built<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
}

impl Scratch {
    pub fn new() -> Scratch {
        Scratch { state: Built::default() }
    }

    fn build(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
        let shader = crate::shared::build_shader(
            device,
            "scratch",
            include_str!("../../shaders/scratch.wgsl"),
        );

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scratch bgl"),
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
            label: Some("scratch layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scratch pipeline"),
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

impl Model for Scratch {
    fn id(&self) -> &'static str {
        "scratch"
    }

    fn name(&self) -> &'static str {
        "Scratch-off"
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

        let (list, progress) = elements(floats, ctx.time);
        let mut paths = [[0.0f32; 4]; MAX_ELEMENTS];
        let mut heads = [[0.0f32; 4]; MAX_ELEMENTS];
        for (i, e) in list.iter().enumerate() {
            paths[i] = [e.origin[0], e.origin[1], e.dir[0], e.dir[1]];
            heads[i] = [e.reach, e.half_width, e.seed, e.live];
        }

        let uniform = Uniform {
            coating: ColorSlotGpu::from_floats(&floats[10..22]),
            revealed: ColorSlotGpu::from_floats(&floats[22..34]),
            element: ColorSlotGpu::from_floats(&floats[34..46]),
            misc: [ctx.aspect(), list.len() as f32, progress, floats[6]],
            misc2: [floats[5], floats[7], floats[8], 0.0],
            paths,
            heads,
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scratch uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scratch bind group"),
            layout: bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("scratch encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scratch pass"),
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

    /// The shader's coverage test, on the CPU, so "is the card finished" is a
    /// question that can be answered without a GPU. Toughness is left out —
    /// the closer is what has to clear a pixel whatever its grain, and this is
    /// what checks that it does.
    fn scratched_at(list: &[Element], p: [f32; 2]) -> f32 {
        let mut best: f32 = 0.0;
        for e in list {
            if e.live <= 0.0 {
                continue;
            }
            let rel = [p[0] - e.origin[0], p[1] - e.origin[1]];
            let along = rel[0] * e.dir[0] + rel[1] * e.dir[1];
            let across = -rel[0] * e.dir[1] + rel[1] * e.dir[0];
            if along > e.reach {
                continue;
            }
            if across.abs() < e.half_width * 0.55 {
                best = best.max(1.0);
            }
        }
        best
    }

    #[test]
    fn the_schema_and_the_slices_render_reads_are_the_same_width() {
        assert_eq!(SCHEMA.width(), 46 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 46 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    /// The promise the module header makes. Every corner and the middle of the
    /// frame have to be gone by the end of the cycle, or the hook is a lie.
    #[test]
    fn every_cycle_finishes() {
        let a = 16.0f32 / 9.0;
        let probes = [
            [0.0f32, 0.0],
            [-a * 0.98, 0.98],
            [a * 0.98, 0.98],
            [-a * 0.98, -0.98],
            [a * 0.98, -0.98],
            [a * 0.5, -0.3],
            [-a * 0.2, 0.7],
        ];
        for element_count in [2.0f32, 9.0, 24.0] {
            for angle in [0.0f32, 0.37, 0.9] {
                for dispersion in [0.0f32, 1.0] {
                    let mut f = row();
                    f[0] = element_count;
                    f[1] = angle;
                    f[2] = dispersion;
                    let cycle = f[9];
                    let (list, _) = elements(&f, cycle * 0.995);
                    for probe in probes {
                        assert!(
                            scratched_at(&list, probe) > 0.5,
                            "{probe:?} was still there at the end of the cycle \
                             (elementos {element_count}, angulo {angle}, dispersion {dispersion})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_card_is_whole_when_a_cycle_starts() {
        // The other end of the same promise: a cycle that opened half cleared
        // would have no story in it.
        let f = row();
        let (list, progress) = elements(&f, 0.0);
        assert!(progress < 0.01);
        assert_eq!(scratched_at(&list, [0.0, 0.0]), 0.0, "the middle was already gone at t=0");
    }

    #[test]
    fn the_screen_only_ever_gets_cleaner_within_a_cycle() {
        // Coverage is a `max` over swept regions and a swept region only grows,
        // so a pixel that came off must not come back. Worth a test because it
        // is exactly what a sign error in `along > reach` would break.
        let f = row();
        let cycle = f[9];
        let probe = [0.31f32, -0.22];
        let mut was = 0.0;
        let mut t = 0.0;
        while t < cycle * 0.99 {
            let (list, _) = elements(&f, t);
            let now = scratched_at(&list, probe);
            assert!(now >= was - 1e-6, "the surface grew back at t={t}");
            was = now;
            t += cycle / 400.0;
        }
    }

    #[test]
    fn the_cycle_starts_over_with_a_different_deal() {
        let f = row();
        let cycle = f[9];
        let (a, _) = elements(&f, cycle * 0.2);
        let (b, _) = elements(&f, cycle * 1.2);
        assert_eq!(a.len(), b.len());
        assert!(
            a.iter().zip(b.iter()).any(|(x, y)| (x.dir[0] - y.dir[0]).abs() > 1e-4),
            "two cycles laid down the same passes"
        );
    }

    #[test]
    fn nothing_launches_before_its_time_and_everything_launches() {
        let f = row();
        let cycle = f[9];
        let (at_start, _) = elements(&f, 0.001);
        assert!(at_start.iter().filter(|e| e.live > 0.0).count() <= 1, "they all went at once");
        let (at_end, _) = elements(&f, cycle * 0.999);
        assert!(at_end.iter().all(|e| e.live > 0.0), "something never crossed at all");
    }

    #[test]
    fn asking_for_the_same_instant_twice_gives_the_same_passes() {
        let f = row();
        let (a, pa) = elements(&f, 7.75);
        let (b, pb) = elements(&f, 7.75);
        assert_eq!(pa, pb);
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.reach, y.reach);
            assert_eq!(x.origin, y.origin);
        }
    }

    #[test]
    fn every_element_crosses_the_whole_frame_at_any_angle() {
        // An element whose line misses the frame is an element that does
        // nothing, and at a hashed angle that is easy to arrange by accident.
        for angle in 0..12 {
            let mut f = row();
            f[1] = angle as f32 / 12.0;
            f[2] = 1.0;
            let (list, _) = elements(&f, f[9] * 0.999);
            for e in &list {
                // Its head is past the far side of the frame by the time the
                // cycle ends.
                assert!(
                    e.reach > DIAG,
                    "an element only got {} of the way across at angle {angle}",
                    e.reach
                );
                let len = (e.dir[0] * e.dir[0] + e.dir[1] * e.dir[1]).sqrt();
                assert!((len - 1.0).abs() < 1e-4, "a direction was not a unit vector");
            }
        }
    }
}
