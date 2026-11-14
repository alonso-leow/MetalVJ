//! `fx.image-pop` — reel media drawn as N instances over the picture.
//!
//! See [`crate::fx`]'s module doc for where this sits and why the atlas is
//! composited on the CPU. Media (stills and short clips) live in a reel as they
//! do today (`data/reels/<name>/`); the node's config names one
//! (`{"reel": "<name>"}`) and the effect cycles its frames.
//!
//! # The instance model, and why it is stateless
//!
//! Instance `i` is born at `i / rate` seconds and lives `life` seconds, so the
//! set alive at `time` is a pure function of the clock and the ingest — the
//! same discipline decision 6 asks of the dust. At most `concurrent` instances
//! are drawn (the newest ones), the media index cycles the atlas round-robin
//! when `select` is unwired, and every position and scale is hashed from
//! `(seed, i)`. Nothing is remembered between frames.
//!
//! # Ports
//!
//! `in` image, `control` (the floats), and `select` — a `Control` input the
//! catalog declares now, for `ai.curator` to feed in wave 2. Nothing reads it
//! yet: [`compose`] takes it as `None`, which is the round-robin case.
//!
//! # The config keys, in the order the compositor reads them
//!
//! | key | meaning | range |
//! |---|---|---|
//! | `rate` | pops per second | 0..20 |
//! | `concurrent` | most instances at once | 1..32 |
//! | `life` | seconds on screen | 0.1..20 |
//! | `fade` | in/out fraction of `life` | 0..0.5 |
//! | `scale` | instance size, fraction of the frame width | 0..2 |
//! | `scale_jitter` | per-instance size spread | 0..1 |
//! | `spread` | 0 centre, 1 the whole frame | 0..1 |
//! | `position_x` | centre of the spread | 0..1 |
//! | `position_y` | centre of the spread | 0..1 |
//! | `seed` | the instance layout's identity | 0..1000 |
//! | `blend` | over/add/screen as a stepped float | 0..1 |
//! | `crop_zoom` | how far into the media to zoom | 0..1 |

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use metal_reel::index::{self, Library};
use metal_reel::read::Rgb;

use super::unit;

/// The effect's kind.
pub const FX_IMAGE_POP_KIND: &str = "fx.image-pop";

/// The shader id `fx.image-pop` names. Kept as a file so the GPU path can pick
/// it up the day the effect contract carries an atlas binding — see
/// [`crate::fx`]'s module doc.
pub const IMAGE_POP_SHADER: &str = "image_pop";

/// At most this many reel rungs are decoded into an [`Atlas`] — the brief's
/// own ceiling, kept so a large library is not held in memory.
pub const MAX_ATLAS_FRAMES: usize = 32;

/// The `blend` values the stepped float names.
pub const BLEND_OVER: f32 = 0.0;
pub const BLEND_ADD: f32 = 0.5;
pub const BLEND_SCREEN: f32 = 1.0;

/// `fx.image-pop`'s twelve floats, read off the node's config with their
/// declared defaults when unset and clamped to their declared ranges.
pub fn image_pop_ingest(node: &metal_graph::Node) -> [f32; 12] {
    let config: serde_json::Value =
        serde_json::from_str(&node.config.to_json_str()).unwrap_or(serde_json::Value::Null);
    let get = |key: &str, default: f32, min: f32, max: f32| {
        config.get(key).and_then(serde_json::Value::as_f64).map(|v| v as f32).unwrap_or(default).clamp(min, max)
    };
    [
        get("rate", 2.0, 0.0, 20.0),
        get("concurrent", 8.0, 1.0, 32.0),
        get("life", 3.0, 0.1, 20.0),
        get("fade", 0.2, 0.0, 0.5),
        get("scale", 0.5, 0.0, 2.0),
        get("scale_jitter", 0.2, 0.0, 1.0),
        get("spread", 0.5, 0.0, 1.0),
        get("position_x", 0.5, 0.0, 1.0),
        get("position_y", 0.5, 0.0, 1.0),
        get("seed", 0.0, 0.0, 1000.0),
        get("blend", BLEND_OVER, 0.0, 1.0),
        get("crop_zoom", 0.0, 0.0, 1.0),
    ]
}

/// The reel a node names, `config.reel`. `None` when it names none — the
/// effect then draws nothing rather than guessing a folder.
pub fn reel_of(node: &metal_graph::Node) -> Option<String> {
    let config: serde_json::Value =
        serde_json::from_str(&node.config.to_json_str()).unwrap_or(serde_json::Value::Null);
    config.get("reel")?.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// The one `fx.image-pop` node's shader id and ingest, when `fx_node` is one.
pub fn image_pop_node(graph: &metal_graph::Graph, fx_node: &str) -> Option<(String, [f32; 12])> {
    let node = graph.node(fx_node)?;
    (node.kind == FX_IMAGE_POP_KIND).then(|| (IMAGE_POP_SHADER.to_string(), image_pop_ingest(node)))
}

/// One instance alive at a given instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pop {
    /// Centre in the frame's normalized coordinates, 0..1.
    pub center: [f32; 2],
    /// Width as a fraction of the frame width.
    pub scale: f32,
    /// The fade envelope at this instant, 0..1.
    pub alpha: f32,
    /// Which atlas frame it shows.
    pub frame: usize,
}

/// The instances alive at `time`, newest first, capped at `concurrent`.
///
/// Pure in `(ingest, time, select, frame_count)`. `select` is the normalized
/// index `ai.curator` will feed in wave 2; `None` is the round-robin default.
pub fn pops(ingest: &[f32; 12], time: f32, select: Option<f32>, frame_count: usize) -> Vec<Pop> {
    if frame_count == 0 {
        return Vec::new();
    }
    let rate = ingest[0].clamp(0.0, 20.0);
    if rate <= 0.0 {
        return Vec::new();
    }
    let concurrent = ingest[1].clamp(1.0, 32.0) as usize;
    let life = ingest[2].clamp(0.1, 20.0);
    let fade = ingest[3].clamp(0.0, 0.5);
    let spread = ingest[6].clamp(0.0, 1.0);
    let (px, py) = (ingest[7].clamp(0.0, 1.0), ingest[8].clamp(0.0, 1.0));
    let seed = ingest[9].clamp(0.0, 1000.0) as u32;
    let base = select
        .map(|s| (s.clamp(0.0, 1.0) * (frame_count.saturating_sub(1)) as f32).round() as usize)
        .unwrap_or(0);

    let last = (time * rate).floor() as i64;
    let first = ((time * rate - life * rate).ceil() as i64).max(0);
    let mut out = Vec::new();
    // Newest first: the cap keeps the most recent instances, which is what a
    // pop looks like as it arrives.
    for i in (first..=last).rev() {
        if out.len() >= concurrent {
            break;
        }
        let age = time - i as f32 / rate;
        if age < 0.0 || age >= life {
            continue;
        }
        let f = fade * life;
        let alpha = if f <= 1e-6 { 1.0 } else { (age / f).min((life - age) / f).clamp(0.0, 1.0) };
        let u = i as u32;
        let ox = unit(seed, u, 10) * 2.0 - 1.0;
        let oy = unit(seed, u, 11) * 2.0 - 1.0;
        let scale = (ingest[4] * (1.0 + ingest[5].clamp(0.0, 1.0) * (unit(seed, u, 12) * 2.0 - 1.0)))
            .max(0.0);
        let frame = (base + i.rem_euclid(frame_count as i64) as usize) % frame_count;
        out.push(Pop {
            center: [(px + ox * spread * 0.5).clamp(0.0, 1.0), (py + oy * spread * 0.5).clamp(0.0, 1.0)],
            scale,
            alpha,
            frame,
        });
    }
    out
}

/// The decoded frames of one reel, held in memory. At most
/// [`MAX_ATLAS_FRAMES`] of them, sampled evenly across the library so a
/// multi-clip reel is represented rather than just its first clip.
pub struct Atlas {
    pub frames: Vec<Rgb>,
}

impl Atlas {
    pub fn empty() -> Atlas {
        Atlas { frames: Vec::new() }
    }

    /// Decodes up to `max` rungs of `lib`, reusing [`metal_reel::index::decode`] —
    /// the same reader the ladder was described with. A rung that will not
    /// decode is skipped rather than failing the atlas.
    pub fn from_library(lib: &Library, max: usize) -> Atlas {
        let total: usize = lib.clips.iter().map(|c| c.rungs()).sum();
        let take = max.min(total);
        if take == 0 {
            return Atlas::empty();
        }
        let mut frames = Vec::with_capacity(take);
        for k in 0..take {
            let flat = k * total / take;
            let mut rem = flat;
            for clip in &lib.clips {
                let n = clip.rungs();
                if rem < n {
                    if let Ok(rgb) = index::decode(&lib.rung_path(clip, rem)) {
                        frames.push(rgb);
                    }
                    break;
                }
                rem -= n;
            }
        }
        Atlas { frames }
    }
}

/// The atlas cache, keyed by data root + reel + the library's own size. A
/// library rebuilt with the same name and rung count would keep its atlas;
/// that is the same "the index is the identity" trade the rest of the reel
/// code makes, and a rebuilt library almost always changes its rung count.
fn cache() -> &'static Mutex<HashMap<String, Arc<Atlas>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Atlas>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Loads (and caches) the atlas for `name` under `data/reels/`. `None` when the
/// library does not exist — the effect then leaves the picture alone.
pub fn load_atlas(data: &Path, name: &str) -> Option<Arc<Atlas>> {
    let lib = Library::load(data, name).ok()?;
    let key = format!("{}:{}:{}:{}", data.display(), name, lib.clips.len(), lib.rungs());
    let mut cache = cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(atlas) = cache.get(&key) {
        return Some(atlas.clone());
    }
    let atlas = Arc::new(Atlas::from_library(&lib, MAX_ATLAS_FRAMES));
    cache.insert(key, atlas.clone());
    Some(atlas)
}

/// Bilinear sample of an RGB frame at `(u, v)` in 0..1, clamped to the edges.
fn sample(rgb: &Rgb, u: f32, v: f32) -> [f32; 3] {
    let (w, h) = (rgb.width as f32, rgb.height as f32);
    if w <= 1.0 || h <= 1.0 || rgb.px.len() < rgb.width as usize * rgb.height as usize * 3 {
        return [0.0, 0.0, 0.0];
    }
    let x = (u.clamp(0.0, 1.0) * (w - 1.0)).clamp(0.0, w - 1.0);
    let y = (v.clamp(0.0, 1.0) * (h - 1.0)).clamp(0.0, h - 1.0);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(rgb.width as usize - 1), (y0 + 1).min(rgb.height as usize - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let at = |px: usize, py: usize, c: usize| rgb.px[(py * rgb.width as usize + px) * 3 + c] as f32 / 255.0;
    let mut out = [0.0f32; 3];
    for c in 0..3 {
        let top = at(x0, y0, c) * (1.0 - tx) + at(x1, y0, c) * tx;
        let bot = at(x0, y1, c) * (1.0 - tx) + at(x1, y1, c) * tx;
        out[c] = top * (1.0 - ty) + bot * ty;
    }
    out
}

/// Blends `src` into `dst` under the stepped `blend` float.
fn blend(dst: f32, src: f32, a: f32, mode: f32) -> f32 {
    let v = if mode < 0.25 {
        dst * (1.0 - a) + src * a
    } else if mode < 0.75 {
        dst + src * a
    } else {
        1.0 - (1.0 - dst) * (1.0 - src * a)
    };
    v.clamp(0.0, 1.0)
}

/// Draws the pops of `atlas` over `frame`, on the CPU.
///
/// Pure in `(frame, atlas, ingest, time, select)` — the live path for both
/// painters, so the CPU and GPU builds cannot drift. `select` is reserved for
/// `ai.curator` (wave 2); `None` cycles the atlas round-robin.
pub fn compose(
    frame: &mut (Vec<u8>, u32, u32),
    atlas: &Atlas,
    ingest: &[f32; 12],
    time: f32,
    select: Option<f32>,
) {
    let (w, h) = (frame.1, frame.2);
    if w == 0 || h == 0 || atlas.frames.is_empty() || frame.0.len() < (w as usize * h as usize * 4) {
        return;
    }
    let mode = ingest[10].clamp(0.0, 1.0);
    let crop = ingest[11].clamp(0.0, 1.0);
    for pop in pops(ingest, time, select, atlas.frames.len()) {
        let media = &atlas.frames[pop.frame];
        if media.width == 0 || media.height == 0 {
            continue;
        }
        // Crop is a centred zoom into the media.
        let (su0, sv0) = (crop * 0.5, crop * 0.5);
        let (su1, sv1) = (1.0 - crop * 0.5, 1.0 - crop * 0.5);
        let src_w = (su1 - su0) * media.width as f32;
        let src_h = (sv1 - sv0) * media.height as f32;
        let dw = pop.scale * w as f32;
        let dh = dw * (src_h / src_w.max(1e-3));
        if dw < 1.0 || dh < 1.0 {
            continue;
        }
        let (cx, cy) = (pop.center[0] * w as f32, pop.center[1] * h as f32);
        let x0 = (cx - dw * 0.5).floor().max(0.0) as i32;
        let x1 = (cx + dw * 0.5).ceil().min(w as f32 - 1.0) as i32;
        let y0 = (cy - dh * 0.5).floor().max(0.0) as i32;
        let y1 = (cy + dh * 0.5).ceil().min(h as f32 - 1.0) as i32;
        for py in y0..=y1 {
            for px in x0..=x1 {
                let u = su0 + ((px as f32 - (cx - dw * 0.5)) / dw) * (su1 - su0);
                let v = sv0 + ((py as f32 - (cy - dh * 0.5)) / dh) * (sv1 - sv0);
                let rgb = sample(media, u, v);
                let p = (py as usize * w as usize + px as usize) * 4;
                for c in 0..3 {
                    let dst = frame.0[p + c] as f32 / 255.0;
                    frame.0[p + c] = (blend(dst, rgb[c], pop.alpha, mode) * 255.0).round() as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_reel::index::{Clip, Stamp};
    use metal_graph::{Domain, Graph};

    fn frame(w: u32, h: u32) -> (Vec<u8>, u32, u32) {
        (vec![0u8; (w as usize * h as usize * 4)], w, h)
    }

    fn node_with(json: &str) -> metal_graph::Node {
        let mut n = metal_graph::Node::new("fx-1", FX_IMAGE_POP_KIND, [0.0, 0.0]);
        n.config = metal_graph::Config::from_json_str(json).unwrap();
        n
    }

    fn frame_hash(px: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in px {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// Three solid frames, so the golden is a pure function of the layout and
    /// not of any decoder.
    fn solid_atlas() -> Atlas {
        let frame_of = |r: u8, g: u8, b: u8| {
            let mut px = Vec::with_capacity(16 * 16 * 3);
            for _ in 0..16 * 16 {
                px.extend_from_slice(&[r, g, b]);
            }
            Rgb { width: 16, height: 16, px }
        };
        Atlas { frames: vec![frame_of(230, 40, 40), frame_of(40, 210, 90), frame_of(60, 90, 230)] }
    }

    #[test]
    fn image_pop_ingest_reads_the_config_with_defaults_and_clamps() {
        let n = node_with(r#"{"rate":4.0,"concurrent":99,"seed":5}"#);
        let got = image_pop_ingest(&n);
        assert_eq!(got[0], 4.0);
        assert_eq!(got[1], 32.0, "concurrent clamps to 32");
        assert_eq!(got[9], 5.0);
        assert_eq!(got[2], 3.0, "life default");
    }

    #[test]
    fn image_pop_never_exceeds_concurrent() {
        let mut n = node_with(r#"{"rate":10,"life":5,"concurrent":8,"seed":1}"#);
        let ingest = image_pop_ingest(&n);
        // rate * life = 50 would be alive uncapped; the cap is 8.
        for t in [0.0f32, 0.3, 1.0, 2.5, 9.9] {
            let alive = pops(&ingest, t, None, 3);
            assert!(alive.len() <= 8, "{} pops at t={t}", alive.len());
        }
        // And the cap is the configured one, not a fixed number.
        n.config = metal_graph::Config::from_json_str(r#"{"rate":10,"life":5,"concurrent":3}"#).unwrap();
        let ingest = image_pop_ingest(&n);
        assert_eq!(pops(&ingest, 1.0, None, 3).len(), 3);
    }

    #[test]
    fn image_pop_unwired_cycles_the_reel() {
        // Many alive at once, three frames: the newest three must show all
        // three frames, in order, because the index is `i % frames`.
        let ingest = image_pop_ingest(&node_with(r#"{"rate":8,"life":3,"concurrent":6,"seed":2}"#));
        let alive = pops(&ingest, 1.0, None, 3);
        assert!(alive.len() >= 3);
        let shown: Vec<usize> = alive.iter().take(3).map(|p| p.frame).collect();
        let mut sorted = shown.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2], "the three newest instances must cycle the reel: {shown:?}");
        // Consecutive instances differ, which is what "round-robin" means.
        assert_ne!(shown[0], shown[1]);
        assert_ne!(shown[1], shown[2]);
    }

    #[test]
    fn image_pop_stays_inside_the_mask() {
        use crate::output::{apply_masks, Mask};
        let atlas = solid_atlas();
        let ingest = image_pop_ingest(&node_with(
            r#"{"rate":8,"life":4,"concurrent":16,"scale":0.8,"spread":1.0,"seed":3,"blend":0}"#,
        ));
        let mut f = frame(64, 36);
        compose(&mut f, &atlas, &ingest, 1.0, None);
        let before = f.0.clone();
        assert!(before.iter().any(|b| *b != 0), "the pops must draw something");
        // The test is only meaningful if the pops actually reach the half the
        // mask will remove — otherwise the mask would be cutting nothing.
        let non_black = |px: &[u8], from: usize, to: usize| {
            (0..f.2 as usize).any(|y| {
                (from..to).any(|x| {
                    let p = (y * f.1 as usize + x) * 4;
                    px[p] != 0 || px[p + 1] != 0 || px[p + 2] != 0
                })
            })
        };
        let half = f.1 as usize / 2;
        assert!(non_black(&before, 0, half), "pops must land in the left half too");
        assert!(non_black(&before, half, f.1 as usize), "pops must reach the masked-away right half");
        // A mask over the left half, in the frame's normalized coordinates.
        let mask = Mask {
            id: "half".into(),
            name: "half".into(),
            points: vec![[0.0, 0.0], [0.5, 0.0], [0.5, 1.0], [0.0, 1.0]],
            ..Mask::default()
        };
        apply_masks(&mut f.0, f.1, f.2, &[mask]);
        // Everything the mask left is in the left half; the right half is gone.
        for y in 0..f.2 as usize {
            for x in 0..f.1 as usize {
                let p = (y * f.1 as usize + x) * 4;
                if f.0[p] != 0 || f.0[p + 1] != 0 || f.0[p + 2] != 0 {
                    assert!(x < f.1 as usize / 2, "a pop leaked past the mask at ({x},{y})");
                }
            }
        }
    }

    #[test]
    fn an_atlas_reads_rungs_from_a_reel_folder() {
        let dir = std::env::temp_dir().join(format!("fw-image-pop-atlas-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        let root = dir.join("reels").join("test-reel");
        let clip_dir = root.join("frames").join("clip");
        metal_vfs::create_dir_all(&clip_dir).unwrap();
        for k in 0..4u32 {
            let mut px = Vec::new();
            for _ in 0..8 * 8 {
                px.extend_from_slice(&[(k * 60) as u8, 20, 200]);
            }
            let mut out = Vec::new();
            jpeg_encoder::Encoder::new(&mut out, 82)
                .encode(&px, 8, 8, jpeg_encoder::ColorType::Rgb)
                .unwrap();
            metal_vfs::write(clip_dir.join(format!("{:06}.jpg", k + 1)), out).unwrap();
        }
        let lib = Library {
            name: "test-reel".into(),
            source: dir.display().to_string(),
            rate: index::FPS,
            width: 8,
            clips: vec![Clip {
                id: "clip".into(),
                file: "clip.mp4".into(),
                seconds: 0.5,
                stamp: Stamp { bytes: 0, modified: 0 },
                rows: vec![[0.0; metal_reel::read::COLUMNS]; 4],
                passages: vec![],
            }],
            root: root.clone(),
        };
        lib.save().unwrap();
        let atlas = Atlas::from_library(&lib, MAX_ATLAS_FRAMES);
        assert_eq!(atlas.frames.len(), 4, "four rungs decode");
        assert_eq!((atlas.frames[0].width, atlas.frames[0].height), (8, 8));

        // The loader reads the same library off disk and caches it: the second
        // call is the same atlas, not a second decode.
        let loaded = load_atlas(&dir, "test-reel").expect("the reel loads");
        assert_eq!(loaded.frames.len(), 4);
        let again = load_atlas(&dir, "test-reel").expect("cached");
        assert!(Arc::ptr_eq(&loaded, &again), "the atlas must be cached");
        assert!(load_atlas(&dir, "no-such-reel").is_none());
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn the_image_pop_shader_is_on_disk_for_the_gpu_path() {
        let wgsl = crate::forge::source(&crate::output::data_root(), IMAGE_POP_SHADER)
            .expect("data/shaders/image_pop.wgsl is a built-in shader");
        assert!(wgsl.contains("fs_main"), "the shader must be a forge effect");
        assert!(wgsl.contains("texture_2d"), "the atlas arrives as the shader's input texture");
    }

    #[test]
    fn image_pop_node_is_found_by_kind_and_names_its_reel() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let n = g.add_node("fx", FX_IMAGE_POP_KIND, [0.0, 0.0]);
        g.node_mut(&n).unwrap().config = metal_graph::Config::from_json_str(r#"{"reel":"foo"}"#).unwrap();
        let node = g.node(&n).unwrap().clone();
        assert_eq!(reel_of(&node).as_deref(), Some("foo"));
        assert!(image_pop_node(&g, &n).is_some());
        let m = g.add_node("fx", "fx.move", [1.0, 0.0]);
        assert!(image_pop_node(&g, &m).is_none());
    }

    /// The pops' frozen frame at a fixed seed and instant, through the same
    /// `compose` call the live path makes. Re-freeze with `FW_PRINT_GOLDEN=1`.
    #[test]
    fn the_image_pop_frame_is_frozen_for_a_seed() {
        let atlas = solid_atlas();
        let ingest = image_pop_ingest(&node_with(
            r#"{"rate":3,"life":4,"concurrent":5,"scale":0.6,"spread":0.7,"seed":9,"blend":0.5,"crop_zoom":0.1}"#,
        ));
        let mut f = frame(64, 36);
        compose(&mut f, &atlas, &ingest, 2.0, None);
        let hash = frame_hash(&f.0);
        if std::env::var("FW_PRINT_GOLDEN").is_ok() {
            eprintln!("GOLDEN image-pop {hash:#018x}");
        }
        assert_eq!(
            hash, 0x64e548664dedea08,
            "the image-pop frame changed; if that is deliberate, re-freeze this value"
        );
    }
}
