//! The models mode and modo edit — `author::models`'s seventeen routes,
//! moved.

use std::path::Path;

use serde_json::{json, Value};

use crate::author::models::{self, example, timeline, EXAMPLES_DIR};
use crate::call::{CallError, Params};

use super::Ctx;

pub fn state(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let models = &ctx.state.models;
    Ok(models
        .listing
        .get_or_init(|| {
            let manifest = &models.manifest;
            json!({
                "rate": manifest.rate,
                "models": manifest.models.iter().map(|m| json!({
                    "id": m.id,
                    "label": m.label,
                    "width": m.width(),
                    "blocks": m.blocks,
                    "defaults": m.defaults(),
                })).collect::<Vec<_>>(),
            })
        })
        .clone())
}

pub fn example_get(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let manifest = &ctx.state.models.manifest;
    let model = params.param("model").unwrap_or_default().to_string();
    let song = params.param("song").unwrap_or_default().to_string();
    let device = params.param("device").map(str::to_string);
    let layer = params.param("layer").map(str::to_string);
    let target = match (&device, &layer) {
        (Some(d), Some(l)) => Some((d.as_str(), l.as_str())),
        _ => None,
    };
    match manifest.model(&model) {
        Some(schema) => {
            let found = example::load(Path::new(EXAMPLES_DIR), &model, &song, target);
            let out = match found {
                Some(ex) => serde_json::to_value(&ex).unwrap_or(Value::Null),
                None => json!({ "model": model, "song": song, "rate": manifest.rate, "ticks": [] }),
            };
            Ok(json!({ "example": out, "defaults": schema.defaults() }))
        }
        None => Err(CallError::Bad(format!("no such model: {model}"))),
    }
}

pub fn example_post(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    match models::save_example(&params.body, &ctx.state.models.manifest) {
        Ok((model, song, n)) => {
            println!("  saved {n} ticks — {model} / {song}");
            Ok(json!({ "ok": true }))
        }
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn song_hits_get(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let song = params.param("song").unwrap_or_default().to_string();
    match models::safe_stem(&song) {
        Ok(song) => Ok(json!({ "ticks": models::load_song_hits(&song) })),
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn song_hits_post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    match models::save_song_hits(&params.body) {
        Ok((song, n)) => {
            println!("  saved {n} golpes — {song}");
            Ok(json!({ "ok": true }))
        }
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn export(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    match models::export(&params.body, &ctx.state.models.manifest) {
        Ok((path, rows)) => {
            println!("  exported {rows} rows → {}", path.display());
            Ok(json!({ "rows": rows, "path": path.display().to_string() }))
        }
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn showing(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let (Some(id), Some(floats)) = (body["model"].as_str(), body["floats"].as_array()) else {
        return Err(CallError::Bad("expected {model, floats}".into()));
    };
    let row: Vec<f32> = floats
        .iter()
        .map(|v| v.as_f64().unwrap_or(0.0) as f32)
        .collect();
    let id = id.to_string();
    let state = &mut *ctx.state;
    match state.preview.showing(&id, row, &state.catalog) {
        Ok(index) => Ok(json!({ "ok": true, "index": index })),
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn library_get(_params: &Params) -> Result<Value, CallError> {
    Ok(models::library_listing())
}

pub fn library_post(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(posted) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let manifest = &ctx.state.models.manifest;
    let widths = |id: &str| manifest.model(id).map(|m| m.width());
    match crate::author::library::save(crate::author::LIBRARY, &posted, &widths) {
        Ok(n) => {
            println!("  library {n} movements → {}", crate::author::LIBRARY);
            Ok(json!({ "ok": true }))
        }
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn movement(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let name = params.param("name").unwrap_or_default();
    match crate::author::library::get(crate::author::LIBRARY, name) {
        Some(m) => Ok(json!(m)),
        None => Err(CallError::Bad(format!("no movement called {name:?}"))),
    }
}

pub fn lift(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let manifest = &ctx.state.models.manifest;
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let (Some(model), Some(song), Some(name)) = (
        body["model"].as_str(),
        body["song"].as_str(),
        body["name"].as_str(),
    ) else {
        return Err(CallError::Bad(
            "expected {model, song, name, from, to}".into(),
        ));
    };
    let from = body["from"].as_u64().unwrap_or(0) as usize;
    let to = body["to"].as_u64().unwrap_or(0) as usize;
    let Ok(song) = models::safe_stem(song) else {
        return Err(CallError::Bad("not a song name".into()));
    };
    let Some(example) = example::load(Path::new(EXAMPLES_DIR), model, &song, None) else {
        return Err(CallError::Bad(format!(
            "nothing authored yet for {model} / {song}"
        )));
    };
    let authored: Vec<(usize, Value)> = example
        .ticks
        .iter()
        .map(|t| (t.tick, json!(t.floats)))
        .collect();
    let ticks = crate::author::library::slice(&authored, from, to);
    if ticks.is_empty() {
        return Err(CallError::Bad(
            "there are no keyframes in that range".into(),
        ));
    }
    let movement = json!({ "name": name, "model": model, "ticks": ticks });
    let widths = |id: &str| manifest.model(id).map(|m| m.width());
    match crate::author::library::save(crate::author::LIBRARY, &movement, &widths) {
        Ok(n) => {
            println!(
                "  lifted {} ticks from {model}/{song} → {name}",
                ticks.len()
            );
            Ok(json!({ "ok": true, "movements": n }))
        }
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn timelines(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(json!(timeline::list(Path::new(crate::author::TIMELINES))
        .iter()
        .map(|t| json!({
            "id": t.id,
            "name": t.name,
            "song": t.song,
            "rate": t.rate,
            "length_ticks": t.length_ticks,
            "segments": t.model_track.len(),
        }))
        .collect::<Vec<_>>()))
}

pub fn timeline_get(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let id = params.param("id").unwrap_or_default();
    match timeline::load(Path::new(crate::author::TIMELINES), id) {
        Some(tl) => Ok(json!(tl)),
        None => Err(CallError::Bad(format!("no timeline called {id:?}"))),
    }
}

pub fn timeline_resolve(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let id = params.param("id").unwrap_or_default();
    let tick = params
        .param("tick")
        .and_then(|t| t.parse::<usize>().ok())
        .unwrap_or(0);
    let Some(tl) = timeline::load(Path::new(crate::author::TIMELINES), id) else {
        return Err(CallError::Bad(format!("no timeline called {id:?}")));
    };
    let cat = timeline::catalog().map_err(CallError::Bad)?;
    let g = timeline::resolve(&cat, &tl, tick);
    match serde_json::from_str::<Value>(&g.to_json()) {
        Ok(v) => Ok(json!({ "tick": tick, "graph": v })),
        Err(e) => Err(CallError::Bad(e.to_string())),
    }
}

pub fn timeline_post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let tl: timeline::Timeline =
        serde_json::from_value(body).map_err(|e| CallError::Bad(format!("not a timeline: {e}")))?;
    let cat = timeline::catalog().map_err(CallError::Bad)?;
    let id = tl.id.clone();
    match timeline::save(Path::new(crate::author::TIMELINES), tl, &cat) {
        Ok(n) => {
            println!("  saved timeline {id} — {n} segments");
            Ok(json!({ "ok": true, "segments": n }))
        }
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn timeline_new(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let (Some(id), Some(song)) = (body["id"].as_str(), body["song"].as_str()) else {
        return Err(CallError::Bad(
            "expected {id, song, name?, length_ticks?}".into(),
        ));
    };
    let name = body["name"].as_str().unwrap_or(id);
    let length = match body["length_ticks"].as_u64() {
        Some(n) => n as usize,
        None => {
            match metal_router::npy::ticks_at(
                Path::new(crate::author::SONGS),
                song,
                metal_schema::TickRate::CANONICAL,
            ) {
                Some(n) => n,
                None => {
                    return Err(CallError::Bad(format!(
                        "{song} has not been digested yet — give a length_ticks, \
                         or digest it so its matrix can decide"
                    )))
                }
            }
        }
    };
    let mut tl = timeline::Timeline::new(id, name, song, length);
    tl.rate = metal_schema::TickRate::CANONICAL.hz() as f32;
    let cat = timeline::catalog().map_err(CallError::Bad)?;
    match timeline::save(Path::new(crate::author::TIMELINES), tl, &cat) {
        Ok(_) => {
            println!("  new timeline {id} — {length} ticks against {song}");
            Ok(json!({ "ok": true, "id": id, "length_ticks": length }))
        }
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn timeline_delete(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let Some(id) = body["id"].as_str() else {
        return Err(CallError::Bad("expected {id}".into()));
    };
    match timeline::remove(Path::new(crate::author::TIMELINES), id) {
        Ok(()) => Ok(json!({ "ok": true })),
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn timeline_segment(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    match models::edit_segment(&body) {
        Ok(tl) => Ok(json!(tl)),
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn timeline_export(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let manifest = &ctx.state.models.manifest;
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let Some(id) = body["id"].as_str() else {
        return Err(CallError::Bad("expected {id}".into()));
    };
    let Some(tl) = timeline::load(Path::new(crate::author::TIMELINES), id) else {
        return Err(CallError::Bad(format!("no timeline called {id:?}")));
    };
    match timeline::export(Path::new(EXAMPLES_DIR), &tl, manifest) {
        Ok(doc) => {
            let out = timeline::export_path_for(Path::new(crate::author::TIMELINES), id);
            let text =
                serde_json::to_string_pretty(&doc).map_err(|e| CallError::Bad(e.to_string()))?;
            crate::atomic::write(&out, &text).map_err(|e| CallError::Bad(e.to_string()))?;
            println!("  exported timeline {id} → {}", out.display());
            Ok(json!({ "ok": true, "path": out.display().to_string(), "ticks": tl.length_ticks }))
        }
        Err(e) => Err(CallError::Bad(e)),
    }
}
