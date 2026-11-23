//! The Curator — `author::curator`'s six routes, moved. Stateless (reads
//! `data/` fresh every time), so every handler here ignores `Ctx` entirely —
//! same shape as `crate::call::graph`.

use serde_json::json;

use metal_reel::curate::{self, Curator, Pick};
use metal_reel::index::Library;

use crate::author::curator::{self, page_data, DIR};
use crate::call::{Bytes, CallError, Params};

use super::Ctx;

fn dir() -> std::path::PathBuf {
    std::path::PathBuf::from(DIR)
}

pub fn state(_ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let song = params.param("song").unwrap_or_default().to_string();
    Ok(page_data((!song.is_empty()).then_some(song.as_str())))
}

pub fn scenes(_ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let name = params.param("library").unwrap_or_default().to_string();
    if !curator::safe(&name) {
        return Err(CallError::Bad("not a library name".into()));
    }
    match Library::load(&dir(), &name) {
        Ok(lib) => Ok(json!({
            "library": lib.name,
            "source": lib.source,
            "rate": lib.rate,
            "clips": lib.clips.iter().map(|c| json!({
                "id": c.id, "file": c.file, "seconds": c.seconds,
            })).collect::<Vec<_>>(),
            "scenes": curator::scenes(&lib),
        })),
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn picks_get(_ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let library = params.param("library").unwrap_or_default().to_string();
    let song = params.param("song").unwrap_or_default().to_string();
    if !curator::safe(&library) {
        return Err(CallError::Bad("not a library name".into()));
    }
    let all = curate::load_picks(&dir(), &library);
    let mine: Vec<&Pick> = all
        .iter()
        .filter(|p| song.is_empty() || p.song == song)
        .collect();
    Ok(json!({ "song": song, "picks": mine }))
}

pub fn picks_post(_ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let library = body["library"].as_str().unwrap_or("").to_string();
    let song = body["song"].as_str().unwrap_or("").to_string();
    if !curator::safe(&library) || !curator::safe(&song) {
        return Err(CallError::Bad("not a library and a song".into()));
    }
    let Ok(lib) = Library::load(&dir(), &library) else {
        return Err(CallError::Bad("no such library".into()));
    };

    let mut picks: Vec<Pick> = body["picks"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    let tick = p["tick"].as_u64()? as usize;
                    let clip = p["clip"].as_str()?.to_string();
                    let passage = p["passage"].as_u64()? as usize;
                    let (_, c) = lib.by_id(&clip)?;
                    if passage >= c.passages.len() {
                        return None;
                    }
                    Some(Pick {
                        song: song.clone(),
                        tick,
                        clip,
                        passage,
                        play: curator::play_of(p),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    picks.sort_by_key(|p| p.tick);
    picks.dedup_by_key(|p| p.tick);
    let n = picks.len();
    match curate::save_picks(&dir(), &library, &song, &picks) {
        Ok(()) => Ok(Bytes {
            kind: "text/plain; charset=utf-8",
            body: format!("{n} picks").into_bytes(),
        }),
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn read(_ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let library = params.param("library").unwrap_or_default().to_string();
    let song = params.param("song").unwrap_or_default().to_string();
    if !curator::safe(&library) {
        return Err(CallError::Bad("not a library name".into()));
    }
    let Ok(lib) = Library::load(&dir(), &library) else {
        return Err(CallError::Bad("no such library".into()));
    };
    let Some(matrix) = curator::matrix_for(&song) else {
        return Err(CallError::Bad(
            "that song has no digest — digest it first".into(),
        ));
    };
    let curator_model = Curator::load(&dir());
    let window = (curate::WINDOW_SECONDS * metal_router::features::RATE) as usize;
    let step = (curate::HOLD_SECONDS * metal_router::features::RATE) as usize;

    let mut out = Vec::new();
    let mut recent = curate::Recent::default();
    let mut tick = 0usize;
    while tick < matrix.len() {
        let end = (tick + step).min(matrix.len());
        let start = end.saturating_sub(window);
        if end - start >= 4 {
            let music = metal_router::features::describe(&matrix[start..end]);
            let now = tick as f32 / metal_router::features::RATE;
            if let Some(cue) = curator_model.choose(&lib, &music, &recent, now) {
                recent.note(&cue);
                out.push(json!({
                    "tick": tick,
                    "clip": cue.clip_id,
                    "passage": cue.passage,
                    "confidence": cue.confidence,
                    "because": cue.because.map(|(n, v)| json!({ "column": n, "weight": v })),
                }));
            }
        }
        tick = end.max(tick + 1);
    }
    Ok(json!({ "ticks": matrix.len(), "trained": curator_model.trained(), "want": out }))
}

pub fn train(_ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let library = params
        .json()
        .and_then(|b| b["library"].as_str().map(str::to_string))
        .unwrap_or_default();
    if !curator::safe(&library) {
        return Err(CallError::Bad("not a library name".into()));
    }
    let Ok(lib) = Library::load(&dir(), &library) else {
        return Err(CallError::Bad("no such library".into()));
    };
    let picks = curate::load_picks(&dir(), &library);
    let (samples, notes) = curate::samples(&picks, &lib, &mut |s| curator::matrix_for(s));
    let coupling = curate::train(&samples);

    if !coupling.trained() {
        let standing = Curator::load(&dir());
        if standing.trained() {
            return Err(CallError::Bad(format!(
                "refusing: this run found no examples and there is a trained \
                 curator on disk ({} picks from {} songs). Nothing was \
                 overwritten.{}",
                standing.coupling.picks,
                standing.coupling.songs,
                if notes.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", notes.join("; "))
                }
            )));
        }
    }

    let curator_model = Curator::from_fit(coupling);
    if let Err(e) = curator_model.save(&dir()) {
        return Err(CallError::Bad(e));
    }
    Ok(json!({
        "trained": curator_model.trained(),
        "picks": curator_model.coupling.picks,
        "songs": curator_model.coupling.songs,
        "error": curator_model.coupling.error,
        "notes": notes,
        "coverage": curator::coverage(&lib, &picks),
    }))
}
