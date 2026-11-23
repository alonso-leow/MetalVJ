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
    LightLeak,
    /// A camera move between two sources through a framed portal — see
    /// [`PortalPart`] and `docs/plan-portal.md` §3.
    Portal,
}

impl Kind {
    pub const ALL: [Kind; 13] = [
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
        Kind::LightLeak,
        Kind::Portal,
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
            Kind::LightLeak => "light_leak",
            Kind::Portal => "portal",
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

/// Which side of a portal transition hosts the camera move.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    /// `Through` when the outgoing source is 3D, `Reveal` when only the
    /// incoming one is, `Through` with a flat host for 2D→2D.
    #[default]
    Auto,
    /// The outgoing source pans to the side and dollies through the portal.
    Through,
    /// The incoming source is the host and the clip plays backwards: the old
    /// picture shrinks into a framed portal, then the camera pans back.
    Reveal,
}

impl Via {
    pub fn parse(s: &str) -> Option<Via> {
        serde_json::from_value(Value::String(s.into())).ok()
    }
}

fn default_angle() -> f32 {
    90.0
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
    /// Portal: how far the camera pans, in degrees (90 or 180).
    #[serde(default = "default_angle")]
    pub angle: f32,
    /// Portal: which side hosts the move.
    #[serde(default)]
    pub via: Via,
}

/// A transition fully resolved from a node's config and the presets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spec {
    pub kind: Kind,
    pub duration: f32,
    pub unit: Unit,
    pub ease: Ease,
    pub dir: Dir,
    /// Portal: pan angle in degrees.
    pub angle: f32,
    /// Portal: which side hosts the move, as configured.
    pub via: Via,
    /// Portal: this node's part, resolved per frame by the engine from the
    /// pair that is switching ([`portal_parts`]). `None` everywhere else.
    pub portal: Option<PortalPart>,
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

/// The preset a slot with nothing chosen plays: the light leak.
pub const DEFAULT_PRESET: &str = "light-leak";

/// The slot value that means an explicit cut: `{"kind": "cut"}` or
/// `{"preset": "cut"}`.
pub const CUT: &str = "cut";

/// [`DEFAULT_PRESET`] from `presets`, or the built-in copy of it when the
/// preset files are missing (a bare test, a fresh checkout).
pub fn default_preset(presets: &[Preset]) -> Preset {
    presets.iter().find(|p| p.id == DEFAULT_PRESET).cloned().unwrap_or_else(|| Preset {
        id: DEFAULT_PRESET.into(),
        name: "Luz filtrada".into(),
        kind: Kind::LightLeak,
        duration: 2.0,
        unit: Unit::Beats,
        ease: Ease::InOut,
        dir: Dir::Left,
        angle: default_angle(),
        via: Via::Auto,
    })
}

/// Resolve one `transition_in/out` config value. A slot with no preset and
/// no `kind` (absent, `null`, or an unknown preset id) plays
/// [`DEFAULT_PRESET`]. `None` = an explicit cut (`"cut"`) or a zero duration.
pub fn resolve(slot: &Value, presets: &[Preset]) -> Option<Spec> {
    let text = |k: &str| slot.get(k).and_then(Value::as_str);
    if text("kind") == Some(CUT) || text("preset") == Some(CUT) {
        return None;
    }
    let named = text("preset").and_then(|id| presets.iter().find(|p| p.id == id));
    let explicit = text("kind").and_then(Kind::parse);
    let preset: Option<Preset> = named.cloned().or_else(|| explicit.is_none().then(|| default_preset(presets)));
    let kind = explicit.or(preset.as_ref().map(|p| p.kind))?;
    let duration = slot["duration"].as_f64().map(|d| d as f32).or(preset.as_ref().map(|p| p.duration)).unwrap_or(1.0);
    let unit = match text("unit") {
        Some("seconds") => Unit::Seconds,
        Some("beats") => Unit::Beats,
        _ => preset.as_ref().map(|p| p.unit).unwrap_or_default(),
    };
    let ease = text("ease").and_then(Ease::parse).or(preset.as_ref().map(|p| p.ease)).unwrap_or_default();
    let dir = text("dir")
        .and_then(|d| serde_json::from_value(Value::String(d.into())).ok())
        .or(preset.as_ref().map(|p| p.dir))
        .unwrap_or_default();
    let angle = slot["angle"].as_f64().map(|a| a as f32).or(preset.as_ref().map(|p| p.angle)).unwrap_or(default_angle());
    let via = text("via").and_then(Via::parse).or(preset.as_ref().map(|p| p.via)).unwrap_or_default();
    (duration > 0.0).then_some(Spec { kind, duration, unit, ease, dir, angle, via, portal: None })
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
        Kind::LightLeak => {
            let blobs = leak_blobs(v);
            let bloom = (std::f32::consts::PI * v).sin();
            let asp = hf / wf;
            for y in 0..hu {
                let uy = (y as f32 + 0.5) / hf;
                for x in 0..wu {
                    let ux = (x as f32 + 0.5) / wf;
                    let mut glow = [0f32; 3];
                    let mut ga = 0f32;
                    for (c, r, col) in &blobs {
                        let g = leak_falloff(ux, uy, asp, *c, *r);
                        for k in 0..3 {
                            glow[k] += col[k] * g;
                        }
                        ga = ga.max(g);
                    }
                    let p = &mut frame[(y * wu + x) * 4..(y * wu + x) * 4 + 4];
                    for k in 0..3 {
                        let b = p[k] as f32 / 255.0;
                        let l = (glow[k] * bloom).min(1.0);
                        p[k] = ((1.0 - (1.0 - b) * (1.0 - l)) * 255.0).round() as u8;
                    }
                    let a = (p[3] as f32 / 255.0 * v + ga * bloom).clamp(0.0, 1.0);
                    p[3] = (a * 255.0).round() as u8;
                }
            }
        }
        Kind::Portal => match spec.portal {
            Some(part) => apply_portal(spec, &part, frame, w, h),
            // Unresolved (a bare call with no engine pairing): a plain fade.
            None => {
                for p in frame.chunks_exact_mut(4) {
                    p[3] = lut_v[p[3] as usize];
                }
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Portal
// ---------------------------------------------------------------------------

/// One node's part in a portal transition at one instant, resolved by the
/// engine from the pair that is switching ([`portal_parts`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PortalPart {
    /// `true`: the picture the camera moves in (it gets a hole where the
    /// portal is, and its rim). `false`: the picture seen through the portal.
    pub host: bool,
    /// A host with no 3D pose: its picture is re-projected as a flat card the
    /// virtual camera turns away from. A 3D host turns its own camera
    /// (`cam_yaw`/`cam_dolly`, written by the engine) and keeps its pixels.
    pub flat: bool,
    /// Clip position, `0` = all host at rest, `1` = through the portal.
    /// `Through` runs it forwards, `Reveal` backwards.
    pub q: f32,
}

/// Vertical field of view of the virtual camera, degrees.
const PORTAL_FOV: f32 = 60.0;
/// Portal height over the screen height when it sits centred at its own
/// distance: how much of the frame the doorway takes before the dolly.
const PORTAL_SIZE: f32 = 0.55;
/// Rim width, as a fraction of the portal's half size.
const PORTAL_RIM: f32 = 0.05;
/// Where the pan ends and the dolly starts.
const PORTAL_PAN_END: f32 = 0.5;
/// World units of model dolly per unit of virtual dolly.
pub const PORTAL_DOLLY_WORLD: f32 = 2.0;

/// The virtual camera at clip position `q`: yaw (radians, positive looks
/// left) and how far it has dollied toward the portal (`0..1-PORTAL_SIZE`;
/// at the end the doorway exactly fills the frame, so the hand-off to the
/// guest alone does not jump). `angle` in degrees, `dir` the side.
pub fn portal_camera(q: f32, angle: f32, dir: Dir) -> (f32, f32) {
    let q = q.clamp(0.0, 1.0);
    let side = if dir == Dir::Right { -1.0 } else { 1.0 };
    let theta = side * angle.clamp(0.0, 180.0).to_radians();
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let pan = smooth((q / PORTAL_PAN_END).min(1.0));
    let push = smooth(((q - PORTAL_PAN_END) / (1.0 - PORTAL_PAN_END)).clamp(0.0, 1.0));
    (theta * pan, push * (1.0 - PORTAL_SIZE))
}

/// What the virtual camera sees through pixel `(x, y)`: the portal-local
/// position `(u, v)` (`-1..1` inside the doorway) where its ray meets the
/// portal plane, if it does, and the same on the host's flat card.
struct PortalView {
    eye: [f32; 3],
    fwd: [f32; 3],
    right: [f32; 3],
    tan_v: f32,
    tan_h: f32,
    centre: [f32; 3],
    normal: [f32; 3],
    axis: [f32; 3],
    half: [f32; 2],
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Forward direction at yaw `a` (positive looks left, right-handed about +Y).
fn yawed(a: f32) -> [f32; 3] {
    [-a.sin(), 0.0, -a.cos()]
}

impl PortalView {
    fn new(q: f32, angle: f32, dir: Dir, aspect: f32) -> PortalView {
        let (yaw, dolly) = portal_camera(q, angle, dir);
        let side = if dir == Dir::Right { -1.0 } else { 1.0 };
        let theta = side * angle.clamp(0.0, 180.0).to_radians();
        let at = yawed(theta);
        let fwd = yawed(yaw);
        let tan_v = (PORTAL_FOV.to_radians() * 0.5).tan();
        // The dolly runs along the portal's own axis once the pan is done;
        // `dolly` is still 0 while panning.
        PortalView {
            eye: [at[0] * dolly, 0.0, at[2] * dolly],
            fwd,
            right: [-fwd[2], 0.0, fwd[0]],
            tan_v,
            tan_h: tan_v * aspect,
            centre: at,
            normal: [-at[0], 0.0, -at[2]],
            axis: [-at[2], 0.0, at[0]],
            half: [PORTAL_SIZE * tan_v * aspect, PORTAL_SIZE * tan_v],
        }
    }

    /// The ray through normalised device coordinates `(nx, ny)` (`-1..1`,
    /// `y` up) meeting the plane through `centre` with `normal`, in that
    /// plane's `(axis, up)` coordinates over `half`.
    fn hit(&self, nx: f32, ny: f32, centre: [f32; 3], normal: [f32; 3], axis: [f32; 3], half: [f32; 2]) -> Option<(f32, f32)> {
        let r = [
            self.fwd[0] + self.right[0] * nx * self.tan_h,
            ny * self.tan_v,
            self.fwd[2] + self.right[2] * nx * self.tan_h,
        ];
        let den = dot(r, normal);
        if den >= -1e-6 {
            return None;
        }
        let rel = [centre[0] - self.eye[0], centre[1] - self.eye[1], centre[2] - self.eye[2]];
        let t = dot(rel, normal) / den;
        if t <= 0.0 {
            return None;
        }
        let p = [self.eye[0] + r[0] * t - centre[0], self.eye[1] + r[1] * t - centre[1], self.eye[2] + r[2] * t - centre[2]];
        Some((dot(p, axis) / half[0], p[1] / half[1]))
    }

    fn portal(&self, nx: f32, ny: f32) -> Option<(f32, f32)> {
        self.hit(nx, ny, self.centre, self.normal, self.axis, self.half)
    }

    /// The host's flat card: the full frame one unit ahead at rest.
    fn card(&self, nx: f32, ny: f32) -> Option<(f32, f32)> {
        self.hit(nx, ny, [0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [self.tan_h, self.tan_v])
    }
}

/// Samples `src` at portal-local `(u, v)` (`-1..1`, `v` up); nearest pixel.
fn sample_uv(src: &[u8], w: u32, h: u32, u: f32, v: f32) -> [u8; 4] {
    let x = (((u + 1.0) * 0.5 * w as f32) as i64).clamp(0, w as i64 - 1) as usize;
    let y = (((1.0 - v) * 0.5 * h as f32) as i64).clamp(0, h as i64 - 1) as usize;
    let i = (y * w as usize + x) * 4;
    [src[i], src[i + 1], src[i + 2], src[i + 3]]
}

/// The rim colour: a cool white glow, brightest at the doorway's edge.
fn rim_pixel(edge: f32) -> [u8; 4] {
    let g = (1.0 - edge).clamp(0.0, 1.0);
    let c = (180.0 + 75.0 * g) as u8;
    [c, c, 255, (255.0 * g.sqrt()) as u8]
}

/// The portal pass for one node. The host keeps (or re-projects) its picture,
/// is cut out where the doorway is and carries its rim; the guest is drawn
/// only inside the doorway. The two parts never overlap, so the result does
/// not depend on which layer is on top.
pub fn apply_portal(spec: &Spec, part: &PortalPart, frame: &mut [u8], w: u32, h: u32) {
    let (wu, hu) = (w as usize, h as usize);
    let view = PortalView::new(part.q, spec.angle, spec.dir, w as f32 / h.max(1) as f32);
    let src = frame.to_vec();
    let rim = 1.0 + PORTAL_RIM;
    for y in 0..hu {
        let ny = 1.0 - 2.0 * (y as f32 + 0.5) / h as f32;
        for x in 0..wu {
            let nx = 2.0 * (x as f32 + 0.5) / w as f32 - 1.0;
            let door = view.portal(nx, ny);
            let inside = door.is_some_and(|(u, v)| u.abs() <= 1.0 && v.abs() <= 1.0);
            let d = &mut frame[(y * wu + x) * 4..(y * wu + x) * 4 + 4];
            if !part.host {
                match door {
                    Some((u, v)) if inside => d.copy_from_slice(&sample_uv(&src, w, h, u, v)),
                    _ => d.fill(0),
                }
                continue;
            }
            if inside {
                d.fill(0);
                continue;
            }
            if part.flat {
                match view.card(nx, ny) {
                    Some((u, v)) if u.abs() <= 1.0 && v.abs() <= 1.0 => d.copy_from_slice(&sample_uv(&src, w, h, u, v)),
                    _ => d.fill(0),
                }
            }
            if let Some((u, v)) = door {
                let edge = u.abs().max(v.abs());
                if edge <= rim {
                    let r = rim_pixel((edge - 1.0) / (rim - 1.0));
                    let a = r[3] as f32 / 255.0;
                    for k in 0..3 {
                        d[k] = (d[k] as f32 * (1.0 - a) + r[k] as f32 * a).round() as u8;
                    }
                    d[3] = d[3].max(r[3]);
                }
            }
        }
    }
}

/// One node of a portal pair, as the engine sees it this frame.
#[derive(Clone, Debug)]
pub struct PortalSide {
    pub node: String,
    /// `true` for the incoming source.
    pub on: bool,
    /// Raw-eased progress of its own transition, `0..1`.
    pub progress: f32,
    /// Whether its model declares the standard 3D pose.
    pub is_3d: bool,
    pub via: Via,
}

/// Each node's [`PortalPart`] for the portal transitions playing this frame.
/// The first outgoing and the first incoming node form the pair; a node with
/// no partner plays its half alone. See [`Via`] for how `Auto` resolves.
pub fn portal_parts(sides: &[PortalSide]) -> Vec<(String, PortalPart)> {
    let out = sides.iter().find(|s| !s.on);
    let inn = sides.iter().find(|s| s.on);
    let via = out.or(inn).map(|s| s.via).unwrap_or_default();
    let via = match via {
        Via::Auto => {
            if out.is_some_and(|s| s.is_3d) || !inn.is_some_and(|s| s.is_3d) {
                Via::Through
            } else {
                Via::Reveal
            }
        }
        v => v,
    };
    let mut parts = Vec::new();
    for s in [out, inn].into_iter().flatten() {
        let host = (via == Via::Through) != s.on;
        let q = if via == Via::Through { s.progress } else { 1.0 - s.progress };
        parts.push((s.node.clone(), PortalPart { host, flat: host && !s.is_3d, q }));
    }
    parts
}

/// The light leak's glow blobs at visibility `v`: centre `(x, y)` in UV,
/// radius, colour (amber, orange, magenta). They sweep left→right as `v`
/// rises; the trailing ones lag. The GPU pass (`transition_gpu.rs`) carries
/// the same constants.
fn leak_blobs(v: f32) -> [([f32; 2], f32, [f32; 3]); 3] {
    let s = -0.35 + 1.7 * v;
    [
        ([s, 0.30], 0.22, [1.0, 0.62, 0.18]),
        ([s - 0.22, 0.72], 0.30, [1.0, 0.36, 0.08]),
        ([s - 0.45, 0.50], 0.20, [0.95, 0.20, 0.55]),
    ]
}

/// Smoothstep falloff `0..1` of one blob at UV `(ux, uy)`, aspect-corrected
/// (`asp` = height / width) so the blobs stay round.
fn leak_falloff(ux: f32, uy: f32, asp: f32, c: [f32; 2], r: f32) -> f32 {
    let (dx, dy) = (ux - c[0], (uy - c[1]) * asp);
    let t = (1.0 - (dx * dx + dy * dy).sqrt() / r).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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
        let spec = Spec { kind: Kind::Fade, duration: 2.0, unit: Unit::Beats, ease: Ease::Linear, dir: Dir::Left, angle: 90.0, via: Via::Auto, portal: None };
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
    fn explicit_cut_is_a_cut() {
        let (mut g, id) = graph(json!({"transition_out": {"kind": "cut"}}));
        let mut t = Toggles::default();
        let off = [SwitchEntry::node(id.clone(), false)];
        t.stamp(&g, &off, 0.0);
        crate::switch::apply(&mut g, &off);
        let mut frame = g.clone();
        t.lift(&mut frame, 0.1, 120.0, &[]);
        assert!(frame.node(&id).unwrap().bypass);
    }

    #[test]
    fn resolve_defaults_to_the_light_leak() {
        let presets = shipped();
        let want = Spec { kind: Kind::LightLeak, duration: 2.0, unit: Unit::Beats, ease: Ease::InOut, dir: Dir::Left, angle: 90.0, via: Via::Auto, portal: None };
        assert_eq!(resolve(&Value::Null, &presets), Some(want));
        assert_eq!(resolve(&json!({}), &presets), Some(want), "an empty slot");
        assert_eq!(resolve(&json!({"preset": "no-such-preset"}), &presets), Some(want), "an unknown preset");
        assert_eq!(resolve(&Value::Null, &[]), Some(want), "no preset files: the built-in copy");
        assert_eq!(resolve(&json!({"duration": 4.0}), &presets).map(|s| s.duration), Some(4.0));
        assert_eq!(resolve(&json!({"kind": "fade"}), &presets).map(|s| s.kind), Some(Kind::Fade), "a kind wins over the default");
    }

    #[test]
    fn resolve_cut_is_explicit() {
        let presets = shipped();
        assert_eq!(resolve(&json!({"kind": "cut"}), &presets), None);
        assert_eq!(resolve(&json!({"preset": "cut"}), &presets), None);
        assert_eq!(resolve(&json!({"preset": "light-leak", "duration": 0}), &presets), None, "a zero duration is a cut");
    }

    #[test]
    fn light_leak_preset_parses() {
        let presets = shipped();
        let p = presets.iter().find(|p| p.id == "light-leak").expect("luz-filtrada.json parses");
        assert_eq!(p.name, "Luz filtrada");
        assert_eq!(p.kind, Kind::LightLeak);
        assert_eq!((p.duration, p.unit, p.ease), (2.0, Unit::Beats, Ease::InOut));
        assert_eq!(Kind::parse("light_leak"), Some(Kind::LightLeak));
        assert_eq!(default_preset(&presets), *p);
    }

    #[test]
    fn light_leak_warms_the_middle_and_leaves_the_ends_clean() {
        let spec = Spec { kind: Kind::LightLeak, duration: 2.0, unit: Unit::Beats, ease: Ease::Linear, dir: Dir::Left, angle: 90.0, via: Via::Auto, portal: None };
        let (w, h) = (64u32, 36u32);
        let src = vec![60u8, 60, 60, 255].repeat((w * h) as usize);
        let mut mid = src.clone();
        apply(&spec, 0.5, &mut mid, w, h, 0);
        // Some pixel gained red over blue: the glow is warm, not grey.
        assert!(mid.chunks_exact(4).any(|p| p[0] > p[2] + 40), "amber/orange/magenta glow");
        let mut zero = src.clone();
        apply(&spec, 0.0, &mut zero, w, h, 0);
        assert!(zero.chunks_exact(4).all(|p| p[3] == 0), "gone at v=0");
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
            let spec = Spec { kind: k, duration: 1.0, unit: Unit::Seconds, ease: Ease::Linear, dir: Dir::Left, angle: 90.0, via: Via::Auto, portal: None };
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
                let spec = Spec { kind: k, duration: 1.0, unit: Unit::Seconds, ease: Ease::Linear, dir: Dir::Left, angle: 90.0, via: Via::Auto, portal: None };
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

    fn portal_spec(dir: Dir, angle: f32, part: PortalPart) -> Spec {
        Spec {
            kind: Kind::Portal,
            duration: 4.0,
            unit: Unit::Beats,
            ease: Ease::Linear,
            dir,
            angle,
            via: Via::Auto,
            portal: Some(part),
        }
    }

    fn side(node: &str, on: bool, is_3d: bool) -> PortalSide {
        PortalSide { node: node.into(), on, progress: 0.25, is_3d, via: Via::Auto }
    }

    #[test]
    fn auto_goes_through_a_3d_origin_and_reveals_into_a_3d_destination() {
        // 3D -> 2D: the outgoing 3D source hosts, the clip runs forwards.
        let parts = portal_parts(&[side("a", false, true), side("b", true, false)]);
        assert_eq!(parts[0], ("a".into(), PortalPart { host: true, flat: false, q: 0.25 }));
        assert_eq!(parts[1], ("b".into(), PortalPart { host: false, flat: false, q: 0.25 }));
        // 2D -> 3D: the incoming 3D source hosts, the clip runs backwards.
        let parts = portal_parts(&[side("a", false, false), side("b", true, true)]);
        assert_eq!(parts[0], ("a".into(), PortalPart { host: false, flat: false, q: 0.75 }));
        assert_eq!(parts[1], ("b".into(), PortalPart { host: true, flat: false, q: 0.75 }));
        // 2D -> 2D: through, with a flat (re-projected) host.
        let parts = portal_parts(&[side("a", false, false), side("b", true, false)]);
        assert!(parts[0].1.host && parts[0].1.flat);
    }

    #[test]
    fn the_camera_pans_first_then_dollies_to_exactly_fill_the_frame() {
        assert_eq!(portal_camera(0.0, 90.0, Dir::Left), (0.0, 0.0));
        let (yaw, dolly) = portal_camera(PORTAL_PAN_END, 90.0, Dir::Left);
        assert!((yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5 && dolly == 0.0);
        let (yaw, _) = portal_camera(1.0, 180.0, Dir::Right);
        assert!((yaw + std::f32::consts::PI).abs() < 1e-5, "right turns the other way");
        let (_, dolly) = portal_camera(1.0, 90.0, Dir::Left);
        assert!((dolly - (1.0 - PORTAL_SIZE)).abs() < 1e-5);
    }

    /// At the end of the clip the guest fills the frame with its own pixels
    /// (no jump at the hand-off) and the host is fully cut away; at the start
    /// the host is untouched and the guest is nowhere.
    #[test]
    fn the_portal_hands_off_without_a_jump() {
        let (w, h) = (64u32, 36u32);
        let picture: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, (i % 7) as u8 * 30, 9, 255]).collect();
        let guest = PortalPart { host: false, flat: false, q: 1.0 };
        let mut px = picture.clone();
        apply_portal(&portal_spec(Dir::Left, 90.0, guest), &guest, &mut px, w, h);
        let same = px.chunks_exact(4).zip(picture.chunks_exact(4)).filter(|(a, b)| a == b).count();
        assert!(same as f32 / (w * h) as f32 > 0.97, "guest at the end is its own frame ({same})");
        let host = PortalPart { host: true, flat: false, q: 1.0 };
        let mut px = picture.clone();
        apply_portal(&portal_spec(Dir::Left, 90.0, host), &host, &mut px, w, h);
        assert!(px.chunks_exact(4).all(|p| p[3] == 0), "host is gone at the end");
        let start = PortalPart { host: true, flat: true, q: 0.0 };
        let mut px = picture.clone();
        apply_portal(&portal_spec(Dir::Left, 90.0, start), &start, &mut px, w, h);
        assert_eq!(px, picture, "a flat host at rest is untouched");
        let guest0 = PortalPart { host: false, flat: false, q: 0.0 };
        let mut px = picture.clone();
        apply_portal(&portal_spec(Dir::Left, 90.0, guest0), &guest0, &mut px, w, h);
        assert!(px.chunks_exact(4).all(|p| p[3] == 0), "the doorway is off screen at rest");
    }

    /// Mid-clip the two halves tile the frame: where the guest shows, the host
    /// is cut, so layer order cannot matter.
    #[test]
    fn host_and_guest_never_overlap() {
        let (w, h) = (80u32, 45u32);
        let full = vec![200u8; (w * h * 4) as usize];
        for q in [0.3, 0.5, 0.7, 0.9] {
            let host = PortalPart { host: true, flat: false, q };
            let guest = PortalPart { host: false, flat: false, q };
            let (mut a, mut b) = (full.clone(), full.clone());
            apply_portal(&portal_spec(Dir::Right, 90.0, host), &host, &mut a, w, h);
            apply_portal(&portal_spec(Dir::Right, 90.0, guest), &guest, &mut b, w, h);
            let guest_px = b.chunks_exact(4).filter(|p| p[3] > 0).count();
            assert!(q < 0.5 || guest_px > 0, "the doorway shows at q={q}");
            for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
                assert!(pb[3] == 0 || pa[3] == 0, "overlap at q={q}");
            }
        }
    }

    #[test]
    fn a_portal_preset_resolves_its_angle_and_via() {
        let slot = serde_json::json!({ "kind": "portal", "duration": 4, "angle": 180, "via": "reveal", "dir": "right" });
        let spec = resolve(&slot, &[]).expect("portal resolves");
        assert_eq!((spec.kind, spec.angle, spec.via, spec.dir), (Kind::Portal, 180.0, Via::Reveal, Dir::Right));
        assert_eq!(Kind::parse("portal"), Some(Kind::Portal));
    }
}
