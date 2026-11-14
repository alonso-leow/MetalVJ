//! `GET /author/dataset` and `GET /author/dataset/example?part=` — the
//! Colourist dataset's authored parts (`author::dataset`), listed for the
//! sequence editor's picker and opened whole as a clip; `POST
//! /author/dataset/status` (`{part, status}`) files a part approved or pending.

use std::path::Path;

use serde_json::json;

use crate::author::dataset;
use crate::call::{CallError, Params};

use super::Ctx;

pub fn list(params: &Params) -> Result<serde_json::Value, CallError> {
    let kind = params.param("set").or(params.param("kind"));
    Ok(json!({
        "parts": dataset::list(Path::new(dataset::dir_for(kind))),
        "loops": dataset::list_in(Path::new(dataset::LOOPS_DIR), 0),
    }))
}

/// `POST /author/dataset/loops/generate`: picks one song per genre and writes
/// the empty loop examples (`author::dataset::generate_loops`).
pub fn generate(_params: &Params) -> Result<serde_json::Value, CallError> {
    let songs = Path::new(dataset::AUDIO);
    let report = dataset::generate_loops(
        Path::new(dataset::LOOPS_DIR),
        Path::new(dataset::GATE),
        Path::new(dataset::CATALOGUE),
        |stem| metal_router::npy::ticks_at(songs, stem, metal_schema::TickRate::CANONICAL),
    )
    .map_err(CallError::Bad)?;
    Ok(json!(report))
}

pub fn example(params: &Params) -> Result<serde_json::Value, CallError> {
    let part = params.param("part").unwrap_or_default().to_string();
    if part.is_empty() {
        return Err(CallError::Bad("which part".into()));
    }
    let dir = dataset::dir_for(params.param("set").or(params.param("kind")));
    dataset::load(Path::new(dir), &part).map_err(CallError::Bad)
}

pub fn status(_ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("body: {part, status}".into()))?;
    let (Some(part), Some(status)) = (body["part"].as_str(), body["status"].as_str()) else {
        return Err(CallError::Bad("body: {part, status}".into()));
    };
    let dir = dataset::dir_for(body["set"].as_str().or(body["kind"].as_str()));
    dataset::set_status(Path::new(dir), part, status).map_err(CallError::Bad)?;
    Ok(json!({ "part": part, "status": status }))
}
