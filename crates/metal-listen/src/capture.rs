//! Getting the room — or the mix bus — into the digester.
//!
//! # What to listen to
//!
//! A microphone is the obvious input and the wrong default. This is a DJ tool:
//! it should hear **what is playing**, which on a PipeWire box is a *monitor*
//! source on the output device, not a capture device pointed at the room. A mic
//! adds the room's reverb, the crowd, and whatever the booth is resonating at,
//! and every one of those lands in the spectral columns as music that is not
//! being played.
//!
//! # Two backends, and why the obvious one is not enough
//!
//! **cpal cannot reach a PipeWire monitor.** This was measured, not assumed,
//! and it cost an afternoon:
//!
//! - `pactl list short sources` shows the monitor. `metal-dash`'s input selector
//!   already lists it, and stores its name.
//! - cpal enumerates *fifteen* input devices on the same machine and **not one
//!   of them is that monitor**. cpal speaks ALSA, and ALSA does not know
//!   PipeWire's monitors exist; its list is plugins and cards.
//! - The documented route is ALSA's `pulse` plugin, which takes its source from
//!   the `PULSE_SOURCE` environment variable. Opening it that way **hangs**, in
//!   cpal's config probe, with no error and no timeout. Confirmed by setting the
//!   variable in the shell before the process even starts: same hang, so it is
//!   the plugin and not this crate.
//!
//! So a monitor is captured by [`Backend::Pipe`] — `parec` writing raw floats to
//! a pipe — and everything else by cpal. Shelling out is already how this
//! repository talks to the audio stack (`metal-dash::devices` runs `pactl`,
//! `metal-ingest` spawns ffmpeg), so this is the house style rather than a retreat
//! from it.
//!
//! # Where the work happens
//!
//! Both backends do almost nothing on their own thread: fold to mono, append,
//! return. All the analysis is in [`Listener::poll`], on whatever thread is
//! asking for a frame.
//!
//! That inverts the usual rule about audio callbacks, and it is safe here for a
//! specific reason: **nothing is being played back**. These streams are
//! read-only, so a late buffer costs one stale frame of visuals rather than an
//! audible click.

use std::io::Read;
use std::process::{Child, ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;
use metal_schema::platform::Command;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::repeat::RepeatState;
use crate::{resample::Resampler, Compat, Digester, Row, TickRate, FEATURES};

/// Roughly two seconds at 48 kHz. A consumer that has stopped asking for
/// frames must not be able to grow this without bound.
const MAX_BACKLOG: usize = 96_000;

/// What the pipe backend asks for. 48 kHz because it is what PipeWire runs at
/// natively, so nothing resamples twice on the way here.
const PIPE_RATE: u32 = 48_000;
const PIPE_CHANNELS: u16 = 2;

/// What the worker has most recently worked out. Read by whoever asks.
#[derive(Clone, Copy)]
struct Analysis {
    latest: Row,
    beats: Option<f32>,
    bpm: Option<f32>,
    confidence: f32,
    repeat: Option<RepeatState>,
    /// Which beat mod 4 is the bar's "1" (`Downbeat::phase`), heard or tapped.
    downbeat: Option<u8>,
    /// Whether the Basic Pitch model actually loaded in the worker. `false`
    /// when the `tone` feature is off (the fields exist in every build so
    /// [`Listener::tone_loaded`] has one cheap, lock-free-to-read answer) or
    /// when `data/models/basic-pitch/model.onnx` is absent (D9). Honest, never
    /// "true because tone is configured".
    tone_loaded: bool,
    /// The worker's last inference wall time in ms; 0.0 before the first one
    /// or with no model. Carried here so the audio thread only ever does the
    /// memcpy of [`Analysis`], never a `Tone` query.
    tone_latency_ms: f32,
}

struct Shared {
    pcm: Mutex<Vec<f32>>,
    /// Signalled by [`Shared::feed`] so the worker sleeps until samples arrive
    /// instead of polling `pcm` on a timer.
    pcm_ready: Condvar,
    /// Samples thrown away because the worker fell behind. Reported rather than
    /// hidden: it is the difference between "the visuals look wrong" and "the
    /// visuals look wrong *and here is why*".
    dropped: AtomicU64,
    analysis: Mutex<Analysis>,
    running: AtomicBool,
    /// A "tap 1" asked for by the operator, consumed by the worker (which owns
    /// the `Digester`) on its next pass.
    tap_one: AtomicBool,
}

impl Shared {
    fn new() -> Arc<Shared> {
        Arc::new(Shared {
            pcm: Mutex::new(Vec::new()),
            pcm_ready: Condvar::new(),
            dropped: AtomicU64::new(0),
            analysis: Mutex::new(Analysis {
                latest: [0.0; FEATURES],
                beats: None,
                bpm: None,
                confidence: 0.0,
                repeat: None,
                downbeat: None,
                tone_loaded: false,
                tone_latency_ms: 0.0,
            }),
            running: AtomicBool::new(true),
            tap_one: AtomicBool::new(false),
        })
    }

    fn feed(&self, mono: &[f32]) {
        let Ok(mut buf) = self.pcm.lock() else { return };
        buf.extend_from_slice(mono);
        if buf.len() > MAX_BACKLOG {
            let excess = buf.len() - MAX_BACKLOG;
            buf.drain(..excess);
            self.dropped.fetch_add(excess as u64, Ordering::Relaxed);
        }
        self.pcm_ready.notify_one();
    }
}

/// What the operator picks from.
pub struct DeviceInfo {
    pub name: String,
    pub channels: u16,
    pub sample_rate: u32,
    /// A monitor of an output, rather than a microphone.
    pub monitor: bool,
    /// Reached by pipe rather than by cpal — see the module note.
    pub pulse: bool,
}

fn looks_like_a_monitor(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("monitor") || n.contains("loopback")
}

/// Is this what `pactl list short sources` calls a source, rather than
/// something ALSA would recognise?
fn is_pulse_source(name: &str) -> bool {
    name.contains('.') && (name.ends_with(".monitor") || name.starts_with("alsa_"))
}

/// Sources `pactl` knows about. Empty if PipeWire/PulseAudio is not running,
/// which is a true statement about the machine rather than an error.
fn pulse_sources() -> Vec<DeviceInfo> {
    let Ok(out) = Command::new("pactl").args(["list", "short", "sources"]).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let _index = cols.next()?;
            let name = cols.next()?.trim();
            if name.is_empty() {
                return None;
            }
            Some(DeviceInfo {
                monitor: looks_like_a_monitor(name),
                name: name.to_string(),
                channels: PIPE_CHANNELS,
                sample_rate: PIPE_RATE,
                pulse: true,
            })
        })
        .collect()
}

/// Everything that can be listened to, from both backends.
pub fn devices() -> Result<Vec<DeviceInfo>, String> {
    // Pulse first: on a PipeWire box these are the ones that matter, and the
    // monitor is only ever in this half of the list.
    let mut out = pulse_sources();

    let host = cpal::default_host();
    if let Ok(inputs) = host.input_devices() {
        for device in inputs {
            let name = device.name().unwrap_or_else(|_| "<unnamed>".into());
            let Ok(cfg) = device.default_input_config() else { continue };
            out.push(DeviceInfo {
                monitor: looks_like_a_monitor(&name),
                name,
                channels: cfg.channels(),
                sample_rate: cfg.sample_rate().0,
                pulse: false,
            });
        }
    }
    Ok(out)
}

/// The prefix a pick carries when it names one program's playback stream
/// (`app:<object.serial>`) rather than a source.
pub const APP_PREFIX: &str = "app:";

/// `parec` (or `pw-record`) writing raw floats into a pipe.
struct Pipe {
    child: Child,
    stop: Arc<AtomicBool>,
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // Without this the recorder outlives the dashboard and holds the
        // source open, which shows up much later as a device that cannot be
        // opened twice.
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Held, never read. Both variants are RAII handles: dropping them is what
/// stops capture, and that is their entire job. The compiler is right that
/// nothing reads the fields and wrong that they could therefore go.
#[allow(dead_code)]
enum Backend {
    Cpal(CpalOwner),
    Pipe(Pipe),
}

/// `S.U7b`: `cpal::Stream` is `!Send` by cpal's own design — it wraps
/// platform audio-callback state that some backends will not let cross a
/// thread boundary — which used to make [`Listener`] itself `!Send` (see
/// `metal-dash/src/main.rs`'s `Shared` doc comment for why that mattered). The
/// fix is not to smuggle the stream across a thread but to never let it
/// leave the one it was built on: [`Listener::open_cpal`] spawns this
/// dedicated thread, builds and plays the stream *there*, and this struct —
/// which `Listener` actually holds — keeps only the `Arc<AtomicBool>` stop
/// flag and the `JoinHandle`, both `Send`. Dropping it stops the flag and
/// joins, which is what drops the stream on its own thread and stops
/// playback, exactly as dropping the bare `cpal::Stream` used to.
struct CpalOwner {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for CpalOwner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub struct Listener {
    // Dropped last, and holding it is what keeps capture alive. A field that
    // looks unused is exactly the kind of thing that gets tidied away.
    _backend: Backend,
    shared: Arc<Shared>,
    worker: Option<std::thread::JoinHandle<()>>,
    name: String,
    rate: u32,
    channels: u16,
    via: &'static str,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::Relaxed);
        self.shared.pcm_ready.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The analysis thread.
///
/// # Why this is not done in `poll`
///
/// It was, and it was wrong in a way only the real app showed. `metal-dash` is a
/// blocking accept loop: it calls `poll` when an HTTP request arrives and never
/// otherwise. Close the browser tab, or simply leave it idle for ten seconds,
/// and nothing drained the capture buffer — which overflowed, and **dropped
/// half a million samples**, measured.
///
/// Dropping samples is not a dropped frame of video. It puts a *hole in the
/// middle of the audio*: the digester sees the two sides spliced together, so
/// the onset column invents a transient at the seam and the beat tracker, which
/// needs ten unbroken seconds to lock, never locks at all. In the first
/// end-to-end run the dashboard reported 0 BPM for exactly this reason.
///
/// So the analysis runs on its own clock, continuously, and `poll` became a
/// read of the latest result. Capture is a stream; how often somebody looks at
/// it is not the stream's business.
fn spawn_worker(shared: Arc<Shared>, rate: u32) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut resampler = Resampler::new(rate);
        // Live, not Librosa: seventy milliseconds is the difference between a
        // visual that hits with the kick and one that answers it.
        let mut digester = Digester::new(Compat::Live, TickRate::Base);
        // The live tone path (step 8): attach Basic Pitch before the first
        // sample. `attach_tone` loads the model on this worker's thread; if the
        // file is missing (`TONE_MODEL`, D9) it attaches a model-less Tone that
        // emits zeros, so the music and voice columns are untouched exactly as
        // before the block existed. Guarded so a `capture`-without-`tone` build
        // stays ONNX-free.
        #[cfg(feature = "tone")]
        digester.attach_tone(Some(std::path::Path::new(crate::TONE_MODEL)));
        let mut at_22050: Vec<f32> = Vec::new();
        let mut latest: Row = [0.0; FEATURES];

        while shared.running.load(Ordering::Relaxed) {
            let captured = {
                let Ok(mut buf) = shared.pcm.lock() else { break };
                while buf.is_empty() && shared.running.load(Ordering::Relaxed) {
                    // Woken by `feed` the moment a callback lands, so a row is
                    // never late by more than the callback period. The timeout
                    // only bounds how long a stop request can go unnoticed.
                    match shared.pcm_ready.wait_timeout(buf, Duration::from_millis(250)) {
                        Ok((guard, _)) => buf = guard,
                        // Poisoned: the same exit as a poisoned `lock` above.
                        Err(_) => return,
                    }
                }
                std::mem::take(&mut *buf)
            };

            if captured.is_empty() {
                continue;
            }

            at_22050.clear();
            resampler.process(&captured, &mut at_22050);
            // Follow the `tone_mode` setting live (and set_wanted(true): see
            // its doc, no consumer query exists yet).
            #[cfg(feature = "tone")]
            digester.sync_tone_mode();
            digester.push(&at_22050);
            while let Some(row) = digester.pop() {
                latest = row;
            }

            if shared.tap_one.swap(false, Ordering::Relaxed) {
                digester.tap_downbeat();
            }
            let tempo = digester.tempo();
            let repeat = digester.repeat();
            if let Ok(mut a) = shared.analysis.lock() {
                a.latest = latest;
                a.beats = tempo.beats();
                a.bpm = tempo.bpm();
                a.confidence = tempo.confidence();
                a.repeat = repeat;
                a.downbeat = digester.downbeat().phase();
                // The tone path's own status, read once per pass on the worker
                // thread and published with the row. `loaded()` is fixed after
                // `attach_tone`; the latency moves as inference runs. With the
                // feature off these stay at their `false`/0.0 defaults.
                #[cfg(feature = "tone")]
                if let Some(t) = digester.tone() {
                    a.tone_loaded = t.loaded();
                    a.tone_latency_ms = t.latency_ms();
                }
            }
        }
    })
}

impl Listener {
    /// Open an input.
    ///
    /// `pick` is either a PulseAudio source name — what `metal-dash`'s selector
    /// stores, and the only way to get a monitor — or a substring of a cpal
    /// device name. Without one, the default input is used, which on a laptop
    /// is the built-in microphone.
    pub fn open(pick: Option<&str>) -> Result<Listener, String> {
        match pick {
            // 2026-10-01: one program's own output stream, by PipeWire
            // `object.serial` (`metal_perform::devices` lists them as
            // `app:<serial>`). Only pw-record can target a stream node.
            Some(want) if want.starts_with(APP_PREFIX) => Listener::open_pipe(&want[APP_PREFIX.len()..], true),
            Some(want) if is_pulse_source(want) => Listener::open_pipe(want, false),
            _ => Listener::open_cpal(pick),
        }
    }

    /// The one that can hear a monitor.
    fn open_pipe(source: &str, stream_only: bool) -> Result<Listener, String> {
        let shared = Shared::new();
        let stop = Arc::new(AtomicBool::new(false));

        let rate = PIPE_RATE.to_string();
        let channels = PIPE_CHANNELS.to_string();
        // parec first — its raw mode is exactly specified. pw-record is the
        // fallback for a box with pipewire-utils but no pulse compatibility
        // tools.
        let attempts: [(&str, Vec<String>); 2] = [
            (
                "parec",
                vec![
                    "--raw".into(),
                    "-d".into(),
                    source.into(),
                    "--format=float32le".into(),
                    format!("--rate={rate}"),
                    format!("--channels={channels}"),
                    // Small enough that a visual answers the music, large
                    // enough not to wake this process thousands of times a
                    // second.
                    "--latency-msec=20".into(),
                ],
            ),
            (
                "pw-record",
                vec![
                    "--raw".into(),
                    "--target".into(),
                    source.into(),
                    "--format=f32".into(),
                    format!("--rate={rate}"),
                    format!("--channels={channels}"),
                    "--latency=20ms".into(),
                    "-".into(),
                ],
            ),
        ];

        let mut last_err = String::new();
        let mut spawned: Option<(Child, &'static str)> = None;
        for (program, args) in attempts {
            if stream_only && program == "parec" {
                continue;
            }
            match Command::new(program)
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => {
                    spawned = Some((child, if program == "parec" { "parec" } else { "pw-record" }));
                    break;
                }
                Err(e) => last_err = format!("{program}: {e}"),
            }
        }
        let (mut child, via) = spawned.ok_or_else(|| {
            format!("cannot record {source:?} — no parec and no pw-record ({last_err})")
        })?;

        let stdout = child.stdout.take().ok_or("recorder produced no pipe")?;
        spawn_reader(stdout, Arc::clone(&shared), PIPE_CHANNELS as usize, Arc::clone(&stop));

        let worker = spawn_worker(Arc::clone(&shared), PIPE_RATE);
        Ok(Listener {
            _backend: Backend::Pipe(Pipe { child, stop }),
            shared,
            worker: Some(worker),
            name: source.to_string(),
            rate: PIPE_RATE,
            channels: PIPE_CHANNELS,
            via,
        })
    }

    fn open_cpal(pick: Option<&str>) -> Result<Listener, String> {
        let host = cpal::default_host();
        let inputs: Vec<cpal::Device> =
            host.input_devices().map_err(|e| e.to_string())?.collect();
        if inputs.is_empty() {
            return Err("no audio input devices at all".into());
        }

        let device = match pick {
            Some(want) => {
                let lower = want.to_ascii_lowercase();
                inputs
                    .iter()
                    .find(|d| d.name().map(|n| n.to_ascii_lowercase() == lower).unwrap_or(false))
                    .or_else(|| {
                        inputs.iter().find(|d| {
                            d.name()
                                .map(|n| n.to_ascii_lowercase().contains(&lower))
                                .unwrap_or(false)
                        })
                    })
                    .cloned()
                    .ok_or_else(|| format!("no input device matching {want:?}"))?
            }
            None => host
                .default_input_device()
                .or_else(|| inputs.first().cloned())
                .ok_or("no default input device")?,
        };

        let name = device.name().unwrap_or_else(|_| "<unnamed>".into());
        let supported = device.default_input_config().map_err(|e| e.to_string())?;
        let format = supported.sample_format();
        let channels = supported.channels();
        let rate = supported.sample_rate().0;
        let config: cpal::StreamConfig = supported.into();

        let shared = Shared::new();
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

        // `S.U7b`: the stream is built, played and dropped entirely on this
        // thread — never on the caller's — because `cpal::Stream` is `!Send`.
        // `device` and `config` are `Send` (they are just device
        // description, not the running callback), so moving *them* here and
        // building the stream on arrival is the whole fix; nothing about the
        // 125 ms analysis window or the callback body below changed.
        let thread_shared = Arc::clone(&shared);
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("metal-listen cpal".into())
            .spawn(move || {
                // One closure body for every sample format, so a device that
                // hands over i16 cannot quietly take a different code path
                // from one that hands over f32.
                macro_rules! stream {
                    ($t:ty, $conv:expr) => {{
                        let sink = Arc::clone(&thread_shared);
                        let ch = channels as usize;
                        let mut mono: Vec<f32> = Vec::new();
                        device.build_input_stream(
                            &config,
                            move |data: &[$t], _: &cpal::InputCallbackInfo| {
                                let conv: fn($t) -> f32 = $conv;
                                mono.clear();
                                for frame in data.chunks_exact(ch) {
                                    let sum: f32 = frame.iter().map(|&s| conv(s)).sum();
                                    mono.push(sum / ch as f32);
                                }
                                sink.feed(&mono);
                            },
                            |e| eprintln!("metal-listen: capture error: {e}"),
                            None,
                        )
                    }};
                }

                let built = match format {
                    cpal::SampleFormat::F32 => stream!(f32, |s| s),
                    cpal::SampleFormat::I16 => stream!(i16, |s| s as f32 / 32768.0),
                    cpal::SampleFormat::U16 => stream!(u16, |s| (s as f32 - 32768.0) / 32768.0),
                    cpal::SampleFormat::I32 => stream!(i32, |s| s as f32 / 2147483648.0),
                    other => {
                        let _ = ready_tx.send(Err(format!("unsupported sample format {other:?}")));
                        return;
                    }
                };
                let stream = match built {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                if let Err(e) = stream.play() {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
                // Open succeeded — `Input::live` (`metal-dash`) is waiting on
                // this to fail loudly if it doesn't.
                let _ = ready_tx.send(Ok(()));

                while !thread_stop.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                // Explicit: this is what stops playback. `stream` would drop
                // here regardless, but the name says why the loop above
                // exists at all.
                drop(stream);
            })
            .map_err(|e| format!("cannot spawn capture thread: {e}"))?;

        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                stop.store(true, Ordering::Relaxed);
                let _ = thread.join();
                return Err(e);
            }
            Err(_) => {
                // The thread died before sending anything — a panic building
                // or playing the stream. `join` surfaces it to stderr via the
                // default panic hook; this call just needs to fail, loudly.
                let _ = thread.join();
                return Err("capture thread died before it could open the device".into());
            }
        }

        let worker = spawn_worker(Arc::clone(&shared), rate);
        Ok(Listener {
            _backend: Backend::Cpal(CpalOwner { stop, thread: Some(thread) }),
            shared,
            worker: Some(worker),
            name,
            rate,
            channels,
            via: "cpal",
        })
    }

    /// The newest 125 ms row.
    ///
    /// A read, not a computation — the analysis runs on its own thread. Asking
    /// faster than 8 Hz returns the same row, which is what the grid means, and
    /// **not** asking does not stop the digest.
    pub fn poll(&mut self) -> Row {
        self.shared.analysis.lock().map(|a| a.latest).unwrap_or([0.0; FEATURES])
    }

    /// Beats since the tracker locked on, or `None` while it has not.
    pub fn beats(&self) -> Option<f32> {
        self.shared.analysis.lock().ok().and_then(|a| a.beats)
    }

    /// The DJ's loop right now, or `None` when there is none.
    ///
    /// Carried through `Shared`/`Analysis` exactly like [`Listener::beats`]:
    /// a read of what the worker last worked out, not a computation on the
    /// asking thread.
    pub fn repeat(&self) -> Option<RepeatState> {
        self.shared.analysis.lock().ok().and_then(|a| a.repeat)
    }

    /// Which beat mod 4 of [`Listener::beats`]' count is the bar's "1", heard
    /// by the downbeat detector or set by [`Listener::tap_downbeat`].
    pub fn downbeat(&self) -> Option<u8> {
        self.shared.analysis.lock().ok().and_then(|a| a.downbeat)
    }

    /// "Tap 1": the beat nearest now becomes the bar's first. Applied by the
    /// analysis worker on its next pass (within ~10 ms while audio flows).
    pub fn tap_downbeat(&self) {
        self.shared.tap_one.store(true, Ordering::Relaxed);
    }

    pub fn bpm(&self) -> Option<f32> {
        self.shared.analysis.lock().ok().and_then(|a| a.bpm)
    }

    pub fn confidence(&self) -> f32 {
        self.shared.analysis.lock().map(|a| a.confidence).unwrap_or(0.0)
    }

    /// Whether Basic Pitch actually loaded in the worker, for whatever wants to
    /// report it (`metal-dash`'s `listen_status`). `false` in every build where
    /// the model is absent (D9) or the `tone` feature is off — the same honest
    /// "no model" the columns themselves carry.
    ///
    /// A read of the worker's last published status, not a computation on the
    /// calling thread: it locks the same `Mutex<Analysis>` every other accessor
    /// does, for the length of one field read.
    pub fn tone_loaded(&self) -> bool {
        self.shared.analysis.lock().map(|a| a.tone_loaded).unwrap_or(false)
    }

    /// The worker's last Basic Pitch inference wall time in ms, or 0.0 before
    /// the first inference / with no model / with the `tone` feature off. Same
    /// cheap read as [`Listener::tone_loaded`].
    pub fn tone_latency_ms(&self) -> f32 {
        self.shared.analysis.lock().map(|a| a.tone_latency_ms).unwrap_or(0.0)
    }

    pub fn dropped(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }

    pub fn device(&self) -> &str {
        &self.name
    }

    /// What the operator should see next to the input selector.
    pub fn describe(&self) -> String {
        let mut s =
            format!("{} — {} ch at {} Hz via {}", self.name, self.channels, self.rate, self.via);
        if let Some(bpm) = self.bpm() {
            s.push_str(&format!(", {bpm:.0} BPM"));
        }
        let dropped = self.dropped();
        if dropped > 0 {
            s.push_str(&format!(", {dropped} samples dropped"));
        }
        s
    }
}

fn spawn_reader(mut stdout: ChildStdout, shared: Arc<Shared>, channels: usize, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let frame_bytes = 4 * channels;
        let mut raw = vec![0u8; 16 * 1024];
        let mut carry: Vec<u8> = Vec::new();
        let mut mono: Vec<f32> = Vec::new();
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match stdout.read(&mut raw) {
                Ok(0) => break,
                Ok(n) => {
                    // A read can land mid-sample, so whole frames are consumed
                    // and the remainder waits for the next one. Getting this
                    // wrong shifts the channel interleave by a sample and every
                    // subsequent frame is half of one and half of the next.
                    carry.extend_from_slice(&raw[..n]);
                    let usable = carry.len() / frame_bytes * frame_bytes;
                    if usable == 0 {
                        continue;
                    }
                    mono.clear();
                    for frame in carry[..usable].chunks_exact(frame_bytes) {
                        let sum: f32 = frame
                            .chunks_exact(4)
                            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                            .sum();
                        mono.push(sum / channels as f32);
                    }
                    shared.feed(&mono);
                    carry.drain(..usable);
                }
                Err(_) => break,
            }
        }
    });
}

/// `S.U7b`: a build-time guarantee, not a runtime one — `metal-dash` needs
/// `Listener` to be `Send` to hold it inside `Input::Live` inside an
/// `Arc<Mutex<…>>` shared between its winit thread and its HTTP listener
/// thread. If a future change (a new `Backend` variant, a field that isn't
/// `Arc`/channel/`JoinHandle`) makes this `!Send` again, this function stops
/// compiling instead of the regression surfacing as a distant, confusing
/// trait-bound error three crates away.
#[allow(dead_code)]
fn _assert_listener_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Listener>();
}
