//! `export.video`'s routes (plan §9.1 E1): start a stepped offline export as a
//! background job, report on it, cancel it.
//!
//! `POST /export/video {device,node,range,fps,codec,bitrate,audio,path}` starts
//! the job (`audio` is `"mix"`, `"file:<path>"` or null — plan §9.1 E3);
//! `GET /export` reports `{state,frames_done,frames_total,fraction,eta,error,
//! path}`; `POST /export {op:"cancel"}` cancels it. The job runs on its
//! own thread with its own headless renderer
//! (`metal_render::headless::Exporter`), so the app's own paint loop is never
//! blocked by an export — the plan's "a real, stepped, resumable video export"
//! with a poll-and-cancel surface (`Exporter::step`) the background job drives.
//!
//! The frame walking is `metal_perform::export::Offline` (the `export.video`
//! Offline executor): it reads the device's graph, takes the generator node
//! the request names (or the graph's first authored one), and produces one
//! [`metal_ingest::export::FrameJob`] per frame of `range`. The renderer here
//! draws each one with `Renderer::solo_chain` and feeds the `Encoder`.
//!
//! Manual §9.1: presets load from `data/export-presets.json` (a serde list
//! with a default preset) — the first one supplies every field the request
//! leaves out (frame size, rate, codec, bitrate).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use serde_json::{json, Value};

use crate::call::{CallError, Ctx, Params};
use metal_ingest::export::{default_presets, first_preset, parse_presets, ExportPreset};
use metal_render::headless::{ExportSettings, Exporter, Report, Step};
use metal_perform::export::{AudioFile, Offline, EXPORT_CHANNELS, EXPORT_RATE};

/// The request's `audio` choice (plan §9.1 E3).
#[derive(Debug, PartialEq)]
enum AudioChoice {
    /// `null`/absent: the E1 video-only export.
    None,
    /// `"mix"`: sum every `audio.file` node in the graph.
    Mix,
    /// `"file:<path>"`: one named song/audio file.
    File(PathBuf),
}

impl AudioChoice {
    /// Reads the route's `audio` field: `null`, `"mix"` or `"file:<path>"`.
    /// Anything else is a 400 rather than a silently dropped track.
    fn parse(v: &Value) -> Result<AudioChoice, CallError> {
        let bad = || CallError::Bad("audio must be \"mix\", \"file:<path>\" or null".into());
        match v {
            Value::Null => Ok(AudioChoice::None),
            Value::String(s) if s == "mix" => Ok(AudioChoice::Mix),
            Value::String(s) => match s.strip_prefix("file:") {
                Some(p) if !p.is_empty() => Ok(AudioChoice::File(PathBuf::from(p))),
                _ => Err(bad()),
            },
            _ => Err(bad()),
        }
    }
}

/// The `file` a song config names, resolved against the corpus when it is a
/// bare name or a project-relative `data/audio/...` path. An absolute path or
/// anything that already exists is used as it stands. Mirrors how the library
/// panel reads the same key.
fn resolve_song(raw: &str) -> PathBuf {
    let p = PathBuf::from(raw);
    if p.is_absolute() || p.exists() {
        return p;
    }
    let name = p.file_name().map(PathBuf::from).unwrap_or_else(|| p.clone());
    let candidate = crate::output::data_root().join("audio").join(&name);
    if candidate.exists() {
        candidate
    } else {
        p
    }
}

/// Every `audio.file` node's configured song, resolved. `"mix"` sums these.
fn graph_audio_files(graph: &metal_graph::Graph) -> Vec<PathBuf> {
    graph
        .nodes
        .iter()
        .filter(|n| n.kind == "audio.file")
        .filter_map(|n| n.config.get("file").and_then(Value::as_str))
        .map(resolve_song)
        .collect()
}

/// The audio track for frames `[start, end)` at `fps`, written to a temp WAV
/// the encoder muxes. The path is the caller's to remove once the export is
/// over — `run` does.
fn range_wav(audio: &AudioFile, start: u64, end: u64, fps: f64) -> Result<PathBuf, CallError> {
    let track = audio.range(start, end, fps, EXPORT_RATE, EXPORT_CHANNELS);
    if track.is_empty() {
        return Err(CallError::Bad("the audio range is empty".into()));
    }
    let path = std::env::temp_dir().join(format!("mvj-export-audio-{}-{}.wav", std::process::id(), start));
    metal_ingest::audio::write_wav(&path, &track).map_err(CallError::Internal)?;
    Ok(path)
}

/// The running (or last-finished) export job. One at a time: a second export
/// while one runs is a 409, not a silently dropped job.
struct Shared {
    cancel: AtomicBool,
    started: Instant,
    state: Mutex<State>,
}

#[derive(Clone)]
struct State {
    /// `idle` | `running` | `done` | `cancelled` | `error`.
    state: &'static str,
    done: u64,
    total: u64,
    error: Option<String>,
    path: String,
}

impl Default for State {
    fn default() -> State {
        State { state: "idle", done: 0, total: 0, error: None, path: String::new() }
    }
}

impl Shared {
    fn new(total: u64, path: String) -> Shared {
        Shared {
            cancel: AtomicBool::new(false),
            started: Instant::now(),
            state: Mutex::new(State { state: "running", done: 0, total, error: None, path }),
        }
    }

    fn set(&self, f: impl FnOnce(&mut State)) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut s);
    }

    /// One snapshot as `GET /export` reports it, fraction and ETA computed
    /// here (the only place they can be: they depend on the clock, and the
    /// clock is not a field the job should keep in a lock).
    fn snapshot(&self) -> Value {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let fraction = if s.total == 0 { 0.0 } else { s.done as f64 / s.total as f64 };
        let eta = if s.state == "running" && s.done > 0 && s.total > s.done {
            let elapsed = self.started.elapsed().as_secs_f64();
            Some(elapsed / s.done as f64 * (s.total - s.done) as f64)
        } else {
            None
        };
        json!({
            "state": s.state,
            "frames_done": s.done,
            "frames_total": s.total,
            "fraction": fraction,
            "eta": eta,
            "error": s.error,
            "path": s.path,
        })
    }
}

fn slot() -> &'static Mutex<Option<Arc<Shared>>> {
    static S: OnceLock<Mutex<Option<Arc<Shared>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// `data/export-presets.json`: the operator's list, or the built-in defaults
/// when the file is missing, unreadable or empty (an export always has a
/// preset to fall back to).
pub fn load_presets() -> Vec<ExportPreset> {
    load_presets_at(&crate::output::data_root())
}

/// [`load_presets`] under `root`.
pub fn load_presets_at(root: &std::path::Path) -> Vec<ExportPreset> {
    let path = root.join("export-presets.json");
    match metal_vfs::read(&path) {
        Ok(bytes) => match std::str::from_utf8(&bytes).ok().and_then(|t| parse_presets(t).ok()) {
            Some(p) => p,
            None => default_presets(),
        },
        Err(_) => default_presets(),
    }
}

/// `POST /export/video`: validate, then start the job on its own thread.
pub fn video(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("expected json".into()))?;
    if running() {
        return Err(CallError::Conflict("an export is already running".into()));
    }
    start_job(&body, None)
}

/// Whether a job is running now.
fn running() -> bool {
    slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|s| s.state.lock().unwrap_or_else(|e| e.into_inner()).state == "running")
}

/// Validates `body` and starts its job on its own thread; `id` is the queue
/// entry it came from, if any.
fn start_job(body: &Value, id: Option<String>) -> Result<Value, CallError> {

    let device = body["device"].as_str().ok_or_else(|| CallError::Bad("expected device".into()))?;
    let path = body["path"].as_str().ok_or_else(|| CallError::Bad("expected path".into()))?;

    let presets = load_presets();
    let preset = first_preset(&presets).cloned().unwrap_or_else(|| default_presets()[0].clone());

    // `range` is `[start, end)` in frames; both required — "export everything"
    // would need a duration the graph does not carry.
    let range = body["range"].as_array().ok_or_else(|| CallError::Bad("expected range [start,end]".into()))?;
    let as_u64 = |v: Option<&Value>| v.and_then(Value::as_u64);
    let (start, end) = match (as_u64(range.first()), as_u64(range.get(1))) {
        (Some(a), Some(b)) if b > a => (a, b),
        _ => return Err(CallError::Bad("range must be [start,end] with end > start".into())),
    };

    let fps = body["fps"].as_f64().filter(|f| *f > 0.0).unwrap_or(preset.fps);
    let codec = body["codec"].as_str().filter(|c| !c.is_empty()).unwrap_or(&preset.codec).to_string();
    let bitrate_kbps = body["bitrate"].as_u64().map(|v| v as u32).or(preset.bitrate_kbps);

    let root = crate::output::data_root();
    let graph = metal_perform::engine::device_graph_at(&root, device)
        .or_else(|| crate::load_graph(device).ok())
        .ok_or_else(|| CallError::Bad(format!("no graph for device {device:?}")))?;
    let node = match body["node"].as_str() {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => metal_perform::engine::authored_generators(&graph)
            .into_iter()
            .next()
            .ok_or_else(|| CallError::Bad("the graph has no generator to export".into()))?,
    };

    // E3: the optional audio track. `"mix"` needs the graph to have a source;
    // `"file:<path>"` reads exactly what it names. Either way the samples are
    // sliced to the export range and written to a temp WAV the encoder muxes.
    let audio = match AudioChoice::parse(&body["audio"])? {
        AudioChoice::None => None,
        AudioChoice::Mix => {
            let paths = graph_audio_files(&graph);
            if paths.is_empty() {
                return Err(CallError::Bad("audio:\"mix\" but the graph has no audio.file source".into()));
            }
            let af = AudioFile::mix(&paths).map_err(CallError::Bad)?;
            Some(range_wav(&af, start, end, fps)?)
        }
        AudioChoice::File(p) => {
            let af = AudioFile::load(&p).map_err(CallError::Bad)?;
            Some(range_wav(&af, start, end, fps)?)
        }
    };

    let settings = ExportSettings {
        path: path.into(),
        width: preset.width.max(1),
        height: preset.height.max(1),
        fps,
        codec,
        bitrate_kbps,
        audio,
        batch: metal_render::headless::EXPORT_BATCH,
    };

    let shared = Arc::new(Shared::new(end - start, path.to_string()));
    *slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(shared.clone());
    *current().lock().unwrap_or_else(|e| e.into_inner()) = id;

    std::thread::spawn(move || {
        run(graph, node, start, end, fps, settings, shared);
        // One at a time: the next queued job starts when this one is over,
        // whether it finished, was cancelled or failed.
        advance(&crate::output::data_root(), &start_job);
    });
    Ok(slot().lock().unwrap_or_else(|e| e.into_inner()).as_ref().expect("just set").snapshot())
}

/// The worker: build the executor, then poll the stepped exporter until it is
/// done, cancelled or fails, reporting progress into `shared`.
fn run(
    graph: metal_graph::Graph,
    node: String,
    start: u64,
    end: u64,
    fps: f64,
    settings: ExportSettings,
    shared: Arc<Shared>,
) {
    // The temp WAV the encoder was handed, removed once the export is over on
    // every path (done, cancelled or failed) so the export leaves no litter.
    let audio_path = settings.audio.clone();
    let painter = metal_perform::stand::Stand::new();
    let offline = match Offline::new(&graph, &node, start, end, fps, &painter) {
        Ok(o) => o,
        Err(e) => {
            cleanup_audio(&audio_path);
            return shared.set(|s| { s.state = "error"; s.error = Some(e); });
        }
    };
    let mut exporter = match Exporter::new(Box::new(offline), settings) {
        Ok(e) => e,
        Err(e) => {
            cleanup_audio(&audio_path);
            return shared.set(|s| { s.state = "error"; s.error = Some(e.to_string()); });
        }
    };
    match drive(&mut exporter, &shared) {
        Ok(report) => shared.set(|s| {
            s.state = "done";
            s.done = report.frames;
            s.total = report.frames;
            s.path = report.path.display().to_string();
        }),
        Err(metal_render::headless::ExportError::Cancelled) => {
            let path = shared.state.lock().unwrap_or_else(|e| e.into_inner()).path.clone();
            let _ = metal_vfs::remove_file(&path);
            shared.set(|s| s.state = "cancelled");
        }
        Err(e) => shared.set(|s| { s.state = "error"; s.error = Some(e.to_string()); }),
    }
    cleanup_audio(&audio_path);
}

/// Removes the temp muxed WAV, if there was one.
fn cleanup_audio(path: &Option<PathBuf>) {
    if let Some(p) = path {
        let _ = metal_vfs::remove_file(p);
    }
}

/// Polls `exporter` until it finishes, publishing progress after each group.
/// Split out so a test can drive a real export without a thread.
fn drive(exporter: &mut Exporter, shared: &Shared) -> Result<Report, metal_render::headless::ExportError> {
    shared.set(|s| s.total = exporter.total());
    loop {
        match exporter.step(&shared.cancel)? {
            Step::Progress(done) => shared.set(|s| { s.done = done; s.state = "running"; }),
            Step::Pending => std::thread::sleep(std::time::Duration::from_millis(5)),
            Step::Done(report) => {
                shared.set(|s| { s.done = report.frames; s.state = "done"; });
                return Ok(report);
            }
        }
    }
}

/// `GET /export`: the last report, or an idle status before any export ran.
pub fn status(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let guard = slot().lock().unwrap_or_else(|e| e.into_inner());
    Ok(match guard.as_ref() {
        Some(shared) => shared.snapshot(),
        None => {
            let idle = State::default();
            json!({
                "state": idle.state,
                "frames_done": idle.done,
                "frames_total": idle.total,
                "fraction": 0.0,
                "eta": Value::Null,
                "error": Value::Null,
                "path": idle.path,
            })
        }
    })
}

/// `POST /export {op:"cancel"}`: ask the running job to stop. The worker
/// removes the partial file and reports `cancelled`.
pub fn control(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("expected json".into()))?;
    match body["op"].as_str() {
        Some("cancel") => {
            let guard = slot().lock().unwrap_or_else(|e| e.into_inner());
            let Some(shared) = guard.as_ref() else {
                return Err(CallError::Bad("no export to cancel".into()));
            };
            shared.cancel.store(true, Ordering::Relaxed);
            Ok(shared.snapshot())
        }
        _ => Err(CallError::Bad("op must be cancel".into())),
    }
}

// -- the queue ----------------------------------------------------------------

/// One waiting export: the `POST /export/video` body it will run with.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct QueueItem {
    pub id: String,
    pub body: Value,
}

/// The queue file's lock and the running job's queue id.
fn queue_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

fn current() -> &'static Mutex<Option<String>> {
    static C: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

fn presets_path(root: &std::path::Path) -> PathBuf {
    root.join("export-presets.json")
}

/// The queue in `data/export-presets.json`: the file is either the bare preset
/// list (as it always was) or `{"presets": [...], "queue": [...]}`.
pub fn read_queue(root: &std::path::Path) -> Vec<QueueItem> {
    let Ok(text) = metal_vfs::read_to_string(presets_path(root)) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    serde_json::from_value(v["queue"].clone()).unwrap_or_default()
}

/// Writes `queue` back, keeping the presets as they were (the default list when
/// the file did not exist yet). The file becomes the object form.
pub fn write_queue(root: &std::path::Path, queue: &[QueueItem]) -> Result<(), CallError> {
    let presets = match metal_vfs::read_to_string(presets_path(root)).ok().and_then(|t| parse_presets(&t).ok()) {
        Some(p) => p,
        None => default_presets(),
    };
    let text = serde_json::to_string_pretty(&json!({ "presets": presets, "queue": queue }))
        .map_err(|e| CallError::Internal(e.to_string()))?;
    metal_vfs::create_dir_all(root).map_err(|e| CallError::Internal(e.to_string()))?;
    metal_vfs::write(presets_path(root), text).map_err(|e| CallError::Internal(e.to_string()))
}

fn next_id() -> String {
    use std::sync::atomic::AtomicU64;
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    format!("q{t:x}-{}", N.fetch_add(1, Ordering::Relaxed))
}

/// Appends a job; the request must carry what a run needs.
pub fn enqueue(root: &std::path::Path, body: &Value) -> Result<QueueItem, CallError> {
    for key in ["device", "path"] {
        if body[key].as_str().is_none() {
            return Err(CallError::Bad(format!("expected {key}")));
        }
    }
    if body["range"].as_array().map(|r| r.len()) != Some(2) {
        return Err(CallError::Bad("expected range [start,end]".into()));
    }
    let _g = queue_lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut queue = read_queue(root);
    let item = QueueItem { id: next_id(), body: body.clone() };
    queue.push(item.clone());
    write_queue(root, &queue)?;
    Ok(item)
}

/// Removes a waiting job. `false` when `id` is not waiting.
pub fn remove(root: &std::path::Path, id: &str) -> Result<bool, CallError> {
    let _g = queue_lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut queue = read_queue(root);
    let before = queue.len();
    queue.retain(|q| q.id != id);
    let gone = queue.len() != before;
    if gone {
        write_queue(root, &queue)?;
    }
    Ok(gone)
}

/// Moves a waiting job to position `to` (clamped), 0 being next.
pub fn reorder(root: &std::path::Path, id: &str, to: usize) -> Result<bool, CallError> {
    let _g = queue_lock().lock().unwrap_or_else(|e| e.into_inner());
    let mut queue = read_queue(root);
    let Some(at) = queue.iter().position(|q| q.id == id) else { return Ok(false) };
    let item = queue.remove(at);
    queue.insert(to.min(queue.len()), item);
    write_queue(root, &queue)?;
    Ok(true)
}

/// Starts the head of the queue with `start` when nothing runs. A job whose
/// request no longer validates is dropped and the next one is tried; returns
/// the id that started, if any.
pub fn advance(root: &std::path::Path, start: &dyn Fn(&Value, Option<String>) -> Result<Value, CallError>) -> Option<String> {
    if running() {
        return None;
    }
    loop {
        let item = {
            let _g = queue_lock().lock().unwrap_or_else(|e| e.into_inner());
            let mut queue = read_queue(root);
            if queue.is_empty() {
                *current().lock().unwrap_or_else(|e| e.into_inner()) = None;
                return None;
            }
            let item = queue.remove(0);
            write_queue(root, &queue).ok()?;
            item
        };
        match start(&item.body, Some(item.id.clone())) {
            Ok(_) => return Some(item.id),
            Err(e) => eprintln!("[export] queued job {} dropped: {e:?}", item.id),
        }
    }
}

/// `GET /export/queue`: the waiting jobs and the running one's id.
pub fn queue_list(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(listing(&crate::output::data_root()))
}

fn listing(root: &std::path::Path) -> Value {
    json!({
        "queue": read_queue(root),
        "running": current().lock().unwrap_or_else(|e| e.into_inner()).clone(),
        "job": slot().lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.snapshot()),
    })
}

/// `POST /export/queue {op}`: `add` (the default; the body is a
/// `/export/video` request), `cancel {id}` (a waiting job leaves the queue; the
/// running one is stopped), `move {id, to}` and `list`.
pub fn queue_control(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("expected json".into()))?;
    let root = crate::output::data_root();
    let id = || body["id"].as_str().ok_or_else(|| CallError::Bad("expected id".into()));
    match body["op"].as_str().unwrap_or("add") {
        "add" => {
            let item = enqueue(&root, &body)?;
            advance(&root, &start_job);
            let mut out = listing(&root);
            out["id"] = json!(item.id);
            Ok(out)
        }
        "cancel" => {
            let id = id()?;
            if current().lock().unwrap_or_else(|e| e.into_inner()).as_deref() == Some(id) {
                if let Some(shared) = slot().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                    shared.cancel.store(true, Ordering::Relaxed);
                }
            } else if !remove(&root, id)? {
                return Err(CallError::NotFound);
            }
            Ok(listing(&root))
        }
        "move" => {
            let to = body["to"].as_u64().ok_or_else(|| CallError::Bad("expected to".into()))? as usize;
            if !reorder(&root, id()?, to)? {
                return Err(CallError::NotFound);
            }
            Ok(listing(&root))
        }
        "list" => Ok(listing(&root)),
        _ => Err(CallError::Bad("op must be add, cancel, move or list".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_graph::{Domain, Endpoint};

    /// The shipped presets file is a valid, non-empty list with a default,
    /// and it round-trips through the same parser the route uses.
    #[test]
    fn the_shipped_presets_file_round_trips() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/export-presets.json");
        let text = std::fs::read_to_string(&path).expect("data/export-presets.json ships");
        let presets = parse_presets(&text).expect("it parses");
        assert!(!presets.is_empty());
        assert_eq!(first_preset(&presets).unwrap().codec, "h264");
        // And the built-in defaults produce the same list shape.
        assert_eq!(parse_presets(&serde_json::to_string(&presets).unwrap()).unwrap(), presets);
        assert_eq!(presets, default_presets());
    }

    fn offline_graph() -> (metal_graph::Graph, String) {
        let mut g = metal_graph::Graph::new("exp", "Export", Domain::Offline);
        let _audio = g.add_node("audio", "audio.file", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [2.0, 0.0]);
        let out = g.add_node("export", "export.video", [4.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        (g, gen)
    }

    /// The job machinery end to end, GPU and ffmpeg both: a tiny range is
    /// rendered by the real exporter, lands a real file and reports progress
    /// climbing to the total. No disk `data/`: the graph is built in memory.
    #[test]
    fn an_export_job_renders_a_tiny_range_and_reports_progress() {
        let (graph, node) = offline_graph();
        let dir = std::env::temp_dir().join(format!("fw-dash-export-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.mp4");

        let settings = ExportSettings {
            path: path.clone(),
            width: 320,
            height: 180,
            fps: 8.0,
            codec: "h264".into(),
            bitrate_kbps: None,
            audio: None,
            batch: 2,
        };
        let painter = metal_perform::stand::Stand::new();
        let offline = Offline::new(&graph, &node, 0, 5, 8.0, &painter).expect("a generator");
        let mut exporter = match Exporter::new(Box::new(offline), settings) {
            Ok(e) => e,
            Err(metal_render::headless::ExportError::Encode(metal_ingest::video::VideoError::Spawn(_))) => {
                eprintln!("skipping: ffmpeg unavailable");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        let shared = Shared::new(exporter.total(), path.display().to_string());
        let report = drive(&mut exporter, &shared).expect("finishes");
        assert_eq!(report.frames, 5);
        assert!(report.bytes > 0);

        let snapshot = shared.snapshot();
        assert_eq!(snapshot["state"], "done");
        assert_eq!(snapshot["frames_done"], 5);
        assert_eq!(snapshot["frames_total"], 5);
        assert_eq!(snapshot["fraction"], 1.0);
        assert!(std::path::Path::new(&path).exists());

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// E3: the route's `audio` field reads as the three forms the plan names,
    /// and anything else is a 400 rather than a silently dropped track.
    #[test]
    fn the_export_audio_choice_parses_its_three_forms() {
        assert_eq!(AudioChoice::parse(&Value::Null).unwrap(), AudioChoice::None);
        assert_eq!(AudioChoice::parse(&json!("mix")).unwrap(), AudioChoice::Mix);
        assert_eq!(
            AudioChoice::parse(&json!("file:data/audio/x.wav")).unwrap(),
            AudioChoice::File(PathBuf::from("data/audio/x.wav"))
        );
        assert!(AudioChoice::parse(&json!("nope")).is_err());
        assert!(AudioChoice::parse(&json!("file:")).is_err());
    }

    /// E3 end to end through the worker: a named song is decoded, sliced to the
    /// range, muxed, and the temp WAV the encoder was handed is cleaned up. The
    /// container carries both streams (ffprobe), and the video-only export is
    /// untouched (its byte-identity is E1's own test).
    #[test]
    fn an_export_job_muxes_a_named_audio_file() {
        let (graph, node) = offline_graph();
        let dir = std::env::temp_dir().join(format!("fw-dash-mux-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.mp4");

        // A 0.5 s 440 Hz tone at the export target, as a WAV on disk.
        let song = dir.join("song.wav");
        let mut samples = Vec::new();
        for i in 0..EXPORT_RATE as usize / 2 {
            let v = 0.3 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / EXPORT_RATE as f64).sin() as f32;
            for _ in 0..EXPORT_CHANNELS {
                samples.push(v);
            }
        }
        let track = metal_ingest::audio::AudioTrack { rate: EXPORT_RATE, channels: EXPORT_CHANNELS, samples };
        metal_ingest::audio::write_wav(&song, &track).unwrap();
        let af = AudioFile::load(&song).unwrap();
        let wav = range_wav(&af, 0, 5, 8.0).unwrap();

        let settings = ExportSettings {
            path: path.clone(),
            width: 320,
            height: 180,
            fps: 8.0,
            codec: "h264".into(),
            bitrate_kbps: None,
            audio: Some(wav.clone()),
            batch: 2,
        };
        let shared = Arc::new(Shared::new(5, path.display().to_string()));
        run(graph, node, 0, 5, 8.0, settings, shared.clone());

        let snapshot = shared.snapshot();
        if snapshot["state"] == "error" {
            eprintln!("skipping: ffmpeg unavailable ({})", snapshot["error"]);
            let _ = metal_vfs::remove_dir_all(&dir);
            return;
        }
        assert_eq!(snapshot["state"], "done", "{snapshot}");

        let out = metal_schema::platform::Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0"])
            .arg(&path)
            .output()
            .expect("ffprobe runs");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("video"), "no video stream: {text:?}");
        assert!(text.contains("audio"), "no audio stream: {text:?}");
        assert!(!wav.exists(), "the temp WAV the encoder was handed is cleaned up");

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    fn job(path: &str) -> Value {
        json!({ "device": "out-1", "range": [0, 30], "path": path })
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fw-export-queue-{tag}-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        dir
    }

    /// FIFO, reorder and cancel on the file in `data/export-presets.json`,
    /// which keeps its presets (a bare list from before is read and kept).
    #[test]
    fn the_queue_is_fifo_reorders_and_cancels_inside_the_presets_file() {
        let dir = temp("fifo");
        std::fs::write(dir.join("export-presets.json"), serde_json::to_string(&default_presets()).unwrap()).unwrap();
        let a = enqueue(&dir, &job("a.mp4")).unwrap();
        let b = enqueue(&dir, &job("b.mp4")).unwrap();
        let c = enqueue(&dir, &job("c.mp4")).unwrap();
        let ids = |d: &PathBuf| read_queue(d).into_iter().map(|q| q.id).collect::<Vec<_>>();
        assert_eq!(ids(&dir), [a.id.clone(), b.id.clone(), c.id.clone()]);
        assert!(reorder(&dir, &c.id, 0).unwrap());
        assert_eq!(ids(&dir), [c.id.clone(), a.id.clone(), b.id.clone()]);
        assert!(remove(&dir, &a.id).unwrap());
        assert!(!remove(&dir, &a.id).unwrap(), "a second cancel finds nothing");
        assert_eq!(ids(&dir), [c.id.clone(), b.id.clone()]);
        assert_eq!(load_presets_at(&dir), default_presets(), "the presets survive the queue");
        assert!(enqueue(&dir, &json!({ "device": "x" })).is_err(), "a request without a path or range is refused");
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// `advance` starts the head, one at a time; a job that no longer validates
    /// is dropped and the next one runs; it is idempotent on an empty queue.
    #[test]
    fn advance_runs_the_head_and_skips_a_job_that_will_not_start() {
        let dir = temp("advance");
        let bad = enqueue(&dir, &job("bad.mp4")).unwrap();
        let good = enqueue(&dir, &job("good.mp4")).unwrap();
        let after = enqueue(&dir, &job("after.mp4")).unwrap();
        let started = std::cell::RefCell::new(Vec::new());
        let fake = |b: &Value, id: Option<String>| -> Result<Value, CallError> {
            if b["path"] == "bad.mp4" {
                return Err(CallError::Bad("no graph".into()));
            }
            started.borrow_mut().push(id.unwrap());
            Ok(json!({}))
        };
        assert_eq!(advance(&dir, &fake), Some(good.id.clone()));
        assert_eq!(*started.borrow(), [good.id.clone()]);
        let left: Vec<String> = read_queue(&dir).into_iter().map(|q| q.id).collect();
        assert_eq!(left, [after.id.clone()], "{} was dropped, {} started", bad.id, good.id);
        assert_eq!(advance(&dir, &fake), Some(after.id));
        assert_eq!(advance(&dir, &fake), None);
        let _ = metal_vfs::remove_dir_all(&dir);
    }
}
