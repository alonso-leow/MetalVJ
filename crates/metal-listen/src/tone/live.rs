//! The live inference thread — step 8 / decision D7.
//!
//! The 32 Hz digest must never wait on a model. So [`Runner`] puts the model on
//! its own thread: [`Runner::push`] only appends to a ring buffer and wakes the
//! worker, and the worker runs Basic Pitch, feeds the shared [`Reducer`], and
//! latches the result. The digester reads the latest latch once per row and
//! moves on; if inference is late, the row carries the previous value and
//! `tone_conf` decays.
//!
//! # The hop
//!
//! The worker infers once it has accumulated [`HOP`] = 2 048 fresh samples
//! (~93 ms). That is the latency budget: one hop plus the model's own context.
//! `the tone design notes` §7 allows raising it to 186 ms ([`HOP_LONG`]) if the step-3
//! benchmark says the CPU cannot keep up next to the renderer — colour tolerates
//! the delay. The constant is here so that change is one line.
//!
//! # Idle is silence, and silence costs nothing
//!
//! The capture stream delivers blocks even when nothing is playing (a monitor of
//! a silent output is all zeros). Running Basic Pitch over that is pure waste, so
//! the worker keeps an [`IdleGate`]: once a whole window of audio is silent it
//! stops inferring, clears the latch (the tone block reads zero, as it does with
//! no sound), and [`Runner::push`] drops further silent blocks before they reach
//! the channel. The worker then blocks in `recv` with no wakeups at all. Dropping
//! silence is exact: the window is all silence either way, so the first window
//! after sound returns has the same contents as before.
//!
//! # Sample mode: look now and then, not constantly
//!
//! Measured 2026-10-29, even [`LIVE_HOP`] holds ~20% of a core. In
//! [`Mode::Sample`] the worker infers once per period (`tone_sample_s`, 4 s by
//! default): a [`SampleGate`] lets only the last window's worth of each period
//! reach the channel, the worker runs ONE inference over it, latches the result
//! (it holds until the next sample) and sleeps in `recv` again. Between samples
//! [`Runner::push`] drops the audio before it is even copied. [`Mode::Off`]
//! starts no runner at all (see [`crate::tone::Tone::set_mode`]).
//!
//! # No model, no thread
//!
//! [`Runner::start`] returns `None` when the path is missing or will not load,
//! and spawns nothing. There is no fake inference anywhere in this file.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use web_time::Instant;

use crate::tone::basic_pitch::{self, AUDIO_N_SAMPLES, FFT_HOP};
use crate::tone::reducer::Reducer;
use crate::tone::Mode;

/// Samples between inferences: ~93 ms at 22 050 Hz. See the module doc.
pub const HOP: usize = 2_048;
/// The raised hop of §7, ~186 ms, if the benchmark is poor.
pub const HOP_LONG: usize = 4_096;
/// The hop the live worker uses. Measured 2026-10-29 (`scripts/perf-scenarios.sh`):
/// at [`HOP`] the worker held ~30% of a core all the time, one full ~2 s window
/// re-inferred ~11 times a second; harmony moves slower than that, so live
/// runs at [`HOP_LONG`] (half the cost).
pub const LIVE_HOP: usize = HOP_LONG;
/// A block whose RMS is under this (-60 dBFS) is silence for the idle gate: a
/// real input's noise floor sits around -70..-60 dBFS and never meets the
/// per-sample [`SILENT_PEAK`], so without this the worker never idled.
const SILENT_RMS: f32 = 1e-3;
/// A block whose every sample is below this magnitude (-80 dBFS) is silence.
/// Digital silence is exactly zero; this only has to sit above a zero-level
/// resampler or dither floor, and far below anything music does.
const SILENT_PEAK: f32 = 1e-4;

/// True when no sample in `samples` reaches [`SILENT_PEAK`].
fn is_silent(samples: &[f32]) -> bool {
    if samples.iter().all(|s| s.abs() < SILENT_PEAK) {
        return true;
    }
    let n = samples.len().max(1) as f32;
    (samples.iter().map(|s| s * s).sum::<f32>() / n).sqrt() < SILENT_RMS
}

/// Decides when the worker is idle: a full window of consecutive silence.
#[derive(Default)]
struct IdleGate {
    silent: usize,
    idle: bool,
}

impl IdleGate {
    /// Account for one block; returns whether the worker is now idle.
    fn observe(&mut self, block: &[f32]) -> bool {
        if is_silent(block) {
            self.silent = self.silent.saturating_add(block.len());
        } else {
            self.silent = 0;
        }
        self.idle = self.silent >= AUDIO_N_SAMPLES;
        self.idle
    }
}

/// Decides which samples of the stream a `sample`-mode worker gets: the last
/// `window` samples of every `period`, nothing in between. Pure counting, no
/// clock, so the cadence is testable without real time.
struct SampleGate {
    period: usize,
    window: usize,
    /// Position inside the current period.
    pos: usize,
}

impl SampleGate {
    /// A period shorter than one window would have no gap, so it is raised to
    /// the window. The first window opens at once, so the first sample lands
    /// one window after start instead of one period.
    fn new(period: usize, window: usize) -> SampleGate {
        let period = period.max(window).max(1);
        SampleGate { period, window, pos: period - window }
    }

    /// Account for a block of `len` samples; returns the sub-range of it to
    /// forward (empty while between samples). A block longer than the gap
    /// forwards only the window that ends inside it first; blocks are a few
    /// hundred ms, far under the gap.
    fn admit(&mut self, len: usize) -> std::ops::Range<usize> {
        let open = self.period - self.window;
        let lo = open.saturating_sub(self.pos).min(len);
        let hi = (self.period - self.pos).min(len);
        self.pos = (self.pos + len) % self.period;
        lo..hi.max(lo)
    }
}

/// What the worker publishes and the digester reads.
struct Latch {
    cols: [f32; metal_schema::TONE],
    latency_ms: f32,
    seq: u64,
}

/// The shared latch. A `Mutex` and not an atomic because the six floats cannot
/// be published as one word; the lock is held for a copy, so the digester never
/// blocks on inference, only on a memcpy.
struct Shared {
    latch: Mutex<Option<Latch>>,
    /// True while the worker has seen a full window of silence. Read by
    /// [`Runner::push`] so silent blocks never reach the worker in that state.
    idle: AtomicBool,
    /// True in [`Mode::Live`], false in [`Mode::Sample`]; read by the worker.
    live: AtomicBool,
    /// False when no on-air consumer reads tone: [`Runner::push`] then drops
    /// everything, so the worker sleeps. See `Tone::set_wanted`.
    wanted: AtomicBool,
    /// Samples per sample-mode period; a change rebuilds the gate in `push`.
    period: AtomicUsize,
}

/// The inference thread and the handles to talk to it.
pub struct Runner {
    tx: Sender<Vec<f32>>,
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
    /// Sample-mode admission, behind a lock because `push` takes `&self`.
    sampler: Mutex<SampleGate>,
}

impl Runner {
    /// Load the model and spawn the worker, or `None` when the model is missing
    /// or unloadable (no thread is spawned).
    pub fn start(model_path: &Path) -> Option<Runner> {
        let model = metal_ml::Model::load_light(model_path).ok()?;
        let (tx, rx) = channel::<Vec<f32>>();
        let shared = Arc::new(Shared {
            latch: Mutex::new(None),
            idle: AtomicBool::new(false),
            live: AtomicBool::new(false),
            wanted: AtomicBool::new(true),
            period: AtomicUsize::new(AUDIO_N_SAMPLES),
        });
        let worker_shared = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("tone-infer".into())
            .spawn(move || worker(model, rx, worker_shared))
            .ok()?;
        let sampler = Mutex::new(SampleGate::new(AUDIO_N_SAMPLES, AUDIO_N_SAMPLES));
        Some(Runner { tx, shared, handle: Some(handle), sampler })
    }

    /// Switch between `Sample` and `Live` (`Off` is [`crate::tone::Tone`]'s
    /// job: it drops the runner) and set the sample period.
    pub fn configure(&self, mode: Mode, sample_s: f32) {
        self.shared.live.store(mode == Mode::Live, Ordering::Relaxed);
        let period = (sample_s.max(0.0) * crate::SR as f32) as usize;
        self.shared.period.store(period.max(AUDIO_N_SAMPLES), Ordering::Relaxed);
    }

    /// Whether any consumer reads tone; false makes [`Runner::push`] drop all.
    pub fn set_wanted(&self, wanted: bool) {
        self.shared.wanted.store(wanted, Ordering::Relaxed);
    }

    /// Append samples to the shared ring buffer and wake the worker.
    ///
    /// Non-blocking: the channel is unbounded, so this only copies and sends.
    /// A dropped worker (channel closed) is ignored rather than panicking — the
    /// digester keeps running.
    pub fn push(&self, samples: &[f32]) {
        if !self.shared.wanted.load(Ordering::Relaxed) {
            return;
        }
        let samples = if self.shared.live.load(Ordering::Relaxed) {
            samples
        } else {
            // Sample mode: only the sampled window's worth goes on.
            let Ok(mut gate) = self.sampler.lock() else { return };
            let period = self.shared.period.load(Ordering::Relaxed);
            if gate.period != period.max(gate.window) {
                *gate = SampleGate::new(period, AUDIO_N_SAMPLES);
            }
            let range = gate.admit(samples.len());
            &samples[range]
        };
        if samples.is_empty() {
            return;
        }
        // While idle the worker's window is already all silence, so a silent
        // block would change nothing; skip the send and let the worker sleep.
        if self.shared.idle.load(Ordering::Relaxed) && is_silent(samples) {
            return;
        }
        let _ = self.tx.send(samples.to_vec());
    }

    /// The latest `(columns, seq)`, or `None` before the first inference has
    /// landed. `seq` rises by one per inference, so a caller can tell a fresh
    /// result from the same one twice.
    pub fn latest(&self) -> Option<([f32; metal_schema::TONE], u64)> {
        let guard = self.shared.latch.lock().ok()?;
        guard.as_ref().map(|l| (l.cols, l.seq))
    }

    /// The last inference's wall time in ms, or 0.0 before the first one.
    pub fn latency_ms(&self) -> f32 {
        self.shared
            .latch
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|l| l.latency_ms))
            .unwrap_or(0.0)
    }

    /// Stop the thread: close the channel and wait for the worker to drain.
    pub fn join(self) {
        let Runner { tx, handle, .. } = self;
        drop(tx);
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

/// Nice the calling thread so inference yields to the UI and the renderer when
/// they compete for a core. Per-thread on Linux (`PRIO_PROCESS` takes a TID);
/// elsewhere it is a no-op. Failure is ignored: the thread simply keeps its
/// priority.
fn lower_priority() {
    #[cfg(target_os = "linux")]
    // SAFETY: `gettid` and `setpriority` take no pointers; the TID is this thread's.
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        let _ = libc::setpriority(libc::PRIO_PROCESS, tid, TONE_NICE);
    }
}

/// The nice value of the tone worker (0 is normal, 19 the lowest).
pub const TONE_NICE: i32 = 10;

/// The worker: own the model, buffer the newest window, infer on every hop.
fn worker(model: metal_ml::Model, rx: Receiver<Vec<f32>>, shared: Arc<Shared>) {
    lower_priority();
    let mut reducer = Reducer::new();
    let mut ring: VecDeque<f32> = VecDeque::with_capacity(AUDIO_N_SAMPLES * 2);
    let mut fresh = 0usize;
    let mut seq = 0u64;
    let mut gate = IdleGate::default();

    // Frames a live hop covers; the reducer is advanced by exactly the frames
    // that are new since the last inference.
    let frames_per_hop = (LIVE_HOP / FFT_HOP).max(1);

    while let Ok(block) = rx.recv() {
        ring.extend(block.iter().copied());
        fresh += block.len();
        while ring.len() > AUDIO_N_SAMPLES {
            ring.pop_front();
        }
        let was_idle = gate.idle;
        let idle = gate.observe(&block);
        shared.idle.store(idle, Ordering::Relaxed);
        if idle {
            if !was_idle {
                // Silence has settled: publish "no harmony", not the last chord.
                if let Ok(mut guard) = shared.latch.lock() {
                    *guard = None;
                }
            }
            fresh = 0;
            continue;
        }
        let live = shared.live.load(Ordering::Relaxed);
        if ring.len() < AUDIO_N_SAMPLES || fresh < if live { LIVE_HOP } else { AUDIO_N_SAMPLES } {
            continue;
        }
        fresh = 0;

        let window: Vec<f32> = ring.iter().copied().collect();
        let started = Instant::now();
        let Ok(frames) = basic_pitch::run_window(&model, &window) else {
            // A run failure leaves the previous latch in place: the row keeps
            // the last real value and conf decays. Nothing is fabricated.
            continue;
        };
        // Live advances the reducer by the hop's new frames; a sample is a
        // whole fresh window, so it starts the reducer over and feeds all of it.
        let newest = if live {
            basic_pitch::newest(&frames, frames_per_hop)
        } else {
            reducer.reset();
            basic_pitch::newest(&frames, usize::MAX)
        };
        for i in 0..newest.note.len() {
            reducer.push_frame(&newest.note[i], &newest.onset[i], 1.0 / basic_pitch::FPS);
        }
        seq += 1;
        let latch = Latch {
            cols: reducer.columns(),
            latency_ms: started.elapsed().as_secs_f32() * 1000.0,
            seq,
        };
        if let Ok(mut guard) = shared.latch.lock() {
            *guard = Some(latch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_idles_after_a_full_window_of_silence_and_wakes_on_sound() {
        let mut gate = IdleGate::default();
        let quiet = vec![0.0f32; HOP];
        let loud = vec![0.5f32; HOP];
        let blocks_to_idle = AUDIO_N_SAMPLES.div_ceil(HOP);
        for _ in 1..blocks_to_idle {
            assert!(!gate.observe(&quiet), "idle before a whole window was silent");
        }
        assert!(gate.observe(&quiet), "a full silent window must idle the worker");
        assert!(gate.observe(&quiet), "stays idle through more silence");
        assert!(!gate.observe(&loud), "one loud block wakes the worker");
        assert!(!gate.observe(&quiet), "a fresh silent block does not idle at once");
    }

    #[test]
    fn a_digital_zero_is_silence_and_music_is_not() {
        assert!(is_silent(&[0.0; 16]));
        assert!(is_silent(&[SILENT_PEAK * 0.5; 16]));
        assert!(!is_silent(&[0.0, 0.0, 0.01, 0.0]));
        let hiss: Vec<f32> = (0..512).map(|i| if i % 2 == 0 { 3e-4 } else { -3e-4 }).collect();
        assert!(is_silent(&hiss), "a -70 dBFS noise floor is silence");
    }

    #[test]
    fn sample_mode_forwards_one_window_per_period() {
        let period = AUDIO_N_SAMPLES * 2;
        let mut gate = SampleGate::new(period, AUDIO_N_SAMPLES);
        // Blocks of 4 divide both the window and the period exactly.
        let mut forwarded = Vec::new();
        for _ in 0..(period * 3 / 4) {
            forwarded.push(gate.admit(4).len());
        }
        let first_period: usize = forwarded[..period / 4].iter().sum();
        let total: usize = forwarded.iter().sum();
        assert_eq!(first_period, AUDIO_N_SAMPLES, "one window in the first period");
        assert_eq!(total, 3 * AUDIO_N_SAMPLES, "one window per period, nothing between");
        // The first window opens at once; the gap follows it.
        let mut gate = SampleGate::new(period, AUDIO_N_SAMPLES);
        assert_eq!(gate.admit(4), 0..4);
        let mut gate = SampleGate::new(period, AUDIO_N_SAMPLES);
        gate.admit(AUDIO_N_SAMPLES);
        assert!(gate.admit(4).is_empty(), "dropped between samples");
    }

    #[test]
    fn a_period_shorter_than_a_window_is_raised_to_one() {
        let mut gate = SampleGate::new(10, AUDIO_N_SAMPLES);
        assert_eq!(gate.admit(AUDIO_N_SAMPLES), 0..AUDIO_N_SAMPLES);
    }

    #[test]
    fn off_runs_no_inference_and_reads_zero() {
        let path = Path::new("data/models/basic-pitch/does-not-exist.onnx");
        let mut tone = crate::tone::Tone::with_mode(Some(path), Mode::Off, 4.0);
        tone.push(&vec![0.5f32; AUDIO_N_SAMPLES]);
        assert!(!tone.loaded(), "off must not start the worker");
        assert_eq!(tone.columns(), [0.0; metal_schema::TONE]);
    }

    #[test]
    fn a_missing_model_starts_no_thread() {
        // No model is committed (D9), so the honest path is `None` — and the
        // point is that `Tone` then stays inert rather than panicking.
        let path = Path::new("data/models/basic-pitch/does-not-exist.onnx");
        assert!(Runner::start(path).is_none(), "a missing model must not spawn a thread");
    }
}
