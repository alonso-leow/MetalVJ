//! Parity P-E: transitions between models — a source switched on or off by
//! an overlays-grid button plays an in/out transition instead of cutting.
//!
//! # Config is a reading of the graph (rule 4)
//!
//! What a node's transitions *are* lives on the node: `config.transition_in`
//! and `config.transition_out`, each `{ preset, duration, unit, ease }` (plus
//! an optional `kind`/`dir` that overrides the preset's). Whether the node is
//! on is still `Node.bypass` and nothing else — [`crate::switch::apply`]
//! writes it at the press, exactly as before, so `Switch::state` reads the new
//! side immediately.
//!
//! # The one thing stored: when the toggle happened
//!
//! The graph cannot know *when* a switch flipped a node, and the transition is
//! a function of that instant. [`Toggles`] keeps `(graph, node) → (at, on)`
//! in [`crate::engine::State`], in memory, beside `overlays` — the same shape
//! as an overlay's [`crate::overlay::Play::started`]. It is never written to
//! disk because `at` is a reading of `State::time()`, a transport clock that
//! restarts with the process: a persisted instant would be meaningless after
//! a restart. Losing it is harmless — no record means "steady state", which
//! is the graph's own `bypass`. It is not a second on/off state: a record
//! whose `on` disagrees with the graph (someone flipped it back by hand) is
//! ignored.
//!
//! # Pure function of the clock, no scheduler
//!
//! [`Toggles::phase`] answers, at any `time`, whether the node is inside its
//! in- or out-transition and how far. Bypass becomes *effective* at the end
//! of the out-transition: until then [`Toggles::lift`] un-bypasses the node in
//! the per-frame copy of the graph the engine draws from (never in the file).
//! Nothing posts a second "now really turn it off" — the frame after the end
//! simply stops lifting it.
//!
//! # Where the pass runs
//!
//! [`apply`] is the CPU stand-in on the node's finished RGBA picture. At the
//! point the engine composes, the generator's frame has already been read
//! back into RGBA (`RunOutput::frames`), so this is the cheap place for it;
//! a GPU version would re-upload the frame only to read it back again.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use metal_graph::Graph;

use crate::switch::SwitchEntry;

/// Config keys on a source node.
pub const IN_KEY: &str = "transition_in";
pub const OUT_KEY: &str = "transition_out";

/// The transition kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Fade,
    Dissolve,
    Wipe,
    Iris,
    ZoomIn,
    ZoomOut,
    Blur,
    Glitch,
    Pixelate,
    Slide,
    Flash,
}

impl Kind {
    pub const ALL: [Kind; 11] = [
        Kind::Fade,
        Kind::Dissolve,
        Kind::Wipe,
        Kind::Iris,
        Kind::ZoomIn,
        Kind::ZoomOut,
        Kind::Blur,
        Kind::Glitch,
        Kind::Pixelate,
        Kind::Slide,
        Kind::Flash,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Kind::Fade => "fade",
            Kind::Dissolve => "dissolve",
            Kind::Wipe => "wipe",
            Kind::Iris => "iris",
            Kind::ZoomIn => "zoom_in",
            Kind::ZoomOut => "zoom_out",
            Kind::Blur => "blur",
            Kind::Glitch => "glitch",
            Kind::Pixelate => "pixelate",
            Kind::Slide => "slide",
            Kind::Flash => "flash",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.id() == s)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    #[default]
    Beats,
    Seconds,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    Linear,
    In,
    Out,
    #[default]
    InOut,
}

impl Ease {
    pub fn apply(self, p: f32) -> f32 {
        let p = p.clamp(0.0, 1.0);
        match self {
            Ease::Linear => p,
            Ease::In => p * p,
            Ease::Out => 1.0 - (1.0 - p) * (1.0 - p),
            Ease::InOut => p * p * (3.0 - 2.0 * p),
        }
    }

    pub fn parse(s: &str) -> Option<Ease> {
        match s {
            "linear" => Some(Ease::Linear),
            "in" => Some(Ease::In),
            "out" => Some(Ease::Out),
            "in_out" => Some(Ease::InOut),
            _ => None,
        }
    }
}

/// Direction for wipe and slide.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    #[default]
    Left,
    Right,
    Up,
    Down,
}

/// One `data/transitions/*.json` file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub duration: f32,
    #[serde(default)]
    pub unit: Unit,
    #[serde(default)]
    pub ease: Ease,
    #[serde(default)]
    pub dir: Dir,
}

/// A transition fully resolved from a node's config and the presets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spec {
    pub kind: Kind,
    pub duration: f32,
    pub unit: Unit,
    pub ease: Ease,
    pub dir: Dir,
}

impl Spec {
    /// Length in seconds at `bpm` (beats are converted at the current tempo).
    pub fn seconds(&self, bpm: f32) -> f32 {
        let d = self.duration.max(0.0);
        match self.unit {
            Unit::Seconds => d,
            Unit::Beats => d * 60.0 / if bpm.is_finite() && bpm > 0.0 { bpm } else { 120.0 },
        }
    }
}

pub fn presets_dir() -> std::path::PathBuf {
    crate::output::data_root().join("transitions")
}

/// Every preset in `dir`, sorted by id. Unreadable files are skipped.
pub fn load_presets_from(dir: &std::path::Path) -> Vec<Preset> {
    let Ok(entries) = metal_vfs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Preset> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| metal_vfs::read_to_string(&p).ok())
        .filter_map(|t| serde_json::from_str(&t).ok())
        .collect();
    out.sort_by(|a: &Preset, b| a.id.cmp(&b.id));
    out
}

pub fn load_presets() -> Vec<Preset> {
    load_presets_from(&presets_dir())
}

/// Resolve one `transition_in/out` config value. `None` = cut (no slot, an
/// unknown preset with no `kind`, or a zero duration).
pub fn resolve(slot: &Value, presets: &[Preset]) -> Option<Spec> {
    if !slot.is_object() {
        return None;
    }
    let preset = slot["preset"].as_str().and_then(|id| presets.iter().find(|p| p.id == id));
    let kind = slot["kind"].as_str().and_then(Kind::parse).or(preset.map(|p| p.kind))?;
    let duration = slot["duration"].as_f64().map(|d| d as f32).or(preset.map(|p| p.duration)).unwrap_or(1.0);
    let unit = match slot["unit"].as_str() {
        Some("seconds") => Unit::Seconds,
        Some("beats") => Unit::Beats,
        _ => preset.map(|p| p.unit).unwrap_or_default(),
    };
    let ease = slot["ease"].as_str().and_then(Ease::parse).or(preset.map(|p| p.ease)).unwrap_or_default();
    let dir = slot["dir"]
        .as_str()
        .and_then(|d| serde_json::from_value(Value::String(d.into())).ok())
        .or(preset.map(|p| p.dir))
        .unwrap_or_default();
    (duration > 0.0).then_some(Spec { kind, duration, unit, ease, dir })
}

/// A node's spec for one side, read off its config.
pub fn node_spec(g: &Graph, node: &str, on: bool, presets: &[Preset]) -> Option<Spec> {
    let n = g.node(node)?;
    let cfg: Value = serde_json::from_str(&n.config.to_json_str()).ok()?;
    resolve(&cfg[if on { IN_KEY } else { OUT_KEY }], presets)
}

/// When a switch last flipped a node, and to which side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Toggle {
    pub at: f32,
    pub on: bool,
}

/// Where a node is in its transition at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Phase {
    pub spec: Spec,
    /// `true` for the in-transition.
    pub on: bool,
    /// Eased progress, `0..1`.
    pub progress: f32,
}

impl Phase {
    /// How much of the picture shows: rises 0→1 on the way in, falls 1→0 out.
    pub fn visibility(&self) -> f32 {
        if self.on {
            self.progress
        } else {
            1.0 - self.progress
        }
    }
}

/// Raw (un-eased) progress of a toggle at `time`; `None` once it is over or
/// when the clock reads earlier than the toggle (a restart or a rewind).
pub fn progress(at: f32, seconds: f32, time: f32) -> Option<f32> {
    if seconds <= 0.0 || time < at {
        return None;
    }
    let p = (time - at) / seconds;
    (p < 1.0).then_some(p)
}

/// `(graph id, node id) → toggle`. See the module docs for why this is the
/// only stored piece.
#[derive(Default, Debug)]
pub struct Toggles {
    map: HashMap<(String, String), Toggle>,
}

impl Toggles {
    /// Record the instant a batch is about to flip — call before
    /// [`crate::switch::apply`]. Entries that change nothing are not stamped.
    pub fn stamp(&mut self, g: &Graph, entries: &[SwitchEntry], time: f32) {
        for e in entries {
            if let Some(n) = g.node(&e.node) {
                if n.bypass == e.on {
                    self.map.insert((g.id.clone(), e.node.clone()), Toggle { at: time, on: e.on });
                }
            }
        }
    }

    pub fn get(&self, graph: &str, node: &str) -> Option<Toggle> {
        self.map.get(&(graph.to_string(), node.to_string())).copied()
    }

    /// The node's phase at `time`, or `None` when it is steady (no record,
    /// the record disagrees with the graph, no transition configured, or it
    /// is over).
    pub fn phase(&self, g: &Graph, node: &str, time: f32, bpm: f32, presets: &[Preset]) -> Option<Phase> {
        self.phase_in(g, node, time, bpm, presets, true)
    }

    /// [`Self::phase`] on a frame copy that [`Self::lift`] already
    /// un-bypassed: `checked = false` skips the graph-agreement test, which
    /// `lift` made against the file's own `bypass` before lifting.
    pub fn phase_in(&self, g: &Graph, node: &str, time: f32, bpm: f32, presets: &[Preset], checked: bool) -> Option<Phase> {
        let t = self.get(&g.id, node)?;
        let n = g.node(node)?;
        if checked && n.bypass == t.on {
            return None;
        }
        let spec = node_spec(g, node, t.on, presets)?;
        let p = progress(t.at, spec.seconds(bpm), time)?;
        Some(Phase { spec, on: t.on, progress: spec.ease.apply(p) })
    }

    /// Bypass deferred: un-bypass, in this frame's copy of `g`, every node
    /// still playing its out-transition.
    /// Returns the ids it lifted.
    pub fn lift(&self, g: &mut Graph, time: f32, bpm: f32, presets: &[Preset]) -> Vec<String> {
        let mut lifted = Vec::new();
        let ids: Vec<String> = self
            .map
            .iter()
            .filter(|((gid, _), t)| *gid == g.id && !t.on)
            .map(|((_, n), _)| n.clone())
            .collect();
        for id in ids {
            if self.phase(g, &id, time, bpm, presets).is_some() {
                if let Some(n) = g.node_mut(&id) {
                    n.bypass = false;
                }
                lifted.push(id);
            }
        }
        lifted
    }

    /// The phase of `node` in a frame graph produced by [`Self::lift`]:
    /// in-transitions are checked against the graph as usual; out-transitions
    /// count only for the nodes `lift` reported.
    pub fn frame_phase(&self, g: &Graph, node: &str, lifted: &[String], time: f32, bpm: f32, presets: &[Preset]) -> Option<Phase> {
        let t = self.get(&g.id, node)?;
        if t.on {
            self.phase(g, node, time, bpm, presets)
        } else if lifted.iter().any(|l| l == node) {
            self.phase_in(g, node, time, bpm, presets, false)
        } else {
            None
        }
    }

    /// Drop records whose transitions are long over (housekeeping only —
    /// a stale record already reads as steady).
    pub fn prune(&mut self, time: f32, keep_for: f32) {
        self.map.retain(|_, t| time >= t.at && time - t.at < keep_for);
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

fn hash(x: u32, y: u32, seed: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

#[cfg(test)]
fn sample(src: &[u8], w: u32, h: u32, x: i64, y: i64) -> [u8; 4] {
    if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
        return [0, 0, 0, 0];
    }
    let i = ((y as u32 * w + x as u32) * 4) as usize;
    [src[i], src[i + 1], src[i + 2], src[i + 3]]
}

/// `x · v` rounded, for every byte value: one lookup per pixel instead of a
/// float multiply.
fn scale_lut(v: f32) -> [u8; 256] {
    std::array::from_fn(|i| (i as f32 * v).round() as u8)
}

/// The blur radius in pixels at visibility `v` (the GPU pass uses the same).
pub fn blur_radius(v: f32) -> i64 {
    ((1.0 - v.clamp(0.0, 1.0)) * 16.0).round() as i64
}

/// Box blur of radius ≈ `radius`, cheap: average `f×f` blocks into a small
/// image, box it there separably, bilinear back up. Clamp-to-edge, as the
/// GPU's separable pass.
fn blur_cheap(frame: &mut [u8], w: u32, h: u32, radius: i64, alpha: &[u8; 256]) {
    let f = ((radius as u32) / 2).max(1);
    let (sw, sh) = (w.div_ceil(f) as usize, h.div_ceil(f) as usize);
    let (wu, hu) = (w as usize, h as usize);
    // 1. Block average.
    let fu = f as usize;
    let mut sum = vec![0u32; sw * sh * 4];
    let mut count = vec![0u32; sw * sh];
    for y in 0..hu {
        let base = (y / fu) * sw;
        let row = &frame[y * wu * 4..(y + 1) * wu * 4];
        for (x, p) in row.chunks_exact(4).enumerate() {
            let si = base + x / fu;
            let d = &mut sum[si * 4..si * 4 + 4];
            d[0] += p[0] as u32;
            d[1] += p[1] as u32;
            d[2] += p[2] as u32;
            d[3] += p[3] as u32;
            count[si] += 1;
        }
    }
    let small: Vec<f32> = sum.iter().enumerate().map(|(i, s)| *s as f32 / count[i / 4].max(1) as f32).collect();
    // 2. Separable box at the small scale, radius r/f (at least 1).
    let r = ((radius as f32 / f as f32).round() as i64).max(1);
    let boxed = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0f32; src.len()];
        let n = (2 * r + 1) as f32;
        for y in 0..sh {
            for x in 0..sw {
                let mut acc = [0f32; 4];
                for d in -r..=r {
                    let (xx, yy) = if horizontal {
                        ((x as i64 + d).clamp(0, sw as i64 - 1) as usize, y)
                    } else {
                        (x, (y as i64 + d).clamp(0, sh as i64 - 1) as usize)
                    };
                    let j = (yy * sw + xx) * 4;
                    for c in 0..4 {
                        acc[c] += src[j + c];
                    }
                }
                let i = (y * sw + x) * 4;
                for c in 0..4 {
                    out[i + c] = acc[c] / n;
                }
            }
        }
        out
    };
    let small = boxed(&boxed(&small, true), false);
    // 3. Bilinear back up (sample at the block centres): lerp the two small
    // rows once per output row, then along x.
    let ff = f as f32;
    let xs: Vec<(usize, usize, f32)> = (0..wu)
        .map(|x| {
            let fx = ((x as f32 + 0.5) / ff - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = fx.floor() as usize;
            (x0 * 4, (x0 + 1).min(sw - 1) * 4, fx - x0 as f32)
        })
        .collect();
    let mut line = vec![0f32; sw * 4];
    for y in 0..hu {
        let fy = ((y as f32 + 0.5) / ff - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = fy.floor() as usize;
        let (y1, ty) = ((y0 + 1).min(sh - 1), fy - y0 as f32);
        let (r0, r1) = (&small[y0 * sw * 4..(y0 + 1) * sw * 4], &small[y1 * sw * 4..(y1 + 1) * sw * 4]);
        for ((l, a), b) in line.iter_mut().zip(r0).zip(r1) {
            *l = a + (b - a) * ty;
        }
        let row = &mut frame[y * wu * 4..(y + 1) * wu * 4];
        for (d, &(x0, x1, tx)) in row.chunks_exact_mut(4).zip(&xs) {
            for c in 0..4 {
                let a = line[x0 + c];
                d[c] = (a + (line[x1 + c] - a) * tx + 0.5) as u8;
            }
            d[3] = alpha[d[3] as usize];
        }
    }
}

thread_local! {
    /// The dissolve's fixed noise (`hash(x, y, 7)`, 16 bits) for the last size.
    static DISSOLVE: std::cell::RefCell<(u32, u32, Vec<u16>)> = const { std::cell::RefCell::new((0, 0, Vec::new())) };
}

/// The CPU pass: `frame` (RGBA, `w×h`) drawn at visibility `v` (`0` gone,
/// `1` untouched) by `spec`. `seed` varies the noise kinds per frame.
///
/// The `Stand`/wasm path and the reference the GPU pass
/// (`crate::transition_gpu`) is diffed against. Written per kind, row-wise,
/// with lookups instead of a per-pixel match: the per-pixel version cost
/// 21–63 ms at 1080p and 1.5 s for blur.
pub fn apply(spec: &Spec, v: f32, frame: &mut [u8], w: u32, h: u32, seed: u32) {
    let v = v.clamp(0.0, 1.0);
    if v >= 1.0 || w == 0 || h == 0 || frame.len() < (w * h * 4) as usize {
        return;
    }
    let (wu, hu) = (w as usize, h as usize);
    let (wf, hf) = (w as f32, h as f32);
    let (cx, cy) = (wf * 0.5, hf * 0.5);
    let u = 1.0 - v;
    let lut_v = scale_lut(v);
    match spec.kind {
        Kind::Fade => {
            for p in frame.chunks_exact_mut(4) {
                p[3] = lut_v[p[3] as usize];
            }
        }
        Kind::Dissolve => {
            DISSOLVE.with(|c| {
                let mut c = c.borrow_mut();
                if c.0 != w || c.1 != h {
                    let noise = (0..h).flat_map(|y| (0..w).map(move |x| (hash(x, y, 7) * 65535.0).round() as u16)).collect();
                    *c = (w, h, noise);
                }
                for (p, &n) in frame.chunks_exact_mut(4).zip(&c.2) {
                    if n as f32 / 65535.0 >= v {
                        p[3] = 0;
                    }
                }
            });
        }
        Kind::Wipe => {
            for y in 0..hu {
                let yf = y as f32 + 0.5;
                for x in 0..wu {
                    let xf = x as f32 + 0.5;
                    let t = match spec.dir {
                        Dir::Left => xf / wf,
                        Dir::Right => 1.0 - xf / wf,
                        Dir::Up => yf / hf,
                        Dir::Down => 1.0 - yf / hf,
                    };
                    if t > v {
                        frame[(y * wu + x) * 4 + 3] = 0;
                    }
                }
            }
        }
        Kind::Iris => {
            let lim = v * (cx * cx + cy * cy).sqrt();
            let lim2 = lim * lim;
            for y in 0..hu {
                let dy2 = (y as f32 + 0.5 - cy).powi(2);
                for x in 0..wu {
                    if (x as f32 + 0.5 - cx).powi(2) + dy2 > lim2 {
                        frame[(y * wu + x) * 4 + 3] = 0;
                    }
                }
            }
        }
        Kind::ZoomIn | Kind::ZoomOut | Kind::Slide | Kind::Pixelate => {
            let src = frame.to_vec();
            let block = (1.0 + u * 48.0).round().max(1.0) as u32;
            // Each kind is a separable lookup: source column per x, source
            // row per y (`-1` = outside, transparent).
            let map = |n: u32, c: f32, along: bool| -> Vec<i64> {
                (0..n)
                    .map(|i| {
                        let f = i as f32 + 0.5;
                        match spec.kind {
                            Kind::ZoomIn | Kind::ZoomOut => {
                                let s = if spec.kind == Kind::ZoomIn { v.max(0.001) } else { 1.0 + u * 2.0 };
                                (c + (f - c) / s).floor() as i64
                            }
                            Kind::Slide => {
                                let d = match (spec.dir, along) {
                                    (Dir::Left, true) => -(u * wf),
                                    (Dir::Right, true) => u * wf,
                                    (Dir::Up, false) => -(u * hf),
                                    (Dir::Down, false) => u * hf,
                                    _ => 0.0,
                                };
                                (f - d).floor() as i64
                            }
                            _ => ((i / block) * block + block / 2).min(n - 1) as i64,
                        }
                    })
                    .map(|s| if s < 0 || s >= n as i64 { -1 } else { s })
                    .collect()
            };
            let (xs, ys) = (map(w, cx, true), map(h, cy, false));
            let lut = match spec.kind {
                Kind::Slide => scale_lut(1.0),
                Kind::Pixelate => scale_lut(v.sqrt()),
                _ => lut_v,
            };
            for (y, &sy) in ys.iter().enumerate() {
                let dst = &mut frame[y * wu * 4..(y + 1) * wu * 4];
                if sy < 0 {
                    dst.fill(0);
                    continue;
                }
                let srow = &src[sy as usize * wu * 4..(sy as usize + 1) * wu * 4];
                for (x, &sx) in xs.iter().enumerate() {
                    let d = &mut dst[x * 4..x * 4 + 4];
                    if sx < 0 {
                        d.fill(0);
                    } else {
                        let s = &srow[sx as usize * 4..sx as usize * 4 + 4];
                        d.copy_from_slice(s);
                        d[3] = lut[s[3] as usize];
                    }
                }
            }
        }
        Kind::Blur => {
            let radius = blur_radius(v);
            if radius > 0 {
                blur_cheap(frame, w, h, radius, &lut_v);
            } else {
                for p in frame.chunks_exact_mut(4) {
                    p[3] = lut_v[p[3] as usize];
                }
            }
        }
        Kind::Glitch => {
            let src = frame.to_vec();
            let split = (u * 12.0) as i64;
            let wi = w as i64;
            let at = |row: &[u8], x: i64| -> [u8; 4] {
                if x < 0 || x >= wi {
                    [0; 4]
                } else {
                    let i = x as usize * 4;
                    [row[i], row[i + 1], row[i + 2], row[i + 3]]
                }
            };
            let uu = u * u;
            for y in 0..h {
                let row_shift =
                    if hash(0, y / 8, seed) < u { ((hash(1, y / 8, seed) - 0.5) * u * wf * 0.4) as i64 } else { 0 };
                let srow = &src[y as usize * wu * 4..(y as usize + 1) * wu * 4];
                let holes: Vec<bool> = (0..w.div_ceil(16)).map(|bx| hash(bx, y / 16, seed) < uu).collect();
                let dst = &mut frame[y as usize * wu * 4..(y as usize + 1) * wu * 4];
                for x in 0..wu {
                    let r = at(srow, x as i64 + row_shift + split);
                    let mut px = at(srow, x as i64 + row_shift);
                    px[0] = r[0];
                    if holes[x / 16] {
                        px[3] = 0;
                    }
                    dst[x * 4..x * 4 + 4].copy_from_slice(&px);
                }
            }
        }
        Kind::Flash => {
            let lut: [u8; 256] = std::array::from_fn(|c| (c as f32 + (255.0 - c as f32) * u).round() as u8);
            for p in frame.chunks_exact_mut(4) {
                for c in p.iter_mut() {
                    *c = lut[*c as usize];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_graph::{Domain, Graph};
    use serde_json::json;

    fn shipped() -> Vec<Preset> {
        load_presets_from(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/transitions"))
    }

    fn graph(cfg: Value) -> (Graph, String) {
        let mut g = Graph::new("g1", "T", Domain::Live);
        let id = g.add_node("n", "gen.turing", [0.0, 0.0]);
        g.node_mut(&id).unwrap().config = metal_graph::Config::from_json_str(&cfg.to_string()).unwrap();
        (g, id)
    }

    #[test]
    fn progress_is_a_pure_function_of_the_clock() {
        assert_eq!(progress(10.0, 2.0, 10.0), Some(0.0));
        assert_eq!(progress(10.0, 2.0, 11.0), Some(0.5));
        assert_eq!(progress(10.0, 2.0, 12.0), None, "over at the end");
        assert_eq!(progress(10.0, 2.0, 9.0), None, "a clock before the toggle reads steady");
        assert_eq!(progress(10.0, 0.0, 10.0), None);
        let spec = Spec { kind: Kind::Fade, duration: 2.0, unit: Unit::Beats, ease: Ease::Linear, dir: Dir::Left };
        assert!((spec.seconds(120.0) - 1.0).abs() < 1e-6, "2 beats at 120 bpm = 1 s");
        assert!((Ease::InOut.apply(0.5) - 0.5).abs() < 1e-6);
        assert_eq!(Ease::In.apply(0.0), 0.0);
        assert_eq!(Ease::Out.apply(1.0), 1.0);
    }

    #[test]
    fn bypass_is_deferred_until_the_out_transition_ends() {
        let presets = shipped();
        let (mut g, id) = graph(json!({"transition_out": {"kind": "fade", "duration": 1.0, "unit": "seconds", "ease": "linear"}}));
        let mut t = Toggles::default();
        let off = [SwitchEntry::node(id.clone(), false)];
        t.stamp(&g, &off, 5.0);
        crate::switch::apply(&mut g, &off);
        assert!(g.node(&id).unwrap().bypass, "the graph reads off at once (switch state)");
        let ph = t.phase(&g, &id, 5.5, 120.0, &presets).unwrap();
        assert!(!ph.on && (ph.visibility() - 0.5).abs() < 1e-6);
        let mut frame = g.clone();
        let lifted = t.lift(&mut frame, 5.5, 120.0, &presets);
        assert!(!frame.node(&id).unwrap().bypass, "still drawn mid-transition");
        let fp = t.frame_phase(&frame, &id, &lifted, 5.5, 120.0, &presets).unwrap();
        assert!((fp.visibility() - 0.5).abs() < 1e-6);
        let mut frame = g.clone();
        t.lift(&mut frame, 6.0, 120.0, &presets);
        assert!(frame.node(&id).unwrap().bypass, "bypass effective at the end");
        // Flipped back by hand: the record no longer matches the graph.
        g.node_mut(&id).unwrap().bypass = false;
        assert!(t.phase(&g, &id, 5.5, 120.0, &presets).is_none());
    }

    #[test]
    fn no_transition_configured_is_a_cut() {
        let (mut g, id) = graph(json!({}));
        let mut t = Toggles::default();
        let off = [SwitchEntry::node(id.clone(), false)];
        t.stamp(&g, &off, 0.0);
        crate::switch::apply(&mut g, &off);
        let mut frame = g.clone();
        t.lift(&mut frame, 0.1, 120.0, &[]);
        assert!(frame.node(&id).unwrap().bypass);
    }

    #[test]
    fn in_transition_rises_and_uses_the_preset() {
        let presets = shipped();
        let (mut g, id) = graph(json!({"transition_in": {"preset": "fundido-1-beat"}}));
        g.node_mut(&id).unwrap().bypass = true;
        let mut t = Toggles::default();
        let on = [SwitchEntry::node(id.clone(), true)];
        t.stamp(&g, &on, 0.0);
        crate::switch::apply(&mut g, &on);
        let ph = t.phase(&g, &id, 0.25, 120.0, &presets).expect("1 beat at 120 = 0.5 s");
        assert!(ph.on && ph.spec.kind == Kind::Fade);
        assert!(t.phase(&g, &id, 0.5, 120.0, &presets).is_none());
    }

    #[test]
    fn every_shipped_preset_parses_and_changes_the_picture() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/transitions");
        let files = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "json")).count();
        let presets = shipped();
        assert!(files >= 8);
        assert_eq!(presets.len(), files, "a preset file did not parse");
        let (w, h) = (32u32, 24u32);
        let src: Vec<u8> = (0..w * h).flat_map(|i| [(i * 7 % 200) as u8, (i * 3 % 180) as u8, 90, 255]).collect();
        for p in &presets {
            let spec = resolve(&json!({"preset": p.id}), &presets).unwrap_or_else(|| panic!("{} resolves", p.id));
            assert!(spec.seconds(120.0) > 0.0, "{}", p.id);
            for v in [0.25f32, 0.5, 0.75] {
                let mut f = src.clone();
                apply(&spec, v, &mut f, w, h, 1);
                assert_ne!(f, src, "{} is a no-op at v={v}", p.id);
            }
            let mut f = src.clone();
            apply(&spec, 1.0, &mut f, w, h, 1);
            assert_eq!(f, src, "{} at full visibility leaves the picture alone", p.id);
        }
    }

    #[test]
    fn every_kind_is_a_non_no_op() {
        let (w, h) = (16u32, 16u32);
        let src: Vec<u8> = (0..w * h).flat_map(|i| [(i * 13 % 250) as u8, 40, (i % 200) as u8, 255]).collect();
        for k in Kind::ALL {
            let spec = Spec { kind: k, duration: 1.0, unit: Unit::Seconds, ease: Ease::Linear, dir: Dir::Left };
            let mut f = src.clone();
            apply(&spec, 0.5, &mut f, w, h, 3);
            assert_ne!(f, src, "{k:?}");
            assert_eq!(Kind::parse(k.id()), Some(k));
        }
    }

    /// `cargo test -p metal-perform --release --lib transition::tests::measure -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn measure() {
        for (w, h) in [(1920u32, 1080u32), (3840, 2160)] {
            let src: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, 40, (i % 199) as u8, 255]).collect();
            for k in Kind::ALL {
                let spec = Spec { kind: k, duration: 1.0, unit: Unit::Seconds, ease: Ease::Linear, dir: Dir::Left };
                let n = 20;
                let mut f = src.clone();
                let t0 = std::time::Instant::now();
                for i in 0..n {
                    apply(&spec, 0.5, &mut f, w, h, i);
                }
                let ms = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
                println!("MEASURE transition {k:?} {w}x{h}: {ms:.2} ms");
            }
        }
    }
}
