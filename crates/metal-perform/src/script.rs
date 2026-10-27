//! O.6 (J.3): the Python runtime for a `script.py` node — `metal_graph::NodeFamily::Script`.
//!
//! `plan-opus.md § O.6`'s own framing: a script node runs a function over a
//! row of floats and writes a row of floats — **it transforms, it does not
//! draw**. The whole design is the boundary: what it sees, what it writes,
//! what happens if it is slow, what happens if it raises.
//!
//! - **What it sees / writes**: [`run`] takes the node's own declared floats
//!   (name/range/default, [`ScriptFloat`] — the same shape `metal-forge`'s
//!   vocabulary and `metal_schema::FloatSpec` already use for "a script with
//!   baked-in constants is a frame, not a node") and the row it was fed, and
//!   returns the row it wrote. Neither side is an image; both are `&[f32]`,
//!   the same as the `control`/`row` ports `metal_graph::NodeKind::catalog`
//!   already gives `script.py`.
//! - **What happens if it is slow**: [`run`] enforces [`SCRIPT_TIMEOUT`] by
//!   running the interpreter as its own subprocess and killing it the
//!   moment the budget is up — a hung or malicious script cannot stall or
//!   crash the render loop that called this function, the same isolation
//!   promise `metal_forge::raster` makes for a hung shader, met by the tool the
//!   task itself suggests for a killable child process: `Command` plus a
//!   deadline, not a GPU submission `wgpu` has no API to cancel.
//! - **What happens if it raises**: an exception in the user's Python is
//!   caught inside the subprocess's own harness and reported back as
//!   ordinary JSON (`{"ok": false, "error": ...}`), which [`run`] turns into
//!   [`ScriptError::Raised`] — a normal `Result`, never a panic, never a
//!   process metal-perform has to reason about beyond its exit.
//!
//! **What is Python-specific here, and what is not — for `S.G`.** The
//! harness script and the `python3` command line are Python-only.
//! [`ScriptFloat`], the row-in/row-out signature, [`SCRIPT_TIMEOUT`]'s
//! kill-on-overrun *shape* and the `{ok, row}`/`{ok, error}` wire contract
//! are the parts `S.G` (Rust/JS runtimes) reuses — a `script.rs`/`script.js`
//! kind hits the same three verification cases this module's own tests do.
//!
//! # S.G: Rust and JavaScript, added on top
//!
//! [`run_js`] is the Python shape almost unchanged: `node` is a system
//! binary here the way `python3` is for O.6 (`which node` on this
//! workspace's dev machines; no `deno_core`/`boa`/`v8` crate anywhere in the
//! workspace, no existing JS-engine story to plug into), so it gets the exact
//! same one-process, one-[`SCRIPT_TIMEOUT`] treatment.
//!
//! Rust is not an interpreter, so [`run_rust`] is O.6's shape run twice:
//! **compile**, under its own [`RUST_COMPILE_TIMEOUT`] (a script that fails
//! to compile is [`ScriptError::CompileFailed`] — a normal `Result`, not a
//! build the caller has to go interpret exit codes for, and not a panic),
//! then **run** the produced binary under the same [`SCRIPT_TIMEOUT`] every
//! other runtime uses, because once compiled it is native code and has no
//! more excuse to be slow than Python or JS do. `rustc` was checked, not
//! assumed, against the budget: a trivial single-file `rustc -O` compile
//! measured 50–200ms on this machine — well inside a several-second
//! allowance, hopelessly over `SCRIPT_TIMEOUT`'s 500ms, which is why
//! compiling gets its own, larger budget (sized like
//! `metal_forge::RASTER_TIMEOUT`'s 5s "slow but real work" allowance) instead
//! of sharing one. No `cargo`/crate support and no WASM runtime — both are
//! a real new dependency and toolchain for a lote whose test bar is
//! "compiles-or-a-`Result`, runs-or-gets-killed, panics-become-a-`Result`".
//!
//! # Real time: one process per script, not one per call
//!
//! The owner's clarification after O.6/S.G landed: *in most cases the node
//! should be applied in real time* — every frame, from the render loop, and
//! **without** an async/worker redesign: the call stays synchronous, the
//! caller still gets its row (or its error) back before it draws.
//!
//! The first cut spawned a fresh interpreter per call. Measured on this
//! machine (2026-09-20, five runs each): `python3 -c pass` **10.5ms**,
//! `node -e 0` **25–30ms**, a trivial `rustc -O` **50–200ms**, and running
//! an already-compiled binary **0.6ms**. Against a 33ms frame (30fps) or a
//! 16ms one (60fps), a Python spawn is a third of the budget for one node
//! and a `node` spawn is the whole frame — before the user's own code has
//! run a line. Compiling Rust per frame is not a question.
//!
//! So this module keeps **one long-lived process per distinct script** — a
//! [`Worker`] in a module-level [`Pool`], keyed by `(language, hash of
//! code)` — and each call is **one JSON line in, one JSON line out** over
//! that process's stdin/stdout, which is the same wire contract as before
//! with a newline where the process boundary used to be. Nothing about the
//! caller's contract changed: [`run`]/[`run_js`]/[`run_rust`] are still
//! `fn(code, floats, row) -> Result<Vec<f32>, ScriptError>`, still bounded
//! by [`SCRIPT_TIMEOUT`], and the pool is an implementation detail behind
//! them. What changed is the cost: after the first call (which pays the
//! spawn, or the compile) a call is a pipe round-trip — well under a
//! millisecond for all three languages.
//!
//! The three isolation promises hold under this design, and the tests below
//! check each one for each language:
//!
//! - **Raises → `Result`.** The harness catches the exception *per
//!   request* and answers `{"ok": false, ...}`; the worker stays alive for
//!   the next frame. A script that fails at definition time (a syntax error,
//!   no `process`) answers that same error to every request rather than
//!   crashing the worker.
//! - **Hangs → killed + `Result`.** The deadline is measured from the
//!   request being written: a reply that has not arrived by then means the
//!   worker is killed and reaped, dropped from the pool, and the call
//!   returns [`ScriptError::Timeout`]. The next call for the same code
//!   respawns lazily. Stdout is read on a reader thread that feeds an
//!   `mpsc` channel, because `recv_timeout` is the only std-only way to wait
//!   on a pipe with a deadline while still holding the `Child` to kill it.
//! - **Bad output → `Result`.** A reply that is not the wire JSON is
//!   [`ScriptError::BadOutput`]. The Python harness also points the user's
//!   `print` at stderr so a stray `print()` cannot corrupt the wire; the JS
//!   `vm` sandbox has no `console` at all; a Rust `println!` inside
//!   `process` *would* corrupt it and is reported as `BadOutput` — the user
//!   sees the error, the loop does not stall.
//!
//! **Regla 4 — state is read live.** The pool key is a hash of the code, so
//! editing a node's source is, by construction, a different worker: the next
//! call after an edit spawns the new one and the old one is left to
//! [`POOL_MAX`]'s least-recently-used eviction (and is killed on drop). The
//! engine reads `Node.config` every evaluation and passes the code in; this
//! module never remembers "the code of node X".
//!
//! **Bounds.** [`POOL_MAX`] live workers (the least recently used is killed
//! when a new one is needed beyond that), and [`RUST_CACHE_MAX`] compiled
//! binaries on disk.
//!
//! # The Rust binary cache
//!
//! [`rust_binary`] compiles a script **once per distinct code** into
//! [`rust_cache_dir`] (`std::env::temp_dir()/fw-script-rs/`, the same
//! machine-local scratch convention `metal-router`'s `metal-router-brain` and
//! `metal-router-npy` use — a binary is a derived, per-machine, per-toolchain
//! artefact and has no business next to the operator's own data under
//! `data/`). The file name is a hash of the *full* source (harness prelude +
//! user code) and of the `rustc` arguments, so a change to either is a
//! different binary; the compile writes to a `.tmp` sibling and renames, so
//! two processes compiling the same hash (the app and a test) cannot see
//! each other's half-written file. A cached binary that has disappeared, or
//! that fails to spawn, is recompiled; beyond [`RUST_CACHE_MAX`] entries the
//! oldest (by modification time) are removed at the next compile.
//! [`compile_count`] is the observable stat the cache test reads: a second
//! call with the same code must not move it.
//!
//! # The engine hook
//!
//! [`evaluate`] is what `engine.rs`'s `render_device` calls once per frame:
//! it walks the graph's `NodeFamily::Script` nodes in dependency order,
//! resolves each one's input row from its `control` in-edge (a `gen.*`
//! node's `row` port — the row `drive::row` just computed for it; another
//! script's `control` out — that script's fresh output; anything else — the
//! frame's raw digest, the same axis-(b) reading
//! `metal_graph::effects::control_source` gives an `fx.*` node's control cable),
//! reads [`ScriptConfig`] out of `Node.config` live, and returns each
//! script's output row keyed by node id. The engine then hands those rows to
//! whatever the script's `control` out-edge feeds — today a `mask` node, see
//! `engine::apply_script_masks`. A script that errors contributes no row
//! this frame and logs once per distinct message (not once per frame), and
//! the frame draws on.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{ChildStdin, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use web_time::{Duration, Instant};
use metal_schema::platform::Command;

use serde::{Deserialize, Serialize};

/// Hard wall-clock budget for one [`run`] call. The render/show loop calls
/// this once per node per frame; half a second is long enough that a script
/// doing real work (a handful of Python function calls over sixteen floats)
/// finishes with room to spare, and short enough that a hung script costs a
/// visibly dropped frame or two rather than a stall anyone would notice as
/// "the show froze". Revisit like `metal_forge::RASTER_TIMEOUT` if either
/// assumption turns out wrong in practice.
pub const SCRIPT_TIMEOUT: Duration = Duration::from_millis(500);

/// Wall-clock budget for a freshly spawned interpreter to finish booting and
/// load the user's code, before its first request is written. Interpreter
/// startup (`node`, `python3`) is not script work and varies with machine
/// load, so it is not charged against [`SCRIPT_TIMEOUT`]; the harness
/// announces `{"ready":true}` once it can answer, and [`Worker::spawn`] waits
/// for that under this budget.
pub const SCRIPT_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Hard wall-clock budget for `rustc`'s own compile step of a `script.rs`
/// node — separate from [`SCRIPT_TIMEOUT`] because compiling, unlike
/// starting an interpreter, has real, measured cost (50–200ms for a trivial
/// single-file `rustc -O` on this machine) before any user logic runs.
/// Sized like `metal_forge::RASTER_TIMEOUT`'s "slow but real work" allowance
/// rather than an interpreter's near-instant startup. Once compiled, running
/// the binary shares [`SCRIPT_TIMEOUT`] with every other runtime — and, with
/// the cache, compiling happens once per distinct code, not once per call.
pub const RUST_COMPILE_TIMEOUT: Duration = Duration::from_secs(5);

/// How many live script processes the pool keeps at once. Sixteen is
/// `Limits::max_layers`' order of magnitude — more distinct scripts than a
/// show has layers is not a case worth a process each. The least recently
/// used worker is killed to make room for a new one.
pub const POOL_MAX: usize = 16;

/// How many compiled `script.rs` binaries stay in [`rust_cache_dir`].
/// Each is a few hundred kilobytes; thirty-two is every script an operator
/// edited tonight plus room, and the oldest are pruned at the next compile.
pub const RUST_CACHE_MAX: usize = 32;

/// One float a script node declares as its own parameter: name, range and
/// the value it starts at — the `metal-forge` rule (`crates/metal-forge`'s vocab
/// module, `metal_schema::FloatSpec`) restated for a node whose body is Python
/// instead of WGSL. A script with a number written straight into the source
/// instead of read out of `floats` is "a frame, not a node" — declaring it
/// here is what makes it a real, adjustable node parameter.
///
/// **`S.AL.3`: `value` is the live knob, `default` is not.** Redeclaring
/// `name`/`min`/`max`/`default` is the heavier op — a different, whole-array
/// write (`set_params`'s `floats` *array* shape) that respawns nothing on
/// its own but does change what the node *is*. Turning the knob in real
/// time (a slider drag, `set_params`'s `floats` *object* shape,
/// `crate::script_floats` — no such module here, see
/// `metal_graph::script_floats::apply_floats_patch`) writes only `value`,
/// exactly the "fx.* `FxConfig::fixed`" pattern `metal_graph::effects` already
/// has: a value the node holds while nothing more specific (a wired
/// `floats` port, see [`evaluate`]) overrides it. `#[serde(default)]` so a
/// graph saved before this field existed still deserializes — `None` reads
/// as "no live value yet, use `default`", never a hard error.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScriptFloat {
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    #[serde(default)]
    pub value: Option<f32>,
}

/// A `script.*` node's own config — `Node.config`'s shape for this kind,
/// read live by [`evaluate`] every frame. `code` must define a top-level
/// `process(floats, row)` returning a list of numbers; anything else is a
/// [`ScriptError::Raised`] naming that, not a crash.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScriptConfig {
    pub code: String,
    #[serde(default)]
    pub floats: Vec<ScriptFloat>,
}

impl ScriptConfig {
    /// The values [`evaluate`] hands a script as its `floats` argument, in
    /// declaration order: each declared float's live [`ScriptFloat::value`]
    /// when it has one, its `default` otherwise — either way clamped to
    /// `[min, max]` as defense in depth (the write side,
    /// `metal_graph::script_floats::apply_floats_patch`, already refuses an
    /// out-of-range value rather than clamping it in; this clamp is for
    /// anything that reached `Node.config` some other way, a hand-edited
    /// graph file included).
    pub fn values(&self) -> Vec<f32> {
        self.values_with_override(None)
    }

    /// [`values`], but a wired `floats` port ([`evaluate`]'s reactive case)
    /// hands its resolved `row` here: `row[i]` wins over
    /// `floats[i]`'s `value`/`default` — the same "wired beats held" rule
    /// `metal_graph::effects::state_of`'s `Reactive` is for an `fx.*` node's
    /// `control` port, mirrored onto a script's own floats. A `row` shorter
    /// than the declared floats leaves the floats past its end on their own
    /// `value`/`default` — a short or absent row is "no override for this
    /// one", never an error.
    pub fn values_with_override(&self, row: Option<&[f32]>) -> Vec<f32> {
        self.floats
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let v = row.and_then(|r| r.get(i).copied()).unwrap_or_else(|| f.value.unwrap_or(f.default));
                v.clamp(f.min, f.max)
            })
            .collect()
    }
}

/// Everything [`run`] can fail with — never a panic, per O.6's third
/// verification case.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ScriptError {
    #[error("script exceeded the {0:?} budget and was killed")]
    Timeout(Duration),
    #[error("script raised: {0}")]
    Raised(String),
    #[error("could not run the script interpreter/compiler: {0}")]
    Spawn(String),
    #[error("script produced output this crate could not parse: {0}")]
    BadOutput(String),
    /// Rust-only: `rustc` exited nonzero. `S.G`'s "the node is not accepted
    /// if it does not compile; the error goes back to the AI" — this is
    /// that error, carrying `rustc`'s own diagnostics.
    #[error("script failed to compile: {0}")]
    CompileFailed(String),
}

/// Which runtime a `script.*` node runs — the `kind` string's suffix, never
/// a second field on the node (`metal_graph::NodeFamily::Script`'s own rule).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Python,
    JavaScript,
    Rust,
}

impl Lang {
    /// `script.py` → Python, `script.rs` → Rust, `script.js` → JavaScript;
    /// `None` for any other kind — the engine's "is this node mine" test.
    pub fn from_kind(kind: &str) -> Option<Lang> {
        match kind {
            "script.py" => Some(Lang::Python),
            "script.js" => Some(Lang::JavaScript),
            "script.rs" => Some(Lang::Rust),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// the three public entry points — one contract, three runtimes
// ---------------------------------------------------------------------------

/// Runs `code`'s `process(floats, row)` in Python under [`SCRIPT_TIMEOUT`].
/// See the module doc for the full contract; [`run_with_timeout`] is this
/// with an explicit budget, for tests that need to simulate an overrun
/// without waiting out the real one.
pub fn run(code: &str, floats: &[f32], row: &[f32]) -> Result<Vec<f32>, ScriptError> {
    run_with_timeout(code, floats, row, SCRIPT_TIMEOUT)
}

/// [`run`], with an explicit `timeout` instead of [`SCRIPT_TIMEOUT`].
pub fn run_with_timeout(
    code: &str,
    floats: &[f32],
    row: &[f32],
    timeout: Duration,
) -> Result<Vec<f32>, ScriptError> {
    call(Lang::Python, code, floats, row, timeout)
}

/// Runs `code`'s `process(floats, row)` under `node`, under [`SCRIPT_TIMEOUT`]
/// — the same budget and the same wire contract as [`run`], just a different
/// interpreter.
pub fn run_js(code: &str, floats: &[f32], row: &[f32]) -> Result<Vec<f32>, ScriptError> {
    run_js_with_timeout(code, floats, row, SCRIPT_TIMEOUT)
}

/// [`run_js`], with an explicit `timeout` instead of [`SCRIPT_TIMEOUT`].
pub fn run_js_with_timeout(
    code: &str,
    floats: &[f32],
    row: &[f32],
    timeout: Duration,
) -> Result<Vec<f32>, ScriptError> {
    call(Lang::JavaScript, code, floats, row, timeout)
}

/// Runs `code`'s `process(floats, row)`: compiles it with `rustc` under
/// [`RUST_COMPILE_TIMEOUT`] the first time this code is seen (a compile
/// failure is [`ScriptError::CompileFailed`], never a panic — `S.G`'s "the
/// node is not accepted if it does not compile"), then runs the cached
/// binary under [`SCRIPT_TIMEOUT`], same as every other runtime.
pub fn run_rust(code: &str, floats: &[f32], row: &[f32]) -> Result<Vec<f32>, ScriptError> {
    run_rust_with_timeout(code, floats, row, SCRIPT_TIMEOUT)
}

/// [`run_rust`], with an explicit `run_timeout` for the execution step
/// instead of [`SCRIPT_TIMEOUT`] — for tests that need to simulate a hung
/// *script* without waiting out the real budget. The compile step always
/// uses [`RUST_COMPILE_TIMEOUT`]: compiling is not what these tests are
/// simulating an overrun of.
pub fn run_rust_with_timeout(
    code: &str,
    floats: &[f32],
    row: &[f32],
    run_timeout: Duration,
) -> Result<Vec<f32>, ScriptError> {
    call(Lang::Rust, code, floats, row, run_timeout)
}

/// [`run`]/[`run_js`]/[`run_rust`] by [`Lang`] — what [`evaluate`] calls
/// once it has read the kind off the node.
pub fn run_lang(lang: Lang, code: &str, floats: &[f32], row: &[f32]) -> Result<Vec<f32>, ScriptError> {
    run_lang_ctx(lang, code, floats, row, &ScriptCtx::default())
}

/// [`run_lang`] with the musical state of this frame: Python and JS read it
/// as the global `ctx` (a dict/object with the fields of [`ScriptCtx`]), Rust
/// as `ctx()`. The scripts stay `process(floats, row)`; nothing is passed
/// that an old script would trip on.
pub fn run_lang_ctx(
    lang: Lang,
    code: &str,
    floats: &[f32],
    row: &[f32],
    ctx: &ScriptCtx,
) -> Result<Vec<f32>, ScriptError> {
    call_ctx(lang, code, floats, row, ctx, SCRIPT_TIMEOUT)
}

/// The musical state a script sees every frame, from the one `MusicClock`:
/// it follows the tracked tempo and downbeat ("1"), so when either is
/// changed the next frame's `ctx` is already the new one.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ScriptCtx {
    /// Current tempo, beats per minute.
    pub bpm: f32,
    /// Musical position in beats (quarter notes); a "1" lands on multiples of 4.
    pub beats: f64,
    /// Bars since the clock started counting.
    pub bar: u64,
    /// `0..4`: which beat of the bar (0 is the "1").
    pub beat_in_bar: u8,
    /// `0..16`: which sixteenth of the bar.
    pub step: u8,
    /// `0..1` through the current beat.
    pub beat_phase: f32,
    /// `0..1` through the current bar (0 at the "1").
    pub bar_phase: f32,
    /// Seconds on the show clock.
    pub time: f32,
}

impl Default for ScriptCtx {
    fn default() -> Self {
        ScriptCtx::from_beats(0.0, crate::clock::FALLBACK_BPM, 0.0)
    }
}

impl ScriptCtx {
    pub fn from_beats(beats: f64, bpm: f32, time: f32) -> ScriptCtx {
        let pos = crate::clock::Position::from_beats(beats);
        let in_beat = beats.max(0.0).rem_euclid(1.0) as f32;
        ScriptCtx {
            bpm,
            beats,
            bar: pos.bar,
            beat_in_bar: pos.beat_in_bar,
            step: pos.step,
            beat_phase: in_beat,
            bar_phase: (beats.max(0.0).rem_euclid(4.0) / 4.0) as f32,
            time,
        }
    }
}

/// The one body behind every entry point: find (or spawn) the worker for
/// this `(lang, code)`, send one request line, wait for one reply line under
/// `timeout`, parse it. A worker that timed out or died is dropped from the
/// pool so the next call starts fresh; a worker that answered — even with an
/// error — is kept, since a raised exception is the script's business, not
/// the process's.
fn call(lang: Lang, code: &str, floats: &[f32], row: &[f32], timeout: Duration) -> Result<Vec<f32>, ScriptError> {
    call_ctx(lang, code, floats, row, &ScriptCtx::default(), timeout)
}

fn call_ctx(
    lang: Lang,
    code: &str,
    floats: &[f32],
    row: &[f32],
    ctx: &ScriptCtx,
    timeout: Duration,
) -> Result<Vec<f32>, ScriptError> {
    let key = (lang, hash_str(code));
    let input = wire_input(floats, row, ctx);

    let mut pool = pool().lock().unwrap_or_else(|e| e.into_inner());
    let worker = pool.get_or_spawn(key, lang, code)?;
    match worker.request(&input, timeout) {
        Ok(line) => parse_wire_output(&line),
        Err(e) => {
            // Timeout, or the process is gone: either way this worker is
            // no longer one we can talk to. `Worker::drop` kills and reaps.
            pool.workers.remove(&key);
            Err(e)
        }
    }
}

/// The `{"floats": ..., "row": ...}` JSON every runtime's harness reads,
/// one per line — the input half of O.6's wire contract, shared verbatim.
fn wire_input(floats: &[f32], row: &[f32], ctx: &ScriptCtx) -> String {
    serde_json::json!({ "floats": floats, "row": row, "ctx": ctx }).to_string()
}

/// The output half of O.6's wire contract: `{"ok": true, "row": [...]}` or
/// `{"ok": false, "error": "..."}`, the same for Python, Rust and JS.
fn parse_wire_output(stdout: &str) -> Result<Vec<f32>, ScriptError> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| ScriptError::BadOutput(format!("{e}: {stdout:?}")))?;

    if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        let row = value
            .get("row")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| ScriptError::BadOutput(format!("no `row` array: {value}")))?
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect();
        Ok(row)
    } else {
        let msg = value.get("error").and_then(serde_json::Value::as_str).unwrap_or("unknown error");
        Err(ScriptError::Raised(msg.to_string()))
    }
}

/// A stable, non-cryptographic hash of a string — the pool key and the
/// on-disk cache name. `DefaultHasher::new()` (fixed SipHash keys, unlike
/// `RandomState`) so the same code hashes the same across runs of the app,
/// which is what lets a binary compiled last night be found tonight.
fn hash_str(s: &str) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------------------
// the pool: one live process per distinct script
// ---------------------------------------------------------------------------

/// The module's one piece of state: every live script process, keyed by
/// language and code hash. A global rather than something threaded through
/// `State` because the workers are OS processes, not data — they outlive no
/// `State` and belong to no device — and because the three `run*` entry
/// points predate the pool and keep their signatures.
fn pool() -> &'static Mutex<Pool> {
    static POOL: OnceLock<Mutex<Pool>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(Pool { workers: HashMap::new() }))
}

struct Pool {
    workers: HashMap<(Lang, u64), Worker>,
}

impl Pool {
    /// The live worker for `key`, spawning one if there is none (or if the
    /// one there was has exited on its own — a harness crash between
    /// frames). Evicts the least recently used worker first when the pool
    /// is at [`POOL_MAX`].
    fn get_or_spawn(&mut self, key: (Lang, u64), lang: Lang, code: &str) -> Result<&mut Worker, ScriptError> {
        let dead = self.workers.get_mut(&key).map(|w| w.exited()).unwrap_or(false);
        if dead {
            self.workers.remove(&key);
        }
        if !self.workers.contains_key(&key) {
            while self.workers.len() >= POOL_MAX {
                let Some(oldest) = self.workers.iter().min_by_key(|(_, w)| w.last_used).map(|(k, _)| *k) else {
                    break;
                };
                self.workers.remove(&oldest);
            }
            let worker = Worker::spawn(lang, code)?;
            self.workers.insert(key, worker);
        }
        Ok(self.workers.get_mut(&key).expect("just inserted or already present"))
    }
}

/// One long-lived script process and the two ends we talk to it through:
/// its stdin (a request line per call) and a channel fed by a reader thread
/// draining its stdout (a reply line per call). The thread ends when the
/// pipe closes, which happens when the child exits or is killed.
struct Worker {
    child: std::process::Child,
    stdin: ChildStdin,
    replies: Receiver<String>,
    last_used: Instant,
}

impl Worker {
    /// Spawns the harness for `lang` around `code`: `python3 -c`, `node -e`,
    /// or the cached (compiled) Rust binary. Stderr is inherited so a
    /// script's own diagnostics — a Python `print` redirected there, a Rust
    /// `eprintln!` — land in the app's log rather than in a pipe nobody
    /// drains (which would eventually block the child on a full buffer).
    fn spawn(lang: Lang, code: &str) -> Result<Worker, ScriptError> {
        let mut command = match lang {
            Lang::Python => {
                let code_json = serde_json::to_string(code).expect("a &str always serializes to JSON");
                let mut c = Command::new("python3");
                c.arg("-c").arg(HARNESS.replace("{code_json}", &code_json));
                c
            }
            Lang::JavaScript => {
                let code_json = serde_json::to_string(code).expect("a &str always serializes to JSON");
                let mut c = Command::new("node");
                c.arg("-e").arg(JS_HARNESS.replace("{code_json}", &code_json));
                c
            }
            Lang::Rust => Command::new(rust_binary(code)?),
        };
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());

        let mut child = match command.spawn() {
            Ok(c) => c,
            // A cached binary that no longer runs (a stale file from another
            // toolchain, a partial write from a crash): recompile once and
            // retry, rather than reporting a `Spawn` error for something a
            // recompile fixes.
            Err(_) if lang == Lang::Rust => {
                let _ = metal_vfs::remove_file(rust_binary_path(code));
                let mut retry = Command::new(rust_binary(code)?);
                retry.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
                retry.spawn().map_err(|e| ScriptError::Spawn(e.to_string()))?
            }
            Err(e) => return Err(ScriptError::Spawn(e.to_string())),
        };

        let stdin = child.stdin.take().ok_or_else(|| ScriptError::Spawn("no stdin pipe".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| ScriptError::Spawn("no stdout pipe".into()))?;
        let (tx, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let worker = Worker { child, stdin, replies, last_used: Instant::now() };
        // Wait for the harness to finish booting. A worker that never gets
        // here is dropped (killed and reaped) by returning the error.
        match worker.replies.recv_timeout(SCRIPT_STARTUP_TIMEOUT) {
            Ok(line) if line.trim() == READY_LINE => Ok(worker),
            Ok(line) => Err(ScriptError::BadOutput(format!("expected {READY_LINE}, got {line:?}"))),
            Err(RecvTimeoutError::Timeout) => Err(ScriptError::Timeout(SCRIPT_STARTUP_TIMEOUT)),
            Err(RecvTimeoutError::Disconnected) => {
                Err(ScriptError::Spawn("script process exited before it was ready".into()))
            }
        }
    }

    /// Whether the process has already exited — checked before reuse so a
    /// worker that died between frames is replaced instead of written to.
    fn exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)) | Err(_))
    }

    /// One round-trip: write `input` plus a newline, wait up to `timeout`
    /// for the reply line. A `Timeout` here means the caller must drop this
    /// worker — the process is mid-request and cannot be reused.
    fn request(&mut self, input: &str, timeout: Duration) -> Result<String, ScriptError> {
        self.last_used = Instant::now();
        // Drain anything a previous, timed-out request might have left —
        // can only happen if the worker was kept after a timeout, which
        // `call` never does, but a stale line here would be answered to
        // the wrong frame, so it is cheap insurance.
        while self.replies.try_recv().is_ok() {}

        let write = self.stdin.write_all(input.as_bytes()).and_then(|_| self.stdin.write_all(b"\n")).and_then(|_| self.stdin.flush());
        if let Err(e) = write {
            return Err(ScriptError::Spawn(format!("script process is gone: {e}")));
        }
        match self.replies.recv_timeout(timeout) {
            Ok(line) => Ok(line),
            Err(RecvTimeoutError::Timeout) => Err(ScriptError::Timeout(timeout)),
            Err(RecvTimeoutError::Disconnected) => {
                Err(ScriptError::Spawn("script process exited without answering".into()))
            }
        }
    }
}

impl Drop for Worker {
    /// Never left running or zombied: kill (a no-op if it already exited)
    /// and reap.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------------
// the three harnesses
// ---------------------------------------------------------------------------

/// The line every harness writes once it has loaded the user's code and can
/// answer requests — see [`SCRIPT_STARTUP_TIMEOUT`].
const READY_LINE: &str = r#"{"ready":true}"#;

/// The Python harness every `script.py` node runs inside. Wraps the user's
/// `code` (embedded as a JSON string literal — valid Python string syntax
/// too, so no second escaping pass is needed), defines `process` once, then
/// answers one request line with one reply line until stdin closes. A
/// raised exception — at definition or per request — becomes the same
/// `{"ok": false, ...}` JSON a well-behaved script would print for its own
/// error, instead of a nonzero exit code metal-perform would have to go
/// interpret. The user's `print` goes to stderr so it cannot corrupt the
/// wire; replies go to the real stdout, flushed per line.
const HARNESS: &str = r#"
import sys, json
CODE = {code_json}
_out = sys.stdout
sys.stdout = sys.stderr
def _reply(obj):
    _out.write(json.dumps(obj) + "\n")
    _out.flush()
fn = None
load_error = None
try:
    ns = {'ctx': {}}
    exec(CODE, ns)
    fn = ns.get('process')
    if fn is None:
        raise NameError("script must define process(floats, row)")
except Exception as e:
    load_error = f'{type(e).__name__}: {e}'
_out.write('{"ready":true}\n')
_out.flush()
for line in sys.stdin:
    if not line.strip():
        continue
    if load_error is not None:
        _reply({'ok': False, 'error': load_error})
        continue
    try:
        data = json.loads(line)
        ns['ctx'] = data.get('ctx', {})
        result = fn(data.get('floats', []), data.get('row', []))
        _reply({'ok': True, 'row': [float(x) for x in result]})
    except Exception as e:
        _reply({'ok': False, 'error': f'{type(e).__name__}: {e}'})
"#;

/// The JavaScript harness every `script.js` node runs inside `node`, the
/// exact shape of [`HARNESS`] for Python: the user's `code` must define a
/// top-level `function process(floats, row)`, run in its own `vm` context so
/// it cannot see or clobber the harness's own `process` (Node's global
/// object, not the user's function). One `readline` line in, one reply line
/// out; a thrown error becomes the same `{"ok": false, ...}` JSON a
/// well-behaved script would print itself.
const JS_HARNESS: &str = r#"
const vm = require('vm');
const readline = require('readline');
const CODE = {code_json};
let fn = null;
let loadError = null;
const sandbox = { ctx: {} };
try {
    vm.createContext(sandbox);
    vm.runInContext(CODE, sandbox);
    if (typeof sandbox.process !== 'function') {
        throw new Error('script must define function process(floats, row)');
    }
    fn = sandbox.process;
} catch (e) {
    loadError = String(e && e.message ? e.message : e);
}
const reply = (obj) => process.stdout.write(JSON.stringify(obj) + "\n");
reply({ ready: true });
const rl = readline.createInterface({ input: process.stdin, terminal: false });
rl.on('line', (line) => {
    if (!line.trim()) return;
    if (loadError !== null) { reply({ ok: false, error: loadError }); return; }
    try {
        const input = JSON.parse(line);
        sandbox.ctx = input.ctx || {};
        const result = fn(input.floats || [], input.row || []);
        reply({ ok: true, row: Array.from(result, Number) });
    } catch (e) {
        reply({ ok: false, error: String(e && e.message ? e.message : e) });
    }
});
"#;

/// The Rust harness a `script.rs` node's `code` is compiled into: `code`
/// must define `fn process(floats: &[f32], row: &[f32]) -> Vec<f32>`, called
/// once per stdin line under `catch_unwind` so a panic becomes
/// `{"ok": false, ...}` instead of a nonzero exit this crate would have to
/// go interpret — and so the process survives to answer the next frame.
/// Deliberately no `serde`/crate dependency inside the compiled script —
/// `parse_wire_input` hand-parses the same flat `{"floats":[...],"row":[...]}`
/// shape so `rustc` never has to resolve an external crate for a
/// single-file compile.
const RUST_HARNESS_PRELUDE: &str = r#"
fn parse_wire_input(input: &str) -> (Vec<f32>, Vec<f32>) {
    fn extract(input: &str, key: &str) -> Vec<f32> {
        let needle = format!("\"{}\":[", key);
        let start = match input.find(&needle) {
            Some(i) => i + needle.len(),
            None => return Vec::new(),
        };
        let end = input[start..].find(']').map(|i| start + i).unwrap_or(input.len());
        input[start..end]
            .split(',')
            .filter_map(|s| s.trim().parse::<f32>().ok())
            .collect()
    }
    (extract(input, "floats"), extract(input, "row"))
}

#[derive(Clone, Copy, Debug, Default)]
#[allow(dead_code)]
struct Ctx { bpm: f32, beats: f64, bar: u64, beat_in_bar: u8, step: u8, beat_phase: f32, bar_phase: f32, time: f32 }
thread_local! { static CTX: std::cell::Cell<Ctx> = std::cell::Cell::new(Ctx::default()); }
#[allow(dead_code)]
fn ctx() -> Ctx { CTX.with(|c| c.get()) }
fn parse_ctx(input: &str) -> Ctx {
    fn num(input: &str, key: &str) -> f64 {
        let needle = format!("\"{}\":", key);
        let start = match input.find(&needle) { Some(i) => i + needle.len(), None => return 0.0 };
        let rest = &input[start..];
        let end = rest.find(|c: char| c == ',' || c == '}').unwrap_or(rest.len());
        rest[..end].trim().parse::<f64>().unwrap_or(0.0)
    }
    Ctx {
        bpm: num(input, "bpm") as f32, beats: num(input, "beats"), bar: num(input, "bar") as u64,
        beat_in_bar: num(input, "beat_in_bar") as u8, step: num(input, "step") as u8,
        beat_phase: num(input, "beat_phase") as f32, bar_phase: num(input, "bar_phase") as f32,
        time: num(input, "time") as f32,
    }
}

fn main() {
    use std::io::{BufRead, Write};
    std::panic::set_hook(Box::new(|_| {}));
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    {
        let mut out = stdout.lock();
        let _ = writeln!(out, "{{\"ready\":true}}");
        let _ = out.flush();
    }
    for line in stdin.lock().lines() {
        let line = match line { Ok(l) => l, Err(_) => break };
        if line.trim().is_empty() { continue; }
        let (floats, row) = parse_wire_input(&line);
        CTX.with(|c| c.set(parse_ctx(&line)));
        let outcome = std::panic::catch_unwind(|| process(&floats, &row));
        let mut out = stdout.lock();
        match outcome {
            Ok(row) => {
                let joined: Vec<String> = row.iter().map(|v| v.to_string()).collect();
                let _ = writeln!(out, "{{\"ok\":true,\"row\":[{}]}}", joined.join(","));
            }
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "script panicked".to_string());
                let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
                let _ = writeln!(out, "{{\"ok\":false,\"error\":\"{}\"}}", escaped);
            }
        }
        let _ = out.flush();
    }
}

"#;

// ---------------------------------------------------------------------------
// the Rust binary cache
// ---------------------------------------------------------------------------

/// The `rustc` arguments a cached binary was built with — part of the cache
/// key, so changing them here invalidates every binary built without them.
const RUSTC_ARGS: &[&str] = &["-O"];

/// How many `rustc` invocations this process has made — the cache's
/// observable stat: a second [`run_rust`] with the same code must not move
/// it. Plain `usize`, monotonic, never reset.
static RUST_COMPILES: AtomicUsize = AtomicUsize::new(0);

/// See [`RUST_COMPILES`].
pub fn compile_count() -> usize {
    RUST_COMPILES.load(Ordering::SeqCst)
}

/// Where compiled `script.rs` binaries live: `temp_dir()/fw-script-rs/`.
/// Machine-local scratch, the same convention as `metal-router-brain`; wiped
/// with the temp dir, and rebuilt on demand when it is.
pub fn rust_cache_dir() -> PathBuf {
    std::env::temp_dir().join("fw-script-rs")
}

/// The full single-file source `rustc` sees for `code`: harness + user code.
fn rust_source(code: &str) -> String {
    format!("{RUST_HARNESS_PRELUDE}{code}\n")
}

/// The cache path for `code`'s binary — a hash of the full source and the
/// `rustc` arguments, so either changing is a different file.
fn rust_binary_path(code: &str) -> PathBuf {
    let key = hash_str(&format!("{}\n--\n{}", rust_source(code), RUSTC_ARGS.join(" ")));
    rust_cache_dir().join(format!("{key:016x}"))
}

/// `code`'s compiled binary: the cached one if it exists, otherwise compiled
/// now under [`RUST_COMPILE_TIMEOUT`] — [`ScriptError::CompileFailed`] with
/// `rustc`'s own diagnostics if that fails. Compiles to a `.tmp` sibling and
/// renames into place, so a concurrent compile of the same hash (the app and
/// a test, two devices with the same script) never observes a partial file.
/// Prunes the cache to [`RUST_CACHE_MAX`] entries after each compile.
fn rust_binary(code: &str) -> Result<PathBuf, ScriptError> {
    let binary = rust_binary_path(code);
    if metal_vfs::is_file(&binary) {
        return Ok(binary);
    }
    let dir = rust_cache_dir();
    metal_vfs::create_dir_all(&dir).map_err(|e| ScriptError::Spawn(e.to_string()))?;

    // `rustc` derives the crate name from the file stem, and a stem with a
    // `.` in it is not a valid crate name — so the scratch stem is
    // underscores only, and `--crate-name` pins it regardless.
    let stem = binary.file_name().and_then(|n| n.to_str()).unwrap_or("script").to_string();
    let nonce = format!("{}_{}", crate::atomic::pid_tag(), rand_seed());
    let source_path = dir.join(format!("s{stem}_{nonce}.rs"));
    let tmp_binary = dir.join(format!("{stem}.{nonce}.tmp"));
    metal_vfs::write(&source_path, rust_source(code)).map_err(|e| ScriptError::Spawn(e.to_string()))?;

    RUST_COMPILES.fetch_add(1, Ordering::SeqCst);
    let mut compile = Command::new("rustc");
    compile.args(RUSTC_ARGS).arg("--crate-name").arg("fw_script").arg(&source_path).arg("-o").arg(&tmp_binary);
    let outcome = compile_with_timeout(compile, RUST_COMPILE_TIMEOUT);
    let _ = metal_vfs::remove_file(&source_path);
    match outcome {
        Ok(()) => {}
        Err(e) => {
            let _ = metal_vfs::remove_file(&tmp_binary);
            return Err(e);
        }
    }
    metal_vfs::rename(&tmp_binary, &binary).map_err(|e| ScriptError::Spawn(e.to_string()))?;
    prune_rust_cache(&dir);
    Ok(binary)
}

/// Runs `rustc` under `timeout` with the O.6 poll-and-kill loop (the
/// compiler is the one process here that is still one-shot): `Ok(())` on a
/// clean exit, [`ScriptError::CompileFailed`] with stderr on a nonzero one,
/// [`ScriptError::Timeout`] if it overran (killed and reaped).
fn compile_with_timeout(mut command: Command, timeout: Duration) -> Result<(), ScriptError> {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| ScriptError::Spawn(e.to_string()))?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ScriptError::Timeout(timeout));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(ScriptError::Spawn(e.to_string())),
        }
    };
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        use std::io::Read;
        let _ = err.read_to_string(&mut stderr);
    }
    if status.success() {
        Ok(())
    } else {
        Err(ScriptError::CompileFailed(stderr.trim().to_string()))
    }
}

/// Removes the oldest binaries (by modification time) beyond
/// [`RUST_CACHE_MAX`]. Leftover `.rs`/`.tmp` siblings from a crashed
/// compile are removed too. Best-effort: a cache that cannot be pruned is
/// still a cache.
fn prune_rust_cache(dir: &std::path::Path) {
    let Ok(entries) = metal_vfs::read_dir(dir) else { return };
    let mut binaries: Vec<(web_time::SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_scratch = path.extension().map(|e| e == "rs" || e == "tmp").unwrap_or(false);
        if is_scratch {
            // Only another process's *live* compile owns a scratch file;
            // one older than the compile budget is a leftover.
            if let Ok(meta) = entry.metadata() {
                if meta.modified().ok().and_then(|m| m.elapsed().ok()).map(|age| age > RUST_COMPILE_TIMEOUT * 2).unwrap_or(true) {
                    let _ = metal_vfs::remove_file(&path);
                }
            }
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            binaries.push((meta.modified().unwrap_or(web_time::SystemTime::UNIX_EPOCH), path));
        }
    }
    if binaries.len() <= RUST_CACHE_MAX {
        return;
    }
    binaries.sort();
    for (_, path) in binaries.iter().take(binaries.len() - RUST_CACHE_MAX) {
        let _ = metal_vfs::remove_file(path);
    }
}

/// A cheap, non-cryptographic per-call nonce for scratch file names — this
/// crate has no `rand` dependency, and uniqueness (not unpredictability) is
/// all a scratch path needs.
fn rand_seed() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish()
}

// ---------------------------------------------------------------------------
// the engine hook: every script node in a graph, once per frame
// ---------------------------------------------------------------------------

/// Evaluates every `script.*` node in `graph` once and returns each one's
/// output row by node id — what `render_device` calls after it has driven
/// the generators' rows and before it composes, so a script's row can feed
/// whatever its `control` out-edge points at.
///
/// **Input row.** The node's `control` in-edge decides, by the same reading
/// `metal_graph::effects::control_source` gives an `fx.*` node's control cable:
/// a `gen.*` node's `row` port is that generator's driven row (`gen_rows`);
/// another script's `control` out is that script's output this frame
/// (scripts are evaluated in dependency order, so it is already there); any
/// other source — `audio.*`, `sensor.*`, `cam.track` — is the frame's raw
/// digest, unshaped (`raw_digest`). No in-edge at all is an empty row: the
/// script runs on its own floats alone.
///
/// **Errors.** A script that raises, hangs (killed at [`SCRIPT_TIMEOUT`]),
/// fails to compile or produces bad output contributes no row this frame —
/// its consumer sees "nothing arrived", exactly as if the cable were
/// unplugged — and the message is logged **once per distinct message per
/// node**, not once per frame: a show at 60fps must not write the same
/// traceback sixty times a second. A `bypass`ed script node is skipped
/// like any other bypassed node.
///
/// Param edges into a script's floats (`metal_graph::bind`) are honoured too —
/// see [`evaluate_with`]; without the generators' parameters this reads a
/// binding from a digest column or another script, not from a generator row.
pub fn evaluate(
    graph: &metal_graph::Graph,
    gen_rows: &HashMap<metal_graph::NodeId, Vec<f32>>,
    raw_digest: &[f32],
) -> HashMap<metal_graph::NodeId, Vec<f32>> {
    evaluate_with(graph, gen_rows, raw_digest, &crate::bind::GenParams::new()).rows
}

/// What [`evaluate_with`] answers: each script's output row, and the floats
/// each one was handed this frame — what `GET /graph/live` shows as its
/// parameters' values.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Evaluated {
    pub rows: HashMap<metal_graph::NodeId, Vec<f32>>,
    pub floats: HashMap<metal_graph::NodeId, Vec<f32>>,
}

/// [`evaluate`], with the bindings: a param edge landing on `param:<float>`
/// of a script sets that float for this frame — the producer's value mapped
/// onto the float's range (`crate::bind::map_value`), after the declared
/// value and a wired `floats` row, so the named cable wins over the
/// positional one. The producer is a digest column, a generator's row float
/// (`gen_params` names them) or another script's output, and a script bound
/// to a pending script waits for it like a `control` or `floats` cable does.
pub fn evaluate_with(
    graph: &metal_graph::Graph,
    gen_rows: &HashMap<metal_graph::NodeId, Vec<f32>>,
    raw_digest: &[f32],
    gen_params: &crate::bind::GenParams,
) -> Evaluated {
    evaluate_clocked(graph, gen_rows, raw_digest, gen_params, crate::script_preset::BeatClock::default())
}

/// [`evaluate_with`], plus the clock the native presets
/// ([`crate::script_preset`]) read their beat from.
pub fn evaluate_clocked(
    graph: &metal_graph::Graph,
    gen_rows: &HashMap<metal_graph::NodeId, Vec<f32>>,
    raw_digest: &[f32],
    gen_params: &crate::bind::GenParams,
    clock: crate::script_preset::BeatClock,
) -> Evaluated {
    let ctx = ScriptCtx::from_beats(clock.beats, clock.bpm, clock.time);
    let is_script = |kind: &str| Lang::from_kind(kind).is_some() || crate::script_preset::is_preset(kind);
    let mut out = Evaluated::default();
    // P-D: `in.artnet` is a source, not a script: its row is the receiver's
    // latest frame, written before any script runs so a binding or a script
    // reads it this frame.
    for n in graph.nodes.iter().filter(|n| !n.bypass && n.kind == crate::artnet::ARTNET_IN_KIND) {
        crate::artnet::listen();
        out.rows.insert(n.id.clone(), crate::artnet::in_row(&crate::artnet::ArtNetInConfig::from_node(n)));
    }
    // M1 (plan "Value sources"): the `source.*` value nodes compute a row from
    // the clock, written before any script runs so a binding reads it this
    // frame — the same seam `in.artnet` just used.
    for n in graph.nodes.iter().filter(|n| !n.bypass && crate::source::is_source_kind(&n.kind)) {
        if let Some(row) = crate::source::evaluate_node(n, clock.beats, clock.time) {
            out.rows.insert(n.id.clone(), row);
        }
    }
    let mut pending: Vec<&metal_graph::Node> =
        graph.nodes.iter().filter(|n| !n.bypass && is_script(&n.kind)).collect();
    if pending.is_empty() {
        return out;
    }
    // Scripts already evaluated this frame, whether or not they answered: a
    // script whose source errored runs with nothing arrived — "as if the
    // cable were unplugged" — instead of waiting for a row that never comes.
    let mut done: std::collections::HashSet<&str> = std::collections::HashSet::new();

    // Dependency order without a full toposort: a script whose `control` or
    // `floats` source, or a binding's producer, is another *pending* script
    // waits a round — `S.AL.3` gives `floats` the same wait `control` already
    // had, since a script chained through either port is just as unready. A
    // cycle (which lint rejects, but a saved graph may still carry) leaves its
    // members pending forever, so the loop stops when a round makes no
    // progress.
    loop {
        let before = pending.len();
        let mut next = Vec::with_capacity(pending.len());
        for node in pending {
            let pending_script = |id: &str| -> bool {
                graph
                    .node(id)
                    .map(|from| is_script(&from.kind) && !from.bypass && !done.contains(from.id.as_str()))
                    .unwrap_or(false)
            };
            let waits_on_pending_script = |port: &str| -> bool {
                graph
                    .edges
                    .iter()
                    .find(|e| e.to.node == node.id && e.to.port == port)
                    .is_some_and(|e| pending_script(&e.from.node))
            };
            let waits_on_binding = metal_graph::bind::param_edges(graph, &node.id)
                .iter()
                .any(|e| pending_script(&e.from.node));
            if waits_on_pending_script("control") || waits_on_pending_script("floats") || waits_on_binding {
                next.push(node);
                continue;
            }
            done.insert(node.id.as_str());
            let row = resolve_port_row(graph, &node.id, "control", gen_rows, &out.rows, raw_digest).unwrap_or_default();
            let floats_row = resolve_port_row(graph, &node.id, "floats", gen_rows, &out.rows, raw_digest);
            let Some((lang, config)) = node_config(node) else { continue };
            let mut values = config.values_with_override(floats_row.as_deref());
            for e in metal_graph::bind::param_edges(graph, &node.id) {
                let Some(name) = metal_graph::params::param_name(&e.to.port) else { continue };
                let Some(i) = config.floats.iter().position(|f| f.name == name) else { continue };
                // A script's own float binding has no Looper in hand here, so
                // the two `control.repeat.*` variables read as no-loop
                // (`RepeatVars::default`); the frame's passes in `engine.rs`
                // supply the real values for every other consumer.
                let repeat = crate::bind::RepeatVars::default();
                let Some((v, min, max)) =
                    crate::bind::var_value(graph, &e.from, gen_rows, Some(&out.rows), gen_params, raw_digest, &repeat)
                else {
                    continue;
                };
                let f = &config.floats[i];
                values[i] = crate::bind::map_value(v, (min, max), (f.min, f.max));
            }
            out.floats.insert(node.id.clone(), values.clone());
            let result = match lang {
                Some(lang) => run_node(node, lang, &config, &values, &row, &ctx),
                None if node.kind == metal_graph::CTL_MATH_KIND => {
                    Some(crate::script_preset::ctl_math(&node.id, &values, &row, clock.time))
                }
                None => crate::script_preset::evaluate(&node.kind, &values, clock),
            };
            if let Some(result) = result {
                out.rows.insert(node.id.clone(), result);
            }
        }
        pending = next;
        if pending.is_empty() || pending.len() == before {
            break;
        }
    }
    out
}

/// What feeds `node_id`'s `port` input, resolved the same way for `control`
/// and `floats` (`S.AL.3` adds the second — one lookup, not two): a `gen.*`
/// node's `row` output is that generator's driven row (`gen_rows`); another
/// script's `control` output already evaluated this frame (`out`) is its
/// fresh row; anything else — `audio.*`, `sensor.*`, `cam.track` — is the
/// frame's raw digest, unshaped. `None` means the port has no incoming edge
/// at all; the caller decides what that means for it (an empty row for
/// `control`, no override for `floats`).
fn resolve_port_row(
    graph: &metal_graph::Graph,
    node_id: &str,
    port: &str,
    gen_rows: &HashMap<metal_graph::NodeId, Vec<f32>>,
    out: &HashMap<metal_graph::NodeId, Vec<f32>>,
    raw_digest: &[f32],
) -> Option<Vec<f32>> {
    let e = graph.edges.iter().find(|e| e.to.node == node_id && e.to.port == port)?;
    let from = graph.node(&e.from.node);
    let is_gen_row = from.map(|n| n.kind.starts_with("gen.")).unwrap_or(false) && e.from.port == "row";
    Some(if is_gen_row {
        gen_rows.get(&e.from.node).cloned().unwrap_or_default()
    } else if let Some(script_row) = out.get(&e.from.node) {
        script_row.clone()
    } else {
        raw_digest.to_vec()
    })
}

/// A script node's runtime and config, read live off `Node.config`
/// (regla 4). `None` for a config that is not a script's, after logging it
/// once — see [`evaluate`].
fn node_config(node: &metal_graph::Node) -> Option<(Option<Lang>, ScriptConfig)> {
    let lang = Lang::from_kind(&node.kind);
    if lang.is_none() && !crate::script_preset::is_preset(&node.kind) {
        return None;
    }
    match serde_json::from_str::<ScriptConfig>(&node.config.to_json_str()) {
        Ok(mut c) => {
            if lang.is_none() && c.floats.is_empty() {
                // A preset added before it was seeded still has its knobs.
                c = crate::script_preset::seed_config(&node.kind)?;
            }
            Some((lang, c))
        }
        Err(_) if lang.is_none() => Some((lang, crate::script_preset::seed_config(&node.kind)?)),
        Err(e) => {
            log_once(&node.id, &format!("config is not a script config: {e}"));
            None
        }
    }
}

/// One script node against one input row, with the float values
/// [`evaluate_with`] resolved for it (declared, overridden positionally by a
/// wired `floats` row — `S.AL.3`, [`ScriptConfig::values_with_override`] —
/// then by name by a binding). `None` for empty code and on any error, after
/// logging it once — see [`evaluate`].
fn run_node(
    node: &metal_graph::Node,
    lang: Lang,
    config: &ScriptConfig,
    values: &[f32],
    row: &[f32],
    ctx: &ScriptCtx,
) -> Option<Vec<f32>> {
    if config.code.trim().is_empty() {
        return None;
    }
    match run_lang_ctx(lang, &config.code, values, row, ctx) {
        Ok(row) => Some(row),
        Err(e) => {
            log_once(&node.id, &e.to_string());
            None
        }
    }
}

/// `eprintln!` a script node's error the first time *this* message is seen
/// for *this* node, and stay quiet while it repeats — the render loop calls
/// [`evaluate`] every frame, and a persistent error would otherwise be a
/// persistent log flood. A different message (the user edited the script
/// and it now fails differently) prints again.
fn log_once(node: &str, message: &str) {
    static LAST: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let last = LAST.get_or_init(|| Mutex::new(HashMap::new()));
    let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
    if last.get(node).map(|m| m == message).unwrap_or(false) {
        return;
    }
    eprintln!("  script {node}: {message}");
    last.insert(node.to_string(), message.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This crate's own `data/agent/shots.json` shot (`script-node-body-side-mask`)
    /// narrates the same scenario at the AI-tool level: a body's x position
    /// decides which side and how wide a mask is. This test is that scenario
    /// run for real, one script processing one row of floats.
    #[test]
    fn a_script_node_turns_a_body_position_into_a_side_and_width() {
        let code = r#"
def process(floats, row):
    x = row[0]
    threshold = floats[0]
    side = 1.0 if x > 0.5 else 0.0
    width = max(abs(x - 0.5) * 2.0, threshold)
    return [side, width]
"#;
        let floats = [0.2f32];
        let row = [0.8f32];
        let out = run(code, &floats, &row).expect("well-behaved script must not error");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], 1.0, "0.8 is right of center");
        assert!((out[1] - 0.6).abs() < 1e-4, "width = |0.8-0.5|*2 = 0.6, got {}", out[1]);
    }

    #[test]
    fn a_script_that_only_reads_its_own_floats_ignores_the_row() {
        let code = "def process(floats, row):\n    return [floats[0] + floats[1]]\n";
        let out = run(code, &[1.5, 2.5], &[]).unwrap();
        assert_eq!(out, vec![4.0]);
    }

    #[test]
    fn a_hung_script_is_cut_off_without_stalling_or_panicking() {
        let code = "import time\ndef process(floats, row):\n    time.sleep(30)\n    return [0.0]\n";
        let start = Instant::now();
        let err = run_with_timeout(code, &[], &[], Duration::from_millis(100)).unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(2), "must return promptly, not after the 30s sleep");
        assert_eq!(err, ScriptError::Timeout(Duration::from_millis(100)));
        // The worker was killed and dropped; the same code respawns lazily
        // and the next call is again a normal, bounded `Result`.
        let again = run_with_timeout(code, &[], &[], Duration::from_millis(100)).unwrap_err();
        assert_eq!(again, ScriptError::Timeout(Duration::from_millis(100)));
    }

    #[test]
    fn an_infinite_loop_is_also_cut_off() {
        let code = "def process(floats, row):\n    while True:\n        pass\n";
        let start = Instant::now();
        let err = run_with_timeout(code, &[], &[], Duration::from_millis(100)).unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(err, ScriptError::Timeout(Duration::from_millis(100)));
    }

    #[test]
    fn a_script_that_raises_returns_a_normal_result_not_a_panic() {
        let code = "def process(floats, row):\n    raise ValueError('boom')\n";
        let err = run(code, &[], &[]).unwrap_err();
        match err {
            ScriptError::Raised(msg) => assert!(msg.contains("ValueError") && msg.contains("boom"), "{msg}"),
            other => panic!("expected Raised, got {other:?}"),
        }
    }

    #[test]
    fn a_script_missing_process_is_a_raised_error_not_a_panic() {
        let code = "x = 1\n";
        let err = run(code, &[], &[]).unwrap_err();
        match err {
            ScriptError::Raised(msg) => assert!(msg.contains("NameError"), "{msg}"),
            other => panic!("expected Raised, got {other:?}"),
        }
    }

    /// A user `print()` must not corrupt the wire: the harness points the
    /// script's stdout at stderr and keeps the real stdout for replies.
    #[test]
    fn a_script_that_prints_still_answers_on_the_wire() {
        let code = "def process(floats, row):\n    print('debugging', row)\n    return [1.0]\n";
        assert_eq!(run(code, &[], &[0.5]).unwrap(), vec![1.0]);
    }

    // --- real time: the same worker answers frame after frame ---

    /// The per-frame fit: after the first call has paid the spawn, a call
    /// is a pipe round-trip. Ten more calls must cost less than one fresh
    /// `python3` spawn each (measured at ~10ms; the bar here is generous so
    /// a loaded CI box does not flake, while still ruling out a respawn).
    #[test]
    fn repeated_calls_reuse_the_worker_instead_of_respawning() {
        let code = "def process(floats, row):\n    return [row[0] * 2.0]\n";
        assert_eq!(run(code, &[], &[1.0]).unwrap(), vec![2.0]);
        let start = Instant::now();
        for i in 0..10 {
            assert_eq!(run(code, &[], &[i as f32]).unwrap(), vec![i as f32 * 2.0]);
        }
        let per_call = start.elapsed() / 10;
        assert!(per_call < Duration::from_millis(8), "a warm call took {per_call:?} — that is a respawn, not a round-trip");
    }

    /// Regla 4: editing the code is a different worker, so the new code
    /// answers immediately — never the old process with the old function.
    #[test]
    fn an_edited_script_is_a_new_worker_with_the_new_code() {
        let v1 = "def process(floats, row):\n    return [1.0]\n";
        let v2 = "def process(floats, row):\n    return [2.0]\n";
        assert_eq!(run(v1, &[], &[]).unwrap(), vec![1.0]);
        assert_eq!(run(v2, &[], &[]).unwrap(), vec![2.0]);
        assert_eq!(run(v1, &[], &[]).unwrap(), vec![1.0]);
    }

    // --- S.G: JavaScript runtime, same three verification cases as O.6 ---

    #[test]
    fn a_script_node_in_js_turns_a_body_position_into_a_side_and_width() {
        let code = r#"
function process(floats, row) {
    var x = row[0];
    var threshold = floats[0];
    var side = x > 0.5 ? 1.0 : 0.0;
    var width = Math.max(Math.abs(x - 0.5) * 2.0, threshold);
    return [side, width];
}
"#;
        let floats = [0.2f32];
        let row = [0.8f32];
        let out = run_js(code, &floats, &row).expect("well-behaved script must not error");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], 1.0, "0.8 is right of center");
        assert!((out[1] - 0.6).abs() < 1e-4, "width = |0.8-0.5|*2 = 0.6, got {}", out[1]);
        // Warm: the same `node` answers the next frame.
        let out = run_js(code, &floats, &[0.2]).unwrap();
        assert_eq!(out[0], 0.0, "0.2 is left of center");
    }

    #[test]
    fn a_hung_js_script_is_cut_off_without_stalling_or_panicking() {
        let code = "function process(floats, row) { while (true) {} }\n";
        let start = Instant::now();
        let err = run_js_with_timeout(code, &[], &[], Duration::from_millis(100)).unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(2), "must return promptly, not hang forever");
        assert_eq!(err, ScriptError::Timeout(Duration::from_millis(100)));
    }

    #[test]
    fn a_js_script_that_throws_returns_a_normal_result_not_a_panic() {
        let code = "function process(floats, row) { throw new Error('boom'); }\n";
        let err = run_js(code, &[], &[]).unwrap_err();
        match err {
            ScriptError::Raised(msg) => assert!(msg.contains("boom"), "{msg}"),
            other => panic!("expected Raised, got {other:?}"),
        }
    }

    #[test]
    fn a_js_script_missing_process_is_a_raised_error_not_a_panic() {
        let code = "var x = 1;\n";
        let err = run_js(code, &[], &[]).unwrap_err();
        match err {
            ScriptError::Raised(msg) => assert!(msg.contains("process"), "{msg}"),
            other => panic!("expected Raised, got {other:?}"),
        }
    }

    // --- S.G: Rust runtime ---
    //
    // Rust has no exceptions, so the "it raises" case from O.6 splits in
    // two here: code that fails to *compile* (`ScriptError::CompileFailed`,
    // the new failure mode `S.G` adds — "the node is not accepted if it
    // does not compile") and code that compiles but *panics* at run time
    // (`ScriptError::Raised`, the closest Rust equivalent of a raised
    // Python exception). Both are covered below, and both are a normal
    // `Result`, never a panic reaching this crate's own test process.

    #[test]
    fn a_script_node_in_rust_turns_a_body_position_into_a_side_and_width() {
        let code = r#"
fn process(floats: &[f32], row: &[f32]) -> Vec<f32> {
    let x = row[0];
    let threshold = floats[0];
    let side = if x > 0.5 { 1.0 } else { 0.0 };
    let width = ((x - 0.5).abs() * 2.0).max(threshold);
    vec![side, width]
}
"#;
        let floats = [0.2f32];
        let row = [0.8f32];
        let out = run_rust(code, &floats, &row).expect("well-behaved script must not error");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], 1.0, "0.8 is right of center");
        assert!((out[1] - 0.6).abs() < 1e-4, "width = |0.8-0.5|*2 = 0.6, got {}", out[1]);
    }

    #[test]
    fn a_hung_rust_script_is_cut_off_without_stalling_or_panicking() {
        let code = "fn process(floats: &[f32], row: &[f32]) -> Vec<f32> { loop {} }\n";
        let start = Instant::now();
        let err = run_rust_with_timeout(code, &[], &[], Duration::from_millis(100)).unwrap_err();
        assert!(
            start.elapsed() < RUST_COMPILE_TIMEOUT + Duration::from_secs(2),
            "must return promptly after compiling, not loop forever"
        );
        assert_eq!(err, ScriptError::Timeout(Duration::from_millis(100)));
    }

    #[test]
    fn a_rust_script_that_panics_returns_a_normal_result_not_a_process_panic() {
        let code = "fn process(floats: &[f32], row: &[f32]) -> Vec<f32> { if row.is_empty() { panic!(\"boom\") } vec![row[0]] }\n";
        let err = run_rust(code, &[], &[]).unwrap_err();
        match err {
            ScriptError::Raised(msg) => assert!(msg.contains("boom"), "{msg}"),
            other => panic!("expected Raised, got {other:?}"),
        }
        // The worker survived its own panic and answers the next frame.
        assert_eq!(run_rust(code, &[], &[3.0]).unwrap(), vec![3.0]);
    }

    #[test]
    fn a_rust_script_that_fails_to_compile_is_a_compile_error_not_a_panic() {
        // No top-level `process` — the harness's `main` fails to resolve
        // the call, which is `rustc`'s own diagnostic, never a Rust panic
        // in this crate's process.
        let code = "let unused = 1;\n";
        let err = run_rust(code, &[], &[]).unwrap_err();
        match err {
            ScriptError::CompileFailed(msg) => {
                assert!(msg.contains("process") || msg.contains("error"), "{msg}")
            }
            other => panic!("expected CompileFailed, got {other:?}"),
        }
    }

    /// The binary cache: the first call compiles, the second with the same
    /// code does not invoke `rustc` at all — and if the cached binary
    /// disappears, the next call compiles again. Run with
    /// `--test-threads=1` (the crate's own rule): [`compile_count`] is
    /// process-wide.
    #[test]
    fn a_second_call_with_the_same_rust_code_does_not_recompile() {
        // Unique per run so a binary cached by an earlier run of this very
        // test cannot make the first call a cache hit.
        let code = format!(
            "fn process(floats: &[f32], row: &[f32]) -> Vec<f32> {{ let _nonce = {}u64; vec![row[0] + 1.0] }}\n",
            rand_seed()
        );
        let path = rust_binary_path(&code);
        let _ = metal_vfs::remove_file(&path);

        let before = compile_count();
        assert_eq!(run_rust(&code, &[], &[1.0]).unwrap(), vec![2.0]);
        assert_eq!(compile_count(), before + 1, "first call compiles");
        assert!(metal_vfs::is_file(&path), "the binary is cached at {}", path.display());

        assert_eq!(run_rust(&code, &[], &[2.0]).unwrap(), vec![3.0]);
        assert_eq!(compile_count(), before + 1, "second call is served from the cache");

        // Evict the worker and the file: the next call must recompile
        // rather than fail to spawn a binary that is not there.
        pool().lock().unwrap().workers.remove(&(Lang::Rust, hash_str(&code)));
        metal_vfs::remove_file(&path).unwrap();
        assert_eq!(run_rust(&code, &[], &[3.0]).unwrap(), vec![4.0]);
        assert_eq!(compile_count(), before + 2, "a vanished binary is compiled again");
        let _ = metal_vfs::remove_file(&path);
    }

    #[test]
    fn script_float_round_trips_through_config_json() {
        let cfg = ScriptConfig {
            code: "def process(floats, row):\n    return row\n".to_string(),
            floats: vec![ScriptFloat { name: "width".to_string(), min: 0.0, max: 1.0, default: 0.3, value: None }],
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: ScriptConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);
    }

    /// `S.AL.3`: an old graph saved before `value` existed still
    /// deserializes — `#[serde(default)]` reads the missing field as `None`,
    /// which [`ScriptConfig::values`] then reads as "use `default`".
    #[test]
    fn a_script_float_with_no_value_field_at_all_deserializes_as_none() {
        let json = r#"{"code":"","floats":[{"name":"w","min":0.0,"max":1.0,"default":0.3}]}"#;
        let cfg: ScriptConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.floats[0].value, None);
        assert_eq!(cfg.values(), vec![0.3]);
    }

    /// `S.AL.3`: `value` inside `[min, max]` wins over `default`, and the
    /// clamp is a no-op for an in-range value.
    #[test]
    fn values_prefers_an_in_range_live_value_over_the_default() {
        let cfg = ScriptConfig {
            code: String::new(),
            floats: vec![ScriptFloat { name: "w".into(), min: 0.0, max: 1.0, default: 0.2, value: Some(0.7) }],
        };
        assert_eq!(cfg.values(), vec![0.7]);
    }

    /// A live `value` outside `[min, max]` — however it got there — is still
    /// clamped at read time, defense in depth behind the write-side refusal
    /// (`metal_graph::script_floats::apply_floats_patch`).
    #[test]
    fn values_clamps_an_out_of_range_live_value() {
        let cfg = ScriptConfig {
            code: String::new(),
            floats: vec![ScriptFloat { name: "w".into(), min: 0.0, max: 1.0, default: 0.2, value: Some(5.0) }],
        };
        assert_eq!(cfg.values(), vec![1.0]);
    }

    /// No live `value` at all falls back to `default`, clamped the same way.
    #[test]
    fn values_falls_back_to_default_when_value_is_none() {
        let cfg = ScriptConfig {
            code: String::new(),
            floats: vec![ScriptFloat { name: "w".into(), min: 0.0, max: 1.0, default: 0.4, value: None }],
        };
        assert_eq!(cfg.values(), vec![0.4]);
    }

    // --- the engine hook: `evaluate` over a graph ---

    use metal_graph::{Domain, Endpoint, Graph};

    fn script_node(g: &mut Graph, id: &str, kind: &str, code: &str, floats: &[(&str, f32)]) -> metal_graph::NodeId {
        let nid = g.add_node(id, kind, [0.0, 0.0]);
        let cfg = ScriptConfig {
            code: code.to_string(),
            floats: floats
                .iter()
                .map(|(n, d)| ScriptFloat { name: n.to_string(), min: 0.0, max: 1.0, default: *d, value: None })
                .collect(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        g.node_mut(&nid).unwrap().config = metal_graph::Config::from_json_str(&json).unwrap();
        nid
    }

    /// A generator's `row` → a script's `control`: the script sees the row
    /// `drive::row` computed for that generator, and its own declared
    /// floats, read off `Node.config` — not a copy held anywhere else.
    #[test]
    fn evaluate_feeds_a_script_its_generator_row_and_its_own_floats() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let s = script_node(&mut g, "s", "script.py", "def process(floats, row):\n    return [row[0] + floats[0]]\n", &[("bias", 0.25)]);
        g.add_edge(Endpoint::new(&gen, "row"), Endpoint::new(&s, "control"));

        let mut rows = HashMap::new();
        rows.insert(gen.clone(), vec![0.5, 0.0]);
        let out = evaluate(&g, &rows, &[9.0]);
        assert_eq!(out.get(&s), Some(&vec![0.75]));
    }

    /// `S.AL.3`: a wired `floats` port wins over both `value` and `default`
    /// — the same "wired beats held" rule `metal_graph::effects::state_of`'s
    /// `Reactive` is for an `fx.*` node's `control` port. The row is mapped
    /// positionally (`row[i]` -> `floats[i]`) and clamped to that float's
    /// own `[min, max]`, whatever `value` or `default` say.
    #[test]
    fn evaluate_lets_a_wired_floats_port_override_both_value_and_default() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let s = g.add_node("s", "script.py", [1.0, 0.0]);
        let cfg = ScriptConfig {
            code: "def process(floats, row):\n    return [floats[0], floats[1]]\n".to_string(),
            floats: vec![
                ScriptFloat { name: "a".into(), min: 0.0, max: 1.0, default: 0.1, value: Some(0.9) },
                ScriptFloat { name: "b".into(), min: 0.0, max: 1.0, default: 0.2, value: None },
            ],
        };
        g.node_mut(&s).unwrap().config =
            metal_graph::Config::from_json_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        g.add_edge(Endpoint::new(&gen, "row"), Endpoint::new(&s, "floats"));

        let mut rows = HashMap::new();
        // 1.5 is out of [0,1] on purpose: the override must still clamp.
        rows.insert(gen.clone(), vec![1.5, 0.4]);
        let out = evaluate(&g, &rows, &[]);
        assert_eq!(
            out.get(&s),
            Some(&vec![1.0, 0.4]),
            "the wired row overrides both `value` (0.9) and `default` (0.1/0.2), clamped"
        );
    }

    /// Any non-`gen.*` control source is the raw digest — the same
    /// axis-(b) reading `metal_graph::effects::control_source` gives an
    /// `fx.*` node — and a script feeding a script sees the first one's
    /// output, whichever order the nodes were added in.
    #[test]
    fn evaluate_chains_scripts_and_reads_raw_ingest_from_sensors() {
        let mut g = Graph::new("g", "t", Domain::Live);
        // Added first on purpose: `b` depends on `a`, which comes later.
        let b = script_node(&mut g, "b", "script.js", "function process(floats, row) { return [row[0] * 10]; }", &[]);
        let sensor = g.add_node("sensor", "sensor.yolo", [0.0, 0.0]);
        let a = script_node(&mut g, "a", "script.py", "def process(floats, row):\n    return [row[1]]\n", &[]);
        g.add_edge(Endpoint::new(&sensor, "control"), Endpoint::new(&a, "control"));
        g.add_edge(Endpoint::new(&a, "control"), Endpoint::new(&b, "control"));

        let out = evaluate(&g, &HashMap::new(), &[0.1, 0.3]);
        assert!((out[&a][0] - 0.3).abs() < 1e-5);
        assert!((out[&b][0] - 3.0).abs() < 1e-4);
    }

    /// A binding sets one float by name, mapped onto its range, after the
    /// declared value; a script bound to another script waits for it, in
    /// whatever order the nodes were added.
    #[test]
    fn evaluate_lets_a_binding_set_one_float_by_name_in_dependency_order() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let consumer = g.add_node("b", "script.py", [1.0, 0.0]);
        let cfg = ScriptConfig {
            code: "def process(floats, row):\n    return [floats[0], floats[1], floats[2]]\n".to_string(),
            floats: vec![
                ScriptFloat { name: "a".into(), min: 0.0, max: 1.0, default: 0.1, value: None },
                ScriptFloat { name: "b".into(), min: 0.0, max: 2.0, default: 0.2, value: Some(0.3) },
                ScriptFloat { name: "c".into(), min: 0.0, max: 1.0, default: 0.4, value: None },
            ],
        };
        g.node_mut(&consumer).unwrap().config =
            metal_graph::Config::from_json_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        let producer = script_node(&mut g, "a", "script.js", "function process(floats, row) { return [0.25]; }", &[]);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        g.add_edge(Endpoint::new(&audio, "control.onset"), Endpoint::new(&consumer, "param:b"));
        g.add_edge(Endpoint::new(&producer, "control.out0"), Endpoint::new(&consumer, "param:c"));

        let mut raw = [0.0f32; 16];
        raw[2] = 0.5;
        let out = evaluate_with(&g, &HashMap::new(), &raw, &crate::bind::GenParams::new());
        let got = &out.rows[&consumer];
        assert!((got[0] - 0.1).abs() < 1e-6, "unbound: its declared value");
        assert!((got[1] - 1.0).abs() < 1e-6, "onset 0.5 across b's 0..2");
        assert!((got[2] - 0.25).abs() < 1e-6, "the other script's output, after it ran");
        assert_eq!(out.floats[&consumer].len(), 3, "what it was handed is reported");
    }

    /// A script that errors contributes no row and takes nothing else down:
    /// the other script in the same graph still answers.
    #[test]
    fn evaluate_skips_a_failing_script_and_keeps_the_rest() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let bad = script_node(&mut g, "bad", "script.py", "def process(floats, row):\n    raise RuntimeError('no')\n", &[]);
        let good = script_node(&mut g, "good", "script.py", "def process(floats, row):\n    return [7.0]\n", &[]);
        let bypassed = script_node(&mut g, "off", "script.py", "def process(floats, row):\n    return [1.0]\n", &[]);
        g.node_mut(&bypassed).unwrap().bypass = true;

        let out = evaluate(&g, &HashMap::new(), &[]);
        assert!(!out.contains_key(&bad));
        assert!(!out.contains_key(&bypassed), "a bypassed script is not run");
        assert_eq!(out.get(&good), Some(&vec![7.0]));
    }

    #[test]
    fn scripts_read_the_musical_ctx_in_every_runtime() {
        let ctx = ScriptCtx::from_beats(6.5, 140.0, 1.0);
        let py = "def process(floats, row):\n    return [ctx['bpm'], ctx['beats'], ctx['beat_in_bar']]";
        let js = "function process(floats, row) { return [ctx.bpm, ctx.beats, ctx.beat_in_bar]; }";
        let rs = "fn process(_f: &[f32], _r: &[f32]) -> Vec<f32> { let c = ctx(); vec![c.bpm, c.beats as f32, c.beat_in_bar as f32] }";
        for (lang, code) in [(Lang::Python, py), (Lang::JavaScript, js), (Lang::Rust, rs)] {
            let out = run_lang_ctx(lang, code, &[], &[], &ctx).unwrap();
            assert_eq!(out, vec![140.0, 6.5, 2.0], "{lang:?}");
        }
        let next = ScriptCtx::from_beats(6.5, 90.0, 1.0);
        assert_eq!(run_lang_ctx(Lang::Python, py, &[], &[], &next).unwrap()[0], 90.0, "a changed bpm reaches the same worker");
    }
}
