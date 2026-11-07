//! The gallery's animated pictures: one GIF per `gen.*` and `fx.*` kind, the
//! four float variants a card cycles through, written once by an ignored
//! test ([`generate`]) into `~/.cache/metalvj/gallery/<kind>.gif`.
//!
//! The shell reads a kind's GIF lazily, the first time one of its cards is on
//! screen: it is decoded once and its frames are uploaded as textures once,
//! then kept in the egui context. A kind with no file has no entry and its
//! card keeps the live picture from the render route. Nothing is decoded per
//! frame; a card only asks for a repaint when its next frame is due.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{ColorImage, Context, Id, TextureHandle, TextureOptions};

use super::preview_cache::{PreviewCache, Space};
use crate::call::render::STAND_IN_HASH;
use crate::call::{Call, Params};

/// How long one frame stays up: the gallery's own cadence, one float variant
/// per second.
pub const FRAME_SECS: f64 = 1.0;

/// The directory the GIFs live in (`~/.cache/metalvj/gallery`), or `None`
/// without a home directory.
pub fn cache_dir() -> Option<PathBuf> {
    let home = metal_config::home_dir();
    (!home.as_os_str().is_empty() && home != PathBuf::from("."))
        .then(|| home.join(".cache/metalvj/gallery"))
}

/// The file a kind's GIF is stored under: the kind's own name, with anything
/// that is not a plain filename character replaced.
pub fn file_name(kind: &str) -> String {
    let safe: String = kind.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("{safe}.gif")
}

/// The frame a card shows at `time` (seconds): frames advance once per
/// [`FRAME_SECS`] and loop.
pub fn frame_index(time: f64, frames: usize) -> usize {
    if frames == 0 {
        return 0;
    }
    ((time.max(0.0) / FRAME_SECS) as usize) % frames
}

/// Seconds until the next frame of an animation is due at `time`.
pub fn secs_to_next_frame(time: f64) -> f64 {
    FRAME_SECS - time.max(0.0) % FRAME_SECS
}

/// Decodes a GIF into its frames as RGBA pixels, with the size of the canvas.
pub fn decode(bytes: &[u8]) -> Option<([usize; 2], Vec<Vec<u8>>)> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut reader = options.read_info(bytes).ok()?;
    let size = [usize::from(reader.width()), usize::from(reader.height())];
    let mut frames = Vec::new();
    while let Some(frame) = reader.read_next_frame().ok()? {
        frames.push(frame.buffer.to_vec());
    }
    (!frames.is_empty()).then_some((size, frames))
}

/// Encodes RGB frames of one size as a looping GIF, one frame per
/// [`FRAME_SECS`]. The generator and the listened route write these; the shell
/// only reads them.
pub fn encode(size: [usize; 2], frames: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let (w, h) = (size[0] as u16, size[1] as u16);
    let mut out = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut out, w, h, &[]).map_err(|e| e.to_string())?;
        encoder.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
        for rgb in frames {
            let mut frame = gif::Frame::from_rgb_speed(w, h, rgb, 10);
            frame.delay = (FRAME_SECS * 100.0).round() as u16;
            encoder.write_frame(&frame).map_err(|e| e.to_string())?;
        }
    }
    Ok(out)
}

/// The textures of every frame of one kind's GIF, shared by its cards.
pub type Frames = Arc<Vec<TextureHandle>>;

/// What the context remembers per kind: its frames, and whether they are a heard
/// picture (a stand-in does not count). `frames` is `None` when neither picture
/// is a GIF that decodes, so the files are looked for once, not every frame.
#[derive(Clone, Default)]
struct Entry {
    frames: Option<Frames>,
    listened: bool,
}

#[derive(Clone, Default)]
struct Cache(HashMap<String, Entry>);

fn cache_id() -> Id {
    Id::new("shell/gallery-anim/cache")
}

/// The animated frames of `kind`'s card, loaded on first use: the listened
/// picture from the preview cache when it has one, else the synthetic
/// placeholder. `None` when neither exists or decodes.
pub fn frames(ctx: &Context, kind: &str) -> Option<Frames> {
    frames_with_root(ctx, kind, PreviewCache::default_root().as_deref(), cache_dir().as_deref())
}

/// Whether `kind`'s frames are a heard picture, not the placeholder or a stand-in.
pub fn is_listened(ctx: &Context, kind: &str) -> bool {
    entry_with_roots(ctx, kind, PreviewCache::default_root().as_deref(), cache_dir().as_deref()).listened
}

/// Drops `kind`'s cached frames so the next [`frames`] reads the disk again.
/// Called after the kind's listened picture was re-rendered.
pub fn reload(ctx: &Context, kind: &str) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(cache_id()).0.remove(kind));
}

/// [`frames`] with the directories given: the preview cache root (the listened
/// picture is read from its gallery space) and the placeholder directory.
fn frames_with_root(ctx: &Context, kind: &str, preview_root: Option<&Path>, placeholder_dir: Option<&Path>) -> Option<Frames> {
    entry_with_roots(ctx, kind, preview_root, placeholder_dir).frames
}

fn entry_with_roots(ctx: &Context, kind: &str, preview_root: Option<&Path>, placeholder_dir: Option<&Path>) -> Entry {
    let hit = ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(cache_id()).0.get(kind).cloned());
    if let Some(entry) = hit {
        return entry;
    }
    // Loaded outside the data lock: uploading a texture takes the context.
    let loaded = load_entry(ctx, kind, preview_root, placeholder_dir);
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(cache_id()).0.insert(kind.to_string(), loaded.clone()));
    loaded
}

fn load_entry(ctx: &Context, kind: &str, preview_root: Option<&Path>, placeholder_dir: Option<&Path>) -> Entry {
    let cache = preview_root.map(PreviewCache::new);
    let picture = cache
        .as_ref()
        .and_then(|c| c.read_gif(Space::Gallery, kind))
        .and_then(|bytes| load_from_bytes(ctx, kind, &bytes));
    if let Some(textures) = picture {
        // A stand-in shows its frames, but its card still lacks a heard picture
        // and keeps asking for one.
        let stand_in = cache
            .as_ref()
            .and_then(|c| c.meta(Space::Gallery, kind))
            .is_some_and(|m| m.input_hash == STAND_IN_HASH);
        return Entry { frames: Some(Arc::new(textures)), listened: !stand_in };
    }
    let placeholder = placeholder_dir
        .and_then(|dir| std::fs::read(dir.join(file_name(kind))).ok())
        .and_then(|bytes| load_from_bytes(ctx, kind, &bytes));
    Entry { frames: placeholder.map(Arc::new), listened: false }
}

/// Uploads the frames of one GIF's bytes as textures. `None` when they do not decode.
fn load_from_bytes(ctx: &Context, kind: &str, bytes: &[u8]) -> Option<Vec<TextureHandle>> {
    let (size, frames) = decode(bytes)?;
    Some(
        frames
            .iter()
            .enumerate()
            .map(|(i, rgba)| {
                let image = ColorImage::from_rgba_unmultiplied(size, rgba);
                ctx.load_texture(format!("gallery-anim/{kind}/{i}"), image, TextureOptions::LINEAR)
            })
            .collect(),
    )
}

/// The route a listened picture is read from (`call::render::kind_listened`).
pub const LISTEN_PATH: &str = "/author/render/kind_listened";

/// The least time between two unselected asks: the gallery asks for one kind
/// at a time, so a grid of cards does not start a render per card at once.
pub const ASK_GAP_SECS: f64 = 0.5;

/// How long an ask may go unanswered before it is given up.
pub const ASK_TIMEOUT_SECS: f64 = 10.0;

/// Whether `kind` has a card with a listened picture: the `fx.*` and `mat.*` kinds.
pub fn is_listen_kind(kind: &str) -> bool {
    kind.starts_with("fx.") || kind.starts_with("mat.")
}

/// The kind to ask for without selecting it: the first of `visible` that is a
/// listen kind, is not in `asked` yet and `lacks` its picture, once
/// [`ASK_GAP_SECS`] has passed since `last_ask`.
pub fn next_unselected_ask(
    visible: &[String],
    asked: &HashSet<String>,
    lacks: impl Fn(&str) -> bool,
    last_ask: Option<f64>,
    now: f64,
) -> Option<String> {
    if last_ask.is_some_and(|t| now - t < ASK_GAP_SECS) {
        return None;
    }
    visible
        .iter()
        .map(String::as_str)
        .find(|k| is_listen_kind(k) && !asked.contains(*k) && lacks(k))
        .map(str::to_string)
}

/// Whether a reply body says its card's picture was written, so the card reloads:
/// a heard picture (`"rendered"`) or the stand-in drawn while none was heard (`"placeholder"`).
pub fn wrote_picture(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .is_ok_and(|v| matches!(v["status"].as_str(), Some("rendered" | "placeholder")))
}

/// A listened-picture ask the door has not answered yet.
struct Ask {
    kind: String,
    selected: bool,
    since: f64,
}

/// The gallery's asks of the listened route. Unselected: each visible kind is
/// asked once a session, at most one new ask per [`ASK_GAP_SECS`]. A picked
/// kind is asked selected at once (the server keeps its own cooldown).
///
/// Nothing blocks the frame: a deferred read answers `None` until its worker
/// has the reply (`reads::CachedDoor`), so each ask is called again on later
/// ticks until it lands.
#[derive(Default)]
pub struct Listens {
    asked: HashSet<String>,
    last_ask: Option<f64>,
    picked: Vec<String>,
    pending: Vec<Ask>,
}

impl Listens {
    /// A card was picked: its listened picture is asked for, selected, on the next tick.
    pub fn pick(&mut self, kind: &str) {
        if is_listen_kind(kind) && !self.picked.iter().any(|k| k == kind) {
            self.picked.push(kind.to_string());
        }
    }

    /// One frame's work. `visible` is the kinds on screen while the gallery is
    /// open (`None` when it is closed); `lacks` says whether a kind has no
    /// heard picture yet (a stand-in counts as lacking). A rendered or placeholder
    /// answer reloads that kind's card.
    pub fn tick(
        &mut self,
        door: &dyn Call,
        ctx: &Context,
        now: f64,
        visible: Option<&[String]>,
        lacks: impl Fn(&str) -> bool,
    ) {
        for kind in self.picked.drain(..) {
            self.pending.push(Ask { kind, selected: true, since: now });
        }
        if let Some(visible) = visible {
            if let Some(kind) = next_unselected_ask(visible, &self.asked, lacks, self.last_ask, now) {
                self.asked.insert(kind.clone());
                self.last_ask = Some(now);
                self.pending.push(Ask { kind, selected: false, since: now });
            }
        }
        let mut waiting = Vec::new();
        for ask in std::mem::take(&mut self.pending) {
            let mut params = Params::default();
            params.query.insert("kind".to_string(), ask.kind.clone());
            params.query.insert("selected".to_string(), if ask.selected { "1" } else { "0" }.to_string());
            match door.call("GET", LISTEN_PATH, params) {
                None if now - ask.since < ASK_TIMEOUT_SECS => waiting.push(ask),
                None => {}
                Some(reply) => {
                    if reply.status == 200 && wrote_picture(&reply.body) {
                        reload(ctx, &ask.kind);
                    }
                }
            }
        }
        self.pending = waiting;
        if !self.pending.is_empty() {
            super::frames::repaint_after(ctx, std::time::Duration::from_millis(250));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_advance_once_per_second_and_loop() {
        assert_eq!(frame_index(0.0, 4), 0);
        assert_eq!(frame_index(0.99, 4), 0);
        assert_eq!(frame_index(1.0, 4), 1);
        assert_eq!(frame_index(4.0, 4), 0, "loops");
        assert_eq!(frame_index(-3.0, 4), 0, "no negative clock");
        assert_eq!(frame_index(5.0, 0), 0, "no frames, no index");
    }

    #[test]
    fn the_next_frame_is_due_at_the_next_boundary() {
        assert!((secs_to_next_frame(0.25) - 0.75).abs() < 1e-9);
        assert!((secs_to_next_frame(2.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_kind_names_its_file_with_plain_characters_only() {
        assert_eq!(file_name("gen.bloom"), "gen.bloom.gif");
        assert_eq!(file_name("fx/odd name"), "fx_odd_name.gif");
    }

    /// A GIF written by [`encode`] decodes back to the same frames' size and
    /// count (the colours are quantised, so only the shape is compared).
    #[test]
    fn a_gif_round_trips_its_frame_count_and_size() {
        let size = [4, 2];
        let frames: Vec<Vec<u8>> = (0..3).map(|i| vec![(i * 80) as u8; 4 * 2 * 3]).collect();
        let bytes = encode(size, &frames).expect("encodes");
        let (back, decoded) = decode(&bytes).expect("decodes");
        assert_eq!(back, size);
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0].len(), 4 * 2 * 4, "RGBA out");
    }

    /// A scratch directory under the system temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir().join(format!("mvj-gallery-anim-{name}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A 4x2 GIF of `frames` solid frames.
    fn gif(frames: usize) -> Vec<u8> {
        encode([4, 2], &vec![vec![40u8; 4 * 2 * 3]; frames]).expect("encodes")
    }

    fn write_placeholder(dir: &Path, kind: &str, bytes: &[u8]) {
        std::fs::create_dir_all(dir).expect("placeholder dir");
        std::fs::write(dir.join(file_name(kind)), bytes).expect("placeholder written");
    }

    fn write_listened(root: &Path, kind: &str, bytes: &[u8]) {
        let meta = crate::shell::preview_cache::Meta { rendered_at: 0, input_hash: 1, frames: 3, size: [4, 2] };
        PreviewCache::new(root).write(Space::Gallery, kind, bytes, &meta).expect("listened written");
    }

    #[test]
    fn a_listened_picture_wins_over_the_placeholder() {
        let scratch = Scratch::new("wins");
        let (previews, placeholder) = (scratch.0.join("previews"), scratch.0.join("gallery"));
        write_placeholder(&placeholder, "gen.x", &gif(1));
        write_listened(&previews, "gen.x", &gif(3));
        let ctx = Context::default();
        let frames = frames_with_root(&ctx, "gen.x", Some(&previews), Some(&placeholder)).expect("frames");
        assert_eq!(frames.len(), 3, "the listened picture's frames");
        assert!(entry_with_roots(&ctx, "gen.x", Some(&previews), Some(&placeholder)).listened);
    }

    /// A stand-in (sidecar hash 0) shows its frames, but is not a heard picture,
    /// so its card still lacks one and keeps asking for it.
    #[test]
    fn a_stand_in_shows_but_is_not_a_heard_picture() {
        let scratch = Scratch::new("stand-in");
        let previews = scratch.0.join("previews");
        let meta = crate::shell::preview_cache::Meta { rendered_at: 0, input_hash: STAND_IN_HASH, frames: 3, size: [4, 2] };
        PreviewCache::new(&previews).write(Space::Gallery, "gen.x", &gif(3), &meta).expect("stand-in written");
        let entry = entry_with_roots(&Context::default(), "gen.x", Some(&previews), None);
        assert_eq!(entry.frames.map(|f| f.len()), Some(3), "the stand-in's frames show");
        assert!(!entry.listened);
    }

    #[test]
    fn without_a_listened_picture_the_placeholder_still_shows() {
        let scratch = Scratch::new("falls-back");
        let (previews, placeholder) = (scratch.0.join("previews"), scratch.0.join("gallery"));
        write_placeholder(&placeholder, "gen.x", &gif(1));
        let ctx = Context::default();
        let frames = frames_with_root(&ctx, "gen.x", Some(&previews), Some(&placeholder)).expect("frames");
        assert_eq!(frames.len(), 1);
        assert!(!entry_with_roots(&ctx, "gen.x", Some(&previews), Some(&placeholder)).listened);

        // A listened file that does not decode also falls back to the placeholder.
        write_listened(&previews, "gen.x", b"not a gif");
        let ctx = Context::default();
        let frames = frames_with_root(&ctx, "gen.x", Some(&previews), Some(&placeholder)).expect("frames");
        assert_eq!(frames.len(), 1, "a corrupt listened file falls back");
        assert!(!entry_with_roots(&ctx, "gen.x", Some(&previews), Some(&placeholder)).listened);

        assert!(frames_with_root(&Context::default(), "gen.none", None, None).is_none(), "no file, no frames");
    }

    #[test]
    fn reload_picks_up_a_newly_written_listened_picture() {
        let scratch = Scratch::new("reload");
        let (previews, placeholder) = (scratch.0.join("previews"), scratch.0.join("gallery"));
        write_placeholder(&placeholder, "gen.x", &gif(1));
        let ctx = Context::default();
        let before = frames_with_root(&ctx, "gen.x", Some(&previews), Some(&placeholder)).expect("frames");
        assert_eq!(before.len(), 1);

        write_listened(&previews, "gen.x", &gif(3));
        let cached = frames_with_root(&ctx, "gen.x", Some(&previews), Some(&placeholder)).expect("frames");
        assert_eq!(cached.len(), 1, "the entry is kept until reload");

        reload(&ctx, "gen.x");
        let fresh = frames_with_root(&ctx, "gen.x", Some(&previews), Some(&placeholder)).expect("frames");
        assert_eq!(fresh.len(), 3, "reload reads the new listened file");
        assert!(entry_with_roots(&ctx, "gen.x", Some(&previews), Some(&placeholder)).listened);
    }
}

#[cfg(test)]
mod listen_tests {
    use super::*;
    use crate::shell::FakeDoor;
    use serde_json::json;

    fn kinds(list: &[&str]) -> Vec<String> {
        list.iter().map(|k| k.to_string()).collect()
    }

    fn listen_calls(door: &FakeDoor) -> Vec<(String, String)> {
        door.calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "GET" && p == LISTEN_PATH)
            .map(|(_, _, params)| (params.query["kind"].clone(), params.query["selected"].clone()))
            .collect()
    }

    #[test]
    fn the_first_visible_listen_kind_without_a_picture_is_asked_next() {
        let visible = kinds(&["gen.x", "fx.a", "mat.b", "fx.c"]);
        let none_asked = HashSet::new();
        assert_eq!(next_unselected_ask(&visible, &none_asked, |k| k != "fx.a", None, 0.0).as_deref(), Some("mat.b"));
        assert_eq!(next_unselected_ask(&visible, &none_asked, |_| false, None, 0.0), None, "every card has its picture");

        let asked: HashSet<String> = ["mat.b".to_string()].into_iter().collect();
        assert_eq!(next_unselected_ask(&visible, &asked, |k| k != "fx.a", None, 0.0).as_deref(), Some("fx.c"), "asked once");
    }

    #[test]
    fn unselected_asks_are_half_a_second_apart() {
        let visible = kinds(&["fx.a", "fx.b"]);
        let asked = HashSet::new();
        assert_eq!(next_unselected_ask(&visible, &asked, |_| true, Some(10.0), 10.3), None);
        assert_eq!(next_unselected_ask(&visible, &asked, |_| true, Some(10.0), 10.5).as_deref(), Some("fx.a"));
    }

    #[test]
    fn only_a_written_picture_reloads_the_card() {
        assert!(wrote_picture(br#"{"status":"rendered"}"#));
        assert!(wrote_picture(br#"{"status":"placeholder"}"#));
        for status in ["fresh", "none", "unsupported", "no_cache"] {
            assert!(!wrote_picture(format!(r#"{{"status":"{status}"}}"#).as_bytes()), "{status}");
        }
        assert!(!wrote_picture(b"not json"));
    }

    #[test]
    fn a_pick_queues_one_selected_ask_for_a_listen_kind_only() {
        let door = FakeDoor::new();
        door.respond("GET", LISTEN_PATH, json!({"status": "fresh"}));
        let ctx = Context::default();
        let mut listens = Listens::default();
        listens.pick("gen.b");
        listens.pick("fx.a");
        listens.pick("fx.a");
        listens.tick(&door, &ctx, 0.0, None, |_| true);
        assert_eq!(listen_calls(&door), vec![("fx.a".to_string(), "1".to_string())]);
    }

    #[test]
    fn visible_kinds_are_asked_one_at_a_time_and_each_once() {
        let door = FakeDoor::new();
        door.respond("GET", LISTEN_PATH, json!({"status": "fresh"}));
        let ctx = Context::default();
        let mut listens = Listens::default();
        let visible = kinds(&["fx.a", "fx.b"]);

        listens.tick(&door, &ctx, 0.0, Some(&visible), |_| true);
        listens.tick(&door, &ctx, 0.1, Some(&visible), |_| true);
        assert_eq!(listen_calls(&door), vec![("fx.a".to_string(), "0".to_string())], "throttled");

        listens.tick(&door, &ctx, 0.6, Some(&visible), |_| true);
        listens.tick(&door, &ctx, 9.0, Some(&visible), |_| true);
        assert_eq!(
            listen_calls(&door),
            vec![("fx.a".to_string(), "0".to_string()), ("fx.b".to_string(), "0".to_string())],
            "each kind asked once a session"
        );
    }

    #[test]
    fn a_closed_gallery_asks_nothing_unselected() {
        let door = FakeDoor::new();
        door.respond("GET", LISTEN_PATH, json!({"status": "fresh"}));
        let mut listens = Listens::default();
        listens.tick(&door, &Context::default(), 0.0, None, |_| true);
        assert!(listen_calls(&door).is_empty());
    }
}
