//! Fineline — a 3D fine-line plotter drawing.
//!
//! After abey79/lines (`ln`: a 3D scene projected to line segments, silhouette
//! skins, hidden-line masking) and abey79/vpype (the line-collection post
//! operations: linemerge, linesimplify, crop, layout). Written from the
//! algorithms; no code was taken from either.
//!
//! **Pipeline, all on the CPU, a pure function of (floats, time), composed on a
//! worker thread one frame behind and cached by stage (see `Composer`):**
//! 1. A scene of `n_objetos` analytic shapes. Each is a welded grid mesh of one
//!    morphing surface `P(u, v)`: sphere, cube (superquadric), torus, cone,
//!    trefoil tube. `forma_a`/`forma_b` index that list (fractions blend two
//!    neighbours), `forma_mezcla` blends A into B, `forma_variedad` offsets the
//!    index per object. The object meshes are cached until a structural float
//!    moves; the camera, spin and light never rebuild them.
//! 2. Strokes: the silhouette (mesh edges between a front and a back face, and
//!    open boundaries), tone-driven hatching (iso-lines of a linear field in
//!    object space, kept where the surface is dark enough, plus a cross layer),
//!    and a ground-plane grid.
//! 3. Hidden lines: a small software depth buffer of every triangle; each
//!    stroke vertex is tested against it. `hide` 1 removes hidden ink, 0
//!    keeps it all, in between it fades.
//! 4. vpype-style post on the projected polylines: `linemerge` (chaining
//!    segments), `linesimplify` (Douglas-Peucker, `simplifica`), `crop` to a
//!    page window with a margin frame (`corte`), dashing (`dotted`) and a
//!    hand wobble that boils at 8 Hz (`trazo_ruido`).
//! 5. Thin triangle strips with per-vertex ink and pixel-true width. The output
//!    is capped at [`MAX_SEGMENTS`] segments.
//!
//! `measure_default_cost` (ignored test) prints ms per frame.

use std::collections::{HashMap, HashSet};
use std::f32::consts::{PI, TAU};
use std::sync::{mpsc, Arc};

use wgpu::util::DeviceExt;

use crate::model::{color_at, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::kit::{self, CAM3D_BLOCK, LOOK_BLOCK};
use crate::shared::{build_shader, ColorSlotGpu};

/// Hard cap on drawn segments per frame (a segment = two consecutive points of
/// one stroke).
pub const MAX_SEGMENTS: usize = 60_000;

const FLOATS: &[FloatSpec] = &[
    // The scene. Structural floats are held (slow drive): sweeping them at beat
    // rate would rebuild the meshes and destroy the picture's identity.
    spec_held("shape_a", 0.0, 4.0, 0.0, 0.25),
    spec_held("shape_b", 0.0, 4.0, 1.0, 0.25),
    spec("shape_mix", 0.0, 1.0, 0.15),
    spec_held("shape_variety", 0.0, 1.0, 1.0, 0.25),
    spec_held("n_objects", 1.0, 9.0, 5.0, 0.25),
    spec_held("dispersion", 0.0, 1.0, 0.8, 0.25),
    spec("turn_object", -2.0, 2.0, 0.25),
    // Skin.
    spec("hatch_density", 0.0, 1.0, 0.55),
    spec("hatch_angle", 0.0, 3.1416, 0.6),
    spec("hatch_cross", 0.0, 1.0, 0.5),
    spec("light_turn", -3.1416, 3.1416, 0.8),
    spec("silhouette_thickness", 0.0, 6.0, 1.7),
    spec("stroke_thickness", 0.2, 4.0, 0.9),
    // vpype-style post.
    spec("stroke_noise", 0.0, 1.0, 0.15),
    spec("simplify", 0.0, 1.0, 0.3),
    spec("cut", 0.0, 1.0, 0.06),
    spec("dotted", 0.0, 1.0, 0.0),
    spec("hide", 0.0, 1.0, 1.0),
    spec("plane", 0.0, 1.0, 0.5),
];

const F_A: usize = 0;
const F_B: usize = 1;
const F_MEZCLA: usize = 2;
const F_VARIEDAD: usize = 3;
const F_N: usize = 4;
const F_DISP: usize = 5;
const F_GIRO: usize = 6;
const F_HDENS: usize = 7;
const F_HANG: usize = 8;
const F_HCRUZ: usize = 9;
const F_LUZ: usize = 10;
const F_SIL: usize = 11;
const F_TRAZO: usize = 12;
const F_RUIDO: usize = 13;
const F_SIMP: usize = 14;
const F_CORTE: usize = 15;
const F_PUNT: usize = 16;
const F_OCULTAR: usize = 17;
const F_PLANO: usize = 18;

/// Blue-black ink: pale and washed far away, dark near. Walked by depth.
const TRAZO: [f32; 12] = [
    0.36, 0.46, 0.74, //
    0.12, 0.22, 0.58, //
    0.05, 0.09, 0.32, //
    0.02, 0.03, 0.14,
];
/// Cream paper, a shade darker towards the bottom.
const FONDO: [f32; 12] = [
    0.96, 0.94, 0.88, //
    0.95, 0.93, 0.86, //
    0.92, 0.90, 0.83, //
    0.89, 0.87, 0.80,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("stroke", &TRAZO),
        color_at("background", &FONDO),
        CAM3D_BLOCK,
        LOOK_BLOCK,
        crate::model::SIGNAL_FLOATS_BLOCK,
        crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

const B_OWN: usize = 0;
const B_TRAZO: usize = 1;
const B_FONDO: usize = 2;
const B_CAM: usize = 3;
const B_LOOK: usize = 4;

fn off(block: usize) -> usize {
    kit::offset_of(SCHEMA.blocks, block)
}

fn clean(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        default
    }
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

/// Number of analytic shapes in the morph list.
pub const SHAPES: usize = 5;

fn sgn_pow(x: f32, e: f32) -> f32 {
    // f32 trig noise (1e-8) raised to a small power is 0.02: snap it to zero.
    if x.abs() < 1e-5 {
        return 0.0;
    }
    x.signum() * x.abs().powf(e)
}

fn knot_centre(t: f64) -> glam::DVec3 {
    let r = 2.0 + (3.0 * t).cos();
    glam::DVec3::new(r * (2.0 * t).cos(), (3.0 * t).sin(), r * (2.0 * t).sin()) * 0.30
}

/// Shape `k` at surface coordinates `(u, v)` in `[0, 1]^2`. `u` runs round the
/// vertical axis, `v` from the bottom (or inside) up and over. Every shape
/// fits roughly in the unit ball and is wound so `∂u × ∂v` points outward.
pub fn shape_point(k: usize, u: f32, v: f32) -> Vec3 {
    let th = -TAU * u;
    match k % SHAPES {
        0 => {
            let ph = PI * (v - 0.5);
            Vec3::new(ph.cos() * th.cos(), ph.sin(), ph.cos() * th.sin())
        }
        1 => {
            let ph = PI * (v - 0.5);
            let e = 0.22;
            Vec3::new(
                sgn_pow(ph.cos(), e) * sgn_pow(th.cos(), e),
                sgn_pow(ph.sin(), e),
                sgn_pow(ph.cos(), e) * sgn_pow(th.sin(), e),
            ) * 0.82
        }
        2 => {
            let psi = TAU * v;
            let (big, small) = (0.68, 0.3);
            let ring = big + small * psi.cos();
            Vec3::new(ring * th.cos(), small * psi.sin(), ring * th.sin())
        }
        3 => {
            let (rad, y) = if v < 0.2 {
                (0.8 * v / 0.2, -0.7)
            } else {
                let t = (v - 0.2) / 0.8;
                (0.8 * (1.0 - t), -0.7 + 1.6 * t)
            };
            Vec3::new(rad * th.cos(), y, rad * th.sin())
        }
        _ => {
            // f64: the Frenet frame comes from a second difference, which f32
            // noise would turn into a seam where u = 0 and u = 1 disagree.
            let t = std::f64::consts::TAU * u as f64;
            let h = 1e-3;
            let (c0, c1, c2) = (knot_centre(t - h), knot_centre(t), knot_centre(t + h));
            let tan = (c2 - c0).normalize_or_zero();
            let acc = c2 + c0 - 2.0 * c1;
            let n = (acc - tan * acc.dot(tan)).normalize_or_zero();
            let b = tan.cross(n);
            let psi = -std::f64::consts::TAU * v as f64;
            let p = c1 + (n * psi.cos() + b * psi.sin()) * 0.2;
            Vec3::new(p.x as f32, p.y as f32, p.z as f32)
        }
    }
}

fn smooth(f: f32) -> f32 {
    f * f * (3.0 - 2.0 * f)
}

/// A continuous shape index: the integer part picks a shape, the fraction
/// blends it with the next one (the list wraps).
pub fn morph_point(s: f32, u: f32, v: f32) -> Vec3 {
    let s = s.rem_euclid(SHAPES as f32);
    let i = s.floor();
    let f = smooth(s - i);
    let a = shape_point(i as usize, u, v);
    if f < 1e-4 {
        return a;
    }
    a.lerp(shape_point(i as usize + 1, u, v), f)
}

/// A welded triangle mesh in object space.
#[derive(Clone, Debug)]
pub struct Mesh {
    pub verts: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    /// Edges as `(a, b)` with the one or two triangles on them (`u32::MAX` =
    /// none, an open boundary).
    pub edges: Vec<(u32, u32, u32, u32)>,
}

/// Builds the surface `lerp(morph(sa), morph(sb), mix)` on an `nu x nv` grid,
/// welding coincident points (poles, seams) so closed shapes are closed.
pub fn build_mesh(sa: f32, sb: f32, mix: f32, nu: usize, nv: usize) -> Mesh {
    let point = |u: f32, v: f32| {
        let a = morph_point(sa, u, v);
        if mix <= 1e-4 {
            a
        } else {
            a.lerp(morph_point(sb, u, v), mix)
        }
    };
    let mut verts: Vec<Vec3> = Vec::new();
    let mut weld: HashMap<[i32; 3], u32> = HashMap::new();
    let mut grid = vec![0u32; (nu + 1) * (nv + 1)];
    for j in 0..=nv {
        for i in 0..=nu {
            let p = point(i as f32 / nu as f32, j as f32 / nv as f32);
            let key = [(p.x * 500.0).round() as i32, (p.y * 500.0).round() as i32, (p.z * 500.0).round() as i32];
            let id = *weld.entry(key).or_insert_with(|| {
                verts.push(p);
                verts.len() as u32 - 1
            });
            grid[j * (nu + 1) + i] = id;
        }
    }
    let mut tris = Vec::new();
    for j in 0..nv {
        for i in 0..nu {
            let a = grid[j * (nu + 1) + i];
            let b = grid[j * (nu + 1) + i + 1];
            let c = grid[(j + 1) * (nu + 1) + i + 1];
            let d = grid[(j + 1) * (nu + 1) + i];
            for t in [[a, b, c], [a, c, d]] {
                if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                    tris.push(t);
                }
            }
        }
    }
    let mut map: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        for k in 0..3 {
            let (x, y) = (t[k], t[(k + 1) % 3]);
            let key = if x < y { (x, y) } else { (y, x) };
            let e = map.entry(key).or_insert((ti as u32, u32::MAX));
            if e.0 != ti as u32 {
                e.1 = ti as u32;
            }
        }
    }
    let mut edges: Vec<_> = map.into_iter().map(|((a, b), (f0, f1))| (a, b, f0, f1)).collect();
    edges.sort_unstable();
    Mesh { verts, tris, edges }
}

/// Signed volume of a mesh (positive when wound outward).
pub fn signed_volume(m: &Mesh) -> f32 {
    m.tris
        .iter()
        .map(|t| {
            let (a, b, c) = (m.verts[t[0] as usize], m.verts[t[1] as usize], m.verts[t[2] as usize]);
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

// ---------------------------------------------------------------------------
// 2D polylines and the vpype operations
// ---------------------------------------------------------------------------

/// One stroke vertex: position in aspect units (y in -1..1), ink opacity and
/// the depth-graded ink position `t` (0 far, 1 near).
#[derive(Clone, Copy, Debug)]
pub struct P2 {
    pub p: Vec2,
    pub a: f32,
    pub t: f32,
}

fn lerp_p2(a: &P2, b: &P2, s: f32) -> P2 {
    P2 { p: a.p.lerp(b.p, s), a: a.a + (b.a - a.a) * s, t: a.t + (b.t - a.t) * s }
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let l2 = ab.length_squared();
    if l2 < 1e-18 {
        return p.distance(a);
    }
    let s = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    p.distance(a + ab * s)
}

/// vpype `linesimplify`: Douglas-Peucker with tolerance `tol`. Keeps the
/// endpoints. Closed loops work because the farthest point splits them.
pub fn simplify(pts: &[P2], tol: f32) -> Vec<P2> {
    if pts.len() < 3 || tol <= 0.0 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((lo, hi)) = stack.pop() {
        if hi <= lo + 1 {
            continue;
        }
        let (mut worst, mut at) = (0.0f32, lo);
        for i in lo + 1..hi {
            let d = seg_dist(pts[i].p, pts[lo].p, pts[hi].p);
            if d > worst {
                worst = d;
                at = i;
            }
        }
        if worst > tol {
            keep[at] = true;
            stack.push((lo, at));
            stack.push((at, hi));
        }
    }
    pts.iter().zip(&keep).filter(|(_, &k)| k).map(|(p, _)| *p).collect()
}

/// vpype `crop`: clips a polyline to the box `[lo, hi]`, returning the pieces
/// that remain (Liang-Barsky per segment, pieces re-joined across segments).
pub fn crop(pts: &[P2], lo: Vec2, hi: Vec2) -> Vec<Vec<P2>> {
    let mut out: Vec<Vec<P2>> = Vec::new();
    let mut cur: Vec<P2> = Vec::new();
    for w in pts.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let d = b.p - a.p;
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        let mut inside = true;
        for (p, q) in [
            (-d.x, a.p.x - lo.x),
            (d.x, hi.x - a.p.x),
            (-d.y, a.p.y - lo.y),
            (d.y, hi.y - a.p.y),
        ] {
            if p.abs() < 1e-12 {
                if q < 0.0 {
                    inside = false;
                    break;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    t0 = t0.max(r);
                } else {
                    t1 = t1.min(r);
                }
            }
        }
        if !inside || t0 > t1 {
            if cur.len() >= 2 {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
            continue;
        }
        let pa = lerp_p2(a, b, t0);
        let pb = lerp_p2(a, b, t1);
        let continues = cur.last().map_or(false, |l| l.p.distance_squared(pa.p) < 1e-12);
        if !continues {
            if cur.len() >= 2 {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
            cur.push(pa);
        }
        cur.push(pb);
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out
}

/// Chops a polyline into dashes of `on` length every `period` of arc length.
pub fn dash(pts: &[P2], period: f32, on: f32) -> Vec<Vec<P2>> {
    if pts.len() < 2 || period <= 1e-6 || on >= period {
        return vec![pts.to_vec()];
    }
    let mut cum = vec![0.0f32];
    for w in pts.windows(2) {
        cum.push(cum.last().copied().unwrap_or(0.0) + w[0].p.distance(w[1].p));
    }
    let total = cum.last().copied().unwrap_or(0.0);
    let at = |s: f32| -> P2 {
        let i = cum.partition_point(|&c| c <= s).clamp(1, pts.len() - 1);
        let l = (cum[i] - cum[i - 1]).max(1e-9);
        lerp_p2(&pts[i - 1], &pts[i], ((s - cum[i - 1]) / l).clamp(0.0, 1.0))
    };
    let mut out = Vec::new();
    let mut k = 0usize;
    loop {
        let (a, b) = (k as f32 * period, (k as f32 * period + on).min(total));
        if a >= total - 1e-7 || k > 200_000 {
            break;
        }
        let mut piece = vec![at(a)];
        let first = cum.partition_point(|&c| c <= a);
        let last = cum.partition_point(|&c| c < b);
        for i in first..last.min(pts.len()) {
            if cum[i] > a && cum[i] < b {
                piece.push(pts[i]);
            }
        }
        piece.push(at(b));
        if piece.len() >= 2 && b > a + 1e-7 {
            out.push(piece);
        }
        k += 1;
    }
    out
}

/// Inserts points so no segment is longer than `max_len`.
pub fn resample(pts: &[P2], max_len: f32) -> Vec<P2> {
    let mut out = Vec::with_capacity(pts.len());
    for (i, p) in pts.iter().enumerate() {
        if i > 0 {
            let q = &pts[i - 1];
            let l = q.p.distance(p.p);
            let n = (l / max_len).ceil().max(1.0) as usize;
            for k in 1..n {
                out.push(lerp_p2(q, p, k as f32 / n as f32));
            }
        }
        out.push(*p);
    }
    out
}

fn key3(p: Vec3) -> [i32; 3] {
    [(p.x * 2000.0).round() as i32, (p.y * 2000.0).round() as i32, (p.z * 2000.0).round() as i32]
}

/// vpype `linemerge`: chains segments that share endpoints into polylines.
/// Duplicate segments are dropped first.
pub fn linemerge(segs: &[(Vec3, Vec3)]) -> Vec<Vec<Vec3>> {
    let mut seen: HashSet<([i32; 3], [i32; 3])> = HashSet::new();
    let mut list: Vec<(Vec3, Vec3)> = Vec::with_capacity(segs.len());
    for &(a, b) in segs {
        let (ka, kb) = (key3(a), key3(b));
        if ka == kb {
            continue;
        }
        let k = if ka < kb { (ka, kb) } else { (kb, ka) };
        if seen.insert(k) {
            list.push((a, b));
        }
    }
    let mut at: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
    for (i, (a, b)) in list.iter().enumerate() {
        at.entry(key3(*a)).or_default().push(i as u32);
        at.entry(key3(*b)).or_default().push(i as u32);
    }
    let mut used = vec![false; list.len()];
    let mut out = Vec::new();
    // Prefer to start at chain ends (valence 1) so open strokes are not split.
    let mut order: Vec<usize> = (0..list.len()).collect();
    order.sort_by_key(|&i| {
        let (a, b) = list[i];
        let v = |p: Vec3| at.get(&key3(p)).map_or(0, |v| v.len());
        (v(a) != 1 && v(b) != 1) as u8
    });
    for start in order {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut chain = std::collections::VecDeque::new();
        chain.push_back(list[start].0);
        chain.push_back(list[start].1);
        for forward in [true, false] {
            loop {
                let end = if forward { *chain.back().expect("non-empty") } else { *chain.front().expect("non-empty") };
                let next = at.get(&key3(end)).and_then(|v| v.iter().copied().find(|&i| !used[i as usize]));
                let Some(i) = next else { break };
                used[i as usize] = true;
                let (a, b) = list[i as usize];
                let other = if key3(a) == key3(end) { b } else { a };
                if forward {
                    chain.push_back(other);
                } else {
                    chain.push_front(other);
                }
            }
        }
        out.push(chain.into_iter().collect());
    }
    out
}

/// Total segments in a set of strokes.
pub fn segment_count(runs: &[Run]) -> usize {
    runs.iter().map(|r| r.pts.len().saturating_sub(1)).sum()
}

/// One finished stroke. `kind`: 0 silhouette, 1 hatch, 2 ground, 3 frame.
#[derive(Clone, Debug)]
pub struct Run {
    pub pts: Vec<P2>,
    pub kind: u8,
}

// ---------------------------------------------------------------------------
// Camera, depth buffer, scene
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct CamFrame {
    pub eye: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub fwd: Vec3,
    pub tan_half: f32,
}

const NEAR: f32 = 0.08;

/// The CAM3D kit as an eye frame looking at the origin. Deterministic in time.
pub fn camera(c: &kit::Cam3d, time: f32) -> CamFrame {
    let f = |v: f32, lo: f32, hi: f32, d: f32| clean(v, lo, hi, d);
    let az = f(c.yaw, -50.0, 50.0, 0.0) + f(c.orbit, -2.0, 2.0, 0.0) * time;
    let distance = (f(c.distance, 0.5, 20.0, 4.0) + f(c.dolly, -5.0, 5.0, 0.0)).max(0.25);
    let (sp, cp) = f(c.pitch, -1.5, 1.5, 0.2).sin_cos();
    let shake = f(c.shake, 0.0, 1.0, 0.0) * 0.06;
    let tremor = Vec3::new(
        (time * 47.0).sin() + (time * 31.0 + 1.0).sin(),
        (time * 61.0 + 2.0).sin() + (time * 37.0).sin(),
        (time * 53.0 + 4.0).sin(),
    ) * shake;
    let eye = Vec3::new(distance * cp * az.sin(), distance * sp + f(c.alt, -5.0, 5.0, 0.0), distance * cp * az.cos()) + tremor;
    let fwd = (-eye).try_normalize().unwrap_or(Vec3::NEG_Z);
    let right0 = fwd.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
    let up0 = right0.cross(fwd);
    let (sr, cr) = f(c.roll, -50.0, 50.0, 0.0).sin_cos();
    CamFrame {
        eye,
        right: right0 * cr + up0 * sr,
        up: up0 * cr - right0 * sr,
        fwd,
        tan_half: (f(c.fov, 0.3, 2.2, 1.0) * 0.5).tan(),
    }
}

impl CamFrame {
    /// Aspect-unit screen position and view depth, or `None` behind the near
    /// plane.
    pub fn project(&self, p: Vec3) -> Option<(Vec2, f32)> {
        let rel = p - self.eye;
        let z = rel.dot(self.fwd);
        if z < NEAR {
            return None;
        }
        let k = 1.0 / (z * self.tan_half);
        Some((Vec2::new(rel.dot(self.right) * k, rel.dot(self.up) * k), z))
    }
}

struct ZBuf {
    w: usize,
    h: usize,
    aspect: f32,
    d: Vec<f32>,
}

impl ZBuf {
    fn new(aspect: f32) -> ZBuf {
        let h = 176usize;
        let w = ((h as f32 * aspect).round() as usize).clamp(32, 640);
        ZBuf { w, h, aspect, d: vec![f32::INFINITY; w * h] }
    }

    fn to_px(&self, p: Vec2) -> Vec2 {
        Vec2::new((p.x / self.aspect * 0.5 + 0.5) * self.w as f32, (0.5 - p.y * 0.5) * self.h as f32)
    }

    fn tri(&mut self, p: [Vec2; 3], z: [f32; 3]) {
        let q = [self.to_px(p[0]), self.to_px(p[1]), self.to_px(p[2])];
        let area = (q[1] - q[0]).perp_dot(q[2] - q[0]);
        if area.abs() < 1e-6 {
            return;
        }
        let minx = q.iter().map(|v| v.x).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize;
        let maxx = (q.iter().map(|v| v.x).fold(f32::NEG_INFINITY, f32::max).ceil().min(self.w as f32 - 1.0)).max(-1.0);
        let miny = q.iter().map(|v| v.y).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize;
        let maxy = (q.iter().map(|v| v.y).fold(f32::NEG_INFINITY, f32::max).ceil().min(self.h as f32 - 1.0)).max(-1.0);
        if maxx < 0.0 || maxy < 0.0 {
            return;
        }
        let iz = [1.0 / z[0], 1.0 / z[1], 1.0 / z[2]];
        let eps = -1e-3;
        for y in miny..=maxy as usize {
            for x in minx..=maxx as usize {
                let c = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let w0 = (q[1] - c).perp_dot(q[2] - c) / area;
                let w1 = (q[2] - c).perp_dot(q[0] - c) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < eps || w1 < eps || w2 < eps {
                    continue;
                }
                let z = 1.0 / (w0 * iz[0] + w1 * iz[1] + w2 * iz[2]).max(1e-9);
                let at = &mut self.d[y * self.w + x];
                if z < *at {
                    *at = z;
                }
            }
        }
    }

    fn hidden(&self, p: Vec2, z: f32) -> bool {
        let q = self.to_px(p);
        if q.x < 0.0 || q.y < 0.0 || q.x >= self.w as f32 || q.y >= self.h as f32 {
            return false;
        }
        let zb = self.d[q.y as usize * self.w + q.x as usize];
        z > zb * 1.012 + 0.035
    }
}

/// Scene parameters read (and clamped) from the float row.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub a: f32,
    pub b: f32,
    pub mix: f32,
    pub variety: f32,
    pub n: usize,
    pub dispersion: f32,
    pub turn: f32,
    pub hatch_dens: f32,
    pub hatch_ang: f32,
    pub hatch_cross: f32,
    pub light: f32,
    pub sil_px: f32,
    pub trazo_px: f32,
    pub noise: f32,
    pub simplify: f32,
    pub cut: f32,
    pub dotted: f32,
    pub hide: f32,
    pub plane: f32,
    /// Hatch spacing divisor: 1 at 1080p and above, below 1 on small outputs
    /// (preview tiles) so they carry fewer hatch lines. Not a float row entry.
    pub hatch_scale: f32,
}

impl Params {
    pub fn from_floats(o: &[f32]) -> Params {
        let g = |i: usize| FLOATS[i];
        let c = |i: usize| clean(o[i], g(i).min, g(i).max, g(i).default);
        Params {
            a: c(F_A),
            b: c(F_B),
            mix: c(F_MEZCLA),
            variety: c(F_VARIEDAD),
            n: c(F_N).round() as usize,
            dispersion: c(F_DISP),
            turn: c(F_GIRO),
            hatch_dens: c(F_HDENS),
            hatch_ang: c(F_HANG),
            hatch_cross: c(F_HCRUZ),
            light: c(F_LUZ),
            sil_px: c(F_SIL),
            trazo_px: c(F_TRAZO),
            noise: c(F_RUIDO),
            simplify: c(F_SIMP),
            cut: c(F_CORTE),
            dotted: c(F_PUNT),
            hide: c(F_OCULTAR),
            plane: c(F_PLANO),
            hatch_scale: 1.0,
        }
    }

    /// The hatch scale for an output `height` px tall: 1 from 1080 up, fewer
    /// lines (spacing / sqrt(h / 1080), floor 0.5) below.
    pub fn hatch_scale_for(height: u32) -> f32 {
        (height.max(1) as f32 / 1080.0).sqrt().clamp(0.5, 1.0)
    }

    /// Every field's bits, for the frame cache.
    fn push_key(&self, k: &mut Vec<u32>) {
        let f = [
            self.a, self.b, self.mix, self.variety, self.dispersion, self.turn, self.hatch_dens, self.hatch_ang, self.hatch_cross,
            self.light, self.sil_px, self.trazo_px, self.noise, self.simplify, self.cut, self.dotted, self.hide, self.plane,
            self.hatch_scale,
        ];
        k.extend(f.iter().map(|v| v.to_bits()));
        k.push(self.n as u32);
    }

    /// The floats that decide the meshes; the cache key.
    fn mesh_key(&self) -> [u32; 5] {
        [self.a.to_bits(), self.b.to_bits(), self.mix.to_bits(), self.variety.to_bits(), self.n as u32]
    }
}

fn hash(n: u32) -> f32 {
    let mut x = n.wrapping_mul(0x9E37_79B1) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297A_2D39);
    x ^= x >> 15;
    (x & 0xFFFF) as f32 / 65535.0
}

fn vnoise(p: Vec2) -> f32 {
    let (i, f) = (p.floor(), p - p.floor());
    let h = |dx: f32, dy: f32| {
        hash(((i.x + dx) as i32 as u32).wrapping_mul(73_856_093) ^ ((i.y + dy) as i32 as u32).wrapping_mul(19_349_663))
    };
    let s = Vec2::new(smooth(f.x), smooth(f.y));
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    l(l(h(0.0, 0.0), h(1.0, 0.0), s.x), l(h(0.0, 1.0), h(1.0, 1.0), s.x), s.y)
}

/// The meshes of the scene's objects, shared by the whole frame.
pub struct SceneMeshes {
    key: [u32; 5],
    meshes: std::sync::Arc<Vec<Mesh>>,
}

impl SceneMeshes {
    pub fn new() -> SceneMeshes {
        SceneMeshes { key: [u32::MAX; 5], meshes: std::sync::Arc::new(Vec::new()) }
    }

    pub fn sync(&mut self, p: &Params) -> &[Mesh] {
        let key = p.mesh_key();
        if key != self.key {
            let (nu, nv) = match p.n {
                0..=3 => (48, 26),
                4..=6 => (40, 22),
                _ => (32, 18),
            };
            self.meshes = std::sync::Arc::new((0..p.n.max(1))
                .map(|i| {
                    let off = p.variety * i as f32;
                    build_mesh(p.a + off, p.b + off, p.mix, nu, nv)
                })
                .collect());
            self.key = key;
        }
        &self.meshes
    }

    /// The current meshes, shareable with the compose worker.
    pub fn shared(&self) -> std::sync::Arc<Vec<Mesh>> {
        self.meshes.clone()
    }
}

impl Default for SceneMeshes {
    fn default() -> Self {
        Self::new()
    }
}

struct Placed {
    world: Vec<Vec3>,
    rot: Mat3,
    pos: Vec3,
    size: f32,
}

fn place(p: &Params, meshes: &[Mesh], time: f32) -> Vec<Placed> {
    let n = meshes.len();
    let spread = 0.3 + 3.0 * p.dispersion;
    meshes
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let h = |k: u32| hash(i as u32 * 31 + k);
            let ang = i as f32 * 2.399_963 + 0.6;
            let rad = if n == 1 { 0.0 } else { spread * ((i as f32 + 0.5) / n as f32).sqrt() };
            let size = 0.62 * (0.75 + 0.5 * h(1)) * if n > 6 { 0.85 } else { 1.0 };
            let pos = Vec3::new(rad * ang.cos(), -0.55 + size * 1.0 - 0.05, rad * ang.sin());
            let yaw = h(2) * TAU + p.turn * time * (0.6 + 0.8 * h(3));
            let rot = Mat3::from_rotation_y(yaw) * Mat3::from_rotation_x((h(4) - 0.5) * 0.4) * Mat3::from_rotation_z((h(5) - 0.5) * 0.3);
            let world = m.verts.iter().map(|&v| pos + rot * (v * size)).collect();
            Placed { world, rot, pos, size }
        })
        .collect()
}

const GROUND_Y: f32 = -0.55;

fn ink_t(z: f32, distance: f32) -> f32 {
    (1.0 - (z - (distance - 2.5)) / 4.5).clamp(0.0, 1.0)
}

/// Runs a 3D polyline through projection and the hidden-line test, returning
/// screen pieces (a piece ends where the ink is fully hidden or clipped by the
/// near plane).
fn project_polyline(
    pl: &[Vec3],
    cam: &CamFrame,
    zb: &ZBuf,
    hide: f32,
    fade: &dyn Fn(Vec3) -> f32,
    max_len: f32,
) -> Vec<Vec<P2>> {
    let distance = cam.eye.length();
    let hidden_a = (1.0 - hide) * 0.5;
    let mut out: Vec<Vec<P2>> = Vec::new();
    let mut cur: Vec<P2> = Vec::new();
    let mut flush = |cur: &mut Vec<P2>, out: &mut Vec<Vec<P2>>| {
        if cur.len() >= 2 {
            out.push(std::mem::take(cur));
        } else {
            cur.clear();
        }
    };
    let mut prev: Option<Vec3> = None;
    for &p in pl {
        let mut seq: Vec<Vec3> = Vec::new();
        if let Some(q) = prev {
            let n = ((q.distance(p) / max_len).ceil() as usize).clamp(1, 400);
            for k in 1..=n {
                seq.push(q.lerp(p, k as f32 / n as f32));
            }
        } else {
            seq.push(p);
        }
        prev = Some(p);
        for w in seq {
            let Some((s, z)) = cam.project(w) else {
                flush(&mut cur, &mut out);
                continue;
            };
            let vis = if zb.hidden(s, z) { hidden_a } else { 1.0 };
            let a = vis * fade(w);
            if a < 0.02 {
                flush(&mut cur, &mut out);
                continue;
            }
            cur.push(P2 { p: s, a, t: ink_t(z, distance) });
        }
    }
    flush(&mut cur, &mut out);
    out
}

fn axis_for(angle: f32) -> Vec3 {
    Vec3::new(angle.cos(), 0.4, angle.sin()).normalize()
}

/// Iso-line segments of `dot(local, axis)` every `d`, on the triangles flagged
/// in `want`, mapped to world by `to_world`.
#[cfg(test)]
fn level_segments(
    local: &[Vec3],
    tris: &[[u32; 3]],
    want: &[bool],
    axis: Vec3,
    d: f32,
    to_world: &dyn Fn(Vec3) -> Vec3,
    out: &mut Vec<(Vec3, Vec3)>,
) {
    level_segments_tagged(local, tris, want, axis, d, to_world, &mut |_, a, b| out.push((a, b)));
}

/// [`level_segments`] reporting the triangle each segment came from.
fn level_segments_tagged(
    local: &[Vec3],
    tris: &[[u32; 3]],
    want: &[bool],
    axis: Vec3,
    d: f32,
    to_world: &dyn Fn(Vec3) -> Vec3,
    out: &mut dyn FnMut(u32, Vec3, Vec3),
) {
    for (ti, t) in tris.iter().enumerate() {
        if !want[ti] {
            continue;
        }
        let p = [local[t[0] as usize], local[t[1] as usize], local[t[2] as usize]];
        let f = [p[0].dot(axis), p[1].dot(axis), p[2].dot(axis)];
        let lo = f.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = f.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let n0 = (lo / d - 0.37).ceil() as i32;
        let n1 = (hi / d - 0.37).floor() as i32;
        if n1 < n0 || n1 - n0 > 64 {
            continue;
        }
        for n in n0..=n1 {
            let level = (n as f32 + 0.37) * d;
            if level < lo || level > hi {
                continue;
            }
            let mut hits: [Vec3; 2] = [Vec3::ZERO; 2];
            let mut count = 0;
            for k in 0..3 {
                let (i, j) = (k, (k + 1) % 3);
                if (f[i] - level) * (f[j] - level) < 0.0 && count < 2 {
                    let s = (level - f[i]) / (f[j] - f[i]);
                    hits[count] = to_world(p[i].lerp(p[j], s));
                    count += 1;
                }
            }
            if count == 2 {
                out(ti as u32, hits[0], hits[1]);
            }
        }
    }
}

/// Hatch segments in mesh-local space, per triangle (ascending), for one axis.
type LocalHatch = Vec<(u32, Vec3, Vec3)>;

/// Mesh-local hatch segments of every triangle, per mesh and layer (main,
/// cross), keyed on the meshes and the hatch parameters. Camera, light and
/// time only choose which triangles show, so they never invalidate it.
#[derive(Default)]
pub struct HatchCache {
    key: Option<[u32; 8]>,
    meshes: Vec<[LocalHatch; 2]>,
}

impl HatchCache {
    fn sync(&mut self, p: &Params, meshes: &[Mesh], d: f32) {
        let cross = p.hatch_cross > 0.01;
        let mk = p.mesh_key();
        let key = [mk[0], mk[1], mk[2], mk[3], mk[4], p.hatch_ang.to_bits(), d.to_bits(), cross as u32];
        if self.key == Some(key) {
            return;
        }
        let ident = |v: Vec3| v;
        self.meshes = meshes
            .iter()
            .map(|m| {
                let all = vec![true; m.tris.len()];
                let layer = |ang: f32| {
                    let mut out: LocalHatch = Vec::new();
                    level_segments_tagged(&m.verts, &m.tris, &all, axis_for(ang), d, &ident, &mut |t, a, b| out.push((t, a, b)));
                    out
                };
                [layer(p.hatch_ang), if cross { layer(p.hatch_ang + PI * 0.5) } else { Vec::new() }]
            })
            .collect();
        self.key = Some(key);
    }
}

/// The whole frame's strokes in screen space, post-processed and capped.
pub fn compose(p: &Params, meshes: &[Mesh], cam: &CamFrame, time: f32, aspect: f32) -> Vec<Run> {
    compose_with(p, meshes, cam, time, aspect, &mut HatchCache::default())
}

/// [`compose`] reusing the mesh-local hatch work in `cache`.
pub fn compose_with(p: &Params, meshes: &[Mesh], cam: &CamFrame, time: f32, aspect: f32, cache: &mut HatchCache) -> Vec<Run> {
    let aspect = aspect.clamp(0.1, 10.0);
    let placed = place(p, meshes, time);
    // Depth buffer of every triangle.
    let mut zb = ZBuf::new(aspect);
    let projected: Vec<Vec<Option<(Vec2, f32)>>> =
        placed.iter().map(|o| o.world.iter().map(|&w| cam.project(w)).collect()).collect();
    for (oi, m) in meshes.iter().enumerate() {
        for t in &m.tris {
            let v: Vec<_> = t.iter().map(|&i| projected[oi][i as usize]).collect();
            if let (Some(a), Some(b), Some(c)) = (v[0], v[1], v[2]) {
                zb.tri([a.0, b.0, c.0], [a.1, b.1, c.1]);
            }
        }
    }

    let light = Vec3::new(p.light.cos(), 0.85, p.light.sin()).normalize();
    let mut pieces: Vec<(Vec<P2>, u8)> = Vec::new();
    let hatch_d = ((0.30 + (0.035 - 0.30) * p.hatch_dens) / p.hatch_scale.max(0.1)).max(0.02);
    if p.hatch_dens > 0.01 {
        cache.sync(p, meshes, hatch_d);
    }
    let thr2 = 0.75 - 0.6 * p.hatch_cross;
    let always = |_: Vec3| 1.0;

    for (oi, m) in meshes.iter().enumerate() {
        let o = &placed[oi];
        // Face normals, front flags, darkness.
        let mut front = vec![false; m.tris.len()];
        let mut dark = vec![0.0f32; m.tris.len()];
        for (ti, t) in m.tris.iter().enumerate() {
            let (a, b, c) = (o.world[t[0] as usize], o.world[t[1] as usize], o.world[t[2] as usize]);
            let n = (b - a).cross(c - a).normalize_or_zero();
            let centre = (a + b + c) / 3.0;
            front[ti] = n.dot(cam.eye - centre) > 0.0;
            dark[ti] = 1.0 - (n.dot(light) * 0.5 + 0.5).powf(1.3);
        }

        // Silhouette: front/back flips and open boundaries.
        let mut sil: Vec<(Vec3, Vec3)> = Vec::new();
        for &(a, b, f0, f1) in &m.edges {
            let s0 = front[f0 as usize];
            let edge = if f1 == u32::MAX { s0 } else { s0 != front[f1 as usize] };
            if edge {
                sil.push((o.world[a as usize], o.world[b as usize]));
            }
        }
        for pl in linemerge(&sil) {
            for piece in project_polyline(&pl, cam, &zb, p.hide, &always, 0.3) {
                pieces.push((piece, 0));
            }
        }

        // Hatching.
        if p.hatch_dens > 0.01 {
            let to_world = |v: Vec3| o.pos + o.rot * (v * o.size);
            let layers = &cache.meshes[oi];
            let mut segs = Vec::new();
            for &(t, a, b) in &layers[0] {
                if front[t as usize] && dark[t as usize] > 0.12 {
                    segs.push((to_world(a), to_world(b)));
                }
            }
            if p.hatch_cross > 0.01 {
                for &(t, a, b) in &layers[1] {
                    if front[t as usize] && dark[t as usize] > thr2 {
                        segs.push((to_world(a), to_world(b)));
                    }
                }
            }
            for pl in linemerge(&segs) {
                for piece in project_polyline(&pl, cam, &zb, p.hide, &always, 0.3) {
                    pieces.push((piece, 1));
                }
            }
        }
    }

    // Ground plane grid, fading with distance from the scene.
    if p.plane > 0.01 {
        let ext = 3.2f32;
        let sp = 1.0 + (0.28 - 1.0) * p.plane;
        let n = (ext / sp).floor() as i32;
        let fade = |w: Vec3| (1.0 - (w.x * w.x + w.z * w.z) / (ext * ext)).clamp(0.0, 1.0).powf(0.8) * (0.35 + 0.65 * p.plane);
        for k in -n..=n {
            let c = k as f32 * sp;
            for line in [[Vec3::new(-ext, GROUND_Y, c), Vec3::new(ext, GROUND_Y, c)], [Vec3::new(c, GROUND_Y, -ext), Vec3::new(c, GROUND_Y, ext)]] {
                for piece in project_polyline(&line, cam, &zb, p.hide, &fade, 0.3) {
                    pieces.push((piece, 2));
                }
            }
        }
    }

    // vpype-style post.
    let m = p.cut * 0.85;
    let (lo, hi) = (Vec2::new(-aspect * (1.0 - m), -(1.0 - m)), Vec2::new(aspect * (1.0 - m), 1.0 - m));
    let tol = p.simplify * p.simplify * 0.05;
    let mut runs: Vec<Run> = Vec::new();
    let phase = (time * 8.0).floor() * 7.31;
    for (pts, kind) in pieces {
        let pts = simplify(&pts, tol);
        for c in crop(&pts, lo, hi) {
            let dashes = if p.dotted > 0.01 {
                dash(&c, 0.012 + 0.08 * (1.0 - p.dotted), (0.012 + 0.08 * (1.0 - p.dotted)) * 0.55)
            } else {
                vec![c]
            };
            for mut d in dashes {
                if d.len() < 2 {
                    continue;
                }
                if p.noise > 0.01 {
                    d = resample(&d, 0.04);
                    let amp = p.noise * 0.012;
                    for v in &mut d {
                        let q = v.p * 16.0 + Vec2::splat(phase);
                        v.p += Vec2::new(vnoise(q) - 0.5, vnoise(q + Vec2::new(31.7, 17.3)) - 0.5) * 2.0 * amp;
                    }
                }
                runs.push(Run { pts: d, kind });
            }
        }
    }
    if p.cut > 0.01 {
        let t = 1.0;
        let c = |x: f32, y: f32| P2 { p: Vec2::new(x, y), a: 1.0, t };
        runs.push(Run { pts: vec![c(lo.x, lo.y), c(hi.x, lo.y), c(hi.x, hi.y), c(lo.x, hi.y), c(lo.x, lo.y)], kind: 3 });
    }
    cap_segments(runs, MAX_SEGMENTS)
}

/// Keeps at most `cap` segments: drops whole strokes at an even stride first,
/// then truncates.
pub fn cap_segments(runs: Vec<Run>, cap: usize) -> Vec<Run> {
    let total = segment_count(&runs);
    if total <= cap {
        return runs;
    }
    let frame: usize = runs.iter().filter(|r| r.kind == 3).map(|r| r.pts.len().saturating_sub(1)).sum();
    let budget = cap.saturating_sub(frame);
    let rest = total - frame;
    let stride = rest.div_ceil(budget.max(1)).max(2);
    let mut left = budget;
    let mut out = Vec::new();
    let mut n = 0usize;
    for r in runs {
        if r.kind == 3 {
            out.push(r);
            continue;
        }
        n += 1;
        if (n - 1) % stride != 0 || left == 0 {
            continue;
        }
        let mut r = r;
        let segs = r.pts.len().saturating_sub(1);
        if segs > left {
            r.pts.truncate(left + 1);
        }
        left -= r.pts.len().saturating_sub(1);
        if r.pts.len() >= 2 {
            out.push(r);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// GPU
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
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
struct BgUniform {
    background: ColorSlotGpu,
    a: [f32; 4],
    b: [f32; 4],
}

struct LinePipe {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl LinePipe {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> LinePipe {
        let src = format!("{}\n{}", kit::WGSL, include_str!("../../shaders/fineline_line.wgsl"));
        let shader = build_shader(device, "fineline line", &src);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fineline line"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fineline line"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let vl = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LVert>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32, 2 => Float32, 3 => Float32x4],
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fineline line"),
            layout: Some(&pl),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), buffers: &[Some(vl)], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
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

/// Triangle-strip vertices for every run, joined by degenerate triangles.
/// Widths are in pixels; `px` is the size of one pixel in aspect units.
fn strip_vertices(runs: &[Run], slot: &ColorSlotGpu, widths: [f32; 4], px: f32) -> Vec<LVert> {
    let mut v: Vec<LVert> = Vec::new();
    for r in runs {
        let half_px = (widths[r.kind.min(3) as usize] * 0.5).max(0.3);
        if widths[r.kind.min(3) as usize] <= 0.0 {
            continue;
        }
        let total = half_px + 1.0;
        let mut pts: Vec<&P2> = Vec::with_capacity(r.pts.len());
        for p in &r.pts {
            if pts.last().map_or(true, |l| l.p.distance_squared(p.p) > 1e-12) {
                pts.push(p);
            }
        }
        if pts.len() < 2 {
            continue;
        }
        let first = v.len();
        for i in 0..pts.len() {
            let d0 = if i > 0 { (pts[i].p - pts[i - 1].p).normalize_or_zero() } else { Vec2::ZERO };
            let d1 = if i + 1 < pts.len() { (pts[i + 1].p - pts[i].p).normalize_or_zero() } else { Vec2::ZERO };
            let t = if i == 0 { d1 } else if i + 1 == pts.len() { d0 } else { (d0 + d1).normalize_or_zero() };
            let t = if t == Vec2::ZERO { d0 + d1 } else { t };
            let n = Vec2::new(-t.y, t.x);
            let ref_n = if i + 1 < pts.len() { Vec2::new(-d1.y, d1.x) } else { Vec2::new(-d0.y, d0.x) };
            let miter = (1.0 / n.dot(ref_n).abs().max(0.4)).min(2.5);
            let c = slot.grad4(pts[i].t);
            let col = [c[0], c[1], c[2], pts[i].a.clamp(0.0, 1.0)];
            let off = n * (total * px * miter);
            for s in [-1.0f32, 1.0] {
                v.push(LVert {
                    pos: (pts[i].p + off * s).into(),
                    side: s * total * miter,
                    soft: half_px * miter,
                    color: col,
                });
            }
        }
        // Join to the previous strip with degenerate vertices.
        if first > 0 {
            let last = v[first - 1];
            let head = v[first];
            v.insert(first, last);
            v.insert(first + 1, head);
        }
    }
    v
}

/// Compose with every stage cache: the mesh-local hatch work, and the whole
/// frame when its inputs repeat bit for bit.
#[derive(Default)]
struct Composer {
    hatch: HatchCache,
    last: Option<(Vec<u32>, Arc<Vec<Run>>)>,
}

impl Composer {
    /// The strokes for these inputs; the very same `Arc` as last time when
    /// nothing that matters changed (time only counts when something spins or
    /// wobbles with it).
    fn run(&mut self, p: &Params, meshes: &[Mesh], cam: &CamFrame, time: f32, aspect: f32) -> Arc<Vec<Run>> {
        let mut key = Vec::with_capacity(48);
        p.push_key(&mut key);
        key.extend([cam.eye, cam.right, cam.up, cam.fwd].iter().flat_map(|v| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]));
        key.push(cam.tan_half.to_bits());
        key.push(aspect.to_bits());
        key.push(if p.turn != 0.0 || p.noise > 0.01 { time.to_bits() } else { 0 });
        if let Some((k, runs)) = &self.last {
            if *k == key {
                return runs.clone();
            }
        }
        let runs = Arc::new(compose_with(p, meshes, cam, time, aspect, &mut self.hatch));
        self.last = Some((key, runs.clone()));
        runs
    }
}

struct Job {
    p: Params,
    meshes: Arc<Vec<Mesh>>,
    cam: CamFrame,
    time: f32,
    aspect: f32,
}

/// A thread running [`Composer`] one frame behind the renderer. Dropping it
/// closes the job channel and joins the thread.
struct Worker {
    tx: Option<mpsc::Sender<Job>>,
    rx: mpsc::Receiver<Arc<Vec<Run>>>,
    handle: Option<std::thread::JoinHandle<()>>,
    busy: bool,
}

impl Worker {
    fn spawn() -> Worker {
        let (tx, jobs) = mpsc::channel::<Job>();
        let (done, rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("fineline compose".into())
            .spawn(move || {
                let mut composer = Composer::default();
                while let Ok(mut job) = jobs.recv() {
                    // Only the newest request matters.
                    while let Ok(newer) = jobs.try_recv() {
                        job = newer;
                    }
                    let runs = composer.run(&job.p, &job.meshes, &job.cam, job.time, job.aspect);
                    if done.send(runs).is_err() {
                        break;
                    }
                }
            })
            .ok();
        Worker { tx: Some(tx), rx, handle, busy: false }
    }

    /// Queues a job; false when the thread is gone.
    fn submit(&mut self, job: Job) -> bool {
        self.busy = self.tx.as_ref().is_some_and(|tx| tx.send(job).is_ok());
        self.busy
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// GPU buffers kept between frames: the vertex buffer only grows.
#[derive(Default)]
struct LineGpu {
    vbuf: Option<wgpu::Buffer>,
    cap: u64,
    ubuf: Option<wgpu::Buffer>,
    bind: Option<(wgpu::TextureFormat, wgpu::BindGroup)>,
    /// What the uploaded vertices were built from.
    vkey: Vec<u8>,
    count: u32,
}

pub struct Fineline {
    bg: Fullscreen,
    lines: Built<LinePipe>,
    meshes: SceneMeshes,
    composer: Composer,
    worker: Option<Worker>,
    /// Compose on the worker thread (one frame behind) instead of inline.
    threaded: bool,
    shown: Option<Arc<Vec<Run>>>,
    /// Bumps whenever `shown` changes to different strokes.
    shown_id: u64,
    gpu: LineGpu,
}

fn bg_src() -> &'static str {
    static SRC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SRC.get_or_init(|| format!("{}\n{}", kit::WGSL, include_str!("../../shaders/fineline_bg.wgsl")))
}

impl Fineline {
    pub fn new() -> Fineline {
        Fineline {
            bg: Fullscreen::new("fineline", bg_src()),
            lines: Built::default(),
            meshes: SceneMeshes::new(),
            composer: Composer::default(),
            worker: None,
            threaded: true,
            shown: None,
            shown_id: 0,
            gpu: LineGpu::default(),
        }
    }

    /// A model that composes inline in `render`, so frames are a pure function
    /// of the inputs (the tests).
    pub fn new_sync() -> Fineline {
        Fineline { threaded: false, ..Fineline::new() }
    }

    fn inputs(floats: &[f32], time: f32) -> (Params, CamFrame, f32) {
        let own = &floats[off(B_OWN)..off(B_OWN) + FLOATS.len()];
        let time = clean(time, -1e6, 1e6, 0.0);
        let cam = camera(&kit::Cam3d::from_floats(&floats[off(B_CAM)..off(B_CAM) + kit::Cam3d::N]), time);
        (Params::from_floats(own), cam, time)
    }

    /// The strokes this model would draw (no GPU, always inline): used by the tests.
    pub fn strokes(&mut self, floats: &[f32], aspect: f32, time: f32) -> Vec<Run> {
        let (p, cam, time) = Self::inputs(floats, time);
        self.meshes.sync(&p);
        let meshes = self.meshes.shared();
        self.composer.run(&p, &meshes, &cam, time, aspect.clamp(0.1, 10.0)).as_ref().clone()
    }

    /// The strokes to draw now. Inline when not threaded and for the first
    /// frame (never draw empty); otherwise the newest finished worker result,
    /// with the inputs of this frame queued for the next.
    fn frame_runs(&mut self, p: &Params, cam: &CamFrame, time: f32, aspect: f32) -> Arc<Vec<Run>> {
        self.meshes.sync(p);
        let meshes = self.meshes.shared();
        let aspect = aspect.clamp(0.1, 10.0);
        if self.threaded {
            if let Some(shown) = self.shown.clone() {
                let w = self.worker.get_or_insert_with(Worker::spawn);
                let mut newest = None;
                while let Ok(r) = w.rx.try_recv() {
                    newest = Some(r);
                    w.busy = false;
                }
                if !w.busy && !w.submit(Job { p: *p, meshes, cam: *cam, time, aspect }) {
                    // The worker died: fall back to inline from here on.
                    self.worker = None;
                    self.threaded = false;
                }
                let runs = newest.unwrap_or(shown);
                self.adopt(&runs);
                return runs;
            }
        }
        let runs = self.composer.run(p, &meshes, cam, time, aspect);
        self.adopt(&runs);
        runs
    }

    fn adopt(&mut self, runs: &Arc<Vec<Run>>) {
        if !self.shown.as_ref().is_some_and(|s| Arc::ptr_eq(s, runs)) {
            self.shown_id += 1;
            self.shown = Some(runs.clone());
        }
    }
}

impl Default for Fineline {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for Fineline {
    fn id(&self) -> &'static str {
        "fineline"
    }

    fn name(&self) -> &'static str {
        "Fineline"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn render(&mut self, ctx: &Ctx, floats: &[f32], target: &wgpu::TextureView, format: wgpu::TextureFormat) {
        let aspect = ctx.aspect().max(0.01);
        let look = kit::Look::from_floats(&floats[off(B_LOOK)..off(B_LOOK) + kit::Look::N]);
        let slot = |blk: usize| ColorSlotGpu::from_floats(&floats[off(blk)..off(blk) + 12]);
        let time = if ctx.time.is_finite() { ctx.time } else { 0.0 };
        let (mut params, cam, time) = Self::inputs(floats, time);
        params.hatch_scale = Params::hatch_scale_for(ctx.height);

        let bg = BgUniform {
            background: slot(B_FONDO),
            a: [aspect, if ctx.fondo_alpha { 1.0 } else { 0.0 }, look.background_level, look.opacity],
            b: [look.shine, look.contrast, look.tone, look.saturation],
        };
        self.bg.draw(ctx, target, format, &bg);

        let runs = self.frame_runs(&params, &cam, time, aspect);
        let px = 2.0 / ctx.height.max(1) as f32;
        let widths = [params.sil_px, params.trazo_px, params.trazo_px * 0.7, params.trazo_px];
        let trazo = slot(B_TRAZO);
        // Rebuild and upload the vertices only when something they come from moved.
        let mut vkey: Vec<u8> = self.shown_id.to_le_bytes().to_vec();
        vkey.extend_from_slice(bytemuck::bytes_of(&trazo));
        vkey.extend(widths.iter().chain(&[px]).flat_map(|w| w.to_bits().to_le_bytes()));
        let g = &mut self.gpu;
        if g.vkey != vkey {
            let verts = strip_vertices(&runs, &trazo, widths, px);
            g.count = verts.len() as u32;
            if !verts.is_empty() {
                let bytes: &[u8] = bytemuck::cast_slice(&verts);
                if g.vbuf.is_none() || bytes.len() as u64 > g.cap {
                    g.cap = (bytes.len() as u64).next_power_of_two().max(4096);
                    g.vbuf = Some(ctx.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("fineline vertices"),
                        size: g.cap,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }));
                }
                if let Some(vb) = &g.vbuf {
                    ctx.queue.write_buffer(vb, 0, bytes);
                }
            }
            g.vkey = vkey;
        }
        if g.count == 0 {
            return;
        }
        let pipe = self.lines.get(format, || LinePipe::new(ctx.device, format));
        let uniform = LineUniform {
            look: [look.shine, look.contrast, look.tone, look.saturation],
            misc: [aspect, look.opacity, 0.0, 0.0],
        };
        let ubuf = g.ubuf.get_or_insert_with(|| {
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fineline line uniform"),
                size: std::mem::size_of::<LineUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        ctx.queue.write_buffer(ubuf, 0, bytemuck::bytes_of(&uniform));
        if g.bind.as_ref().map(|b| b.0) != Some(format) {
            let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fineline line"),
                layout: &pipe.layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
            });
            g.bind = Some((format, bind));
        }
        let (Some(vbuf), Some((_, bind))) = (&g.vbuf, &g.bind) else { return };
        let mut enc = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("fineline lines") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fineline lines"),
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
            pass.set_bind_group(0, bind, &[]);
            pass.set_vertex_buffer(0, vbuf.slice(0..g.count as u64 * std::mem::size_of::<LVert>() as u64));
            pass.draw(0..g.count, 0..1);
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
    const TIMES: [f32; 2] = [0.0, 0.7];

    fn frame_at(gpu: &crate::gpu::Gpu, model: &mut Fineline, floats: &[f32], time: f32) -> Vec<u8> {
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, W, H, FORMAT);
        let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time, width: W, height: H, fondo_alpha: false };
        model.render(&ctx, floats, &view, FORMAT);
        crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, W, H)
    }

    fn frames(gpu: &crate::gpu::Gpu, floats: &[f32]) -> Vec<u8> {
        let mut m = Fineline::new_sync();
        TIMES.iter().flat_map(|&t| frame_at(gpu, &mut m, floats, t)).collect()
    }

    fn base() -> Vec<f32> {
        SCHEMA.defaults()
    }

    fn p2(x: f32, y: f32) -> P2 {
        P2 { p: Vec2::new(x, y), a: 1.0, t: 0.5 }
    }

    #[test]
    fn widths_come_from_the_blocks() {
        assert_eq!(SCHEMA.defaults().len(), SCHEMA.width());
        assert_eq!(off(B_OWN), 0);
        assert_eq!(off(B_TRAZO), FLOATS.len());
        assert_eq!(off(B_CAM) + kit::Cam3d::N, off(B_LOOK));
        assert_eq!(off(B_LOOK) + kit::Look::N, off(B_LOOK + 1));
        assert!(!Fineline::new().camera_track());
    }

    #[test]
    fn indices_match_the_names() {
        let n = |i: usize| FLOATS[i].name;
        assert_eq!((n(F_A), n(F_B), n(F_MEZCLA), n(F_N), n(F_DISP), n(F_GIRO)), ("shape_a", "shape_b", "shape_mix", "n_objects", "dispersion", "turn_object"));
        assert_eq!((n(F_HDENS), n(F_HANG), n(F_HCRUZ), n(F_SIL), n(F_TRAZO), n(F_RUIDO)), ("hatch_density", "hatch_angle", "hatch_cross", "silhouette_thickness", "stroke_thickness", "stroke_noise"));
        assert_eq!((n(F_SIMP), n(F_CORTE), n(F_PUNT), n(F_OCULTAR), n(F_PLANO), n(F_LUZ)), ("simplify", "cut", "dotted", "hide", "plane", "light_turn"));
        for i in [F_A, F_B, F_N, F_DISP, F_VARIEDAD] {
            assert!(FLOATS[i].drive < 1.0, "{} must be held", FLOATS[i].name);
        }
    }

    #[test]
    fn every_shape_is_closed_and_wound_outward() {
        for k in 0..SHAPES {
            let m = build_mesh(k as f32, 0.0, 0.0, 40, 22);
            assert!(signed_volume(&m) > 0.05, "shape {k} is wound inward or empty ({})", signed_volume(&m));
            let open: Vec<_> = m.edges.iter().filter(|e| e.3 == u32::MAX).collect();
            assert!(open.is_empty(), "shape {k} has {} open edges, e.g. {:?} {:?}", open.len(), m.verts[open[0].0 as usize], m.verts[open[0].1 as usize]);
            assert!(m.verts.iter().all(|v| v.is_finite() && v.length() < 1.6));
        }
    }

    #[test]
    fn the_sphere_is_the_unit_sphere_and_morphs_are_continuous() {
        let m = build_mesh(0.0, 0.0, 0.0, 24, 12);
        assert!(m.verts.iter().all(|v| (v.length() - 1.0).abs() < 1e-4));
        let a = morph_point(0.0, 0.3, 0.4);
        let b = morph_point(0.001, 0.3, 0.4);
        assert!(a.distance(b) < 1e-3);
        assert!(morph_point(4.999, 0.3, 0.4).distance(morph_point(0.0, 0.3, 0.4)) < 0.05);
    }

    #[test]
    fn linemerge_chains_and_drops_duplicates() {
        let p = |x: f32| Vec3::new(x, 0.0, 0.0);
        let segs = [(p(0.0), p(1.0)), (p(2.0), p(1.0)), (p(2.0), p(3.0)), (p(1.0), p(0.0)), (Vec3::Y, Vec3::Y + Vec3::X)];
        let mut lines = linemerge(&segs);
        lines.sort_by_key(|l| l.len());
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 2);
        assert_eq!(lines[1].len(), 4);
        let xs: Vec<f32> = lines[1].iter().map(|v| v.x).collect();
        assert!(xs == vec![0.0, 1.0, 2.0, 3.0] || xs == vec![3.0, 2.0, 1.0, 0.0], "{xs:?}");
    }

    #[test]
    fn simplify_removes_collinear_points_and_keeps_corners() {
        let line: Vec<P2> = (0..=10).map(|i| p2(i as f32 * 0.1, 0.0)).collect();
        assert_eq!(simplify(&line, 0.001).len(), 2);
        let mut bent = line.clone();
        bent.extend((1..=5).map(|i| p2(1.0, i as f32 * 0.1)));
        let s = simplify(&bent, 0.001);
        assert_eq!(s.len(), 3);
        assert!((s[1].p - Vec2::new(1.0, 0.0)).length() < 1e-6);
        // A big tolerance flattens a shallow bump; zero tolerance keeps everything.
        let bump = [p2(0.0, 0.0), p2(0.5, 0.01), p2(1.0, 0.0)];
        assert_eq!(simplify(&bump, 0.02).len(), 2);
        assert_eq!(simplify(&bump, 0.0).len(), 3);
        // A closed loop survives as a loop.
        let sq = [p2(0.0, 0.0), p2(1.0, 0.0), p2(1.0, 1.0), p2(0.0, 1.0), p2(0.0, 0.0)];
        assert_eq!(simplify(&sq, 0.01).len(), 5);
    }

    #[test]
    fn crop_clips_to_the_box_and_splits_strokes() {
        let (lo, hi) = (Vec2::splat(-1.0), Vec2::splat(1.0));
        let inside = [p2(-0.5, 0.0), p2(0.5, 0.0)];
        assert_eq!(crop(&inside, lo, hi).len(), 1);
        let across = [p2(-2.0, 0.0), p2(2.0, 0.0)];
        let c = crop(&across, lo, hi);
        assert_eq!(c.len(), 1);
        assert!((c[0][0].p.x + 1.0).abs() < 1e-5 && (c[0][1].p.x - 1.0).abs() < 1e-5);
        // Leaves and re-enters: two pieces.
        let uu = [p2(-0.5, 0.5), p2(-0.5, 2.0), p2(0.5, 2.0), p2(0.5, 0.5)];
        assert_eq!(crop(&uu, lo, hi).len(), 2);
        assert!(crop(&[p2(3.0, 3.0), p2(4.0, 3.0)], lo, hi).is_empty());
    }

    #[test]
    fn dash_cuts_a_line_into_the_right_share() {
        let line = [p2(0.0, 0.0), p2(1.0, 0.0)];
        let d = dash(&line, 0.1, 0.05);
        assert_eq!(d.len(), 10);
        let on: f32 = d.iter().map(|s| s[0].p.distance(s[s.len() - 1].p)).sum();
        assert!((on - 0.5).abs() < 0.01, "{on}");
    }

    #[test]
    fn hatch_lines_are_evenly_spaced_iso_lines() {
        let m = build_mesh(0.0, 0.0, 0.0, 40, 22);
        let want = vec![true; m.tris.len()];
        let mut segs = Vec::new();
        let axis = Vec3::Y;
        level_segments(&m.verts, &m.tris, &want, axis, 0.25, &|v| v, &mut segs);
        assert!(!segs.is_empty());
        // Every segment lies at a level (n + 0.37) * 0.25 of y.
        for (a, b) in &segs {
            let n = a.y / 0.25 - 0.37;
            assert!((n - n.round()).abs() < 1e-3 && (a.y - b.y).abs() < 1e-4, "{a:?} {b:?}");
        }
        let levels: HashSet<i32> = segs.iter().map(|(a, _)| (a.y * 1000.0).round() as i32).collect();
        assert_eq!(levels.len(), 8);
        // Denser spacing, more lines.
        let mut more = Vec::new();
        level_segments(&m.verts, &m.tris, &want, axis, 0.1, &|v| v, &mut more);
        assert!(more.len() > segs.len());
        let chains = linemerge(&segs);
        assert_eq!(chains.len(), levels.len(), "each ring merges into one loop");
    }

    #[test]
    fn the_cap_holds_and_keeps_the_frame() {
        let long = |kind| Run { pts: (0..=1000).map(|i| p2(i as f32 * 1e-3, 0.0)).collect(), kind };
        let runs: Vec<Run> = (0..200).map(|_| long(1)).chain(std::iter::once(long(3))).collect();
        let out = cap_segments(runs, 5000);
        assert!(segment_count(&out) <= 5000);
        assert!(out.iter().any(|r| r.kind == 3));
    }

    #[test]
    fn default_strokes_are_bounded_and_deterministic() {
        let f = base();
        let mut m = Fineline::new();
        let a = m.strokes(&f, 1.5, 0.3);
        let b = Fineline::new().strokes(&f, 1.5, 0.3);
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(&b).all(|(x, y)| x.pts.len() == y.pts.len() && x.pts.iter().zip(&y.pts).all(|(p, q)| p.p == q.p)));
        let n = segment_count(&a);
        assert!(n > 200 && n < MAX_SEGMENTS, "{n} segments");
        assert!(a.iter().flat_map(|r| &r.pts).all(|p| p.p.is_finite() && p.a.is_finite()));
        for kind in 0..4 {
            assert!(a.iter().any(|r| r.kind == kind), "no stroke of kind {kind}");
        }
    }

    #[test]
    fn a_repeated_frame_is_a_cache_hit_and_the_worker_matches_inline() {
        let f = base();
        let (p, cam, time) = Fineline::inputs(&f, 0.3);
        let meshes: Vec<Mesh> = SceneMeshes::new().sync(&p).to_vec();
        let mut c = Composer::default();
        let a = c.run(&p, &meshes, &cam, time, 1.5);
        let b = c.run(&p, &meshes, &cam, time, 1.5);
        assert!(Arc::ptr_eq(&a, &b), "identical inputs must reuse the strokes");
        let moved = c.run(&p, &meshes, &cam, time, 1.6);
        assert!(!Arc::ptr_eq(&a, &moved));
        let same = |x: &[Run], y: &[Run]| {
            x.len() == y.len() && x.iter().zip(y).all(|(r, s)| r.kind == s.kind && r.pts.len() == s.pts.len() && r.pts.iter().zip(&s.pts).all(|(p, q)| p.p == q.p && p.a == q.a))
        };
        // The hatch cache changes nothing against a cold compose.
        assert!(same(&a, &compose(&p, &meshes, &cam, time, 1.5)));
        // The worker answers with what inline composes.
        let mut w = Worker::spawn();
        assert!(w.submit(Job { p, meshes: Arc::new(meshes.clone()), cam, time, aspect: 1.5 }));
        let got = w.rx.recv().expect("worker result");
        assert!(same(&a, &got));
    }

    #[test]
    fn small_outputs_carry_fewer_hatch_lines() {
        let mut f = base();
        f[F_HDENS] = 0.8;
        let (mut p, cam, time) = Fineline::inputs(&f, 0.0);
        let meshes: Vec<Mesh> = SceneMeshes::new().sync(&p).to_vec();
        let full = segment_count(&compose(&p, &meshes, &cam, time, 1.5));
        assert_eq!(Params::hatch_scale_for(1080), 1.0);
        p.hatch_scale = Params::hatch_scale_for(270);
        let small = segment_count(&compose(&p, &meshes, &cam, time, 1.5));
        assert!(small < full, "{small} !< {full}");
    }

    #[test]
    fn the_defaults_are_not_flat_and_deterministic() {
        let gpu = crate::gpu::Gpu::new_headless();
        let f = base();
        let a = frames(&gpu, &f);
        assert_eq!(a, frames(&gpu, &f));
        let first = &a[..(W * H * 4) as usize];
        let mut distinct = std::collections::HashSet::new();
        for px in first.chunks_exact(4) {
            distinct.insert([px[0], px[1], px[2]]);
        }
        assert!(distinct.len() > 16, "default frame is flat ({} colours)", distinct.len());
        assert_ne!(first, &a[(W * H * 4) as usize..], "the objects must spin with time");
    }

    #[test]
    fn every_float_moves_the_frame() {
        let gpu = crate::gpu::Gpu::new_headless();
        let (cam, look) = (off(B_CAM), off(B_LOOK));
        let mut idx: Vec<(usize, FloatSpec)> = FLOATS.iter().enumerate().map(|(i, s)| (i, *s)).collect();
        idx.extend(kit::CAM3D.iter().enumerate().map(|(i, s)| (cam + i, *s)));
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
    fn corner_values_stay_finite_and_under_the_cap() {
        let gpu = crate::gpu::Gpu::new_headless();
        let width = SCHEMA.width();
        for pick in 0..4 {
            let mut f = base();
            for (i, s) in FLOATS.iter().enumerate() {
                f[i] = match pick {
                    0 => s.min,
                    1 => s.max,
                    2 => if i % 2 == 0 { s.min } else { s.max },
                    _ => if i % 2 == 1 { s.min } else { s.max },
                };
            }
            if pick == 3 {
                // Out of range and not-a-number are clamped, not propagated.
                f[F_N] = f32::NAN;
                f[F_A] = 1e9;
                f[F_HDENS] = 50.0;
                f[off(B_CAM) + 3] = -3.0; // dist
                f[off(B_CAM) + 7] = -1e6; // dolly
            }
            assert_eq!(f.len(), width);
            let mut m = Fineline::new();
            for t in [0.0, 1.3] {
                let runs = m.strokes(&f, 16.0 / 9.0, t);
                assert!(segment_count(&runs) <= MAX_SEGMENTS, "pick {pick}: {} segments", segment_count(&runs));
                assert!(runs.iter().flat_map(|r| &r.pts).all(|p| p.p.is_finite() && p.a.is_finite() && p.t.is_finite()));
            }
            let px = frames(&gpu, &f);
            let mut distinct = std::collections::HashSet::new();
            for p in px.chunks_exact(4) {
                distinct.insert([p[0], p[1], p[2]]);
            }
            assert!(distinct.len() > 1 || pick == 3, "pick {pick} drew nothing at all");
        }
    }

    #[test]
    fn dense_settings_stay_under_the_cap() {
        let mut f = base();
        f[F_N] = 9.0;
        f[F_HDENS] = 1.0;
        f[F_HCRUZ] = 1.0;
        f[F_SIMP] = 0.0;
        f[F_RUIDO] = 1.0;
        f[F_PUNT] = 1.0;
        f[F_PLANO] = 1.0;
        f[F_OCULTAR] = 0.0;
        let runs = Fineline::new().strokes(&f, 16.0 / 9.0, 0.5);
        let n = segment_count(&runs);
        assert!(n <= MAX_SEGMENTS, "{n}");
        println!("dense segments: {n}");
    }

    #[test]
    fn hidden_line_removal_removes_ink() {
        let mut f = base();
        f[F_OCULTAR] = 0.0;
        let all = segment_count(&Fineline::new().strokes(&f, 1.5, 0.0));
        f[F_OCULTAR] = 1.0;
        let some = segment_count(&Fineline::new().strokes(&f, 1.5, 0.0));
        assert!(some < all, "{some} !< {all}");
    }

    #[test]
    #[ignore = "measurement, not a gate: cargo test -p metal-render fineline -- --ignored --nocapture"]
    fn measure_default_cost() {
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (1280, 720);
        let (tex, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        for (label, dense) in [("default", false), ("dense", true)] {
            let mut f = base();
            if dense {
                f[F_N] = 9.0;
                f[F_HDENS] = 1.0;
                f[F_HCRUZ] = 1.0;
                f[F_RUIDO] = 0.5;
            }
            let mut m = Fineline::new();
            let mut run = |n: u32| {
                for i in 0..n {
                    let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: i as f32 * 0.016, width: w, height: h, fondo_alpha: false };
                    m.render(&ctx, &f, &view, FORMAT);
                }
                let _ = gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
            };
            run(3);
            let t = std::time::Instant::now();
            run(30);
            let ms = t.elapsed().as_secs_f32() * 1000.0 / 30.0;
            let segs = segment_count(&m.strokes(&f, 16.0 / 9.0, 0.5));
            println!("fineline {label} {w}x{h}: {ms:.2} ms/frame, {segs} segments");
        }
        let _ = crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &tex, w, h);
    }
}
