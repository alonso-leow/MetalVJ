//! `reel.batch`, the multi-source reel (2026-10-01).
//!
//! One node, an ordered list of things to show, stepped on a clock:
//!
//! - **items** (`config.items`): `{"kind":"video","path":…}`,
//!   `{"kind":"image","path":…}`, `{"kind":"scene","id":…}` (a saved or
//!   favourite scene from [`crate::scenes`]) and `{"kind":"group","name":…}` —
//!   a whole favourites group, expanded **at every tick** from the file, so an
//!   installation follows its group as it is edited.
//! - **floats**, each under its own config key (`metal_graph::params`):
//!   `interval` (how long one item shows), `unit` (0 seconds / 1 beats — the
//!   beat count is the input's own, [`beat_clock`]),
//!   `order` (0 sequential, plays once and holds the last / 0.5 loop / 1
//!   random) and `seed`.
//!
//! # Stateless, like the rest of the effects
//!
//! What shows at an instant is a pure function of `(config, clock, list)`:
//! step `k = floor(clock / interval)`, then the order maps `k` to an index.
//! Random is a seeded shuffle **per pass** over the list, built from
//! `fx::mix32`/`fx::unit` on `(seed, pass)` — every item shows once per pass
//! (a large list is covered, not sampled), and the first of a pass is never
//! the last of the one before. The same tick picks the same item, on every
//! screen.
//!
//! A batch whose list is empty (every graph written before this existed)
//! draws nothing, exactly as it always did.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fx::{mix32, unit};
use crate::scenes::Favorites;

pub use metal_graph::REEL_BATCH_KIND;

/// The config key the engine stamps the frame's beat clock under, on the
/// frame's in-memory copy of the graph only (never saved) — see [`stamp`].
pub const CLOCK_KEY: &str = "_clock";

/// One entry of `config.items`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Item {
    /// A video. `start`/`end` (seconds) name a window to loop inside, as a
    /// dropped scene writes them; absent, the whole clip loops.
    Video {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end: Option<f64>,
    },
    Image { path: String },
    Scene { id: String },
    Group { name: String },
}

/// What an item resolves to at a tick: something with pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// A video: its whole length, or the `(start, end)` window it loops in.
    Video { path: String, span: Option<(f64, f64)> },
    /// A still, held for the interval.
    Image { path: String },
    /// `clip` from `start` to `end` seconds, looping inside it.
    Scene { clip: String, start: f64, end: f64 },
}

/// How a batch steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Sequential,
    Loop,
    Random,
}

/// The four floats, read and clamped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stepping {
    pub interval: f32,
    pub beats: bool,
    pub order: Order,
    pub seed: u32,
}

impl Default for Stepping {
    /// The declared defaults (`metal_graph::params`): eight seconds, looping.
    fn default() -> Stepping {
        Stepping { interval: 8.0, beats: false, order: Order::Loop, seed: 0 }
    }
}

fn config(node: &metal_graph::Node) -> Value {
    serde_json::from_str(&node.config.to_json_str()).unwrap_or(Value::Null)
}

/// The node's floats with their declared defaults when unset.
pub fn stepping(node: &metal_graph::Node) -> Stepping {
    stepping_of(&config(node))
}

fn stepping_of(c: &Value) -> Stepping {
    let get = |k: &str, d: f32, lo: f32, hi: f32| {
        c.get(k).and_then(Value::as_f64).map(|v| v as f32).filter(|v| v.is_finite()).unwrap_or(d).clamp(lo, hi)
    };
    let order = get("order", 0.5, 0.0, 1.0);
    Stepping {
        interval: get("interval", 8.0, 0.05, 600.0),
        beats: get("unit", 0.0, 0.0, 1.0) >= 0.5,
        order: if order < 0.25 {
            Order::Sequential
        } else if order < 0.75 {
            Order::Loop
        } else {
            Order::Random
        },
        seed: get("seed", 0.0, 0.0, 1000.0) as u32,
    }
}

/// `config.items`, skipping any entry that does not parse.
pub fn items(node: &metal_graph::Node) -> Vec<Item> {
    items_of(&config(node))
}

fn items_of(c: &Value) -> Vec<Item> {
    c.get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect()
}

/// The list as it stands now: scenes looked up, groups expanded in place.
pub fn expand(items: &[Item], favorites: &Favorites) -> Vec<Entry> {
    let scene = |s: &crate::scenes::Scene| Entry::Scene { clip: s.clip.clone(), start: s.start, end: s.end };
    let mut out = Vec::new();
    for item in items {
        match item {
            Item::Video { path, start, end } => {
                let span = match (start, end) {
                    (Some(a), Some(b)) if b > a => Some((*a, *b)),
                    _ => None,
                };
                out.push(Entry::Video { path: path.clone(), span });
            }
            Item::Image { path } => out.push(Entry::Image { path: path.clone() }),
            Item::Scene { id } => out.extend(favorites.scenes.get(id).map(scene)),
            Item::Group { name } => out.extend(favorites.group(name).into_iter().map(|(_, s)| scene(s))),
        }
    }
    out
}

/// What shows: list `index`, at `step`, `local` clock units into it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    pub index: usize,
    pub step: u64,
    pub local: f32,
}

/// The pick at `clock` (seconds or beats, per `s.beats`) over `n` items.
pub fn pick(s: &Stepping, n: usize, clock: f64) -> Option<Pick> {
    if n == 0 {
        return None;
    }
    let clock = clock.max(0.0);
    let interval = s.interval.max(0.05) as f64;
    let step = (clock / interval).floor() as u64;
    let local = (clock - step as f64 * interval) as f32;
    Some(Pick { index: index_at(s, n, step), step, local })
}

/// Which item step `step` shows.
pub fn index_at(s: &Stepping, n: usize, step: u64) -> usize {
    match (s.order, n) {
        (_, 0 | 1) => 0,
        (Order::Sequential, _) => (step as usize).min(n - 1),
        // Two items have one order that never repeats: alternate.
        (Order::Loop, _) | (Order::Random, 2) => (step % n as u64) as usize,
        (Order::Random, _) => {
            let pass = step / n as u64;
            let at = (step % n as u64) as usize;
            shuffled(s.seed, pass, n)[at]
        }
    }
}

/// A seeded Fisher–Yates permutation of `0..n` for one pass, its first item
/// moved off the previous pass's last so no item shows twice in a row.
fn shuffled(seed: u32, pass: u64, n: usize) -> Vec<usize> {
    let raw = |pass: u64| {
        let key = mix32(seed ^ mix32(pass as u32) ^ mix32((pass >> 32) as u32 ^ 0x5bd1_e995));
        let mut p: Vec<usize> = (0..n).collect();
        for i in (1..n).rev() {
            let j = ((unit(key, i as u32, 0x0b47) * (i + 1) as f32) as usize).min(i);
            p.swap(i, j);
        }
        p
    };
    let mut p = raw(pass);
    if pass > 0 && n > 1 && raw(pass - 1)[n - 1] == p[0] {
        p.swap(0, 1);
    }
    p
}

/// The tempo a beat clock falls back to when the input has none.
pub const FALLBACK_BPM: f32 = 120.0;

/// `(beats, seconds per beat)` at `time`: the input's own phase-locked count
/// ([`crate::live::Input::beats`], what the router and the Looper step on),
/// or — with no count yet (silence, a tracker that has not locked) — a free
/// clock at the input's tempo, [`FALLBACK_BPM`] when it has none, so a batch
/// in beats still steps with no music playing.
pub fn beat_clock(input: &crate::live::Input, time: f32) -> (f64, f64) {
    let bpm = input.bpm();
    let bpm = if bpm > 0.0 { bpm } else { FALLBACK_BPM };
    let spb = 60.0 / bpm as f64;
    match input.beats(time) {
        Some(b) => (b as f64, spb),
        None => (time as f64 / spb, spb),
    }
}

/// Writes the frame's beat clock (`[beats, seconds per beat]`) onto every
/// `reel.batch` node of the frame's in-memory graph, so `Paint::run` steps a
/// beats-unit batch on the music. Never reaches a file.
pub fn stamp(graph: &mut metal_graph::Graph, beats: f64, seconds_per_beat: f64) {
    let ids: Vec<String> = graph.nodes.iter().filter(|n| n.kind == REEL_BATCH_KIND).map(|n| n.id.clone()).collect();
    for id in ids {
        if let Some(node) = graph.node_mut(&id) {
            let mut c = config(node);
            if !c.is_object() {
                c = serde_json::json!({});
            }
            c[CLOCK_KEY] = serde_json::json!([beats, seconds_per_beat]);
            if let Ok(cfg) = metal_graph::Config::from_json_str(&c.to_string()) {
                node.config = cfg;
            }
        }
    }
}

/// What a batch node shows at show `time`, resolved against `favorites`:
/// the entry and the seconds into it. `None` when the list is empty.
pub fn now(node: &metal_graph::Node, favorites: &Favorites, time: f32) -> Option<(Entry, f32)> {
    let c = config(node);
    let s = stepping_of(&c);
    let list = expand(&items_of(&c), favorites);
    let (clock, secs_per_unit) = if s.beats {
        let stamped = c.get(CLOCK_KEY).and_then(Value::as_array);
        let beats = stamped.and_then(|a| a.first()).and_then(Value::as_f64);
        let spb = stamped.and_then(|a| a.get(1)).and_then(Value::as_f64).filter(|v| *v > 0.0);
        let spb = spb.unwrap_or(60.0 / FALLBACK_BPM as f64);
        (beats.unwrap_or(time as f64 / spb), spb)
    } else {
        (time as f64, 1.0)
    };
    let p = pick(&s, list.len(), clock)?;
    Some((list[p.index].clone(), (p.local as f64 * secs_per_unit) as f32))
}

// ---------------------------------------------------------------------------
// pixels
// ---------------------------------------------------------------------------

/// The proxy's stem, the same rule as the sequence editor's `hap_slug`: a
/// video plays from `data/projects/hap/<slug>.mov`, the proxy the editor (and
/// ingest) already made, so a clip previewed there is playable here.
pub fn hap_slug(path: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    let stem = Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
    let stem: String = stem.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
    format!("{stem}-{:016x}", h.finish())
}

/// How a `still` fills its frame when its picture is not the frame's shape:
/// `cover` fills it and crops the overflow, `contain` fits it whole and leaves
/// the rest transparent, `stretch` (the default, and `reel.batch`'s) distorts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fit {
    Stretch,
    Cover,
    Contain,
}

/// A `still`'s `fit` config key; anything but `cover`/`contain` stretches.
pub fn fit_of(c: &Value) -> Fit {
    match c["fit"].as_str() {
        Some("cover") => Fit::Cover,
        Some("contain") => Fit::Contain,
        _ => Fit::Stretch,
    }
}

/// `src` (`sw × sh` RGBA) laid into a `w × h` frame by `fit`, nearest-sampled.
/// Pixels the picture does not cover (only `contain`) stay transparent.
pub fn fit_px(src: &[u8], sw: u32, sh: u32, w: u32, h: u32, fit: Fit) -> Vec<u8> {
    if fit == Fit::Stretch || sw == 0 || sh == 0 || w == 0 || h == 0 {
        return crate::capture::scale(src, sw, sh, w, h);
    }
    let (sw_f, sh_f, w_f, h_f) = (sw as f64, sh as f64, w as f64, h as f64);
    let s = match fit {
        Fit::Cover => (w_f / sw_f).max(h_f / sh_f),
        _ => (w_f / sw_f).min(h_f / sh_f),
    };
    let ox = (w_f - sw_f * s) / 2.0;
    let oy = (h_f - sh_f * s) / 2.0;
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        let fy = (y as f64 + 0.5 - oy) / s;
        if fy < 0.0 || fy >= sh_f {
            continue;
        }
        for x in 0..w {
            let fx = (x as f64 + 0.5 - ox) / s;
            if fx < 0.0 || fx >= sw_f {
                continue;
            }
            let si = ((fy as u32 * sw + fx as u32) * 4) as usize;
            let di = ((y * w + x) * 4) as usize;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    out
}

fn resolve(root: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    }
}

/// An entry's picture at `local` seconds, RGBA at `w × h`. A video with no
/// HAP proxy yet, or an image that will not decode, is `None` (the caller
/// shows black): decoding a whole clip is ingest's job, not a frame's.
pub fn frame(root: &Path, entry: &Entry, local: f32, w: u32, h: u32) -> Option<Vec<u8>> {
    match entry {
        Entry::Image { path } => image(&resolve(root, path), w, h, Fit::Stretch),
        Entry::Video { path, span } => video(root, path, local as f64, *span, false, w, h),
        Entry::Scene { clip, start, end } => video(root, clip, local as f64, Some((*start, *end)), false, w, h),
    }
}

/// A library's source folder and its clips (`id → file`), read once per
/// name: the `reel.clip`/`still` producers ask every frame.
fn library_files(root: &Path, name: &str) -> Option<(PathBuf, Vec<(String, String)>)> {
    static LIBS: OnceLock<Mutex<HashMap<(PathBuf, String), Option<(PathBuf, Vec<(String, String)>)>>>> = OnceLock::new();
    let mut libs = LIBS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    libs.entry((root.to_path_buf(), name.to_string()))
        .or_insert_with(|| {
            let lib = metal_reel::index::Library::load(root, name).ok()?;
            Some((PathBuf::from(&lib.source), lib.clips.iter().map(|c| (c.id.clone(), c.file.clone())).collect()))
        })
        .clone()
}

/// A file a `still`/`reel.clip` config names: an absolute path as is, else a
/// library clip (by id or filename) or a file inside the library's folder,
/// else relative to the data root.
fn source_path(root: &Path, library: Option<&str>, file: &str) -> PathBuf {
    let p = Path::new(file);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    if let Some((dir, clips)) = library.filter(|l| !l.is_empty()).and_then(|l| library_files(root, l)) {
        let name = clips.iter().find(|(id, f)| id == file || f == file).map_or(file, |(_, f)| f.as_str());
        return dir.join(name);
    }
    root.join(p)
}

/// P-E: the picture of a `still` node (`{file}` or `{library, image}`) or a
/// `reel.clip` node (`{library, clip, passage, speed, reverse}`, or the drop
/// shape `{file, start, end}`) at show `time`, RGBA at `w × h`. `None` when
/// the config names nothing or the file has no pixels yet (a video without
/// its HAP proxy): the caller shows black, as `reel.batch` does.
pub fn source_frame(root: &Path, node: &metal_graph::Node, time: f32, w: u32, h: u32) -> Option<Vec<u8>> {
    let c: Value = serde_json::from_str(&node.config.to_json_str()).unwrap_or_default();
    let library = c["library"].as_str();
    if node.kind == "still" {
        let file = c["file"].as_str().or(c["image"].as_str()).filter(|f| !f.is_empty())?;
        return image(&source_path(root, library, file), w, h, fit_of(&c));
    }
    let (clip, span) = clip_and_span(&c)?;
    let path = source_path(root, library, clip);
    let speed = c["speed"].as_f64().filter(|s| *s > 0.0).unwrap_or(1.0);
    let reverse = c["reverse"].as_bool() == Some(true);
    video(root, &path.to_string_lossy(), time as f64 * speed, span, reverse, w, h)
}

/// A `reel.clip` config's file and its play window, reading both naming
/// schemes: the canonical `{clip, passage}` an authored node carries, and the
/// `{file, start, end}` a source dropped on the nodes canvas writes (a dragged
/// scene adds `start`/`end`). `None` when neither names a file.
pub fn clip_and_span(c: &Value) -> Option<(&str, Option<(f64, f64)>)> {
    let clip = c["clip"].as_str().or(c["file"].as_str()).filter(|f| !f.is_empty())?;
    let span = c["passage"]
        .as_array()
        .and_then(|a| Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?)))
        .or_else(|| {
            let a = c["start"].as_f64()?;
            let b = c["end"].as_f64()?;
            (b > a).then_some((a, b))
        });
    Some((clip, span))
}

/// The source time (seconds) a clip shows at `local` seconds into its play:
/// a `passage` `(start, end)` loops inside itself, else the whole `duration`
/// loops. `reverse` walks the window backwards, with or without a passage.
pub fn source_time(local: f64, span: Option<(f64, f64)>, duration: f64, reverse: bool) -> f64 {
    let (lo, len) = match span.filter(|(a, b)| b > a) {
        Some((a, b)) => (a, b - a),
        None => (0.0, duration),
    };
    if len <= 0.0 {
        return lo;
    }
    let within = local.rem_euclid(len);
    lo + if reverse { len - within } else { within }
}

/// A PNG as RGBA8 (8-bit gray/gray-alpha/RGB/RGBA; palettes expanded).
fn decode_png(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    let bytes = metal_vfs::read(path).ok()?;
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let n = (w * h) as usize;
    let px: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..n * 4].to_vec(),
        png::ColorType::Rgb => buf[..n * 3].chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..n * 2].chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => buf[..n].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((px, w, h))
}

fn image(path: &Path, w: u32, h: u32, fit: Fit) -> Option<Vec<u8>> {
    static IMAGES: OnceLock<Mutex<HashMap<(PathBuf, u32, u32, Fit), Vec<u8>>>> = OnceLock::new();
    let key = (path.to_path_buf(), w, h, fit);
    let mut cache = IMAGES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(px) = cache.get(&key) {
        return Some(px.clone());
    }
    let (rgba, iw, ih) = match metal_reel::index::decode(path) {
        Ok(rgb) => (rgb.px.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(), rgb.width, rgb.height),
        Err(_) => decode_png(path)?,
    };
    let px = fit_px(&rgba, iw, ih, w, h, fit);
    if cache.len() > 64 {
        cache.clear();
    }
    cache.insert(key, px.clone());
    Some(px)
}

fn video(root: &Path, path: &str, local: f64, span: Option<(f64, f64)>, reverse: bool, w: u32, h: u32) -> Option<Vec<u8>> {
    static OPEN: OnceLock<Mutex<HashMap<String, metal_reel::hap::Hap>>> = OnceLock::new();
    let mut open = OPEN.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if !open.contains_key(path) {
        let (movie, index) = metal_reel::hap::Hap::paths(&root.join("projects"), &hap_slug(path));
        open.insert(path.to_string(), metal_reel::hap::Hap::open(&movie, &index).ok()?);
    }
    let hap = open.get_mut(path)?;
    let fps = crate::digest::RATE as f64;
    let frames = hap.frames().max(1);
    let t = source_time(local, span, frames as f64 / fps, reverse);
    let k = ((t * fps).max(0.0) as usize) % frames;
    let f = hap.frame(k).ok()?;
    Some(crate::capture::scale(&f.rgba(), f.width, f.height, w, h))
}

#[cfg(test)]
mod source_tests {
    use super::*;

    /// A 2×1 picture, left pixel red, right pixel blue.
    fn two_pixels() -> Vec<u8> {
        vec![255, 0, 0, 255, 0, 0, 255, 255]
    }

    #[test]
    fn cover_fills_the_frame_and_crops_the_overflow() {
        let out = fit_px(&two_pixels(), 2, 1, 4, 4, Fit::Cover);
        assert!(out.chunks_exact(4).all(|p| p[3] == 255), "no transparent pixel");
        let at = |x: usize, y: usize| &out[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
        assert_eq!(at(0, 0), &[255, 0, 0, 255], "left half is the red pixel");
        assert_eq!(at(1, 3), &[255, 0, 0, 255]);
        assert_eq!(at(2, 0), &[0, 0, 255, 255], "right half is the blue pixel");
        assert_eq!(at(3, 2), &[0, 0, 255, 255]);
    }

    #[test]
    fn contain_fits_whole_and_letterboxes_transparent() {
        let out = fit_px(&two_pixels(), 2, 1, 4, 4, Fit::Contain);
        let row = |y: usize| out[y * 16..(y + 1) * 16].chunks_exact(4).map(|p| p[3]).collect::<Vec<u8>>();
        assert!(row(0).iter().all(|a| *a == 0), "top bar is transparent");
        assert!(row(3).iter().all(|a| *a == 0), "bottom bar is transparent");
        assert!(row(1).iter().all(|a| *a == 255) && row(2).iter().all(|a| *a == 255));
        assert_eq!(&out[(2 * 4) * 4..(2 * 4) * 4 + 4], &[255, 0, 0, 255], "the picture's left edge sits at x=0");
        assert_eq!(&out[(2 * 4 + 3) * 4..(2 * 4 + 3) * 4 + 4], &[0, 0, 255, 255]);
    }

    #[test]
    fn stretch_still_fills_with_no_bars() {
        let out = fit_px(&two_pixels(), 2, 1, 4, 4, Fit::Stretch);
        assert!(out.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn a_still_honours_its_fit_config() {
        let dir = std::env::temp_dir().join(format!("mvj-still-fit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("wide.png");
        {
            let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&file).unwrap()), 2, 1);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&two_pixels()).unwrap();
        }
        let mut g = metal_graph::Graph::new("g", "t", metal_graph::Domain::Live);
        let id = g.add_node("s", "still", [0.0, 0.0]);
        let alpha = |g: &metal_graph::Graph, cfg: serde_json::Value| {
            g.node(&id).unwrap();
            let mut g = g.clone();
            g.node_mut(&id).unwrap().config = metal_graph::Config::from_json_str(&cfg.to_string()).unwrap();
            let px = source_frame(&dir, g.node(&id).unwrap(), 0.0, 4, 4).expect("pixels");
            px.chunks_exact(4).map(|p| p[3]).collect::<Vec<u8>>()
        };
        let file_s = file.to_string_lossy().to_string();
        assert!(alpha(&g, serde_json::json!({ "file": file_s, "fit": "contain" })).contains(&0));
        assert!(!alpha(&g, serde_json::json!({ "file": file_s, "fit": "cover" })).contains(&0));
        assert!(!alpha(&g, serde_json::json!({ "file": file_s })).contains(&0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A reversed clip walks its window backwards, with or without a passage.
    #[test]
    fn reverse_plays_backward_with_and_without_a_passage() {
        let fwd = |local: f64, span| source_time(local, span, 4.0, false);
        let back = |local: f64, span| source_time(local, span, 4.0, true);
        assert_eq!(fwd(1.0, None), 1.0, "forward is unchanged");
        assert_eq!(back(1.0, None), 3.0, "no passage: 1 s in is 3 s from the end");
        assert_eq!(back(3.5, None), 0.5);
        assert_eq!(fwd(1.0, Some((1.0, 3.0))), 2.0);
        assert_eq!(back(0.5, Some((1.0, 3.0))), 2.5, "a passage walks back inside itself");
        assert_eq!(back(2.5, Some((1.0, 3.0))), back(0.5, Some((1.0, 3.0))), "it repeats every passage length");
        assert_eq!(back(0.0, Some((2.0, 2.0))), 4.0, "an empty passage is the whole clip, walked back from its end");
    }

    /// A `still` node naming a PNG paints it; a `reel.clip` with nothing to
    /// play is `None` (black), never a panic.
    #[test]
    fn a_still_tile_paints_its_file() {
        let dir = std::env::temp_dir().join(format!("mvj-still-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("red.png");
        {
            let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&file).unwrap()), 4, 4);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let px: Vec<u8> = (0..16).flat_map(|_| [250u8, 10, 10, 255]).collect();
            enc.write_header().unwrap().write_image_data(&px).unwrap();
        }
        let mut g = metal_graph::Graph::new("g", "t", metal_graph::Domain::Live);
        let id = g.add_node("s", "still", [0.0, 0.0]);
        let cfg = serde_json::json!({ "file": file.to_string_lossy() });
        g.node_mut(&id).unwrap().config = metal_graph::Config::from_json_str(&cfg.to_string()).unwrap();
        let px = source_frame(&dir, g.node(&id).unwrap(), 0.0, 8, 8).expect("a still has pixels");
        assert_eq!(px.len(), 8 * 8 * 4);
        assert!(px.chunks_exact(4).all(|p| p[0] > 200 && p[1] < 40 && p[3] == 255));

        let clip = g.add_node("c", "reel.clip", [0.0, 0.0]);
        let cfg = serde_json::json!({ "library": "", "clip": "nothing.mp4" });
        g.node_mut(&clip).unwrap().config = metal_graph::Config::from_json_str(&cfg.to_string()).unwrap();
        assert!(source_frame(&dir, g.node(&clip).unwrap(), 1.0, 8, 8).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenes::Scene;

    fn batch(config: &str) -> metal_graph::Node {
        let mut g = metal_graph::Graph::new("t", "t", metal_graph::Domain::Live);
        let id = g.add_node("b", REEL_BATCH_KIND, [0.0, 0.0]);
        g.node_mut(&id).unwrap().config = metal_graph::Config::from_json_str(config).unwrap();
        g.node(&id).unwrap().clone()
    }

    fn images(n: usize) -> String {
        let items: Vec<String> = (0..n).map(|i| format!(r#"{{"kind":"image","path":"{i}.jpg"}}"#)).collect();
        items.join(",")
    }

    fn shown(node: &metal_graph::Node, f: &Favorites, t: f32) -> Option<Entry> {
        now(node, f, t).map(|(e, _)| e)
    }

    fn img(i: usize) -> Option<Entry> {
        Some(Entry::Image { path: format!("{i}.jpg") })
    }

    /// A `reel.clip` reads the canonical `{clip, passage}` and the canvas
    /// drop's `{file, start, end}` alike, so a dragged video or scene renders.
    #[test]
    fn a_clip_reads_both_the_canonical_and_the_drop_naming() {
        use serde_json::json;
        assert_eq!(
            clip_and_span(&json!({ "clip": "a.mp4", "passage": [1.0, 3.0] })),
            Some(("a.mp4", Some((1.0, 3.0))))
        );
        assert_eq!(
            clip_and_span(&json!({ "file": "b.mp4", "start": 2.0, "end": 4.0 })),
            Some(("b.mp4", Some((2.0, 4.0))))
        );
        assert_eq!(clip_and_span(&json!({ "file": "c.mp4" })), Some(("c.mp4", None)));
        assert_eq!(clip_and_span(&json!({ "start": 2.0, "end": 4.0 })), None);
        assert_eq!(
            clip_and_span(&json!({ "clip": "d.mp4", "passage": [0.0, 1.0], "start": 5.0, "end": 6.0 })),
            Some(("d.mp4", Some((0.0, 1.0))))
        );
    }

    #[test]
    fn an_old_batch_with_no_items_draws_nothing_and_reads_the_defaults() {
        let node = batch("{}");
        assert_eq!(now(&node, &Favorites::default(), 3.0), None);
        assert_eq!(stepping(&node), Stepping::default());
    }

    #[test]
    fn sequential_advances_on_the_interval_and_holds_the_last() {
        let node = batch(&format!(r#"{{"interval":2,"order":0,"items":[{}]}}"#, images(3)));
        let f = Favorites::default();
        assert_eq!(shown(&node, &f, 0.0), img(0));
        assert_eq!(shown(&node, &f, 1.99), img(0));
        assert_eq!(shown(&node, &f, 2.0), img(1));
        assert_eq!(shown(&node, &f, 4.5), img(2));
        assert_eq!(shown(&node, &f, 60.0), img(2), "sequential plays once and holds");
        let looping = batch(&format!(r#"{{"interval":2,"items":[{}]}}"#, images(3)));
        assert_eq!(shown(&looping, &f, 6.0), img(0), "the default loops");
    }

    #[test]
    fn beats_mode_advances_on_the_beat_clock() {
        let mut g = metal_graph::Graph::new("t", "t", metal_graph::Domain::Live);
        let id = g.add_node("b", REEL_BATCH_KIND, [0.0, 0.0]);
        g.node_mut(&id).unwrap().config =
            metal_graph::Config::from_json_str(&format!(r#"{{"interval":4,"unit":1,"items":[{}]}}"#, images(3)))
                .unwrap();
        let f = Favorites::default();
        // Show time is fixed; only the stamped beat count moves the batch.
        stamp(&mut g, 3.9, 0.5);
        assert_eq!(shown(g.node(&id).unwrap(), &f, 100.0), img(0));
        stamp(&mut g, 4.0, 0.5);
        assert_eq!(shown(g.node(&id).unwrap(), &f, 100.0), img(1));
        let (_, local) = {
            stamp(&mut g, 9.0, 0.5);
            now(g.node(&id).unwrap(), &f, 100.0).unwrap()
        };
        assert!((local - 0.5).abs() < 1e-4, "one beat into the step is half a second at 120 BPM");
        // The beat clock itself: a stand-in at a known tempo.
        let sim = crate::live::Input::stand_in();
        let (b10, spb) = beat_clock(&sim, 10.0);
        let (b20, _) = beat_clock(&sim, 20.0);
        assert!(spb > 0.0 && b20 > b10, "the input's beat count runs forward");
    }

    #[test]
    fn random_is_stable_per_seed_and_covers_a_large_list() {
        let n = 500;
        let s = Stepping { interval: 1.0, beats: false, order: Order::Random, seed: 7 };
        let a: Vec<usize> = (0..n as u64 * 3).map(|k| index_at(&s, n, k)).collect();
        let b: Vec<usize> = (0..n as u64 * 3).map(|k| index_at(&s, n, k)).collect();
        assert_eq!(a, b, "the same tick picks the same item");
        let other = Stepping { seed: 8, ..s };
        assert_ne!(a[..n], (0..n as u64).map(|k| index_at(&other, n, k)).collect::<Vec<_>>()[..]);
        for pass in a.chunks(n) {
            let mut seen = pass.to_vec();
            seen.sort_unstable();
            assert_eq!(seen, (0..n).collect::<Vec<_>>(), "every item once per pass");
        }
        assert!(a.windows(2).all(|w| w[0] != w[1]), "never the same item twice in a row");
        assert_ne!(a[..n], (0..n).collect::<Vec<_>>()[..], "shuffled, not in order");
    }

    #[test]
    fn a_group_item_follows_edits_to_the_group() {
        let root = std::env::temp_dir().join(format!("metal-reel-batch-group-{}", std::process::id()));
        metal_vfs::create_dir_all(root.join("scenes")).unwrap();
        let scene = |clip: &str| Scene { clip: clip.into(), start: 1.0, end: 3.0, ..Default::default() };
        let mut fav = Favorites::default();
        fav.scenes.insert("a".into(), scene("a.mp4"));
        fav.scenes.insert("b".into(), scene("b.mp4"));
        fav.groups.insert("club".into(), vec!["a".into()]);
        let write = |f: &Favorites| {
            metal_vfs::write(crate::scenes::path(&root), serde_json::to_string(f).unwrap()).unwrap();
        };
        write(&fav);
        let node = batch(r#"{"interval":1,"items":[{"kind":"group","name":"club"}]}"#);
        let at = |t: f32| shown(&node, &crate::scenes::live(&root), t);
        let a = Some(Entry::Scene { clip: "a.mp4".into(), start: 1.0, end: 3.0 });
        let b = Some(Entry::Scene { clip: "b.mp4".into(), start: 1.0, end: 3.0 });
        assert_eq!(at(1.0), a);
        std::thread::sleep(std::time::Duration::from_millis(20));
        fav.groups.insert("club".into(), vec!["a".into(), "b".into()]);
        write(&fav);
        assert_eq!(at(1.0), b, "the edited group is the list from the next tick on");
        metal_vfs::remove_dir_all(&root).ok();
    }

    #[test]
    fn items_of_every_kind_parse_and_unknown_ones_are_skipped() {
        let node = batch(
            r#"{"items":[{"kind":"video","path":"v.mp4"},{"kind":"image","path":"i.jpg"},
                {"kind":"scene","id":"s"},{"kind":"group","name":"g"},{"kind":"nope"}]}"#,
        );
        assert_eq!(items(&node).len(), 4);
    }

    /// 1c: a scene dropped on the canvas writes `{file, scene, start, end}`.
    /// The reel reads its file and its window and ignores nothing it needs: the
    /// window comes through `clip_and_span` as the loop, and a plain drop (no
    /// window) loops the whole clip.
    #[test]
    fn a_dropped_scene_hands_its_window_to_the_reel() {
        let scene = serde_json::json!({ "file": "/v/a.mp4", "scene": "a-2#0-5000", "start": 2.0, "end": 5.0 });
        assert_eq!(clip_and_span(&scene), Some(("/v/a.mp4", Some((2.0, 5.0)))));
        let plain = serde_json::json!({ "file": "/v/a.mp4" });
        assert_eq!(clip_and_span(&plain), Some(("/v/a.mp4", None)));
        let inverted = serde_json::json!({ "file": "/v/a.mp4", "start": 5.0, "end": 2.0 });
        assert_eq!(clip_and_span(&inverted), Some(("/v/a.mp4", None)), "an empty window is no window");
    }

    /// A dropped video's window is its loop: a video item with `start`/`end`
    /// keeps the span through `expand`, and `source_time` then stays inside
    /// `[start, end)` for every local time, where a plain video walks its
    /// whole length.
    #[test]
    fn a_video_item_loops_its_window_and_a_plain_one_its_length() {
        let node = batch(r#"{"items":[{"kind":"video","path":"v.mp4","start":2.0,"end":5.0},{"kind":"video","path":"w.mp4"}]}"#);
        let entries = expand(&items(&node), &Favorites::default());
        let Entry::Video { span: windowed, .. } = &entries[0] else { panic!("a video") };
        let Entry::Video { span: plain, .. } = &entries[1] else { panic!("a video") };
        assert_eq!(*windowed, Some((2.0, 5.0)));
        assert_eq!(*plain, None, "no window: the whole clip");
        for step in 0..60 {
            let t = source_time(step as f64 * 0.37, *windowed, 60.0, false);
            assert!((2.0..5.0).contains(&t), "{t} left the window");
        }
        assert_eq!(source_time(0.0, *plain, 60.0, false), 0.0);
        assert!((source_time(30.0, *plain, 60.0, false) - 30.0).abs() < 1e-9, "a plain video walks its length");
    }
}
