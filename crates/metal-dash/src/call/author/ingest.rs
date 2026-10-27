//! The ingest pipeline: one background HAP-proxy build per dropped clip, with
//! a phase the shell can read.
//!
//! A clip pasted onto the canvas or dropped on a lane used to show nothing
//! until the editor happened to preview it and the decoder lazily made its
//! proxy — and a Resolume DXV alpha export (YCGCO) never got one at all,
//! because swscale refuses it. This is the enqueue side: the node route and the
//! shell both call [`enqueue`], one job runs per source path, and [`jobs`] is
//! what `/state` carries so the library can say what is happening.
//!
//! Progress is measured, not guessed: ffmpeg's own `-progress` frame count
//! drives `converting` from 0.05 to 0.95, and the phases on either side
//! (`preparing` before, `ready`/`failed` after) are the seam.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use crate::call::{CallError, Params};

/// How one clip's proxy build is going.
#[derive(Clone, Debug)]
pub struct Job {
    pub path: String,
    /// One word for the phase: `preparing`, `decoding`, `encoding`, `ready`.
    pub step: String,
    /// 0..1: ffmpeg's own frame count across `converting` (0.05 → 0.95),
    /// with the phases before and after filling the ends.
    pub progress: f32,
    pub done: bool,
    /// Why it could not be built, in words an operator can act on.
    pub error: Option<String>,
}

/// One job per source path. Keyed by the path as every caller spells it, which
/// is how the proxy slug and the scenes book key it too.
static JOBS: OnceLock<Mutex<HashMap<String, Job>>> = OnceLock::new();

fn jobs_map() -> &'static Mutex<HashMap<String, Job>> {
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether `path`'s proxy under `root` exists and is not older than the source:
/// both the `.mov` and its `.json` index, the older of the two at least as new
/// as the clip's own mtime.
pub(crate) fn proxy_is_fresh(root: &Path, path: &str) -> bool {
    let slug = metal_reel::scenes::slug(path);
    let movie = root.join(format!("{slug}.mov"));
    let index = root.join(format!("{slug}.json"));
    let modified = |p: &Path| metal_vfs::metadata(p).and_then(|m| m.modified()).ok();
    let (Some(src), Some(mov), Some(idx)) = (modified(Path::new(path)), modified(&movie), modified(&index)) else {
        return false;
    };
    mov.min(idx) >= src
}

/// Drops a stale proxy (`<slug>.mov`, `.json`, `.scenes.json` and the scenes
/// poster folder) so the next build starts clean.
fn remove_stale(root: &Path, path: &str) {
    let slug = metal_reel::scenes::slug(path);
    let _ = metal_vfs::remove_file(root.join(format!("{slug}.mov")));
    let _ = metal_vfs::remove_file(root.join(format!("{slug}.json")));
    let _ = metal_vfs::remove_file(root.join(format!("{slug}.scenes.json")));
    let _ = metal_vfs::remove_dir_all(root.join(format!("{slug}.scenes")));
}

fn set(path: &str, step: &str, progress: f32, done: bool, error: Option<String>) {
    if let Some(job) = jobs_map().lock().expect("ingest lock").get_mut(path) {
        job.step = step.to_string();
        job.progress = progress;
        job.done = done;
        job.error = error;
    }
}

/// Queues `path`'s HAP proxy (and its scenes) to be built in the background.
/// A no-op in tests and for a path that is not a file; a no-op while automatic
/// proxy building is switched off (`settings::ingest_proxy`); a no-op while a
/// job for it is already running; and an immediate `ready` when the proxy is
/// fresh.
pub fn enqueue(path: &str) {
    if cfg!(test) || !metal_vfs::is_file(Path::new(path)) {
        return;
    }
    if !crate::call::settings::ingest_proxy() {
        return;
    }
    {
        let mut jobs = jobs_map().lock().expect("ingest lock");
        if jobs.get(path).is_some_and(|j| !j.done) {
            return;
        }
        if proxy_is_fresh(Path::new(super::playout::HAP), path) {
            jobs.insert(
                path.to_string(),
                Job { path: path.to_string(), step: "ready".into(), progress: 1.0, done: true, error: None },
            );
            return;
        }
        jobs.insert(
            path.to_string(),
            Job { path: path.to_string(), step: "preparing".into(), progress: 0.0, done: false, error: None },
        );
    }
    let path = path.to_string();
    std::thread::Builder::new()
        .name("metal-dash ingest".into())
        .spawn(move || {
            let root = Path::new(super::playout::HAP);
            remove_stale(root, &path);
            // A YCGCO clip on an ffmpeg without `zscale` cannot be converted at
            // all. Say so here, in the operator's words, rather than let ffmpeg
            // fail on the first filter with a swscale error.
            if let Ok(info) = metal_ingest::video::probe(Path::new(&path)) {
                if metal_ingest::video::needs_zscale(&info.pix_fmt, &info.color_space)
                    && !metal_ingest::video::zscale_available()
                {
                    set(
                        &path,
                        "failed",
                        0.0,
                        true,
                        Some("cannot convert this clip: its colourspace (ycgco) needs zscale, which this ffmpeg lacks".into()),
                    );
                    return;
                }
            }
            // `hap_proxy`'s one `encode_hap_video` pass reports ffmpeg's own
            // frame count, so `converting` walks from 0.05 to 0.95 as it runs.
            set(&path, "converting", 0.05, false, None);
            let p = path.clone();
            match super::playout::hap_proxy(root, &path, &mut |f| {
                set(&p, "converting", 0.05 + 0.9 * f, false, None);
            }) {
                Ok(_) => set(&path, "ready", 1.0, true, None),
                Err(e) => set(&path, "failed", 0.0, true, Some(e)),
            }
        })
        .expect("the ingest thread starts");
}

/// Every job, for `/state`'s `ingest` block.
pub fn jobs() -> Value {
    Value::Array(
        jobs_map()
            .lock()
            .expect("ingest lock")
            .values()
            .map(|j| {
                json!({
                    "path": j.path,
                    "step": j.step,
                    "progress": j.progress,
                    "done": j.done,
                    "error": j.error,
                })
            })
            .collect(),
    )
}

/// `POST /ingest {path}`: queue the clip's proxy. Answers at once, before the
/// build runs; `/state`'s `ingest` block reports it.
pub fn ingest_post(_ctx: &mut super::Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("expected json".into()))?;
    let path = body["path"].as_str().filter(|p| !p.is_empty()).ok_or_else(|| CallError::Bad("path?".into()))?;
    enqueue(path);
    Ok(json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, modified: std::time::SystemTime) {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        let f = std::fs::File::create(path).unwrap();
        f.set_modified(modified).unwrap();
    }

    /// Freshness is both files present, and the older of them no older than the
    /// source. Anything else rebuilds.
    #[test]
    fn a_proxy_is_fresh_only_when_both_files_are_new_enough() {
        let dir = std::env::temp_dir().join(format!("fw-ingest-fresh-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let src = dir.join("clip.mp4");
        let root = dir.join("hap");
        let slug = metal_reel::scenes::slug(src.to_str().unwrap());
        let movie = root.join(format!("{slug}.mov"));
        let index = root.join(format!("{slug}.json"));

        let now = std::time::SystemTime::now();
        let src_time = now - std::time::Duration::from_secs(10);
        let newer = now;
        touch(&src, src_time);

        assert!(!proxy_is_fresh(&root, src.to_str().unwrap()), "nothing built yet");
        touch(&movie, newer);
        assert!(!proxy_is_fresh(&root, src.to_str().unwrap()), "the index is missing");
        touch(&index, newer);
        assert!(proxy_is_fresh(&root, src.to_str().unwrap()), "both present and newer");

        touch(&movie, src_time - std::time::Duration::from_secs(1));
        assert!(!proxy_is_fresh(&root, src.to_str().unwrap()), "the movie is older than the source");

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// The guard that keeps tests from spawning ffmpeg or writing to `data/`.
    #[test]
    fn enqueue_is_a_no_op_under_test() {
        let path = std::env::temp_dir().join(format!("fw-ingest-noop-{}.mp4", std::process::id()));
        std::fs::write(&path, b"x").unwrap();
        enqueue(path.to_str().unwrap());
        let listed = jobs();
        assert!(
            listed.as_array().unwrap().iter().all(|j| j["path"] != path.to_str().unwrap()),
            "no job is created in tests: {listed}"
        );
        assert!(!proxy_is_fresh(Path::new(super::super::playout::HAP), path.to_str().unwrap()));
        let _ = metal_vfs::remove_file(&path);
    }
}
