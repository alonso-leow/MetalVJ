//! The media board: what the operator picked to *see* and to *play*.
//!
//! 2026-10-15. One small shared surface joins the four places that used to
//! know nothing of each other:
//!
//! - the **biblioteca** publishes the image or video a card selected;
//! - the video **propiedades** gallery pushes the scenes it marked;
//! - the simplified **sequence** reads the board's clip list and its cursor
//!   and reproduces it (a timeline of videos and scene clips);
//! - the **overlay** page turns the same picks into a trigger.
//!
//! It is deliberately *not* a field on [`crate::shell::Shell`]: the board is
//! cross-panel, outlives the frame that wrote it (a drag, a click, a preview
//! pump), and a panel that did not paint must still read what another wrote.
//! That is exactly the trade `preview::publish_sequence` and `scenes::route`
//! already make — egui memory, a plain value, no panel reaching into another
//! panel's private state.
//!
//! **Regla 5/6.** This module re-derives nothing: it carries values a route
//! already resolved and every call it makes goes through [`Call`] like a
//! panel's own.

use egui::{Context, Id};
use serde_json::{json, Value};

use crate::call::Call;

/// A scene of a video file: an id for the book, and a half-open `[start, end)`
/// span in seconds. The same `(id, start, end)` a [`crate::shell::panels::FileDrag`]
/// carries, so a board clip and a dragged scene name the same thing.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub id: String,
    pub start: f64,
    pub end: f64,
}

/// One picked media item: a whole image or video file, or one scene of a
/// video. `path` is the absolute file path the library's `/fs` resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub path: String,
    /// The bare file name, for a label.
    pub name: String,
    /// `"video"` or `"image"` — the library's own words.
    pub kind: String,
    /// When it is one scene of the video rather than all of it.
    pub scene: Option<Scene>,
    /// A video's whole length when the library knew it (seconds), so a
    /// reproduction does not have to guess. Zero when unknown.
    pub seconds: f64,
    /// A loop taken off the media: the simplified sequence wraps this clip's
    /// `[start, end)` span instead of playing it once ("a way to obtain loops
    /// from media"). A whole file loops too.
    pub looping: bool,
}

impl Clip {
    /// The span this clip plays, in seconds: the scene's `[start, end)` when
    /// it is a scene, else `[0, seconds)` (a still is one frame).
    pub fn span(&self) -> (f64, f64) {
        match &self.scene {
            Some(s) => (s.start, s.end),
            None => (0.0, self.seconds),
        }
    }

    /// The length in seconds (`end - start`), never negative.
    pub fn length(&self) -> f64 {
        let (a, b) = self.span();
        (b - a).max(0.0)
    }

    /// Whether two picks name the same thing — used to dedupe a push.
    fn same_as(&self, other: &Clip) -> bool {
        self.path == other.path
            && self.kind == other.kind
            && self.scene.as_ref().map(|s| &s.id) == other.scene.as_ref().map(|s| &s.id)
    }

    /// The `FileDrag` a node canvas drop wants from this clip (kind `video`
    /// for a scene or a whole video, `image` for a still).
    pub fn file_drag(&self) -> crate::shell::panels::FileDrag {
        crate::shell::panels::FileDrag {
            name: self.name.clone(),
            path: self.path.clone(),
            kind: self.kind.clone(),
            seconds: self.scene.as_ref().map(Scene::length).or(Some(self.seconds)),
            scene: self.scene.as_ref().map(|s| (s.id.clone(), s.start, s.end)),
        }
    }
}

impl Scene {
    fn length(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }
}

/// The board: the reproduction queue (`clips`), which one is showing
/// (`cursor`), and whether the full float authoring is on (`effects`, the
/// "Enable effects" button).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    pub clips: Vec<Clip>,
    pub cursor: usize,
    pub effects: bool,
}

impl Board {
    /// The clip the cursor is on, clamped into range.
    pub fn current(&self) -> Option<&Clip> {
        self.clips.get(self.cursor.min(self.clips.len().saturating_sub(1)))
    }
}

fn board_id() -> Id {
    Id::new("shell/media/board")
}

/// The board as it stands (a clone; the stored value is never handed out).
pub fn board(ctx: &Context) -> Board {
    ctx.data(|d| d.get_temp::<Board>(board_id())).unwrap_or_default()
}

pub fn set_board(ctx: &Context, b: Board) {
    ctx.data_mut(|d| d.insert_temp(board_id(), b));
}

/// The clip the cursor is on, if any.
pub fn current(ctx: &Context) -> Option<Clip> {
    board(ctx).current().cloned()
}

/// Append a clip, skipping one already on the board (a repeated pick should
/// not enqueue the same scene twice). Returns whether it was added.
pub fn push(ctx: &Context, clip: Clip) -> bool {
    let mut b = board(ctx);
    if b.clips.iter().any(|c| c.same_as(&clip)) {
        return false;
    }
    b.clips.push(clip);
    set_board(ctx, b);
    true
}

/// Replace the whole queue (the gallery's "send selected", the library's
/// "play all"). Clamps the cursor into the new range.
pub fn set_clips(ctx: &Context, clips: Vec<Clip>) {
    let mut b = board(ctx);
    b.clips = clips;
    b.cursor = b.cursor.min(b.clips.len().saturating_sub(1));
    set_board(ctx, b);
}

/// Empty the queue and reset the cursor (never the effects switch).
pub fn clear(ctx: &Context) {
    let mut b = board(ctx);
    b.clips.clear();
    b.cursor = 0;
    set_board(ctx, b);
}

/// Move the cursor (a click on a row).
pub fn set_cursor(ctx: &Context, cursor: usize) {
    let mut b = board(ctx);
    b.cursor = cursor;
    set_board(ctx, b);
}

/// Whether the clip at `index` loops, and set it — a loop obtained from the
/// media (the gallery's loop button, the sequence's own toggle).
pub fn loop_of(ctx: &Context, index: usize) -> bool {
    board(ctx).clips.get(index).is_some_and(|c| c.looping)
}

pub fn set_loop(ctx: &Context, index: usize, on: bool) {
    let mut b = board(ctx);
    if let Some(c) = b.clips.get_mut(index) {
        c.looping = on;
        set_board(ctx, b);
    }
}

/// The "Enable effects" switch.
pub fn set_effects(ctx: &Context, on: bool) {
    let mut b = board(ctx);
    b.effects = on;
    set_board(ctx, b);
}

// ---------------------------------------------------------------------------
// previewing a clip (the picture, shared by biblioteca and the sequence)
// ---------------------------------------------------------------------------

/// The route and query that show a clip's picture, JPEG, through
/// [`crate::shell::preview::FloatingPreview`]: a still file through the image
/// route, a video (or a scene of one) through the video-frame route. `seek`
/// must have been posted for a video first ([`seek`]).
pub fn preview_route(clip: &Clip) -> (String, Vec<(String, String)>) {
    match clip.kind.as_str() {
        "image" => ("/author/image.jpg".to_string(), vec![("path".to_string(), clip.path.clone())]),
        _ => ("/author/video/frame.jpg".to_string(), vec![("path".to_string(), clip.path.clone())]),
    }
}

/// Point a video's decoder at `t` seconds. A still needs no seek. The routes
/// are the editor's own (`POST /author/video/seek`), never a handler body.
pub fn seek(door: &dyn Call, clip: &Clip, t: f64) {
    if clip.kind == "image" {
        return;
    }
    let _ = door.json(
        "POST",
        "/author/video/seek",
        Some(json!({ "path": clip.path, "t": t.max(0.0) })),
    );
}

/// The clip a [`crate::shell::panels::FileRef`] names — a whole file, no
/// scene. `seconds` is filled when the caller knows it.
pub fn clip_from_file(file: &crate::shell::panels::FileRef, seconds: f64) -> Clip {
    Clip {
        path: file.path.clone(),
        name: file.name.clone(),
        kind: file.kind.clone(),
        scene: None,
        seconds,
        looping: false,
    }
}

/// The export body `POST /author/scenes/export` wants for a scene clip, or
/// `None` for a whole file (the frame export is not a clip export).
pub fn scene_export_body(clip: &Clip, name: Option<&str>) -> Option<Value> {
    let scene = clip.scene.as_ref()?;
    Some(json!({
        "clip": clip.path,
        "start": scene.start,
        "end": scene.end,
        "name": name.unwrap_or(&clip.name),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene_clip(start: f64, end: f64) -> Clip {
        Clip {
            path: "/v/a.mp4".into(),
            name: "a.mp4".into(),
            kind: "video".into(),
            scene: Some(Scene { id: format!("x#{start}-{end}"), start, end }),
            seconds: 10.0,
            looping: false,
        }
    }

    #[test]
    fn span_prefers_the_scene_then_the_whole_length() {
        assert_eq!(scene_clip(1.0, 4.0).span(), (1.0, 4.0));
        let whole = Clip { scene: None, seconds: 10.0, ..scene_clip(0.0, 0.0) };
        assert_eq!(whole.span(), (0.0, 10.0));
        assert_eq!(scene_clip(1.0, 4.0).length(), 3.0);
    }

    #[test]
    fn a_push_dedupes_the_same_pick_and_a_different_scene_is_distinct() {
        let ctx = Context::default();
        assert!(push(&ctx, scene_clip(1.0, 4.0)));
        assert!(!push(&ctx, scene_clip(1.0, 4.0)));
        assert!(push(&ctx, scene_clip(5.0, 8.0)));
        assert_eq!(board(&ctx).clips.len(), 2);
    }

    #[test]
    fn set_clips_clamps_the_cursor_and_clear_keeps_the_effects_switch() {
        let ctx = Context::default();
        set_effects(&ctx, true);
        set_clips(&ctx, vec![scene_clip(0.0, 1.0), scene_clip(2.0, 3.0)]);
        set_cursor(&ctx, 5);
        assert_eq!(board(&ctx).cursor, 5);
        set_clips(&ctx, vec![scene_clip(0.0, 1.0)]);
        assert_eq!(board(&ctx).cursor, 0);
        clear(&ctx);
        assert!(board(&ctx).clips.is_empty());
        assert!(board(&ctx).effects, "clear never touches the effects switch");
    }

    #[test]
    fn a_loop_can_be_taken_off_a_clip() {
        let ctx = Context::default();
        set_clips(&ctx, vec![scene_clip(1.0, 4.0)]);
        assert!(!loop_of(&ctx, 0));
        set_loop(&ctx, 0, true);
        assert!(loop_of(&ctx, 0));
        set_loop(&ctx, 9, true);
        assert!(board(&ctx).clips[0].looping, "an out-of-range index is a no-op");
    }

    #[test]
    fn preview_route_splits_images_from_video() {
        let img = Clip { kind: "image".into(), ..scene_clip(0.0, 0.0) };
        assert_eq!(preview_route(&img).0, "/author/image.jpg");
        assert_eq!(preview_route(&scene_clip(0.0, 1.0)).0, "/author/video/frame.jpg");
    }

    #[test]
    fn scene_export_body_only_names_scenes() {
        assert!(scene_export_body(&scene_clip(1.0, 2.0), None).is_some());
        let whole = Clip { scene: None, ..scene_clip(0.0, 0.0) };
        assert!(scene_export_body(&whole, None).is_none());
    }
}
