//! Paisaje — a scrolling ink landscape, after LingDong-/shan-shui-inf (MIT).
//! The generation rules are re-implemented from reading the algorithm; no code
//! was taken.
//!
//! **What it draws.** Paper and a sky wash with a sun disc (`shaders/paisaje.wgsl`),
//! then CPU-built ink: clouds and birds, and 1–6 depth layers (`capas`), far to
//! near. Each layer is a row of nested-contour mountains (the `mountain` rule:
//! ten rows of fifty points, `cos(x)·noise`, each row a shrunk copy), drawn as
//! a paper-coloured body, a wash, texture strokes that follow the contours and
//! a wobbling outline, with the vegetation rules of `vegetate` (rim bushes,
//! clustered pines, foot trees) and `fracTree` (recursive branching trunks) on
//! top, rocks at the feet and boats on the water. Between layers a mist band
//! veils the lower part of the one behind, and below every foot there is water:
//! ripple lines (`water`) and a mirrored, rippling reflection of the silhouette.
//! Last, the paper grain is laid over the whole picture.
//!
//! **Chunks.** The world is a strip of 512-unit chunks (`chunkloader`). A chunk
//! is a pure function of `(semilla, capas, picos, rama, layer, index)`: a
//! mountain planner (`mountplanner`: local maxima of a seeded noise field,
//! jittered, spaced apart) places objects and each is generated at full density
//! with a random threshold. Scrolling is `time × scroll_vel × scroll_dir`
//! (layer `l` moves at its own parallax); only chunks that enter the window are
//! generated and the cache holds the rest, so a frame is a cheap transform of
//! cached geometry. Densities, heights, wind, ripples, roughness, stroke width
//! and spacing are applied at emit time, so beat-rate modulation never
//! regenerates anything; only the four structural floats (`semilla_deriva`,
//! `capas`, `montana_picos`, `arbol_rama`) rebuild the cache. Objects are
//! anchored (a mountain's children are anchored to it) so a chunk seam never
//! cuts anything; the continuous fields (distant ridge, mist, water wash) are
//! sampled from absolute positions, not per chunk.
//!
//! **Bounded.** One frame emits at most [`CAP_VERTS`] vertices, shared out per
//! layer; objects that do not fit are dropped whole. All inputs are clamped to
//! their ranges, and there is no clock but `ctx.time`: the picture is a pure
//! function of the floats and the time.
//!
//! The strokes use this model's own small pipeline (per-vertex colour, `side`
//! and `along` for a feathered, dry-brushable edge); `shared::line2d` has one
//! flat colour per draw.

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

use wgpu::util::DeviceExt;

use crate::model::{color_at, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::kit::{self, CAM2D_BLOCK, LOOK_BLOCK};
use crate::shared::{build_shader, ColorSlotGpu};

// -- schema -------------------------------------------------------------------

const FLOATS: &[FloatSpec] = &[
    spec("scroll_speed", 0.0, 200.0, 28.0),
    spec("scroll_direction", -1.0, 1.0, 1.0),
    // Chunk reroll: a new integer is a new landscape. Structural.
    spec_held("seed_drift", 0.0, 64.0, 3.0, 0.1),
    // Depth layers 1..6. Structural.
    spec_held("layers", 1.0, 6.0, 4.0, 0.2),
    spec("mountain_height", 0.3, 1.8, 1.0),
    // Peaks per mountain (noise frequency across it). Structural.
    spec_held("mountain_peaks", 0.5, 4.0, 1.4, 0.2),
    spec("mountain_noise", 0.0, 1.0, 0.4),
    spec("mountain_distance", 0.4, 2.0, 1.0),
    spec("fog", 0.0, 1.0, 0.55),
    spec("water_level", 0.0, 1.0, 0.35),
    spec("water_waves", 0.0, 1.0, 0.6),
    spec("reflection", 0.0, 1.0, 0.6),
    spec("tree_density", 0.0, 2.0, 1.0),
    spec("tree_height", 0.4, 2.5, 1.0),
    // Recursion depth of the big trees. Structural.
    spec_held("tree_branch", 0.0, 4.0, 3.0, 0.2),
    spec("tree_wind", 0.0, 2.0, 0.5),
    spec("rock_density", 0.0, 2.0, 1.0),
    spec("cloud_density", 0.0, 1.0, 0.5),
    spec("cloud_speed", -2.0, 3.0, 0.4),
    spec("bird_density", 0.0, 1.0, 0.3),
    spec("boat_density", 0.0, 1.0, 0.35),
    spec("sun_x", -1.0, 1.0, 0.45),
    spec("sun_y", 0.05, 0.8, 0.22),
    spec("sun_size", 0.02, 0.45, 0.14),
    spec("stroke_thickness", 0.3, 3.0, 1.0),
    spec("stroke_noise", 0.0, 1.0, 0.45),
    // Wash against outline: 0 is line only, 1 is full wash.
    spec("fill", 0.0, 1.0, 0.6),
    // Paper grain (the slot of the same name is the paper colour).
    spec("paper_grain", 0.0, 1.0, 0.5),
    spec("seal", 0.0, 1.0, 0.5),
];

const F_VEL: usize = 0;
const F_DIR: usize = 1;
const F_SEMILLA: usize = 2;
const F_CAPAS: usize = 3;
const F_ALT: usize = 4;
const F_PICOS: usize = 5;
const F_RUIDO: usize = 6;
const F_DIST: usize = 7;
const F_NIEBLA: usize = 8;
const F_AGUA: usize = 9;
const F_ONDAS: usize = 10;
const F_REFLEJO: usize = 11;
const F_ADENS: usize = 12;
const F_AALT: usize = 13;
const F_RAMA: usize = 14;
const F_VIENTO: usize = 15;
const F_ROCA: usize = 16;
const F_NUBE: usize = 17;
const F_NVEL: usize = 18;
const F_AVE: usize = 19;
const F_BARCA: usize = 20;
const F_SOLX: usize = 21;
const F_SOLY: usize = 22;
const F_SOLT: usize = 23;
const F_GROSOR: usize = 24;
const F_TRUIDO: usize = 25;
const F_RELLENO: usize = 26;
const F_GRANO: usize = 27;
const F_SELLO: usize = 28;

/// Four inks, pale wash to deep black.
const TINTA: [f32; 12] = [
    0.62, 0.64, 0.66, //
    0.40, 0.41, 0.43, //
    0.21, 0.21, 0.23, //
    0.06, 0.06, 0.07,
];
/// Warm rice paper, a little darker towards the bottom.
const PAPEL: [f32; 12] = [
    0.96, 0.94, 0.88, //
    0.94, 0.92, 0.85, //
    0.91, 0.88, 0.80, //
    0.87, 0.83, 0.74,
];
/// The red of the seal and the sun: seal, sun body, sun shade, halo.
const ACENTO: [f32; 12] = [
    0.72, 0.09, 0.08, //
    0.84, 0.17, 0.11, //
    0.90, 0.30, 0.18, //
    0.97, 0.66, 0.42,
];
/// Sky: cool grey-blue high up, warm pale towards the horizon.
const CIELO: [f32; 12] = [
    0.74, 0.80, 0.83, //
    0.84, 0.88, 0.87, //
    0.93, 0.92, 0.86, //
    0.98, 0.94, 0.85,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("ink", &TINTA),
        color_at("paper", &PAPEL),
        color_at("accent", &ACENTO),
        color_at("sky", &CIELO),
        CAM2D_BLOCK,
        LOOK_BLOCK,
        crate::model::SIGNAL_FLOATS_BLOCK,
        crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

const B_OWN: usize = 0;
const B_TINTA: usize = 1;
const B_PAPEL: usize = 2;
const B_ACENTO: usize = 3;
const B_CIELO: usize = 4;
const B_CAM: usize = 5;
const B_LOOK: usize = 6;

fn off(block: usize) -> usize {
    kit::offset_of(SCHEMA.blocks, block)
}

// -- constants ----------------------------------------------------------------

/// Chunk width, in layer units.
const CW: f64 = 512.0;
/// Half the frame height in world units (the frame is 700 units tall).
const VIEW_H: f32 = 350.0;
/// World y that sits at the middle of the frame.
const Y_CEN: f32 = 330.0;
/// The most vertices one frame may emit.
pub const CAP_VERTS: usize = 200_000;
const SEAL_RESERVE: usize = 2_000;
const CLOUD_L: u8 = 7;
/// How far past a chunk its objects can reach, in layer units.
const REACH: f64 = 1500.0;
/// The widest half-window ever generated, whatever the zoom.
const RW_MAX: f64 = 3000.0;
const ROWS: usize = 10;
const COLS: usize = 40;

// -- noise and random ---------------------------------------------------------

fn h4(seed: u32, x: i32, y: i32, z: i32) -> f32 {
    let mut h = seed.wrapping_mul(0x9e37_79b1)
        ^ (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 16;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 15;
    (h & 0x00ff_ffff) as f32 / 16_777_215.0
}

fn value3(seed: u32, x: f32, y: f32, z: f32) -> f32 {
    let (xf, yf, zf) = (x.floor(), y.floor(), z.floor());
    let (fx, fy, fz) = (x - xf, y - yf, z - zf);
    let q = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (sx, sy, sz) = (q(fx), q(fy), q(fz));
    let (ix, iy, iz) = (xf as i32, yf as i32, zf as i32);
    let h = |dx, dy, dz| h4(seed, ix + dx, iy + dy, iz + dz);
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let z0 = l(l(h(0, 0, 0), h(1, 0, 0), sx), l(h(0, 1, 0), h(1, 1, 0), sx), sy);
    let z1 = l(l(h(0, 0, 1), h(1, 0, 1), sx), l(h(0, 1, 1), h(1, 1, 1), sx), sy);
    l(z0, z1, sz)
}

/// Smooth noise in `[0, 1]`: two octaves, so there is more than one scale.
fn noise3(seed: u32, x: f32, y: f32, z: f32) -> f32 {
    0.68 * value3(seed, x, y, z) + 0.32 * value3(seed ^ 0x5bd1, x * 2.03 + 7.1, y * 2.03 + 3.3, z * 2.03 + 1.7)
}

fn noise1(seed: u32, x: f32) -> f32 {
    noise3(seed, x, 0.37, 0.11)
}

fn noise2(seed: u32, x: f32, y: f32) -> f32 {
    noise3(seed, x, y, 0.11)
}

fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

struct Rng(u64);

impl Rng {
    fn new(parts: &[u64]) -> Rng {
        let mut s = 0x1234_5678_9abc_def0u64;
        for p in parts {
            s = splitmix(s ^ p.wrapping_mul(0xd6e8_feb8_6659_fd93));
        }
        Rng(s)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / 16_777_216.0
    }

    fn r(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }

    fn sign(&mut self) -> f32 {
        if self.next() & 1 == 0 {
            -1.0
        } else {
            1.0
        }
    }

    fn gauss(&mut self) -> f32 {
        (self.f() + self.f() + self.f() + self.f() - 2.0) * 1.7
    }
}

/// A float-free hash of integers, for the per-frame fields (birds).
fn hash_u(a: i64, b: u64) -> f32 {
    Rng::new(&[a as u64, b]).f()
}

// -- parameters ---------------------------------------------------------------

fn fin(v: f32, s: &FloatSpec) -> f32 {
    if v.is_finite() {
        v.clamp(s.min, s.max)
    } else {
        s.default
    }
}

/// The row, read once: every float clamped to its range, NaN read as the default.
#[derive(Clone, Copy)]
struct Pr {
    speed: f32,
    dir: f32,
    seed: u64,
    layers: usize,
    alt: f32,
    picos: f32,
    noise: f32,
    distance: f32,
    fog: f32,
    agua: f32,
    waves: f32,
    reflection: f32,
    adens: f32,
    aalt: f32,
    rama: u32,
    wind: f32,
    roca: f32,
    cloud: f32,
    nvel: f32,
    ave: f32,
    barca: f32,
    sun_x: f32,
    sun_y: f32,
    sun_size: f32,
    gro: f32,
    truido: f32,
    rell: f32,
    grain: f32,
    seal: f32,
    cam: kit::Cam2d,
    look: kit::Look,
}

impl Pr {
    fn from_row(floats: &[f32]) -> Pr {
        let o = |i: usize| fin(floats[off(B_OWN) + i], &FLOATS[i]);
        let c: Vec<f32> = (0..kit::Cam2d::N).map(|i| fin(floats[off(B_CAM) + i], &kit::CAM2D[i])).collect();
        let l: Vec<f32> = (0..kit::Look::N).map(|i| fin(floats[off(B_LOOK) + i], &kit::LOOK[i])).collect();
        Pr {
            speed: o(F_VEL),
            dir: o(F_DIR),
            seed: o(F_SEMILLA).round() as u64,
            layers: (o(F_CAPAS).round() as usize).clamp(1, MAX_LAYERS),
            alt: o(F_ALT),
            picos: o(F_PICOS),
            noise: o(F_RUIDO),
            distance: o(F_DIST),
            fog: o(F_NIEBLA),
            agua: o(F_AGUA),
            waves: o(F_ONDAS),
            reflection: o(F_REFLEJO),
            adens: o(F_ADENS),
            aalt: o(F_AALT),
            rama: o(F_RAMA).round() as u32,
            wind: o(F_VIENTO),
            roca: o(F_ROCA),
            cloud: o(F_NUBE),
            nvel: o(F_NVEL),
            ave: o(F_AVE),
            barca: o(F_BARCA),
            sun_x: o(F_SOLX),
            sun_y: o(F_SOLY),
            sun_size: o(F_SOLT),
            gro: o(F_GROSOR),
            truido: o(F_TRUIDO),
            rell: o(F_RELLENO),
            grain: o(F_GRANO),
            seal: o(F_SELLO),
            cam: kit::Cam2d::from_floats(&c),
            look: kit::Look::from_floats(&l),
        }
    }

    fn key(&self) -> GenKey {
        GenKey { seed: self.seed, layers: self.layers as u8, picos: (self.picos * 20.0).round() as i32, rama: self.rama as u8 }
    }
}

const MAX_LAYERS: usize = 6;

/// What a cached chunk depends on. Anything else is applied at emit time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct GenKey {
    seed: u64,
    layers: u8,
    picos: i32,
    rama: u8,
}

impl GenKey {
    fn seed32(&self, layer: u8) -> u32 {
        (splitmix(self.seed ^ ((layer as u64) << 40) ^ 0x51ed) >> 20) as u32
    }
}

#[derive(Clone, Copy)]
struct Lay {
    d: f32,
    sc: f32,
    par: f32,
    tone: f32,
}

fn lay_of(l: usize, n: usize) -> Lay {
    let d = if n <= 1 { 1.0 } else { l as f32 / (n - 1) as f32 };
    Lay { d, sc: 0.45 + 0.55 * d, par: 0.2 + 0.8 * d, tone: 0.28 + 0.5 * d }
}

/// World y of the feet of a layer.
fn base_y(l: &Lay, agua: f32) -> f32 {
    let near = 680.0 - agua * 420.0;
    let hz = near - (near - 190.0) * 0.6;
    hz + (near - hz) * l.d.powf(0.85)
}

// -- geometry building blocks -------------------------------------------------

const K_INK: u8 = 0;
const K_WASH: u8 = 1;
const K_PAPER: u8 = 2;
const K_MIST: u8 = 3;

const M_NONE: u8 = 0;
const M_WIND: u8 = 1;
const M_REFL: u8 = 2;
const M_RIP: u8 = 3;

/// One vertex of cached geometry, in the local frame of its object (y down,
/// foot at the origin). `ox, oy` is the stroke half-width offset, scaled by
/// `trazo_grosor` at emit; `rough` is added to `y` scaled by `montana_ruido`.
#[derive(Clone, Copy)]
struct CVert {
    x: f32,
    y: f32,
    rough: f32,
    ox: f32,
    oy: f32,
    side: f32,
    along: f32,
    tone: f32,
    alpha: f32,
    w: f32,
    kind: u8,
    mode: u8,
}

#[derive(Clone, Copy)]
struct St {
    kind: u8,
    mode: u8,
    tone: f32,
    alpha: f32,
    /// Constant sway weight.
    w: f32,
    /// If > 0 the sway weight is `(−y / wh)^1.5` instead (trees).
    wh: f32,
}

impl St {
    fn ink(tone: f32, alpha: f32) -> St {
        St { kind: K_INK, mode: M_NONE, tone, alpha, w: 0.0, wh: 0.0 }
    }
    fn wash(tone: f32, alpha: f32) -> St {
        St { kind: K_WASH, ..St::ink(tone, alpha) }
    }
    fn paper(alpha: f32) -> St {
        St { kind: K_PAPER, ..St::ink(0.0, alpha) }
    }
    fn mist(alpha: f32) -> St {
        St { kind: K_MIST, ..St::ink(0.0, alpha) }
    }
    fn wind(mut self, wh: f32) -> St {
        self.mode = M_WIND;
        self.wh = wh;
        self
    }
    fn mode(mut self, m: u8, w: f32) -> St {
        self.mode = m;
        self.w = w;
        self
    }
}

#[derive(Default)]
struct Mesh {
    v: Vec<CVert>,
}

type P3 = [f32; 3];

impl Mesh {
    #[allow(clippy::too_many_arguments)]
    fn vtx(&self, p: P3, o: [f32; 2], side: f32, along: f32, st: &St, alpha: f32) -> CVert {
        let w = if st.wh > 0.0 { ((-p[1]) / st.wh).clamp(0.0, 1.0).powf(1.5) } else { st.w };
        CVert { x: p[0], y: p[1], rough: p[2], ox: o[0], oy: o[1], side, along, tone: st.tone, alpha, w, kind: st.kind, mode: st.mode }
    }

    fn quad(&mut self, a: CVert, b: CVert, c: CVert, d: CVert) {
        self.v.extend_from_slice(&[a, b, c, a, c, d]);
    }

    /// A stroke of variable width along `pts`; `hw(t)` is the half-width at
    /// `t ∈ [0, 1]` along it.
    fn ribbon(&mut self, pts: &[P3], hw: impl Fn(f32) -> f32, st: St, ph: f32) {
        let n = pts.len();
        if n < 2 {
            return;
        }
        let mut along = ph;
        let mut prev: Option<(CVert, CVert)> = None;
        for i in 0..n {
            let a = pts[i.saturating_sub(1)];
            let b = pts[(i + 1).min(n - 1)];
            let (tx, ty) = (b[0] - a[0], b[1] - a[1]);
            let len = (tx * tx + ty * ty).sqrt();
            let (nx, ny) = if len < 1e-5 { (0.0, 1.0) } else { (-ty / len, tx / len) };
            if i > 0 {
                let q = pts[i - 1];
                along += ((pts[i][0] - q[0]).powi(2) + (pts[i][1] - q[1]).powi(2)).sqrt();
            }
            let t = i as f32 / (n - 1) as f32;
            let h = hw(t).max(0.0);
            let l = self.vtx(pts[i], [nx * h, ny * h], 1.0, along, &st, st.alpha);
            let r = self.vtx(pts[i], [-nx * h, -ny * h], -1.0, along, &st, st.alpha);
            if let Some((pl, pr)) = prev {
                self.quad(pl, pr, r, l);
            }
            prev = Some((l, r));
        }
    }

    /// Columns between a top outline and a flat bottom, alpha graded top→bottom.
    fn fill(&mut self, top: &[P3], bottom_y: f32, a_top: f32, a_bot: f32, st: St) {
        self.fill_fade(top, bottom_y, a_top, a_bot, st, false);
    }

    /// [`Mesh::fill`], optionally fading to nothing at both ends so a partial
    /// wash has no hard edge.
    fn fill_fade(&mut self, top: &[P3], bottom_y: f32, a_top: f32, a_bot: f32, st: St, fade: bool) {
        let n = top.len().max(2) - 1;
        let f = |i: usize| if fade { (i as f32 / n as f32 * PI).sin().max(0.0).powf(0.7) } else { 1.0 };
        for (i, w) in top.windows(2).enumerate() {
            let (f0, f1) = (f(i), f(i + 1));
            let t0 = self.vtx(w[0], [0.0, 0.0], 0.0, -1.0, &st, a_top * f0);
            let t1 = self.vtx(w[1], [0.0, 0.0], 0.0, -1.0, &st, a_top * f1);
            let b1 = self.vtx([w[1][0], bottom_y, 0.0], [0.0, 0.0], 0.0, -1.0, &st, a_bot * f1);
            let b0 = self.vtx([w[0][0], bottom_y, 0.0], [0.0, 0.0], 0.0, -1.0, &st, a_bot * f0);
            self.quad(t0, t1, b1, b0);
        }
    }

    fn fan(&mut self, c: [f32; 2], ring: &[[f32; 2]], st: St, alpha: f32) {
        let n = ring.len();
        for i in 0..n {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let v0 = self.vtx([c[0], c[1], 0.0], [0.0, 0.0], 0.0, -1.0, &st, alpha);
            let v1 = self.vtx([a[0], a[1], 0.0], [0.0, 0.0], 0.0, -1.0, &st, alpha);
            let v2 = self.vtx([b[0], b[1], 0.0], [0.0, 0.0], 0.0, -1.0, &st, alpha);
            self.v.extend_from_slice(&[v0, v1, v2]);
        }
    }

    /// A leaf / cloud-lens shape: a lens along `ang` through `(cx, cy)`.
    #[allow(clippy::too_many_arguments)]
    fn blob(&mut self, cx: f32, cy: f32, len: f32, wid: f32, ang: f32, st: St, ph: f32) {
        let (s, c) = ang.sin_cos();
        let pts: Vec<P3> = (0..4).map(|i| {
            let t = i as f32 / 3.0 - 0.5;
            [cx + c * len * t, cy + s * len * t, 0.0]
        }).collect();
        self.ribbon(&pts, |t| wid * 0.5 * (t * PI).sin().max(0.0).powf(0.7), st, ph);
    }
}

fn taper(w: f32) -> impl Fn(f32) -> f32 {
    move |t| w * (t * PI).sin().max(0.0).powf(0.6) + 0.02
}

fn flat(pts: &[[f32; 2]]) -> Vec<P3> {
    pts.iter().map(|p| [p[0], p[1], 0.0]).collect()
}

// -- objects ------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kd {
    Mount,
    Refl,
    Tree,
    Big,
    Rock,
    Boat,
    Cloud,
    Water,
}

struct Obj {
    kind: Kd,
    /// Index of the parent in the same chunk, or `usize::MAX`.
    parent: usize,
    /// Anchor: relative to the chunk origin (no parent) or to the parent's frame.
    ax: f32,
    ay: f32,
    /// Included while the density float is above this.
    thr: f32,
    /// Half-width, for culling.
    ext: f32,
    verts: Vec<CVert>,
}

const NONE: usize = usize::MAX;

fn obj(kind: Kd, parent: usize, ax: f32, ay: f32, thr: f32, ext: f32, m: Mesh) -> Obj {
    Obj { kind, parent, ax, ay, thr, ext, verts: m.v }
}

// -- vegetation ---------------------------------------------------------------

/// Leaf clump (`tree02`, as ink dabs): a tight scatter of short wet strokes.
fn bush(m: &mut Mesh, rng: &mut Rng, size: f32, tone: f32, alpha: f32, wh: f32) {
    for _ in 0..5 {
        let x = rng.gauss() * 5.0 * size;
        let y = rng.gauss() * 3.0 * size - 4.0 * size;
        let len = rng.r(5.0, 12.0) * size;
        let wid = rng.r(2.5, 5.5) * size;
        let ang = rng.r(-0.5, 0.5);
        let st = St::ink(tone, alpha * rng.r(0.7, 1.0)).wind(wh);
        m.blob(x, y, len, wid, ang, st, rng.r(0.0, 50.0));
    }
}

/// A slim pine (`tree01`): two wobbling trunk lines and leaf blobs along them.
fn pine(m: &mut Mesh, rng: &mut Rng, hei: f32, wid: f32, tone: f32, alpha: f32, lean: f32) {
    let reso = 10usize;
    let wh = hei.max(1.0);
    let ns: Vec<[f32; 2]> = (0..reso).map(|_| [rng.f(), rng.f()]).collect();
    let mut l1 = Vec::new();
    let mut l2 = Vec::new();
    for i in 0..reso {
        let t = i as f32 / reso as f32;
        let y = -t * hei;
        let bend = lean * t * t * hei * 0.3;
        l1.push([bend + (ns[i][0] - 0.5) * wid - wid / 2.0, y, 0.0]);
        l2.push([bend + (ns[i][1] - 0.5) * wid + wid / 2.0, y, 0.0]);
    }
    let st = St::ink(tone, alpha).wind(wh);
    m.ribbon(&l1, |t| 0.5 + 0.5 * (1.0 - t), st, rng.r(0.0, 40.0));
    m.ribbon(&l2, |t| 0.5 + 0.5 * (1.0 - t), st, rng.r(0.0, 40.0));
    for i in (reso / 4)..reso {
        let k = (reso - i) as f32;
        let cnt = (k / 7.0).ceil() as usize;
        for _ in 0..cnt {
            let t = i as f32 / reso as f32;
            let x = l1[i][0] * 0.5 + l2[i][0] * 0.5 + (rng.f() - 0.5) * wid * 1.2 * k * 0.5;
            let y = -t * hei + (rng.f() - 0.5) * wid;
            let len = rng.f() * 20.0 * k * 0.2 + 10.0;
            let w = rng.f() * 6.0 + 3.0;
            let ang = (rng.f() - 0.5) * PI / 6.0;
            m.blob(x, y, len, w, ang, St::ink(tone, alpha * rng.r(0.5, 1.0)).wind(wh), rng.r(0.0, 40.0));
        }
    }
}

/// One recursive branch of the big tree (`fracTree` + `branch`): a bent
/// trunk of three joints, tapering; forks at the tip and a side shoot, each
/// shorter and thinner, `dep` levels deep.
#[allow(clippy::too_many_arguments)]
fn branch(m: &mut Mesh, rng: &mut Rng, org: [f32; 2], dep: u32, hei: f32, wid: f32, ang: f32, ben: f32, tone: f32, root_h: f32, count: &mut u32) {
    if *count >= 28 || hei < 3.0 {
        return;
    }
    *count += 1;
    // Three joints, each turning by up to `ben`.
    let g = 3usize;
    let mut a0 = 0.0f32;
    let mut tl = vec![[0.0f32, 0.0f32]];
    let (mut nx, mut ny) = (0.0f32, 0.0f32);
    for _ in 0..g {
        a0 += (ben / 2.0 + rng.f() * ben / 2.0) * rng.sign();
        nx += a0.cos() * hei / g as f32;
        ny -= a0.sin() * hei / g as f32;
        tl.push([nx, ny]);
    }
    // Rotate so the tip points along `ang`.
    let ta = tl[g][1].atan2(tl[g][0]);
    for p in tl.iter_mut() {
        let a = p[1].atan2(p[0]);
        let d = (p[0] * p[0] + p[1] * p[1]).sqrt();
        *p = [d * (a - ta + ang).cos(), d * (a - ta + ang).sin()];
    }
    // Interpolate the centre line.
    let span = ((hei / 20.0).ceil() as usize).clamp(2, 4);
    let mut cl: Vec<P3> = Vec::new();
    for i in 0..=(g * span) {
        let (lo, hi) = ((i / span).min(g), ((i + span - 1) / span).min(g));
        let p = (i % span) as f32 / span as f32;
        let (a, b) = (tl[lo], tl[hi]);
        let f = if lo == hi { 0.0 } else { p };
        cl.push([org[0] + a[0] * (1.0 - f) + b[0] * f, org[1] + a[1] * (1.0 - f) + b[1] * f, 0.0]);
    }
    let w0 = wid;
    m.ribbon(&cl, move |t| w0 * (1.0 - 0.5 * t) * 0.9 + 0.3, St::ink(tone, 0.8).wind(root_h), rng.r(0.0, 60.0));
    let tip = cl[cl.len() - 1];
    if dep == 0 {
        // Foliage at the leaves.
        let mut b = Mesh::default();
        bush(&mut b, rng, (hei / 30.0).clamp(0.5, 1.3), tone + 0.05, 0.4, root_h);
        for v in b.v.iter_mut() {
            v.x += tip[0];
            v.y += tip[1];
        }
        m.v.extend(b.v);
        return;
    }
    // Fork at the tip, one to each side, and a side shoot part-way up.
    for s in [-1.0f32, 1.0] {
        let bar = rng.r(0.18, 0.5);
        let hf = rng.r(0.7, 0.9);
        branch(m, rng, [tip[0], tip[1]], dep - 1, hei * hf, wid * 0.6, ang + s * bar, 0.55, tone, root_h, count);
    }
    if rng.f() < 0.7 {
        let k = rng.r(0.3, 0.7);
        let idx = ((cl.len() - 1) as f32 * k) as usize;
        let s = rng.sign();
        let (hf, da) = (rng.r(0.5, 0.75), rng.r(0.5, 1.0));
        branch(m, rng, [cl[idx][0], cl[idx][1]], dep - 1, hei * hf, wid * 0.5, ang + s * da, 0.55, tone, root_h, count);
    }
}

fn big_tree(rng: &mut Rng, hei: f32, wid: f32, depth: u32, tone: f32) -> Mesh {
    let mut m = Mesh::default();
    let mut count = 0;
    branch(&mut m, rng, [0.0, 0.0], depth, hei, wid, -PI / 2.0, 0.0, tone, hei * (1.0 + depth as f32 * 0.7), &mut count);
    m
}

/// A rock (`rock`): a noisy ellipse, flattened below, with shading strokes.
fn rock(m: &mut Mesh, rng: &mut Rng, wid: f32, hei: f32, tone: f32, seed: u32) {
    let n = 18usize;
    let mut ring: Vec<[f32; 2]> = Vec::new();
    for j in 0..n {
        let a = j as f32 / n as f32 * TAU - PI / 2.0;
        let mut l = wid * hei / ((hei * a.cos()).powi(2) + (wid * a.sin()).powi(2)).sqrt().max(1e-3);
        l *= 0.7 + 0.3 * noise1(seed, j as f32 * 0.35);
        let (x, mut y) = (a.cos() * l, -a.sin() * l);
        if a > PI || a < 0.0 {
            y *= 0.25;
        }
        ring.push([x, y - hei * 0.1]);
    }
    m.fan([0.0, -hei * 0.25], &ring, St::paper(1.0), 1.0);
    m.fan([0.0, -hei * 0.25], &ring, St::wash(tone, 0.5), 0.18);
    let mut closed = flat(&ring);
    closed.push(closed[0]);
    m.ribbon(&closed, taper(1.3), St::ink(tone + 0.2, 0.55), rng.r(0.0, 30.0));
    for _ in 0..4 {
        let a = rng.r(0.0, PI) + PI;
        let r0 = rng.r(0.3, 0.9);
        let pts: Vec<P3> = (0..4).map(|i| {
            let aa = a + i as f32 * 0.22;
            [aa.cos() * wid * r0, -aa.sin().abs() * hei * r0 * 0.5 - hei * 0.1, 0.0]
        }).collect();
        m.ribbon(&pts, taper(0.9), St::ink(tone + 0.1, rng.r(0.2, 0.45)), rng.r(0.0, 30.0));
    }
}

/// A fishing boat with a standing man (`boat01`, simplified).
fn boat(m: &mut Mesh, rng: &mut Rng, s: f32, tone: f32) {
    let l = 46.0 * s;
    let hull: Vec<P3> = (0..=8).map(|i| {
        let u = i as f32 / 8.0 * 2.0 - 1.0;
        [u * l, 2.0 * s * (1.0 - u * u) - 7.0 * s * u.powi(4), 0.0]
    }).collect();
    let st = St::ink(tone + 0.15, 0.85).mode(M_RIP, 0.5);
    m.ribbon(&hull, taper(2.6 * s), st, 0.0);
    let deck = [-l * 0.1, 1.0 * s];
    // Man: body, head, straw hat, pole and line.
    m.ribbon(&[[deck[0], deck[1], 0.0], [deck[0] + 1.0 * s, deck[1] - 11.0 * s, 0.0], [deck[0] + 1.5 * s, deck[1] - 20.0 * s, 0.0]], taper(2.2 * s), st, 3.0);
    m.blob(deck[0] + 1.5 * s, deck[1] - 23.0 * s, 5.0 * s, 5.0 * s, 0.0, st, 1.0);
    m.blob(deck[0] + 1.5 * s, deck[1] - 27.0 * s, 15.0 * s, 4.0 * s, 0.0, st, 2.0);
    let dir = if rng.f() < 0.5 { -1.0 } else { 1.0 };
    let pole: Vec<P3> = (0..4).map(|i| {
        let t = i as f32 / 3.0;
        [deck[0] + dir * (6.0 + t * 38.0) * s, deck[1] - (14.0 + 10.0 * t - 24.0 * t * t) * s, 0.0]
    }).collect();
    m.ribbon(&pole, taper(0.7 * s), St::ink(tone + 0.1, 0.6).mode(M_RIP, 0.5), 5.0);
}

// -- mountains ----------------------------------------------------------------

/// The nested contours of one mountain (`Mount.mountain`): row `j` is the
/// silhouette scaled by `1 − j/ROWS` with its own noise slice.
fn mountain_rows(key: &GenKey, s32: u32, rng: &mut Rng, h: f32, w: f32, sc: f32) -> Vec<[P3; COLS]> {
    let seedf = rng.f() * 40.0;
    let picos = key.picos as f32 / 20.0;
    let mut rows = vec![[[0.0f32; 3]; COLS]; ROWS];
    let mut hoff = 0.0;
    for (j, row) in rows.iter_mut().enumerate() {
        hoff += rng.f() * 3.2 * sc;
        let p = 1.0 - j as f32 / ROWS as f32;
        for (i, pt) in row.iter_mut().enumerate() {
            let x = (i as f32 / (COLS - 1) as f32 - 0.5) * PI;
            let cx = x.cos().max(0.0);
            let n = noise3(s32, x * picos + 10.0, j as f32 * 0.15, seedf);
            let a = 0.18 + 1.2 * n.powf(1.25);
            let rough = (noise3(s32 ^ 0x77, x * picos * 3.0 + 50.0, j as f32 * 0.4, seedf) - 0.5) * 0.3 * h * p * cx.sqrt();
            *pt = [x / PI * w * p, -a * cx * h * p + hoff, rough];
        }
    }
    rows
}

fn gen_mountain(out: &mut Vec<Obj>, rng: &mut Rng, key: &GenKey, lay: &Lay, s32: u32, ax: f32, ay: f32, last_in_chunk: bool) {
    let _ = last_in_chunk;
    let sc = lay.sc;
    let h = (70.0 + rng.f() * 330.0) * sc;
    let w = (320.0 + rng.f() * 280.0) * sc;
    let rows = mountain_rows(key, s32, rng, h, w, sc);
    let ext = w * 0.58;
    let top = &rows[0];
    let tone = lay.tone;

    // Reflection stripes: horizontal dashes where the mirrored silhouette is.
    let mut rm = Mesh::default();
    let maxh = top.iter().map(|p| -p[1]).fold(0.0f32, f32::max);
    let stripes = 18usize;
    for s in 0..stripes {
        let f = (s as f32 + 0.5) / stripes as f32;
        let y = f * maxh * 0.85 + 3.0;
        let mut run: Vec<P3> = Vec::new();
        let flush = |run: &mut Vec<P3>, rm: &mut Mesh, rng: &mut Rng| {
            if run.len() >= 2 {
                let st = St::ink(tone + 0.1, 0.75 * (1.0 - f * 0.7)).mode(M_REFL, 0.4 + f);
                rm.ribbon(run, taper(1.0 + 1.4 * (1.0 - f)), st, rng.r(0.0, 50.0));
            }
            run.clear();
        };
        for (ci, p) in top.iter().enumerate() {
            if ci % 3 != 0 && ci != COLS - 1 {
                continue;
            }
            if -p[1] * 0.85 > y {
                run.push([p[0], y, 0.0]);
            } else {
                flush(&mut run, &mut rm, rng);
            }
        }
        flush(&mut run, &mut rm, rng);
    }
    out.push(obj(Kd::Refl, NONE, ax, ay, 0.0, ext, rm));

    // The mountain itself.
    let mut m = Mesh::default();
    let ridge: Vec<P3> = top.to_vec();
    m.fill(&ridge, 4.0 * sc, 1.0, 1.0, St::paper(1.0));
    m.fill(&ridge, 4.0 * sc, 0.34, 0.05, St::wash(tone, 1.0));
    // A shaded flank: the right-hand half, from the third contour.
    let flank: Vec<P3> = rows[3][COLS / 2..].to_vec();
    m.fill_fade(&flank, 4.0 * sc, 0.26, 0.03, St::wash(tone + 0.1, 1.0), true);
    // Texture strokes along the contours (`texture`).
    let tex = (30.0 + 80.0 * lay.d) as usize;
    for s in 0..tex {
        let dis = if rng.f() > 0.5 { rng.f() / 3.0 } else { 2.0 / 3.0 + rng.f() / 3.0 };
        let mid = (dis * COLS as f32) as i32;
        let hlen = (rng.f() * COLS as f32 * 0.2) as i32;
        let (a, b) = ((mid - hlen).clamp(0, COLS as i32) as usize, (mid + hlen).clamp(0, COLS as i32) as usize);
        let layer = s as f32 / tex as f32 * (ROWS - 1) as f32;
        let (lo, hi) = (layer.floor() as usize, layer.ceil() as usize);
        let p = layer - layer.floor();
        let mut pts: Vec<P3> = Vec::new();
        let mut j = a;
        while j < b {
            let (u, v) = (rows[lo][j], rows[hi][j]);
            let (x, y, r) = (u[0] * p + v[0] * (1.0 - p), u[1] * p + v[1] * (1.0 - p), u[2] * p + v[2] * (1.0 - p));
            let noi = 30.0 * sc / (layer + 1.0);
            let nx = noi * (noise2(s32 ^ 0x31, x * 0.2, j as f32 * 0.5) - 0.5);
            let ny = noi * (noise2(s32 ^ 0x32, y * 0.2, j as f32 * 0.5) - 0.5);
            pts.push([x + nx, y + ny, r]);
            j += 2;
        }
        if pts.len() >= 3 {
            let st = St::ink(tone + 0.18, rng.r(0.12, 0.55));
            m.ribbon(&pts, taper(0.9 + 0.9 * sc), st, rng.r(0.0, 80.0));
        }
    }
    // Outline: a wide faint wobbling line and a thin firm one.
    m.ribbon(&ridge, taper(2.6 * sc + 0.6), St::ink(tone + 0.2, 0.3), rng.r(0.0, 80.0));
    m.ribbon(&ridge, taper(1.2 * sc + 0.4), St::ink(tone + 0.28, 0.8), rng.r(0.0, 80.0));
    out.push(obj(Kd::Mount, NONE, ax, ay, 0.0, ext, m));
    let me = out.len() - 1;

    // Vegetation (`vegetate`): rules over the contour grid.
    let tn = tone + 0.12;
    let rule = |a: f32, b: f32, c: f32| noise3(s32 ^ 0x1d, a, b, c);
    let mut cand = Vec::new();
    for (j, row) in rows.iter().enumerate() {
        for i in (2..COLS - 2).step_by(2) {
            let ns = rule(i as f32 * 0.1, j as f32 * 0.1, 2.0);
            if ns * ns * ns < 0.1 && -row[i][1] / h > 0.5 {
                cand.push((j, i));
            }
        }
    }
    let n_top = cand.len().min(6);
    for _ in 0..n_top {
        let k = (rng.f() * cand.len() as f32) as usize % cand.len();
        let (j, i) = cand.swap_remove(k);
        let mut b = Mesh::default();
        bush(&mut b, rng, (0.5 + 0.9 * sc) * 0.9, tn, 0.38, 14.0 * sc);
        let p = rows[j][i];
        let thr = rng.f();
        out.push(obj(Kd::Tree, me, p[0], p[1], thr, 30.0 * sc, b));
    }
    let mut pcand: Vec<(f32, f32, f32)> = Vec::new();
    for (j, row) in rows.iter().enumerate().skip(1).step_by(2) {
        for i in 0..COLS {
            let ns = rule(i as f32 * 0.2, j as f32 * 0.05, 9.0);
            if ns * ns * ns * ns < 0.012 && -row[i][1] / h < 0.3 {
                pcand.push((row[i][0], row[i][1], (h + row[i][1]) / h));
            }
        }
    }
    let mut kept = 0;
    for (idx, &(x, y, hf)) in pcand.iter().enumerate() {
        if kept >= 5 {
            break;
        }
        let near = pcand.iter().enumerate().filter(|(k, q)| *k != idx && (q.0 - x).powi(2) + (q.1 - y).powi(2) < (45.0 * sc).powi(2)).count();
        if near < 2 {
            continue;
        }
        kept += 1;
        let ht = hf.clamp(0.1, 1.0) * 70.0 * (0.3 + rng.f() * 0.7) * (0.6 + 0.8 * sc);
        let mut b = Mesh::default();
        let pw = rng.r(1.0, 4.0) * sc.max(0.6);
        pine(&mut b, rng, ht.max(8.0), pw, tn, 0.4, 0.0);
        let thr = rng.f();
        out.push(obj(Kd::Tree, me, x, y, thr, 20.0 * sc, b));
    }
    // Foot trees and rocks at the two ends of each contour.
    let mut feet = 0;
    for (j, row) in rows.iter().enumerate() {
        for &i in &[0usize, COLS - 1] {
            let ns = rule(i as f32 * 0.2, j as f32 * 0.05, 5.0);
            let p = row[i];
            if ns * ns * ns * ns < 0.012 && feet < 3 {
                feet += 1;
                let mut b = Mesh::default();
                let ht = ((h + p[1]) / h).clamp(0.2, 1.0) * 120.0 * (0.5 + rng.f() * 0.5) * (0.6 + 0.8 * sc);
                let (pw, lean) = (rng.r(1.0, 5.0) * sc.max(0.6), rng.r(-0.6, 0.6));
                pine(&mut b, rng, ht.max(10.0), pw, tn, 0.4, lean);
                let thr = rng.f();
                out.push(obj(Kd::Tree, me, p[0], p[1], thr, 25.0 * sc, b));
            }
            if rng.f() < 0.05 {
                let mut b = Mesh::default();
                let (rw_, rh_) = (rng.r(20.0, 40.0) * (0.5 + 0.7 * sc), rng.r(20.0, 40.0) * (0.5 + 0.7 * sc));
                rock(&mut b, rng, rw_, rh_, tone + 0.1, s32);
                let thr = rng.f();
                out.push(obj(Kd::Rock, me, p[0], p[1] + 2.0, thr, 30.0, b));
            }
        }
    }
    // Big recursive trees on the banks, nearer layers only.
    if lay.d > 0.5 {
        for &i in &[0usize, COLS - 1] {
            if rng.f() < 0.5 {
                let p = rows[0][i];
                let hei = rng.r(45.0, 85.0) * (0.6 + 0.5 * sc);
                let tw = rng.r(2.0, 3.4) * sc;
                let m = big_tree(rng, hei, tw, key.rama as u32, tone + 0.25);
                let (jx, thr) = (rng.r(-14.0, 14.0) * sc, rng.f());
                out.push(obj(Kd::Big, me, p[0] + jx, p[1] + 2.0, thr, hei * 0.9, m));
            }
        }
    }
}

fn gen_layer_chunk(key: &GenKey, l: u8, k: i64) -> Vec<Obj> {
    let lay = lay_of(l as usize, key.layers as usize);
    let s32 = key.seed32(l);
    let mut rng = Rng::new(&[key.seed, l as u64, k as u64, 11]);
    let mut out: Vec<Obj> = Vec::new();

    // `mountplanner`: local maxima of a seeded noise field, jittered, spaced.
    let samp = 0.0085;
    let kk = (k.rem_euclid(1 << 16)) as f32 * 512.0;
    let ns = |x: f32| noise1(s32, (kk + x) * samp);
    let mut places: Vec<(f32, f32)> = Vec::new();
    let step = 8.0;
    for i in 0..64 {
        let x = i as f32 * step;
        let z0 = ns(x);
        if z0 < 0.5 {
            continue;
        }
        let mut is_max = true;
        for d in -4i32..=4 {
            if d != 0 && ns(x + d as f32 * step) > z0 {
                is_max = false;
                break;
            }
        }
        if !is_max {
            continue;
        }
        let xof = x + (rng.f() - 0.5) * 2.0 * 200.0;
        let yof = rng.r(-6.0, 22.0) * lay.sc;
        if places.iter().all(|p| (p.0 - xof).abs() > 190.0 * lay.sc) {
            places.push((xof, yof));
        }
    }
    places.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    // Water ripple lines under the feet (`water`).
    let water_n = 18;
    for _ in 0..water_n {
        let ax = rng.r(0.0, 512.0);
        let ay = 8.0 + rng.f().powf(1.6) * (60.0 + 250.0 * lay.d);
        let len = rng.r(40.0, 130.0) * (0.5 + 0.5 * lay.sc);
        let thr = rng.f();
        let seed_w = rng.f() * 30.0;
        let n = (len / 10.0).max(3.0) as usize;
        let pts: Vec<P3> = (0..n).map(|i| {
            let j = i as f32 * 10.0 - len * 0.5;
            [j, (j * 0.2).sin() * 1.4 * noise1(s32 ^ 9, j * 0.1 + seed_w) , 0.0]
        }).collect();
        let mut m = Mesh::default();
        let (wa, wp) = (rng.r(0.3, 0.6), rng.r(0.0, 40.0));
        m.ribbon(&pts, taper(0.9), St::ink(0.5, wa).mode(M_RIP, 1.0), wp);
        out.push(obj(Kd::Water, NONE, ax, ay, thr, len * 0.5, m));
    }

    for &(ax, ay) in &places {
        gen_mountain(&mut out, &mut rng, key, &lay, s32, ax, ay, false);
    }

    // A boat now and then, on the water in front of the feet.
    if lay.d > 0.3 && rng.f() < 0.55 {
        let mut m = Mesh::default();
        let bs = lay.sc * rng.r(0.8, 1.2);
        boat(&mut m, &mut rng, bs, lay.tone);
        let (ax, ay) = (rng.r(0.0, 512.0), rng.r(25.0, 70.0 + 80.0 * lay.d));
        let thr = rng.f();
        out.push(obj(Kd::Boat, NONE, ax, ay, thr, 60.0, m));
    }
    // Loose rocks.
    for _ in 0..1 {
        let mut m = Mesh::default();
        let (ax, ay) = (rng.r(0.0, 512.0), rng.r(4.0, 30.0) * lay.sc);
        let s = rng.r(0.5, 1.1) * (0.5 + 0.7 * lay.sc);
        rock(&mut m, &mut rng, 34.0 * s, 24.0 * s, lay.tone + 0.12, s32);
        let thr = rng.f();
        out.push(obj(Kd::Rock, NONE, ax, ay, thr, 40.0, m));
    }
    out
}

/// Clouds: stacked flat lenses of paper with a thin ink line along the top.
fn gen_cloud_chunk(key: &GenKey, k: i64) -> Vec<Obj> {
    let mut rng = Rng::new(&[key.seed, CLOUD_L as u64, k as u64, 23]);
    let mut out = Vec::new();
    let n = 4;
    for c in 0..n {
        let mut m = Mesh::default();
        let np = 4 + (rng.f() * 3.0) as usize;
        let step = rng.r(46.0, 76.0);
        let thick = rng.r(0.7, 1.3);
        let mut top: Vec<P3> = Vec::new();
        for q in 0..np {
            let t = q as f32 / (np - 1) as f32;
            let cx = (q as f32 - (np - 1) as f32 / 2.0) * step;
            let cy = -((t * PI).sin()) * 14.0 * thick + rng.r(-3.0, 3.0);
            let len = rng.r(110.0, 200.0);
            let wid = rng.r(18.0, 32.0) * thick;
            m.blob(cx, cy, len, wid, 0.0, St::mist(0.82), rng.r(0.0, 50.0));
            top.push([cx - len * 0.3, cy - wid * 0.4, 0.0]);
            top.push([cx + len * 0.3, cy - wid * 0.4, 0.0]);
        }
        top.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
        m.ribbon(&top, taper(0.9), St::ink(0.2, 0.2), rng.r(0.0, 50.0));
        for _ in 0..2 {
            let y = rng.r(0.0, 12.0);
            let x0 = rng.r(-step, step);
            let pts: Vec<P3> = (0..4).map(|i| [x0 + i as f32 * 28.0, y + (i as f32 * 0.9).sin() * 2.0, 0.0]).collect();
            m.ribbon(&pts, taper(0.8), St::wash(0.2, 0.55), rng.r(0.0, 50.0));
        }
        let ax = rng.r(0.0, 512.0);
        let ay = rng.r(30.0, 250.0);
        let thr = (c as f32 + rng.f() * 0.9) / n as f32;
        out.push(obj(Kd::Cloud, NONE, ax, ay, thr, step * np as f32 * 0.6 + 120.0, m));
    }
    out
}

// -- cache --------------------------------------------------------------------

#[derive(Default)]
struct Cache {
    key: Option<GenKey>,
    chunks: HashMap<(u8, i64), Vec<Obj>>,
    /// Chunks generated since the model was built.
    made: usize,
}

impl Cache {
    fn ensure(&mut self, key: &GenKey, l: u8, k: i64) {
        if self.key != Some(*key) {
            self.chunks.clear();
            self.key = Some(*key);
        }
        if !self.chunks.contains_key(&(l, k)) {
            self.made += 1;
            let c = if l == CLOUD_L { gen_cloud_chunk(key, k) } else { gen_layer_chunk(key, l, k) };
            self.chunks.insert((l, k), c);
        }
    }
}

// -- camera and colour --------------------------------------------------------

#[derive(Clone, Copy)]
struct View {
    zoom: f32,
    pan: [f32; 2],
    cs: f32,
    sn: f32,
}

impl View {
    fn new(c: &kit::Cam2d, time: f32) -> View {
        let a = c.turn * PI + c.drift * 0.2 * time;
        let (sn, cs) = a.sin_cos();
        View { zoom: c.zoom.clamp(0.1, 8.0), pan: [c.pan_x, c.pan_y], cs, sn }
    }

    fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        let (x, y) = (p[0] - self.pan[0], p[1] - self.pan[1]);
        [(self.cs * x + self.sn * y) * self.zoom, (self.cs * y - self.sn * x) * self.zoom]
    }
}

fn grad4(c: &[f32], t: f32) -> [f32; 3] {
    ColorSlotGpu::from_floats(c).grad4(t)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vert {
    pos: [f32; 2],
    side: f32,
    along: f32,
    color: [f32; 4],
}

struct Emit<'a> {
    pr: &'a Pr,
    view: View,
    t: f32,
    ink: [[f32; 3]; 33],
    paper: &'a [f32],
    fondo_alpha: bool,
    out: &'a mut Vec<Vert>,
}

impl Emit<'_> {
    fn ink_at(&self, tone: f32) -> [f32; 3] {
        self.ink[(tone.clamp(0.0, 1.0) * 32.0).round() as usize]
    }

    /// Transform and colour one object's vertices into the frame.
    #[allow(clippy::too_many_arguments)]
    fn push(&mut self, verts: &[CVert], ux: f32, by: f32, sx: f32, sy: f32, soff: f32, phase: f32, amul: f32, haze: f32) {
        let p = self.pr;
        let fa = self.fondo_alpha;
        let level = p.look.background_level;
        for v in verts {
            let lx = v.x * sx;
            let ly = (v.y + v.rough * p.noise) * sy;
            let (mut dx, mut dy) = (0.0, 0.0);
            match v.mode {
                M_WIND => dx = p.wind * v.w * 7.0 * (self.t * 1.3 + phase + ly * 0.01).sin() * (1.0 + 0.4 * (self.t * 0.37 + phase).sin()),
                M_REFL => dx = p.waves * v.w * 4.0 * (v.y * 0.6 - self.t * 1.8 + phase).sin(),
                M_RIP => dy = p.waves * v.w * 1.6 * (v.x * 0.05 - self.t * 1.6 + phase).sin(),
                _ => {}
            }
            let wx = ux + lx + dx + v.ox * p.gro * soff;
            let wy = by + ly + dy + v.oy * p.gro * soff;
            let pos = self.view.apply([wx / VIEW_H, -(wy - Y_CEN) / VIEW_H]);
            let (rgb, a) = match v.kind {
                K_INK => (self.ink_at(v.tone), v.alpha * amul * (1.0 - haze)),
                K_WASH => (self.ink_at(v.tone), v.alpha * amul * (1.0 - haze) * p.rell),
                _ => {
                    let g = grad4(self.paper, 0.5 - pos[1] * 0.5);
                    let (m, am) = if fa { (1.0, level) } else { (level, 1.0) };
                    ([g[0] * m, g[1] * m, g[2] * m], v.alpha * amul * am)
                }
            };
            self.out.push(Vert { pos, side: v.side, along: v.along, color: [rgb[0], rgb[1], rgb[2], a.clamp(0.0, 1.0)] });
        }
    }
}

/// Everything per-frame that is not chunk geometry: the distant ridge, the
/// water wash and the mist veil. Sampled from absolute layer positions.
fn field_mesh(kind: u8, l: &Lay, cu: f64, rw: f64, time: f32, key: &GenKey, layer: u8, fog: f32) -> Mesh {
    let mut m = Mesh::default();
    let u0 = ((cu - rw) / 24.0).floor() as i64 - 1;
    let u1 = ((cu + rw) / 24.0).ceil() as i64 + 1;
    let s32 = key.seed32(layer) ^ 0xa5a5;
    let at = |i: i64| ((i as f64 * 24.0) - cu) as f32;
    let nz = |i: i64| ((i % 100_000) as f32) * 24.0;
    match kind {
        // Distant ridge: a long low range, continuous across chunks.
        0 => {
            let top: Vec<P3> = (u0..=u1).map(|i| {
                let u = nz(i);
                let hgt = (50.0 + 90.0 * noise1(s32, u * 0.004)) * l.sc * (0.35 + 0.65 * noise1(s32 ^ 5, u * 0.0015 + 3.0));
                [at(i), -hgt, (noise1(s32 ^ 7, u * 0.05) - 0.5) * 6.0 * l.sc]
            }).collect();
            m.fill(&top, 6.0, 1.0, 1.0, St::paper(1.0));
            m.fill(&top, 6.0, 0.16, 0.02, St::wash(l.tone, 1.0));
            m.ribbon(&top, taper(1.4 * l.sc + 0.4), St::ink(l.tone + 0.2, 0.3), 3.0);
        }
        // Water wash under the feet: pale ink fading with depth.
        1 => {
            let d = 70.0 + 260.0 * l.d;
            let top: Vec<P3> = (u0..=u1).step_by(4).map(|i| [at(i), 4.0, 0.0]).collect();
            m.fill(&top, d, 0.12, 0.0, St::wash(0.4, 1.0));
        }
        // Mist: a band that pales the foot of the layer, drifting slowly.
        _ => {
            let hh = 150.0 * l.sc;
            let cols: Vec<i64> = (u0..=u1).step_by(2).collect();
            let rows: Vec<(f32, f32)> = vec![(-1.0, 0.0), (-0.3, 1.0), (0.0, 0.7), (0.2, 0.0)];
            let a = fog * 0.8 * (0.35 + 0.65 * (1.0 - l.d));
            for w in cols.windows(2) {
                let mk = |i: i64, r: usize| {
                    let u = nz(i);
                    let wob = (noise1(s32 ^ 0x11, u * 0.006 + time * 0.03) - 0.5) * 0.5;
                    let (y, al) = rows[r];
                    let yy = if r == 0 { y * hh * (1.0 + wob) } else { y * hh };
                    ([at(i), yy, 0.0], al * a)
                };
                let st = St::mist(1.0);
                for r in 0..rows.len() - 1 {
                    let (p0, a0) = mk(w[0], r);
                    let (p1, a1) = mk(w[1], r);
                    let (p2, a2) = mk(w[1], r + 1);
                    let (p3, a3) = mk(w[0], r + 1);
                    let v = |p: P3, al: f32, m: &Mesh| m.vtx(p, [0.0, 0.0], 0.0, -1.0, &st, al);
                    let (v0, v1, v2, v3) = (v(p0, a0, &m), v(p1, a1, &m), v(p2, a2, &m), v(p3, a3, &m));
                    m.quad(v0, v1, v2, v3);
                }
            }
        }
    }
    m
}

/// Birds: little flapping vees in loose flocks, a pure function of the clock.
fn birds_mesh(cu: f64, rw: f64, time: f32, seed: u64, dens: f32) -> Mesh {
    let mut m = Mesh::default();
    if dens <= 0.001 {
        return m;
    }
    const FW: f64 = 700.0;
    let f0 = ((cu - rw - 400.0) / FW).floor() as i64;
    let f1 = ((cu + rw + 400.0) / FW).ceil() as i64;
    for f in f0..=f1 {
        let h = |i: u64| hash_u(f, seed ^ (i.wrapping_mul(0x9e37)));
        if h(1) >= dens {
            continue;
        }
        let ox = (f as f64 * FW + h(2) as f64 * FW * 0.8 - cu) as f32;
        let oy = 30.0 + h(3) * 170.0;
        let n = 3 + (h(4) * 4.0) as usize;
        for i in 0..n {
            let off = i as f32 - (n as f32 - 1.0) / 2.0;
            let sz = 8.0 + 5.0 * hash_u(f * 31 + i as i64, seed ^ 77);
            let (bx, by) = (ox - off.abs() * 24.0, oy + off.abs() * 9.0 + off * 4.0);
            let flap = (time * 6.5 + i as f32 * 0.8 + f as f32 * 1.3).sin();
            let tip_y = -sz * 0.55 * flap - sz * 0.15;
            for s in [-1.0f32, 1.0] {
                let pts: Vec<P3> = vec![
                    [bx, by, 0.0],
                    [bx + s * sz * 0.45, by - sz * 0.25 * (0.4 + 0.6 * flap) - 1.0, 0.0],
                    [bx + s * sz * 0.9, by + tip_y * 0.7, 0.0],
                    [bx + s * sz * 1.25, by + tip_y + 1.5, 0.0],
                ];
                m.ribbon(&pts, taper(0.9), St::ink(0.78, 0.75), 0.0);
            }
        }
    }
    m
}

fn seal_verts(pr: &Pr, accent: &[f32], paper: &[f32], aspect: f32, out: &mut Vec<Vert>) {
    if pr.seal <= 0.01 {
        return;
    }
    let s = 0.03 + 0.07 * pr.seal;
    let (cx, cy) = (aspect - 0.07 - s, -0.9 + s);
    let red = grad4(accent, 0.0);
    let ink = |x: f32, y: f32, c: [f32; 3], a: f32| Vert { pos: [x, y], side: 0.0, along: -1.0, color: [c[0], c[1], c[2], a] };
    let quad = |x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 3], a: f32, out: &mut Vec<Vert>| {
        let (a0, a1, a2, a3) = (ink(x0, y0, c, a), ink(x1, y0, c, a), ink(x1, y1, c, a), ink(x0, y1, c, a));
        out.extend_from_slice(&[a0, a1, a2, a0, a2, a3]);
    };
    quad(cx - s, cy - s, cx + s, cy + s, red, 0.92, out);
    let pap = grad4(paper, 0.5);
    let t = s * 0.12;
    quad(cx - s * 0.6, cy - t, cx + s * 0.6, cy + t, pap, 0.9, out);
    quad(cx - t, cy - s * 0.6, cx + t, cy + s * 0.6, pap, 0.9, out);
    quad(cx - s * 0.6, cy + s * 0.5, cx - s * 0.1, cy + s * 0.5 + 2.0 * t, pap, 0.9, out);
}

// -- the frame ----------------------------------------------------------------

pub struct FrameInfo {
    pub verts: usize,
    pub made: usize,
}

struct Slots<'a> {
    ink: &'a [f32],
    paper: &'a [f32],
    accent: &'a [f32],
}

/// Window of chunk indices a layer needs for the current camera.
fn chunk_range(cu: f64, rw: f64) -> (i64, i64) {
    (((cu - rw - REACH) / CW).floor() as i64, ((cu + rw + REACH) / CW).floor() as i64)
}

fn build_frame(cache: &mut Cache, pr: &Pr, time: f32, aspect: f32, fondo_alpha: bool, slots: &Slots, out: &mut Vec<Vert>) -> FrameInfo {
    out.clear();
    let key = pr.key();
    let n = pr.layers;
    let view = View::new(&pr.cam, time);
    let scroll = pr.speed as f64 * pr.dir as f64 * time as f64;
    let diag = aspect.hypot(1.0);
    let rw = (((VIEW_H * diag / view.zoom) as f64) + pr.cam.pan_x.abs() as f64 * VIEW_H as f64).min(RW_MAX);
    let made0 = cache.made;

    let cloud_cu = scroll * 0.1 - pr.nvel as f64 * 60.0 * time as f64;
    let mut need: Vec<(u8, i64, i64, f64)> = Vec::new();
    for l in 0..n {
        let lay = lay_of(l, n);
        let cu = scroll * lay.par as f64;
        let (a, b) = chunk_range(cu, rw);
        need.push((l as u8, a, b, cu));
    }
    let (ca, cb) = chunk_range(cloud_cu, rw);
    need.push((CLOUD_L, ca, cb, cloud_cu));
    for &(l, a, b, _) in &need {
        for k in a..=b {
            cache.ensure(&key, l, k);
        }
    }

    let mut ink = [[0.0f32; 3]; 33];
    for (i, c) in ink.iter_mut().enumerate() {
        *c = grad4(slots.ink, i as f32 / 32.0);
    }
    let mut e = Emit { pr, view, t: time, ink, paper: slots.paper, fondo_alpha, out };
    let per = (CAP_VERTS - SEAL_RESERVE) / (n + 1);

    let tmp = |e: &mut Emit, m: &Mesh, by: f32, haze: f32| {
        e.push(&m.v, 0.0, by, 1.0, 1.0, 1.0, 0.0, 1.0, haze);
    };

    // Sky: clouds, then birds.
    {
        let start = e.out.len();
        let &(_, a, b, cu) = need.last().expect("cloud window");
        let dens = pr.cloud;
        for k in a..=b {
            for o in cache.chunks.get(&(CLOUD_L, k)).map(|v| v.as_slice()).unwrap_or(&[]) {
                if o.thr >= dens {
                    continue;
                }
                let ux = ((k as f64 * CW - cu) as f32) + o.ax * pr.distance;
                if (ux as f64).abs() > rw + o.ext as f64 {
                    continue;
                }
                if e.out.len() - start + o.verts.len() > per {
                    continue;
                }
                e.push(&o.verts, ux, o.ay, 1.0, 1.0, 1.0, 0.0, 1.0, 0.0);
            }
        }
        let bm = birds_mesh(cu - 30.0 * time as f64, rw, time, pr.seed, pr.ave);
        if e.out.len() - start + bm.v.len() <= per {
            // Birds fly in their own frame; shift back to the cloud camera.
            e.push(&bm.v, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, 0.0);
        }
    }

    for l in 0..n {
        let lay = lay_of(l, n);
        let &(_, a, b, cu) = &need[l];
        let by = base_y(&lay, pr.agua);
        let haze = (pr.fog * (1.0 - lay.d) * 0.4).clamp(0.0, 0.9);
        let start = e.out.len();
        if l < 2 {
            let m = field_mesh(0, &lay, cu, rw, time, &key, l as u8, pr.fog);
            tmp(&mut e, &m, by, haze);
        }
        {
            let m = field_mesh(1, &lay, cu, rw, time, &key, l as u8, pr.fog);
            tmp(&mut e, &m, by, haze);
        }
        for k in a..=b {
            let objs = match cache.chunks.get(&(l as u8, k)) {
                Some(o) => o,
                None => continue,
            };
            let origin = (k as f64 * CW - cu) as f32;
            let mut pos: Vec<Option<(f32, f32)>> = vec![None; objs.len()];
            for (i, o) in objs.iter().enumerate() {
                let (ux, oy) = if o.parent == NONE {
                    (origin + o.ax * pr.distance, by + o.ay)
                } else {
                    match pos[o.parent] {
                        Some((px, py)) => (px + o.ax, py + o.ay * pr.alt),
                        None => continue,
                    }
                };
                let shown = match o.kind {
                    Kd::Mount => true,
                    Kd::Refl => pr.reflection > 0.004,
                    Kd::Tree | Kd::Big => o.thr < pr.adens * 0.5,
                    Kd::Rock => o.thr < pr.roca * 0.5,
                    Kd::Boat => o.thr < pr.barca,
                    Kd::Water => o.thr < pr.waves,
                    Kd::Cloud => false,
                };
                // The parent's slot is filled even when its parts are not drawn.
                pos[i] = Some((ux, oy));
                if !shown || (ux as f64).abs() > rw + o.ext as f64 + 40.0 {
                    continue;
                }
                if e.out.len() - start + o.verts.len() > per {
                    continue;
                }
                let phase = o.ax * 0.011 + k as f32 * 3.1;
                let (sx, sy, soff, amul) = match o.kind {
                    Kd::Mount => (1.0, pr.alt, 1.0, 1.0),
                    Kd::Refl => (1.0, pr.alt, 1.0, pr.reflection),
                    Kd::Tree | Kd::Big => (pr.aalt, pr.aalt, pr.aalt, 1.0),
                    _ => (1.0, 1.0, 1.0, 1.0),
                };
                e.push(&o.verts, ux, oy, sx, sy, soff, phase, amul, haze);
            }
        }
        if l + 1 < n {
            let m = field_mesh(2, &lay, cu, rw, time, &key, l as u8, pr.fog);
            tmp(&mut e, &m, by, 0.0);
        }
    }
    seal_verts(pr, slots.accent, slots.paper, aspect, e.out);

    // Forget what the window has left behind (keep one chunk of slack).
    cache.chunks.retain(|&(l, k), _| need.iter().any(|&(nl, a, b, _)| nl == l && k >= a - 1 && k <= b + 1));
    FrameInfo { verts: out.len(), made: cache.made - made0 }
}

// -- GPU ----------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BaseU {
    sky: ColorSlotGpu,
    paper: ColorSlotGpu,
    accent: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    e: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct InkU {
    look: [f32; 4],
    misc: [f32; 4],
    grain: [f32; 4],
    gdark: [f32; 4],
    glight: [f32; 4],
}

struct InkPipe {
    ink: wgpu::RenderPipeline,
    grain: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl InkPipe {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> InkPipe {
        let src = format!("{}\n{}", kit::WGSL, include_str!("../../shaders/paisaje_ink.wgsl"));
        let shader = build_shader(device, "paisaje ink", &src);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("paisaje ink"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("paisaje ink"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let target = [Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })];
        let vl = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vert>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32, 2 => Float32, 3 => Float32x4],
        };
        let mk = |label: &str, vs: &str, fs: &str, buffers: &[Option<wgpu::VertexBufferLayout>]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vs), buffers, compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(fs), targets: &target, compilation_options: Default::default() }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let ink = mk("paisaje ink", "vs_ink", "fs_ink", &[Some(vl)]);
        let grain = mk("paisaje grain", "vs_grain", "fs_grain", &[]);
        InkPipe { ink, grain, layout }
    }
}

pub struct Paisaje {
    base: Fullscreen,
    pipe: Built<InkPipe>,
    cache: Cache,
    scratch: Vec<Vert>,
    /// Vertices of the last frame (for the cost report and the tests).
    pub last_verts: usize,
}

fn shader_src() -> &'static str {
    static SRC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SRC.get_or_init(|| format!("{}\n{}", kit::WGSL, include_str!("../../shaders/paisaje.wgsl")))
}

impl Paisaje {
    pub fn new() -> Paisaje {
        Paisaje { base: Fullscreen::new("landscape", shader_src()), pipe: Built::default(), cache: Cache::default(), scratch: Vec::new(), last_verts: 0 }
    }
}

impl Default for Paisaje {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Paisaje {
    fn id(&self) -> &'static str {
        "landscape"
    }

    fn name(&self) -> &'static str {
        "Paisaje"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn render(&mut self, ctx: &Ctx, floats: &[f32], target: &wgpu::TextureView, format: wgpu::TextureFormat) {
        let pr = Pr::from_row(floats);
        let aspect = ctx.aspect().max(0.01);
        let time = if ctx.time.is_finite() { ctx.time } else { 0.0 };
        let slot = |blk: usize| &floats[off(blk)..off(blk) + 12];
        let slot_gpu = |blk: usize| ColorSlotGpu::from_floats(slot(blk));
        let view = View::new(&pr.cam, time);
        let look = pr.look;

        let lay_far = lay_of(0, pr.layers);
        let horizon = base_y(&lay_far, pr.agua) - 160.0 * lay_far.sc;
        let base = BaseU {
            sky: slot_gpu(B_CIELO),
            paper: slot_gpu(B_PAPEL),
            accent: slot_gpu(B_ACENTO),
            a: [aspect, time, if ctx.fondo_alpha { 1.0 } else { 0.0 }, look.background_level],
            b: [pr.sun_x, pr.sun_y, pr.sun_size, horizon],
            c: [look.shine, look.contrast, look.opacity, look.tone],
            d: [look.saturation, view.cs, view.sn, view.zoom],
            e: [view.pan[0], view.pan[1], Y_CEN, VIEW_H],
        };
        self.base.draw(ctx, target, format, &base);

        let slots = Slots { ink: slot(B_TINTA), paper: slot(B_PAPEL), accent: slot(B_ACENTO) };
        let info = build_frame(&mut self.cache, &pr, time, aspect, ctx.fondo_alpha, &slots, &mut self.scratch);
        self.last_verts = info.verts;

        let pipe = self.pipe.get(format, || InkPipe::new(ctx.device, format));
        let dark = grad4(slot(B_TINTA), 0.65);
        let light = grad4(slot(B_PAPEL), 0.0);
        let uniform = InkU {
            look: [look.shine, look.contrast, look.tone, look.saturation],
            misc: [aspect, look.opacity, pr.truido, time],
            grain: [pr.grain, if ctx.fondo_alpha { look.background_level } else { 1.0 }, 1.0, 0.0],
            gdark: [dark[0], dark[1], dark[2], 1.0],
            glight: [(light[0] + 1.0) * 0.5, (light[1] + 1.0) * 0.5, (light[2] + 1.0) * 0.5, 1.0],
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("paisaje ink uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("paisaje ink"),
            layout: &pipe.layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });
        let vbuf = (!self.scratch.is_empty()).then(|| {
            ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("paisaje vertices"),
                contents: bytemuck::cast_slice(&self.scratch),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        let mut enc = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("paisaje ink") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("paisaje ink"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &bind, &[]);
            if let Some(vbuf) = &vbuf {
                pass.set_pipeline(&pipe.ink);
                pass.set_vertex_buffer(0, vbuf.slice(..));
                pass.draw(0..self.scratch.len() as u32, 0..1);
            }
            if pr.grain > 0.0 {
                pass.set_pipeline(&pipe.grain);
                pass.draw(0..3, 0..1);
            }
        }
        ctx.queue.submit(Some(enc.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
    const W: u32 = 128;
    const H: u32 = 80;

    fn frame_at(gpu: &crate::gpu::Gpu, model: &mut Paisaje, floats: &[f32], time: f32, w: u32, h: u32) -> Vec<u8> {
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time, width: w, height: h, fondo_alpha: false, detail: 1.0 };
        model.render(&ctx, floats, &view, FORMAT);
        crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, w, h)
    }

    fn frames(gpu: &crate::gpu::Gpu, floats: &[f32]) -> Vec<u8> {
        frames_at(gpu, floats, W, H)
    }

    fn frames_at(gpu: &crate::gpu::Gpu, floats: &[f32], w: u32, h: u32) -> Vec<u8> {
        let mut m = Paisaje::new();
        [0.0f32, 1.3].iter().flat_map(|&t| frame_at(gpu, &mut m, floats, t, w, h)).collect()
    }

    fn base() -> Vec<f32> {
        SCHEMA.defaults()
    }

    fn colours(px: &[u8]) -> usize {
        let mut s = std::collections::HashSet::new();
        for p in px.chunks_exact(4) {
            s.insert([p[0], p[1], p[2]]);
        }
        s.len()
    }

    fn slots_of(f: &[f32]) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let g = |b: usize| f[off(b)..off(b) + 12].to_vec();
        (g(B_TINTA), g(B_PAPEL), g(B_ACENTO))
    }

    /// The vertex build alone, no GPU.
    fn build(cache: &mut Cache, f: &[f32], t: f32, alpha: bool) -> (Vec<Vert>, FrameInfo) {
        let (ti, pa, ac) = slots_of(f);
        let slots = Slots { ink: &ti, paper: &pa, accent: &ac };
        let mut out = Vec::new();
        let info = build_frame(cache, &Pr::from_row(f), t, 16.0 / 9.0, alpha, &slots, &mut out);
        (out, info)
    }

    #[test]
    fn widths_and_indices_come_from_the_blocks() {
        assert_eq!(SCHEMA.defaults().len(), SCHEMA.width());
        assert_eq!(off(B_OWN), 0);
        assert_eq!(off(B_TINTA), FLOATS.len());
        assert_eq!(off(B_CAM) + kit::Cam2d::N, off(B_LOOK));
        assert_eq!(off(B_LOOK) + kit::Look::N, off(B_LOOK + 1));
        let n = |i: usize| FLOATS[i].name;
        assert_eq!((n(F_VEL), n(F_DIR), n(F_SEMILLA), n(F_CAPAS), n(F_ALT), n(F_PICOS)), ("scroll_speed", "scroll_direction", "seed_drift", "layers", "mountain_height", "mountain_peaks"));
        assert_eq!((n(F_RUIDO), n(F_DIST), n(F_NIEBLA), n(F_AGUA), n(F_ONDAS), n(F_REFLEJO)), ("mountain_noise", "mountain_distance", "fog", "water_level", "water_waves", "reflection"));
        assert_eq!((n(F_ADENS), n(F_AALT), n(F_RAMA), n(F_VIENTO), n(F_ROCA), n(F_NUBE), n(F_NVEL)), ("tree_density", "tree_height", "tree_branch", "tree_wind", "rock_density", "cloud_density", "cloud_speed"));
        assert_eq!((n(F_AVE), n(F_BARCA), n(F_SOLX), n(F_SOLY), n(F_SOLT)), ("bird_density", "boat_density", "sun_x", "sun_y", "sun_size"));
        assert_eq!((n(F_GROSOR), n(F_TRUIDO), n(F_RELLENO), n(F_GRANO), n(F_SELLO)), ("stroke_thickness", "stroke_noise", "fill", "paper_grain", "seal"));
        assert_eq!(FLOATS.len(), F_SELLO + 1);
        for i in [F_SEMILLA, F_CAPAS, F_PICOS, F_RAMA] {
            assert!(FLOATS[i].drive < 1.0, "{} is structural", FLOATS[i].name);
        }
    }

    #[test]
    fn chunks_are_a_pure_function_of_the_key() {
        let key = Pr::from_row(&base()).key();
        let a = gen_layer_chunk(&key, 2, 5);
        let b = gen_layer_chunk(&key, 2, 5);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.verts.len(), y.verts.len());
            assert!(x.verts.iter().zip(&y.verts).all(|(p, q)| p.x == q.x && p.y == q.y && p.alpha == q.alpha));
        }
        let other = gen_layer_chunk(&GenKey { seed: key.seed + 1, ..key }, 2, 5);
        let same = a.len() == other.len() && a.iter().zip(&other).all(|(x, y)| x.verts.len() == y.verts.len());
        assert!(!same, "a new seed must be a new landscape");
        for l in 0..4u8 {
            for k in -3..4 {
                for o in gen_layer_chunk(&key, l, k) {
                    assert!(o.verts.len() < 30_000, "one object is {} vertices", o.verts.len());
                    assert!(o.verts.iter().all(|v| v.x.is_finite() && v.y.is_finite() && v.alpha.is_finite()));
                }
            }
        }
    }

    #[test]
    fn the_cache_does_not_change_the_picture_and_only_new_chunks_are_made() {
        let f = base();
        let mut warm = Cache::default();
        // Walk across several chunk boundaries a little at a time.
        let mut last = Vec::new();
        let mut made_total = 0;
        for i in 0..80 {
            let t = i as f32 * 0.5;
            let (v, info) = build(&mut warm, &f, t, false);
            made_total += info.made;
            last = v;
        }
        let (cold, _) = build(&mut Cache::default(), &f, 39.5, false);
        assert_eq!(last.len(), cold.len());
        assert!(last.iter().zip(&cold).all(|(a, b)| a.pos == b.pos && a.color == b.color), "history changed the geometry");
        // Same instant again: nothing new.
        let (_, again) = build(&mut warm, &f, 39.5, false);
        assert_eq!(again.made, 0);
        // The walk covers ~1100 units of the nearest layer: a handful of new
        // chunks per layer, not a rebuild per frame.
        let first = {
            let (_, i) = build(&mut Cache::default(), &f, 0.0, false);
            i.made
        };
        assert!(made_total < first + 5 * 6, "walk made {made_total} chunks, a cold frame is {first}");
    }

    #[test]
    fn scrolling_is_continuous_across_a_chunk_boundary() {
        // One layer scrolling at scroll_vel: frames either side of a chunk edge
        // differ by no more than frames away from it. A seam that popped an
        // object in or out would be a spike in the per-step pixel difference.
        let gpu = crate::gpu::Gpu::new_headless();
        let mut f = base();
        f[F_CAPAS] = 1.0;
        f[F_VEL] = 60.0;
        let mut m = Paisaje::new();
        let dt = 0.1f32;
        let edge = 512.0 * 3.0 / 60.0;
        let mut diffs = Vec::new();
        let mut prev: Option<Vec<u8>> = None;
        for i in -25..25 {
            let px = frame_at(&gpu, &mut m, &f, edge + i as f32 * dt, 192, 108);
            if let Some(p) = &prev {
                diffs.push(px.iter().zip(p).filter(|(a, b)| a != b).count());
            }
            prev = Some(px);
        }
        let mut sorted = diffs.clone();
        sorted.sort_unstable();
        let median = sorted[sorted.len() / 2];
        let worst = *sorted.last().expect("diffs");
        assert!(median > 0, "the picture should be moving");
        assert!(worst < median * 3 + 200, "a pop at the seam: worst step {worst} vs median {median} ({diffs:?})");
    }

    #[test]
    fn chunk_seams_hold_the_same_geometry_from_either_side() {
        // An object that straddles a seam is anchored once; the continuous
        // fields are sampled from absolute positions. So the two ridge samples
        // at the shared edge are equal whichever chunk "owns" them.
        let key = Pr::from_row(&base()).key();
        let lay = lay_of(0, key.layers as usize);
        let a = field_mesh(0, &lay, 1000.0, 600.0, 0.0, &key, 0, 0.5);
        let b = field_mesh(0, &lay, 1024.0, 600.0, 0.0, &key, 0, 0.5);
        // Same absolute u -> same y, regardless of the window it was cut from.
        let at = |m: &Mesh, u: f32, cu: f32| m.v.iter().find(|v| (v.x - (u - cu)).abs() < 1e-3 && v.kind == K_PAPER).map(|v| v.y);
        let (ya, yb) = (at(&a, 1296.0, 1000.0), at(&b, 1296.0, 1024.0));
        assert!(ya.is_some() && ya == yb, "{ya:?} vs {yb:?}");
    }

    #[test]
    fn the_defaults_are_not_flat_and_deterministic() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = base();
        let a = frames(&gpu, &f);
        assert_eq!(a, frames(&gpu, &f));
        let first = &a[..(W * H * 4) as usize];
        assert!(colours(first) > 40, "default frame is flat ({} colours)", colours(first));
        assert_ne!(first, &a[(W * H * 4) as usize..], "the landscape must move with time");
    }

    #[test]
    fn every_float_moves_the_frame() {
        let gpu = crate::gpu::Gpu::new_headless();
        let (cam, look) = (off(B_CAM), off(B_LOOK));
        let mut idx: Vec<(usize, FloatSpec)> = FLOATS.iter().enumerate().map(|(i, s)| (i, *s)).collect();
        idx.extend(kit::CAM2D.iter().enumerate().map(|(i, s)| (cam + i, *s)));
        idx.extend(kit::LOOK.iter().enumerate().map(|(i, s)| (look + i, *s)));
        let mut b = base();
        // Give the paper a visible grey so `background_level` has something to dim.
        for v in &mut b[off(B_PAPEL)..off(B_PAPEL) + 12] {
            *v = 0.6;
        }
        let at = |name: &str, b: &mut Vec<f32>, v: f32| {
            let i = FLOATS.iter().position(|s| s.name == name).expect("float");
            b[i] = v;
        };
        for (i, spec) in idx {
            let mut b = b.clone();
            // A few floats only show when their subject is large enough to see
            // at this size.
            match spec.name {
                "tree_wind" | "tree_branch" | "tree_height" | "tree_density" => {
                    at("tree_density", &mut b, 2.0);
                    at("layers", &mut b, 2.0);
                }
                "water_waves" | "reflection" => {
                    at("fog", &mut b, 0.0);
                }
                "seal" => {}
                _ => {}
            }
            // Hairline trees need pixels to show up in.
            let (w, h) = if spec.name.starts_with("tree_") || matches!(spec.name, "bird_density" | "boat_density" | "rock_density") { (512, 288) } else { (W, H) };
            let reference = frames_at(&gpu, &b, w, h);
            let far = if (spec.max - b[i]).abs() > (b[i] - spec.min).abs() { spec.max } else { spec.min };
            let far = if spec.name == "tone" { 0.5 } else { far };
            let mut f = b.clone();
            f[i] = far;
            assert!(reference != frames_at(&gpu, &f, w, h), "float `{}` ({} -> {far}) did not move the frame", spec.name, b[i]);
        }
    }

    #[test]
    fn extreme_floats_are_finite_and_under_the_vertex_cap() {
        let gpu = crate::gpu::Gpu::new_headless();
        for pick in 0..5 {
            let mut f = base();
            for (i, s) in FLOATS.iter().enumerate() {
                f[i] = match pick {
                    0 => s.min,
                    1 => s.max,
                    2 => if i % 2 == 0 { s.min } else { s.max },
                    3 => if i % 2 == 0 { s.max } else { s.min },
                    _ => f32::NAN,
                };
            }
            let c = off(B_CAM);
            for (i, s) in kit::CAM2D.iter().enumerate() {
                f[c + i] = if pick == 1 || pick == 3 { s.max } else { s.min };
            }
            let mut cache = Cache::default();
            for t in [0.0f32, 7.3, 1.0e5] {
                let (v, info) = build(&mut cache, &f, t, false);
                assert!(info.verts <= CAP_VERTS, "pick {pick}: {} vertices", info.verts);
                assert!(v.iter().all(|x| x.pos[0].is_finite() && x.pos[1].is_finite() && x.color.iter().all(|c| c.is_finite())), "pick {pick} t {t}: non-finite vertex");
            }
            let mut m = Paisaje::new();
            let px = frame_at(&gpu, &mut m, &f, 3.0, W, H);
            assert!(m.last_verts <= CAP_VERTS);
            if pick != 4 {
                assert!(colours(&px) > 2, "pick {pick} drew a flat frame");
            }
        }
    }

    #[test]
    fn a_fondo_alpha_frame_is_transparent_where_nothing_is_drawn() {
        let gpu = crate::gpu::Gpu::new_headless();
        let mut f = base();
        f[off(B_LOOK) + 3] = 0.0; // fondo_nivel
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, W, H, FORMAT);
        let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: 0.4, width: W, height: H, fondo_alpha: true, detail: 1.0 };
        Paisaje::new().render(&ctx, &f, &view, FORMAT);
        let px = crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, W, H);
        assert!(px.chunks_exact(4).any(|p| p[3] == 0), "some of the frame should be see-through");
        assert!(px.chunks_exact(4).any(|p| p[3] > 0), "the ink should still draw");
    }

    /// Prints the cost of a warm frame; the budget is a loose guard so a
    /// regression of an order of magnitude cannot slip in.
    #[test]
    fn a_warm_frame_is_cheap() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = base();
        let mut m = Paisaje::new();
        let (w, h) = (640u32, 360u32);
        let _ = frame_at(&gpu, &mut m, &f, 0.0, w, h);
        let n = 30;
        let t0 = std::time::Instant::now();
        for i in 0..n {
            let _ = frame_at(&gpu, &mut m, &f, 0.1 + i as f32 / 60.0, w, h);
        }
        let ms = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
        let mut cache = Cache::default();
        let (_, _) = build(&mut cache, &f, 0.0, false);
        let t1 = std::time::Instant::now();
        let mut verts = 0;
        for i in 0..n {
            let (_, info) = build(&mut cache, &f, 0.1 + i as f32 / 60.0, false);
            verts = info.verts;
        }
        let cpu = t1.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("paisaje: {ms:.2} ms/frame incl. readback ({}x{}), {cpu:.2} ms CPU build, {verts} vertices", w, h);
        assert!(ms < 400.0, "a warm frame took {ms} ms");
    }
}

