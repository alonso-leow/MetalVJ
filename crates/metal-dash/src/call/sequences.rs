//! §17.5's surface: `GET /sequences` and `POST /sequences` — the saved
//! collection-timelines and the runs they arm. `sequence.rs` is the
//! mechanism and its own module note explains why it reuses
//! `overlay::Overlays`'s shape; `state.sequences`/`state.runs` were already
//! loaded and consumed (`compile_device_plan` asks `Runs::active`), but no
//! route saved, listed or armed them — the "no route exposes it today"
//! finding `plan-sonnet.md` recorded for `crate::sequence`. This is that
//! wiring, at the same priority the lanes panel's own docs give the
//! resource: a device shows one collection at a time, so a second load or
//! cue replaces rather than stacks.
//!
//! **Regla 6, one level down.** The shell's lanes panel posts these same
//! routes through the door; nothing here knows about panels.

use serde_json::{json, Value};

use crate::call::{CallError, Ctx, Params};
use crate::sequence::{Run, Sequence, SequenceEntry};

/// `GET /sequences` — every saved sequence and every run in the air, both
/// as their own `describe` (the same shapes `GET /state`'s overlays report
/// uses, so a client never parses a second encoding).
pub fn sequences_get(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let time = ctx.state.time();
    Ok(json!({
        "sequences": ctx.state.sequences.describe(),
        "running": ctx.state.runs.report(time),
    }))
}

pub fn sequences_post(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    sequences_post_at(ctx, &body, &path())
}

/// Where the saved sequences live — `data/sequences.json`, the file
/// `Sequences::load` reads at boot.
fn path() -> std::path::PathBuf {
    crate::output::data_root().join("sequences.json")
}

/// The route's body, with the file a `save` writes as a parameter — the
/// same seam `call::settings`'s `post_at` keeps and the same shape
/// `grade::matrix_for(songs_root, …)` takes. The test points it at a
/// scratch file so a save never lands in the repo's `data/` (`R22.3`: the
/// `opening` fixture leaked there when a killed test skipped its guard's
/// `Drop`).
fn sequences_post_at(
    ctx: &mut Ctx,
    body: &Value,
    path: &std::path::Path,
) -> Result<Value, CallError> {
    let time = ctx.state.time();
    match body["op"].as_str().unwrap_or_default() {
        "save" => {
            let entries: Vec<SequenceEntry> = body["entries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|e| serde_json::from_value(e).ok())
                .collect();
            let id = body["id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| body["name"].as_str().unwrap_or_default())
                .to_string();
            let name = body["name"].as_str().unwrap_or_default().to_string();
            let seq = Sequence {
                id: id.clone(),
                name,
                entries,
            };
            // The load-time guard again (`sequence::check`'s own note): a
            // file can be hand-edited in between; and here the caller is a
            // socket, so the check is the save's first act, named on
            // refusal rather than "invalid".
            crate::sequence::check(&seq, &ctx.state.collections).map_err(CallError::Bad)?;
            ctx.state.sequences.list.retain(|s| s.id != seq.id);
            ctx.state.sequences.list.push(seq);
            save_at(path, &ctx.state.sequences.list)
                .map_err(|e| CallError::Internal(e.to_string()))?;
            Ok(json!({ "ok": true, "id": id, "sequences": ctx.state.sequences.list.len() }))
        }
        "run" => {
            let Some(id) = body["id"].as_str().map(str::to_string) else {
                return Err(CallError::Bad("which sequence".into()));
            };
            let Some(device) = body["device"].as_str().map(str::to_string) else {
                return Err(CallError::Bad("which device".into()));
            };
            run(ctx, &id, &device)
        }
        "cue" => {
            let Some(device) = body["device"].as_str().map(str::to_string) else {
                return Err(CallError::Bad("which device".into()));
            };
            let Some(collection) = body["collection"].as_str().map(str::to_string) else {
                return Err(CallError::Bad("which collection".into()));
            };
            // A live cue arms a single unbounded span; the collection it
            // names is checked here so a cue names its refusal, the same
            // way `run` does, instead of compiling to nothing on air.
            if ctx.state.collections.find(&collection).is_none() {
                return Err(CallError::NotFound);
            }
            ctx.state.runs.start(Run::live(device, collection, time));
            Ok(json!({ "ok": true }))
        }
        "stop" => {
            let Some(device) = body["device"].as_str().map(str::to_string) else {
                return Err(CallError::Bad("which device".into()));
            };
            let stopped = ctx.state.runs.stop(&device);
            Ok(json!({ "ok": true, "stopped": stopped }))
        }
        "stop-all" => {
            let n = ctx.state.runs.stop_all();
            Ok(json!({ "ok": true, "stopped": n }))
        }
        _ => Err(CallError::Bad(
            "op is save / run / cue / stop / stop-all".into(),
        )),
    }
}

/// The run half, shared by `POST /sequences {op:"run"}` and
/// `POST /trigger/<id>` on a `sequence-<id>` target (`S.O.4a`), the same
/// way `overlay::play` backs both `POST /overlay {op:"play"}` and an
/// `overlay-<movement>` trigger.
pub(crate) fn run(ctx: &mut Ctx, id: &str, device: &str) -> Result<Value, CallError> {
    let time = ctx.state.time();
    let Some(seq) = ctx.state.sequences.find(id) else {
        return Err(CallError::NotFound);
    };
    // `check` at the door, not only at save — the second check
    // `sequence.rs`'s module note asks for: a collection it names can be
    // deleted between the save and this arm.
    let seq = seq.clone();
    crate::sequence::check(&seq, &ctx.state.collections).map_err(CallError::Bad)?;
    let entries = seq.entries.clone();
    ctx.state.runs.start(Run {
        device: device.to_string(),
        sequence: Some(seq.id),
        entries,
        started: time,
    });
    Ok(json!({ "ok": true }))
}

/// Writes `list` to `path` — `Sequences::save`'s body with the file as a
/// parameter (`grade::matrix_for`'s shape), so the route can be pointed at
/// a scratch file instead of the repo's `data/`.
fn save_at(path: &std::path::Path, list: &[Sequence]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        metal_vfs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(list)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::atomic::write(path, format!("{text}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::dispatch;
    use crate::layers::{Collection, Composition, Layer};
    use std::collections::HashMap;

    /// A scratch `sequences.json` for the one test that saves — the route's
    /// `save_at` writes wherever it is pointed, so the repo's
    /// `data/sequences.json` is never touched (`R22.3`'s fix: the `opening`
    /// fixture leaked there when a killed test skipped its guard's `Drop`).
    /// `grade.rs`'s own `scratch` is the same shape.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("metal-dash-sequences-{name}-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        dir.join("sequences.json")
    }

    /// The same construction `every_route_dispatches` uses, with a
    /// collection named `"probe"` so `check`/`cue` have something to
    /// resolve against.
    fn fixture() -> crate::State {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir for Author::new()");
        let author = crate::Author::new();
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");
        let _ = author;

        let mut paint = crate::paint::painter();
        let mut state = crate::State::new(paint.as_ref());
        state
            .outputs
            .list
            .push(crate::output::Output::new("probe", "Probe", 64, 64));
        state
            .sources
            .entry("probe".to_string())
            .or_insert_with(crate::layers::Device::default);
        state.collections.list.push(Collection {
            id: "cue-probe".into(),
            name: "probe".into(),
            layers: Vec::<Layer>::new(),
            composition: Composition::default(),
        });
        state
    }

    fn post(
        state: &mut crate::State,
        body: Value,
        path: &std::path::Path,
    ) -> Result<Value, crate::call::CallError> {
        let mut paint = crate::paint::painter();
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir for Author::new()");
        let author = crate::Author::new();
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut ctx = Ctx {
            state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        sequences_post_at(&mut ctx, &body, path)
    }

    /// A sequence naming a real collection saves into the scratch file,
    /// reads back through the GET, and runs on a device; `GET /sequences`
    /// reports the run.
    #[test]
    fn a_valid_sequence_saves_runs_and_reports() {
        let path = scratch("save");
        let mut state = fixture();
        let reply = post(
            &mut state,
            json!({ "op": "save", "name": "opening",
                    "entries": [{ "collection": "cue-probe", "start": 0.0, "end": 4.0 }] }),
            &path,
        )
        .expect("a valid sequence saves");
        assert_eq!(reply["id"], json!("opening"));
        let on_disk = metal_vfs::read_to_string(&path).expect("the save wrote the scratch file");
        assert!(
            on_disk.contains("opening"),
            "the scratch file holds the save: {on_disk}"
        );

        let reply = post(
            &mut state,
            json!({ "op": "run", "id": "opening", "device": "probe" }),
            &path,
        )
        .expect("runs");
        assert_eq!(reply["ok"], json!(true));

        let mut paint = crate::paint::painter();
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir for Author::new()");
        let author = crate::Author::new();
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        let got = sequences_get(
            &mut ctx,
            &Params {
                query: HashMap::new(),
                body: Vec::new(),
                tail: None,
            },
        )
        .expect("get");
        let running = got["running"].as_array().expect("runs reported");
        assert_eq!(running.len(), 1);
        assert_eq!(running[0]["sequence"], json!("opening"));
        assert_eq!(running[0]["collection"], json!("cue-probe"));
        let _ = dispatch;
    }

    /// A sequence naming a collection that does not exist is refused at
    /// save, by naming it — not "invalid sequence".
    #[test]
    fn a_sequence_naming_an_unknown_collection_is_refused_by_name() {
        let path = scratch("refused");
        let mut state = fixture();
        let err = post(
            &mut state,
            json!({ "op": "save", "name": "ghost",
                    "entries": [{ "collection": "no-such", "start": 0.0, "end": 1.0 }] }),
            &path,
        )
        .expect_err("refused");
        match err {
            CallError::Bad(msg) => assert!(msg.contains("no-such"), "{msg}"),
            other => panic!("expected Bad, got {other:?}"),
        }
    }

    /// Running a sequence that does not exist is a 404, not a Bad: the
    /// request is well-formed, the name is wrong.
    #[test]
    fn running_an_unknown_sequence_is_not_found() {
        let path = scratch("unknown");
        let mut state = fixture();
        let err = post(
            &mut state,
            json!({ "op": "run", "id": "nope", "device": "probe" }),
            &path,
        )
        .expect_err("refused");
        assert!(matches!(err, CallError::NotFound), "{err:?}");
    }

    /// A live cue arms an unbounded span; `stop` takes it off.
    #[test]
    fn a_live_cue_runs_until_stopped() {
        let path = scratch("cue");
        let mut state = fixture();
        post(
            &mut state,
            json!({ "op": "cue", "device": "probe", "collection": "cue-probe" }),
            &path,
        )
        .expect("cued");
        assert_eq!(
            state.runs.active("probe", 100.0),
            Some("cue-probe"),
            "still on after 100s"
        );
        let reply = post(
            &mut state,
            json!({ "op": "stop", "device": "probe" }),
            &path,
        )
        .expect("stopped");
        assert_eq!(reply["stopped"], json!(true));
        assert_eq!(state.runs.active("probe", 100.0), None);
    }
}
