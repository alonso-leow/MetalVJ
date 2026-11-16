//! Model 0, the Router — `author::router`'s five routes, moved.

use serde_json::json;

use metal_router::{take, Brain};

use crate::author::router::{self, MIN_SECONDS};
use crate::call::{Bytes, CallError, Params};

use super::Ctx;

pub fn take_get(_ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let song = params.param("song").unwrap_or_default().to_string();
    let t = take::load_take(&router::dir(), &song).unwrap_or(take::Take {
        song: song.clone(),
        rate: metal_router::features::RATE,
        picks: Vec::new(),
    });
    Ok(json!({ "song": t.song, "picks": t.picks }))
}

pub fn take_post(ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let song = body["song"].as_str().unwrap_or("").to_string();
    if !take::safe_name(&song) {
        return Err(CallError::Bad("not a song name".into()));
    }
    let (ids, _) = router::classes(&ctx.state.catalog);
    let mut picks: Vec<take::Pick> = body["picks"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    let tick = p["tick"].as_u64()? as usize;
                    let model = p["model"].as_str()?.to_string();
                    ids.contains(&model).then_some(take::Pick { tick, model })
                })
                .collect()
        })
        .unwrap_or_default();
    picks.sort_by_key(|p| p.tick);
    picks.dedup_by_key(|p| p.tick);
    let n = picks.len();
    let t = take::Take {
        song,
        rate: metal_router::features::RATE,
        picks,
    };
    match take::save_take(&router::dir(), &t) {
        Ok(()) => Ok(Bytes {
            kind: "text/plain; charset=utf-8",
            body: format!("{n} picks").into_bytes(),
        }),
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn read(ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let song = params.param("song").unwrap_or_default().to_string();
    let Some(matrix) = router::matrix_for(&song) else {
        return Err(CallError::Bad(
            "that song has no digest — digest it first".into(),
        ));
    };
    let (ids, labels) = router::classes(&ctx.state.catalog);
    let brain = Brain::load(&router::dir());
    let walked = brain.walk(&matrix, MIN_SECONDS, &ids);
    let curve = metal_router::segment::novelty(&matrix);
    const STEP: usize = 4;
    let thinned: Vec<f32> = curve.iter().step_by(STEP).copied().collect();
    Ok(json!({
        "ticks": matrix.len(),
        "noveltyStep": STEP,
        "novelty": thinned,
        "trained": brain.trained(),
        "sections": walked.iter().map(|(s, d)| json!({
            "from": s.from,
            "to": s.to,
            "model": d.as_ref().map(|x| x.model.clone()),
            "label": d.as_ref().and_then(|x| {
                ids.iter().position(|i| *i == x.model).map(|i| labels[i].clone())
            }),
            "confidence": d.as_ref().map(|x| x.confidence),
            "because": d.as_ref().and_then(|x| x.because)
                .map(|(n, v)| json!({ "feature": n, "weight": v })),
        })).collect::<Vec<_>>(),
    }))
}

pub fn state(ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let song = params.param("song").unwrap_or_default().to_string();
    Ok(router::page_data(
        &ctx.state.catalog,
        (!song.is_empty()).then_some(song.as_str()),
    ))
}

pub fn train(ctx: &mut Ctx, _params: &Params) -> Result<serde_json::Value, CallError> {
    let (ids, _) = router::classes(&ctx.state.catalog);
    let (weights, notes) = metal_router::train_from(
        &router::dir(),
        std::path::Path::new(crate::author::SONGS),
        ids,
        MIN_SECONDS,
    );
    let brain = Brain { weights };

    if !brain.trained() {
        let standing = Brain::load(&router::dir());
        if standing.trained() {
            return Err(CallError::Bad(format!(
                "refusing: this run found no examples and there is a trained model \
                 on disk ({} from {} songs). Nothing was overwritten.{}",
                standing.weights.examples,
                standing.weights.songs,
                if notes.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", notes.join("; "))
                }
            )));
        }
    }

    if let Err(e) = brain.save(&router::dir()) {
        return Err(CallError::Bad(e));
    }
    let w = &brain.weights;
    Ok(json!({
        "trained": brain.trained(),
        "examples": w.examples,
        "songs": w.songs,
        "accuracy": w.accuracy,
        "baseline": w.baseline,
        "notes": notes,
        "coverage": router::coverage(&ctx.state.catalog).into_iter()
            .map(|(m, n)| json!({ "model": m, "examples": n }))
            .collect::<Vec<_>>(),
    }))
}
