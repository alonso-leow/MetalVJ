//! Model 30 — River (`river`). Matrix's grammar, taken outdoors: a river running
//! away from you between two banks, stones in the water and along it, and
//! bushes on the bank that answer the wind. Everything is grid, nothing has a
//! colour of its own, and the only light is the moon sitting low at the far end
//! of the water.
//!
//! **The ask** (2026-08-24): *"a new animation based on Matrix but of a
//! landscape with a river and stones around it, some bushes that react to the
//! wind, parameters for the wind and the water, plus all the ones already in
//! Matrix to control the grid."*
//!
//! # Why this is a model and not a Matrix preset
//!
//! Matrix's founding invariant is one sentence: *nothing here is displaced.*
//! The curtain's fold is baked, the door's swing is baked, and every animation
//! in that model is luminance, colour, or where you are standing. A bush that
//! reacts to the wind is a vertex that moves, and water that runs is a whole
//! surface that moves. **The ask breaks the invariant, so it gets its own
//! model** rather than a seventh object in the room and a flag that suspends
//! the rule Matrix is built on.
//!
//! What it does *not* break is the thing under the invariant, which is the
//! part worth keeping: **the frame is still a pure function of `(floats,
//! time)`.** No state is carried between frames, and the displacement is
//! analytic — a gust is a noise field travelling across the valley, a wave is
//! three sines running downstream, both read at the vertex from the clock.
//! Ask for the same instant twice and you get the same river, which is what
//! the render gate, the presets and the automix all rest on.
//!
//! # The rules, which are Matrix's
//!
//!   * **One light.** The moon is at [`MOON`], low over the water at the far
//!     end, and distance from it *is* the shading — so the river runs *into*
//!     the light and the bank at your feet is the dark part of the frame. This
//!     is the same rule as the room's hanging lamp with the sign of the scene
//!     flipped: in a room you stand near the light, outdoors you never do.
//!   * **Colour is depth.** `profundidad` walks the gradient by distance from
//!     the eye rather than by brightness, exactly as in the room.
//!   * **One weave.** A stone, a bank and a bush are cut from the same cloth at
//!     the same rate — `shared::weave` is the builder, shared with Matrix for
//!     precisely this reason.
//!   * **A role is a rate, not a tint.** Five of them again — bank, stone,
//!     moon, leaf, water — and not one of them carries a colour. There is one
//!     colour slot in the whole model, because a second one would be an object
//!     colour by another name.
//!
//! # The two things the shader is told and cannot work out
//!
//! Displacement needs to know where a point sits *within the thing it belongs
//! to*, and neither the terrain function nor the shoreline exists in WGSL — a
//! second copy of [`bed`] written in the shader is two valleys that drift
//! apart. So the mesh carries it, in the `tag.x` slot `shared::weave` leaves
//! to the model: **how deep the water is** under a water vertex, so a wave
//! flattens to nothing at the shoreline instead of climbing the bank, and
//! **how high above its own root** a leaf vertex is, so a gust bends the tip
//! and leaves the bush planted.
//!
//! # `caudal` is a rebuild key, and that is not a compromise
//!
//! The water surface is not a rectangle: it stops where the bed comes up to
//! meet it, and that shoreline is a different curve at every level. Making the
//! level a shader uniform would mean either a sheet built at full width whose
//! edges bury themselves in the bank, or the bed function living in two
//! languages. Making it structural means the shoreline is *real* — raise
//! `caudal` and the river takes more of the bank, and the wet line on the
//! stones moves with it. It is quantised to [`FLOW_STEPS`] so a slider drag
//! is a dozen rebuilds of a few thousand vertices, not one per frame.

use std::f32::consts::{PI, TAU};

use glam::Vec3;
use wgpu::util::DeviceExt;

use crate::model::{
    camera, color_at, background, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema,
    SurfaceSpec,
};
use crate::shared::backdrop::Backdrop;
use crate::shared::mesh::{self as m3, DepthCache};
use crate::shared::surface::{self, MaterialU, PartSet};
use crate::shared::weave::{Weave, LAYOUT};
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    // ---- Matrix's grid floats, by the same names and doing the same job ----
    spec("shine", 0.0, 2.0, 1.0),
    spec("variation", 0.0, 1.0, 0.40),
    spec("pulse", 0.0, 3.0, 0.7),
    spec("sweep", 0.0, 1.0, 0.40),
    spec("drift", 0.0, 1.0, 0.30),
    // Matrix's `lampara` under the name the light actually has out here. 0 is
    // the valley lit flat; 1 hands it all to the moon, and then brightness is
    // distance to the far end of the river and nothing else.
    spec("moon", 0.0, 1.0, 0.70),
    spec("depth", 0.0, 1.0, 0.60),
    // ---- the water ----
    // How fast everything on the surface runs downstream: the wave trains, the
    // advected texture, and how hard the stones stand against it.
    spec("current", 0.0, 2.0, 0.90),
    // How high the surface stands. Tapers to nothing at the shoreline, so this
    // is chop in the middle of the river and never a wave up the bank.
    spec("swell", 0.0, 1.0, 0.50),
    // White water: crests, the collar around a stone that sits in the stream,
    // and the wake behind it. The one float that makes the river read as fast
    // rather than merely moving.
    spec("foam", 0.0, 1.0, 0.50),
    // ---- the wind ----
    spec("wind", 0.0, 1.5, 0.60),
    // 0 is a steady breeze — every bush at the same lean, which reads as a
    // still photograph of a windy day. 1 is gusts: a noise field travelling
    // across the valley, so the bushes go over in sequence and the wind has a
    // direction you can *see* rather than one you are told.
    spec("gusts", 0.0, 1.0, 0.60),
    // Which way it blows, once around the compass. Held: a band spinning the
    // wind through 360 degrees on every beat is not weather, it is a bug that
    // renders.
    spec_held("heading", 0.0, 1.0, 0.15, 0.0),
    // ---- structural: what there is to weave ----
    spec_held("lines", 4.0, 40.0, 18.0, 0.15),
    // How full the river runs. A rebuild key — see the module header.
    spec_held("flow", 0.0, 1.0, 0.55, 0.0),
    spec_held("stones", 0.0, 24.0, 14.0, 0.0),
    spec_held("bushes", 0.0, 16.0, 9.0, 0.0),
    // How much of the valley is filled surface rather than bare grid. 0 is the
    // line-only valley this model has always drawn, byte for byte; 1 fills the
    // ground, the water and the stones with faces that carry a real normal, so
    // the river takes a picture and a material like any other 3D model.
    // Appended (rule 2).
    spec("fill", 0.0, 1.0, 0.0),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        // Crouched at the near bank, a hand's height over the water, looking
        // downstream at the moon. Low on purpose: from standing height a river
        // is a stripe on a plane, and from here the far bank rises across the
        // top of the frame and the water fills the bottom half of it.
        camera([-1.00, 0.62, -1.90], [0.35, -0.20, 5.0], 70.0),
        // Matrix's ramp, unchanged, and for Matrix's reason: no daylight
        // anywhere in it, so nothing can be mistaken for the colour a thing
        // *is*. A river painted river-colours would be exactly the mistake the
        // whole family of models exists to avoid.
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

/// How far the valley runs across, and where it starts and ends downstream.
/// The camera sits just inside [`NEAR`] and the moon just inside [`FAR`].
const ACROSS: f32 = 3.6;
const NEAR: f32 = -2.6;
const FAR: f32 = 14.0;

/// How deep the channel is cut below the plain, and how high the bank climbs
/// away from it.
const DEPTH: f32 = 0.85;
const RISE: f32 = 0.62;

/// The weave's two rates, at this world's scale. Coarser per unit than
/// Matrix's room by a long way and with a far higher cap, which is the whole
/// reason `shared::weave` takes them as arguments: nine points per unit over
/// seventeen units of river would be a hundred and fifty samples a strand, and
/// eighteen of them would be a wave train sampled four times per wavelength.
const POINTS_PER_UNIT: f32 = 22.0;
const MAX_POINTS: usize = 128;
const CELLS_PER_UNIT: f32 = 4.5;
/// Higher than the room's 8 because a boulder can be a metre across here and
/// the room's largest single surface was a wall. At 8 the near stones came out
/// as visible octagons.
const MAX_CELLS: u32 = 14;

/// Where the moon hangs: downstream, off to one side, and low. It is the only
/// light, so the shader needs it and it is a constant here rather than a
/// number buried in [`moon`].
const MOON: Vec3 = Vec3::new(1.05, 0.58, 12.6);

/// How many levels `caudal` is quantised to before it becomes a rebuild key.
const FLOW_STEPS: f32 = 14.0;

/// How many stones standing in the water the surface is told about. The water
/// runs *around* the stones in it, and that costs one uniform slot per stone,
/// so it is capped and the ones on the bank never take a slot.
const MAX_WAKES: usize = 10;

// Which thing a strand belongs to. The ground and the moon are one each; a
// stone and a bush get their own number so that no two of them flicker in step
// or answer the same gust identically.
const GROUND: f32 = 0.0;
const RIVER: f32 = 1.0;
const SKY: f32 = 2.0;
const STONE0: f32 = 8.0;
const BUSH0: f32 = 64.0;

// How a strand is lit — the only "material" here, and it carries no colour.
// Five, as in the room, and in the same order: the flat backdrop first, the
// solid thing standing on it, the thing that makes its own light, and then the
// two that move.
const BANK: f32 = 0.0;
const STONE: f32 = 1.0;
const MOONLIT: f32 = 2.0;
const LEAF: f32 = 3.0;
const WATER: f32 = 4.0;

/// The parts a picture or a material can be laid on (plan: surface sources,
/// materials M6): `ground` is the banks and the valley floor, `water` the river
/// surface, `stones` the rocks and `bushes` the leaves. Their index is the
/// `surfaces` slot in [`Rio`].
const SURFACES: &[&str] = &["ground", "water", "stones", "bushes"];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    color: ColorSlotGpu,
    floats: [f32; 4],
    floats2: [f32; 4],
    aire: [f32; 4],
    water: [f32; 4],
    moon: [f32; 4],
    eye: [f32; 4],
    view_proj: [[f32; 4]; 4],
    wakes: [[f32; 4]; MAX_WAKES],
    // Materials (appended), one `MaterialU` per part in `SURFACES` order: each
    // carries the part's picture setting and its `mat_*` floats, see
    // `material.wgsl`. The viewport in pixels for projector mapping follows,
    // with `relleno` in `.z`.
    mat_ground: MaterialU,
    mat_water: MaterialU,
    mat_stones: MaterialU,
    mat_bushes: MaterialU,
    screen: [f32; 4],
}

/// The baked valley, the structural floats it was baked for, and the stones
/// the water has to be told about — they come out of the same build, because
/// where a stone ended up is decided by the same hashes that placed it. The
/// lines are the valley as it has always been; the filled and glass lists are
/// the same points as faces, so a material has something to land on.
struct Mesh {
    lines: wgpu::Buffer,
    line_count: u32,
    fill: wgpu::Buffer,
    fill_count: u32,
    glass: wgpu::Buffer,
    glass_count: u32,
    key: (u32, u32, u32, u32),
    wakes: Vec<[f32; 4]>,
}

struct Gpu {
    line_pipeline: wgpu::RenderPipeline,
    fill_pipeline: wgpu::RenderPipeline,
    glass_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    depth: DepthCache,
    mesh: Option<Mesh>,
}

pub struct Rio {
    gpu: Built<Gpu>,
    backdrop: Built<Backdrop>,
    /// `ground`, `water`, `stones`, `bushes`, in `SURFACES` order.
    surfaces: PartSet<4>,
}

// ---------------------------------------------------------------------------
// the valley
// ---------------------------------------------------------------------------

/// Where the channel's centre line is, at `z`. Two sines well off a common
/// multiple, so the river never repeats over the length you can see.
fn meander(z: f32) -> f32 {
    0.55 * (z * 0.19).sin() + 0.25 * (z * 0.41 + 1.2).sin()
}

/// How wide the channel is there. Varying it is what stops the water reading
/// as a canal: the river pinches and opens, and the shoreline the water mesh
/// is cut to follows it.
fn halfwidth(z: f32) -> f32 {
    1.25 + 0.40 * (z * 0.23 + 0.7).sin()
}

fn hash2(i: f32, j: f32) -> f32 {
    let n = (i * 127.1 + j * 311.7).sin() * 43758.5453;
    n - n.floor()
}

/// Value noise, -1..1, on the CPU. Small on purpose: it is only ever asked for
/// at the low frequencies that make a bank lumpy, so the plateau problem
/// `shared.wgsl` documents for `hash21` never gets a chance to bite here.
fn lump(x: f32, z: f32) -> f32 {
    let (i, j) = (x.floor(), z.floor());
    let (fx, fz) = (x - i, z - j);
    let (wx, wz) = (fx * fx * (3.0 - 2.0 * fx), fz * fz * (3.0 - 2.0 * fz));
    let a = hash2(i, j);
    let b = hash2(i + 1.0, j);
    let c = hash2(i, j + 1.0);
    let d = hash2(i + 1.0, j + 1.0);
    (a + (b - a) * wx + (c - a) * wz + (a - b - c + d) * wx * wz) * 2.0 - 1.0
}

/// The ground, everywhere. A U-shaped channel inside the banks and a rise
/// outside them, plus lumps that are damped in the water and full strength up
/// the bank — because a riverbed is scoured smooth and a bank is not.
///
/// **The one function in this model that exists only on the CPU.** The shader
/// never calls it and never needs to: everything it would want from the
/// terrain is baked into the mesh instead. See the module header.
fn bed(x: f32, z: f32) -> f32 {
    let d = (x - meander(z)).abs() / halfwidth(z);
    let base = if d < 1.0 {
        -DEPTH * (1.0 - d * d)
    } else {
        RISE * (d - 1.0).min(1.9).powf(1.15)
    };
    let rough = 0.105 * lump(x * 1.1, z * 0.7) + 0.042 * lump(x * 2.9, z * 2.1);
    base + rough * (0.25 + 0.75 * d.min(1.0))
}

/// Where the water surface sits, from `caudal`. Never below the deepest lump
/// in the bed and never over the rim, so there is always a river and it is
/// never a flood.
fn level(flow: f32) -> f32 {
    -DEPTH + 0.10 + (DEPTH - 0.18) * flow.clamp(0.0, 1.0)
}

/// The two shorelines at `z`: march out from the channel's centre until the
/// bed comes up past the water. Marched rather than solved because the bed has
/// noise in it and the noise is the point — a bisection would find *a*
/// crossing, and what is wanted is the outermost one before the bank.
fn shore(z: f32, level: f32) -> (f32, f32) {
    let c = meander(z);
    let (mut l, mut r) = (c - 0.02, c + 0.02);
    let mut x = c;
    while x > c - 2.4 {
        if bed(x, z) > level {
            break;
        }
        l = x;
        x -= 0.02;
    }
    let mut x = c;
    while x < c + 2.4 {
        if bed(x, z) > level {
            break;
        }
        r = x;
        x += 0.02;
    }
    (l.min(c - 0.02), r.max(c + 0.02))
}

/// How deep the water is at a point, 0 at the shoreline and 1 in the deepest
/// part of the channel. This is what the wave amplitude is multiplied by, and
/// it is the whole reason the water mesh carries a value in `tag.x`.
fn submersion(x: f32, z: f32, level: f32) -> f32 {
    ((level - bed(x, z)) / (DEPTH * 0.85)).clamp(0.0, 1.0)
}

/// Downstream, warped. A landscape sampled evenly in `z` spends most of its
/// strands past the horizon and almost none where the camera is looking; the
/// power puts them where the frame is, which under perspective comes out as
/// roughly even spacing on screen.
fn along(u: f32) -> f32 {
    NEAR + (FAR - NEAR) * u.clamp(0.0, 1.0).powf(1.55)
}

fn rnd(seed: f32) -> f32 {
    let n = (seed * 127.1 + 43.7).sin() * 43758.5453;
    n - n.floor()
}

// ---------------------------------------------------------------------------
// what is in it
// ---------------------------------------------------------------------------

/// The ground: one sheet over the whole valley, channel included. The bed
/// under the water is drawn too — in a model where nothing is opaque, a
/// riverbed seen through its own surface is free, and it is most of what makes
/// the water read as water rather than as a lid.
fn valley(w: &mut Weave, n: u32) {
    let nx = n.max(3);
    let nz = (n as f32 * 2.0).round() as u32;
    let span = 2.0 * ACROSS;
    for i in 0..=nx {
        let x = -ACROSS + span * (i as f32 / nx as f32);
        w.strand(GROUND, BANK, FAR - NEAR, |u| {
            let z = along(u);
            Vec3::new(x, bed(x, z), z)
        });
    }
    for j in 0..=nz {
        let z = along(j as f32 / nz as f32);
        w.strand(GROUND, BANK, span, |u| {
            let x = -ACROSS + span * u;
            Vec3::new(x, bed(x, z), z)
        });
    }
    // The filled ground: the same sheet as faces, so the bank takes a material.
    w.quads(nx, nz, GROUND, BANK, |u, v| {
        let x = -ACROSS + span * u;
        let z = along(v);
        (Vec3::new(x, bed(x, z), z), 0.0)
    });
}

/// The water: the same sheet cut to the shoreline instead of to a rectangle.
/// Lengthwise strands hold a constant fraction *across* the river, so they
/// bend with the meander and pinch where it narrows — which is why the river
/// looks like a river even before a single wave has moved.
fn river(w: &mut Weave, n: u32, level: f32) {
    // Denser than the ground it runs through, both ways. This is the room's
    // television trick — *the grid running at a different rate* — and it is
    // doing the same job: at the same rate as the bank the water was a patch
    // of bank that happened to be flat, and no amount of brightness fixed it.
    let nx = n.max(4);
    let nz = (n as f32 * 3.0).round() as u32;
    for i in 0..=nx {
        let s = i as f32 / nx as f32;
        w.strand_val(RIVER, WATER, FAR - NEAR, |u| {
            let z = along(u);
            let (l, r) = shore(z, level);
            let x = l + (r - l) * s;
            (Vec3::new(x, level, z), submersion(x, z, level))
        });
    }
    for j in 0..=nz {
        let z = along(j as f32 / nz as f32);
        let (l, r) = shore(z, level);
        w.strand_val(RIVER, WATER, r - l, |u| {
            let x = l + (r - l) * u;
            (Vec3::new(x, level, z), submersion(x, z, level))
        });
    }
    // The filled water: the same sheet, cut to the shoreline, as faces. The
    // `WATER` role is the model's translucent one, so they land in `glass`.
    w.quads(nx, nz, RIVER, WATER, |u, v| {
        let z = along(v);
        let (l, r) = shore(z, level);
        let x = l + (r - l) * u;
        (Vec3::new(x, level, z), submersion(x, z, level))
    });
}

/// One stone: a lumpy squashed ellipsoid of grid. Not a box — Matrix could get
/// away with boxes because a couch *is* boxes, and a boulder drawn as a crate
/// reads as a crate at any line density.
fn stone(w: &mut Weave, c: Vec3, r: f32, seed: f32, obj: f32) {
    let squash = Vec3::new(1.0 + 0.35 * rnd(seed + 2.3), 0.58 + 0.30 * rnd(seed + 5.1), 1.0);
    let nu = w.cells(TAU * r * 1.6).max(5);
    let nv = w.cells(PI * r * 1.6).max(3);
    w.sheet(nu, nv, obj, STONE, |u, v| {
        let (a, b) = (u * TAU, v * PI);
        let dir = Vec3::new(b.sin() * a.cos(), b.cos(), b.sin() * a.sin());
        // Two low harmonics, seeded per stone: enough that no two are the same
        // stone, cheap enough that a stone is still a stone.
        let bumps = 1.0
            + 0.18 * (a * 3.0 + seed * 4.0).sin() * b.sin()
            + 0.13 * (b * 4.0 + seed * 7.0).sin();
        c + dir * squash * (r * bumps)
    });
}

/// The stones, scattered by hash — some standing in the stream, the rest along
/// the banks. The ones in the water are handed back so the surface can be told
/// where they are: a river with a rock in it has a bulge in front of the rock
/// and a wake behind it, and without that the rock is merely a rock that
/// happens to be wet.
fn stones(w: &mut Weave, count: u32, level: f32) -> Vec<[f32; 4]> {
    let mut wakes = Vec::new();
    for k in 0..count {
        let f = k as f32;
        let z = along(0.04 + 0.92 * rnd(f * 1.7 + 0.3));
        // Which bank, by parity rather than by another hash. Three different
        // hashes of `k` were tried and all three came out lopsided at these
        // counts — eight of nine bushes on the right bank, which reads as a
        // deliberate arrangement rather than as scatter. A coin flip that is
        // fair only in the limit is not fair over fourteen stones.
        let side = if k % 2 == 0 { -1.0 } else { 1.0 };
        let wet = k % 5 < 2;
        let d = if wet {
            side * rnd(f * 5.1 + 0.7) * 0.72
        } else {
            side * (1.02 + 0.95 * rnd(f * 5.1 + 0.7))
        };
        let x = (meander(z) + d * halfwidth(z)).clamp(-ACROSS + 0.3, ACROSS - 0.3);
        let r = 0.07 + 0.17 * rnd(f * 9.3 + 4.4);
        let c = Vec3::new(x, bed(x, z) + r * 0.42, z);
        stone(w, c, r, f, STONE0 + f);
        if wet && wakes.len() < MAX_WAKES && c.y - r * 0.42 < level {
            wakes.push([x, z, r, 1.0]);
        }
    }
    wakes
}

/// One bush: a fountain of twigs from a single root, plus two crown rings
/// through them.
///
/// The rings are what make it a bush rather than a firework, and they are the
/// reason `tag.x` carries height rather than the twig's own parameter: a ring
/// runs *around* the bush, so how far along its strand a point is says nothing
/// about how high it is, and a gust bent by the wrong number would shear the
/// crown off sideways instead of laying the whole bush over.
fn bush(w: &mut Weave, root: Vec3, size: f32, seed: f32, obj: f32) {
    let twigs = 13 + (rnd(seed + 1.3) * 7.0) as u32;
    let height = size * (0.72 + 0.46 * rnd(seed + 3.7));
    // How far out a twig has leaned by the time it is `u` of the way up. The
    // rings sample the same shape at a fixed height, so they sit on the twigs
    // instead of near them.
    //
    // **It opens fast and then stops.** A monotone power — which is what this
    // was — puts every twig's widest point at its tip, and a fan of strands
    // narrow at the bottom and wide at the top with rings around the top is an
    // umbrella. It rendered as one. A bush is widest somewhere in its middle,
    // so the term that opens it is rooted and the term that closes it is
    // linear, and the maximum lands around six tenths of the way up.
    let spread = |u: f32| size * 1.45 * u.powf(0.40) * (1.0 - 0.80 * u * u);
    let climb = |u: f32| height * (1.0 - (1.0 - u) * (1.0 - u));
    let angle = |k: u32| (k as f32 / twigs as f32) * TAU + rnd(seed + k as f32 * 0.7) * 0.55;
    // How much a twig curls round as it climbs. Without it every twig is a
    // straight radial line in plan, and a bush seen from above — which happens
    // the moment the drift walks the camera over one — is a spoked wheel.
    let curl = |k: u32| (rnd(seed + k as f32 * 2.3 + 1.7) - 0.5) * 1.1;
    let lean = |k: u32| 0.55 + 0.75 * rnd(seed + k as f32 * 1.9 + 0.4);

    let arm = |k: u32, u: f32| {
        let a = angle(k) + curl(k) * u;
        Vec3::new(a.cos(), 0.0, a.sin()) * (spread(u) * lean(k))
    };
    for k in 0..twigs {
        w.strand_val(obj, LEAF, height * 1.6, |u| {
            let y = climb(u);
            (root + arm(k, u) + Vec3::Y * y, y / height.max(1e-4))
        });
    }
    for ring in [0.30f32, 0.55, 0.80] {
        let y = climb(ring);
        w.strand_val(obj, LEAF, TAU * spread(ring), |u| {
            // Around the crown, through where each twig is at this height, so
            // the ring is a closed loop with a kink at every twig rather than
            // a smooth circle floating past them.
            let t = u * twigs as f32;
            let k = (t.floor() as u32) % twigs;
            let k2 = (k + 1) % twigs;
            let f = t - t.floor();
            (root + arm(k, ring).lerp(arm(k2, ring), f) + Vec3::Y * y, y / height.max(1e-4))
        });
    }
}

/// The bushes, on the banks and never in the water. Placed by the same hashes
/// the stones are, off a different set of seeds.
fn bushes(w: &mut Weave, count: u32) {
    for k in 0..count {
        let f = k as f32;
        let z = along(0.05 + 0.90 * rnd(f * 2.3 + 5.9));
        let side = if k % 2 == 0 { -1.0 } else { 1.0 };
        // Close to the water. Further up the bank they are legal, in frame,
        // and pointless: the bank climbs fast enough that a bush at twice the
        // channel's half-width is a silhouette on a ridge with nothing near it
        // to be the wind against.
        let d = 0.95 + 0.80 * rnd(f * 6.7 + 3.2);
        let x = (meander(z) + side * d * halfwidth(z)).clamp(-ACROSS + 0.3, ACROSS - 0.3);
        let size = 0.17 + 0.26 * rnd(f * 8.9 + 1.4);
        bush(w, Vec3::new(x, bed(x, z) - size * 0.06, z), size, f * 3.1 + 0.5, BUSH0 + f);
    }
}

/// The moon: rings and spokes, drawn in the plane facing the camera. The one
/// emitter in the model, and the only thing in the frame that is not part of
/// the ground, the water or something standing on them.
fn moon(w: &mut Weave) {
    let r = 0.34;
    for k in 1..=3 {
        let rad = r * k as f32 / 3.0;
        w.strand(SKY, MOONLIT, TAU * rad, |u| {
            let a = u * TAU;
            MOON + Vec3::new(a.cos() * rad, a.sin() * rad, 0.0)
        });
    }
    for k in 0..8 {
        let a = k as f32 / 8.0 * TAU;
        let dir = Vec3::new(a.cos(), a.sin(), 0.0);
        w.line(MOON, MOON + dir * r, SKY, MOONLIT);
    }
}

impl Rio {
    pub fn new() -> Rio {
        Rio {
            gpu: Built::default(),
            backdrop: Built::default(),
            surfaces: PartSet::new(SURFACES),
        }
    }

    fn build_mesh(device: &wgpu::Device, key: (u32, u32, u32, u32)) -> Mesh {
        let (n, flow_step, rocks, shrubs) = key;
        let lvl = level(flow_step as f32 / FLOW_STEPS);
        let mut w = Weave::new(POINTS_PER_UNIT, MAX_POINTS, CELLS_PER_UNIT, MAX_CELLS);
        // The water is the valley's one translucent surface: its filled cells
        // must not write depth, so the bed it is drawn over still shows through.
        w.translucent = |role| role == WATER;

        valley(&mut w, n);
        river(&mut w, n, lvl);
        let wakes = stones(&mut w, rocks, lvl);
        bushes(&mut w, shrubs);
        moon(&mut w);

        let buffer = |label: &str, bytes: &[u8]| {
            let contents = if bytes.is_empty() { &[0u8; 4][..] } else { bytes };
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::VERTEX,
            })
        };
        Mesh {
            lines: buffer("rio lines", bytemuck::cast_slice(&w.v)),
            line_count: w.v.len() as u32,
            fill: buffer("rio fill", bytemuck::cast_slice(&w.fill)),
            fill_count: w.fill.len() as u32,
            glass: buffer("rio glass", bytemuck::cast_slice(&w.glass)),
            glass_count: w.glass.len() as u32,
            key,
            wakes,
        }
    }

    fn build(device: &wgpu::Device, format: wgpu::TextureFormat) -> Gpu {
        let shader = crate::shared::build_shader(
            device,
            "river",
            concat!(
                include_str!("../../shaders/surface.wgsl"),
                "\n",
                include_str!("../../shaders/material.wgsl"),
                "\n",
                include_str!("../../shaders/rio.wgsl")
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
            label: Some("rio bgl"),
            entries: &entries,
        });
        let line_pipeline = m3::build_pipeline(
            device,
            format,
            &shader,
            &bind_group_layout,
            "rio line pipeline",
            wgpu::PrimitiveTopology::LineList,
            Some(LAYOUT),
        );
        let fill_pipeline = m3::build_pipeline_ex(
            device,
            format,
            &shader,
            &bind_group_layout,
            "rio fill pipeline",
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
            "rio glass pipeline",
            wgpu::PrimitiveTopology::TriangleList,
            Some(LAYOUT),
            "fs_fill",
            false,
        );
        Gpu { line_pipeline, fill_pipeline, glass_pipeline, bind_group_layout, depth: DepthCache::default(), mesh: None }
    }
}

impl Model for Rio {
    fn id(&self) -> &'static str {
        "river"
    }

    fn name(&self) -> &'static str {
        "River"
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

        // The four structural floats are the valley's identity: how fine the
        // weave is, how full the river runs, and how many stones and bushes
        // there are. `caudal` is here for the reason the module header gives.
        let key = (
            (floats[13].round() as u32).clamp(4, 40),
            (floats[14].clamp(0.0, 1.0) * FLOW_STEPS).round() as u32,
            (floats[15].round() as u32).clamp(0, 24),
            (floats[16].round() as u32).clamp(0, 16),
        );
        if !matches!(&gpu.mesh, Some(m) if m.key == key) {
            gpu.mesh = Some(Self::build_mesh(ctx.device, key));
        }
        let Some(mesh) = &gpu.mesh else { return };

        // Offsets are derived from `SCHEMA`, not written down: `relleno` was
        // appended to the model's own block, which is why the camera, the
        // colour slot and the background all moved one place (rule 2).
        let cam_at = SCHEMA.offset(1);
        let fill = floats[cam_at - 1];

        // The camera wanders on a slow ellipse around wherever it was put, and
        // its aim on a slower one — Matrix's drift, at this world's scale.
        // Wider here than in the room: a valley you cross a hand's width of
        // reads as still, and the only motion in the frame that is not the
        // wind or the water is this one.
        let drift = floats[4].clamp(0.0, 1.0);
        let cam = &floats[cam_at..cam_at + 7];
        let wander = Vec3::new(
            (ctx.time * 0.19).sin() * 0.85,
            (ctx.time * 0.13 + 1.3).sin() * 0.16,
            (ctx.time * 0.11 + 2.1).cos() * 0.55,
        ) * drift;
        let aim = Vec3::new(
            (ctx.time * 0.09 + 0.7).sin() * 0.70,
            (ctx.time * 0.07 + 2.4).sin() * 0.14,
            (ctx.time * 0.10 + 4.0).cos() * 0.45,
        ) * drift;
        let eye = Vec3::new(cam[0], cam[1], cam[2]) + wander;
        let view_proj = crate::model::tracked_view_proj(
            eye,
            Vec3::new(cam[3], cam[4], cam[5]) + aim,
            cam[6],
            ctx.aspect(),
            crate::model::camera_track_offset(floats),
        );

        let mut wakes = [[0.0f32; 4]; MAX_WAKES];
        for (slot, stone) in wakes.iter_mut().zip(&mesh.wakes) {
            *slot = *stone;
        }

        let color_at = SCHEMA.offset(2);
        let background_at = SCHEMA.offset(3);
        let uniform = Uniform {
            color: ColorSlotGpu::from_floats(&floats[color_at..color_at + 12]),
            floats: [floats[0], floats[1], floats[2], floats[3]],
            floats2: [ctx.time, floats[5], floats[6], level(floats[14])],
            aire: [floats[10], floats[11], floats[12] * TAU, mesh.wakes.len() as f32],
            water: [floats[7], floats[8], floats[9], floats[14]],
            moon: [MOON.x, MOON.y, MOON.z, 0.0],
            eye: [eye.x, eye.y, eye.z, 0.0],
            view_proj,
            wakes,
            mat_ground: self.surfaces.uniform(0),
            mat_water: self.surfaces.uniform(1),
            mat_stones: self.surfaces.uniform(2),
            mat_bushes: self.surfaces.uniform(3),
            screen: [ctx.width as f32, ctx.height as f32, fill, 0.0],
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rio uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }];
        entries.extend(self.surfaces.bind_entries());
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rio bind group"),
            layout: &gpu.bind_group_layout,
            entries: &entries,
        });

        let depth_view = gpu.depth.get(ctx.device, ctx.width, ctx.height);
        let background = ColorSlotGpu::from_floats(&floats[background_at..background_at + 12]);

        // The sky, first and as its own pass — it is what clears the frame,
        // and the pass below loads over it.
        self.backdrop
            .get(format, || Backdrop::new(ctx.device, format))
            .draw(ctx, target, background, 0.0, ctx.fondo_alpha);

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("rio encoder") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rio pass"),
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
            // At `relleno = 0` the valley is exactly the line valley it has
            // always been: nothing is filled, so the frame is byte-identical.
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

    /// A solid red picture laid on each part, at each mapping, is drawn
    /// through the shader: it changes the frame and adds red, and taking it
    /// away again (`set_surface(part, None)`) restores the frame exactly.
    #[test]
    fn a_picture_on_each_part_reaches_the_frame_and_clearing_it_restores_it() {
        use crate::model::{Mapping, SurfaceSpec};
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (160u32, 160u32);
        let floats = SCHEMA.defaults();
        let red: Vec<u8> = [255u8, 0, 0, 255].repeat(16);
        let (texture, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let mut model = Rio::new();
        let ctx = Ctx {
            device: &gpu.device,
            queue: &gpu.queue,
            time: 3.0,
            width: w,
            height: h,
            fondo_alpha: false,
        };
        let draw = |model: &mut Rio| {
            model.render(&ctx, &floats, &view, FORMAT);
            crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &texture, w, h)
        };
        let red_sum = |f: &[u8]| f.chunks(4).map(|p| p[0] as u64).sum::<u64>();
        assert_eq!(model.surfaces(), &["ground", "water", "stones", "bushes"]);

        let plain = draw(&mut model);
        for part in ["ground", "water", "stones", "bushes"] {
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

    /// `relleno` is the valley's filled layer: at 0 it is its lines, byte for
    /// byte; above it the ground and the water are drawn as faces, so a
    /// material lands on the river. Turning it back to 0 restores the lines.
    #[test]
    fn relleno_fills_the_valley_and_a_material_lands_on_the_faces() {
        use crate::model::{MaterialSpec, SurfaceSpec};
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (160u32, 160u32);
        let mut floats = SCHEMA.defaults();
        let fill = SCHEMA.offset(1) - 1;
        assert_eq!(SCHEMA.blocks[0].width(), fill + 1, "relleno is the last of the model's own floats");
        let (texture, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let mut model = Rio::new();
        let ctx = Ctx {
            device: &gpu.device,
            queue: &gpu.queue,
            time: 3.0,
            width: w,
            height: h,
            fondo_alpha: false,
        };
        let draw = |model: &mut Rio, floats: &[f32]| {
            model.render(&ctx, floats, &view, FORMAT);
            crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &texture, w, h)
        };

        let plain = draw(&mut model, &floats);
        floats[fill] = 1.0;
        let solid = draw(&mut model, &floats);
        assert_ne!(plain, solid, "relleno drew no filled faces");

        model.set_surface(
            "water",
            Some(SurfaceSpec::material(MaterialSpec { kind: 5.0, mix: 1.0, ..Default::default() })),
        );
        let glass = draw(&mut model, &floats);
        assert_ne!(solid, glass, "a material on `water` did not change the solid valley");
        model.set_surface("water", None);

        floats[fill] = 0.0;
        let restored = draw(&mut model, &floats);
        assert_eq!(plain, restored, "relleno = 0 is not the line valley");
    }
}
