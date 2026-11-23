//! Gaussian splats: loaders, bounds, transform, crop, decimation and EWA
//! projection.
//!
//! A splat is an anisotropic 3D Gaussian: a centre, three per-axis scales,
//! an orientation quaternion, an opacity and a colour. Splats are stored as
//! parallel arrays (struct of arrays, f32) so that a decimate or a projection
//! pass only touches the fields it needs.
//!
//! Three file formats are read:
//!
//! - `.ply`, binary little-endian, the INRIA 3DGS layout: `x y z`,
//!   `f_dc_0..2` (degree-0 spherical harmonics), `opacity`, `scale_0..2`,
//!   `rot_0..3` (where `rot_0` is w). Any other vertex property (`f_rest_*`,
//!   `nx`, ...) is skipped by its byte size, except `f_rest_*`, which carry
//!   the view-dependent spherical harmonics of degree 1-3 (see
//!   [`Splats::colour_from`]). A plain coloured point cloud
//!   (`x y z red green blue`) is also accepted; each point becomes a small
//!   opaque splat with scale 0.01 and identity rotation.
//! - `.splat`, the antimatter15 layout: 32-byte records of position (3 x f32),
//!   linear scale (3 x f32), RGBA (4 x u8, alpha is opacity) and rotation
//!   (4 x u8, `(v - 128) / 128`, in the order w, x, y, z).
//! - `.spz`, Niantic's gzip-compressed layout (versions 2 and 3): fixed-point
//!   positions, quantised opacity, colour, scale and rotation, and optional
//!   SH. See [`parse_spz`].
//!
//! Activations happen once, at load: `scale = exp(raw)`,
//! `opacity = sigmoid(raw)`, `rgb = clamp(0.5 + 0.28209479 * f_dc, 0, 1)`.
//! The quaternion is normalised and stored as `[x, y, z, w]`.
//!
//! Projection is standard 3DGS EWA. The 3D covariance is pushed through the
//! perspective Jacobian, a 0.3 px^2 low-pass term is added, and the 2x2
//! result is eigen-decomposed into the ellipse's 3-sigma half-axes. World
//! coordinates are metres, right-handed, with the camera looking down -Z.
//! Screen pixels have y pointing down.

use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
use glam::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4};

/// Largest splat count a loader accepts.
pub const MAX_SPLATS: usize = 20_000_000;

/// Pixels² added to the 2D covariance's diagonal (the 3DGS low-pass).
pub const LOW_PASS: f32 = 0.3;
/// Splats nearer the eye than this (metres) are culled.
pub const NEAR_CULL: f32 = 0.05;
/// Longest ellipse half-axis, pixels.
pub const MAX_AXIS_PX: f32 = 1024.0;
const SH_C0: f32 = 0.282_094_79;
const POINT_SCALE: f32 = 0.01;
const SH_C1: f32 = 0.488_602_5;
const SH_C2: [f32; 5] = [
    1.092_548_5,
    -1.092_548_5,
    0.315_391_57,
    -1.092_548_5,
    0.546_274_2,
];
const SH_C3: [f32; 7] = [
    -0.590_043_6,
    2.890_611_4,
    -0.457_045_8,
    0.373_176_33,
    -0.457_045_8,
    1.445_305_7,
    -0.590_043_6,
];

/// A set of splats, struct of arrays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Splats {
    pub pos: Vec<[f32; 3]>,
    pub scale: Vec<[f32; 3]>,
    /// Unit quaternions, `[x, y, z, w]`.
    pub rot: Vec<[f32; 4]>,
    /// Linear opacity in `0..=1`.
    pub opacity: Vec<f32>,
    /// Linear colour in `0..=1`: the degree-0 colour, view-independent.
    pub rgb: Vec<[f32; 3]>,
    /// Higher-order SH coefficients, degree 1-3 only, `sh_stride()` floats per
    /// splat. Per splat the layout is INRIA's: all R coefficients, then G, then
    /// B, each channel ordered by the SH basis index (without the DC term,
    /// which lives in `rgb`). Empty when `sh_degree` is 0.
    pub sh: Vec<f32>,
    pub sh_degree: u8,
}

/// A fixture beam as light on splats: a cone from `apex` along unit `dir`,
/// `cos_half` the cosine of its half-angle, cut at `length` (the first hit),
/// with `rgb` already scaled by the dimmer. Splats keep their baked colour;
/// a beam only brightens what it passes through (see [`beam_light`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeamLight {
    pub apex: [f32; 3],
    pub dir: [f32; 3],
    pub cos_half: f32,
    pub length: f32,
    pub rgb: [f32; 3],
}

/// How much of the cone's edge fades out, as a fraction of `1 - cos_half`.
pub const BEAM_EDGE: f32 = 0.35;
/// Gain of the beam on the splat's own colour, and of its colour added on top.
pub const BEAM_GAIN: f32 = 1.5;
pub const BEAM_ADD: f32 = 0.15;

/// The light at world point `p` from every beam: inside a cone, fading over
/// its edge and to half strength at its far end. Zero outside every cone.
pub fn beam_light(lights: &[BeamLight], p: Vec3) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for l in lights {
        let v = p - Vec3::from_array(l.apex);
        let along = v.dot(Vec3::from_array(l.dir));
        let dist = v.length();
        if along <= 0.0 || along > l.length || dist <= 0.0 {
            continue;
        }
        let cos = along / dist;
        if cos <= l.cos_half {
            continue;
        }
        let edge = ((cos - l.cos_half) / ((1.0 - l.cos_half) * BEAM_EDGE).max(1e-6)).min(1.0);
        let k = edge * (1.0 - 0.5 * along / l.length);
        for c in 0..3 {
            out[c] += l.rgb[c] * k;
        }
    }
    out
}

/// A baked colour under `light` (from [`beam_light`]): multiplied up by it,
/// plus a little of the light's own colour, clamped to 0..1.
pub fn lit(base: [f32; 3], light: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|c| (base[c] * (1.0 + BEAM_GAIN * light[c]) + BEAM_ADD * light[c]).clamp(0.0, 1.0))
}

/// A projected splat: a screen-space ellipse with its colour and depth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ellipse {
    /// Centre in pixels, y down.
    pub centre: [f32; 2],
    /// Half-axes in pixels at 3 sigma.
    pub axis_a: [f32; 2],
    pub axis_b: [f32; 2],
    /// View-space distance in front of the camera, positive.
    pub depth: f32,
    pub rgb: [f32; 3],
    pub alpha: f32,
}

impl Splats {
    fn with_room(n: usize) -> Splats {
        Splats {
            pos: Vec::with_capacity(n),
            scale: Vec::with_capacity(n),
            rot: Vec::with_capacity(n),
            opacity: Vec::with_capacity(n),
            rgb: Vec::with_capacity(n),
            sh: Vec::new(),
            sh_degree: 0,
        }
    }

    /// Floats of SH stored per splat: `3 * ((degree + 1)² - 1)`.
    pub fn sh_stride(&self) -> usize {
        sh_rest_count(self.sh_degree)
    }

    /// The colour of splat `i` seen from `eye` (same space as the positions).
    ///
    /// The view direction is `pos - eye`, the reference 3DGS convention. The
    /// degree-0 part is `rgb` itself (`rgb = 0.5 + C0 * f_dc`), so the result
    /// is `rgb` plus the higher-order terms, clamped to `0..=1`. With degree 0
    /// this is just `rgb[i]`. A splat at the eye keeps `rgb[i]`.
    pub fn colour_from(&self, i: usize, eye: Vec3) -> [f32; 3] {
        let base = self.rgb[i];
        let k = self.sh_stride() / 3;
        if k == 0 {
            return base;
        }
        let d = (Vec3::from_array(self.pos[i]) - eye).normalize_or_zero();
        if d == Vec3::ZERO {
            return base;
        }
        let b = sh_basis(self.sh_degree, d);
        let at = i * self.sh_stride();
        std::array::from_fn(|c| {
            let start = at + c * k;
            let rest: f32 = (0..k).map(|j| b[j] * self.sh[start + j]).sum();
            (base[c] + rest).clamp(0.0, 1.0)
        })
    }

    pub fn len(&self) -> usize {
        self.pos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pos.is_empty()
    }

    /// Axis-aligned bounds of the centres, as `(min, max)`. Empty sets give
    /// zeros.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        if self.pos.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for p in &self.pos {
            let v = Vec3::from_array(*p);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        (lo.to_array(), hi.to_array())
    }

    /// Applies `m` to every splat. Positions go through the full matrix,
    /// rotations by the normalised rotation part, and scales by the uniform
    /// factor `cbrt(|det|)`. Non-uniform scale and mirroring are not
    /// represented exactly.
    pub fn transform(&mut self, m: Mat4) {
        let lin = Mat3::from_mat4(m);
        let s = lin.determinant().abs().cbrt();
        let q_m = if s > 0.0 && s.is_finite() {
            Quat::from_mat3(&(lin * (1.0 / s))).normalize()
        } else {
            Quat::IDENTITY
        };
        for p in &mut self.pos {
            *p = m.transform_point3(Vec3::from_array(*p)).to_array();
        }
        for r in &mut self.rot {
            *r = (q_m * Quat::from_array(*r)).normalize().to_array();
        }
        for sc in &mut self.scale {
            *sc = (Vec3::from_array(*sc) * s).to_array();
        }
        // SH is left as it is. Rotating it needs Wigner-D matrices per degree,
        // which we do not build. Callers that evaluate colour after a
        // transform pass the eye through the inverse model rotation instead
        // (see `colour_from`), so the SH stays in file space.
    }

    /// Keeps the splats whose centre lies inside `min..=max` on every axis.
    pub fn crop(&mut self, min: [f32; 3], max: [f32; 3]) {
        let (lo, hi) = (Vec3::from_array(min), Vec3::from_array(max));
        let keep: Vec<usize> = (0..self.len())
            .filter(|&i| {
                let p = Vec3::from_array(self.pos[i]);
                p.cmpge(lo).all() && p.cmple(hi).all()
            })
            .collect();
        *self = self.select(&keep);
    }

    /// Returns at most `budget` splats, the most important ones, where
    /// importance is `opacity × scale volume`. Ties go to the lower index.
    /// Kept splats stay in their original order.
    pub fn decimate(&self, budget: usize) -> Splats {
        if budget >= self.len() {
            return self.clone();
        }
        let importance: Vec<f32> = (0..self.len())
            .map(|i| {
                let s = self.scale[i];
                self.opacity[i] * s[0] * s[1] * s[2]
            })
            .collect();
        let mut order: Vec<usize> = (0..self.len()).collect();
        order.sort_unstable_by(|&a, &b| importance[b].total_cmp(&importance[a]).then(a.cmp(&b)));
        order.truncate(budget);
        order.sort_unstable();
        self.select(&order)
    }

    fn select(&self, idx: &[usize]) -> Splats {
        let k = self.sh_stride();
        Splats {
            pos: idx.iter().map(|&i| self.pos[i]).collect(),
            scale: idx.iter().map(|&i| self.scale[i]).collect(),
            rot: idx.iter().map(|&i| self.rot[i]).collect(),
            opacity: idx.iter().map(|&i| self.opacity[i]).collect(),
            rgb: idx.iter().map(|&i| self.rgb[i]).collect(),
            sh: idx
                .iter()
                .flat_map(|&i| self.sh[i * k..(i + 1) * k].iter().copied())
                .collect(),
            sh_degree: self.sh_degree,
        }
    }

    /// The 3D covariance `R S Sᵀ Rᵀ` of splat `i`.
    pub fn covariance3(&self, i: usize) -> Mat3 {
        let r = Mat3::from_quat(Quat::from_array(self.rot[i]));
        let s = Mat3::from_diagonal(Vec3::from_array(self.scale[i]));
        let m = r * s;
        m * m.transpose()
    }

    /// Projects splat `i` to a screen ellipse. `None` when the splat is behind
    /// the near cull plane, or when its centre is more than one axis length
    /// outside the viewport. `viewport` is `[width, height]` in pixels.
    pub fn project(&self, i: usize, view: Mat4, proj: Mat4, viewport: [f32; 2]) -> Option<Ellipse> {
        let t = view.transform_point3(Vec3::from_array(self.pos[i]));
        if t.z > -NEAR_CULL {
            return None;
        }
        let clip = proj * Vec4::new(t.x, t.y, t.z, 1.0);
        if clip.w <= 0.0 {
            return None;
        }
        let [vw, vh] = viewport;
        let cx = (clip.x / clip.w * 0.5 + 0.5) * vw;
        let cy = (1.0 - (clip.y / clip.w * 0.5 + 0.5)) * vh;

        let fx = proj.x_axis.x * vw * 0.5;
        let fy = proj.y_axis.y * vh * 0.5;
        let d = -t.z;
        // Rows of the Jacobian, in pixels with y down. Orthographic has no
        // w divide, so its rows are constant (pixels per metre).
        let (j0, j1) = if is_ortho(proj) {
            (Vec3::new(fx, 0.0, 0.0), Vec3::new(0.0, -fy, 0.0))
        } else {
            (
                Vec3::new(fx / d, 0.0, fx * t.x / (d * d)),
                Vec3::new(0.0, -fy / d, -fy * t.y / (d * d)),
            )
        };

        let w = Mat3::from_mat4(view);
        let sig = w * self.covariance3(i) * w.transpose();
        let a = j0.dot(sig * j0) + LOW_PASS;
        let b = j0.dot(sig * j1);
        let c = j1.dot(sig * j1) + LOW_PASS;

        let mid = 0.5 * (a + c);
        let rad = (mid * mid - (a * c - b * b)).max(0.0).sqrt();
        let l1 = mid + rad;
        let l2 = (mid - rad).max(0.0);
        let v = if b.abs() > 1e-12 {
            Vec2::new(b, l1 - a).normalize()
        } else if a >= c {
            Vec2::X
        } else {
            Vec2::Y
        };
        let axis_a = clamp_len(v * (3.0 * l1.sqrt()), MAX_AXIS_PX);
        let axis_b = clamp_len(Vec2::new(-v.y, v.x) * (3.0 * l2.sqrt()), MAX_AXIS_PX);

        let r = axis_a.length().max(axis_b.length());
        if cx < -r || cx > vw + r || cy < -r || cy > vh + r {
            return None;
        }
        Some(Ellipse {
            centre: [cx, cy],
            axis_a: axis_a.to_array(),
            axis_b: axis_b.to_array(),
            depth: d,
            rgb: self.rgb[i],
            alpha: self.opacity[i],
        })
    }
}

/// True for an orthographic projection: its last row is `(0, 0, 0, 1)`, with
/// no w divide. Perspective has `w_axis.w == 0`.
fn is_ortho(proj: Mat4) -> bool {
    proj.w_axis.w == 1.0 && proj.z_axis.w == 0.0
}

/// SH floats per splat for `degree` (0 for none): `3 * ((degree + 1)² - 1)`.
pub fn sh_rest_count(degree: u8) -> usize {
    3 * ((degree as usize + 1).pow(2) - 1)
}

/// The SH basis functions of degree 1..=`deg` at unit direction `d`, the
/// reference 3DGS form. Entry `j` is basis function `j + 1` (the DC term is
/// not here). Unused entries are zero.
fn sh_basis(deg: u8, d: Vec3) -> [f32; 15] {
    let (x, y, z) = (d.x, d.y, d.z);
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, yz, xz) = (x * y, y * z, x * z);
    let mut b = [0.0f32; 15];
    if deg >= 1 {
        b[0] = -SH_C1 * y;
        b[1] = SH_C1 * z;
        b[2] = -SH_C1 * x;
    }
    if deg >= 2 {
        b[3] = SH_C2[0] * xy;
        b[4] = SH_C2[1] * yz;
        b[5] = SH_C2[2] * (2.0 * zz - xx - yy);
        b[6] = SH_C2[3] * xz;
        b[7] = SH_C2[4] * (xx - yy);
    }
    if deg >= 3 {
        b[8] = SH_C3[0] * y * (3.0 * xx - yy);
        b[9] = SH_C3[1] * xy * z;
        b[10] = SH_C3[2] * y * (4.0 * zz - xx - yy);
        b[11] = SH_C3[3] * z * (2.0 * zz - 3.0 * xx - 3.0 * yy);
        b[12] = SH_C3[4] * x * (4.0 * zz - xx - yy);
        b[13] = SH_C3[5] * z * (xx - yy);
        b[14] = SH_C3[6] * x * (xx - 3.0 * yy);
    }
    b
}

fn clamp_len(v: Vec2, max: f32) -> Vec2 {
    let l = v.length();
    if l > max {
        v * (max / l)
    } else {
        v
    }
}

/// Indices of `depths`, farthest first. Equal depths keep index order.
/// A 4-pass LSD radix sort on the float bit pattern, so 1M entries take a few
/// milliseconds in release.
pub fn sort_back_to_front(depths: &[f32]) -> Vec<u32> {
    let mut keys: Vec<(u32, u32)> = depths
        .iter()
        .enumerate()
        .map(|(i, &d)| {
            let b = d.to_bits();
            let sortable = if b >> 31 == 1 { !b } else { b | 0x8000_0000 };
            (!sortable, i as u32)
        })
        .collect();
    let mut tmp = vec![(0u32, 0u32); keys.len()];
    for shift in (0..32).step_by(8) {
        let mut counts = [0usize; 256];
        for &(k, _) in &keys {
            counts[((k >> shift) & 0xff) as usize] += 1;
        }
        let mut starts = [0usize; 256];
        let mut acc = 0;
        for (s, c) in starts.iter_mut().zip(counts.iter()) {
            *s = acc;
            acc += c;
        }
        for &e in &keys {
            let b = ((e.0 >> shift) & 0xff) as usize;
            tmp[starts[b]] = e;
            starts[b] += 1;
        }
        std::mem::swap(&mut keys, &mut tmp);
    }
    keys.into_iter().map(|(_, i)| i).collect()
}

/// Loads a `.ply`, `.splat` or `.spz` file, dispatching on the extension.
pub fn load(path: &Path) -> Result<Splats, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    from_bytes(&path.to_string_lossy(), &bytes)
}

/// Parses splat bytes, dispatching on the extension of `name`. The browser
/// has no file system, so the bytes come from the in-memory tree instead.
pub fn from_bytes(name: &str, bytes: &[u8]) -> Result<Splats, String> {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("ply") => parse_ply(bytes),
        Some("splat") => parse_splat(bytes),
        Some("spz") => parse_spz(bytes),
        _ => Err(format!(
            "{name}: unsupported splat file (want .ply, .splat or .spz)"
        )),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Ty {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Ty {
    fn parse(s: &str) -> Option<Ty> {
        Some(match s {
            "char" | "int8" => Ty::I8,
            "uchar" | "uint8" => Ty::U8,
            "short" | "int16" => Ty::I16,
            "ushort" | "uint16" => Ty::U16,
            "int" | "int32" => Ty::I32,
            "uint" | "uint32" => Ty::U32,
            "float" | "float32" => Ty::F32,
            "double" | "float64" => Ty::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Ty::I8 | Ty::U8 => 1,
            Ty::I16 | Ty::U16 => 2,
            Ty::I32 | Ty::U32 | Ty::F32 => 4,
            Ty::F64 => 8,
        }
    }

    /// Reads one little-endian value from the front of `b`.
    fn read(self, b: &[u8]) -> f32 {
        match self {
            Ty::I8 => b[0] as i8 as f32,
            Ty::U8 => b[0] as f32,
            Ty::I16 => i16::from_le_bytes([b[0], b[1]]) as f32,
            Ty::U16 => u16::from_le_bytes([b[0], b[1]]) as f32,
            Ty::I32 => i32::from_le_bytes(b[..4].try_into().unwrap()) as f32,
            Ty::U32 => u32::from_le_bytes(b[..4].try_into().unwrap()) as f32,
            Ty::F32 => f32::from_le_bytes(b[..4].try_into().unwrap()),
            Ty::F64 => f64::from_le_bytes(b[..8].try_into().unwrap()) as f32,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Field {
    off: usize,
    ty: Ty,
}

fn at(data: &[u8], stride: usize, row: usize, f: Field) -> f32 {
    f.ty.read(&data[row * stride + f.off..])
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn normalise_quat(q: [f32; 4]) -> [f32; 4] {
    let len = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    if len > 1e-12 && len.is_finite() {
        q.map(|c| c / len)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// Parses a binary little-endian PLY. Accepts the INRIA gaussian layout or a
/// plain coloured point cloud. Only the first `element vertex` block is read.
pub fn parse_ply(bytes: &[u8]) -> Result<Splats, String> {
    const END: &[u8] = b"end_header\n";
    let end = bytes
        .windows(END.len())
        .position(|w| w == END)
        .ok_or("PLY: no end_header line")?;
    let header = std::str::from_utf8(&bytes[..end]).map_err(|_| "PLY: header is not text")?;
    let data = &bytes[end + END.len()..];

    let mut lines = header.lines();
    if lines.next().map(str::trim) != Some("ply") {
        return Err("PLY: missing 'ply' magic line".into());
    }
    let mut binary_le = false;
    let mut count: Option<usize> = None;
    let mut in_vertex = false;
    let mut props: Vec<(&str, Ty)> = Vec::new();
    for line in lines {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("format") => match words.next() {
                Some("binary_little_endian") => binary_le = true,
                Some("ascii") => {
                    return Err("PLY: ascii files are not supported, export as binary_little_endian".into())
                }
                Some("binary_big_endian") => {
                    return Err("PLY: big-endian files are not supported, export as binary_little_endian".into())
                }
                other => return Err(format!("PLY: unknown format {other:?}")),
            },
            Some("element") => {
                let name = words.next();
                let n: usize = words
                    .next()
                    .and_then(|w| w.parse().ok())
                    .ok_or("PLY: bad element line")?;
                in_vertex = name == Some("vertex");
                if in_vertex {
                    count = Some(n);
                } else if count.is_none() {
                    return Err(format!(
                        "PLY: element '{}' before the vertex element is not supported",
                        name.unwrap_or("?")
                    ));
                }
            }
            Some("property") if in_vertex => {
                let ty = words.next().unwrap_or("");
                let t = Ty::parse(ty).ok_or_else(|| {
                    format!("PLY: unsupported vertex property type '{ty}' (lists are not supported)")
                })?;
                let name = words.next().ok_or("PLY: property without a name")?;
                props.push((name, t));
            }
            _ => {}
        }
    }
    if !binary_le {
        return Err("PLY: no binary_little_endian format line".into());
    }
    let n = count.ok_or("PLY: no vertex element")?;
    if n > MAX_SPLATS {
        return Err(format!("PLY: {n} vertices exceeds the cap of {MAX_SPLATS}"));
    }

    let mut offs = Vec::with_capacity(props.len());
    let mut stride = 0usize;
    for (_, t) in &props {
        offs.push(stride);
        stride += t.size();
    }
    let need = n
        .checked_mul(stride)
        .ok_or("PLY: vertex data size overflows")?;
    if data.len() < need {
        return Err(format!(
            "PLY: truncated data, need {need} bytes, have {}",
            data.len()
        ));
    }

    let field = |name: &str| -> Option<Field> {
        props
            .iter()
            .position(|(p, _)| *p == name)
            .map(|k| Field { off: offs[k], ty: props[k].1 })
    };
    let req = |name: &str| field(name).ok_or_else(|| format!("PLY: missing property '{name}'"));

    let xyz = [req("x")?, req("y")?, req("z")?];
    let mut out = Splats::with_room(n);
    if field("f_dc_0").is_some() {
        let dc = [req("f_dc_0")?, req("f_dc_1")?, req("f_dc_2")?];
        let op = req("opacity")?;
        let sc = [req("scale_0")?, req("scale_1")?, req("scale_2")?];
        let rt = [req("rot_0")?, req("rot_1")?, req("rot_2")?, req("rot_3")?];
        // f_rest_0.. are the SH coefficients in INRIA order (R, then G, then B).
        let mut rest = Vec::new();
        while let Some(f) = field(&format!("f_rest_{}", rest.len())) {
            rest.push(f);
        }
        let degree = (0..=3u8)
            .find(|&d| sh_rest_count(d) == rest.len())
            .ok_or_else(|| {
                format!(
                    "PLY: {} f_rest_* properties is not 0, 9, 24 or 45 (degree 0-3)",
                    rest.len()
                )
            })?;
        for row in 0..n {
            let g = |f: Field| at(data, stride, row, f);
            out.pos.push(xyz.map(g));
            out.rgb.push(dc.map(|f| (0.5 + SH_C0 * g(f)).clamp(0.0, 1.0)));
            out.opacity.push(sigmoid(g(op)));
            out.scale.push(sc.map(|f| g(f).exp()));
            // INRIA stores the quaternion as w, x, y, z.
            out.rot.push(normalise_quat([g(rt[1]), g(rt[2]), g(rt[3]), g(rt[0])]));
            out.sh.extend(rest.iter().map(|&f| g(f)));
        }
        out.sh_degree = degree;
    } else if field("red").is_some() {
        let col = [req("red")?, req("green")?, req("blue")?];
        let k = col.map(|f| if f.ty == Ty::U8 { 1.0 / 255.0 } else { 1.0 });
        for row in 0..n {
            let g = |f: Field| at(data, stride, row, f);
            out.pos.push(xyz.map(g));
            out.rgb.push(std::array::from_fn(|c| (g(col[c]) * k[c]).clamp(0.0, 1.0)));
            out.scale.push([POINT_SCALE; 3]);
            out.opacity.push(1.0);
            out.rot.push([0.0, 0.0, 0.0, 1.0]);
        }
    } else {
        return Err("PLY: no gaussian fields (f_dc_0) and no colour (red green blue)".into());
    }
    Ok(out)
}

/// Parses a gzip-compressed `.spz` file (format versions 2 and 3).
///
/// Header (16 bytes, little-endian): magic `0x5053474e`, version, `numPoints`,
/// `shDegree`, `fractionalBits`, flags, reserved. Then whole-file arrays, one
/// entry per point:
///
/// - positions: 3 x 24-bit two's complement, divided by `1 << fractionalBits`;
/// - alphas: `u8`, `sigmoid(opacity) * 255`;
/// - colours: 3 x `u8`, `f_dc = (b / 255 - 0.5) / 0.15`, then the usual
///   `0.5 + C0 * f_dc`;
/// - scales: 3 x `u8`, `log(scale) = b / 16 - 10`;
/// - rotations: v2 is 3 x `u8` (xyz, w >= 0 derived); v3 is one `u32` in the
///   "smallest three" form: the top 2 bits index the largest component, and the
///   other three are 9-bit magnitudes over 511 times `1 / sqrt 2` plus a sign
///   bit, low bits holding the highest remaining component (w, z, y, x);
/// - SH: `3 * ((deg + 1)² - 1)` x `u8`, `(b - 128) / 128`, channel innermost
///   (r, g, b per coefficient); stored transposed into [`Splats::sh`] order.
///
/// Field orders follow Niantic's reference loader (load-spz.cc).
/// Coordinates are taken as stored.
pub fn parse_spz(bytes: &[u8]) -> Result<Splats, String> {
    const MAGIC: u32 = 0x5053_474e;
    let mut raw = Vec::new();
    GzDecoder::new(bytes)
        .read_to_end(&mut raw)
        .map_err(|e| format!(".spz: not gzip-compressed: {e}"))?;
    if raw.len() < 16 {
        return Err(".spz: shorter than its 16-byte header".into());
    }
    let u32_at = |k: usize| u32::from_le_bytes(raw[k..k + 4].try_into().unwrap());
    if u32_at(0) != MAGIC {
        return Err(".spz: bad magic".into());
    }
    let version = u32_at(4);
    if version != 2 && version != 3 {
        return Err(format!(".spz: version {version} is not 2 or 3"));
    }
    let n = u32_at(8) as usize;
    let degree = raw[12];
    let frac = raw[13] as i32;
    if degree > 3 {
        return Err(format!(".spz: SH degree {degree} is above 3"));
    }
    if n > MAX_SPLATS {
        return Err(format!(".spz: {n} points exceeds the cap of {MAX_SPLATS}"));
    }
    let sh_n = sh_rest_count(degree);
    let rot_n = if version == 2 { 3 } else { 4 };
    let sizes = [9, 1, 3, 3, rot_n, sh_n];
    let need = sizes
        .iter()
        .try_fold(16usize, |acc, &s| n.checked_mul(s).and_then(|b| acc.checked_add(b)))
        .ok_or(".spz: size overflows")?;
    if raw.len() < need {
        return Err(format!(".spz: truncated, need {need} bytes, have {}", raw.len()));
    }
    let mut at = [0usize; 6];
    let mut o = 16;
    for (k, &s) in sizes.iter().enumerate() {
        at[k] = o;
        o += n * s;
    }
    let [pos_b, alpha_b, col_b, scale_b, rot_b, sh_b] = at;
    let scale_fr = 2f32.powi(-frac);

    let mut out = Splats::with_room(n);
    for i in 0..n {
        let p = &raw[pos_b + i * 9..pos_b + i * 9 + 9];
        let xyz = std::array::from_fn(|c| {
            let v = (p[c * 3] as u32) | (p[c * 3 + 1] as u32) << 8 | (p[c * 3 + 2] as u32) << 16;
            (((v << 8) as i32) >> 8) as f32 * scale_fr
        });
        out.pos.push(xyz);
        out.opacity.push(raw[alpha_b + i] as f32 / 255.0);
        let c = &raw[col_b + i * 3..col_b + i * 3 + 3];
        out.rgb.push(std::array::from_fn(|k| {
            let f_dc = (c[k] as f32 / 255.0 - 0.5) / 0.15;
            (0.5 + SH_C0 * f_dc).clamp(0.0, 1.0)
        }));
        let s = &raw[scale_b + i * 3..scale_b + i * 3 + 3];
        out.scale.push(std::array::from_fn(|k| (s[k] as f32 / 16.0 - 10.0).exp()));
        let r = &raw[rot_b + i * rot_n..rot_b + (i + 1) * rot_n];
        out.rot.push(if version == 2 {
            let xyz = [0, 1, 2].map(|k| (r[k] as f32 - 127.5) / 127.5);
            let w = (1.0 - xyz.iter().map(|v| v * v).sum::<f32>()).max(0.0).sqrt();
            normalise_quat([xyz[0], xyz[1], xyz[2], w])
        } else {
            smallest_three(u32::from_le_bytes(r.try_into().unwrap()))
        });
        // spz interleaves channels per coefficient (r, g, b, r, g, b, ...);
        // `Splats::sh` keeps PLY order (all r, then g, then b).
        let src = &raw[sh_b + i * sh_n..sh_b + (i + 1) * sh_n];
        let k = sh_n / 3;
        out.sh.extend((0..3).flat_map(|c| (0..k).map(move |j| (src[j * 3 + c] as f32 - 128.0) / 128.0)));
    }
    out.sh_degree = degree;
    Ok(out)
}

/// Decodes the spz v3 "smallest three" quaternion, returned as `[x, y, z, w]`.
fn smallest_three(packed: u32) -> [f32; 4] {
    // As Niantic's load-spz.cc: components are read from w down to x, low bits
    // first; each is 9 bits of magnitude over 511 plus a sign bit.
    let big = (packed >> 30) as usize;
    let mut comp = packed;
    let mut q = [0.0f32; 4];
    for c in (0..4).rev() {
        if c == big {
            continue;
        }
        let mag = (comp & 0x1ff) as f32 / 511.0 * std::f32::consts::FRAC_1_SQRT_2;
        q[c] = if (comp >> 9) & 1 == 1 { -mag } else { mag };
        comp >>= 10;
    }
    let rest: f32 = q.iter().map(|v| v * v).sum();
    q[big] = (1.0 - rest).max(0.0).sqrt();
    normalise_quat(q)
}

/// Parses antimatter15 `.splat` records (32 bytes each).
pub fn parse_splat(bytes: &[u8]) -> Result<Splats, String> {
    const REC: usize = 32;
    if bytes.len() % REC != 0 {
        return Err(format!(
            ".splat: length {} is not a multiple of 32",
            bytes.len()
        ));
    }
    let n = bytes.len() / REC;
    if n > MAX_SPLATS {
        return Err(format!(".splat: {n} records exceeds the cap of {MAX_SPLATS}"));
    }
    let mut out = Splats::with_room(n);
    for rec in bytes.chunks_exact(REC) {
        let f = |k: usize| f32::from_le_bytes(rec[k..k + 4].try_into().unwrap());
        out.pos.push([f(0), f(4), f(8)]);
        out.scale.push([f(12), f(16), f(20)]);
        out.rgb
            .push([rec[24], rec[25], rec[26]].map(|c| c as f32 / 255.0));
        out.opacity.push(rec[27] as f32 / 255.0);
        // Bytes 28..32 are w, x, y, z.
        let r = [rec[28], rec[29], rec[30], rec[31]].map(|c| (c as f32 - 128.0) / 128.0);
        out.rot.push(normalise_quat([r[1], r[2], r[3], r[0]]));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_4;

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    fn f32le(out: &mut Vec<u8>, v: f32) {
        out.extend_from_slice(&v.to_le_bytes());
    }

    /// A binary PLY with one `element vertex` block and the given body.
    fn ply(n: usize, props: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        let mut h = format!("ply\nformat binary_little_endian 1.0\ncomment test\nelement vertex {n}\n");
        for (ty, name) in props {
            h.push_str(&format!("property {ty} {name}\n"));
        }
        h.push_str("end_header\n");
        let mut out = h.into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn push_splat(sp: &mut Splats, x: f32, s: f32, o: f32) {
        sp.pos.push([x, 0.0, 0.0]);
        sp.scale.push([s, s, s]);
        sp.rot.push([0.0, 0.0, 0.0, 1.0]);
        sp.opacity.push(o);
        sp.rgb.push([1.0, 1.0, 1.0]);
    }

    #[test]
    fn splat_ply_inria_fields_decode() {
        let props = [
            ("float", "x"), ("float", "y"), ("float", "z"),
            ("float", "nx"), ("float", "ny"), ("float", "nz"),
            ("float", "f_dc_0"), ("float", "f_dc_1"), ("float", "f_dc_2"),
            ("float", "f_rest_0"), ("float", "f_rest_1"), ("double", "f_rest_2"),
            ("float", "f_rest_3"), ("float", "f_rest_4"), ("float", "f_rest_5"),
            ("float", "f_rest_6"), ("float", "f_rest_7"), ("float", "f_rest_8"),
            ("uchar", "junk"),
            ("float", "opacity"),
            ("float", "scale_0"), ("float", "scale_1"), ("float", "scale_2"),
            ("float", "rot_0"), ("float", "rot_1"), ("float", "rot_2"), ("float", "rot_3"),
        ];
        let mut body = Vec::new();
        // Splat A.
        for v in [1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 0.0, 1.0, -1.0, 9.0, 9.0] {
            f32le(&mut body, v);
        }
        body.extend_from_slice(&7.0f64.to_le_bytes());
        for v in [10.0, 11.0, 12.0, 13.0, 14.0, 15.0] {
            f32le(&mut body, v);
        }
        body.push(200);
        f32le(&mut body, 0.0); // opacity -> 0.5
        for v in [std::f32::consts::LN_2, 0.0, 3.0f32.ln()] {
            f32le(&mut body, v);
        }
        for v in [1.0, 1.0, 0.0, 0.0] {
            f32le(&mut body, v); // w, x, y, z
        }
        // Splat B.
        for v in [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 10.0, -10.0, 0.0, 0.0, 0.0] {
            f32le(&mut body, v);
        }
        body.extend_from_slice(&0.0f64.to_le_bytes());
        for _ in 0..6 {
            f32le(&mut body, 0.0);
        }
        body.push(0);
        f32le(&mut body, 2.0);
        for v in [0.0, 0.0, 0.0] {
            f32le(&mut body, v);
        }
        for v in [0.0, 0.0, 0.0, 0.0] {
            f32le(&mut body, v); // degenerate quaternion
        }

        let sp = parse_ply(&ply(2, &props, &body)).unwrap();
        assert_eq!(sp.len(), 2);
        assert_eq!(sp.pos[0], [1.0, 2.0, 3.0]);
        assert!(sp.scale[0].iter().zip([2.0, 1.0, 3.0]).all(|(a, b)| close(*a, b, 1e-5)));
        assert!(close(sp.opacity[0], 0.5, 1e-6));
        assert!(close(sp.rgb[0][0], 0.5, 1e-6));
        assert!(close(sp.rgb[0][1], 0.78209479, 1e-6));
        assert!(close(sp.rgb[0][2], 0.21790521, 1e-6));
        // Quaternion (w, x, y, z) = (1, 1, 0, 0) is stored as [x, y, z, w].
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!(close(sp.rot[0][0], h, 1e-6));
        assert!(close(sp.rot[0][1], 0.0, 1e-6));
        assert!(close(sp.rot[0][2], 0.0, 1e-6));
        assert!(close(sp.rot[0][3], h, 1e-6));
        // Splat B: clamped colour, sigmoid(2), identity for a zero quaternion.
        assert_eq!(sp.rgb[1], [1.0, 0.0, 0.5]);
        assert!(close(sp.opacity[1], 1.0 / (1.0 + (-2.0f32).exp()), 1e-6));
        assert_eq!(sp.rot[1], [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(sp.scale[1], [1.0, 1.0, 1.0]);
        // Nine f_rest values make degree 1; they are kept in file order.
        assert_eq!(sp.sh_degree, 1);
        assert_eq!(sp.sh.len(), 2 * 9);
        assert_eq!(&sp.sh[..9], &[9.0, 9.0, 7.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0]);
        assert!(sp.sh[9..].iter().all(|&v| v == 0.0));
    }

    #[test]
    fn splat_ply_bad_rest_count_rejected() {
        let props = [
            ("float", "x"), ("float", "y"), ("float", "z"),
            ("float", "f_dc_0"), ("float", "f_dc_1"), ("float", "f_dc_2"),
            ("float", "f_rest_0"), ("float", "f_rest_1"), ("float", "f_rest_2"),
            ("float", "opacity"),
            ("float", "scale_0"), ("float", "scale_1"), ("float", "scale_2"),
            ("float", "rot_0"), ("float", "rot_1"), ("float", "rot_2"), ("float", "rot_3"),
        ];
        let body = vec![0u8; 17 * 4];
        let err = parse_ply(&ply(1, &props, &body)).unwrap_err();
        assert!(err.contains("f_rest_"), "{err}");
    }

    #[test]
    fn splat_colour_from_degree_zero_is_rgb() {
        let mut sp = Splats::default();
        push_splat(&mut sp, 0.0, 1.0, 1.0);
        sp.rgb[0] = [0.2, 0.4, 0.6];
        assert_eq!(sp.colour_from(0, Vec3::new(0.0, 0.0, 9.0)), [0.2, 0.4, 0.6]);
    }

    #[test]
    fn splat_colour_from_degree_one_follows_the_reference_basis() {
        // Splat at the origin, eye on +z: the view direction pos - eye is -z.
        // The only term that survives is C1 * z * the z coefficient (index 1).
        let mut sp = Splats::default();
        push_splat(&mut sp, 0.0, 1.0, 1.0);
        sp.rgb[0] = [0.5, 0.5, 0.5];
        sp.sh_degree = 1;
        sp.sh = vec![0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let c = sp.colour_from(0, Vec3::new(0.0, 0.0, 5.0));
        let want = 0.5 - SH_C1 * 0.5;
        assert!(close(c[0], want, 1e-6), "{c:?}");
        assert!(close(c[1], 0.5, 1e-6), "{c:?}");
        // Eye on -z flips the sign of the z term.
        let back = sp.colour_from(0, Vec3::new(0.0, 0.0, -5.0));
        assert!(close(back[0], 0.5 + SH_C1 * 0.5, 1e-6), "{back:?}");
        // Results clamp to 0..=1 on both sides.
        sp.sh[1] = -100.0;
        let hot = sp.colour_from(0, Vec3::new(0.0, 0.0, 5.0));
        assert_eq!(hot[0], 1.0);
        sp.sh[1] = 100.0;
        let cold = sp.colour_from(0, Vec3::new(0.0, 0.0, 5.0));
        assert_eq!(cold[0], 0.0);
    }

    #[test]
    fn splat_colour_from_degree_three_is_finite_and_directional() {
        let mut sp = Splats::default();
        push_splat(&mut sp, 0.0, 1.0, 1.0);
        sp.rgb[0] = [0.5; 3];
        sp.sh_degree = 3;
        sp.sh = (0..45).map(|k| ((k * 7) % 11) as f32 * 0.01 - 0.05).collect();
        for eye in [
            Vec3::new(3.0, 1.0, -2.0),
            Vec3::new(-1.0, 4.0, 2.0),
            Vec3::new(0.0, 0.0, 0.0),
        ] {
            let c = sp.colour_from(0, eye);
            assert!(c.iter().all(|v| (0.0..=1.0).contains(v)), "{c:?}");
        }
        assert_ne!(
            sp.colour_from(0, Vec3::new(3.0, 1.0, -2.0)),
            sp.colour_from(0, Vec3::new(-3.0, -1.0, 2.0))
        );
    }

    #[test]
    fn splat_sh_carried_by_crop_decimate_and_transform() {
        let mut sp = Splats::default();
        for (x, s) in [(0.0, 0.1), (1.0, 1.0), (2.0, 0.2)] {
            push_splat(&mut sp, x, s, 1.0);
        }
        // Degree 1: 9 floats per splat. Splat i's first float is 9 * i.
        sp.sh_degree = 1;
        sp.sh = (0..27).map(|k| k as f32).collect();
        let mut kept = sp.clone();
        kept.crop([0.5, -1.0, -1.0], [2.5, 1.0, 1.0]);
        assert_eq!(kept.sh_degree, 1);
        assert_eq!(kept.sh.len(), 2 * 9);
        assert_eq!(kept.sh[0], 9.0);
        assert_eq!(kept.sh[9], 18.0);
        // Decimate by importance: the big splat (index 1) is kept alone.
        let one = sp.decimate(1);
        assert_eq!(one.pos, vec![[1.0, 0.0, 0.0]]);
        assert_eq!(one.sh[0], 9.0);
        assert_eq!(one.sh.len(), 9);
        // Transform leaves SH in file space.
        let mut moved = sp.clone();
        moved.transform(Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0)));
        assert_eq!(moved.sh, sp.sh);
        assert_eq!(moved.pos[0], [5.0, 0.0, 0.0]);
    }

    /// One point of a synthetic spz file, already quantised.
    struct SpzPoint {
        fixed: [i32; 3],
        alpha: u8,
        col: [u8; 3],
        scale: [u8; 3],
        rot: Vec<u8>,
        sh: Vec<u8>,
    }

    /// Writes an spz file (header, planar arrays), then gzips it.
    fn spz_file(version: u32, degree: u8, frac: u8, pts: &[SpzPoint]) -> Vec<u8> {
        use flate2::{write::GzEncoder, Compression};
        use std::io::Write;
        let mut raw = Vec::new();
        raw.extend_from_slice(&0x5053_474eu32.to_le_bytes());
        raw.extend_from_slice(&version.to_le_bytes());
        raw.extend_from_slice(&(pts.len() as u32).to_le_bytes());
        raw.extend_from_slice(&[degree, frac, 0, 0]);
        for p in pts {
            for v in p.fixed {
                raw.extend_from_slice(&(v as u32).to_le_bytes()[..3]);
            }
        }
        raw.extend(pts.iter().map(|p| p.alpha));
        raw.extend(pts.iter().flat_map(|p| p.col));
        raw.extend(pts.iter().flat_map(|p| p.scale));
        raw.extend(pts.iter().flat_map(|p| p.rot.clone()));
        raw.extend(pts.iter().flat_map(|p| p.sh.clone()));
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&raw).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn splat_spz_v3_round_trip() {
        // 8 fractional bits: 1.0 is 256, -2.0 is -512, 0.5 is 128.
        let p0 = SpzPoint {
            fixed: [256, -512, 128],
            alpha: 128,
            col: [128, 128, 128],
            scale: [160, 160, 160],
            // Largest component w (index 3), the rest zero: identity.
            rot: 0xC000_0000u32.to_le_bytes().to_vec(),
            sh: (0..9).map(|k| if k == 1 { 255 } else { 128 }).collect(),
        };
        // Largest w = 0.8, x = 0.6 as magnitude 434 in the third slot (bits 20..30).
        let p1 = SpzPoint {
            fixed: [0, 0, 0],
            alpha: 255,
            col: [255, 0, 0],
            scale: [0, 0, 0],
            rot: ((3u32 << 30) | (434 << 20)).to_le_bytes().to_vec(),
            sh: vec![0; 9],
        };
        let bytes = spz_file(3, 1, 8, &[p0, p1]);
        let sp = from_bytes("scan.spz", &bytes).unwrap();
        assert_eq!(sp.len(), 2);
        assert_eq!(sp.pos[0], [1.0, -2.0, 0.5]);
        assert!(close(sp.opacity[0], 128.0 / 255.0, 1e-6));
        assert_eq!(sp.opacity[1], 1.0);
        let f_dc = (128.0 / 255.0 - 0.5) / 0.15;
        assert!(close(sp.rgb[0][0], 0.5 + SH_C0 * f_dc, 1e-6));
        assert!(close(sp.rgb[1][0], 1.0, 1e-6), "{:?}", sp.rgb[1]);
        assert!(close(sp.rgb[1][1], 0.0, 1e-6), "{:?}", sp.rgb[1]);
        assert!(close(sp.scale[0][0], 1.0, 1e-6));
        assert!(close(sp.scale[1][0], (-10.0f32).exp(), 1e-9));
        assert_eq!(sp.rot[0], [0.0, 0.0, 0.0, 1.0]);
        assert!(close(sp.rot[1][0], 0.6, 2e-3), "{:?}", sp.rot[1]);
        assert!(close(sp.rot[1][3], 0.8, 3e-3), "{:?}", sp.rot[1]);
        assert_eq!(sp.sh_degree, 1);
        assert_eq!(sp.sh.len(), 18);
        // Byte 1 is coefficient 0 of green, which lands at g * 3 + 0.
        assert!(close(sp.sh[3], 127.0 / 128.0, 1e-6));
        assert_eq!(sp.sh[1], 0.0);
        assert_eq!(sp.sh[9], -1.0);
    }

    #[test]
    fn splat_spz_v2_quaternion_and_degree_zero() {
        let p = SpzPoint {
            fixed: [-1, 2, 0],
            alpha: 0,
            col: [0, 255, 128],
            scale: [0, 0, 0],
            rot: vec![128, 128, 128],
            sh: Vec::new(),
        };
        let sp = from_bytes("old.spz", &spz_file(2, 0, 0, &[p])).unwrap();
        assert_eq!(sp.pos[0], [-1.0, 2.0, 0.0]);
        assert_eq!(sp.opacity[0], 0.0);
        assert!(sp.sh.is_empty());
        assert_eq!(sp.sh_degree, 0);
        let q = sp.rot[0];
        assert!(close(q.iter().map(|c| c * c).sum::<f32>(), 1.0, 1e-5));
        assert!(q[3] > 0.99, "{q:?}");
        // Byte 0 and 255 are far outside the colour range: they clamp.
        assert_eq!(sp.rgb[0][0], 0.0);
        assert_eq!(sp.rgb[0][1], 1.0);
    }

    #[test]
    fn splat_spz_rejects_bad_input() {
        let p = SpzPoint {
            fixed: [0; 3],
            alpha: 0,
            col: [0; 3],
            scale: [0; 3],
            rot: vec![128; 4],
            sh: Vec::new(),
        };
        let good = spz_file(3, 0, 0, &[p]);
        assert!(parse_spz(&good[..0]).is_err());
        assert!(parse_spz(b"not gzip at all").is_err());
        // Version 4 and truncated payloads are refused.
        let v4 = spz_file(4, 0, 0, &[SpzPoint { fixed: [0; 3], alpha: 0, col: [0; 3], scale: [0; 3], rot: vec![128; 4], sh: Vec::new() }]);
        assert!(parse_spz(&v4).unwrap_err().contains("version 4"));
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(&good[..]).read_to_end(&mut raw).unwrap();
        raw.truncate(20);
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut enc, &raw).unwrap();
        let short = enc.finish().unwrap();
        assert!(parse_spz(&short).unwrap_err().contains("truncated"));
    }

    #[test]
    fn splat_from_bytes_dispatches_on_the_name() {
        let mut bytes = Vec::new();
        for v in [1.0f32, 2.0, 3.0, 0.1, 0.1, 0.1] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.extend_from_slice(&[255, 255, 255, 255, 255, 128, 128, 128]);
        assert_eq!(from_bytes("a.SPLAT", &bytes).unwrap().len(), 1);
        assert!(from_bytes("a.txt", &bytes).is_err());
    }

    #[test]
    fn splat_antimatter_round_trip() {
        let mut bytes = Vec::new();
        for v in [1.0, 2.0, 3.0, 0.5, 0.25, 2.0] {
            f32le(&mut bytes, v);
        }
        bytes.extend_from_slice(&[255, 0, 128, 64]);
        bytes.extend_from_slice(&[192, 192, 128, 128]); // w, x, y, z
        for _ in 0..32 {
            bytes.push(0);
        }
        bytes.truncate(64);
        // Second record is all zero except rgba, rotation bytes all 128.
        let sp = parse_splat(&bytes).unwrap();
        assert_eq!(sp.len(), 2);
        assert_eq!(sp.pos[0], [1.0, 2.0, 3.0]);
        assert_eq!(sp.scale[0], [0.5, 0.25, 2.0]);
        assert!(close(sp.rgb[0][0], 1.0, 1e-6));
        assert!(close(sp.rgb[0][1], 0.0, 1e-6));
        assert!(close(sp.rgb[0][2], 128.0 / 255.0, 1e-6));
        assert!(close(sp.opacity[0], 64.0 / 255.0, 1e-6));
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!(close(sp.rot[0][0], h, 1e-5));
        assert!(close(sp.rot[0][3], h, 1e-5));
        assert!(close(sp.rot[0][1], 0.0, 1e-5));
        assert!(parse_splat(&bytes[..33]).is_err());
    }

    #[test]
    fn splat_point_cloud_ply() {
        let props = [("float", "x"), ("float", "y"), ("float", "z"), ("uchar", "red"), ("uchar", "green"), ("uchar", "blue")];
        let mut body = Vec::new();
        for v in [0.0, 0.0, 0.0] {
            f32le(&mut body, v);
        }
        body.extend_from_slice(&[255, 0, 128]);
        for v in [1.0, 2.0, 3.0] {
            f32le(&mut body, v);
        }
        body.extend_from_slice(&[0, 255, 0]);
        let sp = parse_ply(&ply(2, &props, &body)).unwrap();
        assert_eq!(sp.len(), 2);
        assert_eq!(sp.pos[1], [1.0, 2.0, 3.0]);
        assert_eq!(sp.scale[0], [POINT_SCALE; 3]);
        assert_eq!(sp.opacity, vec![1.0, 1.0]);
        assert_eq!(sp.rot[0], [0.0, 0.0, 0.0, 1.0]);
        assert!(close(sp.rgb[0][2], 128.0 / 255.0, 1e-6));
        assert_eq!(sp.rgb[1], [0.0, 1.0, 0.0]);
    }

    #[test]
    fn splat_ascii_and_big_endian_rejected() {
        let ascii = b"ply\nformat ascii 1.0\nelement vertex 0\nproperty float x\nend_header\n";
        let err = parse_ply(ascii).unwrap_err();
        assert!(err.contains("ascii"), "{err}");
        let be = b"ply\nformat binary_big_endian 1.0\nelement vertex 0\nproperty float x\nend_header\n";
        let err = parse_ply(be).unwrap_err();
        assert!(err.contains("big-endian"), "{err}");
    }

    #[test]
    fn splat_decimate_keeps_largest() {
        let mut sp = Splats::default();
        // Volumes: 1, 0.001, 64, 0.125, 27.
        for (x, s) in [(0.0, 1.0), (1.0, 0.1), (2.0, 4.0), (3.0, 0.5), (4.0, 3.0)] {
            push_splat(&mut sp, x, s, 1.0);
        }
        let two = sp.decimate(2);
        assert_eq!(two.pos, vec![[2.0, 0.0, 0.0], [4.0, 0.0, 0.0]]);
        let one = sp.decimate(1);
        assert_eq!(one.pos, vec![[2.0, 0.0, 0.0]]);
        assert_eq!(sp.decimate(10).len(), 5);
        // Opacity matters too: a dim giant loses to an opaque medium one.
        let mut dim = Splats::default();
        push_splat(&mut dim, 0.0, 10.0, 0.01); // 10 volume units → 0.01 * 1000 = 10
        push_splat(&mut dim, 1.0, 2.0, 1.0); // 8 * 1 = 8
        assert_eq!(dim.decimate(1).pos, vec![[0.0, 0.0, 0.0]]);
    }

    #[test]
    fn splat_unit_projects_to_centre() {
        let mut sp = Splats::default();
        push_splat(&mut sp, 0.0, 1.0, 1.0);
        sp.pos[0] = [0.0, 0.0, -5.0];
        let proj = Mat4::perspective_rh_gl(60f32.to_radians(), 1.0, 0.1, 100.0);
        let e = sp.project(0, Mat4::IDENTITY, proj, [800.0, 800.0]).unwrap();
        assert!(close(e.centre[0], 400.0, 1e-2), "{:?}", e.centre);
        assert!(close(e.centre[1], 400.0, 1e-2), "{:?}", e.centre);
        assert!(close(e.depth, 5.0, 1e-4));
        // fx = (1 / tan 30°) * 400; 3 sigma of sqrt(fx/5)^2 + 0.3.
        let fx = 400.0 / 30f32.to_radians().tan();
        let expect = 3.0 * fx / 5.0;
        let la = (e.axis_a[0].powi(2) + e.axis_a[1].powi(2)).sqrt();
        let lb = (e.axis_b[0].powi(2) + e.axis_b[1].powi(2)).sqrt();
        assert!(close(la, expect, 0.5), "{la} vs {expect}");
        assert!(close(la, lb, 1e-2));
        assert!(close(e.axis_a[0] * e.axis_b[0] + e.axis_a[1] * e.axis_b[1], 0.0, 1e-2));
    }

    #[test]
    fn splat_ortho_ellipse_matches_world_size_times_scale() {
        let mut sp = Splats::default();
        push_splat(&mut sp, 0.0, 0.5, 1.0);
        sp.pos[0] = [0.0, 0.0, -5.0];
        let (hw, hh) = (4.0f32, 3.0f32);
        let proj = Mat4::orthographic_rh(-hw, hw, -hh, hh, 0.1, 100.0);
        let vp = [800.0, 600.0];
        let e = sp.project(0, Mat4::IDENTITY, proj, vp).unwrap();
        assert!(close(e.centre[0], 400.0, 1e-2), "{:?}", e.centre);
        assert!(close(e.centre[1], 300.0, 1e-2), "{:?}", e.centre);
        // Pixels per metre along x: half the width maps to hw metres.
        let ppm = 400.0 / hw;
        // Isotropic splat of sigma 0.5 m: 3 sigma in pixels plus the low-pass term.
        let expect = 3.0 * (ppm * ppm * 0.25 + LOW_PASS).sqrt();
        let la = (e.axis_a[0].powi(2) + e.axis_a[1].powi(2)).sqrt();
        let lb = (e.axis_b[0].powi(2) + e.axis_b[1].powi(2)).sqrt();
        assert!(close(la, expect, 1e-3 * expect), "{la} vs {expect}");
        assert!(close(lb, expect, 1e-3 * expect), "{lb} vs {expect}");
        // Distance does not change the size in ortho.
        sp.pos[0] = [0.0, 0.0, -50.0];
        let far = sp.project(0, Mat4::IDENTITY, proj, vp).unwrap();
        let lf = (far.axis_a[0].powi(2) + far.axis_a[1].powi(2)).sqrt();
        assert!(close(lf, la, 1e-3 * la), "{lf} vs {la}");
    }

    #[test]
    fn splat_sort_farthest_first() {
        let order = sort_back_to_front(&[1.0, 5.0, 3.0, 5.0]);
        assert_eq!(order, vec![1, 3, 2, 0]);
        assert_eq!(sort_back_to_front(&[-1.0, 2.0, -3.0]), vec![1, 0, 2]);
        assert!(sort_back_to_front(&[]).is_empty());
    }

    #[test]
    fn splat_behind_camera_is_none() {
        let mut sp = Splats::default();
        push_splat(&mut sp, 0.0, 1.0, 1.0);
        let proj = Mat4::perspective_rh_gl(FRAC_PI_4, 1.0, 0.1, 100.0);
        sp.pos[0] = [0.0, 0.0, 5.0];
        assert!(sp.project(0, Mat4::IDENTITY, proj, [800.0, 800.0]).is_none());
        sp.pos[0] = [0.0, 0.0, -0.01];
        assert!(sp.project(0, Mat4::IDENTITY, proj, [800.0, 800.0]).is_none());
    }

    #[test]
    fn splat_crop_and_transform() {
        let mut sp = Splats::default();
        for x in [0.0, 1.0, 2.0, 3.0] {
            push_splat(&mut sp, x, 1.0, 1.0);
        }
        sp.crop([0.5, -1.0, -1.0], [2.5, 1.0, 1.0]);
        assert_eq!(sp.pos.iter().map(|p| p[0]).collect::<Vec<_>>(), vec![1.0, 2.0]);
        assert_eq!(sp.bounds(), ([1.0, 0.0, 0.0], [2.0, 0.0, 0.0]));

        let m = Mat4::from_scale_rotation_translation(Vec3::splat(2.0), Quat::IDENTITY, Vec3::X);
        sp.transform(m);
        assert_eq!(sp.pos[0], [3.0, 0.0, 0.0]);
        assert!(close(sp.scale[0][0], 2.0, 1e-5));
    }

    #[test]
    fn a_beam_lights_only_inside_its_cone_and_before_its_hit() {
        let l = BeamLight {
            apex: [0.0, 4.0, 0.0],
            dir: [0.0, -1.0, 0.0],
            cos_half: 15f32.to_radians().cos(),
            length: 4.0,
            rgb: [1.0, 0.0, 0.0],
        };
        let on_axis = beam_light(&[l], Vec3::new(0.0, 2.0, 0.0));
        assert!(on_axis[0] > 0.7 && on_axis[1] == 0.0, "{on_axis:?}");
        assert_eq!(beam_light(&[l], Vec3::new(2.0, 2.0, 0.0)), [0.0; 3], "outside the cone");
        assert_eq!(beam_light(&[l], Vec3::new(0.0, -1.0, 0.0)), [0.0; 3], "past the hit");
        assert_eq!(beam_light(&[l], Vec3::new(0.0, 5.0, 0.0)), [0.0; 3], "behind the apex");
        let near = beam_light(&[l], Vec3::new(0.0, 3.5, 0.0))[0];
        let far = beam_light(&[l], Vec3::new(0.0, 0.5, 0.0))[0];
        assert!(near > far, "fades with distance: {near} {far}");
        assert_eq!(lit([0.2, 0.2, 0.2], [0.0; 3]), [0.2, 0.2, 0.2]);
        let red = lit([0.2, 0.2, 0.2], on_axis);
        assert!(red[0] > 0.4 && red[1] == 0.2, "{red:?}");
    }
}
