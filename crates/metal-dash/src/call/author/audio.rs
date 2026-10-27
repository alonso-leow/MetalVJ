//! The shared audio library — `author::audio`'s eight routes, moved.

use serde_json::json;

use crate::author::audio;
use crate::call::{Bytes, CallError, Params};

use super::Ctx;

pub fn audio(ctx: &mut Ctx, _params: &Params) -> Result<Bytes, CallError> {
    let actx = ctx.actx();
    let path = actx.open.lock().expect("open lock").audio.clone();
    let Some(p) = path else {
        return Err(CallError::NotFound);
    };
    match metal_vfs::read(&p) {
        Ok(bytes) => Ok(Bytes {
            kind: audio::mime(&p),
            body: bytes,
        }),
        Err(_) => Err(CallError::NotFound),
    }
}

pub fn songs(ctx: &mut Ctx, _params: &Params) -> Result<serde_json::Value, CallError> {
    let actx = ctx.actx();
    let mut listing = audio::songs(
        std::path::Path::new(crate::author::SONGS),
        std::path::Path::new(crate::author::TRACKS),
        &actx.ingest.states(),
    );
    let open = actx.open.lock().expect("open lock").audio.clone();
    let open_stem = open
        .as_deref()
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .map(str::to_owned);
    for song in &mut listing {
        let is_open = song.get("name").and_then(serde_json::Value::as_str) == open_stem.as_deref();
        song["open"] = json!(is_open);
    }
    Ok(json!({ "songs": listing }))
}

pub fn add(ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let actx = ctx.actx();
    let name = params.param("name").unwrap_or_default().to_string();
    let last = params.param("last") != Some("0");
    match actx.ingest.chunk(&name, &params.body, last) {
        Ok(p) => {
            if last {
                println!(
                    "  added {} ({} bytes) — queued",
                    p.display(),
                    params.body.len()
                );
            }
            Ok(json!({ "ok": true }))
        }
        Err(e) => {
            eprintln!("  refused: {e}");
            Err(CallError::Bad(e))
        }
    }
}

pub fn digest(ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let actx = ctx.actx();
    let name = params.param("name").unwrap_or_default().to_string();
    match actx.ingest.digest_now(&name) {
        Ok(()) => Ok(json!({ "ok": true })),
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn open(ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let actx = ctx.actx();
    match audio::choose(&params.body) {
        Ok(next) => {
            println!(
                "  opened  {}  ·  {}",
                next.audio
                    .as_deref()
                    .unwrap_or(std::path::Path::new("no audio"))
                    .display(),
                next.track.display()
            );
            *actx.open.lock().expect("open lock") = next;
            Ok(json!({ "ok": true }))
        }
        Err(e) => Err(CallError::Bad(e)),
    }
}

pub fn trim_get(_ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let song = params.param("song").unwrap_or_default().to_string();
    Ok(audio::trim_of_at(std::path::Path::new(audio::TRIMS), &song))
}

pub fn trim_post(_ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let song = params.param("song").unwrap_or_default().to_string();
    audio::set_trim_at(std::path::Path::new(audio::TRIMS), &song, &params.body)
        .map_err(CallError::Bad)
}

pub fn reveal(_ctx: &mut Ctx, _params: &Params) -> Result<serde_json::Value, CallError> {
    let dir = std::path::Path::new(crate::author::SONGS);
    let _ = metal_vfs::create_dir_all(dir);
    let full = metal_vfs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    // `S.U7`: under `cargo test`, `call::author::tests::every_author_route_dispatches`
    // calls every row in the table including this one — a real `spawn` opens
    // a file manager on the operator's desktop that inherits `cargo test`'s
    // stdout, which is what made `cargo test | grep` hang until someone
    // closed the window by hand. Behaviour outside `cfg(test)` is unchanged.
    if cfg!(test) {
        return Ok(json!({ "opened": full.display().to_string() }));
    }
    match metal_schema::platform::Command::new("xdg-open").arg(&full).spawn() {
        Ok(_) => Ok(json!({ "opened": full.display().to_string() })),
        Err(e) => Err(CallError::Bad(format!("cannot open the folder: {e}"))),
    }
}
