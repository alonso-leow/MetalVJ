//! The skeleton mode — `author::rig`'s ten routes, moved.

use serde_json::json;

use crate::author::{agent, library, rig, LIBRARY};
use crate::call::{CallError, Params};

use super::Ctx;

pub fn track(ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let track = ctx.actx().open.lock().expect("open lock").track.clone();
    match rig::save(&params.body, &track) {
        Ok(ticks) => {
            println!("  saved {ticks} ticks → {}", track.display());
            Ok(json!({ "ok": true }))
        }
        Err(e) => {
            eprintln!("  refused: {e}");
            Err(CallError::Bad(e))
        }
    }
}

/// `GET /rig/track`: the whole open track, frames included — the shell's rig
/// lane (`S.U9c`'s authoring half) reads it once per cadence to draw which
/// ticks carry a statement and to copy a range. The record's own shape,
/// serialized: `Track` is already serde, so this invents nothing.
pub fn track_get(ctx: &mut Ctx, _params: &Params) -> Result<serde_json::Value, CallError> {
    let actx = ctx.actx();
    let path = actx.open.lock().expect("open lock").track.clone();
    let track = agent::read(&path, actx.skeleton);
    Ok(serde_json::to_value(&track).unwrap_or(serde_json::Value::Null))
}

pub fn examples(ctx: &mut Ctx, _params: &Params) -> Result<serde_json::Value, CallError> {
    let actx = ctx.actx();
    let (audio, track) = {
        let open = actx.open.lock().expect("open lock");
        (open.audio.clone(), open.track.clone())
    };
    Ok(rig::examples(audio.as_deref(), &track, actx.ingest))
}

pub fn library_get(_params: &Params) -> Result<serde_json::Value, CallError> {
    Ok(library::listing(LIBRARY))
}

pub fn library_post(_ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let Some(posted) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    match library::save(LIBRARY, &posted, &|_| None) {
        Ok(n) => {
            println!("  library {n} movements → {LIBRARY}");
            Ok(json!({ "ok": true }))
        }
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn state(ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let actx = ctx.actx();
    let track = actx.open.lock().expect("open lock").track.clone();
    let tick = params.num("tick", 0.0).max(0.0) as usize;
    Ok(agent::state_at(
        &track,
        tick,
        actx.skeleton,
        actx.nodes,
        actx.desk,
    ))
}

pub fn proposal(ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let actx = ctx.actx();
    let track = actx.open.lock().expect("open lock").track.clone();
    match agent::proposal(&params.body, &track, actx.skeleton, actx.nodes, actx.desk) {
        Ok(out) => {
            println!("  proposal → {}", track.display());
            Ok(out)
        }
        Err(e) => {
            eprintln!("  refused: {e}");
            Err(CallError::Bad(e))
        }
    }
}

pub fn undo(ctx: &mut Ctx, _params: &Params) -> Result<serde_json::Value, CallError> {
    let actx = ctx.actx();
    agent::undo(actx.skeleton, actx.desk).map_err(CallError::Bad)
}

