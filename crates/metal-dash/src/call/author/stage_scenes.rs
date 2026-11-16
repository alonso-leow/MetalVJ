//! `GET /author/stage/scenes` (summaries), `GET /author/stage/scene?slug=`
//! (one scene whole), `POST /author/stage/scene` (save a scene body) and
//! `GET /author/stage/scene/clip?slug=` (the scene as matrix keyframes).

use std::path::Path;

use serde_json::json;

use crate::author::stage_scenes as store;
use crate::call::{CallError, Params};

use super::Ctx;

fn slug(params: &Params) -> Result<String, CallError> {
    params.param("slug").filter(|s| !s.is_empty()).map(str::to_string).ok_or_else(|| CallError::Bad("which slug".into()))
}

pub fn list(_params: &Params) -> Result<serde_json::Value, CallError> {
    Ok(json!({ "scenes": store::list(Path::new(store::DIR)) }))
}

pub fn get(params: &Params) -> Result<serde_json::Value, CallError> {
    let s = store::load(Path::new(store::DIR), &slug(params)?).map_err(CallError::Bad)?;
    serde_json::to_value(s).map_err(|e| CallError::Bad(e.to_string()))
}

pub fn clip(params: &Params) -> Result<serde_json::Value, CallError> {
    let c = store::clip(Path::new(store::DIR), &slug(params)?).map_err(CallError::Bad)?;
    serde_json::to_value(c).map_err(|e| CallError::Bad(e.to_string()))
}

pub fn save(_ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("body: a scene".into()))?;
    let s: metal_stage::scene::Scene = serde_json::from_value(body.clone()).map_err(|e| CallError::Bad(e.to_string()))?;
    let row = store::save(Path::new(store::DIR), &s).map_err(CallError::Bad)?;
    serde_json::to_value(row).map_err(|e| CallError::Bad(e.to_string()))
}
