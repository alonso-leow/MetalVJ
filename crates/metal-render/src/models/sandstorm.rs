//! Model 23 — Sandstorm. A 2D canvas with grains of sand falling into it from
//! a source: a point you place with coordinates, a mouth whose width you set,
//! and a gradient the grains take their colours from.
//!
//! *"A 2D canvas where grains of sand in custom colours fall from above,
//! positioned with coordinates, and another parameter to set the width of the
//! sand source."* Three of the floats below are that sentence —
//! `origen_x`, `origen_y`, `ancho` — and everything else is what a falling
//! grain needs in order to be falling rather than descending.
//!
//! # There is nothing on this side of it
//!
//! Rings, Puzzle, Mengano and Lazer all solve on the CPU and hand the GPU an
//! answer, because each of them has a *decision* in it — a bounce, a fusion, a
//! ring that is now gone — and a fragment shader has nowhere to keep a
//! decision. Sand has no decisions. A grain is emitted and then it falls, and
//! where it is at time `t` is a closed form. So the whole model is the shader,
//! and this file is a uniform.
//!
//! That is worth stating rather than assuming, because it is the reason the
//! grain count is not a parameter and there is no particle buffer. The shader
//! **inverts** the fall: a pixel's height is an age, an age plus a pixel's `x`
//! is the lane it was poured from, and grains sit on a fixed lattice in
//! `(lane, emission time)`. A pixel visits the fifteen lattice cells around
//! its own answer, whether the frame holds two hundred grains or two hundred
//! thousand. `shaders/sandstorm.wgsl` has the derivation and the one place it
//! approximates.
//!
//! Pure function of `(floats, time)` by construction, then — the property
//! automix depends on, and the one Turing had to grow a `SEEK_GAP` to fake.
//!
//! # Two floats are structural
//!
//! `carriles` and `densidad` set the lattice spacing, and the lattice is where
//! the grains *are*. Driven by the music they would re-deal every grain in the
//! frame on every transient, which does not read as sand reacting — it reads
//! as the model being restarted several times a second. They are `spec_held`
//! for the same reason Turing's feed and kill rates are.

use wgpu::util::DeviceExt;

use crate::model::{color_at, background, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    // The ask's coordinates. `origen_y` opens just above the top edge, so the
    // sand is already moving when it enters the frame rather than welling up
    // out of a point on it.
    spec("origin_x", -1.6, 1.6, 0.0),
    spec("origin_y", -1.0, 1.4, 1.15),
    // The ask's width of the source. Lanes span it, so a wider mouth pours
    // more sand instead of spreading the same sand thinner.
    spec("width", 0.02, 3.2, 0.9),
    // Structural — see the module header.
    spec_held("lanes", 4.0, 256.0, 40.0, 0.15),
    spec_held("density", 4.0, 400.0, 45.0, 0.2),
    // The fall. `caida` is the speed it leaves the source at and `gravedad` is
    // what happens after; with `gravedad` at 0 the curtain descends evenly,
    // which is a different and legal picture.
    spec("fall", 0.0, 3.0, 0.6),
    spec("gravity", 0.0, 3.0, 1.2),
    // Sideways, in the same two terms: a steady drift and a wind that builds.
    spec("drift", -1.5, 1.5, 0.15),
    spec("wind", -2.0, 2.0, 0.0),
    // The mouth opens into a cone rather than falling as a rigid column. Kept
    // linear in the age on purpose — see the shader header, where that is the
    // difference between a divide and an equation with the answer on both
    // sides of it.
    spec("fan", 0.0, 2.0, 0.35),
    spec("turbulence", 0.0, 1.0, 0.25),
    spec("swirl", 0.2, 6.0, 1.6),
    spec("size", 0.001, 0.03, 0.005),
    spec("life", 0.5, 12.0, 4.0),
    spec("wake", 0.0, 1.0, 0.3),
    spec("shine", 0.0, 1.0, 0.45),
    // 0: a grain's colour is its own number, so the curtain is speckled.
    // 1: a grain's colour is how far it has fallen, so the curtain is banded.
    spec("hue", 0.0, 1.0, 0.35),
];

/// Not white, for the reason every slot in this suite that opens somewhere
/// else is not white: four white stops here would draw a curtain of identical
/// white specks, and *"coloured grains"* is the ask. Pale, gold, ochre,
/// shadow — a solid colour is still one drag of four stops away.
const SAND: [f32; 12] = [
    0.98, 0.93, 0.78, //
    0.93, 0.76, 0.42, //
    0.72, 0.48, 0.22, //
    0.36, 0.24, 0.14,
];

/// The storm (2026-10): the camera inside it. Appended as a second block
/// *after* the colour slots so every float above keeps its column.
const STORM: &[FloatSpec] = &[
    // The old single-stream curtain, now one layer among many. 0 hides it.
    spec("curtain", 0.0, 1.0, 0.1),
    // How full the depth layers are: the share of lattice cells holding a streak.
    spec("flurry", 0.0, 1.0, 0.9),
    // Far/near scale ratio: bigger is a deeper storm (fine far grain, huge near streaks).
    spec("depth_range", 1.0, 12.0, 8.0),
    // Gusts: sideways surges that slant the whole storm and sway it.
    spec("gust", 0.0, 1.0, 0.5),
    spec("gust_rate", 0.05, 2.0, 0.35),
    // Streak length (0: grains, 1: long rain-like lines) and grain size multiplier.
    spec("streak", 0.0, 1.0, 0.3),
    spec("grit", 0.2, 4.0, 1.0),
    // Light shafts through the dust, depth fog, and how much the dust takes the sand's colour.
    spec("shafts", 0.0, 1.0, 0.35),
    spec("fog", 0.0, 1.0, 0.45),
    spec("tint", 0.0, 1.0, 0.5),
    // Visibility: 1 clear, lower thickens the air. `pulse` dips it rhythmically
    // at `pulse_rate` Hz (bind either to the music).
    spec("visibility", 0.0, 1.0, 1.0),
    spec("pulse", 0.0, 1.0, 0.25),
    spec("pulse_rate", 0.0, 4.0, 0.5),
    // How strongly wind and drift lean the streaks.
    spec("slant", 0.0, 2.0, 1.0),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[Block::Floats(FLOATS), color_at("sand", &SAND), background(), Block::Floats(STORM), crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK, crate::model::EFFECTS_COLOR_BLOCK],
};

// ---------------------------------------------------------------------------
// the inversion, on the CPU too
// ---------------------------------------------------------------------------

/// How old a grain must be to have fallen `depth` below the source: `depth =
/// v0*a + g*a^2/2`, solved for `a`.
///
/// It is here as well as in the shader because it is the one piece of
/// arithmetic the model would be silently wrong about — a NaN at `gravedad =
/// 0`, or a negative root — and a WGSL function cannot be unit tested. The two
/// are the same three lines; the test below is what keeps them so.
pub fn age_at_depth(depth: f32, v0: f32, g: f32) -> f32 {
    if g < 1e-3 {
        return depth / v0.max(1e-3);
    }
    ((v0 * v0 + 2.0 * g * depth).max(0.0).sqrt() - v0) / g
}

/// Where a grain of age `a`, poured from `lane_x`, is now. The forward half of
/// the pair — `age_at_depth` inverts its `y`.
pub fn grain_at(origin: [f32; 2], lane_x: f32, a: f32, f: &[f32]) -> [f32; 2] {
    let (fall, grav, drift, wind, fan) =
        (f[5].max(0.0), f[6].max(0.0), f[7], f[8], f[9].max(0.0));
    let off = lane_x - origin[0];
    [
        origin[0] + off * (1.0 + fan * a) + drift * a + 0.5 * wind * a * a,
        origin[1] - (fall * a + 0.5 * grav * a * a),
    ]
}

/// The other half of the inversion: which lane a point at `x`, whose age is
/// already known, was poured from. Exactly what the shader divides by.
pub fn lane_of(origin: [f32; 2], x: f32, a: f32, f: &[f32]) -> f32 {
    let (drift, wind, fan) = (f[7], f[8], f[9].max(0.0));
    origin[0] + (x - origin[0] - (drift * a + 0.5 * wind * a * a)) / (1.0 + fan * a)
}

// ---------------------------------------------------------------------------
// the GPU side
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    sand: ColorSlotGpu,
    background: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    e: [f32; 4],
    f: [f32; 4],
    g: [f32; 4],
    h: [f32; 4],
    i: [f32; 4],
}

pub struct Sandstorm {
    state: Built<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
}

impl Sandstorm {
    pub fn new() -> Sandstorm {
        Sandstorm { state: Built::default() }
    }

    fn build(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
        let shader = crate::shared::build_shader(
            device,
            "sandstorm",
            include_str!("../../shaders/sandstorm.wgsl"),
        );

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sandstorm bgl"),
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
            label: Some("sandstorm layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sandstorm pipeline"),
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

impl Model for Sandstorm {
    fn id(&self) -> &'static str {
        "sandstorm"
    }

    fn name(&self) -> &'static str {
        "Sandstorm"
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

        let lanes = floats[3].round().clamp(4.0, 256.0);
        // The lattice is spaced in seconds between emissions, not in
        // emissions per second, because that is what the shader divides by.
        let cell_t = 1.0 / floats[4].clamp(1.0, 2000.0);

        let uniform = Uniform {
            sand: ColorSlotGpu::from_floats(&floats[17..29]),
            background: ColorSlotGpu::from_floats(&floats[29..41]),
            a: [ctx.aspect(), ctx.time, floats[0], floats[1]],
            b: [floats[2], floats[5], floats[6], floats[7]],
            c: [floats[8], floats[10], floats[11], lanes],
            d: [cell_t, floats[12], floats[13], floats[14]],
            e: [floats[15], floats[16], floats[9], 0.0],
            f: [floats[42], floats[43], floats[44], floats[45]],
            g: [floats[46], floats[47], floats[48], floats[49]],
            h: [floats[50], floats[51], floats[52], floats[53]],
            i: [floats[41], floats[54], 0.0, 0.0],
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sandstorm uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sandstorm bind group"),
            layout: bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("sandstorm encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sandstorm pass"),
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
        // 17 floats + two colour slots + 14 storm floats. `render` slices `[17..29]` and
        // `[29..41]` by hand.
        assert_eq!(SCHEMA.width(), 55 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 55 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    #[test]
    fn the_age_and_the_fall_are_inverses_of_each_other() {
        // The whole model. If these two ever disagree, a pixel asks about a
        // grain that is somewhere else and the frame is empty — with nothing
        // out of range, nothing logged, and nothing else able to notice.
        let f = row();
        let origin = [f[0], f[1]];
        for &fall in &[0.0f32, 0.6, 3.0] {
            for &grav in &[0.0f32, 1.2, 3.0] {
                if fall == 0.0 && grav == 0.0 {
                    continue; // sand that never leaves the mouth
                }
                let mut g = f.clone();
                g[5] = fall;
                g[6] = grav;
                for &a in &[0.05f32, 0.4, 1.5, 3.0] {
                    let pos = grain_at(origin, origin[0], a, &g);
                    let depth = origin[1] - pos[1];
                    let back = age_at_depth(depth, fall, grav);
                    assert!(
                        (back - a).abs() < 1e-3,
                        "age {a} fell to depth {depth} and inverted to {back} \
                         (fall {fall}, gravity {grav})"
                    );
                }
            }
        }
    }

    #[test]
    fn the_fan_inverts_too_and_never_folds_two_lanes_onto_one() {
        // The horizontal half of the inversion, which the test above does not
        // reach because it pours everything down the middle lane. If `abanico`
        // ever stopped being linear in the age, this is where it would show:
        // `lane_of` would stop being the inverse of `grain_at`, and every pixel
        // would ask about a grain in the wrong lane — an empty frame, in range,
        // with nothing logged.
        let f = row();
        let origin = [f[0], f[1]];
        for &fan in &[0.0f32, 0.35, 2.0] {
            let mut g = f.clone();
            g[9] = fan;
            for &lane in &[-0.4f32, 0.0, 0.45] {
                for &a in &[0.05f32, 0.9, 2.5] {
                    let pos = grain_at(origin, origin[0] + lane, a, &g);
                    let back = lane_of(origin, pos[0], a, &g) - origin[0];
                    assert!(
                        (back - lane).abs() < 1e-3,
                        "lane {lane} at age {a} came back as {back} (fan {fan})"
                    );
                }
            }
        }
    }

    #[test]
    fn no_slider_makes_the_age_a_nan() {
        // `gravedad` at 0 degenerates the quadratic to 0/0, and `caida` at 0
        // degenerates the linear branch. Both are inside the declared range
        // and both are where an operator will actually put them.
        for &fall in &[0.0f32, 0.6, 3.0] {
            for &grav in &[0.0f32, 1e-4, 1.2, 3.0] {
                for &depth in &[0.0f32, 0.01, 2.4] {
                    let a = age_at_depth(depth, fall, grav);
                    assert!(a.is_finite() && a >= 0.0, "depth {depth}, {fall}, {grav} -> {a}");
                }
            }
        }
    }

    #[test]
    fn the_sand_crosses_the_frame_inside_a_grains_life() {
        // A `vida` shorter than the fall means grains fading out in mid-air,
        // which is a legal picture but a terrible default: the curtain would
        // stop halfway down and never reach the bottom edge.
        let f = row();
        let to_bottom = f[1] + 1.0; // the source, down to y = -1
        let a = age_at_depth(to_bottom, f[5], f[6]);
        assert!(a < f[13], "the default sand dies at {a}s, {to_bottom} short of the floor");
    }

    #[test]
    fn a_grain_is_never_sampled_further_than_the_lanes_a_pixel_visits() {
        // The shader checks 3 lanes either side of the pixel's own. A grain
        // wider than a lane would be drawn as a column of clipped slivers, and
        // it is `ancho` at its narrowest against `tamano` at its widest that
        // gets there first.
        let f = row();
        let cell_w = f[2] / f[3];
        assert!(
            f[12] < cell_w * 3.0,
            "a default grain ({}) is wider than the three lanes around it ({})",
            f[12],
            cell_w * 3.0
        );
    }
}
