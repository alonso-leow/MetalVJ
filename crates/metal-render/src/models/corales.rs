//! Corales — differential line growth, after inconvergent's differential-line
//! (MIT). Written from the algorithm; no code was taken.
//!
//! A closed (or open) polyline of nodes grows by three local rules, repeated:
//! every node is pulled towards its two curve neighbours when they are further
//! than the resting length, pushed away from every *other* node closer than
//! `rep_radio` (found through a spatial hash), and nudged by smoothing and
//! noise; then long edges split at the midpoint (more often where the curve
//! bends: the `curl` bias of the source's `spawn_curl`) and very short edges
//! collapse. The pushing keeps the curve from touching itself while the
//! splitting keeps it from going straight, so it buckles into the folds of a
//! brain or a coral.
//!
//! **State advances by time.** The curve is the accumulation of its own
//! history, so like `dla` and `clouds` it is not a pure function of the clock:
//! iterations are `pasos_frame * 60 * crecimiento_vel` per second, a fractional
//! rate is carried, asking for the same instant twice steps nothing, and a
//! resize, a rewind, a jump beyond [`SEEK_GAP`], a changed seed or shape, or a
//! rising `reinicio_ritmo` starts a new curve (with a burn-in so the first frame
//! already has folds). Everything is deterministic: a splitmix stream seeded
//! from `semilla`, the shape and how many restarts have happened.
//!
//! **Never stalls, never explodes.** Splitting stops at `nodos_max` (hard cap
//! [`HARD_CAP`]); a curve that has sat full for [`FULL_HOLD`] seconds restarts
//! with a new variant instead of freezing for ever. Every force is a fraction of
//! the resting length and the per-iteration move is clamped to a fraction of it,
//! the per-frame iteration count is capped by node count, parameters are
//! clamped to their ranges, and positions stay inside a box.
//!
//! **Drawing.** The curve is transformed by the CAM2D kit on the CPU, then
//! drawn as variable-width triangle strips (width by age/curvature, a halo at
//! the young, sharply bent tips). The optional fill is the even-odd interior,
//! rasterised on the CPU into a small coverage mask that `shaders/corales.wgsl`
//! composes; `relleno_modo` 2 lays the picture handed to the `relleno` surface
//! (projector mapping), or a procedural stand-in while none is connected.
//! `shared::line2d` has one flat colour and no per-vertex width, so the strokes
//! use this model's own small pipeline (same NDC convention as `line2d`).

use wgpu::util::DeviceExt;

use crate::model::{color_at, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema, SurfaceSpec};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::kit::{self, CAM2D_BLOCK, LOOK_BLOCK};
use crate::shared::sim::{FixedStep, SIM_HZ};
use crate::shared::surface::Part;
use crate::shared::{build_shader, ColorSlotGpu};

const FLOATS: &[FloatSpec] = &[
    // Seed curve: 0 circle, 1 ring (two loops), 2 line (open), 3 blob. Held.
    spec_held("seed_shape", 0.0, 3.0, 0.0, 0.2),
    // Changes the random stream (a different coral, same rules). Held.
    spec_held("seed", 0.0, 1.0, 0.0, 0.0),
    // Node budget; growth stops here. Held.
    spec_held("nodes_max", 200.0, 20000.0, 5000.0, 0.2),
    // Growth iterations per 1/60 s. Held.
    spec_held("steps_frame", 1.0, 16.0, 3.0, 0.2),
    // Distance within which non-neighbour nodes push each other, world units
    // (the frame is 2 high). Held.
    spec_held("repeat_radius", 0.03, 0.3, 0.1, 0.2),
    spec("repeat_force", 0.0, 3.0, 1.0),
    spec("attract_force", 0.0, 2.0, 1.0),
    // Blend of each node's move with its neighbours' (coherent motion).
    spec("align", 0.0, 1.0, 0.3),
    spec("noise", 0.0, 1.0, 0.15),
    // Laplacian smoothing of the curve.
    spec("tension", 0.0, 1.0, 0.25),
    // 0 split anywhere, 1 split where the curve bends most.
    spec("curl", 0.0, 1.0, 0.6),
    spec("split_threshold", 0.006, 0.06, 0.02),
    spec("split_prob", 0.0, 1.0, 0.1),
    spec("collapse", 0.0, 1.0, 0.25),
    spec("growth_speed", 0.0, 2.0, 1.0),
    // A rising edge through 0.5 restarts the curve (the drop).
    spec("reset_rhythm", 0.0, 1.0, 0.0),
    // Beat tremor of the drawn curve (never of the simulation).
    spec("shake", 0.0, 1.0, 0.0),
    spec("stroke_width", 0.001, 0.03, 0.008),
    spec("stroke_width_variation", 0.0, 1.0, 0.5),
    spec("stroke_shine", 0.0, 3.0, 1.1),
    spec("stroke_alpha", 0.0, 1.0, 0.9),
    spec("stroke_noise", 0.0, 1.0, 0.2),
    // Glow at the young, bent tips.
    spec("tip", 0.0, 1.0, 0.5),
    // 0 none, 1 solid, 2 source. Held.
    spec_held("fill_mode", 0.0, 2.0, 0.0, 0.2),
    spec("fill_alpha", 0.0, 1.0, 0.6),
    spec("fill_scale", 0.1, 5.0, 1.0),
    spec("fill_turn", -1.0, 1.0, 0.0),
];

const F_FORMA: usize = 0;
const F_SEMILLA: usize = 1;
const F_NODOS: usize = 2;
const F_PASOS: usize = 3;
const F_RADIO: usize = 4;
const F_REP: usize = 5;
const F_ATRAC: usize = 6;
const F_ALINEA: usize = 7;
const F_RUIDO: usize = 8;
const F_TENSION: usize = 9;
const F_CURL: usize = 10;
const F_UMBRAL: usize = 11;
const F_PROB: usize = 12;
const F_COLAPSO: usize = 13;
const F_VEL: usize = 14;
const F_REINICIO: usize = 15;
const F_SACUDIDA: usize = 16;
const F_ANCHO: usize = 17;
const F_ANCHO_VAR: usize = 18;
const F_BRILLO: usize = 19;
const F_ALFA: usize = 20;
const F_TRUIDO: usize = 21;
const F_PUNTA: usize = 22;
const F_MODO: usize = 23;
const F_RALFA: usize = 24;
const F_RESC: usize = 25;
const F_RGIRO: usize = 26;

/// Dark red through coral to cream: old curve dark, young tips bright.
const TRAZO: [f32; 12] = [
    0.30, 0.03, 0.05, //
    0.75, 0.14, 0.12, //
    1.00, 0.42, 0.28, //
    1.00, 0.86, 0.70,
];
/// Deep teal fill, lit towards the middle of the cells.
const RELLENO: [f32; 12] = [
    0.004, 0.012, 0.030, //
    0.012, 0.070, 0.130, //
    0.040, 0.220, 0.300, //
    0.200, 0.560, 0.560,
];
const FONDO: [f32; 12] = [
    0.003, 0.004, 0.010, //
    0.003, 0.004, 0.010, //
    0.006, 0.008, 0.018, //
    0.008, 0.010, 0.022,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("stroke", &TRAZO),
        color_at("fill", &RELLENO),
        color_at("background", &FONDO),
        CAM2D_BLOCK,
        LOOK_BLOCK,
        crate::model::SIGNAL_FLOATS_BLOCK,
        crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

const B_OWN: usize = 0;
const B_TRAZO: usize = 1;
const B_RELLENO: usize = 2;
const B_FONDO: usize = 3;
const B_CAM: usize = 4;
const B_LOOK: usize = 5;

fn off(block: usize) -> usize {
    kit::offset_of(SCHEMA.blocks, block)
}

const SURFACES: &[&str] = &["fill"];

// -- simulation ---------------------------------------------------------------

/// Hard ceiling on nodes, whatever `nodos_max` says.
pub const HARD_CAP: usize = 20_000;
/// Most nodes a seed curve starts with (all loops together).
const SEED_MAX: usize = 1200;
/// A forward jump beyond this is a seek and restarts the curve.
const SEEK_GAP: f32 = 30.0;
/// Iterations run at seed time so the first frame already has folds.
const BURN_IN: u32 = 24;
/// Seconds a full curve may sit before it restarts as a new variant.
const FULL_HOLD: f32 = 10.0;
/// Positions never leave `[-BOX, BOX]`.
const BOX: f32 = 8.0;
/// Force constants: displacement per iteration as a fraction of the resting
/// length (`split_umbral`) per unit of the float.
const K_REP: f32 = 0.012;
const K_SPRING: f32 = 0.5;
const K_TENSION: f32 = 0.5;
const K_NOISE: f32 = 0.06;
/// A single iteration never moves a node more than this many resting lengths.
const MAX_MOVE: f32 = 0.35;
const NONE: u32 = u32::MAX;
/// Candidate neighbours a node looks at per iteration, at most: bounds the
/// cost of a huge `rep_radio` on a huge curve.
const MAX_CANDIDATES: usize = 160;
/// Candidate visits one frame may spend on growth iterations (about 5 ms),
/// after the first iteration, which always runs.
const WORK_BUDGET: u64 = 2_500_000;

#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub p: [f32; 2],
    pub born: u32,
}

/// A run of `len` nodes starting at `start`; `closed` joins last to first.
#[derive(Clone, Copy, Debug)]
pub struct Loop {
    pub start: usize,
    pub len: usize,
    pub closed: bool,
}

/// The simulation floats, clamped to their ranges (NaN reads as the default).
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub cap: usize,
    pub radius: f32,
    pub rep: f32,
    pub atrac: f32,
    pub align: f32,
    pub noise: f32,
    pub tension: f32,
    pub curl: f32,
    pub threshold: f32,
    pub prob: f32,
    pub collapse: f32,
}

fn clean(row: &[f32], i: usize) -> f32 {
    let s = &FLOATS[i];
    let v = row.get(i).copied().unwrap_or(s.default);
    if v.is_finite() {
        v.clamp(s.min, s.max)
    } else {
        s.default
    }
}

impl Params {
    pub fn from_row(row: &[f32]) -> Params {
        Params {
            cap: (clean(row, F_NODOS).round() as usize).min(HARD_CAP),
            radius: clean(row, F_RADIO),
            rep: clean(row, F_REP),
            atrac: clean(row, F_ATRAC),
            align: clean(row, F_ALINEA),
            noise: clean(row, F_RUIDO),
            tension: clean(row, F_TENSION),
            curl: clean(row, F_CURL),
            threshold: clean(row, F_UMBRAL),
            prob: clean(row, F_PROB),
            collapse: clean(row, F_COLAPSO),
        }
    }
}

/// splitmix64: a small deterministic stream.
#[derive(Clone, Copy, Debug)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `[0, 1)`.
    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn signed(&mut self) -> f32 {
        self.f() * 2.0 - 1.0
    }
}

/// A uniform grid over points: `query` visits every point within a radius of
/// a position, without testing all of them.
pub struct Grid {
    cell: f32,
    ox: f32,
    oy: f32,
    nx: usize,
    ny: usize,
    start: Vec<u32>,
    items: Vec<u32>,
    /// Positions in cell order, so a cell is one contiguous read.
    ordered: Vec<[f32; 2]>,
}

const GRID_MAX_DIM: usize = 512;

impl Grid {
    pub fn build(pts: &[[f32; 2]], cell: f32) -> Grid {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in pts {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        if pts.is_empty() {
            (x0, y0, x1, y1) = (0.0, 0.0, 0.0, 0.0);
        }
        let cell = cell.max((x1 - x0).max(y1 - y0) / GRID_MAX_DIM as f32).max(1e-4);
        let nx = ((x1 - x0) / cell) as usize + 1;
        let ny = ((y1 - y0) / cell) as usize + 1;
        let mut g = Grid { cell, ox: x0, oy: y0, nx, ny, start: vec![0; nx * ny + 1], items: vec![0; pts.len()], ordered: vec![[0.0; 2]; pts.len()] };
        for p in pts {
            let c = g.cell_of(*p);
            g.start[c + 1] += 1;
        }
        for c in 0..nx * ny {
            g.start[c + 1] += g.start[c];
        }
        let mut fill = g.start.clone();
        for (i, p) in pts.iter().enumerate() {
            let c = g.cell_of(*p);
            g.items[fill[c] as usize] = i as u32;
            g.ordered[fill[c] as usize] = *p;
            fill[c] += 1;
        }
        g
    }

    fn cell_of(&self, p: [f32; 2]) -> usize {
        let cx = (((p[0] - self.ox) / self.cell) as usize).min(self.nx - 1);
        let cy = (((p[1] - self.oy) / self.cell) as usize).min(self.ny - 1);
        cy * self.nx + cx
    }

    /// Calls `f(i)` for every point in a cell the disc of radius `r` around
    /// `p` touches (the caller still tests the distance), until `f` returns
    /// `false`. `f` gets the point's index and position.
    pub fn query(&self, p: [f32; 2], r: f32, mut f: impl FnMut(usize, [f32; 2]) -> bool) {
        let lo = |v: f32, o: f32, n: usize| (((v - o - r) / self.cell).floor().max(0.0) as usize).min(n - 1);
        let hi = |v: f32, o: f32, n: usize| (((v - o + r) / self.cell).floor().max(0.0) as usize).min(n - 1);
        let (cx0, cx1) = (lo(p[0], self.ox, self.nx), hi(p[0], self.ox, self.nx));
        let (cy0, cy1) = (lo(p[1], self.oy, self.ny), hi(p[1], self.oy, self.ny));
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                let c = cy * self.nx + cx;
                for k in self.start[c]..self.start[c + 1] {
                    if !f(self.items[k as usize] as usize, self.ordered[k as usize]) {
                        return;
                    }
                }
            }
        }
    }
}

/// Threads for one force pass: none for small curves, and none in the
/// browser build, which has no threads.
fn worker_count(n: usize) -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = n;
        1
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if n < 1500 {
            return 1;
        }
        std::thread::available_parallelism().map_or(1, |c| c.get()).clamp(1, 4)
    }
}

pub struct Sim {
    pub nodes: Vec<Node>,
    pub loops: Vec<Loop>,
    pub iter: u32,
    rng: Rng,
    nbr: Vec<[u32; 2]>,
    lid: Vec<u32>,
    loc: Vec<u32>,
    pub turn: Vec<f32>,
    disp: Vec<[f32; 2]>,
    pts: Vec<[f32; 2]>,
    /// Candidate neighbours visited by the last step (its cost).
    pub work: u64,
}

impl Sim {
    /// A fresh curve of `shape` (0 circle, 1 ring, 2 line, 3 blob) with node
    /// spacing near `umbral`, from `seed`.
    pub fn new(shape: u32, seed: u64, threshold: f32) -> Sim {
        let mut rng = Rng(seed ^ 0xC0DA_1E5);
        let mut nodes = Vec::new();
        let mut loops = Vec::new();
        let spacing = (threshold * 0.8).max(1e-3);
        let count = |len: f32, per: usize| ((len / spacing) as usize).clamp(12, per);
        let closed_loop = |nodes: &mut Vec<Node>, loops: &mut Vec<Loop>, rng: &mut Rng, r0: f32, wob: [f32; 4], n: usize| {
            let start = nodes.len();
            let (ph1, ph2) = (rng.f() * 6.283, rng.f() * 6.283);
            for k in 0..n {
                let a = k as f32 / n as f32 * std::f32::consts::TAU;
                let r = r0 * (1.0 + wob[0] * (3.0 * a + ph1).sin() + wob[1] * (5.0 * a + ph2).sin())
                    + r0 * 0.004 * rng.signed();
                nodes.push(Node { p: [r * a.cos(), r * a.sin()], born: 0 });
            }
            loops.push(Loop { start, len: n, closed: true });
        };
        match shape {
            1 => {
                closed_loop(&mut nodes, &mut loops, &mut rng, 0.55, [0.0; 4], count(3.45, SEED_MAX / 2));
                closed_loop(&mut nodes, &mut loops, &mut rng, 0.25, [0.0; 4], count(1.57, SEED_MAX / 2));
            }
            2 => {
                let n = count(1.4, SEED_MAX);
                for k in 0..n {
                    let x = -0.7 + 1.4 * k as f32 / (n - 1) as f32;
                    nodes.push(Node { p: [x, 0.004 * rng.signed()], born: 0 });
                }
                loops.push(Loop { start: 0, len: n, closed: false });
            }
            3 => closed_loop(&mut nodes, &mut loops, &mut rng, 0.3, [0.25, 0.15, 0.0, 0.0], count(1.9, SEED_MAX)),
            _ => closed_loop(&mut nodes, &mut loops, &mut rng, 0.32, [0.0; 4], count(2.0, SEED_MAX)),
        }
        let n = nodes.len();
        Sim {
            nodes,
            loops,
            iter: 0,
            rng,
            nbr: vec![[NONE; 2]; n],
            lid: vec![0; n],
            loc: vec![0; n],
            turn: vec![0.0; n],
            disp: vec![[0.0; 2]; n],
            pts: vec![[0.0; 2]; n],
            work: 0,
        }
    }

    fn topology(&mut self) {
        let n = self.nodes.len();
        self.nbr.resize(n, [NONE; 2]);
        self.lid.resize(n, 0);
        self.loc.resize(n, 0);
        self.turn.resize(n, 0.0);
        self.disp.resize(n, [0.0; 2]);
        self.pts.clear();
        self.pts.extend(self.nodes.iter().map(|nd| nd.p));
        for (l, lp) in self.loops.iter().enumerate() {
            for k in 0..lp.len {
                let i = lp.start + k;
                let prev = if k > 0 {
                    (i - 1) as u32
                } else if lp.closed {
                    (lp.start + lp.len - 1) as u32
                } else {
                    NONE
                };
                let next = if k + 1 < lp.len {
                    (i + 1) as u32
                } else if lp.closed {
                    lp.start as u32
                } else {
                    NONE
                };
                self.nbr[i] = [prev, next];
                self.lid[i] = l as u32;
                self.loc[i] = k as u32;
            }
        }
    }

    /// Turning angle at every node, radians (0 at open ends).
    pub fn compute_turns(&mut self) {
        let n = self.nodes.len();
        self.topology();
        for i in 0..n {
            let [pv, nx] = self.nbr[i];
            self.turn[i] = if pv == NONE || nx == NONE {
                0.0
            } else {
                let (a, b, c) = (self.pts[pv as usize], self.pts[i], self.pts[nx as usize]);
                let (d1, d2) = ([b[0] - a[0], b[1] - a[1]], [c[0] - b[0], c[1] - b[1]]);
                let cross = d1[0] * d2[1] - d1[1] * d2[0];
                let dot = d1[0] * d2[0] + d1[1] * d2[1];
                cross.atan2(dot).abs()
            };
        }
    }

    /// The force on every node of `out` (node `first + k` is `out[k]`), from
    /// the positions as they are now. Returns the candidates visited.
    fn forces(&self, p: &Params, grid: &Grid, first: usize, out: &mut [[f32; 2]]) -> u64 {
        let nearl = p.threshold;
        let r = p.radius.max(1e-3);
        let r2 = r * r;
        let mut work = 0u64;
        for (k0, slot) in out.iter_mut().enumerate() {
            let i = first + k0;
            let pi = self.pts[i];
            let mut f = [0.0f32; 2];
            // Spring to the curve neighbours when they are too far.
            for &nb in &self.nbr[i] {
                if nb == NONE {
                    continue;
                }
                let q = self.pts[nb as usize];
                let d = [q[0] - pi[0], q[1] - pi[1]];
                let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
                if l > nearl {
                    let k = K_SPRING * p.atrac * (l - nearl) / l;
                    f[0] += d[0] * k;
                    f[1] += d[1] * k;
                }
            }
            // Smoothing towards the neighbours' midpoint.
            let [pv, nx] = self.nbr[i];
            if pv != NONE && nx != NONE {
                let (a, c) = (self.pts[pv as usize], self.pts[nx as usize]);
                f[0] += (0.5 * (a[0] + c[0]) - pi[0]) * K_TENSION * p.tension;
                f[1] += (0.5 * (a[1] + c[1]) - pi[1]) * K_TENSION * p.tension;
            }
            // Repulsion from every node within the radius that is not a near
            // neighbour along the same curve.
            let (li, ki) = (self.lid[i], self.loc[i] as i64);
            let lp = self.loops[li as usize];
            let mut seen = 0usize;
            grid.query(pi, r, |j, q| {
                seen += 1;
                if seen > MAX_CANDIDATES {
                    return false;
                }
                let d = [pi[0] - q[0], pi[1] - q[1]];
                let d2 = d[0] * d[0] + d[1] * d[1];
                if d2 >= r2 || j == i {
                    return true;
                }
                if self.lid[j] == li {
                    let mut dk = (self.loc[j] as i64 - ki).abs();
                    if lp.closed {
                        dk = dk.min(lp.len as i64 - dk);
                    }
                    if dk <= 2 {
                        return true;
                    }
                }
                let distance = d2.sqrt();
                let k = if distance > 1e-6 { (r - distance) / distance } else { 0.0 } * K_REP * p.rep * nearl / 0.02;
                f[0] += d[0] * k;
                f[1] += d[1] * k;
                true
            });
            work += seen as u64;
            let (ix, iy) = (i as i32, self.iter as i32);
            f[0] += (hash3(ix, iy, 11) * 2.0 - 1.0) * K_NOISE * p.noise * nearl;
            f[1] += (hash3(ix, iy, 12) * 2.0 - 1.0) * K_NOISE * p.noise * nearl;
            *slot = f;
        }
        work
    }

    /// One growth iteration: relax positions, then split and collapse.
    pub fn step(&mut self, p: &Params) {
        let n = self.nodes.len();
        if n == 0 {
            return;
        }
        self.compute_turns();
        let nearl = p.threshold;
        let r = p.radius.max(1e-3);
        let grid = Grid::build(&self.pts, r);
        let mut disp = std::mem::take(&mut self.disp);
        let threads = worker_count(n);
        let work = if threads <= 1 {
            self.forces(p, &grid, 0, &mut disp)
        } else {
            // Each node's force reads only the frozen positions and a hash of
            // (iteration, node), so the split cannot change the result.
            let chunk = n.div_ceil(threads);
            let this: &Sim = self;
            let grid = &grid;
            std::thread::scope(|sc| {
                let handles: Vec<_> = disp
                    .chunks_mut(chunk)
                    .enumerate()
                    .map(|(c, out)| sc.spawn(move || this.forces(p, grid, c * chunk, out)))
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap_or(0)).sum()
            })
        };
        self.disp = disp;
        self.work = work;
        // Alignment: blend each move with its neighbours' mean move, clamp,
        // apply. Open ends stay pinned.
        let maxd = MAX_MOVE * nearl;
        let old = std::mem::take(&mut self.disp);
        for i in 0..n {
            let [pv, nx] = self.nbr[i];
            if pv == NONE || nx == NONE {
                continue;
            }
            let (a, c) = (old[pv as usize], old[nx as usize]);
            let mut m = [
                old[i][0] * (1.0 - p.align) + 0.5 * (a[0] + c[0]) * p.align,
                old[i][1] * (1.0 - p.align) + 0.5 * (a[1] + c[1]) * p.align,
            ];
            let l = (m[0] * m[0] + m[1] * m[1]).sqrt();
            if !l.is_finite() {
                continue;
            }
            if l > maxd {
                m = [m[0] / l * maxd, m[1] / l * maxd];
            }
            let np = &mut self.nodes[i].p;
            np[0] = (np[0] + m[0]).clamp(-BOX, BOX);
            np[1] = (np[1] + m[1]).clamp(-BOX, BOX);
        }
        self.disp = old;
        self.iter = self.iter.wrapping_add(1);
        self.rebuild(p);
    }

    /// Splits long edges (more likely where the curve bends) and collapses
    /// short ones, into a fresh node list.
    fn rebuild(&mut self, p: &Params) {
        let n = self.nodes.len();
        let mut budget = p.cap.saturating_sub(n);
        let mut out: Vec<Node> = Vec::with_capacity(n + n / 8 + 8);
        let mut loops: Vec<Loop> = Vec::with_capacity(self.loops.len());
        let collapse_len = p.collapse * 0.8 * p.threshold;
        let curl = p.curl;
        let turn = std::mem::take(&mut self.turn);
        // Turning angles were computed before the move; they are only a bias.
        for lp in self.loops.clone() {
            let start = out.len();
            let min_live = if lp.closed { 6 } else { 3 };
            let mut live = lp.len;
            let mut k = 0;
            while k < lp.len {
                let i = lp.start + k;
                let wrap = k + 1 == lp.len;
                let j = if !wrap { i + 1 } else if lp.closed { lp.start } else { i };
                let has_next = !wrap || lp.closed;
                let (a, b) = (self.nodes[i], self.nodes[j]);
                let e = ((b.p[0] - a.p[0]).powi(2) + (b.p[1] - a.p[1]).powi(2)).sqrt();
                if has_next && !wrap && e < collapse_len && live > min_live && self.rng.f() < 0.5 {
                    out.push(Node { p: [0.5 * (a.p[0] + b.p[0]), 0.5 * (a.p[1] + b.p[1])], born: a.born.min(b.born) });
                    live -= 1;
                    k += 2;
                    continue;
                }
                out.push(a);
                if has_next && e > p.threshold && budget > 0 && out.len() + (lp.len - k) < HARD_CAP {
                    let c = 0.5 * (turn.get(i).copied().unwrap_or(0.0) + turn.get(j).copied().unwrap_or(0.0));
                    let bias = (c / 0.5).clamp(0.0, 1.0);
                    let pr = p.prob * (1.0 - curl + curl * bias);
                    if self.rng.f() < pr {
                        out.push(Node { p: [0.5 * (a.p[0] + b.p[0]), 0.5 * (a.p[1] + b.p[1])], born: self.iter });
                        budget -= 1;
                    }
                }
                k += 1;
            }
            loops.push(Loop { start, len: out.len() - start, closed: lp.closed });
        }
        self.nodes = out;
        self.loops = loops;
        self.turn = turn;
        self.topology();
    }
}

/// The clock around a [`Sim`]: when to step, when to restart.
pub struct Driver {
    pub sim: Option<Sim>,
    wall: Option<f32>,
    /// Sim steps owed per sim tick, fractional.
    carry: f32,
    /// The 30 Hz tick clock; the sim steps only on ticks.
    clock: FixedStep,
    key: (u32, u32, u32, u32),
    high: bool,
    pub variant: u32,
    full_t: f32,
}

impl Default for Driver {
    fn default() -> Self {
        Driver {
            sim: None,
            wall: None,
            carry: 0.0,
            clock: FixedStep::default(),
            key: (0, 0, 0, 0),
            high: false,
            variant: 0,
            full_t: 0.0,
        }
    }
}

/// `semilla` quantised to 1/64 of its range: a small modulation keeps the
/// same curve instead of reseeding it.
fn seed_step(own: &[f32]) -> u32 {
    (clean(own, F_SEMILLA) * 64.0).round() as u32
}

impl Driver {
    fn fresh(&mut self, own: &[f32], p: &Params) {
        let shape = clean(own, F_FORMA).round() as u32;
        let seed = (seed_step(own) as u64) << 32 | self.variant as u64;
        let mut sim = Sim::new(shape, seed.wrapping_mul(0x2545_F491_4F6C_DD1D) ^ shape as u64, p.threshold);
        for _ in 0..BURN_IN {
            sim.step(p);
        }
        self.sim = Some(sim);
        self.carry = 0.0;
        self.clock.reset();
        self.full_t = 0.0;
    }

    /// Advances to `time`; `size` is the target size (a resize restarts).
    pub fn update(&mut self, time: f32, size: (u32, u32), own: &[f32]) -> &Sim {
        let p = Params::from_row(own);
        let time = if time.is_finite() { time } else { 0.0 };
        let high = clean(own, F_REINICIO) > 0.5;
        let edge = high && !self.high;
        self.high = high;
        if edge {
            self.variant = self.variant.wrapping_add(1);
        }
        let key = (size.0, size.1, clean(own, F_FORMA).round() as u32, seed_step(own));
        let reseed = edge
            || self.sim.is_none()
            || key != self.key
            || self.wall.map_or(true, |prev| !(0.0..=SEEK_GAP).contains(&(time - prev)));
        let dt = self.wall.map_or(0.0, |prev| (time - prev).clamp(0.0, 0.1));
        self.wall = Some(time);
        self.key = key;
        if reseed {
            self.fresh(own, &p);
            return self.sim.as_ref().expect("seeded above");
        }
        // Steps per second, spread over the sim ticks that fall in this draw.
        let rate = clean(own, F_PASOS) * 60.0 * clean(own, F_VEL) / SIM_HZ;
        let mut whole = 0.0;
        for _ in 0..self.clock.tick(dt) {
            self.carry += rate;
            let w = self.carry.floor().max(0.0);
            self.carry -= w;
            whole += w;
        }
        let wanted = whole.min(32.0) as u32;
        let sim = self.sim.as_mut().expect("present when not reseeding");
        let (mut done, mut spent) = (0u32, 0u64);
        while done < wanted && (done == 0 || spent < WORK_BUDGET) {
            sim.step(&p);
            spent += sim.work;
            done += 1;
        }
        // Whatever the budget refused is dropped (slowdown, not a backlog).
        if dt > 0.0 {
            if sim.nodes.len() + 1 >= p.cap {
                self.full_t += dt;
            } else {
                self.full_t = 0.0;
            }
            if self.full_t > FULL_HOLD {
                self.variant = self.variant.wrapping_add(1);
                self.fresh(own, &p);
            }
        }
        self.sim.as_ref().expect("present")
    }
}

// -- drawing ------------------------------------------------------------------

fn hash3(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ (z as u32).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 16;
    (h & 0xffff) as f32 / 65535.0
}

/// Smooth value noise in `[0, 1]`.
fn vnoise(x: f32, y: f32, z: f32) -> f32 {
    let (xf, yf, zf) = (x.floor(), y.floor(), z.floor());
    let (fx, fy, fz) = (x - xf, y - yf, z - zf);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sy, sz) = (s(fx), s(fy), s(fz));
    let (ix, iy, iz) = (xf as i32, yf as i32, zf as i32);
    let h = |dx, dy, dz| hash3(ix + dx, iy + dy, iz + dz);
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let z0 = l(l(h(0, 0, 0), h(1, 0, 0), sx), l(h(0, 1, 0), h(1, 1, 0), sx), sy);
    let z1 = l(l(h(0, 0, 1), h(1, 0, 1), sx), l(h(0, 1, 1), h(1, 1, 1), sx), sy);
    l(z0, z1, sz)
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct LVert {
    pos: [f32; 2],
    side: f32,
    soft: f32,
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LineUniform {
    look: [f32; 4],
    misc: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FillUniform {
    fill: ColorSlotGpu,
    background: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    e: [f32; 4],
}

struct LinePipe {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl LinePipe {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> LinePipe {
        let src = format!("{}\n{}", kit::WGSL, include_str!("../../shaders/corales_line.wgsl"));
        let shader = build_shader(device, "corales line", &src);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("corales line"),
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
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("corales line"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let vl = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LVert>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32, 2 => Float32, 3 => Float32x4],
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("corales line"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(vl)],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        LinePipe { pipeline, layout }
    }
}

struct Mask {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

impl Mask {
    fn new(device: &wgpu::Device, w: u32, h: u32) -> Mask {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("corales mask"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Mask { texture, view, size: (w, h) }
    }
}

/// The camera: view-space position of a world point, y up, frame height 2.
#[derive(Clone, Copy)]
struct View {
    zoom: f32,
    pan: [f32; 2],
    cs: f32,
    sn: f32,
}

impl View {
    fn new(c: &kit::Cam2d, time: f32) -> View {
        let a = c.turn * std::f32::consts::PI + c.drift * 0.2 * time;
        let (sn, cs) = a.sin_cos();
        View { zoom: c.zoom.clamp(0.1, 8.0), pan: [c.pan_x, c.pan_y], cs, sn }
    }

    fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        let (x, y) = (p[0] - self.pan[0], p[1] - self.pan[1]);
        [(self.cs * x + self.sn * y) * self.zoom, (self.cs * y - self.sn * x) * self.zoom]
    }
}

/// Even-odd interior of the polygons (screen pixel coordinates) as coverage.
pub fn rasterize(polys: &[Vec<[f32; 2]>], w: usize, h: usize) -> Vec<u8> {
    let mut rows: Vec<Vec<f32>> = vec![Vec::new(); h];
    for poly in polys {
        let n = poly.len();
        for k in 0..n {
            let (a, b) = (poly[k], poly[(k + 1) % n]);
            if a[1] == b[1] || !(a[0].is_finite() && a[1].is_finite() && b[0].is_finite() && b[1].is_finite()) {
                continue;
            }
            let (lo, hi) = if a[1] < b[1] { (a, b) } else { (b, a) };
            let r0 = (lo[1] - 0.5).ceil().max(0.0) as i64;
            let r1 = ((hi[1] - 0.5).ceil() as i64 - 1).min(h as i64 - 1);
            for r in r0..=r1 {
                let t = (r as f32 + 0.5 - lo[1]) / (hi[1] - lo[1]);
                rows[r as usize].push(lo[0] + (hi[0] - lo[0]) * t);
            }
        }
    }
    let mut mask = vec![0u8; w * h];
    for (r, xs) in rows.iter_mut().enumerate() {
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in xs.chunks_exact(2) {
            let x0 = (pair[0] - 0.5).ceil().max(0.0) as i64;
            let x1 = ((pair[1] - 0.5).ceil() as i64).min(w as i64);
            for x in x0..x1 {
                mask[r * w + x as usize] = 255;
            }
        }
    }
    mask
}

pub struct Corales {
    driver: Driver,
    fill: Fullscreen,
    lines: Built<LinePipe>,
    mask: Option<Mask>,
    mask_sampler: Option<wgpu::Sampler>,
    /// The last stroke vertex buffer and the vertices it holds; re-uploaded
    /// only when the strokes changed.
    vbuf: Option<(wgpu::Buffer, Vec<LVert>)>,
    surface: Part,
}

fn shader_src() -> &'static str {
    static SRC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SRC.get_or_init(|| format!("{}\n{}", kit::WGSL, include_str!("../../shaders/corales.wgsl")))
}

impl Corales {
    pub fn new() -> Corales {
        Corales {
            driver: Driver::default(),
            fill: Fullscreen::with_textures("corals", shader_src(), 2),
            lines: Built::default(),
            mask: None,
            mask_sampler: None,
            vbuf: None,
            surface: Part::default(),
        }
    }
}

impl Default for Corales {
    fn default() -> Self {
        Self::new()
    }
}

/// Everything the strokes need, computed from the simulation and the row.
struct Strokes {
    halo: Vec<LVert>,
    halo_strips: Vec<(u32, u32)>,
    line: Vec<LVert>,
    line_strips: Vec<(u32, u32)>,
    polys: Vec<Vec<[f32; 2]>>,
}

#[allow(clippy::too_many_arguments)]
fn build_strokes(sim: &Sim, own: &[f32], slot: &ColorSlotGpu, view: &View, time: f32, mask_px: Option<(usize, usize, f32)>) -> Strokes {
    let width = clean(own, F_ANCHO);
    let wvar = clean(own, F_ANCHO_VAR);
    let bright = clean(own, F_BRILLO);
    let alpha = clean(own, F_ALFA);
    let truido = clean(own, F_TRUIDO);
    let tip = clean(own, F_PUNTA);
    let shake = clean(own, F_SACUDIDA);
    let n = sim.nodes.len();
    let now = sim.iter;
    // View-space positions, with the beat tremor.
    let mut vp: Vec<[f32; 2]> = Vec::with_capacity(n);
    for (i, nd) in sim.nodes.iter().enumerate() {
        let mut p = view.apply(nd.p);
        if shake > 0.0 {
            let t = time * 22.0;
            let wob = [vnoise(nd.p[0] * 3.0, nd.p[1] * 3.0, t) - 0.5, vnoise(nd.p[0] * 3.0 + 9.1, nd.p[1] * 3.0, t) - 0.5];
            let jit = [hash3(i as i32, (time * 40.0) as i32, 1) - 0.5, hash3(i as i32, (time * 40.0) as i32, 2) - 0.5];
            p[0] += shake * (wob[0] * 0.05 + jit[0] * 0.006) * view.zoom;
            p[1] += shake * (wob[1] * 0.05 + jit[1] * 0.006) * view.zoom;
        }
        vp.push(p);
    }
    let mut out = Strokes { halo: Vec::new(), halo_strips: Vec::new(), line: Vec::new(), line_strips: Vec::new(), polys: Vec::new() };
    for lp in &sim.loops {
        if lp.len < 2 {
            continue;
        }
        // Per-node colour and width.
        let mut seg_pts: Vec<[f32; 2]> = Vec::with_capacity(lp.len + 1);
        let mut info: Vec<(f32, [f32; 3], f32, f32)> = Vec::with_capacity(lp.len + 1); // width, rgb, tip, alpha
        for k in 0..lp.len {
            let i = lp.start + k;
            let age = now.saturating_sub(sim.nodes[i].born) as f32;
            let youth = (-age / 300.0).exp();
            let tipy = (-age / 40.0).exp();
            let curv = (sim.turn.get(i).copied().unwrap_or(0.0) / 0.6).clamp(0.0, 1.0);
            let t = (0.15 + 0.5 * youth + 0.45 * curv).clamp(0.0, 1.0);
            let nz = vnoise(vp[i][0] * 7.0, vp[i][1] * 7.0, time * 0.6) - 0.5;
            let w = width * view.zoom * (1.0 + wvar * (1.4 * (1.0 - youth) - 0.6 + 1.2 * curv)) * (1.0 + truido * nz * 1.2);
            let b = bright * (1.0 + truido * nz * 1.6);
            let c = slot.grad4(t);
            info.push((w.max(width * view.zoom * 0.1), [c[0] * b, c[1] * b, c[2] * b], tipy * (0.4 + curv), alpha));
            seg_pts.push(vp[i]);
        }
        if lp.closed {
            info.push(info[0]);
            seg_pts.push(seg_pts[0]);
        }
        let m = seg_pts.len();
        // Normals by mitre.
        let seg_n = |a: [f32; 2], b: [f32; 2]| {
            let d = [b[0] - a[0], b[1] - a[1]];
            let l = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-9);
            [-d[1] / l, d[0] / l]
        };
        let mut normals: Vec<([f32; 2], f32)> = Vec::with_capacity(m);
        for k in 0..m {
            let before = if k > 0 { Some(seg_n(seg_pts[k - 1], seg_pts[k])) } else if lp.closed && m > 2 { Some(seg_n(seg_pts[m - 2], seg_pts[0])) } else { None };
            let after = if k + 1 < m { Some(seg_n(seg_pts[k], seg_pts[k + 1])) } else if lp.closed && m > 2 { Some(seg_n(seg_pts[0], seg_pts[1])) } else { None };
            let (nrm, scale) = match (before, after) {
                (Some(a), Some(b)) => {
                    let s = [a[0] + b[0], a[1] + b[1]];
                    let l = (s[0] * s[0] + s[1] * s[1]).sqrt();
                    if l < 1e-4 {
                        (a, 1.0)
                    } else {
                        let nn = [s[0] / l, s[1] / l];
                        let d = (nn[0] * a[0] + nn[1] * a[1]).max(0.5);
                        (nn, 1.0 / d)
                    }
                }
                (Some(a), None) | (None, Some(a)) => (a, 1.0),
                (None, None) => ([0.0, 1.0], 1.0),
            };
            normals.push((nrm, scale));
        }
        let strip = |dst: &mut Vec<LVert>, widen: f32, soft: f32, tipmul: Option<f32>| {
            for k in 0..m {
                let (nrm, scale) = normals[k];
                let (w, rgb, tip, a) = info[k];
                let half = 0.5 * w * widen * scale;
                let (color, aa) = match tipmul {
                    Some(g) => {
                        let hot = slot.grad4(1.0);
                        ([hot[0] * bright, hot[1] * bright, hot[2] * bright, 0.0], g * tip)
                    }
                    None => {
                        let mixw = (tip * tip).clamp(0.0, 1.0) * 0.6;
                        let hot = slot.grad4(1.0);
                        ([rgb[0] + (hot[0] * bright - rgb[0]) * mixw, rgb[1] + (hot[1] * bright - rgb[1]) * mixw, rgb[2] + (hot[2] * bright - rgb[2]) * mixw, 0.0], a)
                    }
                };
                let c = [color[0], color[1], color[2], aa.clamp(0.0, 1.0)];
                let p = seg_pts[k];
                dst.push(LVert { pos: [p[0] - nrm[0] * half, p[1] - nrm[1] * half], side: -1.0, soft, color: c });
                dst.push(LVert { pos: [p[0] + nrm[0] * half, p[1] + nrm[1] * half], side: 1.0, soft, color: c });
            }
        };
        if tip > 0.001 {
            let s = out.halo.len() as u32;
            strip(&mut out.halo, 7.0, 1.0, Some(tip * 0.55));
            out.halo_strips.push((s, out.halo.len() as u32 - s));
        }
        let s = out.line.len() as u32;
        strip(&mut out.line, 1.0, 0.0, None);
        out.line_strips.push((s, out.line.len() as u32 - s));
        if let Some((mw, mh, aspect)) = mask_px {
            out.polys.push(
                (0..lp.len)
                    .map(|k| {
                        let p = vp[lp.start + k];
                        [(p[0] / aspect * 0.5 + 0.5) * mw as f32, (0.5 - p[1] * 0.5) * mh as f32]
                    })
                    .collect(),
            );
        }
    }
    out
}

impl Model for Corales {
    fn id(&self) -> &'static str {
        "corals"
    }

    fn name(&self) -> &'static str {
        "Corales"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn is_3d(&self) -> bool {
        true
    }

    fn surfaces(&self) -> &'static [&'static str] {
        SURFACES
    }

    fn set_surface(&mut self, part: &str, spec: Option<SurfaceSpec>) {
        if part == SURFACES[0] {
            self.surface.set(spec);
        }
    }

    fn render(&mut self, ctx: &Ctx, floats: &[f32], target: &wgpu::TextureView, format: wgpu::TextureFormat) {
        let own = &floats[off(B_OWN)..off(B_OWN) + FLOATS.len()];
        let aspect = ctx.aspect().max(0.01);
        let time = if ctx.time.is_finite() { ctx.time } else { 0.0 };
        let cam = kit::Cam2d::from_floats(&floats[off(B_CAM)..off(B_CAM) + kit::Cam2d::N]);
        let look = kit::Look::from_floats(&floats[off(B_LOOK)..off(B_LOOK) + kit::Look::N]);
        let slot = |blk: usize| ColorSlotGpu::from_floats(&floats[off(blk)..off(blk) + 12]);
        let view = View::new(&cam, time);
        let mode = clean(own, F_MODO).round() as u32;

        self.surface.sync(ctx.device, ctx.queue);
        let sim = self.driver.update(time, (ctx.width, ctx.height), own);

        // Mask size: bounded, aspect-true.
        let mh = ctx.height.clamp(1, 288);
        let mw = ((mh as f32 * aspect).round() as u32).clamp(1, 1024);
        let strokes = build_strokes(sim, own, &slot(B_TRAZO), &view, time, (mode > 0).then_some((mw as usize, mh as usize, aspect)));

        if self.mask.as_ref().map_or(true, |m| m.size != (mw, mh)) {
            self.mask = Some(Mask::new(ctx.device, mw, mh));
        }
        let mask = self.mask.as_ref().expect("built above");
        let bytes = if mode > 0 { rasterize(&strokes.polys, mw as usize, mh as usize) } else { vec![0u8; (mw * mh) as usize] };
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &mask.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(mw), rows_per_image: Some(mh) },
            wgpu::Extent3d { width: mw, height: mh, depth_or_array_layers: 1 },
        );
        let sampler = self.mask_sampler.get_or_insert_with(|| {
            ctx.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("corales mask"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            })
        });

        let pic = self.surface.slot();
        let setting = self.surface.uniform.a;
        let u = FillUniform {
            fill: slot(B_RELLENO),
            background: slot(B_FONDO),
            a: [aspect, time, if ctx.fondo_alpha { 1.0 } else { 0.0 }, look.background_level],
            b: [mode as f32, clean(own, F_RALFA), clean(own, F_RESC) * setting[2].max(0.01), clean(own, F_RGIRO)],
            c: [look.shine, look.contrast, look.opacity, look.tone],
            d: [look.saturation, if pic.is_fallback() { 0.0 } else { 1.0 }, setting[0], 1.0 / mw as f32],
            e: [1.0 / mh as f32, 0.0, 0.0, 0.0],
        };
        self.fill.draw_textured(ctx, target, format, &u, &[(&mask.view, sampler), (pic.view(), pic.sampler())]);

        // Strokes over the fill.
        let pipe = self.lines.get(format, || LinePipe::new(ctx.device, format));
        let uniform = LineUniform {
            look: [look.shine, look.contrast, look.tone, look.saturation],
            misc: [aspect, look.opacity, 0.0, 0.0],
        };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("corales line uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("corales line"),
            layout: &pipe.layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });
        let mut verts = strokes.halo.clone();
        let halo_len = verts.len() as u32;
        verts.extend_from_slice(&strokes.line);
        if verts.is_empty() {
            return;
        }
        if self.vbuf.as_ref().map_or(true, |(_, last)| *last != verts) {
            let buf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("corales vertices"),
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::VERTEX,
            });
            self.vbuf = Some((buf, verts.clone()));
        }
        let vbuf = &self.vbuf.as_ref().expect("set above").0;
        let mut enc = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("corales lines") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("corales lines"),
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
            pass.set_pipeline(&pipe.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.set_vertex_buffer(0, vbuf.slice(..));
            for &(s, l) in &strokes.halo_strips {
                if l >= 4 {
                    pass.draw(s..s + l, 0..1);
                }
            }
            for &(s, l) in &strokes.line_strips {
                if l >= 4 {
                    pass.draw(halo_len + s..halo_len + s + l, 0..1);
                }
            }
        }
        ctx.queue.submit(Some(enc.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
    const W: u32 = 96;
    const H: u32 = 64;
    /// The clock a test walks: enough for the curve to have grown.
    const TIMES: [f32; 4] = [0.0, 0.5, 1.0, 1.5];

    fn frame_at(gpu: &crate::gpu::Gpu, model: &mut Corales, floats: &[f32], time: f32) -> Vec<u8> {
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, W, H, FORMAT);
        let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time, width: W, height: H, fondo_alpha: false };
        model.render(&ctx, floats, &view, FORMAT);
        crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, W, H)
    }

    /// Frames at every step of [`TIMES`], concatenated, from a fresh model.
    fn frames(gpu: &crate::gpu::Gpu, floats: &[f32]) -> Vec<u8> {
        let mut m = Corales::new();
        // Small steps so the carry matters like it does live.
        let mut out = Vec::new();
        let mut t = 0.0f32;
        for &end in &TIMES {
            while t + 0.1 < end {
                t += 0.1;
                frame_at(gpu, &mut m, floats, t);
            }
            t = end;
            out.extend(frame_at(gpu, &mut m, floats, end));
        }
        out
    }

    fn base() -> Vec<f32> {
        let mut f = SCHEMA.defaults();
        for v in &mut f[off(B_FONDO)..off(B_FONDO) + 12] {
            *v = 0.3;
        }
        // Fill floats only show with a fill: source mode, procedural stand-in.
        f[F_MODO] = 2.0;
        // A small budget the curve reaches inside the test clock, so raising
        // it is visible.
        f[F_NODOS] = 300.0;
        f
    }

    fn row(p: &[(usize, f32)]) -> Vec<f32> {
        let mut f = SCHEMA.defaults();
        for &(i, v) in p {
            f[i] = v;
        }
        f
    }

    fn sim_row(p: &[(usize, f32)]) -> Vec<f32> {
        row(p)
    }

    #[test]
    fn widths_and_indices_come_from_the_blocks() {
        assert_eq!(SCHEMA.defaults().len(), SCHEMA.width());
        assert_eq!(off(B_OWN), 0);
        assert_eq!(off(B_TRAZO), FLOATS.len());
        assert_eq!(off(B_CAM) + kit::Cam2d::N, off(B_LOOK));
        let n = |i: usize| FLOATS[i].name;
        assert_eq!((n(F_FORMA), n(F_NODOS), n(F_REINICIO), n(F_PUNTA), n(F_RGIRO)), ("seed_shape", "nodes_max", "reset_rhythm", "tip", "fill_turn"));
        assert_eq!((n(F_ANCHO), n(F_MODO), n(F_UMBRAL), n(F_SACUDIDA)), ("stroke_width", "fill_mode", "split_threshold", "shake"));
        assert_eq!(FLOATS.len(), F_RGIRO + 1);
        assert_eq!(Corales::new().surfaces(), &["fill"]);
    }

    #[test]
    fn the_grid_finds_exactly_the_brute_force_neighbours() {
        let mut rng = Rng(7);
        let pts: Vec<[f32; 2]> = (0..600).map(|_| [rng.signed() * 1.5, rng.signed()]).collect();
        for &r in &[0.03f32, 0.1, 0.25] {
            let g = Grid::build(&pts, r);
            for i in (0..pts.len()).step_by(7) {
                let mut hit: Vec<usize> = Vec::new();
                g.query(pts[i], r, |j, _| {
                    let d = (pts[i][0] - pts[j][0]).hypot(pts[i][1] - pts[j][1]);
                    if d < r {
                        hit.push(j);
                    }
                    true
                });
                hit.sort_unstable();
                let want: Vec<usize> =
                    (0..pts.len()).filter(|&j| (pts[i][0] - pts[j][0]).hypot(pts[i][1] - pts[j][1]) < r).collect();
                assert_eq!(hit, want, "r {r} node {i}");
            }
        }
        // Empty and single-point sets do not panic.
        Grid::build(&[], 0.1).query([0.0, 0.0], 0.1, |_, _| panic!("empty"));
        // The visitor can stop the walk.
        let mut seen = 0;
        Grid::build(&pts, 0.3).query([0.0, 0.0], 5.0, |_, _| {
            seen += 1;
            seen < 10
        });
        assert_eq!(seen, 10);
        let one = [[0.5f32, 0.5]];
        let mut c = 0;
        Grid::build(&one, 0.1).query([0.5, 0.5], 0.1, |_, _| {
            c += 1;
            true
        });
        assert_eq!(c, 1);
    }

    fn quiet(threshold: f32) -> Params {
        let mut p = Params::from_row(&SCHEMA.defaults());
        p.threshold = threshold;
        p.rep = 0.0;
        p.atrac = 0.0;
        p.tension = 0.0;
        p.noise = 0.0;
        p.align = 0.0;
        p.collapse = 0.0;
        p
    }

    fn line_sim(xs: &[f32]) -> Sim {
        let nodes: Vec<Node> = xs.iter().map(|&x| Node { p: [x, 0.0], born: 0 }).collect();
        let n = nodes.len();
        let mut s = Sim::new(2, 1, 0.02);
        s.nodes = nodes;
        s.loops = vec![Loop { start: 0, len: n, closed: false }];
        s.nbr = vec![[NONE; 2]; n];
        s.lid = vec![0; n];
        s.loc = vec![0; n];
        s.turn = vec![0.0; n];
        s.disp = vec![[0.0; 2]; n];
        s
    }

    #[test]
    fn a_long_edge_splits_at_its_midpoint_and_a_short_one_does_not() {
        let mut p = quiet(0.1);
        p.prob = 1.0;
        p.curl = 0.0;
        p.cap = 100;
        let mut s = line_sim(&[0.0, 0.3, 0.35, 0.5]);
        s.step(&p);
        let xs: Vec<f32> = s.nodes.iter().map(|n| n.p[0]).collect();
        // 0.3 and 0.35 are 0.05 apart (< 0.1): kept. The others split.
        assert_eq!(xs.len(), 6, "{xs:?}");
        assert!((xs[1] - 0.15).abs() < 1e-5 && (xs[4] - 0.425).abs() < 1e-5, "{xs:?}");
        assert_eq!(s.loops[0].len, 6);
        // With probability 0 nothing splits.
        let mut q = quiet(0.1);
        q.prob = 0.0;
        q.cap = 100;
        let mut s = line_sim(&[0.0, 0.3, 0.6]);
        s.step(&q);
        assert_eq!(s.nodes.len(), 3);
    }

    #[test]
    fn short_edges_collapse_but_a_loop_keeps_its_minimum() {
        let mut p = quiet(0.1);
        p.collapse = 1.0; // collapse below 0.08
        p.prob = 0.0;
        p.cap = 100;
        let xs: Vec<f32> = (0..40).map(|k| k as f32 * 0.01).collect();
        let mut s = line_sim(&xs);
        for _ in 0..40 {
            s.step(&p);
        }
        assert!(s.nodes.len() < 40, "nothing collapsed");
        assert!(s.nodes.len() >= 3, "an open curve keeps at least 3 nodes, has {}", s.nodes.len());
        // A closed circle never drops below 6.
        let mut c = Sim::new(0, 3, 0.2);
        let q = Params { collapse: 1.0, prob: 0.0, ..quiet(0.5) };
        for _ in 0..200 {
            c.step(&q);
        }
        assert!(c.nodes.len() >= 6);
    }

    #[test]
    fn the_cap_bounds_the_count_and_the_curve_keeps_its_loops() {
        let mut p = Params::from_row(&SCHEMA.defaults());
        p.cap = 400;
        p.prob = 1.0;
        let mut s = Sim::new(1, 5, p.threshold);
        for _ in 0..300 {
            s.step(&p);
            let sum: usize = s.loops.iter().map(|l| l.len).sum();
            assert_eq!(sum, s.nodes.len());
            assert!(s.nodes.len() <= p.cap.max(SEED_MAX), "{}", s.nodes.len());
            assert!(s.nodes.iter().all(|n| n.p[0].is_finite() && n.p[1].is_finite() && n.p[0].abs() <= BOX && n.p[1].abs() <= BOX));
        }
        assert_eq!(s.loops.len(), 2);
        assert!(s.nodes.len() > 300, "it did grow ({})", s.nodes.len());
    }

    #[test]
    fn the_threaded_force_pass_matches_the_single_threaded_one() {
        let mut p = Params::from_row(&SCHEMA.defaults());
        p.prob = 1.0;
        p.cap = 4000;
        let mut a = Sim::new(1, 9, p.threshold);
        for _ in 0..120 {
            a.step(&p);
        }
        assert!(a.nodes.len() > 300, "too few nodes to split meaningfully ({})", a.nodes.len());
        a.compute_turns();
        let grid = Grid::build(&a.pts, p.radius);
        let mut single = vec![[0.0f32; 2]; a.nodes.len()];
        a.forces(&p, &grid, 0, &mut single);
        let mut halves = vec![[0.0f32; 2]; a.nodes.len()];
        let mid = halves.len() / 2;
        let (x, y) = halves.split_at_mut(mid);
        a.forces(&p, &grid, 0, x);
        a.forces(&p, &grid, mid, y);
        assert_eq!(single, halves);
    }

    #[test]
    fn rasterize_is_even_odd() {
        let outer = vec![[2.0, 2.0], [18.0, 2.0], [18.0, 18.0], [2.0, 18.0]];
        let inner = vec![[8.0, 8.0], [12.0, 8.0], [12.0, 12.0], [8.0, 12.0]];
        let m = rasterize(&[outer, inner], 20, 20);
        let at = |x: usize, y: usize| m[y * 20 + x];
        assert_eq!(at(4, 4), 255);
        assert_eq!(at(10, 10), 0, "the hole");
        assert_eq!(at(0, 0), 0);
        assert_eq!(at(19, 19), 0);
        assert_eq!(m.iter().filter(|&&v| v == 255).count(), 16 * 16 - 4 * 4);
    }

    #[test]
    fn the_driver_holds_still_without_time_and_is_deterministic() {
        let f = SCHEMA.defaults();
        let own = &f[..FLOATS.len()];
        let run = || {
            let mut d = Driver::default();
            let mut counts = Vec::new();
            for k in 0..30 {
                let s = d.update(k as f32 * 0.05, (W, H), own);
                counts.push((s.nodes.len(), s.iter, s.nodes[s.nodes.len() / 2].p));
            }
            counts
        };
        assert_eq!(run(), run());
        let mut d = Driver::default();
        d.update(0.0, (W, H), own);
        d.update(0.3, (W, H), own);
        let a = d.sim.as_ref().map(|s| (s.iter, s.nodes.len(), s.nodes[0].p)).unwrap();
        d.update(0.3, (W, H), own);
        d.update(0.3, (W, H), own);
        assert_eq!(a, d.sim.as_ref().map(|s| (s.iter, s.nodes.len(), s.nodes[0].p)).unwrap());
        // A rewind and a resize restart it.
        d.update(1.0, (W, H), own);
        let grown = d.sim.as_ref().unwrap().iter;
        d.update(0.2, (W, H), own);
        assert!(d.sim.as_ref().unwrap().iter < grown, "rewind restarts");
        d.update(0.25, (W, H), own);
        d.update(0.3, (W + 8, H), own);
        assert_eq!(d.sim.as_ref().unwrap().iter, BURN_IN, "resize restarts");
    }

    #[test]
    fn a_full_curve_restarts_instead_of_stalling_and_a_drop_restarts_it() {
        let f = sim_row(&[(F_NODOS, 200.0), (F_PROB, 1.0)]);
        let own = &f[..FLOATS.len()];
        let mut d = Driver::default();
        let mut t = 0.0;
        let mut v0 = 0;
        let mut restarted = false;
        for _ in 0..400 {
            d.update(t, (W, H), own);
            t += 0.1;
            if d.variant > v0 {
                restarted = true;
                v0 = d.variant;
            }
        }
        assert!(restarted, "a full curve must restart within FULL_HOLD");
        // Rising edge of reinicio_ritmo.
        let mut hi = f.clone();
        hi[F_REINICIO] = 1.0;
        let v = d.variant;
        d.update(t, (W, H), &f[..FLOATS.len()]);
        d.update(t + 0.05, (W, H), &hi[..FLOATS.len()]);
        assert_eq!(d.variant, v + 1);
        d.update(t + 0.1, (W, H), &hi[..FLOATS.len()]);
        assert_eq!(d.variant, v + 1, "held high is not a new edge");
    }

    #[test]
    fn extreme_floats_stay_finite_and_bounded_over_many_frames() {
        for pick in 0..4 {
            let mut f = SCHEMA.defaults();
            for (i, s) in FLOATS.iter().enumerate() {
                f[i] = match pick {
                    0 => s.min,
                    1 => s.max,
                    2 => if i % 2 == 0 { s.min } else { s.max },
                    _ => if i % 2 == 0 { s.max } else { s.min },
                };
            }
            f[F_REINICIO] = 0.0;
            let own = f[..FLOATS.len()].to_vec();
            let mut d = Driver::default();
            let mut t = 0.0;
            for _ in 0..90 {
                let s = d.update(t, (W, H), &own);
                t += 1.0 / 60.0;
                assert!(s.nodes.len() <= HARD_CAP, "pick {pick}: {} nodes", s.nodes.len());
                let cap = Params::from_row(&own).cap;
                assert!(s.nodes.len() <= cap.max(SEED_MAX), "pick {pick}: {} > cap {cap}", s.nodes.len());
                assert!(
                    s.nodes.iter().all(|n| n.p[0].is_finite() && n.p[1].is_finite() && n.p[0].abs() <= BOX && n.p[1].abs() <= BOX),
                    "pick {pick} left the box or went non-finite"
                );
            }
        }
        // Out-of-range and non-finite values read as their defaults/clamps.
        let mut f = SCHEMA.defaults();
        for v in f[..FLOATS.len()].iter_mut() {
            *v = f32::NAN;
        }
        let mut d = Driver::default();
        for k in 0..20 {
            let s = d.update(k as f32 * 0.05, (W, H), &f[..FLOATS.len()]);
            assert!(s.nodes.iter().all(|n| n.p[0].is_finite() && n.p[1].is_finite()));
        }
        let big = vec![1e30f32; FLOATS.len()];
        let mut d = Driver::default();
        for k in 0..20 {
            let s = d.update(k as f32 * 0.05, (W, H), &big);
            assert!(s.nodes.len() <= HARD_CAP);
        }
    }

    fn distinct(px: &[u8]) -> usize {
        let mut s = std::collections::HashSet::new();
        for p in px.chunks_exact(4) {
            s.insert([p[0], p[1], p[2]]);
        }
        s.len()
    }

    #[test]
    fn the_defaults_are_not_flat_and_deterministic() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = SCHEMA.defaults();
        let a = frames(&gpu, &f);
        assert_eq!(a, frames(&gpu, &f));
        let sz = (W * H * 4) as usize;
        assert!(distinct(&a[..sz]) > 8, "first frame is flat ({} colours)", distinct(&a[..sz]));
        assert!(distinct(&a[sz * 3..]) > 8);
        assert_ne!(&a[..sz], &a[sz * 3..], "the curve must grow with time");
    }

    #[test]
    fn a_picture_on_the_relleno_surface_changes_the_fill() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = base();
        let plain = frames(&gpu, &f);
        let mut m = Corales::new();
        let px: Vec<u8> = (0..16).flat_map(|i| if i % 2 == 0 { [250, 20, 20, 255] } else { [20, 20, 250, 255] }).collect();
        m.set_surface("fill", Some(crate::model::SurfaceSpec::image(&px, 4, 4, 1.0, crate::model::Mapping::Projector, 1.0)));
        let mut t = 0.0f32;
        let mut last = Vec::new();
        for &end in &TIMES {
            while t + 0.1 < end {
                t += 0.1;
                frame_at(&gpu, &mut m, &f, t);
            }
            t = end;
            last = frame_at(&gpu, &mut m, &f, end);
        }
        assert_ne!(last, plain[plain.len() - (W * H * 4) as usize..]);
        m.set_surface("fill", None);
        m.set_surface("nope", None);
    }

    #[test]
    fn every_float_moves_the_frame() {
        let gpu = crate::gpu::Gpu::new_headless();
        let (cam, look) = (off(B_CAM), off(B_LOOK));
        let mut idx: Vec<(usize, FloatSpec)> = FLOATS.iter().enumerate().map(|(i, s)| (i, *s)).collect();
        idx.extend(kit::CAM2D.iter().enumerate().map(|(i, s)| (cam + i, *s)));
        idx.extend(kit::LOOK.iter().enumerate().map(|(i, s)| (look + i, *s)));
        let b = base();
        let reference = frames(&gpu, &b);
        for (i, spec) in idx {
            let far = if (spec.max - b[i]).abs() > (b[i] - spec.min).abs() { spec.max } else { spec.min };
            let far = if spec.name == "tone" { 0.5 } else { far };
            let mut f = b.clone();
            f[i] = far;
            assert!(reference != frames(&gpu, &f), "float `{}` ({} -> {far}) did not move the frame", spec.name, b[i]);
        }
    }

    #[test]
    fn solid_fill_and_each_seed_shape_draw() {
        let gpu = crate::gpu::Gpu::new_headless();
        let none = frames(&gpu, &SCHEMA.defaults());
        let solid = frames(&gpu, &row(&[(F_MODO, 1.0)]));
        assert_ne!(none, solid);
        let mut seen = Vec::new();
        for shape in 0..4 {
            let px = frames(&gpu, &row(&[(F_FORMA, shape as f32)]));
            assert!(distinct(&px[..(W * H * 4) as usize]) > 8, "shape {shape} is flat");
            assert!(!seen.contains(&px), "shape {shape} repeats another");
            seen.push(px);
        }
    }

    #[test]
    #[ignore = "measurement, not a gate: cargo test -p metal-render corales -- --ignored --nocapture"]
    fn measure_default_cost() {
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (1280u32, 720u32);
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        for (label, cap, mode) in [("default", 5000.0, 0.0), ("full 20k", 20000.0, 0.0), ("solid fill", 5000.0, 1.0)] {
            let mut f = row(&[(F_NODOS, cap), (F_MODO, mode)]);
            f[F_PUNTA] = 0.5;
            let mut m = Corales::new();
            let mut run = |n: u32, t0: u32| {
                for i in 0..n {
                    let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: (t0 + i) as f32 / 60.0, width: w, height: h, fondo_alpha: false };
                    m.render(&ctx, &f, &view, FORMAT);
                }
                let _ = gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
            };
            run(600, 0); // grow to the cap
            let t = std::time::Instant::now();
            run(60, 600);
            let ms = t.elapsed().as_secs_f32() * 1000.0 / 60.0;
            let n = m.driver.sim.as_ref().map_or(0, |s| s.nodes.len());
            println!("corales {label} {w}x{h}: {ms:.2} ms/frame at {n} nodes");
        }
        let _ = crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, w, h);
        // The CPU halves alone: the simulation clock, then the stroke build.
        let f = SCHEMA.defaults();
        let own = &f[..FLOATS.len()];
        let mut d = Driver::default();
        let mut t = 0.0f32;
        for _ in 0..300 {
            d.update(t, (w, h), own);
            t += 1.0 / 60.0;
        }
        let tm = std::time::Instant::now();
        for _ in 0..60 {
            d.update(t, (w, h), own);
            t += 1.0 / 60.0;
        }
        let sim_ms = tm.elapsed().as_secs_f32() * 1000.0 / 60.0;
        let sim = d.sim.as_ref().expect("seeded");
        let view = View::new(&kit::Cam2d::from_floats(&f[off(B_CAM)..off(B_CAM) + kit::Cam2d::N]), 0.0);
        let slot = ColorSlotGpu::from_floats(&f[off(B_TRAZO)..off(B_TRAZO) + 12]);
        let tm = std::time::Instant::now();
        for _ in 0..60 {
            build_strokes(sim, own, &slot, &view, 1.0, None);
        }
        let draw_ms = tm.elapsed().as_secs_f32() * 1000.0 / 60.0;
        println!("cpu: sim {sim_ms:.2} ms/frame, stroke build {draw_ms:.2} ms ({} nodes)", sim.nodes.len());
    }
}
