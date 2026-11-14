//! Parity lote P-A: the engine side of `fx.chroma_key`, `fx.level`,
//! `fx.lookup`, `fx.math`, `fx.projection` and the `gen.ramp` source (the
//! catalog side is `metal_graph::parity`).
//!
//! **Two painters, one picture.** `fx.chroma_key`, `fx.level`, `fx.math` and
//! `fx.projection` are built-in effect shaders (`data/shaders/<id>.wgsl`)
//! run through the `fx.forge` pass (`metal_render::models::forge`'s effect
//! contract) on a host with an adapter; [`apply_cpu`] is the CPU stand-in's
//! honest version of the same per-pixel math, and the GPU painter falls back
//! to it if the shader cannot draw — the pairing `fx.particles` has. Each
//! node writes its floats into the ingest slots (`floats2.x` onward) in the
//! order [`ingest`] lists them; the shaders read them in that order.
//!
//! `fx.lookup` and `gen.ramp` are CPU on both painters: a `.cube` LUT and a
//! palette's colours need a second texture, which the forge effect contract
//! does not bind (the same reason `fx.image-pop` composes on the CPU).
//!
//! Colour math is in display (sRGB byte) space, like `fx.flash`; alpha is
//! straight (not premultiplied).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use metal_graph::parity as p;

/// A node's config, parsed once.
pub fn config(node: &metal_graph::Node) -> serde_json::Value {
    serde_json::from_str(&node.config.to_json_str()).unwrap_or_default()
}

/// The node's floats, in [`metal_graph::parity::floats_of`] order, each read
/// off its own config key and clamped to its declared range (default when
/// unset).
pub fn floats(kind: &str, cfg: &serde_json::Value) -> Vec<f32> {
    p::floats_of(kind)
        .unwrap_or_default()
        .iter()
        .map(|&(name, min, max, default)| {
            cfg.get(name).and_then(serde_json::Value::as_f64).map(|v| v as f32).unwrap_or(default).clamp(min, max)
        })
        .collect()
}

/// `"#rrggbb"` (or `"rrggbb"`) → `0..1` RGB.
pub fn hex_rgb(hex: &str) -> Option<[f32; 3]> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok().map(|b| b as f32 / 255.0);
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// The chroma key's colour: config `key`, default pure green.
pub fn key_colour(cfg: &serde_json::Value) -> [f32; 3] {
    cfg.get("key").and_then(serde_json::Value::as_str).and_then(hex_rgb).unwrap_or([0.0, 1.0, 0.0])
}

/// `fx.math`'s swizzle: config `swizzle`, four of `r g b a 0 1` (anything
/// else is the identity `rgba`). Each source is an index: 0..3 a channel,
/// 4 zero, 5 one.
pub fn swizzle(cfg: &serde_json::Value) -> [u8; 4] {
    let s = cfg.get("swizzle").and_then(serde_json::Value::as_str).unwrap_or("rgba");
    let idx = |c: char| match c {
        'r' | 'x' => Some(0),
        'g' | 'y' => Some(1),
        'b' | 'z' => Some(2),
        'a' | 'w' => Some(3),
        '0' => Some(4),
        '1' => Some(5),
        _ => None,
    };
    let v: Vec<u8> = s.to_ascii_lowercase().chars().filter_map(idx).collect();
    if s.chars().count() == 4 && v.len() == 4 {
        [v[0], v[1], v[2], v[3]]
    } else {
        [0, 1, 2, 3]
    }
}

/// The built-in shader an effect kind draws on the GPU, `None` for the
/// CPU-only ones.
pub fn shader_of(kind: &str) -> Option<&'static str> {
    match kind {
        p::CHROMA_KEY_KIND => Some("chroma_key"),
        p::LEVEL_KIND => Some("level"),
        p::MATH_KIND => Some("math"),
        p::PROJECTION_KIND => Some("projection"),
        _ => None,
    }
}

/// What a GPU effect's shader is handed: its floats, plus (chroma key) the
/// key colour first and (math) the swizzle packed as one base-6 number last.
/// At most sixteen — the forge ingest's width.
pub fn ingest(kind: &str, cfg: &serde_json::Value) -> Vec<f32> {
    let f = floats(kind, cfg);
    match kind {
        p::CHROMA_KEY_KIND => {
            let k = key_colour(cfg);
            let mut v = vec![k[0], k[1], k[2]];
            v.extend(f);
            v
        }
        p::MATH_KIND => {
            let s = swizzle(cfg);
            let mut v = f;
            v.push(s[0] as f32 + 6.0 * s[1] as f32 + 36.0 * s[2] as f32 + 216.0 * s[3] as f32);
            v
        }
        _ => f,
    }
}

/// Runs a P-A effect on `frame` (RGBA8) on the CPU. `root` is the data root
/// (`fx.lookup` reads its palette or `.cube` through `metal-vfs` there).
pub fn apply_cpu(kind: &str, cfg: &serde_json::Value, frame: &mut (Vec<u8>, u32, u32), root: &std::path::Path) {
    let f = floats(kind, cfg);
    match kind {
        p::CHROMA_KEY_KIND => chroma_key(&mut frame.0, key_colour(cfg), &f),
        p::LEVEL_KIND => level(&mut frame.0, &f),
        p::MATH_KIND => math(&mut frame.0, &f, swizzle(cfg)),
        p::PROJECTION_KIND => frame.0 = projection(frame, &f),
        p::LOOKUP_KIND => lookup(&mut frame.0, cfg, &f, root),
        _ => {}
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1e-5)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn to_unit(px: &[u8]) -> [f32; 4] {
    [px[0] as f32 / 255.0, px[1] as f32 / 255.0, px[2] as f32 / 255.0, px[3] as f32 / 255.0]
}

fn to_byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn luma(c: &[f32]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// The (Cb, Cr) of an RGB colour — the plane the key's distance is in, so a
/// shadow on the green screen is still green.
fn cbcr(c: &[f32]) -> [f32; 2] {
    [
        -0.168_736 * c[0] - 0.331_264 * c[1] + 0.5 * c[2],
        0.5 * c[0] - 0.418_688 * c[1] - 0.081_312 * c[2],
    ]
}

/// `fx.chroma_key`: alpha from the CbCr distance to `key` (0 at the key,
/// rising over `tolerance .. tolerance + softness`), spill suppression on the
/// key's dominant channel, `invert` keeps the key instead.
pub fn chroma_key(px: &mut [u8], key: [f32; 3], f: &[f32]) {
    let [tol, soft, spill, invert] = [f[0], f[1], f[2], f[3]];
    let kc = cbcr(&key);
    let dom = (0..3).max_by(|&a, &b| key[a].total_cmp(&key[b])).unwrap_or(1);
    for p in px.chunks_exact_mut(4) {
        let mut c = to_unit(p);
        let q = cbcr(&c);
        let d = ((q[0] - kc[0]).powi(2) + (q[1] - kc[1]).powi(2)).sqrt();
        let mut a = smoothstep(tol * 0.5, (tol + soft) * 0.5 + 1e-4, d);
        if invert >= 0.5 {
            a = 1.0 - a;
        }
        let others = (0..3).filter(|&i| i != dom).map(|i| c[i]).fold(0.0f32, f32::max);
        if c[dom] > others {
            c[dom] -= (c[dom] - others) * spill;
        }
        p[0] = to_byte(c[0]);
        p[1] = to_byte(c[1]);
        p[2] = to_byte(c[2]);
        p[3] = to_byte(c[3] * a);
    }
}

/// `fx.level` on one value.
pub fn level_value(c: f32, f: &[f32]) -> f32 {
    let [bi, wi, bo, wo, gamma, bright, contrast, _, invert, _] =
        [f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7], f[8], f[9]];
    let mut v = ((c - bi) / (wi - bi).max(1e-4)).clamp(0.0, 1.0);
    v = v.powf(1.0 / gamma.max(1e-3));
    v = bo + v * (wo - bo);
    v = (v - 0.5) * contrast + 0.5 + bright;
    if invert >= 0.5 {
        v = 1.0 - v;
    }
    v.clamp(0.0, 1.0)
}

/// `fx.level` (TD Level TOP): in/out range, gamma, brightness, contrast,
/// invert, `opacity` as the mix with the input; `channel` 0 all, 1..3 one.
pub fn level(px: &mut [u8], f: &[f32]) {
    let opacity = f[7];
    let channel = f[9].round() as usize;
    let lut: Vec<u8> = (0..256)
        .map(|i| {
            let c = i as f32 / 255.0;
            to_byte(c + (level_value(c, f) - c) * opacity)
        })
        .collect();
    for p in px.chunks_exact_mut(4) {
        for (i, c) in p[..3].iter_mut().enumerate() {
            if channel == 0 || channel == i + 1 {
                *c = lut[*c as usize];
            }
        }
    }
}

/// `fx.math` on one pixel (`0..1` RGBA).
pub fn math_pixel(c: [f32; 4], f: &[f32], sw: [u8; 4]) -> [f32; 4] {
    let pick = |i: u8| match i {
        0..=3 => c[i as usize],
        4 => 0.0,
        _ => 1.0,
    };
    let mut v = [pick(sw[0]), pick(sw[1]), pick(sw[2]), pick(sw[3])];
    for i in 0..4 {
        v[i] = v[i] * f[i] + f[4 + i];
        if f[9] >= 0.5 {
            v[i] = v[i].abs();
        }
    }
    for x in v.iter_mut().take(3) {
        *x = x.max(0.0).powf(f[8]);
        if f[10] >= 0.5 {
            *x = 1.0 - *x;
        }
    }
    if f[11] >= 0.5 {
        for x in &mut v {
            *x = x.clamp(0.0, 1.0);
        }
    }
    if f[12] >= 0.5 {
        v[3] = luma(&v).clamp(0.0, 1.0);
    }
    v
}

/// `fx.math`: per-pixel channel math.
pub fn math(px: &mut [u8], f: &[f32], sw: [u8; 4]) {
    for p in px.chunks_exact_mut(4) {
        let v = math_pixel(to_unit(p), f, sw);
        for i in 0..4 {
            p[i] = to_byte(v[i]);
        }
    }
}

// --- projection ------------------------------------------------------------

type V3 = [f32; 3];

/// A direction (y up, +z forward) for `uv` (`0..1`, y down) in layout `mode`
/// (0 equirect, 1 cubemap cross 4×3, 2 fisheye), `None` outside the picture.
pub fn uv_to_dir(mode: u8, u: f32, v: f32, fov: f32) -> Option<V3> {
    use std::f32::consts::{PI, TAU};
    match mode {
        0 => {
            let lon = (u - 0.5) * TAU;
            let lat = (0.5 - v) * PI;
            Some([lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos()])
        }
        1 => {
            let cx = (u * 4.0).floor().clamp(0.0, 3.0) as i32;
            let cy = (v * 3.0).floor().clamp(0.0, 2.0) as i32;
            let s = (u * 4.0 - cx as f32) * 2.0 - 1.0;
            let t = (v * 3.0 - cy as f32) * 2.0 - 1.0;
            match (cx, cy) {
                (1, 1) => Some([s, -t, 1.0]),
                (2, 1) => Some([1.0, -t, -s]),
                (3, 1) => Some([-s, -t, -1.0]),
                (0, 1) => Some([-1.0, -t, s]),
                (1, 0) => Some([s, 1.0, t]),
                (1, 2) => Some([s, -1.0, -t]),
                _ => None,
            }
        }
        _ => {
            let px = (u - 0.5) * 2.0;
            let py = (0.5 - v) * 2.0;
            let r = (px * px + py * py).sqrt();
            if r > 1.0 {
                return None;
            }
            let theta = r * fov.to_radians() * 0.5;
            let phi = py.atan2(px);
            Some([theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos()])
        }
    }
}

/// The inverse of [`uv_to_dir`]: where direction `d` lands in layout `mode`.
pub fn dir_to_uv(mode: u8, d: V3, fov: f32) -> Option<[f32; 2]> {
    use std::f32::consts::{PI, TAU};
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
    let [x, y, z] = [d[0] / len, d[1] / len, d[2] / len];
    match mode {
        0 => Some([x.atan2(z) / TAU + 0.5, 0.5 - y.clamp(-1.0, 1.0).asin() / PI]),
        1 => {
            let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
            let (cell, s, t) = if az >= ax && az >= ay {
                if z > 0.0 {
                    ((1, 1), x / az, -y / az)
                } else {
                    ((3, 1), -x / az, -y / az)
                }
            } else if ax >= ay {
                if x > 0.0 {
                    ((2, 1), -z / ax, -y / ax)
                } else {
                    ((0, 1), z / ax, -y / ax)
                }
            } else if y > 0.0 {
                ((1, 0), x / ay, z / ay)
            } else {
                ((1, 2), x / ay, -z / ay)
            };
            let s = ((s + 1.0) * 0.5).clamp(0.0, 0.9999);
            let t = ((t + 1.0) * 0.5).clamp(0.0, 0.9999);
            Some([(cell.0 as f32 + s) / 4.0, (cell.1 as f32 + t) / 3.0])
        }
        _ => {
            let theta = z.clamp(-1.0, 1.0).acos();
            let r = theta / (fov.to_radians() * 0.5).max(1e-4);
            if r > 1.0 {
                return None;
            }
            let phi = y.atan2(x);
            Some([0.5 + r * phi.cos() * 0.5, 0.5 - r * phi.sin() * 0.5])
        }
    }
}

/// Yaw (about y), pitch (about x), roll (about z), degrees, applied to `d`.
pub fn rotate(d: V3, yaw: f32, pitch: f32, roll: f32) -> V3 {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    let (sr, cr) = roll.to_radians().sin_cos();
    let d = [d[0] * cr - d[1] * sr, d[0] * sr + d[1] * cr, d[2]];
    let d = [d[0], d[1] * cp - d[2] * sp, d[1] * sp + d[2] * cp];
    [d[0] * cy + d[2] * sy, d[1], -d[0] * sy + d[2] * cy]
}

fn sample_bilinear(frame: &(Vec<u8>, u32, u32), u: f32, v: f32) -> [u8; 4] {
    let (w, h) = (frame.1 as i64, frame.2 as i64);
    let x = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let y = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |xx: i64, yy: i64, c: usize| frame.0[((yy * w + xx) * 4) as usize + c] as f32;
    let mut out = [0u8; 4];
    for (c, o) in out.iter_mut().enumerate() {
        let top = at(x0, y0, c) * (1.0 - fx) + at(x1, y0, c) * fx;
        let bot = at(x0, y1, c) * (1.0 - fx) + at(x1, y1, c) * fx;
        *o = (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// `fx.projection`: each output pixel's direction in layout `to`, rotated,
/// sampled from the input in layout `from`. Outside either layout is
/// transparent black.
pub fn projection(frame: &(Vec<u8>, u32, u32), f: &[f32]) -> Vec<u8> {
    let (w, h) = (frame.1, frame.2);
    let (from, to) = (f[0].round() as u8, f[1].round() as u8);
    let mut out = vec![0u8; (w * h * 4) as usize];
    if w == 0 || h == 0 || frame.0.len() != out.len() {
        return frame.0.clone();
    }
    for y in 0..h {
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            let Some(d) = uv_to_dir(to, u, v, f[5]) else { continue };
            let d = rotate(d, f[2], f[3], f[4]);
            let Some(uv) = dir_to_uv(from, d, f[5]) else { continue };
            let i = ((y * w + x) * 4) as usize;
            out[i..i + 4].copy_from_slice(&sample_bilinear(frame, uv[0], uv[1]));
        }
    }
    out
}

// --- lookup ----------------------------------------------------------------

/// A parsed `.cube` 3D LUT: `size³` RGB entries, red fastest.
#[derive(Debug, Clone, PartialEq)]
pub struct Cube {
    pub size: usize,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub data: Vec<[f32; 3]>,
}

/// Parses an Adobe/Resolve `.cube` file (`LUT_3D_SIZE`, optional
/// `DOMAIN_MIN`/`DOMAIN_MAX`, then `size³` lines of `r g b`).
pub fn parse_cube(text: &str) -> Result<Cube, String> {
    let mut size = 0usize;
    let (mut min, mut max) = ([0.0f32; 3], [1.0f32; 3]);
    let mut data = Vec::new();
    let three = |rest: &str| -> Result<[f32; 3], String> {
        let v: Vec<f32> = rest.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        (v.len() == 3).then(|| [v[0], v[1], v[2]]).ok_or_else(|| format!("expected three numbers: {rest:?}"))
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("TITLE") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("LUT_3D_SIZE") {
            size = rest.trim().parse().map_err(|_| format!("bad LUT_3D_SIZE {rest:?}"))?;
        } else if let Some(rest) = line.strip_prefix("DOMAIN_MIN") {
            min = three(rest)?;
        } else if let Some(rest) = line.strip_prefix("DOMAIN_MAX") {
            max = three(rest)?;
        } else if line.starts_with("LUT_1D_SIZE") {
            return Err("a 1D LUT, not a 3D one".into());
        } else if line.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '.') {
            data.push(three(line)?);
        }
    }
    if !(2..=256).contains(&size) || data.len() != size * size * size {
        return Err(format!("LUT_3D_SIZE {size} wants {} entries, found {}", size * size * size, data.len()));
    }
    Ok(Cube { size, min, max, data })
}

impl Cube {
    /// Trilinear lookup of `c` (`0..1` RGB).
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let g = |i: usize| ((c[i] - self.min[i]) / (self.max[i] - self.min[i]).max(1e-6)).clamp(0.0, 1.0) * (n - 1) as f32;
        let (r, gg, b) = (g(0), g(1), g(2));
        let (r0, g0, b0) = (r.floor() as usize, gg.floor() as usize, b.floor() as usize);
        let (r1, g1, b1) = ((r0 + 1).min(n - 1), (g0 + 1).min(n - 1), (b0 + 1).min(n - 1));
        let (fr, fg, fb) = (r - r0 as f32, gg - g0 as f32, b - b0 as f32);
        let at = |ri: usize, gi: usize, bi: usize| self.data[ri + gi * n + bi * n * n];
        let mut out = [0.0; 3];
        for (k, o) in out.iter_mut().enumerate() {
            let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
            let c00 = lerp(at(r0, g0, b0)[k], at(r1, g0, b0)[k], fr);
            let c10 = lerp(at(r0, g1, b0)[k], at(r1, g1, b0)[k], fr);
            let c01 = lerp(at(r0, g0, b1)[k], at(r1, g0, b1)[k], fr);
            let c11 = lerp(at(r0, g1, b1)[k], at(r1, g1, b1)[k], fr);
            *o = lerp(lerp(c00, c10, fg), lerp(c01, c11, fg), fb);
        }
        out
    }
}

/// Parsed `.cube` files, by path — a file is read once, not every frame.
fn cube_cache() -> &'static Mutex<HashMap<std::path::PathBuf, Option<Arc<Cube>>>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<std::path::PathBuf, Option<Arc<Cube>>>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The `.cube` a lookup names, under `root` (a path relative to the data
/// root; `..` refused), read through `metal-vfs`.
pub fn load_cube(root: &std::path::Path, rel: &str) -> Option<Arc<Cube>> {
    if rel.is_empty() || rel.split(['/', '\\']).any(|c| c == "..") || std::path::Path::new(rel).is_absolute() {
        return None;
    }
    let path = root.join(rel);
    let mut cache = cube_cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .entry(path.clone())
        .or_insert_with(|| match metal_vfs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| parse_cube(&t)) {
            Ok(c) => Some(Arc::new(c)),
            Err(e) => {
                crate::forge::report(rel, &e);
                None
            }
        })
        .clone()
}

/// A stored palette's colours (`data/library/palettes.json`, by `name`).
pub fn palette_colours(root: &std::path::Path, name: &str) -> Option<Vec<[f32; 3]>> {
    let text = metal_vfs::read_to_string(root.join("library").join("palettes.json")).ok()?;
    let all: serde_json::Value = serde_json::from_str(&text).ok()?;
    let p = all.as_array()?.iter().find(|p| p["name"].as_str() == Some(name))?;
    let flat: Vec<f32> = p["colors"].as_array()?.iter().filter_map(|v| v.as_f64()).map(|v| v as f32).collect();
    let out: Vec<[f32; 3]> = flat.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    (!out.is_empty()).then_some(out)
}

/// Colour stops from a config: `palette` (a stored palette) else `stops`
/// (`[[r,g,b], …]`), else black → white.
pub fn stops(cfg: &serde_json::Value, root: &std::path::Path) -> Vec<[f32; 3]> {
    if let Some(c) = cfg.get("palette").and_then(serde_json::Value::as_str).and_then(|n| palette_colours(root, n)) {
        return c;
    }
    let explicit: Vec<[f32; 3]> = cfg
        .get("stops")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    let v: Vec<f32> = s.as_array()?.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect();
                    (v.len() >= 3).then(|| [v[0], v[1], v[2]])
                })
                .collect()
        })
        .unwrap_or_default();
    if explicit.is_empty() {
        vec![[0.0; 3], [1.0; 3]]
    } else {
        explicit
    }
}

/// The colour at `t` (`0..1`) along evenly spaced `stops`.
pub fn gradient(stops: &[[f32; 3]], t: f32) -> [f32; 3] {
    match stops.len() {
        0 => [t; 3],
        1 => stops[0],
        n => {
            let x = t.clamp(0.0, 1.0) * (n - 1) as f32;
            let i = (x.floor() as usize).min(n - 2);
            let f = x - i as f32;
            let (a, b) = (stops[i], stops[i + 1]);
            [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f]
        }
    }
}

/// `fx.lookup`: `source` 0 maps the chosen channel (`channel` 0 luma, 1..4
/// r g b a) through the palette gradient; 1 maps RGB through the `.cube`.
/// `mix` blends with the input. Nothing to look up is the identity.
pub fn lookup(px: &mut [u8], cfg: &serde_json::Value, f: &[f32], root: &std::path::Path) {
    let (source, channel, mix) = (f[0].round() as u8, f[1].round() as usize, f[2]);
    let mut apply = |map: &dyn Fn([f32; 4]) -> [f32; 3]| {
        for p in px.chunks_exact_mut(4) {
            let c = to_unit(p);
            let m = map(c);
            for i in 0..3 {
                p[i] = to_byte(c[i] + (m[i] - c[i]) * mix);
            }
        }
    };
    if source == 1 {
        let Some(cube) = cfg.get("cube").and_then(serde_json::Value::as_str).and_then(|c| load_cube(root, c)) else {
            return;
        };
        apply(&|c| cube.apply([c[0], c[1], c[2]]));
    } else {
        let s = stops(cfg, root);
        apply(&|c| gradient(&s, if channel == 0 { luma(&c) } else { c[(channel - 1).min(3)] }));
    }
}

// --- ramp ------------------------------------------------------------------

/// The ramp's coordinate at `uv` (`0..1`, y down) before phase/repeat.
pub fn ramp_t(shape: u8, u: f32, v: f32, f: &[f32], aspect: f32) -> f32 {
    use std::f32::consts::TAU;
    let (dx, dy) = ((u - f[2]) * aspect, v - f[3]);
    let a = f[1].to_radians();
    match shape {
        0 => {
            let (s, c) = a.sin_cos();
            // Unit-length along the angle across the frame's half extents.
            let ext = (aspect * 0.5 * c.abs() + 0.5 * s.abs()).max(1e-4);
            (dx * c + dy * s) / (2.0 * ext) + 0.5
        }
        1 => (dx * dx + dy * dy).sqrt() * 2.0,
        2 => (dy.atan2(dx) - a).rem_euclid(TAU) / TAU,
        _ => (dx.abs() + dy.abs()) * 2.0,
    }
}

/// Rasterises `gen.ramp` at `w × h`. `phase` (already including whatever
/// drives it) shifts the gradient, `repeat` tiles it.
pub fn ramp(cfg: &serde_json::Value, w: u32, h: u32, root: &std::path::Path) -> Vec<u8> {
    let f = floats(p::RAMP_KIND, cfg);
    let s = stops(cfg, root);
    let shape = f[0].round() as u8;
    let (phase, repeat) = (f[4], f[5].max(1.0));
    let aspect = w as f32 / h.max(1) as f32;
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            let t = ramp_t(shape, u, v, &f, aspect) * repeat + phase;
            let t = if (0.0..=1.0).contains(&t) && repeat <= 1.0 { t } else { t.rem_euclid(1.0) };
            let c = gradient(&s, t);
            let i = ((y * w + x) * 4) as usize;
            out[i..i + 4].copy_from_slice(&[to_byte(c[0]), to_byte(c[1]), to_byte(c[2]), 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn defaults(kind: &str) -> Vec<f32> {
        floats(kind, &json!({}))
    }

    fn ramp_frame(n: u32) -> Vec<u8> {
        (0..n * n).flat_map(|i| [(i * 7 % 256) as u8, (i * 3 % 256) as u8, (i * 11 % 256) as u8, 255]).collect()
    }

    /// Every GPU effect's ingest fits the forge contract's sixteen slots.
    #[test]
    fn every_ingest_fits_the_forge_slots() {
        for id in metal_graph::PARITY_EFFECT_IDS {
            let kind = format!("fx.{id}");
            assert!(ingest(&kind, &json!({})).len() <= 16, "{kind}");
            assert_eq!(floats(&kind, &json!({})).len(), p::floats_of(&kind).unwrap().len());
        }
    }

    #[test]
    fn defaults_are_the_identity_where_they_should_be() {
        let frame = ramp_frame(8);
        for kind in [p::LEVEL_KIND, p::MATH_KIND, p::LOOKUP_KIND] {
            let mut px = frame.clone();
            match kind {
                p::LEVEL_KIND => level(&mut px, &defaults(kind)),
                p::MATH_KIND => math(&mut px, &defaults(kind), [0, 1, 2, 3]),
                _ => {
                    // A grey gradient on luma is not the identity, but a cube
                    // that is not there is.
                    lookup(&mut px, &json!({"source": 1.0, "cube": "nope.cube"}), &floats(kind, &json!({"source": 1.0})), std::path::Path::new("/nonexistent"));
                }
            }
            assert_eq!(px, frame, "{kind} at its defaults leaves the picture");
        }
    }

    #[test]
    fn the_key_colour_goes_transparent_and_the_rest_stays() {
        let mut px = vec![0, 255, 0, 255, 200, 40, 40, 255, 0, 200, 0, 255];
        chroma_key(&mut px, [0.0, 1.0, 0.0], &defaults(p::CHROMA_KEY_KIND));
        assert_eq!(px[3], 0, "pure green keyed out");
        assert_eq!(px[7], 255, "red kept");
        assert!(px[11] < 40, "a darker green keyed too");
        let mut inv = vec![0, 255, 0, 255, 200, 40, 40, 255];
        chroma_key(&mut inv, [0.0, 1.0, 0.0], &[0.3, 0.1, 0.0, 1.0]);
        assert_eq!((inv[3], inv[7]), (255, 0), "invert keeps the key");
    }

    #[test]
    fn level_maps_ranges_and_inverts() {
        let mut f = defaults(p::LEVEL_KIND);
        f[0] = 0.5; // black in
        assert_eq!(level_value(0.4, &f), 0.0);
        assert_eq!(level_value(1.0, &f), 1.0);
        f[8] = 1.0;
        assert_eq!(level_value(1.0, &f), 0.0);
        let mut px = vec![100, 100, 100, 255];
        let mut g = defaults(p::LEVEL_KIND);
        g[8] = 1.0;
        g[9] = 1.0; // red only
        level(&mut px, &g);
        assert_eq!(px, vec![155, 100, 100, 255]);
    }

    #[test]
    fn math_swizzles_and_puts_luma_in_alpha() {
        assert_eq!(swizzle(&json!({"swizzle": "bgr1"})), [2, 1, 0, 5]);
        assert_eq!(swizzle(&json!({"swizzle": "nope"})), [0, 1, 2, 3]);
        let mut px = vec![255, 0, 0, 10];
        math(&mut px, &defaults(p::MATH_KIND), [2, 1, 0, 5]);
        assert_eq!(px, vec![0, 0, 255, 255]);
        let mut f = defaults(p::MATH_KIND);
        f[12] = 1.0;
        let mut white = vec![255, 255, 255, 0];
        math(&mut white, &f, [0, 1, 2, 3]);
        assert_eq!(white[3], 255);
        let packed = ingest(p::MATH_KIND, &json!({"swizzle": "bgr1"}));
        assert_eq!(*packed.last().unwrap(), 2.0 + 6.0 + 0.0 + 216.0 * 5.0);
    }

    #[test]
    fn projection_layouts_round_trip() {
        for mode in 0..3u8 {
            for &(u, v) in &[(0.3, 0.4), (0.6, 0.55), (0.45, 0.5)] {
                let (u, v) = if mode == 1 { (0.25 + u * 0.25, 1.0 / 3.0 + v / 3.0) } else { (u, v) };
                let d = uv_to_dir(mode, u, v, 180.0).expect("inside");
                let back = dir_to_uv(mode, d, 180.0).expect("lands");
                assert!((back[0] - u).abs() < 1e-3 && (back[1] - v).abs() < 1e-3, "mode {mode}: {u},{v} → {back:?}");
            }
        }
        // The cube cross's front face centre looks straight ahead.
        let d = uv_to_dir(1, 0.375, 0.5, 0.0).unwrap();
        assert!(d[0].abs() < 1e-5 && d[1].abs() < 1e-5 && d[2] > 0.0);
        // Equirect → equirect with no rotation is (nearly) the identity.
        let frame = (ramp_frame(16), 16, 16);
        let f = [0.0, 0.0, 0.0, 0.0, 0.0, 180.0];
        let out = projection(&frame, &f);
        let diff: i32 = out.iter().zip(&frame.0).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(diff <= 1, "identity projection drifted by {diff}");
        // A fisheye leaves its corners transparent.
        let fish = projection(&frame, &[0.0, 2.0, 0.0, 0.0, 0.0, 180.0]);
        assert_eq!(fish[3], 0);
        // Yaw turns the picture: half a turn on an equirect shifts it half the width.
        let yawed = projection(&frame, &[0.0, 0.0, 180.0, 0.0, 0.0, 180.0]);
        assert_ne!(yawed, out);
    }

    #[test]
    fn a_cube_parses_and_looks_up() {
        let mut text = String::from("TITLE \"t\"\nLUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    // An inverting LUT.
                    text += &format!("{} {} {}\n", 1 - r, 1 - g, 1 - b);
                }
            }
        }
        let cube = parse_cube(&text).unwrap();
        let c = cube.apply([0.25, 0.5, 1.0]);
        assert!((c[0] - 0.75).abs() < 1e-5 && (c[1] - 0.5).abs() < 1e-5 && c[2].abs() < 1e-5);
        assert!(parse_cube("LUT_3D_SIZE 3\n0 0 0\n").is_err());

        let dir = std::env::temp_dir().join(format!("mvj-lookup-{}", std::process::id()));
        metal_vfs::create_dir_all(dir.join("luts")).unwrap();
        metal_vfs::write(dir.join("luts/inv.cube"), &text).unwrap();
        let mut px = vec![255, 0, 0, 255];
        let cfg = json!({"source": 1.0, "cube": "luts/inv.cube"});
        lookup(&mut px, &cfg, &floats(p::LOOKUP_KIND, &cfg), &dir);
        assert_eq!(px, vec![0, 255, 255, 255]);
        assert!(load_cube(&dir, "../etc/passwd").is_none(), "no climbing out of data");
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn a_palette_lookup_maps_luma_to_the_stops() {
        let cfg = json!({"stops": [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]});
        let mut px = vec![0, 0, 0, 255, 255, 255, 255, 255];
        lookup(&mut px, &cfg, &floats(p::LOOKUP_KIND, &cfg), std::path::Path::new("/nonexistent"));
        assert_eq!(px, vec![255, 0, 0, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn every_ramp_shape_draws_and_phase_moves_it() {
        let root = std::path::Path::new("/nonexistent");
        for shape in 0..4 {
            let a = ramp(&json!({"shape": shape}), 32, 16, root);
            assert_eq!(a.len(), 32 * 16 * 4);
            let distinct: std::collections::HashSet<u8> = a.chunks(4).map(|p| p[0]).collect();
            assert!(distinct.len() > 4, "shape {shape} is a gradient, not a fill");
            let b = ramp(&json!({"shape": shape, "phase": 0.5}), 32, 16, root);
            assert_ne!(a, b, "shape {shape}: phase moves it");
        }
        // Linear, left to right: dark on the left, bright on the right.
        let lin = ramp(&json!({}), 32, 4, root);
        assert!(lin[0] < 20 && lin[31 * 4] > 235);
    }
}
