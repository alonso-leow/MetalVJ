//! Model 15 — Room (`room`). A room with things in it, and nothing in it but weave:
//! six walls of grid plus a couch, a television, a hanging lamp, a table with
//! a telephone, a curtained window and a door ajar — every one of them drawn
//! with the *same* grid the walls are drawn with, and none of them carrying a
//! colour, a surface or a shade of its own.
//!
//! It began as Chamber's room with the movement taken out and the light put
//! in. That was the whole model and it was thin: six flat faces are a box, and
//! a box lights the same way whichever corner you are in. What the room was
//! missing was **something to be at a distance from**. The ask of 2026-08-21
//! names the six things by name, and the reason they belong here rather than
//! in a new model is in how they are lit:
//!
//! > *"the idea is to make a lysergic visualisation of the real world,
//! > completely excluding colours as we know them and instead seeing depth
//! > through the grid pattern; colour will really be part of the animation,
//! > but only in the grid, not as objects."*
//!
//! So there is no couch-coloured couch. There is no lit side and dark side of
//! a lamp shade. Nothing here has an albedo at all: **an object is a region of
//! grid**, and everything you can read off it — that it is near, that it is
//! solid, that it is a screen, that it is cloth — arrives as brightness and as
//! a position along one gradient. Three rules do all of it:
//!
//!   * the lamp is the only light, so distance from it *is* the shading;
//!   * the gradient is walked by distance from the eye, so colour *is* depth;
//!   * the weave is one weave — [`CELLS_PER_UNIT`] and [`POINTS_PER_UNIT`] are
//!     the same for a wall and for a table leg, so a leg is not a wall drawn
//!     small, it is the same cloth cut smaller.
//!
//! Two things the old model did are deliberately gone. The six faces are no
//! longer six instances of one face — a room that contains things cannot be
//! one face repeated, so the whole room is baked into a single line buffer and
//! drawn once, and `caras`/`lineas`/`muebles` are all rebuild keys. And the
//! depth fade is measured from the camera rather than from the room's centre,
//! which was a fair approximation of an empty cube and is simply wrong once
//! there is a couch between you and a wall.
//!
//! What has *not* changed is the invariant the model was founded on:
//! **nothing here is displaced.** No vertex moves with time. The curtain's
//! fold is baked into its geometry, the door's swing is baked into its
//! geometry, and every animation in this model is luminance, colour, or where
//! you are standing. That is still what separates it from Chamber — and it is
//! what separates it from `models::rio`, which is this model's grammar applied
//! to a river valley and which breaks precisely that one rule, because a bush
//! that answers the wind is a vertex that moves.
//!
//! The builder this model was written around now lives in `shared::weave`,
//! unchanged and with the room's own rates passed in. It moved for River, and the
//! header there has the argument for why the vocabulary is shared and the two
//! numbers are not.

use std::f32::consts::{FRAC_PI_2, TAU};

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::model::{
    camera, color_at, background, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema,
    SurfaceSpec,
};
use crate::shared::backdrop::Backdrop;
use crate::shared::surface::{self, MaterialU, PartSet};
use crate::shared::mesh::{self as m3, DepthCache};
use crate::shared::weave::{Weave, LAYOUT};
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    spec("shine", 0.0, 2.0, 1.0),
    spec("variation", 0.0, 1.0, 0.45),
    spec("pulse", 0.0, 3.0, 0.8),
    spec("sweep", 0.0, 1.0, 0.5),
    // How much the camera wanders from where the operator put it. This is the
    // model's own motion and not the digest driving a camera — that never
    // happens anywhere in the suite, for the reason `drive.rs` records.
    spec("drift", 0.0, 1.0, 0.35),
    // 0 lights the room flat, the way it was lit before there was anything in
    // it to cast the question. 1 hands the whole room to the lamp, and then
    // brightness is distance and nothing else.
    spec("lamp", 0.0, 1.0, 0.75),
    // How much of the gradient is walked by distance from the eye rather than
    // by brightness. This is the float the ask is actually about.
    spec("depth", 0.0, 1.0, 0.6),
    spec_held("lines", 4.0, 40.0, 16.0, 0.15),
    spec_held("faces", 1.0, 6.0, 6.0, 0.0),
    // How many of the six things are in the room, in the order they were
    // asked for: couch, television, lamp, table with telephone, curtained
    // window, door. Structural — a live signal emptying the room mid-set is
    // not a swell, it is a different model.
    spec_held("furniture", 0.0, 6.0, 6.0, 0.0),
    // How much of the room is a filled surface rather than bare grid. 0 is the
    // line-only room this model has always drawn, byte for byte; 1 fills every
    // cell with a face that carries a real normal, so the room takes a picture
    // and a material on its parts like any other 3D model. Appended (rule 2).
    spec("fill", 0.0, 1.0, 0.0),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        // Standing against the back wall looking down the room: the couch is
        // along the left, the television faces it from the right, and the
        // window and the door are both on the wall ahead. Every one of the six
        // is in the opening frame, which is the only arrangement of a room and
        // a 96-degree lens where that is true.
        camera([-0.05, 0.26, -0.72], [0.08, -0.28, 0.75], 96.0),
        // Not a room's colours — a ramp with no daylight anywhere in it, so
        // nothing can be mistaken for the colour a thing *is*: near-black
        // violet, magenta, cyan, white-hot. One slot, deliberately: a second
        // one would be an object colour by another name.
        color_at(
            "mesh",
            &[
                0.02, 0.01, 0.07, //
                0.55, 0.05, 0.62, //
                0.05, 0.76, 0.88, //
                1.00, 0.97, 0.90,
            ],
        ),
        background(),
        Block::Floats(CAMERA_TRACK_FLOATS),
    crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
    crate::model::EFFECTS_COLOR_BLOCK,
],
};

/// Reserved for `cam.track` (H4) — see `models::turing`'s block for the
/// convention and the reasoning.
const CAMERA_TRACK_FLOATS: &[FloatSpec] = &[
    spec("cam_x", -3.0, 3.0, 0.0),
    spec("cam_y", -3.0, 3.0, 0.0),
    spec("cam_z", -3.0, 3.0, 0.0),
];

/// The cube's half-extent, and how wide one wall is. Both Chamber's, so the
/// two rooms are the same room seen two ways.
const FACE: f32 = 2.0;

/// How finely the luminance field is sampled along a line, in points per unit
/// of room. The wall grid has always been 18 points across a 2-unit face;
/// keeping the rate rather than the count is what makes a table leg lit at the
/// same resolution as the wall behind it.
const POINTS_PER_UNIT: f32 = 9.0;
const MAX_POINTS: usize = 18;

/// How many cells a gridded surface gets per unit of its own size. One number
/// for the whole room: the couch and the wall are the same weave, which is the
/// entire ask.
const CELLS_PER_UNIT: f32 = 4.5;

/// Where the bulb hangs. The shader needs it — it is the room's only light —
/// so it is a constant here rather than a number buried in `lamp()`.
const BULB: Vec3 = Vec3::new(0.05, 0.24, -0.05);

// Which thing a line belongs to. Only ever a hash seed, so that two lines that
// happen to cross do not flicker as one.
const ROOM: f32 = 0.0;
const COUCH: f32 = 1.0;
const TELE: f32 = 2.0;
const LAMP: f32 = 3.0;
const TABLE: f32 = 4.0;
const WINDOW: f32 = 5.0;
const DOOR: f32 = 6.0;

// How a line is lit. This is the only "material" in the model and it carries
// no colour: five numbers the shader branches on, nothing more.
const WALL: f32 = 0.0;
const SOLID: f32 = 1.0;
const EMIT: f32 = 2.0;
const CLOTH: f32 = 3.0;
const GLASS: f32 = 4.0;

/// The parts a picture or a material can be laid on (plan: surface sources,
/// materials M6): the six things in the room, in `obj` order. The lamp is the
/// one object with no part — it is the light, not a surface. Its index is the
/// `surfaces` slot in [`Room`].
const SURFACES: &[&str] = &["walls", "couch", "tv", "table", "window", "door"];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    color: ColorSlotGpu,
    floats: [f32; 4],
    /// (time, lampara, profundidad, relleno)
    floats2: [f32; 4],
    bulb: [f32; 4],
    eye: [f32; 4],
    view_proj: [[f32; 4]; 4],
    // Materials (appended), one `MaterialU` per part in `SURFACES` order: each
    // carries the part's picture setting and its `mat_*` floats, see
    // `material.wgsl`. The viewport in pixels for projector mapping follows.
    mat_walls: MaterialU,
    mat_couch: MaterialU,
    mat_tv: MaterialU,
    mat_table: MaterialU,
    mat_window: MaterialU,
    mat_door: MaterialU,
    screen: [f32; 4],
}

/// The baked room: its lines, and the filled/translucent faces the weave emits
/// from the same points, so every part can wear a material (materials M6).
struct Mesh {
    lines: wgpu::Buffer,
    line_count: u32,
    fill: wgpu::Buffer,
    fill_count: u32,
    glass: wgpu::Buffer,
    glass_count: u32,
}

struct Gpu {
    line_pipeline: wgpu::RenderPipeline,
    fill_pipeline: wgpu::RenderPipeline,
    glass_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    depth: DepthCache,
    /// The baked room, and the three structural floats it was baked for.
    mesh: Option<(Mesh, (u32, u32, u32))>,
}

pub struct Room {
    gpu: Built<Gpu>,
    backdrop: Built<Backdrop>,
    /// `walls`..`door`, in `SURFACES` order.
    surfaces: PartSet<6>,
}

/// The room under construction. `shared::weave` has the builder and the
/// meaning of every slot in a vertex; these are the rates this room is cut at.
fn room() -> Weave {
    let mut w = Weave::new(POINTS_PER_UNIT, MAX_POINTS, CELLS_PER_UNIT, 8);
    // The window panes are the room's translucent surfaces: their filled cells
    // must not write depth, so a wall behind them still draws.
    w.translucent = |role| role == GLASS;
    w
}

/// The 6 interior faces of the cube, exactly Chamber's — the room is the same
/// room, and two hand-copied sets of transforms would be two rooms that drift
/// apart. Applied on the CPU now rather than per instance on the GPU: a room
/// that contains a couch is not one face drawn six times.
fn cube_faces() -> [Mat4; 6] {
    let h = 1.0;
    [
        Mat4::from_translation(Vec3::new(0.0, -h, 0.0)),
        Mat4::from_translation(Vec3::new(0.0, h, 0.0)) * Mat4::from_rotation_z(std::f32::consts::PI),
        Mat4::from_translation(Vec3::new(-h, 0.0, 0.0)) * Mat4::from_rotation_z(FRAC_PI_2),
        Mat4::from_translation(Vec3::new(h, 0.0, 0.0)) * Mat4::from_rotation_z(-FRAC_PI_2),
        Mat4::from_translation(Vec3::new(0.0, 0.0, -h)) * Mat4::from_rotation_x(-FRAC_PI_2),
        Mat4::from_translation(Vec3::new(0.0, 0.0, h)) * Mat4::from_rotation_x(FRAC_PI_2),
    ]
}

/// Against the left wall, along it, facing the television.
///
/// Four boxes — base, back, two arms — was the first attempt and it rendered
/// as a crate. **A slab is not a seat.** What says "couch" at this line
/// density is not the outline, it is the *seams*: three cushions on the base
/// and three against the back, with a hand's gap between them, so the grid
/// breaks where a couch breaks. Ten boxes, and it is the cheapest legibility
/// in the room.
fn couch(r: &mut Weave) {
    r.boxy(Vec3::new(-0.72, -0.93, -0.15), Vec3::new(0.22, 0.06, 0.42), COUCH, SOLID);
    r.boxy(Vec3::new(-0.93, -0.68, -0.15), Vec3::new(0.05, 0.22, 0.42), COUCH, SOLID);
    for dz in [-0.27f32, 0.0, 0.27] {
        r.boxy(Vec3::new(-0.72, -0.83, -0.15 + dz), Vec3::new(0.22, 0.05, 0.12), COUCH, SOLID);
        r.boxy(Vec3::new(-0.84, -0.70, -0.15 + dz), Vec3::new(0.05, 0.14, 0.12), COUCH, SOLID);
    }
    r.boxy(Vec3::new(-0.74, -0.78, -0.53), Vec3::new(0.24, 0.10, 0.05), COUCH, SOLID);
    r.boxy(Vec3::new(-0.74, -0.78, 0.23), Vec3::new(0.24, 0.10, 0.05), COUCH, SOLID);
}

/// On the right wall, facing the couch. The screen is a separate sheet a
/// millimetre proud of the body and the only thing in the room besides the
/// lamp that makes its own light — everything a television is, in this model,
/// is that its grid is the one running at a different rate.
fn television(r: &mut Weave) {
    r.boxy(Vec3::new(0.86, -0.92, 0.05), Vec3::new(0.11, 0.08, 0.28), TELE, SOLID);
    r.boxy(Vec3::new(0.92, -0.52, 0.05), Vec3::new(0.04, 0.26, 0.42), TELE, SOLID);
    r.sheet(7, 5, TELE, EMIT, |u, v| {
        Vec3::new(0.875, -0.74 + 0.44 * v, -0.34 + 0.68 * u)
    });
}

/// A point on the shade, `u` around it. Two rings and the ribs between them
/// are a cone; a cone lit from inside is a lamp.
fn shade(y: f32, rad: f32, u: f32) -> Vec3 {
    let a = u * TAU;
    Vec3::new(BULB.x + a.cos() * rad, y, BULB.z + a.sin() * rad)
}

fn lamp(r: &mut Weave) {
    r.line(Vec3::new(BULB.x, 0.995, BULB.z), Vec3::new(BULB.x, 0.46, BULB.z), LAMP, SOLID);
    for (y, rad) in [(0.46, 0.055), (0.37, 0.13), (0.27, 0.20)] {
        r.strand(LAMP, EMIT, TAU * rad, |u| shade(y, rad, u));
    }
    for k in 0..12 {
        let u = k as f32 / 12.0;
        r.line(shade(0.46, 0.055, u), shade(0.27, 0.20, u), LAMP, EMIT);
    }
}

/// A side table at the couch's front end, four legs under it, and on it the
/// telephone: a base and a handset resting across it.
fn table(r: &mut Weave) {
    r.boxy(Vec3::new(-0.78, -0.60, 0.55), Vec3::new(0.16, 0.02, 0.13), TABLE, SOLID);
    for (dx, dz) in [(-0.14, -0.11), (0.14, -0.11), (-0.14, 0.11), (0.14, 0.11)] {
        r.boxy(
            Vec3::new(-0.78 + dx, -0.81, 0.55 + dz),
            Vec3::new(0.014, 0.19, 0.014),
            TABLE,
            SOLID,
        );
    }
    r.boxy(Vec3::new(-0.78, -0.545, 0.55), Vec3::new(0.075, 0.035, 0.05), TABLE, SOLID);
    r.boxy(Vec3::new(-0.78, -0.495, 0.55), Vec3::new(0.085, 0.02, 0.03), TABLE, SOLID);
}

/// In the wall ahead: a three-by-three window, a rod, and two panels of
/// curtain hanging beside it.
///
/// The fold in the curtain is **baked**, not animated — this model does not
/// move a vertex, and a curtain that swayed would be the first one it ever
/// moved. It deepens toward the floor so the panel hangs rather than
/// corrugating, and the strands follow the fold because [`Weave::sheet`]
/// samples the surface instead of drawing its chord.
fn window(r: &mut Weave) {
    // The panes glow. Drawn as wall, the frame sat at the wall's own
    // brightness with the wall's own grid running behind it and vanished into
    // it — three of the six things asked for, invisible. **A hole in a wall is
    // not wall.** In a room lit by one lamp a window is the brightest thing in
    // it anyway, so this is the reading the model already implies rather than
    // an exception made to be seen.
    r.flat(
        Vec3::new(-0.70, -0.28, 0.985),
        Vec3::X * 0.62,
        Vec3::Y * 0.74,
        WINDOW,
        GLASS,
    );
    r.line(Vec3::new(-0.86, 0.56, 0.93), Vec3::new(0.08, 0.56, 0.93), WINDOW, SOLID);
    for x0 in [-0.86f32, -0.18] {
        r.sheet(7, 5, WINDOW, CLOTH, |u, v| {
            let fold = (u * 9.0).sin() * 0.045 * (0.35 + 0.65 * v);
            Vec3::new(x0 + 0.26 * u, 0.54 - 0.96 * v, 0.93 + fold)
        });
    }
}

/// In the same wall, ajar.
///
/// Flat in its frame a door is a rectangle on a wall and reads as one. Swung,
/// it is the only thing in the room that says which side of it you are on —
/// and the wedge of nothing behind it is the only place in this room where the
/// grid stops, which is worth more here than a seventh object would be.
fn door(r: &mut Weave) {
    let hinge = Vec3::new(0.28, -1.0, 0.985);
    // Solid rather than wall, for the reason `window` records: at the wall's
    // own brightness a frame is lost in the wall's own grid.
    r.flat(hinge, Vec3::X * 0.64, Vec3::Y * 1.34, DOOR, SOLID);
    let a = 0.46f32;
    let swing = Vec3::new(a.cos(), 0.0, -a.sin()) * 0.64;
    r.sheet(3, 6, DOOR, SOLID, |u, v| hinge + swing * u + Vec3::Y * (1.34 * v));
}

/// A vertex buffer that is never zero-sized: a model may draw no translucent
/// faces at all, and wgpu still wants a buffer to bind before a zero-count draw.
fn vertex_buffer(device: &wgpu::Device, label: &str, bytes: &[u8]) -> wgpu::Buffer {
    let contents = if bytes.is_empty() { &[0u8; 4][..] } else { bytes };
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents,
        usage: wgpu::BufferUsages::VERTEX,
    })
}

impl Room {
    pub fn new() -> Room {
        Room {
            gpu: Built::default(),
            backdrop: Built::default(),
            surfaces: PartSet::new(SURFACES),
        }
    }

    /// The whole room: `faces` walls at `n` lines each way, then the first
    /// `things` of the six objects. Lines are the room as it has always been;
    /// the filled and glass lists are the same points as faces, so a material
    /// has something to land on.
    fn build_mesh(device: &wgpu::Device, n: u32, faces: u32, things: u32) -> Mesh {
        let mut r = room();

        for xf in cube_faces().iter().take(faces as usize) {
            r.sheet(n - 1, n - 1, ROOM, WALL, |u, v| {
                xf.transform_point3(Vec3::new((u - 0.5) * FACE, 0.0, (v - 0.5) * FACE))
            });
        }

        let built: [fn(&mut Weave); 6] = [couch, television, lamp, table, window, door];
        for build in built.iter().take(things as usize) {
            build(&mut r);
        }

        Mesh {
            lines: vertex_buffer(device, "room lines", bytemuck::cast_slice(&r.v)),
            line_count: r.v.len() as u32,
            fill: vertex_buffer(device, "room fill", bytemuck::cast_slice(&r.fill)),
            fill_count: r.fill.len() as u32,
            glass: vertex_buffer(device, "room glass", bytemuck::cast_slice(&r.glass)),
            glass_count: r.glass.len() as u32,
        }
    }

    fn build(device: &wgpu::Device, format: wgpu::TextureFormat) -> Gpu {
        let shader = crate::shared::build_shader(
            device,
            "room",
            concat!(
                include_str!("../../shaders/surface.wgsl"),
                "\n",
                include_str!("../../shaders/material.wgsl"),
                "\n",
                include_str!("../../shaders/room.wgsl")
            ),
        );
        let mut entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }];
        entries.extend(surface::layout_entries(SURFACES.len()));
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("room bgl"),
            entries: &entries,
        });
        let line_pipeline = m3::build_pipeline(
            device,
            format,
            &shader,
            &bind_group_layout,
            "room line pipeline",
            wgpu::PrimitiveTopology::LineList,
            Some(LAYOUT),
        );
        let fill_pipeline = m3::build_pipeline_ex(
            device,
            format,
            &shader,
            &bind_group_layout,
            "room fill pipeline",
            wgpu::PrimitiveTopology::TriangleList,
            Some(LAYOUT),
            "fs_fill",
            true,
        );
        let glass_pipeline = m3::build_pipeline_ex(
            device,
            format,
            &shader,
            &bind_group_layout,
            "room glass pipeline",
            wgpu::PrimitiveTopology::TriangleList,
            Some(LAYOUT),
            "fs_fill",
            false,
        );
        Gpu { line_pipeline, fill_pipeline, glass_pipeline, bind_group_layout, depth: DepthCache::default(), mesh: None }
    }
}

impl Model for Room {
    fn id(&self) -> &'static str {
        "room"
    }

    fn name(&self) -> &'static str {
        "Room"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn backdrop(&self) -> bool {
        true
    }

    /// H4: a `cam.track` node may drive the reserved `cam_x/cam_y/cam_z`.
    fn camera_track(&self) -> bool {
        true
    }

    fn surfaces(&self) -> &'static [&'static str] {
        SURFACES
    }

    fn is_3d(&self) -> bool {
        true
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
        self.surfaces.sync(ctx.device, ctx.queue);
        let gpu = self.gpu.get(format, || Self::build(ctx.device, format));

        // The three structural floats are the mesh's identity. `caras` and
        // `muebles` join `lineas` here because the room is baked now: they
        // used to be an instance count and a thing that did not exist.
        let key = (
            (floats[7].round() as u32).clamp(2, 40),
            (floats[8].round() as u32).clamp(1, 6),
            (floats[9].round() as u32).clamp(0, 6),
        );
        if !matches!(&gpu.mesh, Some((_, built)) if *built == key) {
            gpu.mesh = Some((Self::build_mesh(ctx.device, key.0, key.1, key.2), key));
        }
        let Some((mesh, _)) = &gpu.mesh else { return };

        // Offsets are derived from `SCHEMA`, not written down: `relleno` was
        // appended to the model's own block, which is why the camera, the
        // colour slot and the background all moved one place (rule 2).
        let cam_at = SCHEMA.offset(1);
        let fill = floats[cam_at - 1];

        // The camera wanders on a slow ellipse around wherever it was put, and
        // its aim wanders on a slower one. From inside a room with things in it
        // that reads as moving around them — which is the only motion this
        // model has, and now the only one that reveals anything.
        let drift = floats[4].clamp(0.0, 1.0);
        let cam = &floats[cam_at..cam_at + 7];
        let wander = Vec3::new(
            (ctx.time * 0.23).sin() * 0.42,
            (ctx.time * 0.17 + 1.3).sin() * 0.22,
            (ctx.time * 0.19 + 2.1).cos() * 0.42,
        ) * drift;
        let aim = Vec3::new(
            (ctx.time * 0.13 + 0.7).sin() * 0.30,
            (ctx.time * 0.11 + 2.4).sin() * 0.16,
            (ctx.time * 0.15 + 4.0).cos() * 0.30,
        ) * drift;
        let eye = Vec3::new(cam[0], cam[1], cam[2]) + wander;
        let view_proj = crate::model::tracked_view_proj(
            eye,
            Vec3::new(cam[3], cam[4], cam[5]) + aim,
            cam[6],
            ctx.aspect(),
            crate::model::camera_track_offset(floats),
        );

        let color_at = SCHEMA.offset(2);
        let background_at = SCHEMA.offset(3);
        let uniform = Uniform {
            color: ColorSlotGpu::from_floats(&floats[color_at..color_at + 12]),
            floats: [floats[0], floats[1], floats[2], floats[3]],
            floats2: [ctx.time, floats[5], floats[6], fill],
            bulb: [BULB.x, BULB.y, BULB.z, 0.0],
            eye: [eye.x, eye.y, eye.z, 0.0],
            view_proj,
            mat_walls: self.surfaces.uniform(0),
            mat_couch: self.surfaces.uniform(1),
            mat_tv: self.surfaces.uniform(2),
            mat_table: self.surfaces.uniform(3),
            mat_window: self.surfaces.uniform(4),
            mat_door: self.surfaces.uniform(5),
            screen: [ctx.width as f32, ctx.height as f32, 0.0, 0.0],
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("room uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }];
        entries.extend(self.surfaces.bind_entries());
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("room bind group"),
            layout: &gpu.bind_group_layout,
            entries: &entries,
        });

        let depth_view = gpu.depth.get(ctx.device, ctx.width, ctx.height);

        let background = ColorSlotGpu::from_floats(&floats[background_at..background_at + 12]);

        // The background, first and as its own pass — it is what clears the
        // frame now, and the pass below loads over it. See `shared::backdrop`
        // for why this is a gradient rather than a clear colour.
        self.backdrop
            .get(format, || Backdrop::new(ctx.device, format))
            .draw(ctx, target, background, 0.0, ctx.fondo_alpha);

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("room encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("room pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &bind_group, &[]);
            // At `relleno = 0` the room is exactly the line room it has always
            // been: nothing is filled, so the frame is byte-identical.
            if fill > 0.0 {
                pass.set_pipeline(&gpu.fill_pipeline);
                pass.set_vertex_buffer(0, mesh.fill.slice(..));
                pass.draw(0..mesh.fill_count, 0..1);
                pass.set_pipeline(&gpu.glass_pipeline);
                pass.set_vertex_buffer(0, mesh.glass.slice(..));
                pass.draw(0..mesh.glass_count, 0..1);
            }
            pass.set_pipeline(&gpu.line_pipeline);
            pass.set_vertex_buffer(0, mesh.lines.slice(..));
            pass.draw(0..mesh.line_count, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

    /// A solid red picture on each part, at each mapping, is drawn through the
    /// shader: it changes the frame and adds red, and taking it away again
    /// restores the frame exactly.
    #[test]
    fn a_picture_on_each_part_reaches_the_frame_and_clearing_it_restores_it() {
        use crate::model::{Mapping, SurfaceSpec};
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (160u32, 160u32);
        let floats = SCHEMA.defaults();
        let red: Vec<u8> = [255u8, 0, 0, 255].repeat(16);
        let (texture, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let mut model = Room::new();
        let ctx = Ctx {
            device: &gpu.device,
            queue: &gpu.queue,
            time: 3.0,
            width: w,
            height: h,
            fondo_alpha: false,
        };
        let draw = |model: &mut Room| {
            model.render(&ctx, &floats, &view, FORMAT);
            crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &texture, w, h)
        };
        let red_sum = |f: &[u8]| f.chunks(4).map(|p| p[0] as u64).sum::<u64>();
        assert_eq!(model.surfaces(), &["walls", "couch", "tv", "table", "window", "door"]);

        let plain = draw(&mut model);
        for part in ["walls", "couch", "tv", "table", "window", "door"] {
            for mapping in [Mapping::Object, Mapping::Projector] {
                model.set_surface(part, Some(SurfaceSpec::image(&red, 4, 4, 1.0, mapping, 1.0)));
                let painted = draw(&mut model);
                assert_ne!(plain, painted, "{part} {mapping:?}: a red picture did not change the frame");
                assert!(
                    red_sum(&painted) > red_sum(&plain),
                    "{part} {mapping:?}: the red picture added no red: {} vs {}",
                    red_sum(&painted),
                    red_sum(&plain)
                );
                model.set_surface(part, None);
                let restored = draw(&mut model);
                assert_eq!(plain, restored, "{part} {mapping:?}: clearing the picture did not restore the frame");
            }
        }
    }

    /// `relleno` is the filled layer: at 0 the room is its lines, byte for
    /// byte; above it the same points are drawn as faces, so a material lands
    /// on the room. Turning it back to 0 restores the line frame exactly.
    #[test]
    fn relleno_fills_the_room_and_a_material_lands_on_the_faces() {
        use crate::model::{MaterialSpec, SurfaceSpec};
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (160u32, 160u32);
        let mut floats = SCHEMA.defaults();
        let fill = SCHEMA.offset(1) - 1;
        assert_eq!(SCHEMA.blocks[0].width(), fill + 1, "relleno is the last of the model's own floats");
        let (texture, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let mut model = Room::new();
        let ctx = Ctx {
            device: &gpu.device,
            queue: &gpu.queue,
            time: 3.0,
            width: w,
            height: h,
            fondo_alpha: false,
        };
        let draw = |model: &mut Room, floats: &[f32]| {
            model.render(&ctx, floats, &view, FORMAT);
            crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &texture, w, h)
        };

        let plain = draw(&mut model, &floats);
        floats[fill] = 1.0;
        let solid = draw(&mut model, &floats);
        assert_ne!(plain, solid, "relleno drew no filled faces");

        model.set_surface(
            "walls",
            Some(SurfaceSpec::material(MaterialSpec { kind: 4.0, mix: 1.0, ..Default::default() })),
        );
        let metal = draw(&mut model, &floats);
        assert_ne!(solid, metal, "a material on `walls` did not change the solid room");
        model.set_surface("walls", None);

        floats[fill] = 0.0;
        let restored = draw(&mut model, &floats);
        assert_eq!(plain, restored, "relleno = 0 is not the line room");
    }
}
