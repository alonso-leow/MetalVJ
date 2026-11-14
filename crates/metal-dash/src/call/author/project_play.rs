//! `/project/play`: a song's project played on the air as one run.
//!
//! - `GET /project/play` lists the saved projects (`data/projects/*.json`)
//!   and the runs in the air.
//! - `POST /project/play {song, op:"play"|"stop", loop?}` arms or releases
//!   one song. `play` reads the project file, builds one overlay play per
//!   model clip (`metal_perform::project_runs::plays_from_project`), and
//!   arms them through the run state in `State::project_runs`.
//!
//! The trigger target `project-<song>` runs the same [`play_song`] body, the
//! way `sequence-<id>` runs `sequences::run`.

use serde_json::{json, Value};

use metal_perform::project_runs::plays_from_project;

use crate::call::{CallError, Ctx, Params};

use super::playout::project_path;

/// `GET /project/play`.
pub fn get(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let time = ctx.state.time();
    let projects: Vec<Value> = metal_vfs::read_dir(super::playout::PROJECTS)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let song = name.strip_suffix(".json")?.to_string();
            let clips = metal_vfs::read_to_string(e.path())
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v["clips"].as_array().map(Vec::len))
                .unwrap_or(0);
            Some(json!({
                "song": song,
                "clips": clips,
                "running": ctx.state.project_runs.running(&song, time),
            }))
        })
        .collect();
    Ok(json!({
        "projects": projects,
        "running": ctx.state.project_runs.report(time),
    }))
}

/// `POST /project/play`.
pub fn post(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("expected json".into()))?;
    let song = body["song"].as_str().unwrap_or_default().to_string();
    if song.is_empty() {
        return Err(CallError::Bad("which song".into()));
    }
    match body["op"].as_str().unwrap_or("play") {
        "play" => {
            let looping = body["loop"].as_bool().unwrap_or(false);
            let armed = play_song(ctx, &song, looping)?;
            Ok(json!({ "ok": true, "song": song, "plays": armed }))
        }
        "stop" => {
            let stopped = ctx.state.project_runs.stop(&mut ctx.state.overlays, &song);
            Ok(json!({ "ok": true, "stopped": stopped }))
        }
        other => Err(CallError::Bad(format!("no such op: {other}"))),
    }
}

/// The play half, shared by `POST /project/play {op:"play"}` and a
/// `project-<song>` trigger. Returns how many layers were armed.
pub(crate) fn play_song(ctx: &mut Ctx, song: &str, looping: bool) -> Result<usize, CallError> {
    let path = project_path(song).ok_or_else(|| CallError::Bad("song?".into()))?;
    let text = metal_vfs::read_to_string(&path).map_err(|_| CallError::NotFound)?;
    let project: Value =
        serde_json::from_str(&text).map_err(|e| CallError::Bad(format!("{}: {e}", path.display())))?;

    let ids = ctx.paint.ids();
    let schemas = ctx.paint.schemas();
    let time = ctx.state.time();
    let plays = plays_from_project(&project, time, looping, |model_id| {
        let at = ids.iter().position(|id| *id == model_id)?;
        Some((at, schemas[at].width()))
    })
    .map_err(CallError::Bad)?;
    if plays.is_empty() {
        return Err(CallError::Bad(format!("{song:?} has no model clips to play")));
    }
    Ok(ctx.state.project_runs.start(&mut ctx.state.overlays, song, plays))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// The fixture `overlay.rs`'s tests use: `Author::new()` reads the repo
    /// root, so it runs from there, and nothing here writes `data/`.
    fn fixture() -> (Box<dyn crate::paint::Paint>, crate::State, crate::listeners::Listeners, crate::Author) {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = crate::Author::new();
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");
        let paint = crate::paint::painter();
        let state = crate::State::new(paint.as_ref());
        let sim = crate::listeners::Listeners::stand_in();
        (paint, state, sim, author)
    }

    fn body(v: Value) -> Params {
        Params {
            query: HashMap::new(),
            body: v.to_string().into_bytes(),
            tail: None,
        }
    }

    #[test]
    fn stopping_a_song_that_is_not_running_says_so() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        let reply = post(&mut ctx, &body(json!({ "song": "nothing-here", "op": "stop" }))).expect("stop");
        assert_eq!(reply["stopped"], json!(false));
    }

    #[test]
    fn a_missing_project_file_and_a_bad_op_are_refused() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        assert!(matches!(
            post(&mut ctx, &body(json!({ "song": "no-such-song-xyz", "op": "play" }))),
            Err(CallError::NotFound)
        ));
        assert!(matches!(
            post(&mut ctx, &body(json!({ "song": "x", "op": "explode" }))),
            Err(CallError::Bad(_))
        ));
        assert!(matches!(post(&mut ctx, &body(json!({ "op": "play" }))), Err(CallError::Bad(_))));
        assert!(ctx.state.project_runs.is_empty());
    }
}
