//! The Colourist's two routes — `author::grade`'s dataset assembly, moved
//! behind `POST /author/grade/train` and `GET /author/grade/state`, the way
//! `router.rs` exposes `author::router`. `S.AP` point 2/4.

use std::path::Path;

use serde_json::json;

use metal_reel::grade::{self, Colourist};

use crate::author::grade as authored;
use crate::author::models::EXAMPLES_DIR;
use crate::author::SONGS;
use crate::call::{CallError, Params};

use super::Ctx;

/// Looks a model up in both the manifest (for its `ModelSchema`, the JSON
/// mirror `dense::interpolate` reads) and the live renderer (for its real
/// `metal_schema::Schema`, the one `grade::offset_of` needs `drive` from) —
/// `None` when either does not know the id, which for these two registries
/// (one generated from the other) should only ever happen together.
fn lookup<'a>(
    ctx: &'a Ctx,
    model: &str,
) -> Option<(
    &'a crate::author::models::schema::ModelSchema,
    &'static metal_render::model::Schema,
)> {
    let model_schema = ctx.state.models.manifest.model(model)?;
    let ids = ctx.state.catalog.ids();
    let idx = ids.iter().position(|i| *i == model)?;
    Some((model_schema, ctx.state.catalog.schemas()[idx]))
}

pub fn state(ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let model = params.param("model").unwrap_or_default().to_string();
    if model.is_empty() {
        return Err(CallError::Bad("which model".into()));
    }
    let Some((_, real_schema)) = lookup(ctx, &model) else {
        return Err(CallError::Bad(format!("no such model: {model}")));
    };
    let c = Colourist::load(Path::new(authored::DATA), &model, real_schema);
    Ok(json!({
        "model": model,
        "trained": c.trained(),
        "songs": c.grade().map(|g| g.songs).unwrap_or(0),
        "takes": c.grade().map(|g| g.takes).unwrap_or(0),
        "error": c.grade().and_then(|g| g.error).map(|(ours, base)| json!({ "ours": ours, "base": base })),
    }))
}

/// `GET /grade/matrix?song=` — the raw digest a lane paints as columns,
/// split by [`authored::matrix_from_rows`]'s documented order. An
/// undigested/missing song is not an error (rule 5: the route resolves what
/// it can and says so) — it comes back as `ticks: 0` and empty arrays, which
/// is what the lane reads as "no song — nothing to paint yet".
pub fn matrix(_ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let song = params.param("song").unwrap_or_default().to_string();
    if song.is_empty() {
        return Err(CallError::Bad("which song".into()));
    }
    // The matrix rows are canonical; the ruler the lane paints is canonical too.
    let rate = metal_schema::TickRate::CANONICAL.hz() as f32;
    let m = authored::matrix_for(Path::new(SONGS), &song, rate).unwrap_or(authored::Matrix {
        song: song.clone(),
        rate,
        ticks: 0,
        rms: Vec::new(),
        bandwidth: Vec::new(),
        onset: Vec::new(),
        centroid: Vec::new(),
        chroma: Vec::new(),
        voice: Vec::new(),
        has_voice: false,
        base_only: false,
    });
    Ok(serde_json::to_value(m).unwrap_or(json!({ "song": song, "rate": rate, "ticks": 0 })))
}

pub fn train(ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let model = body["model"].as_str().unwrap_or_default().to_string();
    if model.is_empty() {
        return Err(CallError::Bad("which model".into()));
    }
    let Some((model_schema, real_schema)) = lookup(ctx, &model) else {
        return Err(CallError::Bad(format!("no such model: {model}")));
    };

    let mut takes = authored::takes_for(
        Path::new(EXAMPLES_DIR),
        Path::new(SONGS),
        &model,
        model_schema,
        real_schema,
    );
    // The dataset's parts (`author::dataset`): only the ticks inside each span.
    takes.extend(crate::author::dataset::takes_for(
        Path::new(crate::author::dataset::DIR),
        Path::new(SONGS),
        &model,
        model_schema,
        real_schema,
    ));

    // The same guard `router::train` applies: a run that found nothing must
    // not erase a grade a previous, real run produced.
    if takes.is_empty() {
        let standing = Colourist::load(Path::new(authored::DATA), &model, real_schema);
        if standing.trained() {
            return Err(CallError::Bad(format!(
                "refusing: no authored examples found for {model} and there is a \
                 trained grade on disk. Nothing was overwritten."
            )));
        }
    }

    let fitted = grade::train(&model, real_schema, &takes);
    let (songs, n_takes, error) = (fitted.songs, fitted.takes, fitted.error);
    if let Err(e) = Colourist::from_grade(fitted).save(Path::new(authored::DATA)) {
        return Err(CallError::Bad(e));
    }
    Ok(json!({
        "model": model,
        "trained": true,
        "songs": songs,
        "takes": n_takes,
        "error": error.map(|(ours, base)| json!({ "ours": ours, "base": base })),
    }))
}
