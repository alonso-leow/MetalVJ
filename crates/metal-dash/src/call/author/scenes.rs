//! 2026-10-01: a video's scenes and the book of kept ones (`metal_reel::scenes`).
//!
//! - `GET /author/scenes?clip=<path>` — the clip's scenes as its ingest cut
//!   them, each with the book's flags, name and groups folded in. A clip with
//!   no cut list yet answers `pending` and starts its proxy (the same
//!   background pass a seek starts), so the properties gallery fills itself.
//! - `GET /author/scenes` — every saved or favourite scene across all
//!   videos, and the groups: the library's Scenes route.
//! - `POST /author/scenes {op, …}` — `save` / `favourite` (`id`, the scene's
//!   `clip`/`start`/`end`/`poster`, `on`), `rename` (`id`, `name`), `file`
//!   (`id`, `group`, `on`; the scene's fields too when it is not kept yet),
//!   `group.create` / `group.delete` (`group`), `group.rename` (`group`,
//!   `to`). Deleting a group never unfavourites.
//! - `GET /author/scenes/poster.jpg?path=` — one poster, only from under the
//!   proxies' folder.
//! - `POST /author/scenes/export {clip, start, end, name?}` — trims
//!   `[start, end)` out of a video, by stream copy, into the export folder.

use std::path::Path;

use serde_json::{json, Value};

use metal_reel::scenes::{self, Scene, SceneBook};

use crate::call::{Bytes, CallError, Params};

use super::playout::HAP;
use super::Ctx;

/// The book, under the data root (`metal_reel::scenes::BOOK`).
pub const BOOK: &str = "data/scenes/favorites.json";

pub fn get(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    match params.param("clip").filter(|c| !c.is_empty()) {
        Some(clip) => {
            let listed = clip_scenes(Path::new(HAP), Path::new(BOOK), clip);
            if listed["pending"] == true {
                super::playout::ingest(clip);
            }
            Ok(listed)
        }
        None => Ok(book_scenes(&SceneBook::try_load(Path::new(BOOK)).map_err(CallError::Bad)?)),
    }
}

pub fn post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("expected json".into()))?;
    apply_at(Path::new(BOOK), &body).map_err(CallError::Bad)
}

pub fn poster(_ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let path = params.param("path").unwrap_or_default();
    let p = resolve_poster(path);
    let inside = (path.starts_with("projects/hap/") || p.starts_with(HAP)) && p.extension().is_some_and(|x| x == "jpg") && !path.contains("..");
    if !inside {
        return Err(CallError::Bad("not a poster".into()));
    }
    let body = metal_vfs::read(&p).map_err(|_| CallError::NotFound)?;
    Ok(Bytes { kind: "image/jpeg", body })
}

/// A poster named as the cut list stores it (`projects/hap/…`, relative to
/// the data root) resolves under this install's data root; an absolute path
/// written before that rule is kept as it is.
pub(crate) fn resolve_poster(path: &str) -> std::path::PathBuf {
    if path.starts_with("projects/hap/") {
        return crate::output::data_root().join(path);
    }
    Path::new(path).to_path_buf()
}

/// `POST /author/scenes/export {clip, start, end, name?}`: trims `[start, end)`
/// out of the video at `clip` and writes it into the export folder
/// (`settings::export_dir`, `~/Pictures/MetalVJ` when empty) as
/// `<stem-of-name-or-clip>-<start_ms>-<end_ms>.mp4`, so repeats never collide.
pub fn export(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("expected json".into()))?;
    let clip = body["clip"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| CallError::Bad("clip?".into()))?;
    let start = body["start"].as_f64().ok_or_else(|| CallError::Bad("start?".into()))?;
    let end = body["end"].as_f64().ok_or_else(|| CallError::Bad("end?".into()))?;
    if !(start >= 0.0 && end > start) {
        return Err(CallError::Bad("need 0 <= start < end".into()));
    }
    if !metal_vfs::is_file(Path::new(clip)) {
        return Err(CallError::Bad(format!("no such clip: {clip}")));
    }
    let stem = body["name"].as_str().filter(|s| !s.is_empty()).unwrap_or(clip);
    let stem = Path::new(stem).file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
    let stem: String = stem
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect();
    let (a, b) = ((start * 1000.0).round() as i64, (end * 1000.0).round() as i64);
    let path = crate::call::settings::export_dir().join(format!("{stem}-{a}-{b}.mp4"));
    trim(clip, start, end, &path).map_err(CallError::Bad)?;
    Ok(json!({
        "path": path.to_string_lossy(),
        "start": start,
        "end": end,
        "seconds": end - start,
    }))
}

/// Trims `[start, end)` out of `clip` into `dest` by stream copy (fast, no
/// re-encode). `-ss`/`-to` sit before `-i` so both read on the source's own
/// clock, the cut ffmpeg's copy mode can make without touching a frame.
///
/// Under test nothing runs: the file is not what the route's shape is checked
/// against, and the test host need not have ffmpeg.
fn trim(clip: &str, start: f64, end: f64, dest: &Path) -> Result<(), String> {
    if cfg!(test) {
        let _ = (clip, start, end, dest);
        return Ok(());
    }
    #[cfg(not(test))]
    {
        if let Some(dir) = dest.parent() {
            metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        use metal_schema::platform::Command;
        let out = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-ss", &format!("{start:.3}"), "-to", &format!("{end:.3}"), "-i", clip, "-c", "copy"])
            .arg(dest)
            .output()
            .map_err(|e| format!("ffmpeg: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
    }
    Ok(())
}

/// One scene as a row: its own fields, the book's flags and its groups.
fn row(book: &SceneBook, id: &str, clip: &str, start: f64, end: f64, poster: Option<&str>) -> Value {
    let kept = book.scenes.get(id);
    json!({
        "id": id,
        "clip": clip,
        "start": start,
        "end": end,
        "seconds": end - start,
        "poster": kept.and_then(|s| s.poster.as_deref()).or(poster),
        "name": kept.map_or("", |s| s.name.as_str()),
        "saved": kept.is_some_and(|s| s.saved),
        "favourite": kept.is_some_and(|s| s.favourite),
        "groups": book.groups_of(id),
    })
}

/// `clip`'s scenes off its cut list under `hap_root`, with `book`'s state.
pub(crate) fn clip_scenes(hap_root: &Path, book: &Path, clip: &str) -> Value {
    let book = SceneBook::load(book);
    let Some(cuts) = scenes::load_cuts(hap_root, &scenes::slug(clip)) else {
        return json!({ "clip": clip, "scenes": [], "pending": true, "groups": book.groups.keys().collect::<Vec<_>>() });
    };
    let rows: Vec<Value> = cuts
        .iter()
        .map(|c| {
            // The ingest's own poster wins here: the book's copy may name a
            // file an earlier ingest wrote and this one replaced.
            let mut r = row(&book, &c.id, &c.clip, c.start, c.end, None);
            r["poster"] = json!(c.poster);
            r
        })
        .collect();
    json!({ "clip": clip, "scenes": rows, "pending": false, "groups": book.groups.keys().collect::<Vec<_>>() })
}

/// Every kept scene, by clip then start, and the groups.
pub(crate) fn book_scenes(book: &SceneBook) -> Value {
    let mut kept: Vec<(&String, &Scene)> = book.scenes.iter().collect();
    kept.sort_by(|a, b| a.1.clip.cmp(&b.1.clip).then(a.1.start.total_cmp(&b.1.start)));
    let rows: Vec<Value> = kept
        .into_iter()
        .map(|(id, s)| row(book, id, &s.clip, s.start, s.end, s.poster.as_deref()))
        .collect();
    json!({ "scenes": rows, "groups": book.groups })
}

/// The scene a body names, from its own fields.
fn scene_of(body: &Value) -> Result<(String, Scene), String> {
    let id = body["id"].as_str().filter(|s| !s.is_empty()).ok_or("id?")?.to_string();
    let clip = body["clip"].as_str().filter(|s| !s.is_empty()).ok_or("clip?")?.to_string();
    let start = body["start"].as_f64().ok_or("start?")?;
    let end = body["end"].as_f64().ok_or("end?")?;
    let poster = body["poster"].as_str().map(str::to_string);
    Ok((id, Scene { clip, start, end, poster, ..Scene::default() }))
}

/// Applies one `POST /author/scenes` body to the book at `path`.
pub(crate) fn apply_at(path: &Path, body: &Value) -> Result<Value, String> {
    let mut book = SceneBook::try_load(path)?;
    let on = body["on"].as_bool().unwrap_or(true);
    let group = body["group"].as_str().unwrap_or_default();
    match body["op"].as_str().unwrap_or_default() {
        op @ ("save" | "favourite") => {
            let (id, scene) = scene_of(body)?;
            let kept = book.upsert(&id, scene);
            if op == "save" {
                kept.saved = on;
            } else {
                kept.favourite = on;
            }
            if let Some(name) = body["name"].as_str() {
                kept.name = name.to_string();
            }
            book.prune(&id);
        }
        "rename" => {
            let id = body["id"].as_str().unwrap_or_default();
            let scene = book.scenes.get_mut(id).ok_or_else(|| format!("no scene {id:?}"))?;
            scene.name = body["name"].as_str().unwrap_or_default().trim().to_string();
        }
        "file" => {
            let id = body["id"].as_str().unwrap_or_default().to_string();
            if on && !book.scenes.contains_key(&id) {
                // Filing a scene that is not kept yet keeps it: a group is
                // a list of scenes the operator chose.
                let (id, scene) = scene_of(body)?;
                book.upsert(&id, scene).saved = true;
            }
            book.file(&id, group, on)?;
        }
        "group.create" => book.create_group(group)?,
        "group.rename" => book.rename_group(group, body["to"].as_str().unwrap_or_default())?,
        "group.delete" => book.delete_group(group)?,
        other => return Err(format!("unknown op {other:?}")),
    }
    book.save(path)?;
    Ok(json!({ "ok": true, "scenes": book.scenes.len(), "groups": book.groups }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A poster the cut list names data-root relative resolves under this
    /// install's data root; an absolute one from before that rule stays as is.
    #[test]
    fn a_relative_poster_resolves_under_the_data_root() {
        let rel = super::super::playout::poster_rel("clip-ab12", 3);
        assert_eq!(rel, "projects/hap/clip-ab12.scenes/3.jpg");
        assert_eq!(resolve_poster(&rel), crate::output::data_root().join(&rel));
        assert_eq!(resolve_poster("/old/absolute/0.jpg"), std::path::PathBuf::from("/old/absolute/0.jpg"));
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fw-scenes-{name}-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        dir
    }

    /// `author::State::new` reads `data/schema.json` off a bare relative path,
    /// so the cwd is the repo root for its construction, as in `mod`'s tests;
    /// the export handler ignores the state.
    fn ctx_state() -> crate::author::State {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir to repo root");
        let state = crate::author::State::new(None, std::path::PathBuf::from("data/tracks/untitled.track.json"));
        std::env::set_current_dir(&prior).expect("restore cwd");
        state
    }

    /// A valid body trims into the export folder and answers the window back;
    /// a body with `end <= start` (and a missing clip) is refused. Under test
    /// nothing is written, so this checks the route's shape, not ffmpeg.
    #[test]
    fn export_trims_a_clip_and_names_the_file_by_its_window() {
        let dir = temp("export");
        metal_vfs::create_dir_all(&dir).unwrap();
        let clip = dir.join("take one.mp4");
        metal_vfs::write(&clip, b"not really a video, just needs to exist").unwrap();

        let mut state = ctx_state();
        let mut ctx = Ctx { state: &mut state };
        let body = |v: Value| Params { query: Default::default(), body: v.to_string().into_bytes(), tail: None };

        let clip_s = clip.to_string_lossy().into_owned();
        let reply = export(
            &mut ctx,
            &body(json!({ "clip": clip_s, "start": 1.0, "end": 2.5, "name": "intro take" })),
        )
        .expect("exports");
        assert_eq!(reply["start"], json!(1.0));
        assert_eq!(reply["end"], json!(2.5));
        assert_eq!(reply["seconds"], json!(1.5));
        let path = reply["path"].as_str().unwrap();
        assert!(path.ends_with("intro_take-1000-2500.mp4"), "{path}");

        assert!(
            export(&mut ctx, &body(json!({ "clip": clip_s, "start": 2.0, "end": 1.0 }))).is_err(),
            "end <= start is refused"
        );
        assert!(
            export(&mut ctx, &body(json!({ "clip": clip_s, "start": -1.0, "end": 1.0 }))).is_err(),
            "a negative start is refused"
        );
        assert!(
            export(&mut ctx, &body(json!({ "start": 0.0, "end": 1.0 }))).is_err(),
            "no clip is refused"
        );
        assert!(
            export(&mut ctx, &body(json!({ "clip": "/v/nope.mp4", "start": 0.0, "end": 1.0 }))).is_err(),
            "a non-file clip is refused"
        );
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// The Scenes route lists kept scenes from two videos, and a group's
    /// delete leaves their favourites standing.
    #[test]
    fn the_route_lists_scenes_from_two_videos_and_a_group_delete_keeps_favourites() {
        let dir = temp("route");
        let book = dir.join("favorites.json");
        for (clip, id) in [("/v/a.mp4", "a-1#0-2000"), ("/v/b.mp4", "b-2#2000-4000")] {
            let body = json!({ "op": "favourite", "id": id, "clip": clip, "start": 0.0, "end": 2.0, "on": true });
            apply_at(&book, &body).unwrap();
            apply_at(&book, &json!({ "op": "file", "id": id, "group": "hall", "on": true })).unwrap();
        }
        apply_at(&book, &json!({ "op": "save", "id": "b-2#0-1000", "clip": "/v/b.mp4", "start": 0.0, "end": 1.0, "name": "intro" }))
            .unwrap();
        apply_at(&book, &json!({ "op": "group.rename", "group": "hall", "to": "foyer" })).unwrap();
        let listed = book_scenes(&SceneBook::load(&book));
        let rows = listed["scenes"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["clip"], "/v/a.mp4");
        assert_eq!(rows[1]["name"], "intro");
        assert_eq!(rows[2]["groups"], json!(["foyer"]));
        assert_eq!(listed["groups"]["foyer"].as_array().unwrap().len(), 2);

        apply_at(&book, &json!({ "op": "group.delete", "group": "foyer" })).unwrap();
        let after = SceneBook::load(&book);
        assert!(after.groups.is_empty());
        assert_eq!(after.favourites().count(), 2);
        assert!(apply_at(&book, &json!({ "op": "nope" })).is_err());
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clip_without_a_cut_list_is_pending() {
        let dir = temp("pending");
        let v = clip_scenes(&dir, &dir.join("favorites.json"), "/v/none.mp4");
        assert_eq!(v["pending"], true);
        assert_eq!(v["scenes"], json!([]));
    }
}
