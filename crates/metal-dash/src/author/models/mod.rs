//! MODELS mode: a suite animation.
//!
//! A flat float row per tick, authored against a model's published schema and
//! previewed live by the real renderers. `metal-render` draws a row into a
//! picture; this produces the row.
//!
//! **What changed in the merge.** The audio library moved out to
//! [`crate::author::audio`] and is now shared with the rig mode — this module used to
//! carry its own adapted copy of it. And the `/live` relay is gone: this used
//! to POST the row it was showing to a *separate* preview server over
//! `127.0.0.1:<live-port>/live`, best-effort, invisible if nobody was
//! listening. The preview is in this process now, so [`State::showing`] hands
//! the row over by a function call. A row can no longer be lost to a
//! misconfigured port, and there is no port to configure.
//!
//! The float manifest (`data/schema.json`) is still read as plain JSON rather
//! than shared as a Rust type with `metal-render`, and still generated from the
//! renderers — see `metal_render::manifest` for why that file is generated and
//! what the hand-maintained version got wrong.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

pub mod agent_tools;
pub mod camera_solve;
pub mod dense;
pub mod example;
pub mod schema;
pub mod timeline;

use schema::Manifest;

pub(crate) const EXAMPLES_DIR: &str = crate::author::ANIMATIONS;

/// The models mode's own state: the manifest, loaded once.
///
/// Loaded at startup rather than per request because a manifest that changed
/// underneath an open page would silently change what a row means — and the
/// file is generated from the renderers this same binary is running, so it
/// cannot legitimately change while the server is up.
pub struct State {
    pub manifest: Manifest,
    /// `GET /author/models/state`'s reply, built once from `manifest` (which
    /// never changes while the server is up, see above). Building it walked
    /// every model's defaults and blocks on each request, under the author
    /// lock, for ~25 ms a call.
    pub(crate) listing: std::sync::OnceLock<Value>,
}

impl State {
    pub fn new() -> State {
        let manifest = Manifest::load(Path::new(crate::author::SCHEMA)).unwrap_or_else(|e| {
            eprintln!("error: cannot read {}: {e}", crate::author::SCHEMA);
            eprintln!("  run: cargo run -p metal-render --bin emit-schema");
            std::process::exit(1);
        });
        // One clock. `data/schema.json` carries its own rate and the audio
        // library has one too; if they ever disagreed, every authored tick
        // would be written against a grid the digested matrix is not on, and
        // the misalignment would only surface as a trained model that moved
        // slightly early. Cheap to check, impossible to notice otherwise.
        if (manifest.rate as f64 - crate::author::audio::RATE).abs() > 1e-6 {
            eprintln!(
                "error: {} says rate {}, the audio grid is {} Hz",
                crate::author::SCHEMA,
                manifest.rate,
                crate::author::audio::RATE
            );
            std::process::exit(1);
        }
        State { manifest, listing: std::sync::OnceLock::new() }
    }
}

/// What the page needs about the models before its first fetch: the whole
/// manifest, so switching model is a client-side redraw rather than a round
/// trip, plus what the renderers in *this binary* actually are.
///
/// Both, because they are two different authorities and their disagreement is
/// the failure worth catching. The manifest says what a row means; the
/// renderer says what exists. They are generated from each other
/// (`metal_render::manifest`), so a mismatch means `data/schema.json` is stale
/// and every row authored against it will be scrambled on the way into a
/// model — which is exactly the drift that went unnoticed once already, when
/// a block-order difference left the total width unchanged.
pub fn page_data(
    state: &State,
    catalog: &crate::author::render::Catalog,
    song: Option<&Path>,
) -> Value {
    payload(&state.manifest, song, &catalog.ids(), &catalog.names())
}

/// The payload itself, without the renderer.
///
/// Split out so it can be tested: building a `Renderer` means building a GPU
/// device and every model's pipelines, which is not something a unit test
/// about JSON shape should require — and a machine with no adapter would fail
/// the test for the wrong reason.
fn payload(
    manifest: &Manifest,
    song: Option<&Path>,
    built: &[&'static str],
    names: &[&'static str],
) -> Value {
    let stale: Vec<&str> = built
        .iter()
        .filter(|id| manifest.model(id).is_none())
        .copied()
        .collect();
    // The per-model entries with their default bindings. `/author/pagedata` is
    // where the properties panel reads a model's `restaurar fuentes` table:
    // the manifest itself carries no binding, so it is laid out here, the same
    // shape `GET /author/models/state` uses for the same models.
    let models: Vec<Value> = manifest
        .models
        .iter()
        .map(|m| {
            json!({
                "id": m.id,
                "label": m.label,
                "width": m.width(),
                "blocks": m.blocks,
                "defaults": m.defaults(),
                "bindings": m
                    .bindings()
                    .iter()
                    .map(|(param, var)| json!({ "param": param, "var": var }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "manifest": manifest,
        "models": models,
        "rate": manifest.rate,
        "hasSong": song.is_some(),
        "songName": song.and_then(|p| p.file_stem()).and_then(|s| s.to_str()),
        "built": built,
        "names": names,
        // Empty unless something is wrong. The page shows it loudly if not.
        "stale": stale,
    })
}

// the cleanup: the raw-socket `serve` dispatcher that used to live here moved to
// `crate::call::author::models` as typed handler functions over
// `edit_segment`/`library_listing`/`safe_stem`/`load_song_hits`/
// `save_song_hits`/`save_example`/`export` below (now `pub(crate)`) and
// `timeline`/`example`, unchanged.

/// `{timeline, track, op, segment}` → the saved timeline.
///
/// `op` is `add` / `move` / `remove`. `move` replaces the named segment
/// wholesale rather than patching its ends, because the page already holds the
/// whole segment and sending half of it back would mean two encodings of the
/// same thing. The reply is the *whole* timeline as saved, not an ack: it is
/// what makes the page's optimistic paint correctable — if the server sorted,
/// renamed or refused something, the next paint shows the truth rather than
/// the guess.
pub(crate) fn edit_segment(body: &Value) -> Result<timeline::Timeline, String> {
    let id = body["timeline"].as_str().ok_or("expected {timeline}")?;
    let track = body["track"].as_str().unwrap_or("model");
    let op = body["op"].as_str().unwrap_or("add");
    let root = Path::new(crate::author::TIMELINES);
    let mut tl = timeline::load(root, id).ok_or_else(|| format!("no timeline called {id:?}"))?;

    let seg_id = body["segment"]["id"]
        .as_str()
        .or_else(|| body["id"].as_str())
        .ok_or("a segment needs an id")?
        .to_string();

    match (track, op) {
        ("model", "remove") => {
            tl.model_track.retain(|s| s.id != seg_id);
            // An effect whose host is gone has nothing to sit behind, and
            // leaving it would make the next save fail validation with a
            // message about a segment the operator already deleted. Same
            // "removing a node removes its edges" rule `Graph::remove_node`
            // already keeps.
            tl.effect_tracks.retain(|e| e.on != seg_id);
            // And the same for a light cue that was following this generator.
            // Only the reactive ones: a `Fixed` cue holds its own values and
            // never needed a model, so deleting the generator under it is not
            // a reason to take the lamp off stage.
            tl.signal_track
                .retain(|s| s.on != seg_id || !matches!(s.state, metal_schema::EffectState::Reactive));
        }
        ("model", _) => {
            let seg: timeline::ModelSegment = serde_json::from_value(body["segment"].clone())
                .map_err(|e| format!("not a model segment: {e}"))?;
            tl.model_track.retain(|s| s.id != seg.id);
            tl.model_track.push(seg);
        }
        ("effect", "remove") => tl.effect_tracks.retain(|e| e.id != seg_id),
        ("effect", _) => {
            let seg: timeline::EffectSegment = serde_json::from_value(body["segment"].clone())
                .map_err(|e| format!("not an effect segment: {e}"))?;
            tl.effect_tracks.retain(|e| e.id != seg.id);
            tl.effect_tracks.push(seg);
        }
        ("media", "remove") => tl.media_track.retain(|m| m.id != seg_id),
        ("media", _) => {
            let seg: timeline::MediaSegment = serde_json::from_value(body["segment"].clone())
                .map_err(|e| format!("not a media segment: {e}"))?;
            tl.media_track.retain(|m| m.id != seg.id);
            tl.media_track.push(seg);
        }
        ("still", "remove") => tl.still_track.retain(|s| s.id != seg_id),
        ("still", _) => {
            let seg: timeline::StillSegment = serde_json::from_value(body["segment"].clone())
                .map_err(|e| format!("not a still segment: {e}"))?;
            tl.still_track.retain(|s| s.id != seg.id);
            tl.still_track.push(seg);
        }
        ("text", "remove") => tl.text_track.retain(|t| t.id != seg_id),
        ("text", _) => {
            let seg: timeline::TextSegment = serde_json::from_value(body["segment"].clone())
                .map_err(|e| format!("not a text segment: {e}"))?;
            tl.text_track.retain(|t| t.id != seg.id);
            tl.text_track.push(seg);
        }
        ("signal", "remove") => tl.signal_track.retain(|s| s.id != seg_id),
        ("signal", _) => {
            let seg: timeline::SignalSegment = serde_json::from_value(body["segment"].clone())
                .map_err(|e| format!("not a signal segment: {e}"))?;
            tl.signal_track.retain(|s| s.id != seg.id);
            tl.signal_track.push(seg);
        }
        _ => return Err(format!("no such track: {track:?}")),
    }

    let cat = timeline::catalog()?;
    // Saved before replying, and the reply is what came back off disk — so a
    // client can never be told an edit landed that did not.
    timeline::save(root, tl, &cat)?;
    timeline::load(root, id).ok_or_else(|| "saved but could not read back".to_string())
}

pub(crate) fn library_listing() -> Value {
    crate::author::library::listing(crate::author::LIBRARY)
}

/// A song name that came off a socket, checked before it is pasted into a
/// path.
///
/// Names here are **stems**, not file names — `click-100`, not
/// `click-100.wav` — because one stem addresses a song, its digested matrix
/// and its authored example across three directories. So this cannot be the
/// usual "resolve it and check the file exists" guard; it has to reject the
/// name itself.
///
/// It matters because `export` joins this into `data/audio` and again into
/// `data/animations`, and a name of `../../something` would land outside both
/// with no error anywhere. Nothing in the page can produce such a name; the
/// point is that the server does not depend on that being true.
pub(crate) fn safe_stem(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("no song given".into());
    }
    if name.starts_with('.')
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains('\0')
    {
        return Err(format!("{name:?} is not a song name"));
    }
    Ok(name.to_owned())
}

fn song_hits_path(song: &str) -> PathBuf {
    Path::new(EXAMPLES_DIR)
        .join("_song")
        .join(format!("{song}.json"))
}

/// The song's own important ticks — `None` on disk reads as none marked yet,
/// same convention as `example::load`.
pub(crate) fn load_song_hits(song: &str) -> Vec<usize> {
    let Ok(text) = metal_vfs::read_to_string(song_hits_path(song)) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// `{song, ticks}` in, sorted and de-duplicated onto disk.
pub(crate) fn save_song_hits(body: &[u8]) -> Result<(String, usize), String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let song = safe_stem(v.get("song").and_then(Value::as_str).unwrap_or_default())?;
    let mut ticks: Vec<usize> = v
        .get("ticks")
        .and_then(Value::as_array)
        .ok_or("expected {song, ticks}")?
        .iter()
        .filter_map(Value::as_u64)
        .map(|t| t as usize)
        .collect();
    ticks.sort_unstable();
    ticks.dedup();
    let path = song_hits_path(&song);
    if let Some(dir) = path.parent() {
        metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    crate::atomic::write(&path, serde_json::to_string(&ticks).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    Ok((song, ticks.len()))
}

/// Parses `{model, song, rate, ticks, device?, layer?}`, validates it
/// against that model's schema, and writes it. `device`/`layer`, both
/// optional, are addressing — peeled off the envelope before the rest is
/// parsed into [`example::Example`], so they never end up serialized inside
/// the file itself, shared or override. Absent, this saves at exactly the
/// path it always has. Returns what got saved, for the server log line.
pub(crate) fn save_example(
    body: &[u8],
    manifest: &Manifest,
) -> Result<(String, String, usize), String> {
    let mut v: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let obj = v.as_object_mut().ok_or("expected a json object")?;
    let device = obj
        .remove("device")
        .and_then(|d| d.as_str().map(str::to_string));
    let layer = obj
        .remove("layer")
        .and_then(|l| l.as_str().map(str::to_string));
    let target = match (&device, &layer) {
        (Some(d), Some(l)) => Some((d.as_str(), l.as_str())),
        _ => None,
    };
    let example: example::Example = serde_json::from_value(v).map_err(|e| e.to_string())?;
    let schema = manifest
        .model(&example.model)
        .ok_or_else(|| format!("no such model: {}", example.model))?;
    let (model, song_name) = (example.model.clone(), example.song.clone());
    let n = example::save(Path::new(EXAMPLES_DIR), example, schema, target)?;
    Ok((model, song_name, n))
}

/// `{model, song, device?, layer?}` → the dense `.npy`, written beside the
/// sparse example it came from. Row count comes from the song's own digested
/// matrix — refused rather than guessed if the song has not been digested
/// yet, since a length no other matrix agrees with is worse than an export
/// that waits. `device`/`layer` absent exports the shared corpus at exactly
/// the pre-fix path; given, exports that layer's override at its own sibling
/// `.8hz.npy`, so a layer-specific `.npy` never overwrites the shared one or
/// another layer's.
pub(crate) fn export(body: &[u8], manifest: &Manifest) -> Result<(PathBuf, usize), String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let model_id = v
        .get("model")
        .and_then(Value::as_str)
        .ok_or("no model given")?;
    let song_name = v
        .get("song")
        .and_then(Value::as_str)
        .ok_or("no song given")?;
    let device = v.get("device").and_then(Value::as_str);
    let layer = v.get("layer").and_then(Value::as_str);
    // Checked here, not just inside `example::load`: `load` silently falls
    // back to the shared corpus on a bad component, which is right for a
    // *read*, but the file name built below is a *write* target, and an
    // unchecked `device`/`layer` string is exactly the traversal shape
    // `safe_stem` already guards the song name against.
    let target = match (device, layer) {
        (Some(d), Some(l)) => Some((example::safe_component(d)?, example::safe_component(l)?)),
        _ => None,
    };
    let schema = manifest
        .model(model_id)
        .ok_or_else(|| format!("no such model: {model_id}"))?;

    // `model_id` was checked above by being found in the manifest; this is the
    // matching check for the other half — see `safe_stem`.
    let song_name = safe_stem(song_name)?;

    let example = example::load(Path::new(EXAMPLES_DIR), model_id, &song_name, target)
        .ok_or_else(|| format!("no example saved yet for {model_id} / {song_name}"))?;
    // Canonical row count, so the (canonical) keyframe ticks and the matrix
    // rows are on the same grid and cannot disagree.
    let rows = metal_router::npy::ticks_at(
        Path::new(crate::author::SONGS),
        &song_name,
        metal_schema::TickRate::CANONICAL,
    )
    .ok_or_else(|| {
        format!("{song_name} has not been digested yet — its audio matrix decides the row count")
    })?;

    let dense = dense::interpolate(&example, schema.width(), rows);
    let file_name = match target {
        Some((d, l)) => metal_schema::digest_file_name(&format!("{song_name}@{d}-{l}"), metal_schema::TickRate::CANONICAL),
        None => metal_schema::digest_file_name(&song_name, metal_schema::TickRate::CANONICAL),
    };
    let out = Path::new(EXAMPLES_DIR).join(model_id).join(file_name);
    dense::write_npy(&out, &dense, schema.width())?;
    Ok((out, rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        serde_json::from_str(
            r#"{"rate":8.0,"models":[
                {"id":"geoterrain","label":"Geoterrain","blocks":[
                    {"type":"floats","fields":[{"name":"radius","min":0.0,"max":5.0,"default":1.0}]},
                    {"type":"color_slot","name":"principal"}
                ]}
            ]}"#,
        )
        .unwrap()
    }

    #[test]
    fn the_payload_carries_the_whole_manifest_so_switching_model_needs_no_round_trip() {
        let data = payload(&manifest(), None, &["geoterrain"], &["Geoterrain"]);
        let models = data["manifest"]["models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["id"], "geoterrain");
        assert_eq!(data["hasSong"], false);
    }

    #[test]
    fn the_payload_carries_each_models_default_bindings() {
        let manifest: Manifest = serde_json::from_str(
            r#"{"rate":8.0,"models":[{"id":"forge","label":"Forge","blocks":[
                {"type":"floats","fields":[{"name":"rms","min":0.0,"max":1.0,"default":0.5}]}
            ]}]}"#,
        )
        .unwrap();
        let data = payload(&manifest, None, &[], &[]);
        assert_eq!(data["models"][0]["id"], "forge");
        assert_eq!(data["models"][0]["bindings"][0], json!({ "param": "rms", "var": "rms" }));
    }

    #[test]
    fn an_open_song_carries_its_stem_not_its_full_path() {
        let data = payload(
            &manifest(),
            Some(Path::new("data/audio/click-100.wav")),
            &["geoterrain"],
            &["Geoterrain"],
        );
        assert_eq!(data["hasSong"], true);
        assert_eq!(data["songName"], "click-100");
    }

    #[test]
    fn saving_an_example_for_an_unknown_model_is_refused() {
        let body = br#"{"model":"nope","song":"s","rate":8.0,"ticks":[]}"#;
        assert!(save_example(body, &manifest()).is_err());
    }

    #[test]
    fn saving_a_wrong_width_tick_is_refused_rather_than_written() {
        let body = br#"{"model":"geoterrain","song":"s","rate":8.0,
                        "ticks":[{"tick":0,"floats":[1.0,2.0]}]}"#; // schema wants 13
        assert!(save_example(body, &manifest()).is_err());
    }

    /// The traversal guard for the *other* name a request carries. `choose`
    /// covers the song being opened and lives with the audio library now;
    /// this covers the song named in an export, which is a stem rather than a
    /// file name and so cannot be checked the same way. See `safe_stem`.
    #[test]
    fn a_song_name_that_climbs_out_of_the_data_directory_is_refused() {
        for bad in ["../../etc/passwd", "..", "a/b", "a\\b", ".hidden", ""] {
            assert!(safe_stem(bad).is_err(), "{bad:?} should not be a song name");
        }
        assert_eq!(safe_stem(" click-100 ").expect("a plain name"), "click-100");
    }

    #[test]
    fn the_payload_flags_a_renderer_the_manifest_has_never_heard_of() {
        // A stale data/schema.json is the failure that scrambles every
        // authored row while every width still looks plausible. The page shows
        // this loudly; here it just has to be present and correct.
        let data = payload(
            &manifest(),
            None,
            &["geoterrain", "turing"],
            &["Geoterrain", "Turing"],
        );
        assert_eq!(
            data["stale"].as_array().expect("stale"),
            &[serde_json::json!("turing")]
        );
        let clean = payload(&manifest(), None, &["geoterrain"], &["Geoterrain"]);
        assert!(clean["stale"].as_array().expect("stale").is_empty());
    }
}
