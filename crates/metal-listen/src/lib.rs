//! **The real digest.** Sixteen columns on the 8 Hz grid, computed from audio
//! as it arrives, in Rust, on this machine.
//!
//! `docs/AUDIO_FEATURES.md` fixed the contract and `data/digest/digest_audio.py`
//! implemented it for whole files. That script is not a thing a live set can
//! run: it takes a path, decodes all of it, and returns a matrix. This crate is
//! the same sixteen columns computed incrementally, so `metal-dash` can be driven
//! by what is actually playing instead of by `digest::Simulated`.
//!
//! # What this is not
//!
//! It is **not a second specification**. Every number here is chosen to match
//! librosa's, because the model is trained on librosa's output and a serving
//! path with its own opinions is the quietest way to ruin a model
//! (`AUDIO_FEATURES.md`, "one pipeline, not two"). Where the offline version
//! does something a stream genuinely cannot, this crate approximates it, says
//! so out loud, and `tests/reference.rs` *measures* the gap rather than
//! asserting it is small.
//!
//! There are exactly two such places, and neither is a judgement call:
//!
//! 1. **The decibel floor.** `power_to_db`'s `top_db` clamps at 80 dB below the
//!    loudest frame *in the file*. A stream uses a running peak. See
//!    [`features::power_to_db_running`] — the effect is confined to bins that
//!    are already silent, and it cancels almost entirely in the difference the
//!    onset column is built from.
//! 2. **Chroma tuning.** `chroma_stft(tuning=None)` estimates the tuning of the
//!    whole recording before building its filterbank. That is a two-pass
//!    operation. This crate assumes A440, which is right for nearly everything
//!    and wrong by a fraction of a semitone for the rest.
//!
//! A third difference is a *choice*, not a limitation — see [`Compat`].
//!
//! # The clock
//!
//! One row per 125 ms, which is `metal_rig::motion::KEY_RATE`, which is why an
//! audio row and a motion keyframe are the same instant. Window boundaries are
//! counted in **samples**, never in wall-clock time: a set runs for hours and a
//! timer would drift against the audio clock, putting rows on the wrong side of
//! a beat by the end of the night.
//!
//! # Using it
//!
//! ```
//! use metal_listen::{Compat, Digester, TickRate};
//!
//! // Mono, at metal_listen::SR. Anything else has to go through `resample` first.
//! let samples = vec![0.0f32; metal_listen::SR as usize];
//! let mut d = Digester::new(Compat::Live, TickRate::Base);
//! d.push(&samples);
//! while let Some(row) = d.pop() {
//!     // row[2] is the onset — the column an audience sees a visual answer.
//!     let _onset = row[2];
//! }
//! ```

use std::collections::VecDeque;
use std::sync::Arc;

use rustfft::{num_complex::Complex, Fft, FftPlanner};

pub mod downbeat;
pub mod features;
pub mod filters;
pub mod repeat;
pub mod resample;
pub mod tempo;
pub mod tone;
pub mod voice;

#[cfg(feature = "capture")]
pub mod capture;

#[cfg(feature = "decode")]
pub mod file;

/// The base rate in Hz — the minimum of the ladder (`metal_schema::BASE_HZ`),
/// not a rate any one digest runs at. A `Digester` carries its own
/// [`TickRate`]; this constant only describes the ladder's bottom rung, so a
/// hard-coded "8" has one place to come from.
pub const RATE: f32 = metal_schema::BASE_HZ as f32;

/// A tick level, re-exported from `metal_schema` — one ladder, one type.
pub use metal_schema::TickRate;
/// Width of a digest row: the music prefix (`MUSIC`, columns 0..16), the voice
/// block (`VOICE`, columns 16..29) and the tone block (`TONE`, columns
/// 29..35). Declared once in `metal_schema::FEATURES`.
pub const FEATURES: usize = metal_schema::FEATURES;
/// The music prefix width — see [`metal_schema::MUSIC`].
pub const MUSIC: usize = metal_schema::MUSIC;
/// The voice block width — see [`metal_schema::VOICE`].
pub const VOICE: usize = metal_schema::VOICE;
/// The tone block width — see [`metal_schema::TONE`].
pub const TONE: usize = metal_schema::TONE;

/// Where the registry keeps Basic Pitch's ONNX graph (`the tone design notes` D9).
///
/// The file is **not committed** — `data/models/*/model.onnx` is git-ignored
/// and `data/models/basic-pitch/model.toml` records the sha256 `metal_ml::fetch`
/// checks before installing it. This is the one place that path is written
/// down: the live capture worker ([`capture`]) and the offline tone pass
/// ([`file::digest_file_with_tone`]) both point at it, so they cannot disagree
/// about which model the tone block means. Ungated on purpose: a build without
/// the `tone` feature still names the path in its diagnostics, and the capture
/// worker needs it whether or not `decode` (which owns [`file`]) is on.
pub const TONE_MODEL: &str = "data/models/basic-pitch/model.onnx";
/// The sample rate every feature is defined at.
///
/// Not negotiable, and not a preference. `librosa.load(sr=22050)` resamples —
/// and therefore low-passes at 11 025 Hz — before it measures anything, so the
/// corpus has no energy above that and its centroids and bandwidths are
/// computed over that band. Analysing a live 48 kHz stream in place would put
/// every spectral column in a different range from the training data.
pub const SR: u32 = 22050;
pub const N_FFT: usize = 2048;
pub const HOP: usize = 512;
pub const N_MELS: usize = 128;
pub const N_CHROMA: usize = 12;
/// librosa's `power_to_db` default.
pub const TOP_DB: f32 = 80.0;

/// What each column is, in `metal_schema::DIGEST_COLUMNS`'s order — the music
/// prefix, the voice block, then the tone block.
pub const COLUMNS: [&str; FEATURES] = metal_schema::DIGEST_COLUMNS;

/// One 125 ms row.
pub type Row = [f32; FEATURES];

/// Which end of the fidelity/latency trade to sit on.
///
/// The two modes compute **identical numbers**. They differ only in which
/// 125 ms row a given onset is attached to, and that is worth a knob because
/// the two consumers want opposite things.
///
/// librosa shifts its onset envelope back by three frames — one for the
/// difference itself and two more to undo the STFT's centring — so the value
/// reported at frame *t* describes the transition into frame *t−2*. Reproducing
/// that costs 70 ms of extra delay on top of the 125 ms the window already
/// takes to fill.
///
/// - [`Compat::Librosa`] — for producing matrices that must line up with the
///   corpus, and for the reference test. Use it when the numbers will be
///   compared to `digest_audio.py` or fed to a model trained on it.
/// - [`Compat::Live`] — for a room. Reports the flux as soon as it exists,
///   which is about 70 ms earlier, and 70 ms is the difference between a visual
///   that hits with the kick and one that answers it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Compat {
    Librosa,
    Live,
}

/// The features of one native STFT frame, before pooling — about 43 of these
/// go into every eight rows.
///
/// Recorded only when [`Digester::trace_native`] has been called. It exists so
/// the reference test can say *which* feature diverged rather than only that
/// the matrix did.
#[derive(Clone, Debug, Default)]
pub struct Native {
    pub rms: f32,
    pub bandwidth: f32,
    pub centroid: f32,
    pub onset: f32,
    pub chroma: [f32; N_CHROMA],
}

/// Sums for the 125 ms window currently being filled.
struct Window {
    index: u64,
    frames: u32,
    rms: f64,
    bandwidth: f64,
    centroid: f64,
    /// Max, not mean. A drum hit anywhere inside the window has to survive:
    /// averaging divides a transient by five and hands the model a smear where
    /// there was an attack.
    onset: f32,
    chroma: [f64; N_CHROMA],
}

impl Window {
    fn new(index: u64) -> Window {
        Window {
            index,
            frames: 0,
            rms: 0.0,
            bandwidth: 0.0,
            centroid: 0.0,
            onset: 0.0,
            chroma: [0.0; N_CHROMA],
        }
    }

    fn row(&self) -> Row {
        let mut row = [0.0f32; FEATURES];
        if self.frames == 0 {
            // Zero is the honest "nothing measured here" and not a guess, which
            // is what `digest_audio.py` writes for the same case.
            return row;
        }
        let n = self.frames as f64;
        row[0] = (self.rms / n) as f32;
        row[1] = (self.bandwidth / n) as f32;
        row[2] = self.onset;
        row[3] = (self.centroid / n) as f32;
        for c in 0..N_CHROMA {
            row[4 + c] = (self.chroma[c] / n) as f32;
        }
        row
    }
}

/// The tempo map of a whole digest: what [`Digester::record_tempo`] keeps
/// and `file::digest_file_with_tempo` turns into `<song>.tempo.json`.
///
/// Times are seconds from the start of the audio, at the centre of the STFT
/// frame the event was heard in (frame × [`HOP`] / [`SR`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TempoLog {
    /// (start time, BPM): one entry at the first lock, one at every re-lock
    /// and one whenever the glided tempo has moved more than 2 % since the
    /// last entry.
    pub segments: Vec<(f64, f32)>,
    /// (time, integer beat index on the tracker's count) of every beat.
    pub beats: Vec<(f64, i64)>,
    last_beat: Option<i64>,
    last_changes: u32,
}

impl TempoLog {
    fn observe(&mut self, tempo: &tempo::Tempo, frame: u64) {
        let (Some(b), Some(bpm)) = (tempo.beats(), tempo.bpm()) else {
            return;
        };
        let t = frame as f64 * HOP as f64 / SR as f64;
        let new_segment = match self.segments.last() {
            None => true,
            Some(&(_, last)) => {
                tempo.changes() != self.last_changes || ((bpm - last) / last).abs() > 0.02
            }
        };
        if new_segment {
            self.segments.push((t, bpm));
            self.last_changes = tempo.changes();
        }
        let now = b.floor() as i64;
        if self.last_beat.is_none_or(|l| now > l) {
            self.beats.push((t, now));
            self.last_beat = Some(now);
        }
    }
}

/// Audio in, rows out.
pub struct Digester {
    compat: Compat,
    /// The tick level this digest emits at. The DSP is identical at every
    /// level; only the window the native frames pool into changes, so a base
    /// machine really pays the base per-tick cost and a fine one the fine cost.
    rate: TickRate,
    fft: Arc<dyn Fft<f32>>,
    window_fn: Vec<f32>,
    freqs: Vec<f32>,
    mel: filters::Filterbank,
    chroma: filters::Filterbank,

    /// The padded signal that has not been turned into frames yet.
    ///
    /// Primed with `N_FFT / 2` zeros, which is not an implementation detail:
    /// it is what makes frame 0 here identical to frame 0 of a centred librosa
    /// STFT. Without it every frame would be offset by 1024 samples and the
    /// whole matrix would be shifted against the corpus.
    buf: Vec<f32>,
    frame: u64,

    scratch: Vec<Complex<f32>>,
    mag: Vec<f32>,
    power: Vec<f32>,
    mel_db: Vec<f32>,
    prev_mel_db: Vec<f32>,
    have_prev: bool,
    onset_delay: VecDeque<f32>,
    peak_db: f32,
    tempo: tempo::Tempo,
    /// The bar's "1", heard from the same flux on the same beat clock.
    downbeat: downbeat::Downbeat,
    /// The tempo map of the whole run, when asked for ([`Digester::record_tempo`]).
    /// Off by default: a room runs for hours and has no use for every beat.
    tempo_log: Option<TempoLog>,
    repeat: repeat::Repeat,

    /// The native frames since the last integer [`tempo::Tempo`] beat, pooled
    /// into a [`repeat::BeatVector`] when the next one arrives.
    ///
    /// Kept separately from `native` because `native` only exists while the
    /// reference test is tracing, and the detector runs in the room. It is
    /// bounded by `HISTORY_BEATS` of slack: `flush_beat` clears it at every
    /// beat, so it holds at most one beat's frames, and a tempo wobble cannot
    /// let it grow past a couple.
    beat_frames: Vec<Native>,

    /// The integer beat [`tempo::Tempo`] had reached at the previous frame, so
    /// a boundary crossing can be detected without a second clock.
    ///
    /// `None` while the tracker has not locked, which is also the gate the
    /// brief's decision 10 needs: no tempo lock, no detector.
    last_beat: Option<u32>,

    /// The last state [`repeat::Repeat::push`] returned.
    ///
    /// Cached rather than re-asked, because the detector has no query for "am
    /// I locked" beyond the value of the most recent push, and because a
    /// `Digester::repeat()` in `metal-dash`'s poll must not advance the detector.
    last_repeat: Option<repeat::RepeatState>,

    current: Window,
    out: VecDeque<Row>,
    native: Option<Vec<Native>>,

    /// The voice path: the same input resampled to the 16 kHz `webrtc-vad`
    /// and the analyser need, and the analyser itself. Independent of the
    /// music path, so the prefix stays bit-identical.
    voice_resampler: resample::Resampler,
    voice: voice::Analyser,
    voice_16k: Vec<f32>,
    voice_out: VecDeque<voice::VoiceRow>,

    /// The tone path: Basic Pitch on its own thread, read once per row. Off
    /// until [`Digester::attach_tone`] is called, so the music and voice blocks
    /// are untouched for any caller that does not opt in. When attached but no
    /// model loaded, [`pair_tone`] writes an all-zero block (D8).
    #[cfg(feature = "tone")]
    tone: Option<tone::Tone>,
    /// Completed tone blocks in tick order, when the tone path can produce one
    /// per row; [`pair_tone`] prefers this queue and otherwise reads the latest
    /// latched columns. Empty for a live stream, whose results arrive
    /// asynchronously — the latest read is the point of D7.
    #[cfg(feature = "tone")]
    tone_out: VecDeque<[f32; TONE]>,
}

/// The rate [`voice::Analyser`] works at, and the only rate `webrtc-vad`
/// accepts that the voice DSP is defined on.
pub const VOICE_SR: u32 = 16_000;

impl Digester {
    /// A digester that emits `rate` rows per second. Files use
    /// [`TickRate::CANONICAL`]; a live source uses the machine's active level.
    pub fn new(compat: Compat, rate: TickRate) -> Digester {
        let bins = N_FFT / 2 + 1;
        let mut planner = FftPlanner::new();
        Digester {
            compat,
            rate,
            fft: planner.plan_fft_forward(N_FFT),
            window_fn: features::hann_periodic(N_FFT),
            freqs: filters::fft_frequencies(SR, N_FFT),
            mel: filters::mel(SR, N_FFT, N_MELS),
            chroma: filters::chroma(SR, N_FFT, N_CHROMA),
            buf: vec![0.0; N_FFT / 2],
            frame: 0,
            scratch: vec![Complex { re: 0.0, im: 0.0 }; N_FFT],
            mag: vec![0.0; bins],
            power: vec![0.0; bins],
            mel_db: vec![0.0; N_MELS],
            prev_mel_db: vec![0.0; N_MELS],
            have_prev: false,
            onset_delay: VecDeque::new(),
            peak_db: f32::NEG_INFINITY,
            tempo: tempo::Tempo::new(),
            downbeat: downbeat::Downbeat::new(),
            tempo_log: None,
            repeat: repeat::Repeat::new(),
            beat_frames: Vec::new(),
            last_beat: None,
            last_repeat: None,
            current: Window::new(0),
            out: VecDeque::new(),
            native: None,
            voice_resampler: resample::Resampler::with_target(SR, VOICE_SR),
            voice: voice::Analyser::new(rate),
            voice_16k: Vec::new(),
            voice_out: VecDeque::new(),
            #[cfg(feature = "tone")]
            tone: None,
            #[cfg(feature = "tone")]
            tone_out: VecDeque::new(),
        }
    }

    /// Close a music row, pairing it with the voice row for the same tick.
    ///
    /// The voice analyser closes a tick when the next tick's first frame
    /// arrives, so by the time a music window closes its voice row is there.
    /// A tick with no row (the very first frames of a stream) is an honest
    /// all-zero voice block.
    fn pair_voice(&mut self, mut row: Row) -> Row {
        if let Some(v) = self.voice_out.pop_front() {
            row[MUSIC..MUSIC + VOICE].copy_from_slice(&v);
        }
        row
    }

    /// Attach the tone path, loading the Basic Pitch model if `model_path`
    /// points at a real file. `None` (or a missing/unloadable file) attaches a
    /// model-less Tone that emits the all-zero block, which is the D8 chroma
    /// fallback.
    #[cfg(feature = "tone")]
    pub fn attach_tone(&mut self, model_path: Option<&std::path::Path>) {
        self.tone = Some(tone::Tone::new(model_path));
    }

    /// The tone path, once attached.
    #[cfg(feature = "tone")]
    pub fn tone(&self) -> Option<&tone::Tone> {
        self.tone.as_ref()
    }

    /// Fill the tone block of a row: the queued block if there is one, else the
    /// latest latched columns ([`tone::Tone::columns`]), else — unattached or
    /// no model — zeros. Reads the *latest*, never a per-tick queue, because
    /// inference is asynchronous (D7): the freshest result is the honest one.
    fn pair_tone(&mut self, row: Row) -> Row {
        #[cfg(feature = "tone")]
        {
            let mut row = row;
            let cols = self
                .tone_out
                .pop_front()
                .or_else(|| self.tone.as_ref().map(tone::Tone::columns))
                .unwrap_or([0.0; TONE]);
            row[MUSIC + VOICE..].copy_from_slice(&cols);
            row
        }
        #[cfg(not(feature = "tone"))]
        {
            row
        }
    }

    /// Start recording per-frame features for the reference test.
    pub fn trace_native(&mut self) {
        self.native = Some(Vec::new());
    }

    pub fn native(&self) -> &[Native] {
        self.native.as_deref().unwrap_or(&[])
    }

    /// Native STFT frames consumed so far.
    pub fn frames(&self) -> u64 {
        self.frame
    }

    /// The beat clock. Fed from the same flux the onset column is built from,
    /// so it costs nothing extra and cannot disagree with what column 2 says.
    pub fn tempo(&self) -> &tempo::Tempo {
        &self.tempo
    }

    /// The bar's "1" — see [`downbeat`]. Live and file digests both get it,
    /// because both run through here.
    pub fn downbeat(&self) -> &downbeat::Downbeat {
        &self.downbeat
    }

    /// "Tap 1" at the current instant: the beat nearest now becomes the bar's
    /// first until the next tap. Returns the position (beat index mod 4), or
    /// `None` when the tracker has not locked and there is no beat to tap.
    pub fn tap_downbeat(&mut self) -> Option<u8> {
        let b = self.tempo.beats()? as f64;
        Some(self.downbeat.tap(b))
    }

    /// Start keeping the tempo map of this run (every beat's time, BPM
    /// segments) for [`Digester::tempo_log`]. The file path turns it on; a
    /// room does not.
    pub fn record_tempo(&mut self) {
        self.tempo_log.get_or_insert_with(TempoLog::default);
    }

    /// The tempo map so far, if [`Digester::record_tempo`] was called.
    pub fn tempo_log(&self) -> Option<&TempoLog> {
        self.tempo_log.as_ref()
    }

    /// The DJ's loop right now, or `None` when there is none.
    ///
    /// The last [`repeat::RepeatState`] the detector emitted, held so a caller
    /// polling faster than the beat can read the same answer twice — the same
    /// contract as [`tempo::Tempo::beats`]. `None` before the detector has ever
    /// locked, and `None` again the beat after it releases.
    pub fn repeat(&self) -> Option<repeat::RepeatState> {
        self.last_repeat
    }

    /// The tick level this digester emits at.
    pub fn rate(&self) -> TickRate {
        self.rate
    }

    /// Which row at `rate` a native frame belongs to.
    ///
    /// Integer arithmetic on a sample count, deliberately: this is
    /// `floor(frame * HOP * rate / SR)` done in a way that cannot drift. At
    /// [`TickRate::Base`] it is the same mapping `digest_audio.py` gets from
    /// comparing `frames_to_time` against window edges, and because the ladder
    /// nests (`32 = 4 × 8` on the same anchor) four fine windows are exactly
    /// one base window.
    fn window_of(frame: u64, rate: TickRate) -> u64 {
        frame * HOP as u64 * rate.hz() as u64 / SR as u64
    }

    /// Feed mono samples at [`SR`]. Anything else must be resampled first —
    /// see [`resample`].
    pub fn push(&mut self, samples: &[f32]) {
        // The voice path reads the same samples at its own rate. Feeding it
        // before `frame_ready` means a voice tick is closed by the time the
        // music window that shares its boundary is emitted.
        self.voice_16k.clear();
        self.voice_resampler.process(samples, &mut self.voice_16k);
        self.voice.push(&self.voice_16k);
        while let Some(v) = self.voice.pop() {
            self.voice_out.push_back(v);
        }

        // The tone path reads the same 22 050 Hz samples; its runner just
        // buffers and wakes, so this never blocks the digest (D7).
        #[cfg(feature = "tone")]
        if let Some(t) = self.tone.as_mut() {
            t.push(samples);
        }

        self.buf.extend_from_slice(samples);
        while self.buf.len() >= N_FFT {
            self.frame_ready();
            self.buf.drain(..HOP);
        }
    }

    /// End of file: pad the tail the way librosa does, then close the last row.
    ///
    /// `center=True` pads `n_fft / 2` zeros at **both** ends, and the trailing
    /// half is the reason a librosa STFT has one more frame than a naive
    /// streaming one. Skip this and a file digested here is a frame short of
    /// the same file digested by `digest_audio.py` — which is a whole row
    /// missing off the end of every track in the corpus.
    ///
    /// Never call it on a live stream. The next samples are still coming, and
    /// this would splice silence into the middle of the set.
    pub fn finish(&mut self) {
        let tail = vec![0.0f32; N_FFT / 2];
        self.push(&tail);
        // Close the voice path's trailing partial tick before the music's, so
        // the last row still finds a row to pair with.
        self.voice.flush();
        while let Some(v) = self.voice.pop() {
            self.voice_out.push_back(v);
        }
        // Flush the tone path too, so the trailing row still gets its block
        // rather than falling through to whatever the latest latch happened to
        // hold at end of file.
        #[cfg(feature = "tone")]
        if let Some(t) = &self.tone {
            self.tone_out.push_back(t.columns());
        }
        self.flush();
    }

    /// Close the window being filled, whatever is in it.
    ///
    /// For a file this produces the trailing partial row. For a live stream it
    /// should not be called at all — the next samples are still coming.
    pub fn flush(&mut self) {
        if self.current.frames > 0 {
            let row = self.current.row();
            let row = self.pair_voice(row);
            let row = self.pair_tone(row);
            self.out.push_back(row);
            self.current = Window::new(self.current.index + 1);
        }
    }

    pub fn pop(&mut self) -> Option<Row> {
        self.out.pop_front()
    }

    pub fn available(&self) -> usize {
        self.out.len()
    }

    fn frame_ready(&mut self) {
        let t = self.frame;

        // --- RMS, off the raw samples, before any window is applied --------
        let rms = features::rms(&self.buf[..N_FFT]);

        // --- the spectrum --------------------------------------------------
        for i in 0..N_FFT {
            self.scratch[i] = Complex { re: self.buf[i] * self.window_fn[i], im: 0.0 };
        }
        self.fft.process(&mut self.scratch);
        for k in 0..self.mag.len() {
            self.mag[k] = self.scratch[k].norm();
            self.power[k] = self.mag[k] * self.mag[k];
        }

        // --- the two spectral shape columns --------------------------------
        let centroid_hz = features::centroid(&self.mag, &self.freqs);
        let bandwidth_hz = features::bandwidth(&self.mag, &self.freqs, centroid_hz);
        // The scaler is division by Nyquist, not a fitted transform: nothing is
        // learned per song and nothing per corpus, so a stream at a venue and
        // the training set see the same number for the same sound.
        let centroid_norm = (centroid_hz / (SR as f32 / 2.0)).clamp(0.0, 1.0);

        // --- chroma ---------------------------------------------------------
        let mut chroma = [0.0f32; N_CHROMA];
        self.chroma.apply(&self.power, &mut chroma);
        features::normalize_inf(&mut chroma);

        // --- onset ----------------------------------------------------------
        self.mel.apply(&self.power, &mut self.mel_db);
        features::power_to_db_running(&mut self.mel_db, &mut self.peak_db, TOP_DB);

        let flux = if self.have_prev {
            features::spectral_flux(&self.mel_db, &self.prev_mel_db)
        } else {
            0.0
        };
        // The beat clock reads the flux where it happens, never the delayed
        // copy: `Compat::Librosa` would otherwise hand the tracker a signal
        // three frames stale and it would lock 70 ms behind the room.
        self.tempo.push(flux);
        if let Some(b) = self.tempo.beats() {
            self.downbeat.observe(b as f64, flux);
        }
        if let Some(log) = self.tempo_log.as_mut() {
            log.observe(&self.tempo, self.frame);
        }

        let onset = if self.have_prev {
            match self.compat {
                // Report it where librosa reports it: three frames on, which
                // is one for the difference and two to undo the centring.
                Compat::Librosa => {
                    self.onset_delay.push_back(flux);
                    if self.onset_delay.len() > 2 {
                        self.onset_delay.pop_front().unwrap_or(0.0)
                    } else {
                        0.0
                    }
                }
                // Report it now. Seventy milliseconds earlier, same number.
                Compat::Live => flux,
            }
        } else {
            0.0
        };
        self.prev_mel_db.copy_from_slice(&self.mel_db);
        self.have_prev = true;

        if let Some(trace) = self.native.as_mut() {
            trace.push(Native { rms, bandwidth: bandwidth_hz, centroid: centroid_hz, onset, chroma });
        }

        // --- the loop detector, on the beat grid -----------------------------
        //
        // The detector compares one cycle of bars against the one before, and
        // "cycle" is only a number of *beats*. So the native frames are pooled
        // by beat here, where the beat clock is, and the detector never sees a
        // 125 ms row. `beat_vector` gets one whole beat of frames at each
        // boundary; the frame being processed now belongs to the beat it will
        // be pooled into next, so it is buffered before the boundary is read.
        self.beat_frames.push(Native {
            rms,
            bandwidth: bandwidth_hz,
            centroid: centroid_hz,
            onset,
            chroma,
        });
        match self.tempo.beats() {
            // Locked: a new integer beat means the frames since the last one
            // are a whole beat and can be pooled. `>` on the floor, so a
            // fractional beat that lands exactly on an integer still counts as
            // crossing into the next beat.
            Some(beats) => {
                let now = beats.floor() as u32;
                if let Some(last) = self.last_beat {
                    if now > last {
                        let frames = std::mem::take(&mut self.beat_frames);
                        self.last_repeat = self.repeat.push(repeat::beat_vector(&frames));
                    }
                }
                self.last_beat = Some(now);
            }
            // Decision 10: no tempo lock, no detector. There is no beat grid
            // to pool frames onto, and a vector pooled from arbitrary frames
            // would be a second opinion about where the beats are. The buffer
            // is dropped rather than kept, so a stale half-beat cannot be
            // handed to the first beat after a lock.
            None => {
                self.beat_frames.clear();
                self.last_beat = None;
                self.last_repeat = None;
            }
        }

        // --- pool into the 125 ms window ------------------------------------
        let w = Self::window_of(t, self.rate);
        while w > self.current.index {
            // Emitting even an empty window keeps row *n* at second *n/8*. A
            // skipped one would silently shorten the timeline, and every row
            // after it would describe the wrong moment.
            let row = self.current.row();
            let row = self.pair_voice(row);
            let row = self.pair_tone(row);
            self.out.push_back(row);
            self.current = Window::new(self.current.index + 1);
        }

        self.current.frames += 1;
        self.current.rms += rms as f64;
        self.current.bandwidth += bandwidth_hz as f64;
        self.current.centroid += centroid_norm as f64;
        self.current.onset = self.current.onset.max(onset);
        for c in 0..N_CHROMA {
            self.current.chroma[c] += chroma[c] as f64;
        }

        self.frame += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, secs: f32) -> Vec<f32> {
        let n = (SR as f32 * secs) as usize;
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / SR as f32).sin())
            .collect()
    }

    #[test]
    fn frames_land_on_the_window_the_offline_script_would_put_them_in() {
        // 0.125 s is 5.38 frames, so the first six frames straddle windows 0
        // and 1 exactly where the float arithmetic in the Python does.
        assert_eq!(Digester::window_of(0, TickRate::Base), 0);
        assert_eq!(Digester::window_of(5, TickRate::Base), 0);
        assert_eq!(Digester::window_of(6, TickRate::Base), 1);
        assert_eq!(Digester::window_of(11, TickRate::Base), 2);

        // It must never step backwards, and it must still be counting the same
        // eight rows a second at the end of a long night. A wall clock would
        // have drifted seconds by here; this is what the integer arithmetic on
        // a sample count is for.
        let mut last = 0;
        for f in (0..200_000u64).step_by(37) {
            let w = Digester::window_of(f, TickRate::Base);
            assert!(w >= last, "frame {f} went backwards");
            last = w;
        }
        let six_hours = 6 * 3600 * SR as u64 / HOP as u64;
        let want = 6 * 3600 * RATE as u64;
        let got = Digester::window_of(six_hours, TickRate::Base);
        assert!(
            (got as i64 - want as i64).abs() <= 1,
            "six hours in, row {got} where the clock says {want}"
        );
    }

    #[test]
    fn eight_rows_come_out_of_every_second_of_audio() {
        let mut d = Digester::new(Compat::Live, TickRate::Base);
        d.push(&sine(440.0, 10.0));
        // The last window is still filling and the centring pushes one row's
        // worth of samples past the end, so allow the boundary either way.
        let n = d.available();
        assert!((79..=81).contains(&n), "10 s of audio gave {n} rows, expected ~80");
    }

    #[test]
    fn a_pure_tone_puts_the_centroid_on_itself() {
        let mut d = Digester::new(Compat::Live, TickRate::Base);
        d.push(&sine(1000.0, 2.0));
        let rows: Vec<Row> = std::iter::from_fn(|| d.pop()).collect();
        // Skip the first rows: they contain the primed zeros and the onset of
        // the tone itself, neither of which is steady state.
        let mid = &rows[8..];
        let mean = mid.iter().map(|r| r[3]).sum::<f32>() / mid.len() as f32;
        let expect = 1000.0 / (SR as f32 / 2.0);
        assert!(
            (mean - expect).abs() < 0.02,
            "centroid {mean} should sit near {expect} for a 1 kHz tone"
        );
    }

    #[test]
    fn silence_is_all_zeroes_and_not_a_burst_of_static() {
        let mut d = Digester::new(Compat::Live, TickRate::Base);
        d.push(&vec![0.0f32; SR as usize * 2]);
        let rows: Vec<Row> = std::iter::from_fn(|| d.pop()).collect();
        assert!(!rows.is_empty());
        for (i, row) in rows.iter().enumerate() {
            for (c, &v) in row.iter().enumerate() {
                assert!(v.abs() < 1e-6, "row {i} column {} ({}) is {v}", c, COLUMNS[c]);
            }
        }
    }

    #[test]
    fn a_hit_after_silence_shows_up_in_the_onset_column() {
        let mut d = Digester::new(Compat::Live, TickRate::Base);
        d.push(&vec![0.0f32; SR as usize]);
        d.push(&sine(440.0, 1.0));
        let rows: Vec<Row> = std::iter::from_fn(|| d.pop()).collect();
        let quiet: f32 = rows[2..6].iter().map(|r| r[2]).sum();
        let at_the_hit = rows[7..12].iter().map(|r| r[2]).fold(0.0f32, f32::max);
        assert!(quiet < 1e-3, "silence should not have onsets, got {quiet}");
        assert!(at_the_hit > 1.0, "the attack should be visible, got {at_the_hit}");
    }

    #[test]
    fn feeding_the_same_audio_in_different_sized_chunks_changes_nothing() {
        // A capture callback hands over whatever the device felt like giving,
        // so the row boundaries must come from the sample count and not from
        // how the samples arrived.
        let audio = sine(330.0, 3.0);
        let mut whole = Digester::new(Compat::Librosa, TickRate::Base);
        whole.push(&audio);
        let a: Vec<Row> = std::iter::from_fn(|| whole.pop()).collect();

        let mut ragged = Digester::new(Compat::Librosa, TickRate::Base);
        let mut i = 0;
        for (n, chunk) in [7usize, 1024, 3, 999, 512, 8191].iter().cycle().enumerate() {
            if i >= audio.len() {
                break;
            }
            let end = (i + chunk).min(audio.len());
            ragged.push(&audio[i..end]);
            i = end;
            let _ = n;
        }
        let b: Vec<Row> = std::iter::from_fn(|| ragged.pop()).collect();

        assert_eq!(a.len(), b.len(), "chunking changed how many rows came out");
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            for c in 0..FEATURES {
                assert!(
                    (x[c] - y[c]).abs() < 1e-6,
                    "row {i} column {} differs: {} vs {}",
                    COLUMNS[c],
                    x[c],
                    y[c]
                );
            }
        }
    }

    #[test]
    fn the_music_prefix_is_bit_identical_to_before() {
        // Captured on d70b03b8, before the voice block was appended: the same
        // synthetic input digested by the same music path, columns 0..16 only.
        // The voice analyser is new and runs alongside; it must not move a
        // single bit of the music the corpus and the Colourist are defined on.
        #[rustfmt::skip]
        const BEFORE: [[f32; MUSIC]; 16] = [
            [0.13436258, 701.7645, 0.49175945, 0.050913773, 0.0047752573, 0.0026936254, 0.0020542096, 0.0019675065, 0.0022804358, 0.0031254536, 0.0057555325, 0.029829629, 0.3468269, 1.0, 0.36761567, 0.025100682],
            [0.18014815, 530.4762, 0.43698463, 0.04575669, 0.00011706206, 9.779166e-6, 2.2938518e-6, 1.8015612e-6, 3.97471e-6, 1.5853455e-5, 0.00018527494, 0.011245586, 0.3152747, 1.0, 0.33717644, 0.009115633],
            [0.19883174, 533.7329, 0.26084825, 0.045798942, 0.00011694858, 9.850299e-6, 2.3535986e-6, 1.8515307e-6, 4.0154137e-6, 1.586881e-5, 0.00018495505, 0.011233285, 0.31524074, 1.0, 0.33713517, 0.00910503],
            [0.19639432, 536.6247, 0.066627845, 0.04584404, 0.00011678888, 9.7454e-6, 2.2816985e-6, 1.7930374e-6, 3.962179e-6, 1.5817408e-5, 0.0001849609, 0.011235634, 0.31525072, 1.0, 0.3371487, 0.009106333],
            [0.17185465, 538.91, 0.07381114, 0.045878436, 0.00011740401, 9.900645e-6, 2.3698008e-6, 1.8628305e-6, 4.0334285e-6, 1.5925081e-5, 0.00018549155, 0.011250147, 0.31528232, 1.0, 0.3371831, 0.009120735],
            [0.13220899, 540.03644, 0.03029713, 0.045897152, 0.00011810919, 9.89651e-6, 2.3305486e-6, 1.8291772e-6, 4.019377e-6, 1.6007207e-5, 0.00018664864, 0.011278836, 0.31535387, 1.0, 0.33726916, 0.009147038],
            [0.08861497, 546.80756, 0.019194812, 0.045987688, 0.0001195504, 1.0101359e-5, 2.3986952e-6, 1.8839951e-6, 4.1142507e-6, 1.6313475e-5, 0.0001888857, 0.011294797, 0.31538668, 1.0, 0.3373032, 0.009164615],
            [0.05522105, 549.3488, 0.02172485, 0.046019413, 0.000120452336, 1.0374191e-5, 2.4760416e-6, 1.9522759e-6, 4.258435e-6, 1.6857797e-5, 0.00019194823, 0.011166613, 0.31504014, 1.0, 0.33688357, 0.009058035],
            [0.042087417, 555.1917, 0.0, 0.0460999, 0.0001198257, 1.0485649e-5, 2.4812841e-6, 1.9627378e-6, 4.335319e-6, 1.7260101e-5, 0.0001935556, 0.010952071, 0.31448472, 1.0, 0.33621442, 0.008873576],
            [0.0515418, 554.203, 0.09586353, 0.04608549, 0.000120529694, 1.0451533e-5, 2.5100026e-6, 1.9802433e-6, 4.29931e-6, 1.696826e-5, 0.00019235787, 0.011132497, 0.31494978, 1.0, 0.33677283, 0.009029218],
            [0.083152615, 554.19727, 0.1759599, 0.046086792, 0.00011968594, 1.0105404e-5, 2.388107e-6, 1.8781436e-6, 4.1175285e-6, 1.6365424e-5, 0.00018932584, 0.01128793, 0.31536725, 1.0, 0.33728075, 0.009159096],
            [0.12560184, 559.43207, 0.15351512, 0.046159495, 0.00011830553, 9.933275e-6, 2.3490943e-6, 1.8451185e-6, 4.039206e-6, 1.6047527e-5, 0.0001869003, 0.011283255, 0.31536305, 1.0, 0.33727992, 0.009151344],
            [0.1638923, 572.10205, 0.1415981, 0.046338234, 0.000117491734, 9.875692e-6, 2.3504924e-6, 1.8449307e-6, 4.0201503e-6, 1.592839e-5, 0.00018567577, 0.011256041, 0.31529865, 1.0, 0.33720317, 0.009125934],
            [0.19319583, 571.11676, 0.090415835, 0.046327844, 0.00011689717, 9.780043e-6, 2.3045707e-6, 1.8084904e-6, 3.9765114e-6, 1.583623e-5, 0.00018501598, 0.011237241, 0.3152538, 1.0, 0.3371512, 0.0091081085],
            [0.20007898, 571.2393, 0.08729006, 0.04632678, 0.00011696614, 9.864991e-6, 2.3647935e-6, 1.8597005e-6, 4.022394e-6, 1.587019e-5, 0.0001849294, 0.011232353, 0.31523803, 1.0, 0.3371316, 0.0091044195],
            [0.17414172, 720.4102, 0.04782176, 0.050928947, 0.0038554084, 0.0021476394, 0.0016315908, 0.0015729358, 0.001815268, 0.0024742526, 0.0046556764, 0.026270436, 0.34184825, 1.0, 0.36245677, 0.02198748],
        ];

        let sr = SR as usize;
        let audio: Vec<f32> = (0..sr * 2)
            .map(|i| {
                let t = i as f32 / sr as f32;
                let env = 0.3 + 0.2 * (2.0 * std::f32::consts::PI * 0.7 * t).sin();
                let a = (2.0 * std::f32::consts::PI * 220.0 * t).sin();
                let b = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
                let c = 0.25 * (2.0 * std::f32::consts::PI * 1760.0 * t).sin();
                env * (a + b + c) * 0.5
            })
            .collect();

        let mut d = Digester::new(Compat::Librosa, TickRate::Base);
        d.push(&audio);
        d.finish();
        let rows: Vec<Row> = std::iter::from_fn(|| d.pop()).collect();
        assert_eq!(rows.len(), BEFORE.len(), "the row count changed");
        for (i, (got, want)) in rows.iter().zip(BEFORE.iter()).enumerate() {
            assert_eq!(&got[..MUSIC], &want[..], "row {i}'s music prefix moved");
        }
    }

    #[test]
    fn no_tempo_lock_means_no_repeat() {
        // Decision 10: the detector runs on the beat grid, and with no tempo
        // lock there is no grid. Two seconds of digital silence neither locks
        // the tracker nor gives the detector anything, and `repeat()` must say
        // so rather than inventing a one-beat loop out of a flat vector.
        let mut d = Digester::new(Compat::Live, TickRate::Base);
        d.push(&vec![0.0f32; SR as usize * 2]);
        while d.pop().is_some() {}
        assert!(d.tempo().beats().is_none(), "silence must not lock the tempo");
        assert!(d.repeat().is_none(), "no tempo lock means no repeat");
    }

    #[test]
    fn the_digester_exposes_a_repeat_after_a_locked_repeating_signal() {
        // A steady four-on-the-floor at 128 BPM: the tempo tracker locks on
        // the pulse, and the same bars keep coming, so the detector has two
        // matching cycles to compare. The exact lock point is the detector's
        // business; what this seam test owns is that a locked, repeating
        // signal reaches `Digester::repeat()` at all.
        let bpm = 128.0f32;
        let beat_frames = (SR as f32 * 60.0 / bpm / HOP as f32) as usize;
        let period = (SR as f32 * 60.0 / bpm) as usize;
        let secs = 24.0;
        let n = (SR as f32 * secs) as usize;

        let mut audio = vec![0.0f32; n];
        // A kick on every beat and a snare on the backbeat of a two-beat
        // phrase, over a bass note that changes every two beats. That is a
        // repeating two-beat pattern with enough shape for the cosine to see.
        let mut beat = 0usize;
        let mut at = 0usize;
        while at < n {
            let is_backbeat = beat % 2 == 1;
            let freq = if beat % 4 < 2 { 110.0 } else { 165.0 };
            let decay = (period / 3).max(1);
            for i in 0..period.min(n - at) {
                let amp = (-(i as f32) / decay as f32).exp();
                let freq = if is_backbeat { 220.0 } else { freq };
                audio[at + i] += amp
                    * (2.0 * std::f32::consts::PI * freq * i as f32 / SR as f32).sin();
            }
            at += period;
            beat += 1;
            let _ = beat_frames;
        }

        let mut d = Digester::new(Compat::Live, TickRate::Base);
        d.push(&audio);
        d.finish();
        while d.pop().is_some() {}

        if d.tempo().beats().is_none() {
            // The tracker is not the subject of this test; if the synthetic
            // pulse did not lock it, there is nothing for the seam to pass on.
            eprintln!("skipping: the synthetic pulse did not lock the tempo");
            return;
        }
        assert!(
            d.repeat().is_some(),
            "a locked, repeating signal must surface through Digester::repeat()"
        );
    }

    #[test]
    fn the_two_compat_modes_differ_only_in_when_the_onset_lands() {
        let audio = sine(220.0, 2.0);
        let mut lib = Digester::new(Compat::Librosa, TickRate::Base);
        let mut live = Digester::new(Compat::Live, TickRate::Base);
        lib.push(&audio);
        live.push(&audio);
        let a: Vec<Row> = std::iter::from_fn(|| lib.pop()).collect();
        let b: Vec<Row> = std::iter::from_fn(|| live.pop()).collect();
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            for c in 0..FEATURES {
                if c == 2 {
                    continue; // the onset column is the whole point
                }
                assert!(
                    (x[c] - y[c]).abs() < 1e-6,
                    "row {i} column {} should not depend on the mode",
                    COLUMNS[c]
                );
            }
        }
    }

    // -- the tick ladder in the digester ------------------------------------

    /// The fine level is only a finer pooling of the *same* native frames
    /// (`HOP` 512 at 22 050 Hz is 43 Hz, faster than 32 Hz), so no fine tick
    /// can be empty and the window index never skips one.
    #[test]
    fn every_fine_tick_has_at_least_one_native_frame() {
        let mut d = Digester::new(Compat::Live, TickRate::Fine);
        d.push(&sine(440.0, 10.0));
        d.finish();
        let rows: Vec<Row> = std::iter::from_fn(|| d.pop()).collect();
        let frames = d.frames();
        // At least one native frame per emitted fine row...
        assert!(
            frames >= rows.len() as u64,
            "{frames} native frames for {} fine rows",
            rows.len()
        );
        // ...and the mapping cannot jump a window between two native frames,
        // which is what guarantees no tick is skipped mid-stream.
        let mut prev = 0u64;
        for f in 0..frames {
            let w = Digester::window_of(f, TickRate::Fine);
            assert!(
                w <= prev + 1,
                "frame {f} jumped from fine window {prev} to {w}, skipping one"
            );
            prev = w;
        }
        // 10 s of audio is ~320 fine ticks, four times the base rate.
        assert!((319..=322).contains(&rows.len()), "got {} fine rows", rows.len());
    }

    /// The base digest and the four-way pooled projection of the fine digest
    /// agree on the music prefix. Weighting by the native frames each fine
    /// window actually holds is what makes them exact: a fine window holds one
    /// or two frames (1.35 on average), so an unweighted mean of four fine
    /// means would not equal the base window's own mean.
    ///
    /// Only columns `0..MUSIC` are compared. The voice path is pooled by its own
    /// 10 ms-frame rule (decision 2), not by these columns' rules, so it is the
    /// voice analyser's business, not the music window's.
    #[test]
    fn the_direct_base_digester_equals_the_pooled_fine_one() {
        let sr = SR as usize;
        let audio: Vec<f32> = (0..sr * 3)
            .map(|i| {
                let t = i as f32 / sr as f32;
                let env = 0.4 + 0.2 * (2.0 * std::f32::consts::PI * 0.9 * t).sin();
                env * ((2.0 * std::f32::consts::PI * 220.0 * t).sin()
                    + 0.5 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                    + 0.25 * (2.0 * std::f32::consts::PI * 1760.0 * t).sin())
                    * 0.5
            })
            .collect();

        let mut fine = Digester::new(Compat::Librosa, TickRate::Fine);
        fine.push(&audio);
        fine.finish();
        let fine_rows: Vec<Row> = std::iter::from_fn(|| fine.pop()).collect();
        let nframes = fine.frames();

        // How many native frames fell in each fine window — the weight.
        let mut counts = vec![0i64; fine_rows.len()];
        for f in 0..nframes {
            let w = Digester::window_of(f, TickRate::Fine) as usize;
            if w < counts.len() {
                counts[w] += 1;
            }
        }

        let mut base = Digester::new(Compat::Librosa, TickRate::Base);
        base.push(&audio);
        base.finish();
        let base_rows: Vec<Row> = std::iter::from_fn(|| base.pop()).collect();

        assert!(base_rows.len() >= 4, "not enough base rows to compare");
        let groups = (fine_rows.len() / 4).min(base_rows.len());
        for k in 0..groups {
            let group = &fine_rows[4 * k..4 * k + 4];
            let w = &counts[4 * k..4 * k + 4];
            for c in 0..MUSIC {
                let want = match metal_schema::pool_rule(c) {
                    metal_schema::Pool::Mean => {
                        let num: f64 =
                            group.iter().zip(w).map(|(r, &n)| r[c] as f64 * n as f64).sum();
                        let den: f64 = w.iter().map(|&n| n as f64).sum();
                        (num / den) as f32
                    }
                    metal_schema::Pool::Max => {
                        group.iter().map(|r| r[c]).fold(f32::NEG_INFINITY, f32::max)
                    }
                    metal_schema::Pool::Median => {
                        let mut v: Vec<f32> = group.iter().map(|r| r[c]).collect();
                        v.sort_by(f32::total_cmp);
                        (v[1] as f64 + v[2] as f64) as f32 / 2.0
                    }
                    metal_schema::Pool::Last => group[3][c],
                };
                let got = base_rows[k][c];
                assert!(
                    (got - want).abs() <= 1e-4 * (1.0 + want.abs()),
                    "base row {k} column {c} ({}) is {got}, pooled fine is {want}",
                    COLUMNS[c]
                );
            }
        }
    }
}
