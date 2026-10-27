//! The voice analyser — a pure function of a 16 kHz PCM stream.
//!
//! `the voice design notes` decision 3: `Analyser::push(&[f32])` in, `pop() -> VoiceRow`
//! out, at whatever [`TickRate`] the source runs. It knows nothing about mix
//! vs stem; `Digester` owns one and fills columns 16..29 (V2). When stems
//! land, the same analyser is fed the `vocals` stem and the contract does not
//! change.
//!
//! # It is causal
//!
//! Every frame looks only backwards (see [`yin`]), and a tick is not closed
//! until a frame belonging to the *next* tick arrives. A frame at time `t` can
//! therefore never be affected by audio after `t`. `flush` closes the trailing
//! partial tick for offline use — that one row is not causal and is only used
//! at the end of a file.
//!
//! # The thirteen columns
//!
//! [`VOICE_COLUMNS`], all `0..1`, pooled over the tick by the rule in decision
//! 2: mean for continuous columns, median for pitch, max for hiss, last value
//! for steady/syllables/style.
//!
//! # The tick rate
//!
//! [`TickRate`] here is a local stand-in for `metal_schema::TickRate` (V2b). The
//! analyser works on fixed 10 ms frames and pools them into ticks; the DSP is
//! identical at every level, only the pooling changes.

pub mod envelope;
pub mod lpc;
pub mod style;
pub mod vad;
pub mod yin;

use std::collections::VecDeque;

use envelope::{BandEnergy, Bands};
use lpc::{Formants, Lpc, LPC_WIN};
use style::{Features, Scores};
use vad::Vad;
use yin::Yin;

pub use yin::{F0_MAX, F0_MIN};

/// A 10 ms frame at 16 kHz.
pub const FRAME: usize = 160;
/// ~1.2 s of frames, the history the style rule reads.
pub const HISTORY: usize = 120;
/// YIN periodicity above this counts as voiced.
pub const VOICED_PERIODICITY: f32 = 0.45;
/// Speech-band share required for the *voiced* presence branch.
pub const SPEECH_SHARE: f32 = 0.5;
/// Speech-band share required for the *unvoiced* (whisper) branch.
pub const SPEECH_SHARE_UNVOICED: f32 = 0.30;
/// Semitones of jitter that drive steady to zero.
const STEADY_JITTER: f32 = 0.4;
/// F1 map: 250–900 Hz → 0..1 (jaw openness).
const OPEN_LO: f32 = 250.0;
const OPEN_HI: f32 = 900.0;
/// F2 map: 700–2500 Hz → 0..1 (spread vs round).
const WIDE_LO: f32 = 700.0;
const WIDE_HI: f32 = 2500.0;

/// A tick level — the one ladder, declared in `metal_schema` (V2b). Re-exported
/// so `voice::TickRate` keeps reading as this module's own type; there is no
/// second enum and no local stand-in any more.
pub use metal_schema::TickRate;

/// Frames per tick at `rate` (not necessarily whole — 12.5 at 8 Hz).
pub fn frames_per_tick(rate: TickRate) -> f32 {
    rate.hz() as f32 * 0.01
}

/// The thirteen voice columns, in order — declared once in
/// [`metal_schema::VOICE_COLUMNS`]. Indices 0..13 of a [`VoiceRow`].
pub const VOICE_COLUMNS: [&str; metal_schema::VOICE] = metal_schema::VOICE_COLUMNS;

pub const VOICE_FEATURES: usize = metal_schema::VOICE;
/// `voice` — fused presence.
pub const VOICE: usize = 0;
pub const VOICE_LEVEL: usize = 1;
pub const PITCH: usize = 2;
pub const VOICING: usize = 3;
pub const STEADY: usize = 4;
pub const SYLLABLES: usize = 5;
pub const OPEN: usize = 6;
pub const WIDE: usize = 7;
pub const HISS: usize = 8;
pub const SING: usize = 9;
pub const RAP: usize = 10;
pub const SPEAK: usize = 11;
pub const WHISPER: usize = 12;

/// One tick of voice: thirteen `0..1` values.
pub type VoiceRow = [f32; VOICE_FEATURES];

/// One 10 ms frame's measurements, before pooling.
#[derive(Clone, Copy, Debug)]
struct Frame {
    presence: bool,
    f0: f32,
    periodicity: f32,
    speech_power: f32,
}

impl Frame {
    fn voiced(&self) -> bool {
        self.periodicity > VOICED_PERIODICITY && self.f0 > 0.0
    }
}

/// What the tick currently being filled has accumulated.
#[derive(Default)]
struct Tick {
    /// The tick index, or `i64::MIN` before the first frame.
    index: i64,
    frames: u32,
    presence: f32,
    voiced: u32,
    pitch: Vec<f32>,
    voicing: f32,
    level: f32,
    open: f32,
    wide: f32,
    hiss: f32,
}

impl Tick {
    fn start(&mut self, index: i64) {
        self.index = index;
        self.frames = 0;
        self.presence = 0.0;
        self.voiced = 0;
        self.pitch.clear();
        self.voicing = 0.0;
        self.level = 0.0;
        self.open = 0.0;
        self.wide = 0.0;
        self.hiss = 0.0;
    }

    fn has_frames(&self) -> bool {
        self.frames > 0
    }
}

/// The analyser. Feed it mono 16 kHz samples.
pub struct Analyser {
    rate: TickRate,
    vad: Vad,
    yin: Yin,
    bands: BandEnergy,
    lpc: Lpc,

    /// Samples not yet formed into a whole 40 ms window.
    pending: Vec<f32>,
    /// The window currently being analysed (YIN's 40 ms).
    window: Vec<f32>,
    frame_index: u64,

    history: VecDeque<Frame>,
    /// Running peak speech-band dB, so `level` is a moving reference.
    peak_db: f32,

    tick: Tick,
    out: VecDeque<VoiceRow>,
}

impl Analyser {
    pub fn new(rate: TickRate) -> Analyser {
        Analyser {
            rate,
            vad: Vad::new(),
            yin: Yin::new(),
            bands: BandEnergy::new(),
            lpc: Lpc::new(),
            pending: Vec::new(),
            window: vec![0.0; yin::W],
            frame_index: 0,
            history: VecDeque::with_capacity(HISTORY),
            peak_db: -120.0,
            tick: Tick { index: i64::MIN, ..Tick::default() },
            out: VecDeque::new(),
        }
    }

    pub fn rate(&self) -> TickRate {
        self.rate
    }

    /// Feed mono 16 kHz samples. Anything else must be resampled first.
    pub fn push(&mut self, samples: &[f32]) {
        self.pending.extend_from_slice(samples);
        while self.pending.len() >= yin::W {
            self.window.copy_from_slice(&self.pending[..yin::W]);
            self.analyze_frame();
            self.pending.drain(..FRAME);
        }
    }

    /// Close the trailing partial tick, for the end of a file only.
    pub fn flush(&mut self) {
        if self.tick.has_frames() {
            let row = self.close_tick();
            self.pending.clear();
            self.out.push_back(row);
        }
    }

    pub fn pop(&mut self) -> Option<VoiceRow> {
        self.out.pop_front()
    }

    pub fn available(&self) -> usize {
        self.out.len()
    }

    fn analyze_frame(&mut self) {
        let w = &self.window;
        let last = &w[yin::W - FRAME..];
        let vad = self.vad.is_voice(last);
        let (f0, periodicity) = self.yin.analyze(w);
        let bands: Bands = self.bands.analyze(&w[yin::W - envelope::BAND_FFT..]);
        let formants: Formants = self.lpc.analyze(&w[yin::W - LPC_WIN..]);

        let speech_db = 10.0 * (bands.speech_power + 1e-12).log10();
        self.peak_db = self.peak_db.max(speech_db);

        let voiced_presence =
            vad && bands.speech_share > SPEECH_SHARE && periodicity > VOICED_PERIODICITY;
        // Whisper is speech too: it is unvoiced, so it must not need YIN. The
        // plan's `voice` definition carries periodicity but also calls whisper
        // "presence with low voicing", so the unvoiced branch is explicit here.
        let unvoiced_presence = vad
            && bands.speech_share > SPEECH_SHARE_UNVOICED
            && bands.hiss_share > envelope::HISS_GATE
            && periodicity <= VOICED_PERIODICITY;
        let presence = voiced_presence || unvoiced_presence;

        let frame = Frame {
            presence,
            f0,
            periodicity,
            speech_power: bands.speech_power,
        };

        // Open the tick bookkeeping before pooling this frame.
        let ti = (self.frame_index as f32 * self.rate.hz() as f32 / 100.0).floor() as i64;
        if self.tick.index == i64::MIN {
            self.tick.start(ti);
        }
        while ti > self.tick.index {
            let row = self.close_tick();
            self.out.push_back(row);
            self.tick.start(self.tick.index + 1);
        }

        // The tick that just closed must keep the history it had at its close;
        // the current frame belongs to the new tick and is added after.
        self.tick.frames += 1;
        self.tick.presence += presence as u32 as f32;
        self.tick.hiss = self.tick.hiss.max(bands.hiss_share);
        if frame.voiced() {
            self.tick.voiced += 1;
            self.tick.pitch.push(f0);
            self.tick.voicing += periodicity;
            let level = ((speech_db - (self.peak_db - 40.0)) / 40.0).clamp(0.0, 1.0);
            self.tick.level += level;
            if formants.present && formants.f1 > 0.0 {
                self.tick.open += ((formants.f1 - OPEN_LO) / (OPEN_HI - OPEN_LO)).clamp(0.0, 1.0);
            }
            if formants.present && formants.f2 > 0.0 {
                self.tick.wide += ((formants.f2 - WIDE_LO) / (WIDE_HI - WIDE_LO)).clamp(0.0, 1.0);
            }
        }

        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(frame);
        self.frame_index += 1;
    }

    fn close_tick(&mut self) -> VoiceRow {
        let mut row = [0.0f32; VOICE_FEATURES];
        if self.tick.frames == 0 {
            return row;
        }
        let n = self.tick.frames as f32;
        let voice = (self.tick.presence / n).clamp(0.0, 1.0);
        row[VOICE] = voice;

        if self.tick.voiced > 0 {
            let v = self.tick.voiced as f32;
            self.tick.pitch.sort_by(f32::total_cmp);
            let median = self.tick.pitch[self.tick.pitch.len() / 2];
            row[PITCH] = pitch_normalised(median);
            row[VOICING] = (self.tick.voicing / v).clamp(0.0, 1.0);
            row[VOICE_LEVEL] = (self.tick.level / v).clamp(0.0, 1.0);
            row[OPEN] = (self.tick.open / v).clamp(0.0, 1.0);
            row[WIDE] = (self.tick.wide / v).clamp(0.0, 1.0);
        }
        row[HISS] = self.tick.hiss.clamp(0.0, 1.0);

        // Anything that only makes sense with a voice is zero when there is
        // none, so silence is all zeros rather than a stray one.
        if voice > 0.0 {
            let steady = steady_of(&self.history);
            let syllables = envelope::syllable_rate(&envelope_of(&self.history)) / 12.0;
            row[STEADY] = steady;
            row[SYLLABLES] = syllables.clamp(0.0, 1.0);
            let features = Features {
                voice,
                voicing: row[VOICING],
                steady,
                syllables: row[SYLLABLES],
                hiss: row[HISS],
                voiced_run: voiced_run_of(&self.history),
                beat_phase: None,
            };
            let Scores { sing, rap, speak, whisper } = style::score(&features);
            row[SING] = sing;
            row[RAP] = rap;
            row[SPEAK] = speak;
            row[WHISPER] = whisper;
        }
        row
    }
}

fn pitch_normalised(f0: f32) -> f32 {
    if f0 <= F0_MIN {
        return 0.0;
    }
    (f0 / F0_MIN).log2() / (F0_MAX / F0_MIN).log2()
}

/// F0 stability from the last ~400 ms: semitone jitter between adjacent voiced
/// frames, mapped so `0.4` semitones of jitter is zero steady.
fn steady_of(history: &VecDeque<Frame>) -> f32 {
    let recent: Vec<f32> = history
        .iter()
        .rev()
        .take(40)
        .filter(|f| f.voiced())
        .map(|f| f.f0)
        .collect();
    if recent.len() < 2 {
        return 0.0;
    }
    // `recent` is newest-first; differences are the same either way.
    let mut total = 0.0f32;
    for w in recent.windows(2) {
        total += (12.0 * (w[0] / w[1]).log2()).abs();
    }
    let jitter = total / (recent.len() - 1) as f32;
    (1.0 - (jitter / STEADY_JITTER).clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

fn envelope_of(history: &VecDeque<Frame>) -> Vec<f32> {
    history.iter().map(|f| f.speech_power).collect()
}

fn voiced_run_of(history: &VecDeque<Frame>) -> f32 {
    if history.is_empty() {
        return 0.0;
    }
    let (mut best, mut run) = (0usize, 0usize);
    for f in history {
        if f.presence {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best as f32 / history.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rate: TickRate, audio: &[f32]) -> Vec<VoiceRow> {
        let mut a = Analyser::new(rate);
        a.push(audio);
        a.flush();
        let mut rows = Vec::new();
        while let Some(r) = a.pop() {
            rows.push(r);
        }
        rows
    }

    fn last(rate: TickRate, audio: &[f32]) -> VoiceRow {
        *run(rate, audio).last().expect("at least one tick")
    }

    fn sine(freq: f32, secs: f32, amp: f32) -> Vec<f32> {
        let n = (yin::VOICE_SR as f32 * secs) as usize;
        (0..n)
            .map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / yin::VOICE_SR as f32).sin())
            .collect()
    }

    /// A glottal-like tone: harmonics `1/n` up to 7.5 kHz, frequency `f(t)`.
    /// A bare sine has no speech-band energy and WebRTC VAD rightly ignores it;
    /// a real sung tone is not a sine, so the test is not either.
    fn sung(f: impl Fn(f32) -> f32, secs: f32, amp: f32) -> Vec<f32> {
        let sr = yin::VOICE_SR as f32;
        let n = (sr * secs) as usize;
        let mut phase = 0.0f32;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f32 / sr;
            let f0 = f(t).max(1.0);
            phase += std::f32::consts::TAU * f0 / sr;
            let max_harmonic = (7500.0 / f0) as usize;
            let mut v = 0.0;
            for h in 1..=max_harmonic {
                v += (h as f32 * phase).sin() / h as f32;
            }
            out.push(amp * v * 0.6);
        }
        out
    }

    fn white_noise(secs: f32, amp: f32, seed: u32) -> Vec<f32> {
        // xorshift, so the test has no dependency and is deterministic.
        let n = (yin::VOICE_SR as f32 * secs) as usize;
        let mut s = seed.max(1);
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp
            })
            .collect()
    }

    #[test]
    fn a_steady_sine_with_vibrato_reads_as_sing() {
        // A sung 330 Hz with ±0.4 semitone vibrato at 5 Hz for two seconds. F0
        // is inside the speech band; a 220 Hz fundamental would sit below the
        // 300 Hz edge and the band share would read low for the wrong reason.
        let audio = sung(
            |t| 330.0 * 2.0f32.powf(0.4 / 1200.0 * (std::f32::consts::TAU * 5.0 * t).sin()),
            2.0,
            0.5,
        );
        let row = last(TickRate::Fine, &audio);
        assert!(row[VOICE] > 0.3, "presence {} too low", row[VOICE]);
        assert!(row[STEADY] > 0.6, "steady {} should be high for a held note", row[STEADY]);
        let best = row[SING].max(row[RAP]).max(row[SPEAK]).max(row[WHISPER]);
        assert_eq!(best, row[SING], "sing should win: {:?}", &row[9..13]);
    }

    #[test]
    fn pink_noise_drums_have_no_voice() {
        // Dark noise plus a 60 Hz kick every half second: no speech band to
        // speak of, so the fused presence stays down.
        let sr = yin::VOICE_SR as f32;
        let secs = 2.0;
        let n = (sr * secs) as usize;
        let noise = white_noise(secs, 0.3, 12345);
        // One-pole low-pass at ~300 Hz keeps energy below the speech band.
        let alpha = 1.0 - (-std::f32::consts::TAU * 300.0 / sr).exp();
        let mut lp = 0.0f32;
        let mut audio = Vec::with_capacity(n);
        for (i, &xn) in noise.iter().enumerate() {
            lp += alpha * (xn - lp);
            let t = i as f32 / sr;
            let kick = {
                let phase = t % 0.5;
                let env = (-phase * 30.0).exp();
                env * (std::f32::consts::TAU * 60.0 * phase).sin()
            };
            audio.push(lp * 0.5 + kick * 0.7);
        }
        let rows = run(TickRate::Fine, &audio);
        let mean = |c: usize| rows.iter().map(|r| r[c]).sum::<f32>() / rows.len() as f32;
        assert!(mean(VOICE) < 0.4, "pink noise + drums read as voice {}", mean(VOICE));
        assert!(mean(SING) < 0.3, "sing {}", mean(SING));
        assert!(mean(WHISPER) < 0.3, "whisper {}", mean(WHISPER));
    }

    #[test]
    fn band_limited_noise_bursts_read_as_whisper() {
        // Broadband noise through a 300 Hz high-pass: speech-band and hiss
        // share both present, no periodicity. Amplitude-modulated into bursts.
        let sr = yin::VOICE_SR as f32;
        let secs = 2.0;
        let raw = white_noise(secs, 0.4, 999);
        let rc = 1.0 / (std::f32::consts::TAU * 300.0);
        let dt = 1.0 / sr;
        let mut prev_in = 0.0f32;
        let mut prev_out = 0.0f32;
        let mut audio = Vec::with_capacity(raw.len());
        for (i, &x) in raw.iter().enumerate() {
            let y = rc / (rc + dt) * (prev_out + x - prev_in);
            prev_in = x;
            prev_out = y;
            let t = i as f32 / sr;
            let burst = 0.5 + 0.5 * (std::f32::consts::TAU * 5.0 * t).sin();
            audio.push(y * burst);
        }
        let rows = run(TickRate::Fine, &audio);
        let mean = |c: usize| rows.iter().map(|r| r[c]).sum::<f32>() / rows.len() as f32;
        assert!(mean(VOICE) > 0.2, "unvoiced speech should still be present {}", mean(VOICE));
        assert!(mean(WHISPER) > mean(SING), "whisper {} vs sing {}", mean(WHISPER), mean(SING));
        assert!(mean(WHISPER) > mean(SPEAK), "whisper {} vs speak {}", mean(WHISPER), mean(SPEAK));
    }

    #[test]
    fn a_glide_lowers_steady() {
        let held = sung(|_| 330.0, 0.6, 0.5);
        // A fast portamento 330→990 Hz over the whole clip, so the last 400 ms
        // (the steady window) is still moving.
        let glide = sung(|t| 330.0 + 660.0 * (t / 0.6).clamp(0.0, 1.0), 0.6, 0.5);
        let s_held = last(TickRate::Fine, &held);
        let s_glide = last(TickRate::Fine, &glide);
        assert!(
            s_glide[STEADY] + 0.2 < s_held[STEADY],
            "glide steady {} should be well below held steady {}",
            s_glide[STEADY],
            s_held[STEADY]
        );
    }

    #[test]
    fn silence_is_all_zero_not_nan() {
        let rows = run(TickRate::Fine, &vec![0.0f32; yin::VOICE_SR as usize * 2]);
        assert!(!rows.is_empty());
        for row in &rows {
            for (c, &v) in row.iter().enumerate() {
                assert!(v.is_finite(), "column {} ({}) is not finite", c, VOICE_COLUMNS[c]);
                assert!(v.abs() < 1e-6, "column {} ({}) is {v} on silence", c, VOICE_COLUMNS[c]);
            }
        }
    }

    #[test]
    fn the_analyser_is_causal() {
        // A prefix that ends mid-tick, and a suffix that differs. Every tick the
        // prefix closes on its own must be identical in both runs: the extra
        // audio is after those ticks and may not reach back.
        let prefix = sine(180.0, 1.0, 0.5);
        let mut suffix = sine(600.0, 0.8, 0.7);
        suffix.extend(white_noise(0.2, 0.5, 7));

        let mut a = Analyser::new(TickRate::Fine);
        a.push(&prefix);
        let mut rows_a = Vec::new();
        while let Some(r) = a.pop() {
            rows_a.push(r);
        }

        let mut b = Analyser::new(TickRate::Fine);
        b.push(&prefix);
        b.push(&suffix);
        let mut rows_b = Vec::new();
        while let Some(r) = b.pop() {
            rows_b.push(r);
        }

        assert!(!rows_a.is_empty(), "the prefix must close at least one tick");
        for (i, (x, y)) in rows_a.iter().zip(&rows_b).enumerate() {
            for c in 0..VOICE_FEATURES {
                assert!(
                    (x[c] - y[c]).abs() < 1e-6,
                    "tick {i} column {} ({}) changed with later audio: {} vs {}",
                    c,
                    VOICE_COLUMNS[c],
                    x[c],
                    y[c]
                );
            }
        }
    }
}
