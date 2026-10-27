//! Music loops (the loops design notes T9): the template library and a node's loop choice,
//! for the properties panel's "loop" page.
//!
//! - `GET /loops?target=model:<m>|effect:<e>` — the templates aimed at that
//!   target with their origin, and the one `Default` resolves to.
//! - `GET /loops/get?id=` — one loop and its origin.
//! - `POST /loops/save {loop, as_copy?}` — a user loop. A factory id is
//!   refused unless `as_copy`, which saves it under a fresh user id instead.
//! - `GET /loops/palettes` — every palette set as sRGB `0..1`.
//! - `GET /loops/node?graph=&node=` — what a `gen.*`/`fx.*` node plays: its
//!   choice, the loop that resolves, the state badge, and the running
//!   musical position (`State::music_pos`).
//! - `POST /loops/choose {graph, node, loop}` — `"default"` removes the
//!   config key, `"none"` writes it, anything else is a loop id.
//! - `POST /loops/song {song, from}` / `{stop}` — the editor's playback for
//!   the musical clock: it follows the song's tempo sidecar (T15).
//! - `POST /loops/tap` — "tap 1": the input's beat nearest now becomes the
//!   bar's downbeat (`Source::tap_downbeat`). `{tapped:false}` on the
//!   stand-in (its "1" is fixed at beat 0) or before the tracker locks.
//!
//! Every read goes through `metal_perform::engine`'s own resolvers
//! (`loop_choice`/`resolve_loop`/`effect_loop`), never a second copy.

use serde_json::{json, Value};

use super::{CallError, Ctx, Params};
use metal_perform::pattern::{Library, Loop, Origin, TargetKind};

fn origin_str(o: Option<Origin>) -> &'static str {
    match o {
        Some(Origin::User) => "user",
        _ => "factory",
    }
}

/// `gen.<m>` → the model, `fx.<e>` → the effect; anything else has no loops.
pub(crate) fn target_of_kind(kind: &str) -> Option<TargetKind> {
    if let Some(m) = kind.strip_prefix("gen.") {
        return Some(TargetKind::Model(m.to_string()));
    }
    kind.strip_prefix("fx.").map(|e| TargetKind::Effect(e.to_string()))
}

fn parse_target(s: &str) -> Option<TargetKind> {
    let (k, name) = s.split_once(':')?;
    match k {
        "model" => Some(TargetKind::Model(name.to_string())),
        "effect" => Some(TargetKind::Effect(name.to_string())),
        _ => None,
    }
}

pub fn list(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(target) = params.param("target").and_then(parse_target) else {
        return Err(CallError::Bad("target must be model:<name> or effect:<name>".into()));
    };
    let lib = &ctx.state.loops;
    let loops: Vec<Value> = lib
        .for_target(&target)
        .map(|l| json!({ "id": l.id, "name": l.name, "origin": origin_str(lib.origin(&l.id)) }))
        .collect();
    let default = lib.default_for(&target).map(|l| json!({ "id": l.id, "name": l.name }));
    Ok(json!({ "loops": loops, "default": default }))
}

pub fn get(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(id) = params.param("id") else {
        return Err(CallError::Bad("which loop".into()));
    };
    let lib = &ctx.state.loops;
    let Some(l) = lib.get(id) else {
        return Err(CallError::NotFound);
    };
    Ok(json!({ "loop": l, "origin": origin_str(lib.origin(id)) }))
}

/// A user id derived from `base` that no loop has yet.
fn fresh_id(lib: &Library, base: &str) -> String {
    let stem = base.trim_end_matches(|c: char| c.is_ascii_digit() || c == '-');
    let stem = stem.strip_suffix("-user").unwrap_or(stem);
    (1..)
        .map(|n| if n == 1 { format!("{stem}-user") } else { format!("{stem}-user-{n}") })
        .find(|id| lib.get(id).is_none())
        .expect("an unbounded range always finds one")
}

pub fn save(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let mut l: Loop =
        serde_json::from_value(body["loop"].clone()).map_err(|e| CallError::Bad(format!("loop: {e}")))?;
    let lib = &mut ctx.state.loops;
    if body["as_copy"].as_bool().unwrap_or(false) {
        l.id = fresh_id(lib, &l.id);
    } else if lib.origin(&l.id) == Some(Origin::Factory) {
        return Err(CallError::Bad(format!("{} is a factory template: save it as a copy", l.id)));
    }
    let id = l.id.clone();
    lib.save(l).map_err(|e| CallError::Bad(e.to_string()))?;
    Ok(json!({ "id": id }))
}

pub fn palettes(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let list: Vec<Value> = ctx
        .state
        .loops
        .palettes()
        .map(|p| {
            let groups: Vec<Value> = (0..p.groups.len())
                .map(|g| json!((0..4).map(|c| p.colour(g as u16, c)).collect::<Vec<_>>()))
                .collect();
            json!({ "id": p.id, "name": p.name, "groups": groups })
        })
        .collect();
    Ok(json!({ "palettes": list }))
}

pub fn node(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let (Some(gid), Some(nid)) = (params.param("graph"), params.param("node")) else {
        return Err(CallError::Bad("which graph and node".into()));
    };
    let g = crate::load_graph(gid).map_err(CallError::Bad)?;
    let Some(n) = g.node(nid) else {
        return Err(CallError::NotFound);
    };
    let Some(target) = target_of_kind(&n.kind) else {
        return Err(CallError::Bad(format!("{} has no loops", n.kind)));
    };
    let lib = &ctx.state.loops;
    let parsed = metal_perform::engine::loop_choice(Some(n));
    let choice = match &parsed {
        metal_perform::engine::LoopChoice::Default => "default".to_string(),
        metal_perform::engine::LoopChoice::None => "none".to_string(),
        metal_perform::engine::LoopChoice::Id(id) => id.clone(),
    };
    let (playing, state) = match &target {
        TargetKind::Model(m) => {
            let l = metal_perform::engine::resolve_loop(lib, m, &parsed);
            (l, if l.is_some() { "looped" } else { "reactive" })
        }
        TargetKind::Effect(_) => {
            let l = metal_perform::engine::effect_loop(&g, nid, lib);
            let state = match (l, metal_graph::effects::state_of(&g, nid)) {
                (Some(_), _) => "looped",
                (None, metal_schema::EffectState::Reactive) => "reactive",
                (None, metal_schema::EffectState::Off) => "off",
                (None, metal_schema::EffectState::Fixed(_)) => "fixed",
            };
            (l, state)
        }
    };
    let pos = ctx.state.music_pos;
    Ok(json!({
        "target": match &target {
            TargetKind::Model(m) => format!("model:{m}"),
            TargetKind::Effect(e) => format!("effect:{e}"),
        },
        "choice": choice,
        "playing": playing.map(|l| l.id.clone()),
        "state": state,
        "step": pos.step,
        "bar": pos.bar,
        // T16: what the "Tap 1" header shows.
        "beat": pos.beat_in_bar,
        "bpm": ctx.state.music.bpm(),
        "downbeat": ctx.sim.downbeat(),
    }))
}

pub fn choose(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let (Some(gid), Some(nid), Some(choice)) = (body["graph"].as_str(), body["node"].as_str(), body["loop"].as_str())
    else {
        return Err(CallError::Bad("needs graph, node and loop".into()));
    };
    let mut g = crate::load_graph(gid).map_err(CallError::Bad)?;
    let Some(n) = g.node_mut(nid) else {
        return Err(CallError::NotFound);
    };
    if target_of_kind(&n.kind).is_none() {
        return Err(CallError::Bad(format!("{} has no loops", n.kind)));
    }
    let mut obj = serde_json::from_str::<Value>(&n.config.to_json_str())
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    if choice == "default" {
        obj.remove("loop");
    } else {
        obj.insert("loop".into(), json!(choice));
    }
    n.config = metal_graph::Config::from_json_str(&Value::Object(obj).to_string())
        .map_err(|e| CallError::Internal(e.to_string()))?;
    crate::save_graph(&g).map_err(CallError::Bad)?;
    Ok(json!({ "graph": gid, "node": nid, "loop": choice }))
}

/// `POST /loops/song {song, from}` / `{stop: true}` (the loops design notes T15): the
/// sequence editor's playback, for the musical clock. Posted next to
/// `/author/play` (which plays the sound) on every start, jump and stop. With
/// the song's `<stem>.tempo.json` the clock follows the song's beats and "1"s
/// from `from` (a jump is a seek: the clock may go back); without one the
/// clock stays on the live input / free clock for this play and the sidecar
/// is written in the background for the next.
#[cfg(not(feature = "gpu"))]
pub fn song(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    // No sequence editor in this build: nothing plays songs.
    ctx.state.stop_song();
    Ok(json!({ "playing": false, "beats": false }))
}

#[cfg(feature = "gpu")]
pub fn song(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    use crate::call::author::playout;
    let body = params.json().unwrap_or_else(|| json!({}));
    let song = body["song"].as_str().unwrap_or_default();
    if body["stop"].as_bool() == Some(true) || song.is_empty() {
        ctx.state.stop_song();
        return Ok(json!({ "playing": false, "beats": false }));
    }
    let path = playout::song_audio(song).ok_or_else(|| CallError::Bad(format!("no audio for {song}")))?;
    let from = body["from"].as_f64().unwrap_or(0.0).max(0.0);
    let beats = match playout::tempo_track(&path) {
        Some(track) => {
            ctx.state.play_song(track, from);
            true
        }
        None => {
            ctx.state.stop_song();
            playout::make_sidecar_later(&path);
            false
        }
    };
    Ok(json!({ "playing": true, "from": from, "beats": beats }))
}

pub fn tap(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let tapped = ctx.sim.tap_downbeat();
    Ok(json!({ "tapped": tapped, "downbeat": ctx.sim.downbeat() }))
}
