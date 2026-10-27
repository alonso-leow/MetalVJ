//! `GET /author/dataset` and `GET /author/dataset/example?part=` — the
//! Colourist dataset's authored parts (`author::dataset`), listed for the
//! sequence editor's picker and opened whole as a clip; `POST
//! /author/dataset/status` (`{part, status}`) files a part approved or pending.

use std::path::Path;

use serde_json::json;

use crate::author::dataset;
use crate::call::{CallError, Params};

use super::Ctx;

pub fn list(_params: &Params) -> Result<serde_json::Value, CallError> {
    Ok(json!({ "parts": dataset::list(Path::new(dataset::DIR)) }))
}

pub fn example(params: &Params) -> Result<serde_json::Value, CallError> {
    let part = params.param("part").unwrap_or_default().to_string();
    if part.is_empty() {
        return Err(CallError::Bad("which part".into()));
    }
    dataset::load(Path::new(dataset::DIR), &part).map_err(CallError::Bad)
}

pub fn status(_ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("body: {part, status}".into()))?;
    let (Some(part), Some(status)) = (body["part"].as_str(), body["status"].as_str()) else {
        return Err(CallError::Bad("body: {part, status}".into()));
    };
    dataset::set_status(Path::new(dataset::DIR), part, status).map_err(CallError::Bad)?;
    Ok(json!({ "part": part, "status": status }))
}
