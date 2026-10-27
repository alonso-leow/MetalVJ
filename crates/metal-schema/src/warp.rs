//! `fx.warp`: 2D space distortion described by a **coordinate mesh**
//! (the loops design notes T12). Pure maths here — the parameter list, the mesh, the
//! field and a CPU reference renderer — so the catalog (`metal-graph`), the GPU
//! pass (`metal_render::shared::warp`) and the stand-in painter (`metal-perform`)
//! all evaluate the *same* field.
//!
//! # The notation
//!
//! A warp is a displacement field `D(uv)` over the frame (`uv` in `0..1`,
//! y down). It is never written per pixel; it is written as a **mesh**: an
//! `N × N` grid of control points ([`N`] = 17) sitting at
//! `p_ij = (i, j) / (N - 1)`, each carrying a displacement `d_ij` (a `vec2`,
//! in uv units). The output pixel at `uv` samples the picture at
//! `uv + D(uv)`; `D` is the **Catmull-Rom (bicubic) interpolation** of the
//! `d_ij` over the 4 × 4 control points round `uv`'s cell, edge points
//! repeated past the border. So:
//!
//! - every `d_ij = 0` is the identity (amount 0 renders the input exactly);
//! - the field is smooth (C¹) between points and passes through every
//!   `d_ij` — a mesh point says exactly where its neighbourhood is pulled from;
//! - the mesh is the band limit: a pattern finer than about four points per
//!   cycle cannot be written, which is why `frequency` tops out at
//!   [`MAX_CYCLES`] cycles across the frame.
//!
//! The mesh itself is *generated* from a handful of scalars — the warp's
//! params, every one `0..1` so a loop drives it like any other float
//! ([`PARAMS`]):
//!
//! | param       | meaning |
//! |-------------|---------|
//! | `mode`      | the pattern, `floor(mode × 6)`: 0 radial stretch, 1 horizontal waves, 2 vertical waves, 3 concentric ripples, 4 swirl, 5 diagonal interference |
//! | `amount`    | strength; 0 is the identity in every mode |
//! | `frequency` | cycles across the frame, `0.5 + f × (MAX_CYCLES - 0.5)` (waves, ripples, interference) |
//! | `phase`     | `0..1` of a cycle — loop it to make waves roll |
//! | `center_x`, `center_y` | the centre of radial stretch, ripples and swirl |
//! | `mosh`      | `0..1`: how much of the displaced sample comes from the warp's *previous output* instead of the current frame — datamosh-style smearing that accumulates |
//!
//! Per control point, with `r = (p - c) · (aspect, 1)` (aspect-corrected so
//! circles stay round), `A = amount × 0.15`, `f` the cycles and `φ = 2π ×
//! phase`:
//!
//! - **radial stretch**: `d = -(p - c) × amount × 0.6` — the frame is pulled
//!   from nearer the centre, so the picture swells outward (amount 1 shows
//!   the inner 40 %);
//! - **horizontal waves**: `d = (0, A sin(2π f u + φ))`;
//! - **vertical waves**: `d = (A sin(2π f v + φ), 0)`;
//! - **ripples**: `d = r̂ × A sin(2π f |r| - φ)`;
//! - **swirl**: `r` rotated by `θ = amount × π × (1 - |r| / 0.75)²` (0 past
//!   0.75), `d = rot(r, θ) - r`;
//! - **interference**: `d = A (sin(2π f (u + v) + φ), sin(2π f (u - v) + φ))`.
//!
//! # Mosh
//!
//! `out = mix(cur(uv + D), prev(uv + D), mosh)` where `prev` is this warp
//! node's own previous output (the same size; on the first frame, or after a
//! resize, `prev = cur`). Re-displacing the previous output frame after frame
//! is what smears it along the field, the way a datamosh drags old pixels
//! along stale motion vectors. The history is the node's own rather than the
//! post chain's trail textures: those live per *model* inside the generator's
//! post pass, while `fx.*` nodes run on the picture after readback.

use crate::FloatSpec;

/// Control points per side.
pub const N: usize = 17;

/// The most cycles across the frame the mesh can carry honestly (≥ 4
/// points per cycle).
pub const MAX_CYCLES: f32 = 4.0;

/// The patterns, in `mode` order.
pub const MODES: [&str; 6] = ["radial", "waves_h", "waves_v", "ripples", "swirl", "interference"];

/// The warp's params, in this order everywhere (config keys, the GPU
/// uniform). Every one `0..1`.
pub const PARAMS: &[FloatSpec] = &[
    crate::spec("mode", 0.0, 1.0, 0.0),
    crate::spec("amount", 0.0, 1.0, 0.0),
    crate::spec("frequency", 0.0, 1.0, 0.3),
    crate::spec("phase", 0.0, 1.0, 0.0),
    crate::spec("center_x", 0.0, 1.0, 0.5),
    crate::spec("center_y", 0.0, 1.0, 0.5),
    crate::spec("mosh", 0.0, 1.0, 0.0),
];

/// One warp's values, read from a node config.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Warp {
    pub mode: f32,
    pub amount: f32,
    pub frequency: f32,
    pub phase: f32,
    pub center: [f32; 2],
    pub mosh: f32,
}

impl Default for Warp {
    fn default() -> Warp {
        Warp::from_values(&PARAMS.iter().map(|p| p.default).collect::<Vec<_>>())
    }
}

impl Warp {
    /// From values in [`PARAMS`] order (missing ones take the default),
    /// each clamped to `0..1`.
    pub fn from_values(v: &[f32]) -> Warp {
        let at = |i: usize| v.get(i).copied().unwrap_or(PARAMS[i].default).clamp(0.0, 1.0);
        Warp {
            mode: at(0),
            amount: at(1),
            frequency: at(2),
            phase: at(3),
            center: [at(4), at(5)],
            mosh: at(6),
        }
    }

    /// From a node config object: each param under its own key.
    pub fn from_config(config: &serde_json::Value) -> Warp {
        let v: Vec<f32> = PARAMS
            .iter()
            .map(|p| config.get(p.name).and_then(serde_json::Value::as_f64).map(|x| x as f32).unwrap_or(p.default))
            .collect();
        Warp::from_values(&v)
    }

    /// Which pattern `mode` picks (index into [`MODES`]).
    pub fn pattern(&self) -> usize {
        ((self.mode * MODES.len() as f32) as usize).min(MODES.len() - 1)
    }

    /// The displacement at one control point `p` (uv), for a frame of
    /// `aspect` = w / h. See the module note for each pattern.
    pub fn point(&self, p: [f32; 2], aspect: f32) -> [f32; 2] {
        use std::f32::consts::{PI, TAU};
        if self.amount <= 0.0 {
            return [0.0, 0.0];
        }
        let a = self.amount * 0.15;
        let f = 0.5 + self.frequency * (MAX_CYCLES - 0.5);
        let ph = TAU * self.phase;
        let c = self.center;
        let r = [(p[0] - c[0]) * aspect, p[1] - c[1]];
        let len = (r[0] * r[0] + r[1] * r[1]).sqrt();
        match self.pattern() {
            0 => [-(p[0] - c[0]) * self.amount * 0.6, -(p[1] - c[1]) * self.amount * 0.6],
            1 => [0.0, a * (TAU * f * p[0] + ph).sin()],
            2 => [a * (TAU * f * p[1] + ph).sin(), 0.0],
            3 => {
                if len < 1e-6 {
                    return [0.0, 0.0];
                }
                let s = a * (TAU * f * len - ph).sin() / len;
                [r[0] * s / aspect, r[1] * s]
            }
            4 => {
                let fall = (1.0 - len / 0.75).max(0.0);
                let th = self.amount * PI * fall * fall;
                let (sn, cs) = th.sin_cos();
                let rot = [r[0] * cs - r[1] * sn, r[0] * sn + r[1] * cs];
                [(rot[0] - r[0]) / aspect, rot[1] - r[1]]
            }
            _ => [a * (TAU * f * (p[0] + p[1]) + ph).sin(), a * (TAU * f * (p[0] - p[1]) + ph).sin()],
        }
    }

    /// The whole mesh, row-major (`j * N + i`, `j` down).
    pub fn mesh(&self, aspect: f32) -> Vec<[f32; 2]> {
        let mut out = Vec::with_capacity(N * N);
        for j in 0..N {
            for i in 0..N {
                let p = [i as f32 / (N - 1) as f32, j as f32 / (N - 1) as f32];
                out.push(self.point(p, aspect));
            }
        }
        out
    }
}

/// Catmull-Rom weights for `t` in `0..1` over points `-1, 0, 1, 2`.
fn cr(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    ]
}

/// `D(uv)`: the mesh's Catmull-Rom interpolation at `uv` — the same sum
/// `warp.wgsl` does.
pub fn field(mesh: &[[f32; 2]], uv: [f32; 2]) -> [f32; 2] {
    let n = N as i32;
    let g = [uv[0].clamp(0.0, 1.0) * (N - 1) as f32, uv[1].clamp(0.0, 1.0) * (N - 1) as f32];
    let base = [(g[0].floor() as i32).min(n - 2), (g[1].floor() as i32).min(n - 2)];
    let (wx, wy) = (cr(g[0] - base[0] as f32), cr(g[1] - base[1] as f32));
    let mut d = [0.0f32; 2];
    for (b, wyb) in wy.iter().enumerate() {
        let j = (base[1] + b as i32 - 1).clamp(0, n - 1) as usize;
        for (a, wxa) in wx.iter().enumerate() {
            let i = (base[0] + a as i32 - 1).clamp(0, n - 1) as usize;
            let w = wxa * wyb;
            let m = mesh[j * N + i];
            d[0] += m[0] * w;
            d[1] += m[1] * w;
        }
    }
    d
}

/// Bilinear sample of an RGBA8 frame at uv (clamped to the edge), pixel
/// centres at `(x + 0.5) / w`.
fn sample(px: &[u8], w: u32, h: u32, uv: [f32; 2]) -> [f32; 4] {
    let x = (uv[0] * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let y = (uv[1] * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |x: u32, y: u32, c: usize| px[((y * w + x) * 4) as usize + c] as f32;
    let mut out = [0.0; 4];
    for (c, o) in out.iter_mut().enumerate() {
        let top = at(x0, y0, c) * (1.0 - fx) + at(x1, y0, c) * fx;
        let bot = at(x0, y1, c) * (1.0 - fx) + at(x1, y1, c) * fx;
        *o = top * (1.0 - fy) + bot * fy;
    }
    out
}

/// The CPU reference: `cur` warped (and moshed against `prev`, when given
/// and the same size). Amount 0 with mosh 0 returns `cur` byte for byte.
pub fn apply_cpu(warp: &Warp, cur: &[u8], prev: Option<&[u8]>, w: u32, h: u32) -> Vec<u8> {
    let len = (w as usize) * (h as usize) * 4;
    if w == 0 || h == 0 || cur.len() != len {
        return cur.to_vec();
    }
    let prev = prev.filter(|p| p.len() == len);
    if warp.amount <= 0.0 && (warp.mosh <= 0.0 || prev.is_none()) {
        return cur.to_vec();
    }
    let mesh = warp.mesh(w as f32 / h as f32);
    let mut out = vec![0u8; len];
    for y in 0..h {
        for x in 0..w {
            let uv = [(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32];
            let d = field(&mesh, uv);
            let at = [uv[0] + d[0], uv[1] + d[1]];
            let a = sample(cur, w, h, at);
            let c = match prev {
                Some(p) if warp.mosh > 0.0 => {
                    let b = sample(p, w, h, at);
                    [0, 1, 2, 3].map(|i| a[i] + (b[i] - a[i]) * warp.mosh)
                }
                _ => a,
            };
            let o = ((y * w + x) * 4) as usize;
            for i in 0..4 {
                out[o + i] = c[i].round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32) -> Vec<u8> {
        (0..w * h).flat_map(|i| [(i * 7 % 256) as u8, (i * 13 % 256) as u8, (i % w * 9) as u8, 255]).collect()
    }

    #[test]
    fn the_field_passes_through_every_mesh_point() {
        let w = Warp { mode: 0.6, amount: 0.8, ..Warp::default() };
        let mesh = w.mesh(1.5);
        for (j, i) in [(0, 0), (3, 5), (8, 8), (16, 16), (16, 0)] {
            let uv = [i as f32 / (N - 1) as f32, j as f32 / (N - 1) as f32];
            let d = field(&mesh, uv);
            let m = mesh[j * N + i];
            assert!((d[0] - m[0]).abs() < 1e-5 && (d[1] - m[1]).abs() < 1e-5, "{i},{j}: {d:?} vs {m:?}");
        }
    }

    #[test]
    fn amount_zero_is_the_identity_in_every_mode() {
        let px = frame(23, 17);
        for k in 0..MODES.len() {
            let w = Warp { mode: (k as f32 + 0.5) / 6.0, amount: 0.0, ..Warp::default() };
            assert!(w.mesh(1.0).iter().all(|d| *d == [0.0, 0.0]), "{}", MODES[k]);
            assert_eq!(apply_cpu(&w, &px, Some(&px), 23, 17), px);
            let on = Warp { amount: 0.7, ..w };
            assert_ne!(apply_cpu(&on, &px, None, 23, 17), px, "{} moves pixels", MODES[k]);
        }
    }

    #[test]
    fn modes_and_config_read_back() {
        assert_eq!(Warp { mode: 0.0, ..Warp::default() }.pattern(), 0);
        assert_eq!(Warp { mode: 1.0, ..Warp::default() }.pattern(), 5);
        let w = Warp::from_config(&serde_json::json!({"amount": 0.5, "mosh": 2.0}));
        assert_eq!((w.amount, w.mosh, w.center), (0.5, 1.0, [0.5, 0.5]));
    }

    #[test]
    fn mosh_pulls_from_the_previous_frame() {
        let cur = vec![0u8; 8 * 8 * 4];
        let prev = vec![200u8; 8 * 8 * 4];
        let w = Warp { mosh: 0.5, ..Warp::default() };
        assert!(apply_cpu(&w, &cur, Some(&prev), 8, 8).iter().all(|&b| b == 100));
        assert_eq!(apply_cpu(&w, &cur, None, 8, 8), cur, "no history yet: the frame itself");
    }
}
