//! **V0 spike — measure, do not ship.** Voice presence on the corpus before
//! any of it is allowed near `Digester`.
//!
//! `the voice design notes` V0 asks for a number, not an opinion: does WebRTC VAD plus
//! YIN plus a speech-band test actually find the voice on a full mix, what does
//! it cost, and does `webrtc-vad` cross-build. This example decodes a song to
//! 16 kHz, runs the VAD at all four modes and an FFT-difference YIN at a 10 ms
//! hop, fuses them, and prints per-second timelines and CPU per second of
//! audio. With `--report` it scores itself against `data/voice-labels/*.toml`.
//!
//! It is deliberately a standalone spike. The real analyser lands in
//! `metal_listen::voice` (V1); nothing here is imported by product code.
//!
//! ```text
//! CARGO_TARGET_DIR=~/.cache/fw-targets/voice-v0v1 \
//!   cargo run -p metal-listen --features decode --example voice_probe -- --report
//! ```
//!
//! A spike, so a few style lints are silenced rather than contorted to fit the
//! linter's taste; the real analyser in `metal_listen::voice` is clippy-clean.
#![allow(
    clippy::needless_range_loop,
    clippy::unnecessary_cast,
    clippy::vec_init_then_push,
    clippy::same_item_push
)]
//!
//! Flags:
//!   `--report`          score against the labels (regenerates as labels fill in)
//!   `--timeline`        one line per second per song
//!   `--song <stem>`     restrict to a stem (repeatable)
//!   `--corpus <dir>`    audio dir (default `~/.cache/metalvj/data/audio`)
//!   `--labels <dir>`    label dir (default `data/voice-labels`)
//!   `--seconds <n>`     cap each song to `n` seconds
//!   `--anchor`          only run the native-frame anchor check and exit

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustfft::{num_complex::Complex, Fft, FftPlanner};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

// --- the voice path's own grid ---------------------------------------------
//
// Mirrors `the voice design notes` facts that shape the design: the digest is 22 050 Hz
// but the VAD only accepts 8/16/32/48 kHz, so the voice path has its own
// 16 kHz resample, and it still works on 10 ms frames whatever the tick rate.

const VOICE_SR: u32 = 16_000;
const FRAME_MS: usize = 10;
const FRAME_LEN: usize = (VOICE_SR as usize * FRAME_MS) / 1000; // 160
const YIN_W: usize = 640; // 40 ms difference window
const YIN_TAU_MIN: usize = (VOICE_SR as usize) / 1100; // 55 Hz ceiling -> 14
const YIN_TAU_MAX: usize = (VOICE_SR as usize) / 55; // 1100 Hz floor  -> 290
const YIN_THRESHOLD: f32 = 0.15;
const BAND_FFT: usize = 256;
const F0_MIN: f32 = 55.0;
const F0_MAX: f32 = 1100.0;
/// YIN periodicity above this counts as "voiced" for presence.
const VOICED_PERIODICITY: f32 = 0.45;
/// Speech-band (300–3400 Hz) share of the frame's power, above which a frame is
/// allowed to be voice.
const SPEECH_SHARE: f32 = 0.5;
/// Fraction of a tick's frames that must decide "voice" for the tick to say so.
const PRESENCE_FRAC: f32 = 0.3;

// --- YIN: FFT difference function, CMNDF, threshold --------------------------

/// The pitch detector `the voice design notes` decision 12 pins down: `rustfft`, a 40 ms
/// window, τ for 55–1100 Hz, absolute threshold 0.15, parabolic interpolation.
struct Yin {
    n: usize,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    buf: Vec<Complex<f32>>,
}

impl Yin {
    fn new() -> Yin {
        let want = YIN_W + YIN_TAU_MAX + 1;
        let mut n = 1;
        while n < want {
            n <<= 1;
        }
        let mut planner = FftPlanner::new();
        Yin {
            n,
            fft: planner.plan_fft_forward(n),
            ifft: planner.plan_fft_inverse(n),
            buf: vec![Complex::new(0.0, 0.0); n],
        }
    }

    /// `(f0, periodicity)` for the window `x` (length ≥ [`YIN_W`]).
    /// `(0.0, 0.0)` when the window is silent or unpitched.
    fn analyze(&mut self, x: &[f32]) -> (f32, f32) {
        let w = YIN_W.min(x.len());
        for c in self.buf.iter_mut() {
            *c = Complex::new(0.0, 0.0);
        }
        for i in 0..w {
            self.buf[i].re = x[i];
        }
        self.fft.process(&mut self.buf);
        for c in self.buf.iter_mut() {
            *c = Complex::new(c.norm_sqr(), 0.0);
        }
        self.ifft.process(&mut self.buf);

        // Suffix energy sum_{j=tau}^{w-1} x[j]^2, so d(tau) can be formed.
        let mut suffix = vec![0.0f32; w + 1];
        for j in (0..w).rev() {
            suffix[j] = suffix[j + 1] + x[j] * x[j];
        }
        let r0 = self.buf[0].re / self.n as f32;
        if r0 < 1e-9 {
            return (0.0, 0.0);
        }

        let mut d = vec![0.0f32; YIN_TAU_MAX + 1];
        for tau in 1..=YIN_TAU_MAX.min(w - 1) {
            if tau >= suffix.len() {
                break;
            }
            let r_tau = self.buf[tau].re / self.n as f32;
            d[tau] = (r0 + suffix[tau] - 2.0 * r_tau).max(0.0);
        }

        // Cumulative-mean-normalised difference.
        let mut running = 0.0f32;
        let mut cmndf = vec![1.0f32; YIN_TAU_MAX + 1];
        for tau in 1..=YIN_TAU_MAX {
            running += d[tau];
            if running > 1e-12 {
                cmndf[tau] = d[tau] * tau as f32 / running;
            } else {
                cmndf[tau] = 1.0;
            }
        }

        // First dip under the threshold past tau_min, else the global min.
        let lo = YIN_TAU_MIN.max(1);
        let mut found = None;
        for tau in lo..=YIN_TAU_MAX {
            if cmndf[tau] < YIN_THRESHOLD {
                found = Some(tau);
                break;
            }
        }
        let tau = found.unwrap_or_else(|| {
            (lo..=YIN_TAU_MAX)
                .min_by(|&a, &b| cmndf[a].total_cmp(&cmndf[b]))
                .unwrap_or(lo)
        });
        // Parabolic interpolation on d′ around the dip.
        let mut tau_f = tau as f32;
        if tau > lo && tau < YIN_TAU_MAX {
            let (a, b, c) = (cmndf[tau - 1], cmndf[tau], cmndf[tau + 1]);
            let denom = a - 2.0 * b + c;
            if denom.abs() > 1e-9 {
                tau_f += 0.5 * (a - c) / denom;
            }
        }
        let f0 = VOICE_SR as f32 / tau_f;
        let periodicity = (1.0 - cmndf[tau]).clamp(0.0, 1.0);
        if !(F0_MIN..=F0_MAX).contains(&f0) {
            return (f0, periodicity);
        }
        (f0, periodicity)
    }
}

// --- speech band share + hiss, off a small FFT -------------------------------

struct Bands {
    n: usize,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    buf: Vec<Complex<f32>>,
    /// Bin indices for 300..3400 and 4000..7900.
    speech_bins: Vec<usize>,
    hiss_bins: Vec<usize>,
    total_bins: Vec<usize>,
}

impl Bands {
    fn new() -> Bands {
        let n = BAND_FFT;
        let mut planner = FftPlanner::new();
        let window: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / (n - 1) as f32;
                0.5 - 0.5 * (std::f32::consts::TAU * t).cos()
            })
            .collect();
        let mut speech_bins = Vec::new();
        let mut hiss_bins = Vec::new();
        let mut total_bins = Vec::new();
        for k in 1..n / 2 {
            let hz = k as f32 * VOICE_SR as f32 / n as f32;
            if (300.0..=3400.0).contains(&hz) {
                speech_bins.push(k);
            }
            if (4000.0..=7900.0).contains(&hz) {
                hiss_bins.push(k);
            }
            if hz < VOICE_SR as f32 / 2.0 {
                total_bins.push(k);
            }
        }
        Bands {
            n,
            fft: planner.plan_fft_forward(n),
            window,
            buf: vec![Complex::new(0.0, 0.0); n],
            speech_bins,
            hiss_bins,
            total_bins,
        }
    }

    fn analyze(&mut self, x: &[f32]) -> (f32, f32) {
        let n = self.n.min(x.len());
        for (i, c) in self.buf.iter_mut().enumerate() {
            if i < n {
                *c = Complex::new(x[i] * self.window[i], 0.0);
            } else {
                *c = Complex::new(0.0, 0.0);
            }
        }
        self.fft.process(&mut self.buf);
        let mut total = 0.0f32;
        let mut speech = 0.0f32;
        let mut hiss = 0.0f32;
        for &k in &self.total_bins {
            total += self.buf[k].norm_sqr();
        }
        for &k in &self.speech_bins {
            speech += self.buf[k].norm_sqr();
        }
        for &k in &self.hiss_bins {
            hiss += self.buf[k].norm_sqr();
        }
        if total < 1e-9 {
            return (0.0, 0.0);
        }
        (speech / total, hiss / total)
    }
}

// --- one 10 ms voice frame --------------------------------------------------

#[derive(Clone, Copy)]
struct Frame {
    /// VAD decision per mode 0..3.
    vad: [bool; 4],
    f0: f32,
    periodicity: f32,
    /// speech-band power share, 0..1
    speech: f32,
    /// 4–8 kHz power share, 0..1
    hiss: f32,
    /// frame RMS in dBFS-ish, for a live readout later
    rms_db: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Method {
    Vad(usize),
    Yin,
    Fusion(usize),
}

impl Method {
    fn name(self) -> String {
        match self {
            Method::Vad(k) => format!("VAD m{k}"),
            Method::Yin => "YIN".into(),
            Method::Fusion(k) => format!("fusion m{k}"),
        }
    }
}

fn frame_presence(f: &Frame, m: Method) -> bool {
    let voiced = f.periodicity > VOICED_PERIODICITY && (F0_MIN..=F0_MAX).contains(&f.f0);
    match m {
        Method::Vad(k) => f.vad[k],
        Method::Yin => voiced,
        Method::Fusion(k) => f.vad[k] && voiced && f.speech > SPEECH_SHARE,
    }
}

// --- decode to mono f32 at 16 kHz -------------------------------------------
//
// A copy of `file::digest_file_with_tempo`'s decode loop, stopping before the
// digest: V0 must not touch product code, and the probe wants raw samples at
// 16 kHz rather than 22 050.

fn decode_mono_16k(path: &Path, max_seconds: Option<f32>) -> Result<(Vec<f32>, Duration), String> {
    let started = Instant::now();
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("not a readable audio file: {e}"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("no audio track in the file")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("unsupported codec: {e}"))?;

    let mut resampler: Option<metal_listen::resample::Resampler> = None;
    let mut interleaved: Option<SampleBuffer<f32>> = None;
    let (mut mono, mut resampled) = (Vec::new(), Vec::new());
    let mut out = Vec::new();
    let cap = max_seconds.map(|s| (s * VOICE_SR as f32) as usize);
    loop {
        if cap.is_some_and(|c| out.len() >= c) {
            break;
        }
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(format!("reading the file: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(format!("decoding: {e}")),
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        let buf = interleaved
            .get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, spec));
        if buf.capacity() < decoded.capacity() * channels {
            *buf = SampleBuffer::new(decoded.capacity() as u64, spec);
        }
        buf.copy_interleaved_ref(decoded);
        mono.clear();
        metal_listen::resample::to_mono(buf.samples(), channels, &mut mono);
        resampled.clear();
        resampler
            .get_or_insert_with(|| metal_listen::resample::Resampler::with_target(spec.rate, VOICE_SR))
            .process(&mono, &mut resampled);
        out.extend_from_slice(&resampled);
    }
    if resampler.is_none() {
        return Err("the file has no decodable audio".into());
    }
    if let Some(c) = cap {
        out.truncate(c);
    }
    Ok((out, started.elapsed()))
}

// --- the pass: frames, all four VAD modes -----------------------------------

fn analyse(audio: &[f32]) -> (Vec<Frame>, Duration) {
    let mut vad = [
        webrtc_vad::Vad::new_with_rate_and_mode(
            webrtc_vad::SampleRate::Rate16kHz,
            webrtc_vad::VadMode::Quality,
        ),
        webrtc_vad::Vad::new_with_rate_and_mode(
            webrtc_vad::SampleRate::Rate16kHz,
            webrtc_vad::VadMode::LowBitrate,
        ),
        webrtc_vad::Vad::new_with_rate_and_mode(
            webrtc_vad::SampleRate::Rate16kHz,
            webrtc_vad::VadMode::Aggressive,
        ),
        webrtc_vad::Vad::new_with_rate_and_mode(
            webrtc_vad::SampleRate::Rate16kHz,
            webrtc_vad::VadMode::VeryAggressive,
        ),
    ];
    let mut yin = Yin::new();
    let mut bands = Bands::new();
    let mut i16buf = vec![0i16; FRAME_LEN];
    let mut frames = Vec::with_capacity(audio.len() / FRAME_LEN + 1);

    let started = Instant::now();
    let mut at = 0;
    while at + FRAME_LEN <= audio.len() {
        let frame = &audio[at..at + FRAME_LEN];
        // VAD: i16, one 10 ms frame per call, four independent modes.
        let mut vad_bits = [false; 4];
        for (k, s) in frame.iter().enumerate() {
            i16buf[k] = (s * 32767.0).clamp(-32768.0, 32767.0) as i16;
        }
        for (m, v) in vad.iter_mut().enumerate() {
            vad_bits[m] = v.is_voice_segment(&i16buf).unwrap_or(false);
        }
        // YIN on a 40 ms window starting here, clamped to the tail.
        let yin_end = (at + YIN_W).min(audio.len());
        let (f0, periodicity) = yin.analyze(&audio[at..yin_end]);
        // Bands on the 16 ms window; enough to see 300–3400 and 4–8 kHz.
        let band_end = (at + BAND_FFT).min(audio.len());
        let (speech, hiss) = bands.analyze(&audio[at..band_end]);
        let rms = (frame.iter().map(|&v| v * v).sum::<f32>() / frame.len() as f32).sqrt();
        let rms_db = 20.0 * (rms + 1e-9).log10();
        frames.push(Frame { vad: vad_bits, f0, periodicity, speech, hiss, rms_db });
        at += FRAME_LEN;
    }
    (frames, started.elapsed())
}

// --- pooling to a tick rate -------------------------------------------------
//
// The same integer anchor the digester uses: tick = floor(frame * rate / 100),
// with the frames being 10 ms. At 8 Hz that is 12.5 frames a tick and at 32 Hz
// 3.125; a tick is never assumed to hold a whole number of frames.

/// Presence fraction per tick for one method, at `rate` Hz.
fn tick_fractions(frames: &[Frame], rate: f32, m: Method) -> Vec<f32> {
    if frames.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<f32> = Vec::new();
    let mut acc = 0.0f32;
    let mut n = 0u32;
    let mut cur: i64 = -1;
    for (i, f) in frames.iter().enumerate() {
        let tick = (i as f32 * rate / 100.0).floor() as i64;
        if cur == -1 {
            cur = tick;
        }
        if tick != cur {
            out.push(if n == 0 { 0.0 } else { acc / n as f32 });
            acc = 0.0;
            n = 0;
            // A long gap (a file shorter than its last tick) still emits one
            // value per tick it skipped, so time never compresses.
            for _ in cur + 1..tick {
                out.push(0.0);
            }
            cur = tick;
        }
        acc += frame_presence(f, m) as u32 as f32;
        n += 1;
    }
    out.push(if n == 0 { 0.0 } else { acc / n as f32 });
    out
}

/// Per-second presence decision (fraction ≥ [`PRESENCE_FRAC`]).
fn second_presence(frames: &[Frame], m: Method) -> Vec<bool> {
    let per_second = (1000 / FRAME_MS) as usize; // 100
    frames
        .chunks(per_second)
        .map(|c| {
            let hits = c.iter().filter(|f| frame_presence(f, m)).count();
            hits as f32 / c.len() as f32 >= PRESENCE_FRAC
        })
        .collect()
}

// --- labels -----------------------------------------------------------------

struct Labels {
    song: String,
    voice: Option<Vec<(f64, f64)>>,
}

fn read_label(path: &Path) -> Result<Labels, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: toml::Value = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
    let song = v
        .get("song")
        .and_then(|s| s.as_str())
        .ok_or_else(|| format!("{}: no song key", path.display()))?
        .to_string();
    // No `voice` key at all means the template is still pending. An empty list
    // is a real label — "no voice here" — which is why click-100.toml has one.
    let voice = match v.get("voice") {
        None => None,
        Some(toml::Value::Array(a)) => {
            let mut spans = Vec::new();
            for e in a {
                let arr = e.as_array().ok_or("voice span is not a [start, end]")?;
                if arr.len() != 2 {
                    return Err("voice span is not a [start, end]".into());
                }
                let a0 = arr[0].as_float().ok_or("span start is not a number")?;
                let a1 = arr[1].as_float().ok_or("span end is not a number")?;
                spans.push((a0, a1));
            }
            Some(spans)
        }
        Some(_) => return Err("voice is not an array".into()),
    };
    Ok(Labels { song, voice })
}

fn truth_at(labels: &Labels, second: usize) -> bool {
    let mid = second as f64 + 0.5;
    labels
        .voice
        .as_ref()
        .is_some_and(|spans| spans.iter().any(|&(a, b)| mid >= a && mid < b))
}

// --- native-frame anchor check ----------------------------------------------
//
// `the voice design notes`: the exact 8 = pool(4 × 32) rule depends on both levels
// using the same anchor. The digest assigns a native frame to a window with
// `floor(frame * HOP * RATE / SR)` — an integer division on the frame's start
// sample, never its centre. This proves the 32 Hz windows nest exactly four to
// an 8 Hz one for every frame.

fn anchor_check() {
    use metal_listen::{HOP, RATE, SR};
    let base = |f: u64| f * HOP as u64 * RATE as u64 / SR as u64;
    let fine = |f: u64| f * HOP as u64 * (RATE as u64 * 4) / SR as u64;
    let mut bad = 0u64;
    for f in 0..2_000_000u64 {
        let (b, x) = (base(f), fine(f));
        if x / 4 != b || x % 4 > 3 {
            bad += 1;
        }
    }
    let verdict = if bad == 0 { "PASS" } else { "FAIL" };
    println!("anchor: native frame -> window is the frame's start sample (integer), not its centre");
    println!("anchor: floor(f*HOP*RATE/SR); 32 Hz nests 4:1 in 8 Hz over 2e6 frames: {verdict} ({bad} bad)");
    println!("anchor: frame 6 -> 8 Hz window {}, 32 Hz window {} (4×{} + {})", base(6), fine(6), fine(6) / 4, fine(6) % 4);
}

// --- CPU report -------------------------------------------------------------

fn cpu_line(label: &str, dur: Duration, seconds: f64) {
    let ms = dur.as_secs_f64() * 1000.0;
    let per_sec = if seconds > 0.0 { ms / seconds } else { 0.0 };
    println!(
        "  {label:<34} {ms:9.1} ms total   {per_sec:8.3} ms/s audio   {:6.3} % of one core",
        per_sec / 10.0
    );
}

// --- main -------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut report = false;
    let mut timeline = false;
    let mut anchor_only = false;
    let mut songs: Vec<String> = Vec::new();
    let mut corpus = std::env::var_os("HOME")
        .map(|_| metal_config::cache_dir().join("data/audio"))
        .unwrap_or_else(|| PathBuf::from("data/audio"));
    let mut labels_dir = PathBuf::from("data/voice-labels");
    let mut max_seconds: Option<f32> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--report" => report = true,
            "--timeline" => timeline = true,
            "--anchor" => anchor_only = true,
            "--song" => {
                i += 1;
                songs.push(args.get(i).cloned().unwrap_or_default());
            }
            "--corpus" => {
                i += 1;
                corpus = PathBuf::from(args.get(i).cloned().unwrap_or_default());
            }
            "--labels" => {
                i += 1;
                labels_dir = PathBuf::from(args.get(i).cloned().unwrap_or_default());
            }
            "--seconds" => {
                i += 1;
                max_seconds = args.get(i).and_then(|s| s.parse().ok());
            }
            other => eprintln!("voice_probe: ignoring unknown arg {other}"),
        }
        i += 1;
    }

    anchor_check();
    if anchor_only {
        return;
    }

    // Which songs: `--song` wins; `--report` reads the label dir; otherwise the
    // whole corpus.
    let mut todo: Vec<(String, PathBuf, Option<PathBuf>)> = Vec::new(); // stem, audio, label
    if report {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&labels_dir)
            .map_err(|e| format!("{}: {e}", labels_dir.display()))
            .unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(2);
            })
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        entries.sort();
        for label in entries {
            let labels = match read_label(&label) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("voice_probe: {e}");
                    continue;
                }
            };
            if !songs.is_empty() && !songs.iter().any(|s| s == &labels.song) {
                continue;
            }
            match find_audio(&corpus, &labels.song) {
                Some(audio) => todo.push((labels.song.clone(), audio, Some(label.clone()))),
                None => eprintln!(
                    "voice_probe: no audio for label '{}' in {}",
                    labels.song,
                    corpus.display()
                ),
            }
        }
    } else if songs.is_empty() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&corpus)
            .map_err(|e| format!("{}: {e}", corpus.display()))
            .unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(2);
            })
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| matches!(x, "mp3" | "wav" | "flac" | "ogg" | "m4a"))
            })
            .collect();
        entries.sort();
        for audio in entries {
            let stem = audio
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_string();
            todo.push((stem, audio, None));
        }
    } else {
        for stem in &songs {
            match find_audio(&corpus, stem) {
                Some(audio) => todo.push((stem.clone(), audio, None)),
                None => eprintln!("voice_probe: no audio for '{stem}' in {}", corpus.display()),
            }
        }
    }

    if todo.is_empty() {
        eprintln!("voice_probe: nothing to analyse");
        return;
    }

    let methods = [
        Method::Vad(0),
        Method::Vad(1),
        Method::Vad(2),
        Method::Vad(3),
        Method::Yin,
        Method::Fusion(2),
    ];

    println!("\n=== voice_probe ===");
    println!("corpus: {}", corpus.display());
    println!("labels: {}{}", labels_dir.display(), if report { "" } else { " (not read; pass --report)" });

    // Confusion totals per method, factored as (tp, fp, fn, tn).
    let mut totals = vec![(0u64, 0u64, 0u64, 0u64); methods.len()];
    let mut labeled_seconds = 0u64;
    let mut pending: Vec<String> = Vec::new();

    // CPU is additive over songs, like the corpus.
    let mut decode_total = Duration::ZERO;
    let mut dsp_total = Duration::ZERO;
    let mut pool8_total = Duration::ZERO;
    let mut pool32_total = Duration::ZERO;
    let mut audio_seconds = 0.0f64;

    for (stem, audio, label_path) in &todo {
        let (samples, decode_dur) = match decode_mono_16k(audio, max_seconds) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("voice_probe: {stem}: {e}");
                continue;
            }
        };
        let seconds = samples.len() as f64 / VOICE_SR as f64;
        decode_total += decode_dur;
        let (frames, dsp_dur) = analyse(&samples);
        dsp_total += dsp_dur;
        audio_seconds += seconds;

        // Time the pooling separately at both levels.
        let t = Instant::now();
        let _ = tick_fractions(&frames, 8.0, Method::Fusion(2));
        pool8_total += t.elapsed();
        let t = Instant::now();
        let _ = tick_fractions(&frames, 32.0, Method::Fusion(2));
        pool32_total += t.elapsed();

        // Per-song presence summary, always.
        let p_vad = second_presence(&frames, Method::Vad(2));
        let p_yin = second_presence(&frames, Method::Yin);
        let p_fuse = second_presence(&frames, Method::Fusion(2));
        let nsec = p_fuse.len();
        let frac = |v: &[bool]| {
            if v.is_empty() {
                0.0
            } else {
                v.iter().filter(|&&b| b).count() as f32 / v.len() as f32
            }
        };
        println!(
            "\n{stem}: {seconds:.1}s, {nsec} seconds\ndecision VAD m2 {:.2}  YIN {:.2}  fusion {:.2}  (fraction of seconds voiced)",
            frac(&p_vad),
            frac(&p_yin),
            frac(&p_fuse)
        );
        let mut pitches: Vec<f32> = frames
            .iter()
            .filter(|f| f.periodicity > VOICED_PERIODICITY && (F0_MIN..=F0_MAX).contains(&f.f0))
            .map(|f| f.f0)
            .collect();
        pitches.sort_by(f32::total_cmp);
        let pitch_med = pitches.get(pitches.len() / 2).copied().unwrap_or(0.0);
        let mean = |sel: fn(&Frame) -> f32| {
            if frames.is_empty() {
                0.0
            } else {
                frames.iter().map(sel).sum::<f32>() / frames.len() as f32
            }
        };
        println!(
            "  pitch med {pitch_med:.0} Hz  voicing {:.2}  speech {:.2}  hiss {:.2}  rms {:.1} dB",
            mean(|f| f.periodicity),
            mean(|f| f.speech),
            mean(|f| f.hiss),
            mean(|f| f.rms_db)
        );

        if timeline {
            for s in 0..nsec.min(600) {
                let mark = if p_fuse[s] { 'V' } else { '.' };
                println!(
                    "  t={:04}s VAD {} YIN {} FUSE {}   {}",
                    s,
                    p_vad[s] as u8,
                    p_yin[s] as u8,
                    p_fuse[s] as u8,
                    mark
                );
            }
        }

        if report {
            if let Some(lp) = label_path {
                let labels = match read_label(lp) {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!("voice_probe: {e}");
                        continue;
                    }
                };
                if labels.voice.is_none() {
                    pending.push(stem.clone());
                    continue;
                }
                // Per-song score, per method.
                let n = nsec.max(
                    labels
                        .voice
                        .as_ref()
                        .map(|v| v.iter().map(|&(_, b)| b.ceil() as usize).max().unwrap_or(0))
                        .unwrap_or(0),
                );
                println!(
                    "  score  method        TP    FP    FN    TN    prec    rec"
                );
                for (mi, m) in methods.iter().enumerate() {
                    let pred = second_presence(&frames, *m);
                    let mut tp = 0u64;
                    let mut fp = 0u64;
                    let mut fneg = 0u64;
                    let mut tn = 0u64;
                    for s in 0..n {
                        let t = truth_at(&labels, s);
                        let p = pred.get(s).copied().unwrap_or(false);
                        match (t, p) {
                            (true, true) => tp += 1,
                            (false, true) => fp += 1,
                            (true, false) => fneg += 1,
                            (false, false) => tn += 1,
                        }
                    }
                    println!(
                        "  {:<14} {:5} {:5} {:5} {:5}   {:>5}   {:>5}",
                        m.name(),
                        tp,
                        fp,
                        fneg,
                        tn,
                        if tp + fp > 0 {
                            format!("{:.2}", tp as f32 / (tp + fp) as f32)
                        } else {
                            "n/a".into()
                        },
                        if tp + fneg > 0 {
                            format!("{:.2}", tp as f32 / (tp + fneg) as f32)
                        } else {
                            "n/a".into()
                        }
                    );
                    let t = &mut totals[mi];
                    t.0 += tp;
                    t.1 += fp;
                    t.2 += fneg;
                    t.3 += tn;
                }
                labeled_seconds += n as u64;
            }
        }
    }

    // CPU table.
    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    println!("\nCPU (laptop, {profile} build):");
    cpu_line("decode -> 16 kHz (excluded)", decode_total, audio_seconds);
    cpu_line("resample+VAD+YIN+bands", dsp_total, audio_seconds);
    cpu_line("pooling @ 8 Hz", pool8_total, audio_seconds);
    cpu_line("pooling @ 32 Hz", pool32_total, audio_seconds);
    cpu_line(
        "voice path total @ 8 Hz",
        dsp_total + pool8_total,
        audio_seconds,
    );
    cpu_line(
        "voice path total @ 32 Hz",
        dsp_total + pool32_total,
        audio_seconds,
    );
    println!("  ({:.1} s of audio analysed, {} songs)", audio_seconds, todo.len());

    if report {
        println!("\n=== precision / recall ===");
        if labeled_seconds == 0 && pending.is_empty() {
            println!("no labels found");
        }
        println!("labeled seconds: {labeled_seconds}");
        if !pending.is_empty() {
            println!(
                "LABELS PENDING (template, no voice spans marked): {}",
                pending.join(", ")
            );
        }
        println!("overall (all labeled files; click-100 included as all no-voice):");
        println!("  method          TP      FP      FN      TN    prec    rec");
        for (mi, m) in methods.iter().enumerate() {
            let (tp, fp, fneg, tn) = totals[mi];
            println!(
                "  {:<14} {:6} {:7} {:7} {:7}   {:>5}   {:>5}",
                m.name(),
                tp,
                fp,
                fneg,
                tn,
                if tp + fp > 0 {
                    format!("{:.2}", tp as f32 / (tp + fp) as f32)
                } else {
                    "n/a".into()
                },
                if tp + fneg > 0 {
                    format!("{:.2}", tp as f32 / (tp + fneg) as f32)
                } else {
                    "n/a".into()
                }
            );
        }
    }
}

fn find_audio(corpus: &Path, stem: &str) -> Option<PathBuf> {
    for ext in ["mp3", "wav", "flac", "ogg", "m4a"] {
        let p = corpus.join(format!("{stem}.{ext}"));
        if p.exists() {
            return Some(p);
        }
    }
    None
}
