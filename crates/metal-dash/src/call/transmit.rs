//! The transmission surface: what is on air, the float monitor, the router,
//! footage, transport, drive, I/O selection, the scene projection, triggers,
//! and `/reload` (`O.7b`). Grouped here because every one of them reads or
//! writes `State` directly rather than through `outputs`/`layers` — see
//! `call/outputs.rs` for the ones that do.

use web_time::Instant;

use serde_json::{json, Value};

use crate::call::{CallError, Ctx, Params};
use crate::layers::{Blend, Sector};
use crate::router::Mode;
use crate::{devices, digest, output, router};

pub fn state(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let state = &mut *ctx.state;
    let sim = &mut *ctx.sim;
    let paint = &mut *ctx.paint;
    let time = state.time();
    let models = paint.model_count();
    let air = state.airing(sim, time);
    let on_air = air.on_air;
    state.reels.collect();
    if !state.overlays.is_empty() {
        let alive: Vec<(String, String)> = state
            .device_ids()
            .into_iter()
            .flat_map(|d| {
                let ids: Vec<String> = state.routed_to(&d).iter().map(|l| l.id.clone()).collect();
                ids.into_iter().map(move |l| (d.clone(), l))
            })
            .collect();
        state
            .overlays
            .prune(time, |d, l| alive.iter().any(|(a, b)| a == d && b == l));
    }
    let mut outputs =
        state
            .outputs
            .describe(&state.limits, &air, models, &state.sources, &state.layers);
    // P5: each screen carries its own triggers, read straight off `sources`
    // (regla 4 — the config is a read of the graph, not a second state).
    if let Some(rows) = outputs.as_array_mut() {
        for row in rows {
            if let Some(id) = row["id"].as_str() {
                if let Some(dev) = state.sources.get(id) {
                    // O.10b: `source` travels with the row so the shell's
                    // keyboard reader can match `TriggerSource::Key` without a
                    // second route (`shell::keybinds::triggers_from_state`).
                    // The OSC/MIDI readers read `state.sources` directly.
                    row["triggers"] = json!(dev
                        .triggers
                        .iter()
                        .map(|t| json!({
                            "id": t.id,
                            "source": t.source,
                            "target": t.target,
                            "params": t.params,
                        }))
                        .collect::<Vec<_>>());
                }
            }
        }
    }
    // Loop detection (Phase 4B): read the Looper's status once, so the
    // `repeat` block below and any other reader agree on one snapshot.
    let repeat = state.looper.status();
    // O.10b: the learn arm, read off the one global (`crate::input::learn`)
    // so the grid and any AI tool see the same "waiting for a control"
    // state — the read the overlays grid's learn badge paints. In the air
    // only, so `null` is the normal resting value (regla 4: a read, never a
    // second state).
    let learning = crate::input::learn()
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|a| json!({ "trigger": a.trigger, "kind": a.kind })));
    let payload = json!({
        "assets": crate::asset_list(paint),
        "reel": state.reels.report(),
        "ingest": ingest_jobs(),
        "on_air": on_air,
        "running": state.paused_at.is_none(),
        "learning": learning,
        "time": time,
        "node": state.local_node,
        "port": state.port,
        "router": {
            "mode": state.router.mode.as_str(),
            "chosen": state.router.chosen,
            "picked": state.router.picked,
            "group": state.router.group(),
            "position": state.router.position(),
            "plays": state.router.plays(),
            "regroup_in": state.router.regroup_in(time),
            "beats_per_switch": router::BEATS_PER_SWITCH,
            "regroup_seconds": router::REGROUP_SECONDS,
            "guessing_tempo": state.router.guessing_tempo,
            "model": {
                "trained": state.router.content.brain.trained(),
                "classes": state.router.content.brain.weights.classes,
                "examples": state.router.content.brain.weights.examples,
                "songs": state.router.content.brain.weights.songs,
                "accuracy": state.router.content.brain.weights.accuracy,
                "baseline": state.router.content.brain.weights.baseline,
                "sensitivity": state.router.content.sensitivity,
                "warmth": state.router.content.warmth(),
                "holding_for": state.router.content.holding_for(time),
                "hold_seconds": router::HOLD_SECONDS,
                "falling_back": state.router.content.falling_back,
                "on": state.router.content.decision.as_ref().map(|d| json!({
                    "model": d.model,
                    "confidence": d.confidence,
                    "because": d.because.map(|(n, v)| json!({ "feature": n, "weight": v })),
                    "ranked": d.ranked.iter().take(5)
                        .map(|(m, p)| json!({ "model": m, "p": p }))
                        .collect::<Vec<_>>(),
                })),
            },
        },
        "devices": state.devices(&air, models),
        "outputs": outputs,
        "collections": state.collections.describe(models),
        "overlays": state.overlays.report(time),
        "limits": state.limits.describe(),
        "blends": Blend::all().iter().map(|b| b.as_str()).collect::<Vec<_>>(),
        "feeds": crate::layers::Feed::all().iter().map(|f| f.as_str()).collect::<Vec<_>>(),
        "sector_presets": Sector::presets().iter()
            .map(|(id, label, s)| json!({
                "id": id, "label": label,
                "x": s.x, "y": s.y, "w": s.w, "h": s.h,
            })).collect::<Vec<_>>(),
        "post": {
            "shake": state.post.shake,
            "light_leak": state.post.light_leak,
            "ghost": state.post.ghost,
            "kaleido": state.post.kaleido,
            "crt": state.post.crt,
            "glitch": state.post.glitch,
            "posterize": state.post.posterize,
            "zoom": state.post.zoom,
            "fire": state.post.fire,
            "lorenz": state.post.lorenz,
            "vignette": state.post.vignette,
            "blur": state.post.blur,
            "sharpen": state.post.sharpen,
            "grain": state.post.grain,
        },
        // Loop detection (Phase 4B): what the readout (`shell/header.rs`,
        // `L=<n> ●`) and any AI tool read — the Looper's own `Status`, never
        // a second copy of it (regla 4/5). `engaged` is the *fact* (bool) the
        // readout fills its marker from; `detector` carries the found length
        // and confidence even in `arm`, when the detector lights up but does
        // not act.
        "repeat": {
            "mode": repeat.mode.as_str(),
            "engaged": repeat.engaged.is_some(),
            "length_beats": repeat.length_beats,
            "phase": repeat.phase,
            "detector": repeat.detector.map(|d| json!({
                "length_beats": d.length_beats,
                "since_beat": d.since_beat,
                "confidence": d.confidence,
            })),
        },
        "io": devices::describe(
            state.selected_input.as_deref(),
            state.selected_display.as_deref(),
        ),
        "digest": {
            "real": sim.is_real(),
            "describe": sim.describe(),
            "controllable": sim.controllable(),
            "confidence": sim.confidence(),
            "rate": digest::RATE,
        },
        "paint": { "real": paint.real(), "describe": paint.describe() },
        // S.U18 companion: what this instance can do for the one client.
        // The native shell hides desktop-only controls behind a lock
        // overlay (`shell::restricted`) when the node it composes into
        // cannot serve them — a build without `gpu` has no window to open,
        // so `current_exe --monitor` (`POST /monitor op open`) would spawn
        // a child that paints nothing. `files` (`call::files`) works
        // headless, so it is true everywhere it can be: the flag is only
        // false if the data directory is not writable at all.
        "capabilities": {
            "windows": cfg!(feature = "gpu"),
            "files": true,
        },
    });
    Ok(payload)
}

pub fn monitor(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let state = &mut *ctx.state;
    let sim = &mut *ctx.sim;
    let paint = &mut *ctx.paint;
    let time = state.time();
    let frame = sim.poll(time);
    let air = state.airing(sim, time);
    let models = paint.model_count();
    let index = air.on_air;
    let schema = paint.schemas()[index];
    let passage = state.router.content.passage();
    let base = state.base[index].clone();
    let row = crate::row_for(paint, state, index, &base, &frame, time, passage, None);

    let mut sources = Vec::new();
    for device in state.device_ids() {
        let mut layers: Vec<crate::layers::Layer> =
            state.routed_to(&device).into_iter().cloned().collect();
        state
            .overlays
            .on(&device, time)
            .apply(&air, models, &mut layers);
        for layer in layers {
            let showing = layer.showing(&air, models);
            let schema = paint.schemas()[showing];
            let base: Vec<f32> = layer
                .base
                .clone()
                .unwrap_or_else(|| state.base[showing.min(state.base.len() - 1)].clone());
            let row = crate::row_for(
                paint,
                state,
                showing,
                &base,
                &frame,
                time,
                passage,
                layer.library.as_deref(),
            );
            sources.push(json!({
                "device": device,
                "layer": layer.id,
                "showing": showing,
                "model": paint.names()[showing],
                "floats": crate::named_floats(schema, &row),
            }));
        }
    }

    Ok(json!({
        "digest": digest::COLUMNS.iter().zip(frame.f.iter())
            .map(|(name, v)| json!({ "name": name, "v": v }))
            .collect::<Vec<_>>(),
        "floats": crate::named_floats(schema, &row),
        "model": paint.names()[index],
        "sources": sources,
        "onset": frame.onset(),
    }))
}

/// S.U16/O.11's spawn side — the `POST` sibling of `monitor`'s `GET` route.
pub fn monitor_open(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    if body["op"].as_str() != Some("open") {
        return Err(CallError::Bad(format!(
            "no such op: {}",
            body["op"].as_str().unwrap_or("")
        )));
    }
    let Some(device) = body["device"].as_str() else {
        return Err(CallError::Bad("a device id is required".into()));
    };
    if ctx.state.outputs.find(device).is_none() {
        return Err(CallError::NotFound);
    }
    let exe = std::env::current_exe().map_err(|e| CallError::Internal(e.to_string()))?;
    let child = metal_schema::platform::Command::new(exe)
        .arg("--monitor")
        .arg(device)
        .arg("--port")
        .arg(ctx.state.port.to_string())
        .spawn()
        .map_err(|e| CallError::Internal(e.to_string()))?;
    Ok(json!({ "opened": true, "pid": child.id() }))
}

pub fn select(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    // LAY.14: select **by id** when one is given, so a saved preset survives a
    // model inserted in the middle of the registry. The index path stays for
    // the shell's existing on-air choice (and for callers that only ever knew
    // position); a request carrying neither is still an error.
    let chosen = if let Some(id) = body["id"].as_str() {
        ctx.paint
            .ids()
            .iter()
            .position(|x| *x == id)
            .ok_or_else(|| CallError::Bad(format!("no model called {id:?}")))?
    } else {
        let Some(index) = body["index"].as_u64() else {
            return Err(CallError::Bad("expected an index or an id".into()));
        };
        if index as usize >= ctx.paint.model_count() {
            return Err(CallError::Bad("that animation is not built yet".into()));
        }
        index as usize
    };
    ctx.state.router.chosen = chosen;
    ctx.state.router.mode = Mode::Single;
    // Cued, not yet on air necessarily: warm it in the background so it does
    // not stall the first frame that shows it.
    crate::warm::request(chosen);
    Ok(json!({ "ok": true }))
}

pub fn router(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    if let Some(m) = body["mode"].as_str().and_then(Mode::parse) {
        ctx.state.router.mode = m;
    }
    if let Some(list) = body["picked"].as_array() {
        let model_count = ctx.paint.model_count();
        ctx.state.router.picked = list
            .iter()
            .filter_map(|v| v.as_u64())
            .map(|v| v as usize)
            .filter(|i| *i < model_count)
            .collect();
        for &i in &ctx.state.router.picked {
            crate::warm::request(i);
        }
    }
    if let Some(v) = body["sensitivity"].as_f64() {
        ctx.state.router.content.sensitivity = (v as f32).clamp(0.0, 1.0);
    }
    if body["reload"].as_bool() == Some(true) {
        ctx.state.router.content.brain =
            metal_router::Brain::load(&output::data_root().join(metal_router::DIR));
    }
    Ok(json!({ "ok": true }))
}

pub fn reel(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let state = &mut *ctx.state;
    let paint = &mut *ctx.paint;
    match body["op"].as_str().unwrap_or("") {
        "index" => {
            let Some(path) = body["path"].as_str().filter(|s| !s.is_empty()) else {
                return Err(CallError::Bad("expected a folder".into()));
            };
            let path = std::path::PathBuf::from(path);
            let name = body["name"]
                .as_str()
                .map(str::to_string)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| {
                    metal_reel::index::slug(
                        &path
                            .file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_else(|| "biblioteca".into()),
                    )
                });
            let mut opts = metal_reel::index::Options::default();
            if let Some(r) = body["rate"].as_f64() {
                opts.rate = (r as f32).clamp(1.0, 30.0);
            }
            if let Some(w) = body["width"].as_u64() {
                opts.width = (w as u32).clamp(160, 1920);
            }
            match state.reels.index(path, name, opts) {
                Ok(()) => Ok(json!({ "ok": true })),
                Err(e) => Err(CallError::Bad(e)),
            }
        }
        "open" => {
            // P0: a `path` opens a folder that may not be indexed yet — the
            // lazy open. `name` (or the path's own basename) names the
            // library; the open itself waits for the index job (`collect`).
            if let Some(path) = body["path"].as_str().filter(|s| !s.is_empty()) {
                let path = std::path::PathBuf::from(path);
                let name = body["name"]
                    .as_str()
                    .map(str::to_string)
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| {
                        metal_reel::index::slug(
                            &path
                                .file_name()
                                .map(|f| f.to_string_lossy().to_string())
                                .unwrap_or_else(|| "biblioteca".into()),
                        )
                    });
                match state.reels.open_lazy(path, name.clone()) {
                    Ok(indexing) => Ok(json!({ "opening": name, "indexing": indexing })),
                    Err(e) => Err(CallError::Bad(e)),
                }
            } else {
                let name = body["name"].as_str().unwrap_or("");
                match state.reels.open(Some(name)) {
                    Ok(()) => Ok(json!({ "ok": true })),
                    Err(e) => Err(CallError::Bad(e)),
                }
            }
        }
        "close" => {
            let _ = state.reels.open(None);
            for i in 0..paint.model_count() {
                if let Some(reel) = paint.reel(i) {
                    reel.attach(None);
                    reel.cue(None);
                }
            }
            Ok(json!({ "ok": true }))
        }
        "reload" => {
            state.reels.curator = metal_reel::Curator::load(&output::data_root());
            state.reels.relearn();
            if let Some(model) = body["model"].as_str() {
                state.reels.reload(model);
            } else {
                for id in paint.ids() {
                    state.reels.reload(id);
                }
            }
            Ok(json!({ "ok": true }))
        }
        other => Err(CallError::Bad(format!("no such op: {other}"))),
    }
}

pub fn reel_files(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let state = &mut *ctx.state;
    let name = params
        .param("name")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| state.reels.default_library().map(str::to_string));
    let Some(name) = name else {
        return Err(CallError::Bad("which library".into()));
    };
    match state.reels.files(&name) {
        Ok(files) => Ok(json!({
            "name": name,
            "files": files.into_iter().map(|(id, file, seconds)| json!({
                "id": id, "file": file, "seconds": seconds,
            })).collect::<Vec<_>>(),
        })),
        Err(e) => Err(CallError::Bad(e)),
    }
}

/// The extensions `/fs` recognises, by kind — closed sets, copied rather
/// than derived: `metal-reel`'s own `EXTENSIONS` is private and
/// `author::audio::SOUND` is `gpu`-only, and a route that only says what it
/// can open must depend on neither.
const FS_AUDIO: [&str; 7] = ["wav", "mp3", "flac", "ogg", "m4a", "opus", "aiff"];
const FS_VIDEO: [&str; 10] = [
    "mp4", "mov", "mkv", "avi", "webm", "m4v", "mpg", "mpeg", "wmv", "flv",
];
const FS_IMAGE: [&str; 6] = ["jpg", "jpeg", "png", "gif", "webp", "bmp"];

/// What a file is, from its extension alone — no probing, one level.
fn fs_kind(ext: &str) -> &'static str {
    let e = ext.to_ascii_lowercase();
    if FS_AUDIO.contains(&e.as_str()) {
        "audio"
    } else if FS_VIDEO.contains(&e.as_str()) {
        "video"
    } else if FS_IMAGE.contains(&e.as_str()) {
        "image"
    } else {
        "other"
    }
}

/// `GET /fs?path=<dir>` — one level of the file system, the browser
/// Biblioteca's left column walks. Without `path`, the root is `$HOME`.
///
/// `kind` is by extension (see [`fs_kind`]); a `.json` inside the movements
/// library's own directory is a `"sequence"`. A video's `seconds` is only
/// present when the folder is already indexed — read off the open library,
/// never probed here (regla 5). Never recurses: one level, like a file
/// manager.
pub fn fs_list(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let state = &*ctx.state;
    let path = match params.param("path").filter(|s| !s.is_empty()) {
        Some(p) => std::path::PathBuf::from(p),
        None => std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .ok_or_else(|| CallError::Bad("no $HOME to start from".into()))?,
    };
    if !metal_vfs::is_dir(&path) {
        return Err(CallError::Bad(format!(
            "{} is not a folder",
            path.display()
        )));
    }
    let parent = path.parent().map(|p| p.to_string_lossy().into_owned());
    let library_dir = output::data_root().join("library");
    let in_library_dir = metal_vfs::canonicalize(&path)
        .map(|p| p == library_dir)
        .unwrap_or(false);
    let seconds: Option<std::collections::HashMap<String, f32>> =
        state.reels.library_for_source(&path).map(|lib| {
            lib.clips
                .iter()
                .map(|c| (c.file.clone(), c.seconds))
                .collect()
        });

    let entries =
        metal_vfs::read_dir(&path).map_err(|e| CallError::Bad(format!("{}: {e}", path.display())))?;
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<Value> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            dirs.push(name);
        } else if kind.is_file() {
            let ext = std::path::Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let kind = if ext.eq_ignore_ascii_case("json") && in_library_dir {
                "sequence"
            } else {
                fs_kind(ext)
            };
            let mut row = json!({ "file": name, "kind": kind });
            if kind == "video" {
                if let Some(s) = seconds.as_ref().and_then(|m| m.get(&name)) {
                    row["seconds"] = json!(s);
                }
            }
            files.push(row);
        }
    }
    dirs.sort();
    files.sort_by(|a, b| a["file"].as_str().cmp(&b["file"].as_str()));
    Ok(json!({
        "path": path.to_string_lossy(),
        "parent": parent,
        "dirs": dirs,
        "files": files,
    }))
}

pub fn transport(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let state = &mut *ctx.state;
    match state.paused_at.take() {
        Some(at) => state.started = Instant::now() - std::time::Duration::from_secs_f32(at),
        None => state.paused_at = Some(state.time()),
    }
    Ok(json!({ "ok": true }))
}

pub fn drive(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    crate::apply_post_op(ctx.state, ctx.sim, &body);
    Ok(json!({ "ok": true }))
}

pub fn io(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let state = &mut *ctx.state;
    let reply = json!({ "ok": true });
    // Plan "Audio input is a node": the audio input is chosen on an
    // `audio.live` node (`source` in its config, `POST /graph/node
    // {op:"config"}`), and nowhere else — `input` here is refused rather than
    // silently swapping a process-wide listener that no longer exists.
    if body.get("input").is_some() {
        return Err(CallError::Bad(
            "the audio input is an audio.live node's `source` (POST /graph/node {op:\"config\"})".into(),
        ));
    }
    if let Some(v) = body["display"].as_str() {
        state.selected_display = Some(v.to_string());
    }
    Ok(reply)
}

/// `GET /reel/scenes` (2026-10-01): the saved/favourite scenes and the
/// favourites groups `reel.batch`'s list can name — `data/scenes/favorites.json`
/// as `metal_perform::scenes` reads it (empty when the file is not there yet).
pub fn reel_scenes(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let f = metal_perform::scenes::load(&output::data_root());
    let scenes: Vec<Value> = f
        .scenes
        .iter()
        .filter(|(_, s)| s.saved || s.favourite)
        .map(|(id, s)| json!({ "id": id, "name": s.name, "clip": s.clip, "start": s.start, "end": s.end }))
        .collect();
    let groups: Vec<Value> = f.groups.iter().map(|(name, ids)| json!({ "name": name, "count": ids.len() })).collect();
    Ok(json!({ "scenes": scenes, "groups": groups }))
}

/// `GET /io`: everything the machine can listen to (devices and the programs
/// playing right now) and see (cameras, screens, windows), plus what is open.
pub fn io_get(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let mut v = devices::describe(ctx.state.selected_input.as_deref(), ctx.state.selected_display.as_deref());
    v["listening"] = json!({ "describe": ctx.sim.describe(), "real": ctx.sim.is_real() });
    v["listen"] = listen_status_json(ctx.sim);
    v["video"] = json!(crate::capture::sources()
        .iter()
        .map(|s| json!({ "id": s.id, "label": s.label, "kind": s.kind }))
        .collect::<Vec<_>>());
    Ok(v)
}

pub fn scene(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let state = &*ctx.state;
    let scene = crate::escena::load(&output::data_root());
    let projected: Vec<Value> = state
        .outputs
        .list
        .iter()
        .map(|o| {
            let masks = scene.project(&o.id);
            json!({
                "output": o.id,
                "placed": scene.view(&o.id).is_some(),
                "masks": masks.iter().map(|m| m.describe(&masks)).collect::<Vec<_>>(),
            })
        })
        .collect();
    let body = params.param("body").and_then(|s| {
        let n: Vec<f32> = s.split(',').filter_map(|v| v.trim().parse().ok()).collect();
        (n.len() == 3).then(|| [n[0], n[1], n[2]])
    });
    let placed = body.map(|b| match scene.nearest_wall(b) {
        Some((wall, s, t, d)) => json!({
            "at": b,
            "wall": wall.id,
            "s": s,
            "t": t,
            "distance": d,
            "spotlight": scene.spotlight("spot", b, (0.25, 0.25)),
        }),
        None => json!({ "at": b, "wall": Value::Null }),
    });
    // `scene` is the English key; `escena` stays for an old client reading `GET /escena`.
    Ok(json!({ "scene": scene, "escena": scene, "outputs": projected, "body": placed }))
}

/// `POST /scene` (also `/escena`, the old alias) — the one write to the scene. Today only `{ op: "fit",
/// wall, output, corners }`: four `(s, t) ↔ (x, y)` correspondences dragged
/// onto the real wall become that output's homography, stored as
/// `Wall::fit[output]` and saved through [`crate::escena::Escena::save`]
/// (plan: "calibrate wall").
pub fn escena_post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    match body["op"].as_str().unwrap_or_default() {
        "fit" => {
            let wall_id = body["wall"]
                .as_str()
                .ok_or_else(|| CallError::Bad("which wall: `wall`".into()))?;
            let output = body["output"]
                .as_str()
                .ok_or_else(|| CallError::Bad("which output: `output`".into()))?;
            let corners = body["corners"]
                .as_array()
                .filter(|c| c.len() == 4)
                .ok_or_else(|| CallError::Bad("four `corners`, each [s, t, x, y]".into()))?;
            let mut st = [[0.0f32; 2]; 4];
            let mut xy = [[0.0f32; 2]; 4];
            for (i, corner) in corners.iter().enumerate() {
                let a = corner
                    .as_array()
                    .filter(|a| a.len() == 4)
                    .ok_or_else(|| CallError::Bad("each corner is [s, t, x, y]".into()))?;
                let n = |k: usize| -> Result<f32, CallError> {
                    a[k].as_f64()
                        .map(|v| v as f32)
                        .ok_or_else(|| CallError::Bad("corner numbers".into()))
                };
                st[i] = [n(0)?, n(1)?];
                xy[i] = [n(2)?, n(3)?];
            }
            let h = crate::escena::Homography::from_corners(&st, &xy)
                .ok_or_else(|| CallError::Bad("those four corners are not a calibration".into()))?;
            let root = output::data_root();
            let mut scene = crate::escena::load(&root);
            {
                let wall = scene
                    .walls
                    .iter_mut()
                    .find(|w| w.id == wall_id)
                    .ok_or_else(|| CallError::Bad(format!("no such wall: {wall_id}")))?;
                wall.fit.insert(output.to_string(), h.to_fit());
            }
            scene
                .save(&root)
                .map_err(|e| CallError::Bad(format!("cannot save the scene: {e}")))?;
            Ok(json!({ "ok": true, "wall": wall_id, "output": output, "fit": h.to_fit() }))
        }
        other => Err(CallError::Bad(format!("no such op: {other}"))),
    }
}

pub fn reload(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let state = &mut *ctx.state;
    state.outputs = crate::Outputs::load();

    let (mut sources, pool) = output::load_devices();
    for out in &state.outputs.list {
        sources
            .entry(out.id.clone())
            .or_insert_with(crate::layers::Device::default);
    }
    state.sources = sources;
    state.layers = pool;

    state.collections = crate::Collections::load();
    state.sequences = crate::sequence::Sequences::load();
    state.switches = crate::switch::Switches::load();
    state.reels = crate::Reels::new(&output::data_root());

    Ok(json!({ "reloaded": true }))
}

/// O.10b: the learn half of `POST /trigger/<id>` — `{"op":"learn", …}`.
///
/// Either arms the trigger (`{"op":"learn"}` / `{"op":"learn","kind":"key"}`)
/// so the reader sends the first thing it hears, or writes a concrete
/// `source` (`{"op":"learn","source":{"source":"osc","addr":"/pad/1"}}`), the
/// shape the readers post. The arm is in-the-air only (`crate::input::learn`)
/// and is cleared on every write, so a learn never outlives its first source.
///
/// The trigger is searched across `state.sources` — the same list the fire
/// path searches — so an id that does not exist is refused before anything is
/// armed or written. A source already used by a *different* trigger is still
/// reassigned, and the reply carries `"warning":"reassigned"` (plan point 3).
fn learn_trigger(ctx: &mut Ctx, trigger_id: &str, body: Value) -> Result<Value, CallError> {
    if trigger_id.is_empty() {
        return Err(CallError::Bad("a learn op needs a trigger id".into()));
    }
    let device_id = ctx
        .state
        .sources
        .iter()
        .find(|(_, d)| d.triggers.iter().any(|t| t.id == trigger_id))
        .map(|(id, _)| id.clone());
    let Some(device_id) = device_id else {
        return Err(CallError::Bad(format!("no such trigger: {trigger_id}")));
    };

    if let Some(source_value) = body.get("source").filter(|v| !v.is_null()) {
        let source: crate::trigger::TriggerSource = serde_json::from_value(source_value.clone())
            .map_err(|e| CallError::Bad(format!("bad source: {e}")))?;
        // Plan point 3: a control already used by another trigger is
        // reassigned, but the reply says so.
        let reused = ctx
            .state
            .sources
            .values()
            .chain(std::iter::once(&ctx.state.preview))
            .flat_map(|d| d.triggers.iter())
            .any(|t| t.id != trigger_id && t.source == source);
        if let Some(dev) = ctx.state.sources.get_mut(&device_id) {
            if let Some(t) = dev.triggers.iter_mut().find(|t| t.id == trigger_id) {
                t.source = source.clone();
            }
        }
        // The same save path every other `Device.triggers` writer uses
        // (`call::outputs::outputs`): `outputs.json` through and back. Not
        // in tests — rule 8, a test must never write the operator's `data/`;
        // the in-memory mutation above is still exercised in full.
        if !cfg!(test) {
            if let Err(e) = ctx.state.outputs.save(&ctx.state.sources, &ctx.state.layers) {
                eprintln!("  outputs not saved: {e}");
            }
        }
        *crate::input::learn().lock().expect("learn lock") = None;
        let mut reply = json!({ "learned": trigger_id, "source": source });
        if reused {
            reply["warning"] = json!("reassigned");
        }
        return Ok(reply);
    }

    let kind = body["kind"].as_str().unwrap_or("osc").to_string();
    *crate::input::learn().lock().expect("learn lock") = Some(crate::input::LearnArm {
        trigger: trigger_id.to_string(),
        kind: kind.clone(),
    });
    Ok(json!({ "learning": trigger_id, "kind": kind }))
}

/// O.10: `POST /trigger/<id>`. The trigger id is the prefix route's tail.
///
/// O.10b: the body may instead carry `{"op":"learn", …}`, which writes the
/// heard source into this trigger (see [`learn_trigger`]) rather than firing
/// it. With any other body the route fires exactly as it always did.
pub fn trigger(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let trigger_id = params.tail.as_deref().unwrap_or_default().to_string();

    // Parse the body once; the fire path below reads it again through
    // `params.json()` for `switch-`'s own `state`, unchanged.
    let body = params.json();
    if body.as_ref().and_then(|b| b["op"].as_str()) == Some("learn") {
        return learn_trigger(ctx, &trigger_id, body.unwrap_or_else(|| json!({})));
    }

    let trigger = ctx
        .state
        .sources
        .values()
        .flat_map(|d| d.triggers.iter())
        .find(|t| t.id == trigger_id)
        .cloned();

    let Some(trigger) = trigger else {
        return Err(CallError::Bad(format!("no such trigger: {trigger_id}")));
    };

    match trigger.target.as_str() {
        "transport" => {
            let state = &mut *ctx.state;
            match state.paused_at.take() {
                Some(at) => state.started = Instant::now() - std::time::Duration::from_secs_f32(at),
                None => state.paused_at = Some(state.time()),
            }
        }
        // P5: `overlay-<movement>` plays the named movement on the device
        // and layer its own `params` carry — the same body `POST /overlay
        // {op:"play"}` runs, through the same fn (`super::overlay::play`).
        target if target.starts_with("overlay-") => {
            let movement = target.trim_start_matches("overlay-");
            let device = trigger.params["device"].as_str().unwrap_or(crate::PREVIEW);
            let layer = trigger.params["layer"].as_str().ok_or_else(|| {
                CallError::Bad(format!(
                    "trigger {trigger_id:?}: an overlay trigger needs a layer in params"
                ))
            })?;
            let looping = trigger.params["loop"].as_bool().unwrap_or(false);
            super::overlay::play(ctx, device, layer, movement, looping)?;
        }
        // `S.O.4a`: fires the same body `POST /sequences {op:"run"}` runs,
        // through `super::sequences::run`, with `device` off the trigger's
        // own saved `params` (same shape as `overlay-`'s `device`/`layer`
        // above).
        target if target.starts_with("sequence-") => {
            let id = target.trim_start_matches("sequence-").to_string();
            let device = trigger
                .params
                .get("device")
                .and_then(Value::as_str)
                .unwrap_or(crate::PREVIEW)
                .to_string();
            super::sequences::run(ctx, &id, &device)?;
        }
        // `S.AK`/`O.28` point 3: a switch fires the same `on`/`off` batch
        // `POST /switch` runs. A body `{"state": "on"|"off"}` picks a side
        // (MIDI/OSC note on/off, once `O.10b` lands, will send exactly
        // this); no body toggles — the trigger's own `params` never carry
        // this, it comes off the fired request itself, same as `overlay-`'s
        // `device`/`layer` above but read from the HTTP body rather than
        // the trigger's saved `params`.
        target if target.starts_with("switch-") => {
            let id = target.trim_start_matches("switch-");
            let sw = ctx
                .state
                .switches
                .find(id)
                .cloned()
                .ok_or_else(|| CallError::Bad(format!("no such switch: {id}")))?;
            let turn_on = match params
                .json()
                .and_then(|b| b["state"].as_str().map(str::to_string))
            {
                Some(s) if s == "on" => true,
                Some(s) if s == "off" => false,
                Some(other) => {
                    return Err(CallError::Bad(format!(
                        "trigger {trigger_id:?}: state must be \"on\" or \"off\", got {other:?}"
                    )))
                }
                None => !sw.state(&crate::load_graph(&sw.graph).map_err(CallError::Bad)?),
            };
            let entries = if turn_on {
                sw.set.clone()
            } else {
                sw.off_entries()
            };
            let mut g = crate::load_graph(&sw.graph).map_err(CallError::Bad)?;
            // Parity P-E: the toggle instant, so the engine plays the node's
            // in/out transition (`metal_perform::transition`).
            let now = ctx.state.time();
            ctx.state.transitions.stamp(&g, &entries, now);
            metal_perform::switch::apply(&mut g, &entries);
            crate::save_graph(&g).map_err(CallError::Internal)?;
        }
        // The overlays grid's page buttons over MIDI/OSC: one step of the
        // stored page, the same `grid_page` a click on `‹`/`›` writes. The
        // panel clamps it to the pages it lays out.
        "grid-page-next" => {
            super::settings::step_grid_page(1).map_err(CallError::Bad)?;
        }
        "grid-page-prev" => {
            super::settings::step_grid_page(-1).map_err(CallError::Bad)?;
        }
        _ => {
            return Err(CallError::Bad(format!(
                "unknown trigger target: {}",
                trigger.target
            )))
        }
    }

    Ok(json!({ "fired": trigger_id }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// The same fixture shape `call::tests::every_route_dispatches` builds:
    /// a painter, a `State` with one output called `"probe"`, and an
    /// `Author` — whose `gpu`-feature construction reads `data/schema.json`
    /// off a bare relative path, hence the cwd dance around it. Deliberately
    /// stops short of ever calling `monitor_open` with a body that would
    /// reach `Command::spawn` — see that function's own doc comment.
    fn fixture() -> (
        Box<dyn crate::paint::Paint>,
        crate::State,
        crate::listeners::Listeners,
        crate::Author,
    ) {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = crate::Author::new();
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let paint = crate::paint::painter();
        let mut state = crate::State::new(paint.as_ref());
        state
            .outputs
            .list
            .push(crate::output::Output::new("probe", "Probe", 64, 64));
        state
            .sources
            .entry("probe".to_string())
            .or_insert_with(crate::layers::Device::default);
        let sim = crate::listeners::Listeners::stand_in();
        (paint, state, sim, author)
    }

    fn body(v: Value) -> Params {
        Params {
            query: HashMap::new(),
            body: v.to_string().into_bytes(),
            tail: None,
        }
    }

    /// A missing `op`, or one this route does not know, take the same
    /// branch — both refuse with `CallError::Bad`, before anything can
    /// reach `Command::spawn`.
    #[test]
    fn monitor_open_bad_op_is_bad_request() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };

        let missing_op = body(json!({ "device": "probe" }));
        assert!(matches!(
            monitor_open(&mut ctx, &missing_op),
            Err(CallError::Bad(_))
        ));

        let wrong_op = body(json!({ "op": "close", "device": "probe" }));
        assert!(matches!(
            monitor_open(&mut ctx, &wrong_op),
            Err(CallError::Bad(_))
        ));
    }

    /// `GET /listen/status` gives every bound `audio.live` a `tone` object
    /// with exactly the three keys step 11 promises (the tone design notes §6). The
    /// stand-in loads no model, so all three read the honest zeros — the same
    /// fact the colour fallback (D8) keys off.
    #[test]
    fn listen_status_reports_each_node_s_tone_object() {
        let mut pool = crate::listeners::Listeners::stand_in();
        let mut g = metal_graph::Graph::new("g", "g", metal_graph::Domain::Live);
        let id = g.add_node("audio", crate::listeners::AUDIO_LIVE, [0.0, 0.0]);
        g.node_mut(&id).unwrap().config =
            metal_graph::Config::from_json_str("{\"source\":\"stand-in\"}").unwrap();
        pool.digests(&g, crate::listeners::Digests::zero(), 0.0);

        let v = listen_status_json(&pool);
        let tone = &v["nodes"][0]["tone"];
        assert!(tone.is_object(), "every node carries a tone object: {v}");
        assert_eq!(tone["loaded"], json!(false), "the stand-in loads no model");
        assert_eq!(tone["real"], json!(false));
        assert_eq!(tone["latency_ms"], json!(0.0));
        assert!(tone["loaded"].is_boolean() && tone["real"].is_boolean() && tone["latency_ms"].is_number());
    }

    /// A well-formed `{"op":"open",...}` naming a device that is not in
    /// `state.outputs` refuses with `CallError::NotFound` — and does so
    /// before `Command::spawn`, which this test must never reach.
    #[test]
    fn monitor_open_unknown_device_is_not_found() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };

        let params = body(json!({ "op": "open", "device": "nope" }));
        assert!(matches!(
            monitor_open(&mut ctx, &params),
            Err(CallError::NotFound)
        ));
    }

    /// `GET /fs` lists one level, kinds by extension, skips dotfiles, and
    /// refuses a path that is not a folder.
    #[test]
    fn fs_lists_one_level_with_kinds() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let dir = std::env::temp_dir().join(format!("fw-fs-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(dir.join("sub")).unwrap();
        metal_vfs::write(dir.join("take.wav"), b"x").unwrap();
        metal_vfs::write(dir.join("clip.mp4"), b"x").unwrap();
        metal_vfs::write(dir.join("notes.txt"), b"x").unwrap();
        metal_vfs::write(dir.join(".hidden"), b"x").unwrap();

        let mut list = |dir: &std::path::Path| {
            let params = Params {
                query: HashMap::from([("path".to_string(), dir.to_string_lossy().into_owned())]),
                body: Vec::new(),
                tail: None,
            };
            let mut ctx = Ctx {
                state: &mut state,
                paint: paint.as_mut(),
                sim: &mut sim,
                author: &author,
            };
            fs_list(&mut ctx, &params)
        };

        let reply = list(&dir).expect("a folder lists");
        assert_eq!(reply["dirs"], json!(["sub"]));
        let files = reply["files"].as_array().expect("files is a list");
        let kind = |f: &str| {
            files
                .iter()
                .find(|r| r["file"] == f)
                .map(|r| r["kind"].clone())
                .unwrap_or_else(|| panic!("{f} missing from {files:?}"))
        };
        assert_eq!(kind("take.wav"), "audio");
        assert_eq!(kind("clip.mp4"), "video");
        assert_eq!(kind("notes.txt"), "other");
        assert!(
            files.iter().all(|r| r["file"] != ".hidden"),
            "dotfiles are skipped"
        );
        assert!(
            !files.iter().any(|r| r["file"] == "sub"),
            "one level only: a folder is named, not walked"
        );
        let shown = reply["path"].as_str().unwrap_or_default().to_string();
        assert_eq!(shown, dir.to_string_lossy(), "the path is echoed back");

        let not_a_folder = dir.join("take.wav");
        assert!(
            matches!(list(&not_a_folder), Err(CallError::Bad(_))),
            "a file is refused, not listed"
        );
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// P5: a trigger written through `POST /outputs {op:"trigger"}` lands on
    /// the device and `GET /state` reports it per screen — the read the
    /// properties panel and the grid both use.
    #[test]
    fn posting_a_trigger_adds_it_to_the_device_and_state_reports_it() {
        let (mut paint, mut state, mut sim, author) = fixture();
        crate::call::outputs::set_trigger(
            state.sources.get_mut("probe").unwrap(),
            &json!({
                "id": "pedal",
                "target": "overlay-drop",
                "params": { "device": "probe", "layer": "l1", "loop": true },
            }),
        )
        .expect("a well-formed trigger");

        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        let reply = super::state(&mut ctx, &Params::default()).expect("GET /state");
        let probe = reply["outputs"]
            .as_array()
            .expect("outputs")
            .iter()
            .find(|o| o["id"] == "probe")
            .expect("the probe screen");
        let t = probe["triggers"]
            .as_array()
            .expect("triggers reported")
            .iter()
            .find(|t| t["id"] == "pedal")
            .expect("the pedal trigger");
        assert_eq!(t["target"], "overlay-drop");
        assert_eq!(t["params"]["layer"], "l1");
        assert_eq!(t["params"]["loop"], true);
    }

    /// Phase 4B: `GET /state` carries the Looper's `repeat` block, read off
    /// `State.looper` (regla 4/5 — the readout and any AI tool read the same
    /// status, never a second copy). Locking a manual loop moves `engaged`
    /// true and reports its length.
    #[test]
    fn state_reports_the_looper_repeat() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };

        let idle = super::state(&mut ctx, &Params::default()).expect("GET /state");
        assert_eq!(idle["repeat"]["mode"], "arm", "arm is the Looper's default");
        assert_eq!(idle["repeat"]["engaged"], false);
        assert_eq!(idle["repeat"]["length_beats"], 0);

        ctx.state.looper.lock(Some(4));
        let engaged = super::state(&mut ctx, &Params::default()).expect("GET /state");
        assert_eq!(engaged["repeat"]["engaged"], true);
        assert_eq!(engaged["repeat"]["length_beats"], 4);
        assert_eq!(engaged["repeat"]["mode"], "arm");
    }

    /// P5: firing an `overlay-<movement>` trigger no longer answers
    /// "deferred" — it reaches the shared play body (`overlay::play`), which
    /// here refuses because `probe` has no layer `l1` routed to it.
    #[test]
    fn firing_an_overlay_trigger_reaches_the_play_body() {
        let (mut paint, mut state, mut sim, author) = fixture();
        state
            .sources
            .get_mut("probe")
            .unwrap()
            .triggers
            .push(crate::trigger::Trigger {
                id: "pedal".into(),
                source: crate::trigger::TriggerSource::HttpPost {
                    route: "pedal".into(),
                },
                target: "overlay-drop".into(),
                params: json!({ "device": "probe", "layer": "l1", "loop": true }),
            });

        let params = Params {
            query: HashMap::new(),
            body: Vec::new(),
            tail: Some("pedal".into()),
        };
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        match trigger(&mut ctx, &params) {
            Err(CallError::Bad(m)) => {
                assert!(
                    m.contains("layer"),
                    "the play body should have been reached: {m}"
                );
            }
            other => panic!("expected the play body's refusal, got {other:?}"),
        }
    }

    /// `S.O.4a`: a `sequence-<id>` trigger reaches the shared run body
    /// (`super::sequences::run`) and actually arms the device — no longer
    /// the empty stub.
    #[test]
    fn firing_a_sequence_trigger_runs_it() {
        let (mut paint, mut state, mut sim, author) = fixture();
        state.collections.list.push(crate::layers::Collection {
            id: "cue-probe".into(),
            name: "probe".into(),
            layers: Vec::<crate::layers::Layer>::new(),
            composition: crate::layers::Composition::default(),
        });
        state.sequences.list.push(crate::sequence::Sequence {
            id: "opening".into(),
            name: "opening".into(),
            entries: vec![crate::sequence::SequenceEntry {
                collection: "cue-probe".into(),
                start: 0.0,
                end: 4.0,
            }],
        });
        state
            .sources
            .get_mut("probe")
            .unwrap()
            .triggers
            .push(crate::trigger::Trigger {
                id: "pedal".into(),
                source: crate::trigger::TriggerSource::HttpPost {
                    route: "pedal".into(),
                },
                target: "sequence-opening".into(),
                params: json!({ "device": "probe" }),
            });

        let params = Params {
            query: HashMap::new(),
            body: Vec::new(),
            tail: Some("pedal".into()),
        };
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        trigger(&mut ctx, &params).expect("the run body should have armed the device");
        // The entry spans `[0, 4)` from `started` (near-zero, a fresh
        // `State`) — `1.0` lands inside it; the cue test elsewhere uses
        // `100.0` only because a live cue's span is unbounded.
        assert_eq!(ctx.state.runs.active("probe", 1.0), Some("cue-probe"));
    }

    /// The learn arm is a process-wide static; each test that touches it
    /// starts from `None` so the suite is order-independent.
    fn reset_learn() {
        *crate::input::learn().lock().expect("learn lock") = None;
    }

    fn push_http_trigger(state: &mut crate::State, id: &str) {
        state
            .sources
            .get_mut("probe")
            .unwrap()
            .triggers
            .push(crate::trigger::Trigger::new_http(
                id.to_string(),
                "transport".to_string(),
            ));
    }

    /// `POST /trigger/<id> {op:"learn"}` with no source arms the trigger and
    /// `GET /state` reports `learning` — the read the grid's badge paints.
    #[test]
    fn learning_arms_without_a_source_and_get_state_reports_it() {
        reset_learn();
        let (mut paint, mut state, mut sim, author) = fixture();
        push_http_trigger(&mut state, "pedal");

        let params = Params {
            query: HashMap::new(),
            body: json!({ "op": "learn", "kind": "key" }).to_string().into_bytes(),
            tail: Some("pedal".into()),
        };
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        let reply = trigger(&mut ctx, &params).expect("the arm is set");
        assert_eq!(reply["learning"], "pedal");
        assert_eq!(reply["kind"], "key");

        let st = super::state(&mut ctx, &Params::default()).expect("GET /state");
        assert_eq!(st["learning"]["trigger"], "pedal");
        assert_eq!(st["learning"]["kind"], "key");

        reset_learn();
    }

    /// An armed learn that hears a concrete source writes it into exactly
    /// that trigger's `.source`, leaves every other trigger alone, and
    /// clears the arm — "learn writes the first source heard and nothing
    /// else" (plan point 5).
    #[test]
    fn learn_writes_the_first_source_heard_and_nothing_else() {
        reset_learn();
        let (mut paint, mut state, mut sim, author) = fixture();
        push_http_trigger(&mut state, "pedal");
        push_http_trigger(&mut state, "other");
        *crate::input::learn().lock().unwrap() = Some(crate::input::LearnArm {
            trigger: "pedal".to_string(),
            kind: "osc".to_string(),
        });

        let params = Params {
            query: HashMap::new(),
            body: json!({
                "op": "learn",
                "source": { "source": "osc", "addr": "/pad/1" },
            })
            .to_string()
            .into_bytes(),
            tail: Some("pedal".into()),
        };
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        let reply = trigger(&mut ctx, &params).expect("the source is written");
        assert_eq!(reply["learned"], "pedal");
        assert_eq!(reply["source"]["source"], "osc");
        assert_eq!(reply["source"]["addr"], "/pad/1");

        let triggers = &ctx.state.sources.get("probe").unwrap().triggers;
        let pedal = triggers.iter().find(|t| t.id == "pedal").unwrap();
        assert!(
            pedal.source.matches_osc("/pad/1"),
            "the pedal learned the address"
        );
        let other = triggers.iter().find(|t| t.id == "other").unwrap();
        assert!(
            other.source.route().is_some(),
            "only the learned trigger changed"
        );
        assert!(
            crate::input::learn().lock().unwrap().is_none(),
            "the arm is cleared by the write"
        );
    }

    /// Plan point 3: a source already used by another trigger is reassigned,
    /// and the reply carries `"warning":"reassigned"`.
    #[test]
    fn learning_a_source_another_trigger_uses_warns_and_reassigns() {
        reset_learn();
        let (mut paint, mut state, mut sim, author) = fixture();
        state
            .sources
            .get_mut("probe")
            .unwrap()
            .triggers
            .push(crate::trigger::Trigger::new_osc(
                "other".to_string(),
                "transport".to_string(),
                "/pad/1".to_string(),
            ));
        push_http_trigger(&mut state, "pedal");

        let params = Params {
            query: HashMap::new(),
            body: json!({
                "op": "learn",
                "source": { "source": "osc", "addr": "/pad/1" },
            })
            .to_string()
            .into_bytes(),
            tail: Some("pedal".into()),
        };
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        let reply = trigger(&mut ctx, &params).expect("the source is written");
        assert_eq!(reply["warning"], "reassigned");
    }

    /// A learn naming an id that is not here refuses with `Bad` and arms
    /// nothing.
    #[test]
    fn learning_an_unknown_trigger_is_refused() {
        reset_learn();
        let (mut paint, mut state, mut sim, author) = fixture();
        let params = Params {
            query: HashMap::new(),
            body: json!({ "op": "learn" }).to_string().into_bytes(),
            tail: Some("nope".into()),
        };
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        assert!(matches!(trigger(&mut ctx, &params), Err(CallError::Bad(_))));
        assert!(crate::input::learn().lock().unwrap().is_none());
    }
}

// ---------------------------------------------------------------------------
// plan "Audio input is a node": the listener pool's routes
// ---------------------------------------------------------------------------

/// `GET /listen/sources`: what an `audio.live`'s source combo offers — the
/// system default (`""`), the deliberate stand-in, every capture source
/// (PipeWire/Pulse sources and monitors) and every program playing right now
/// (`app:<serial>`). `id` is what goes in the node's `source`.
pub fn listen_sources(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let mut out = vec![
        json!({ "id": "", "label": "default input", "kind": "default" }),
        json!({ "id": crate::live::STAND_IN, "label": "stand-in (simulated)", "kind": "stand-in" }),
    ];
    for i in devices::inputs() {
        out.push(json!({ "id": i.id, "label": i.label, "kind": "input" }));
    }
    for a in devices::apps() {
        out.push(json!({ "id": a.id, "label": a.label, "kind": "app" }));
    }
    Ok(json!({ "sources": out }))
}

/// `GET /listen/status`: every bound `audio.live` and its listener's readout
/// `{ real, describe, bpm, dropped }`, the seed (`--listen`), and the pool's
/// open/error counts. What the chrome indicator, the node's readout and the
/// `listen_status` tool read.
pub fn listen_status(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(listen_status_json(ctx.sim))
}

/// [`listen_status`]'s body.
///
/// Each `nodes[]` entry carries a `tone` object — the Basic Pitch model's
/// state for that listener (the tone design notes §6 step 11): `loaded` is true only when
/// the worker actually loaded the model (the tone design notes D9), and `real` is the
/// same fact under the name an operator reads, because there is no stand-in
/// tone path to distinguish it from: a missing model or the `tone` feature
/// off both report `loaded: false, real: false`, and the colour mapping then
/// falls back to chroma (D8). An old 29-wide song has no tone columns, so it
/// reaches the same fallback through `tone_conf`. `latency_ms` is the
/// worker's last inference wall time, 0.0 before the first one or with no
/// model.
pub fn listen_status_json(pool: &crate::listeners::Listeners) -> Value {
    let nodes: Vec<Value> = pool
        .status()
        .into_iter()
        .map(|s| {
            json!({
                "graph": s.graph,
                "node": s.node,
                "source": s.want.as_pick(),
                "using": s.using.as_ref().map(|k| k.as_pick().to_string()),
                "short": s.using.as_ref().unwrap_or(&s.want).short(),
                "real": s.real,
                "describe": s.describe,
                "bpm": s.bpm,
                "dropped": s.dropped,
                "error": s.error,
                "tone": { "loaded": s.tone_loaded, "real": s.tone_loaded, "latency_ms": s.tone_latency_ms },
            })
        })
        .collect();
    json!({
        "seed": pool.seed().as_pick(),
        "clock": { "describe": pool.describe(), "real": pool.is_real() },
        "open": pool.open_count(),
        "opens": pool.opens,
        "errors": pool.errors,
        "nodes": nodes,
    })
}

/// `POST /listen/probe {source}` — the probe button: [`PROBE_SECONDS`] of
/// the source's flux (mean onset, 0..1), without changing what any node
/// reads. It also clears remembered failures, so a device plugged in since
/// gets tried again on the next frame.
pub fn listen_probe(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json {source}".into()));
    };
    let Some(pick) = body["source"].as_str() else {
        return Err(CallError::Bad("which source".into()));
    };
    let key = crate::listeners::SourceKey::from_pick(pick);
    ctx.sim.retry();
    match ctx.sim.probe(&key, PROBE_SECONDS) {
        Ok(flux) => Ok(json!({ "ok": true, "source": pick, "flux": flux, "seconds": PROBE_SECONDS })),
        Err(e) => Ok(json!({ "ok": false, "source": pick, "error": e })),
    }
}

/// How long the probe listens (plan Design 6: "2 s of flux").
pub const PROBE_SECONDS: f32 = 2.0;

/// The ingest queue for the transmit payload; the browser build has none.
fn ingest_jobs() -> Value {
    #[cfg(feature = "gpu")]
    {
        crate::call::author::ingest::jobs()
    }
    #[cfg(not(feature = "gpu"))]
    {
        json!([])
    }
}
