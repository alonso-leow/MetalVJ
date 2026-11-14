//! The performance monitor (2026-10-09): why the client stutters, measured.
//!
//! Two things are recorded, process-wide and always on (a few `Instant`s and
//! one short mutex per record — cheap next to what they measure):
//!
//! - **Every call through the shared lock** — the shell's door, the preview
//!   worker, every HTTP request — as `(thread, route)`: how long it *waited*
//!   for `Shared`'s lock and how long it then *held* it. A UI frame that
//!   waits behind a preview render is the classic stall; this names it.
//! - **Every client frame**: the shell's `ui` pass, then tessellate + paint,
//!   plus the door time the UI thread spent inside that frame. A frame over
//!   [`SLOW_FRAME_MS`] is logged to stderr with its worst calls, so a
//!   stutter leaves a line saying what it was.
//!
//! [`snapshot`] summarises the last [`WINDOW`]; `GET /perf` answers it as
//! JSON and the shell's F12 HUD paints it.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use web_time::{Duration, Instant};

use serde_json::{json, Value};

/// The span a snapshot summarises.
pub const WINDOW: Duration = Duration::from_secs(3);
/// A client frame slower than this is a stutter worth a log line.
pub const SLOW_FRAME_MS: f32 = 50.0;
/// How many slow frames the snapshot keeps.
const SLOW_KEEP: usize = 12;

struct Call {
    at: Instant,
    key: String,
    wait: f32,
    run: f32,
}

struct FrameRec {
    at: Instant,
    ui: f32,
    paint: f32,
    door: f32,
}

#[derive(Default)]
struct Perf {
    calls: VecDeque<Call>,
    frames: VecDeque<FrameRec>,
    /// The UI thread's calls since the current frame began.
    frame_calls: Vec<(String, f32, f32)>,
    slow: VecDeque<Value>,
    spans: VecDeque<(Instant, &'static str, f32)>,
}

fn perf() -> &'static Mutex<Perf> {
    static P: OnceLock<Mutex<Perf>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(Perf::default()))
}

/// The surface's present mode, set once when the window is configured; the
/// `frame.present` span is the time spent in that mode's present.
static PRESENT_MODE: OnceLock<String> = OnceLock::new();

/// Records the window's chosen present mode for [`snapshot`].
pub fn set_present_mode(mode: String) {
    let _ = PRESENT_MODE.set(mode);
}

/// The present case the window decided once at startup (`the design notes` G, step 2):
/// (a) the window shares the painter's device, (b) it cannot, so previews
/// cross as a raw RGBA readback, or (c) there is no painter device and the
/// `Cpu` backend / JPEG previews are used. Reported by `GET /perf` so the
/// chosen case is observable without a window.
static PREVIEW_PATH: AtomicU8 = AtomicU8::new(0);

/// Records the present case ([`preview_path`]) the window chose.
pub fn set_preview_path(case: &str) {
    let v = match case {
        "a_shared" => 1,
        "b_readback" => 2,
        "c_jpeg" => 3,
        _ => 0,
    };
    PREVIEW_PATH.store(v, Ordering::Relaxed);
}

/// The recorded present case, `"unset"` before a window exists.
pub fn preview_path() -> &'static str {
    match PREVIEW_PATH.load(Ordering::Relaxed) {
        1 => "a_shared",
        2 => "b_readback",
        3 => "c_jpeg",
        _ => "unset",
    }
}

fn ms(d: Duration) -> f32 {
    d.as_secs_f32() * 1000.0
}

fn thread_label() -> String {
    std::thread::current().name().unwrap_or("worker").to_string()
}

fn trim(p: &mut Perf, now: Instant) {
    while p.calls.front().is_some_and(|c| now.duration_since(c.at) > WINDOW) {
        p.calls.pop_front();
    }
    while p.frames.front().is_some_and(|f| now.duration_since(f.at) > WINDOW) {
        p.frames.pop_front();
    }
}

/// One call through the shared lock: `wait` to acquire it, `run` held.
pub fn call(method: &str, path: &str, wait: Duration, run: Duration) {
    let thread = thread_label();
    let key = format!("{thread} · {method} {path}");
    let now = Instant::now();
    let Ok(mut p) = perf().lock() else { return };
    if thread == "main" {
        p.frame_calls.push((format!("{method} {path}"), ms(wait), ms(run)));
    }
    p.calls.push_back(Call { at: now, key, wait: ms(wait), run: ms(run) });
    trim(&mut p, now);
}

/// Times `f` as a call that already holds whatever it needs — for a route
/// that takes no shared lock (`/author/*`).
pub fn timed<T>(method: &str, path: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let out = f();
    call(method, path, Duration::ZERO, t.elapsed());
    out
}

/// The start of a client frame: forget the previous frame's calls.
pub fn frame_begin() {
    if let Ok(mut p) = perf().lock() {
        p.frame_calls.clear();
    }
}

/// The end of a client frame: `ui` (the shell's pass) and `paint`
/// (tessellate + GPU submit). Logs the frame when it is slow.
pub fn frame_end(ui: Duration, paint: Duration) {
    let now = Instant::now();
    let Ok(mut p) = perf().lock() else { return };
    let calls = std::mem::take(&mut p.frame_calls);
    let door: f32 = calls.iter().map(|(_, w, r)| w + r).sum();
    let (ui, paint) = (ms(ui), ms(paint));
    p.frames.push_back(FrameRec { at: now, ui, paint, door });
    if ui + paint > SLOW_FRAME_MS {
        let mut worst = calls.clone();
        worst.sort_by(|a, b| (b.1 + b.2).total_cmp(&(a.1 + a.2)));
        worst.truncate(4);
        let line = worst
            .iter()
            .map(|(k, w, r)| format!("{k} wait {w:.1} run {r:.1}"))
            .collect::<Vec<_>>()
            .join(" | ");
        eprintln!(
            "perf: slow frame {:.0} ms (ui {ui:.0}, paint {paint:.0}, door {door:.0} in {} calls) {line}",
            ui + paint,
            calls.len()
        );
        p.slow.push_back(json!({
            "ms": ui + paint, "ui": ui, "paint": paint, "door": door, "calls": calls.len(),
            "worst": worst.iter().map(|(k, w, r)| json!({ "call": k, "wait": w, "run": r })).collect::<Vec<_>>(),
        }));
        while p.slow.len() > SLOW_KEEP {
            p.slow.pop_front();
        }
    }
    trim(&mut p, now);
}

fn pct(sorted: &[f32], q: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f32 * q).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

/// The last [`WINDOW`], summarised: frame rate and percentiles, the UI
/// thread's door share, every `(thread, route)` by total time held + waited,
/// and the recent slow frames.
pub fn snapshot() -> Value {
    let now = Instant::now();
    let Ok(mut p) = perf().lock() else { return json!({}) };
    trim(&mut p, now);
    let secs = WINDOW.as_secs_f32();
    let mut totals: Vec<f32> = p.frames.iter().map(|f| f.ui + f.paint).collect();
    totals.sort_by(f32::total_cmp);
    let n = p.frames.len().max(1) as f32;
    let mut routes: HashMap<&str, (u32, f32, f32, f32)> = HashMap::new();
    for c in &p.calls {
        let e = routes.entry(c.key.as_str()).or_default();
        e.0 += 1;
        e.1 += c.wait;
        e.2 += c.run;
        e.3 = e.3.max(c.wait + c.run);
    }
    let mut rows: Vec<Value> = routes
        .into_iter()
        .map(|(k, (count, wait, run, max))| {
            json!({
                "call": k, "per_s": count as f32 / secs,
                "wait_ms": wait / count as f32, "run_ms": run / count as f32,
                "max_ms": max, "busy": (wait + run) / (secs * 1000.0),
            })
        })
        .collect();
    rows.sort_by(|a, b| b["busy"].as_f64().unwrap_or(0.0).total_cmp(&a["busy"].as_f64().unwrap_or(0.0)));
    rows.truncate(16);
    let frames = p.frames.len().max(1) as f32;
    let mut spans: HashMap<&str, (f32, f32)> = HashMap::new();
    for (_, label, took) in &p.spans {
        let e = spans.entry(label).or_default();
        e.0 += took;
        e.1 = e.1.max(*took);
    }
    let mut span_rows: Vec<Value> = spans
        .into_iter()
        .map(|(k, (sum, max))| json!({ "span": k, "ms_per_frame": sum / frames, "max_ms": max }))
        .collect();
    span_rows.sort_by(|a, b| {
        b["ms_per_frame"].as_f64().unwrap_or(0.0).total_cmp(&a["ms_per_frame"].as_f64().unwrap_or(0.0))
    });
    let mut out = json!({
        "window_s": secs,
        // The mode `frame.present` ran under ("unset" before a window exists).
        "present_mode": PRESENT_MODE.get().map_or("unset", String::as_str),
        // G (step 2): which of case (a)/(b)/(c) the window took for previews.
        "preview_path": preview_path(),
        // G1: where the frame composes, decided once at startup
        // (`METALVJ_BACKEND=cpu|gpu` overrides the adapter's capability).
        "backend": metal_perform::backend::active().map_or("unset", |b| b.as_str()),
        "spans": span_rows,
        "fps": p.frames.len() as f32 / secs,
        "frame_ms": { "p50": pct(&totals, 0.5), "p95": pct(&totals, 0.95), "max": totals.last().copied().unwrap_or(0.0) },
        "ui_ms": p.frames.iter().map(|f| f.ui).sum::<f32>() / n,
        "paint_ms": p.frames.iter().map(|f| f.paint).sum::<f32>() / n,
        "door_ms": p.frames.iter().map(|f| f.door).sum::<f32>() / n,
        "slow_frames": p.frames.iter().filter(|f| f.ui + f.paint > SLOW_FRAME_MS).count(),
        "calls": rows,
        "recent_slow": p.slow.iter().cloned().collect::<Vec<_>>(),
    });
    // The remote-layer fetch/decode counters live in `remote_layers` (same
    // process); joined here so `GET /perf` carries them with the rest. The
    // module is desktop-gpu only, so the field is too.
    #[cfg(all(feature = "desktop", any(feature = "gpu", feature = "glow")))]
    {
        out["remote"] = crate::remote_layers::remote_stats();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Step 2/4: `GET /perf` names the present case the window took.
    #[test]
    fn the_snapshot_reports_the_present_case() {
        set_preview_path("b_readback");
        assert_eq!(snapshot()["preview_path"], "b_readback");
    }

    #[test]
    fn a_recorded_call_shows_in_the_snapshot() {
        call("GET", "/perf-test-route", Duration::from_millis(2), Duration::from_millis(3));
        let s = snapshot();
        assert!(s["calls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["call"].as_str().unwrap().ends_with("GET /perf-test-route")));
    }
}

/// Times one piece of the UI pass (a panel, a section) until dropped —
/// `let _span = perf::Span::new("nodes");` — so the HUD and `GET /perf`
/// say where the frame's own CPU goes, not just its door calls.
pub struct Span {
    label: &'static str,
    started: Instant,
}

impl Span {
    pub fn new(label: &'static str) -> Span {
        Span { label, started: Instant::now() }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        record_span(self.label, ms(self.started.elapsed()));
    }
}

/// Spans at or above this are logged to stderr under [`trace`].
const TRACE_SPAN_MS: f32 = 2.0;

/// `METALVJ_UI_TRACE` in the environment: every UI-thread span over
/// [`TRACE_SPAN_MS`] and every lock wait over it is logged as it happens, so a
/// stutter can be read line by line instead of only as `GET /perf` averages.
pub fn trace() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("METALVJ_UI_TRACE").is_some())
}

fn record_span(label: &'static str, took: f32) {
    let now = Instant::now();
    if trace() && took >= TRACE_SPAN_MS {
        let thread = std::thread::current().name().unwrap_or("worker").to_string();
        eprintln!("perf: span {label} {took:.2} ms ({thread})");
    }
    if let Ok(mut p) = perf().lock() {
        p.spans.push_back((now, label, took));
        while p.spans.front().is_some_and(|(at, _, _)| now.duration_since(*at) > WINDOW) {
            p.spans.pop_front();
        }
    }
}

/// Locks `m`, recording how long the lock took to acquire as a span named
/// `label`. Use it where a UI-thread wait on a mutex is suspected: the wait is
/// the frame's cost, and it does not show as a shared-lock call.
pub fn locked<'a, T>(m: &'a Mutex<T>, label: &'static str) -> std::sync::LockResult<std::sync::MutexGuard<'a, T>> {
    let started = Instant::now();
    let guard = m.lock();
    record_span(label, ms(started.elapsed()));
    guard
}

/// Records a span that was timed by the caller (a phase whose start and end
/// are not one scope).
pub fn span_done(label: &'static str, took: Duration) {
    record_span(label, ms(took));
}

/// Times `f` as a span named `label` (see [`Span`]).
pub fn span<T>(label: &'static str, f: impl FnOnce() -> T) -> T {
    let _span = Span::new(label);
    f()
}
