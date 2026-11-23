//! P-D — SMPTE timecode in: `clock.timecode`.
//!
//! Three wires carry the same `hh:mm:ss:ff` and all three end here:
//!
//! - **LTC** — the biphase-mark audio signal on one channel of an audio input
//!   ([`LtcDecoder`]; [`ltc_encode`] synthesizes it, for the tests and for a
//!   future "generate").
//! - **MTC** — MIDI quarter-frame (`F1 0nnn dddd`) and full-frame SysEx
//!   ([`MtcAssembler`]); the bytes come from `midir` in `metal-dash`.
//! - **Art-Net ArtTimeCode** — `crate::artnet::parse_timecode`.
//!
//! What leaves is a position in seconds ([`Timecode::to_seconds`]), and a
//! [`Chase`] that turns "last heard" into "where the playhead is now":
//! `follow` stops within a few frames of the timecode stopping, `freewheel N`
//! keeps running N frames through a dropout. Nothing here opens a device —
//! `metal-dash::timecode` owns the readers — so everything is testable with
//! synthesized audio and packets.

use serde::{Deserialize, Serialize};
use web_time::{Duration, Instant};

/// The node kind.
pub const TIMECODE_KIND: &str = metal_graph::CLOCK_TIMECODE_KIND;

/// The four SMPTE rates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fps {
    #[serde(rename = "24")]
    F24,
    #[serde(rename = "25")]
    F25,
    #[serde(rename = "29.97df")]
    F2997Df,
    #[serde(rename = "30")]
    F30,
}

impl Fps {
    pub const ALL: [Fps; 4] = [Fps::F24, Fps::F25, Fps::F2997Df, Fps::F30];

    /// Frames per second as counted (30 for drop-frame).
    pub fn nominal(self) -> u32 {
        match self {
            Fps::F24 => 24,
            Fps::F25 => 25,
            Fps::F2997Df | Fps::F30 => 30,
        }
    }

    /// Real frames per second.
    pub fn rate(self) -> f64 {
        match self {
            Fps::F2997Df => 30_000.0 / 1001.0,
            f => f64::from(f.nominal()),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Fps::F24 => "24",
            Fps::F25 => "25",
            Fps::F2997Df => "29.97df",
            Fps::F30 => "30",
        }
    }

    pub fn from_label(s: &str) -> Option<Fps> {
        Fps::ALL.into_iter().find(|f| f.label() == s)
    }

    /// ArtTimeCode's `Type` and MTC's rate bits share this order.
    pub fn artnet_type(self) -> u8 {
        match self {
            Fps::F24 => 0,
            Fps::F25 => 1,
            Fps::F2997Df => 2,
            Fps::F30 => 3,
        }
    }

    pub fn from_artnet_type(t: u8) -> Option<Fps> {
        Fps::ALL.get(t as usize).copied()
    }
}

/// One SMPTE address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timecode {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub fps: Fps,
}

impl Timecode {
    /// The frame count since 00:00:00:00, drop-frame aware.
    pub fn frame_count(&self) -> u64 {
        let n = u64::from(self.fps.nominal());
        let secs = u64::from(self.hours) * 3600 + u64::from(self.minutes) * 60 + u64::from(self.seconds);
        let mut frames = secs * n + u64::from(self.frames);
        if self.fps == Fps::F2997Df {
            let total_minutes = u64::from(self.hours) * 60 + u64::from(self.minutes);
            frames -= 2 * (total_minutes - total_minutes / 10);
        }
        frames
    }

    /// Seconds since midnight, on the real clock.
    pub fn to_seconds(&self) -> f64 {
        self.frame_count() as f64 / self.fps.rate()
    }

    /// The address of the frame `secs` falls in.
    pub fn from_seconds(secs: f64, fps: Fps) -> Timecode {
        let mut frames = (secs.max(0.0) * fps.rate() + 1e-6).floor() as u64;
        if fps == Fps::F2997Df {
            let d = frames / 17_982;
            let m = frames % 17_982;
            frames += 18 * d + if m < 2 { 0 } else { 2 * ((m - 2) / 1798) };
        }
        let n = u64::from(fps.nominal());
        Timecode {
            frames: (frames % n) as u8,
            seconds: ((frames / n) % 60) as u8,
            minutes: ((frames / (n * 60)) % 60) as u8,
            hours: ((frames / (n * 3600)) % 24) as u8,
            fps,
        }
    }

    pub fn text(&self) -> String {
        let sep = if self.fps == Fps::F2997Df { ';' } else { ':' };
        format!("{:02}:{:02}:{:02}{sep}{:02}", self.hours, self.minutes, self.seconds, self.frames)
    }
}

// ---------------------------------------------------------------------------
// LTC
// ---------------------------------------------------------------------------

/// The sync word, bits 64..80 of a frame read LSB-first.
const LTC_SYNC: u16 = 0xBFFC;

/// The 80 bits of one LTC frame, bit 0 first on the wire.
pub fn ltc_bits(tc: &Timecode) -> [bool; 80] {
    let mut b = [false; 80];
    let mut put = |at: usize, width: usize, v: u8| {
        for i in 0..width {
            b[at + i] = (v >> i) & 1 == 1;
        }
    };
    put(0, 4, tc.frames % 10);
    put(8, 2, tc.frames / 10);
    put(10, 1, u8::from(tc.fps == Fps::F2997Df));
    put(16, 4, tc.seconds % 10);
    put(24, 3, tc.seconds / 10);
    put(32, 4, tc.minutes % 10);
    put(40, 3, tc.minutes / 10);
    put(48, 4, tc.hours % 10);
    put(56, 2, tc.hours / 10);
    for i in 0..16 {
        b[64 + i] = (LTC_SYNC >> i) & 1 == 1;
    }
    // Bit 27 (polarity correction) makes the frame's count of zeros even, so
    // every frame starts on the same level — set it the way a generator does.
    let zeros = b.iter().filter(|x| !**x).count();
    if zeros % 2 == 1 {
        b[27] = true;
    }
    b
}

/// A streaming LTC encoder: biphase mark, a transition at every bit edge and
/// one more mid-bit for a 1. It keeps the level and the sample clock across
/// frames, so consecutive frames join without a click or a lost edge.
pub struct LtcEncoder {
    fps: Fps,
    rate: u32,
    amp: f32,
    level: f32,
    /// Seconds since the first bit.
    t: f64,
    /// Samples emitted so far.
    emitted: usize,
}

impl LtcEncoder {
    /// An encoder at `rate` Hz, amplitude ±`amp`, for `fps` frames.
    pub fn new(fps: Fps, rate: u32, amp: f32) -> LtcEncoder {
        LtcEncoder { fps, rate, amp, level: amp, t: 0.0, emitted: 0 }
    }

    /// Appends the 80 bits of `tc` to `out`.
    pub fn push_frame(&mut self, tc: &Timecode, out: &mut Vec<f32>) {
        let bit_s = 1.0 / (self.fps.rate() * 80.0);
        let rate = f64::from(self.rate);
        for bit in ltc_bits(tc) {
            self.level = -self.level;
            let half = self.t + bit_s / 2.0;
            let end = self.t + bit_s;
            while (self.emitted as f64) / rate < half {
                out.push(self.level);
                self.emitted += 1;
            }
            if bit {
                self.level = -self.level;
            }
            while (self.emitted as f64) / rate < end {
                out.push(self.level);
                self.emitted += 1;
            }
            self.t = end;
        }
    }
}

/// LTC audio for `frames` consecutive frames starting at `start`, at `rate`
/// Hz, amplitude ±`amp` (see [`LtcEncoder`]).
pub fn ltc_encode(start: &Timecode, frames: usize, rate: u32, amp: f32) -> Vec<f32> {
    let mut enc = LtcEncoder::new(start.fps, rate, amp);
    let mut out = Vec::new();
    let mut first = start.frame_count();
    for _ in 0..frames {
        let tc = Timecode::from_seconds((first as f64 + 0.5) / start.fps.rate(), start.fps);
        first += 1;
        enc.push_frame(&tc, &mut out);
    }
    out
}

/// Frames of lead [`LtcGen`] keeps ahead of the playhead.
const LTC_LEAD_FRAMES: u64 = 1;

/// LTC generation (`clock.timecode` in generate mode): the frames that are
/// due at a playhead, encoded in order. The playhead is a position in seconds
/// on the sequence clock; the frame at it is the frame the chain carries.
pub struct LtcGen {
    fps: Fps,
    enc: LtcEncoder,
    /// Frame count of the next frame to encode, `None` before the first call.
    next: Option<u64>,
}

impl LtcGen {
    pub fn new(fps: Fps, rate: u32, amp: f32) -> LtcGen {
        LtcGen { fps, enc: LtcEncoder::new(fps, rate, amp), next: None }
    }

    /// Appends the samples of every frame up to [`LTC_LEAD_FRAMES`] past the
    /// playhead. A playhead that jumps back, or that the chain fell behind,
    /// restarts the chain at the playhead (a click, once, at a seek).
    pub fn due(&mut self, playhead: f64, out: &mut Vec<f32>) {
        let now = Timecode::from_seconds(playhead.max(0.0), self.fps).frame_count();
        let restart = match self.next {
            None => true,
            Some(n) => n < now || n > now + 8 * LTC_LEAD_FRAMES,
        };
        if restart {
            self.enc = LtcEncoder::new(self.fps, self.enc.rate, self.enc.amp);
            self.next = Some(now);
        }
        let Some(mut n) = self.next else { return };
        while n <= now + LTC_LEAD_FRAMES {
            let tc = Timecode::from_seconds((n as f64 + 0.5) / self.fps.rate(), self.fps);
            self.enc.push_frame(&tc, out);
            n += 1;
        }
        self.next = Some(n);
    }
}

/// Biphase-mark LTC decoder over a stream of mono samples. Adapts to the
/// bit rate (so it locks to 24, 25, 29.97 and 30 alike and to tape that
/// drifts), reads the frame when the sync word lands, and reports the rate
/// from the measured bit period and the drop-frame flag.
pub struct LtcDecoder {
    rate: u32,
    /// Samples since the last transition.
    since: f64,
    high: bool,
    /// Estimated samples per bit.
    period: f64,
    half_pending: bool,
    reg: u128,
    /// Samples fed in total (the decoded frame's end position).
    pos: u64,
    /// Peak follower, for the zero-crossing hysteresis.
    peak: f32,
}

impl LtcDecoder {
    pub fn new(rate: u32) -> LtcDecoder {
        LtcDecoder {
            rate,
            since: 0.0,
            high: false,
            period: f64::from(rate) / (80.0 * 25.0),
            half_pending: false,
            reg: 0,
            pos: 0,
            peak: 0.0,
        }
    }

    fn bit(&mut self, b: bool) -> Option<Timecode> {
        self.reg = (self.reg >> 1) | (u128::from(b) << 79);
        if ((self.reg >> 64) & 0xffff) as u16 != LTC_SYNC {
            return None;
        }
        let r = self.reg;
        let f = |at: u32, w: u32| ((r >> at) & ((1u128 << w) - 1)) as u8;
        let drop = f(10, 1) == 1;
        let measured = f64::from(self.rate) / (self.period * 80.0);
        let fps = if drop {
            Fps::F2997Df
        } else if measured < 24.5 {
            Fps::F24
        } else if measured < 27.5 {
            Fps::F25
        } else {
            Fps::F30
        };
        let tc = Timecode {
            frames: f(0, 4) + 10 * f(8, 2),
            seconds: f(16, 4) + 10 * f(24, 3),
            minutes: f(32, 4) + 10 * f(40, 3),
            hours: f(48, 4) + 10 * f(56, 2),
            fps,
        };
        let valid = tc.frames < fps.nominal() as u8 && tc.seconds < 60 && tc.minutes < 60 && tc.hours < 24;
        valid.then_some(tc)
    }

    fn interval(&mut self, iv: f64) -> Option<Timecode> {
        if iv > self.period * 0.75 {
            // A whole cell: a zero. Too long is a dropout: resynchronize.
            if iv > self.period * 1.6 {
                self.half_pending = false;
                return None;
            }
            self.period = self.period * 0.9 + iv * 0.1;
            self.half_pending = false;
            self.bit(false)
        } else {
            self.period = self.period * 0.9 + iv * 2.0 * 0.1;
            if self.half_pending {
                self.half_pending = false;
                self.bit(true)
            } else {
                self.half_pending = true;
                None
            }
        }
    }

    /// Feeds samples; returns every frame completed in them with the sample
    /// position (since the decoder started) where its sync word ended.
    pub fn push(&mut self, samples: &[f32]) -> Vec<(Timecode, u64)> {
        let mut out = Vec::new();
        for &s in samples {
            self.pos += 1;
            self.peak = (self.peak * 0.9995).max(s.abs());
            let thr = (self.peak * 0.15).max(1e-4);
            self.since += 1.0;
            let flip = if self.high { s < -thr } else { s > thr };
            if flip {
                self.high = !self.high;
                let iv = self.since;
                self.since = 0.0;
                if let Some(tc) = self.interval(iv) {
                    out.push((tc, self.pos));
                }
            }
        }
        out
    }

    /// The sample rate it was built for.
    pub fn rate(&self) -> u32 {
        self.rate
    }
}

// ---------------------------------------------------------------------------
// MTC
// ---------------------------------------------------------------------------

/// Assembles MIDI Time Code from quarter-frame messages and full-frame
/// SysEx. A quarter-frame time is complete after piece 7 and describes the
/// frame two frames back (eight quarter frames span two frames).
#[derive(Default)]
pub struct MtcAssembler {
    pieces: [u8; 8],
    seen: u8,
}

impl MtcAssembler {
    pub fn new() -> MtcAssembler {
        MtcAssembler::default()
    }

    fn assemble(&self) -> Option<Timecode> {
        let p = &self.pieces;
        let fps = Fps::from_artnet_type((p[7] >> 1) & 0x03)?;
        Some(Timecode {
            frames: p[0] | ((p[1] & 0x01) << 4),
            seconds: p[2] | ((p[3] & 0x03) << 4),
            minutes: p[4] | ((p[5] & 0x03) << 4),
            hours: p[6] | ((p[7] & 0x01) << 4),
            fps,
        })
    }

    /// One MIDI message. Returns the timecode and how many frames it lags
    /// the moment this message arrived (2 for quarter frames, 0 for a full
    /// frame).
    pub fn feed(&mut self, msg: &[u8]) -> Option<(Timecode, u8)> {
        match msg {
            [0xF1, data, ..] => {
                let piece = ((data >> 4) & 0x07) as usize;
                self.pieces[piece] = data & 0x0f;
                if piece == 0 {
                    self.seen = 1;
                } else if self.seen & (1 << (piece - 1)) != 0 {
                    self.seen |= 1 << piece;
                } else {
                    self.seen = 0;
                }
                if piece == 7 && self.seen == 0xff {
                    self.seen = 0;
                    return self.assemble().map(|tc| (tc, 2));
                }
                None
            }
            [0xF0, 0x7F, _, 0x01, 0x01, hr, mn, sc, fr, ..] => {
                let fps = Fps::from_artnet_type((hr >> 5) & 0x03)?;
                Some((Timecode { hours: hr & 0x1f, minutes: *mn, seconds: *sc, frames: *fr, fps }, 0))
            }
            _ => None,
        }
    }
}

/// The eight quarter-frame messages for `tc` (what a generator sends).
pub fn mtc_quarter_frames(tc: &Timecode) -> [[u8; 2]; 8] {
    let vals = [
        tc.frames & 0x0f,
        tc.frames >> 4,
        tc.seconds & 0x0f,
        tc.seconds >> 4,
        tc.minutes & 0x0f,
        tc.minutes >> 4,
        tc.hours & 0x0f,
        (tc.hours >> 4) | (tc.fps.artnet_type() << 1),
    ];
    let mut out = [[0u8; 2]; 8];
    for (i, v) in vals.iter().enumerate() {
        out[i] = [0xF1, ((i as u8) << 4) | (v & 0x0f)];
    }
    out
}

/// MTC generation (`clock.timecode` in generate mode): the quarter-frame
/// messages due at a playhead. Eight pieces make one address, two frames
/// long: block `b` carries frame `2b`, which the receiver reads two frames
/// late ([`MtcAssembler`]'s lag), so the sender needs no lead of its own.
pub struct MtcGen {
    fps: Fps,
    /// Quarter-frame index of the next piece, `None` before the first call.
    next: Option<u64>,
}

impl MtcGen {
    pub fn new(fps: Fps) -> MtcGen {
        MtcGen { fps, next: None }
    }

    /// Every piece whose time is at or before `playhead` (seconds), in order,
    /// continuing from the last call. A seek restarts at a block boundary.
    pub fn due(&mut self, playhead: f64) -> Vec<[u8; 2]> {
        let now = (playhead.max(0.0) * 4.0 * self.fps.rate()).floor() as u64;
        let restart = match self.next {
            None => true,
            Some(q) => q > now + 8 || q + 8 < now,
        };
        let mut q = match (restart, self.next) {
            (false, Some(q)) => q,
            _ => now / 8 * 8,
        };
        let mut out = Vec::new();
        while q <= now {
            let block = q / 8;
            let tc = Timecode::from_seconds(((2 * block) as f64 + 0.5) / self.fps.rate(), self.fps);
            out.push(mtc_quarter_frames(&tc)[(q % 8) as usize]);
            q += 1;
        }
        self.next = Some(q);
        out
    }
}

// ---------------------------------------------------------------------------
// chase
// ---------------------------------------------------------------------------

/// How the playhead follows the incoming timecode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChaseMode {
    /// Stops within a few frames of the timecode stopping.
    Follow,
    /// Keeps running through a dropout for `freewheel_frames`.
    Freewheel,
}

/// Frames `follow` bridges between two arrivals (MTC lands every two
/// frames; LTC every frame).
pub const FOLLOW_FRAMES: u32 = 4;

/// The last timecode heard and when; [`Chase::position`] runs it forward.
#[derive(Debug, Clone)]
pub struct Chase {
    pub mode: ChaseMode,
    pub freewheel_frames: u32,
    last: Option<(f64, Fps, Instant)>,
    pub last_tc: Option<Timecode>,
}

impl Chase {
    pub fn new(mode: ChaseMode, freewheel_frames: u32) -> Chase {
        Chase { mode, freewheel_frames, last: None, last_tc: None }
    }

    /// A timecode heard at `now`, already corrected to "this instant"
    /// (`late_frames` added).
    pub fn hear(&mut self, tc: Timecode, late_frames: f64, now: Instant) {
        let pos = tc.to_seconds() + late_frames / tc.fps.rate();
        self.last = Some((pos, tc.fps, now));
        self.last_tc = Some(tc);
    }

    fn window(&self, fps: Fps) -> Duration {
        let frames = match self.mode {
            ChaseMode::Follow => FOLLOW_FRAMES,
            ChaseMode::Freewheel => self.freewheel_frames.max(FOLLOW_FRAMES),
        };
        Duration::from_secs_f64(f64::from(frames) / fps.rate())
    }

    /// Where the playhead is at `now`, or `None` when the timecode stopped
    /// longer ago than the mode bridges.
    pub fn position(&self, now: Instant) -> Option<f64> {
        let (pos, fps, at) = self.last?;
        let elapsed = now.saturating_duration_since(at);
        (elapsed <= self.window(fps)).then(|| pos + elapsed.as_secs_f64())
    }
}

/// `clock.timecode`'s config. `{"source": "ltc"|"mtc"|"artnet", "input":
/// "<audio pick>", "channel": 1, "midi_port": "<substring>", "fps":
/// "25"|null, "chase": "follow"|"freewheel", "freewheel_frames": 10,
/// "offset": 0.0}`. `offset` seconds are subtracted: a show whose timecode
/// starts at 01:00:00:00 sets 3600.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimecodeConfig {
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default = "default_channel")]
    pub channel: u16,
    #[serde(default)]
    pub midi_port: Option<String>,
    #[serde(default)]
    pub fps: Option<Fps>,
    #[serde(default = "default_chase")]
    pub chase: ChaseMode,
    #[serde(default = "default_freewheel")]
    pub freewheel_frames: u32,
    #[serde(default)]
    pub offset: f64,
    /// `read` (the default) listens; `generate` sends `source`'s wire from the
    /// lanes playhead (LTC to `output`, MTC to `midi_out`).
    #[serde(default)]
    pub mode: TimecodeMode,
    /// Generate, LTC: the audio output (a Pulse/PipeWire sink; empty is the
    /// default sink).
    #[serde(default)]
    pub output: Option<String>,
    /// Generate, MTC: the MIDI output port (a substring of its name).
    #[serde(default)]
    pub midi_out: Option<String>,
}

/// `clock.timecode`'s direction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimecodeMode {
    #[default]
    Read,
    Generate,
}

fn default_source() -> String {
    "ltc".into()
}
fn default_channel() -> u16 {
    1
}
fn default_chase() -> ChaseMode {
    ChaseMode::Follow
}
fn default_freewheel() -> u32 {
    10
}

impl Default for TimecodeConfig {
    fn default() -> TimecodeConfig {
        serde_json::from_str("{}").expect("all fields default")
    }
}

impl TimecodeConfig {
    pub fn from_node(node: &metal_graph::Node) -> TimecodeConfig {
        if node.config.is_empty() {
            return TimecodeConfig::default();
        }
        serde_json::from_str(&node.config.to_json_str()).unwrap_or_default()
    }

    /// What a generator is opened from: the wire's output. `None` for a wire
    /// that cannot be generated here (Art-Net's ArtTimeCode is not yet).
    pub fn writer_key(&self) -> Option<String> {
        match self.source.as_str() {
            "mtc" => Some(format!("mtc-out {}", self.midi_out.as_deref().unwrap_or(""))),
            "ltc" => Some(format!("ltc-out {}", self.output.as_deref().unwrap_or(""))),
            _ => None,
        }
    }

    /// What a reader is opened from: a change here reopens it, a new chase
    /// mode or offset does not.
    pub fn reader_key(&self) -> String {
        match self.source.as_str() {
            "mtc" => format!("mtc {}", self.midi_port.as_deref().unwrap_or("")),
            "artnet" => "artnet".into(),
            _ => format!("ltc {} {}", self.input.as_deref().unwrap_or(""), self.channel.max(1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(h: u8, m: u8, s: u8, f: u8, fps: Fps) -> Timecode {
        Timecode { hours: h, minutes: m, seconds: s, frames: f, fps }
    }

    #[test]
    fn seconds_round_trip_at_every_rate() {
        for fps in Fps::ALL {
            for secs in [0.0, 1.5, 59.99, 61.0, 600.2, 3601.7] {
                let t = Timecode::from_seconds(secs, fps);
                let back = t.to_seconds();
                assert!((back - secs).abs() <= 1.0 / fps.rate() + 1e-9, "{fps:?} {secs} → {} → {back}", t.text());
                assert_eq!(Timecode::from_seconds(back, fps), t);
            }
        }
    }

    #[test]
    fn drop_frame_skips_frames_zero_and_one_except_every_tenth_minute() {
        let fps = Fps::F2997Df;
        assert_eq!(tc(0, 1, 0, 2, fps).frame_count(), 1800);
        assert_eq!(Timecode::from_seconds(1800.0 / fps.rate(), fps), tc(0, 1, 0, 2, fps));
        // Minute 10 keeps its frames 0 and 1.
        assert_eq!(Timecode::from_seconds(17982.0 / fps.rate(), fps), tc(0, 10, 0, 0, fps));
        // One hour of 29.97df is 107892 frames.
        assert_eq!(tc(1, 0, 0, 0, fps).frame_count(), 107_892);
    }

    #[test]
    fn ltc_bits_carry_the_sync_word_and_even_parity() {
        let b = ltc_bits(&tc(12, 34, 56, 7, Fps::F25));
        let sync: u16 = (0..16).map(|i| u16::from(b[64 + i]) << i).sum();
        assert_eq!(sync, LTC_SYNC);
        assert_eq!(b.iter().filter(|x| !**x).count() % 2, 0);
    }

    /// Synthesized LTC at every rate decodes back to consecutive frames with
    /// the right rate.
    #[test]
    fn ltc_decodes_synthesized_audio_at_every_rate() {
        for fps in Fps::ALL {
            let start = tc(1, 2, 3, 4, fps);
            let audio = ltc_encode(&start, 12, 48_000, 0.5);
            let mut d = LtcDecoder::new(48_000);
            // Odd chunk sizes: the decoder is a stream.
            let mut got = Vec::new();
            for chunk in audio.chunks(333) {
                got.extend(d.push(chunk));
            }
            assert!(got.len() >= 10, "{fps:?}: decoded {} of 12", got.len());
            let (last, _) = *got.last().unwrap();
            assert_eq!(last.fps, fps, "{fps:?} detected");
            // The last frame's closing half-bit needs the next frame's edge,
            // so a stream that ends there yields frames start..=start+10.
            assert_eq!(last.frame_count(), start.frame_count() + 10, "{fps:?}: {}", last.text());
            for w in got.windows(2) {
                assert_eq!(w[1].0.frame_count(), w[0].0.frame_count() + 1);
            }
        }
    }

    #[test]
    fn ltc_survives_inverted_polarity_and_a_quiet_level() {
        let start = tc(0, 0, 10, 0, Fps::F30);
        let audio: Vec<f32> = ltc_encode(&start, 8, 44_100, 0.05).into_iter().map(|s| -s).collect();
        let mut d = LtcDecoder::new(44_100);
        let got = d.push(&audio);
        assert!(got.len() >= 6);
        assert_eq!(got.last().unwrap().0, tc(0, 0, 10, 6, Fps::F30), "frames up to the one before the end");
    }

    #[test]
    fn mtc_quarter_frames_and_full_frame_assemble() {
        let t = tc(10, 20, 30, 12, Fps::F25);
        let mut a = MtcAssembler::new();
        let mut got = None;
        for m in mtc_quarter_frames(&t) {
            if let Some(x) = a.feed(&m) {
                got = Some(x);
            }
        }
        assert_eq!(got, Some((t, 2)));
        // A piece out of order does not complete a time.
        let mut a = MtcAssembler::new();
        let q = mtc_quarter_frames(&t);
        for m in q.iter().skip(1) {
            assert_eq!(a.feed(m), None);
        }
        let full = [0xF0, 0x7F, 0x7F, 0x01, 0x01, (3 << 5) | 5, 6, 7, 8, 0xF7];
        assert_eq!(a.feed(&full), Some((tc(5, 6, 7, 8, Fps::F30), 0)));
    }

    #[test]
    fn chase_follow_stops_and_freewheel_runs_on() {
        let t0 = Instant::now();
        let mut c = Chase::new(ChaseMode::Follow, 10);
        assert_eq!(c.position(t0), None);
        c.hear(tc(0, 0, 10, 0, Fps::F25), 0.0, t0);
        let at = |ms| t0 + Duration::from_millis(ms);
        assert!((c.position(at(100)).unwrap() - 10.1).abs() < 1e-6, "runs between arrivals");
        assert_eq!(c.position(at(200)), None, "follow: >4 frames of silence stops");
        let mut c = Chase::new(ChaseMode::Freewheel, 25);
        c.hear(tc(0, 0, 10, 0, Fps::F25), 2.0, t0);
        assert!((c.position(at(900)).unwrap() - (10.08 + 0.9)).abs() < 1e-6, "freewheels 25 frames, lag added");
        assert_eq!(c.position(at(1100)), None);
    }

    #[test]
    fn config_defaults_and_reader_key() {
        let n = metal_graph::Node::new("t", TIMECODE_KIND, [0.0, 0.0]);
        let c = TimecodeConfig::from_node(&n);
        assert_eq!(c.source, "ltc");
        assert_eq!(c.channel, 1);
        assert_eq!(c.chase, ChaseMode::Follow);
        assert_eq!(c.reader_key(), "ltc  1");
        let c: TimecodeConfig = serde_json::from_str(r#"{"source":"mtc","fps":"29.97df","chase":"freewheel"}"#).unwrap();
        assert_eq!(c.fps, Some(Fps::F2997Df));
        assert_eq!(c.chase, ChaseMode::Freewheel);
        assert_eq!(c.reader_key(), "mtc ");
    }

    /// LTC generated in playhead-sized steps, as the output thread feeds it,
    /// decodes back to consecutive frames starting at the playhead's frame.
    #[test]
    fn ltc_generated_from_a_playhead_decodes_back() {
        for fps in Fps::ALL {
            let rate = 48_000;
            let mut g = LtcGen::new(fps, rate, 0.5);
            let mut audio = Vec::new();
            let step = 1.0 / 50.0;
            for i in 0..60 {
                g.due(1.0 + f64::from(i) * step, &mut audio);
            }
            let mut d = LtcDecoder::new(rate);
            let got: Vec<Timecode> = audio.chunks(777).flat_map(|c| d.push(c)).map(|(t, _)| t).collect();
            assert!(got.len() >= 10, "{fps:?}: decoded {}", got.len());
            let first = Timecode::from_seconds(1.0, fps);
            assert_eq!(got[0], first, "{fps:?}: the chain starts at the playhead's frame");
            for w in got.windows(2) {
                assert_eq!(w[1].frame_count(), w[0].frame_count() + 1, "{fps:?}");
            }
        }
    }

    /// A playhead that jumps back restarts the chain there: the next frame
    /// decodes at the new position, not after the old one.
    #[test]
    fn ltc_generator_restarts_on_a_seek() {
        let mut g = LtcGen::new(Fps::F25, 48_000, 0.5);
        let mut audio = Vec::new();
        g.due(100.0, &mut audio);
        let before = audio.len();
        g.due(2.0, &mut audio);
        let mut d = LtcDecoder::new(48_000);
        let got: Vec<Timecode> = audio[before..].chunks(512).flat_map(|c| d.push(c)).map(|(t, _)| t).collect();
        assert!(!got.is_empty());
        assert_eq!(got[0], Timecode::from_seconds(2.0, Fps::F25), "the seek lands on the playhead");
    }

    /// MTC generated from a playhead, fed to the assembler, reads back the
    /// even frames in order with the receiver's two-frame lag.
    #[test]
    fn mtc_generated_from_a_playhead_assembles_back() {
        for fps in Fps::ALL {
            let mut g = MtcGen::new(fps);
            let mut a = MtcAssembler::new();
            let mut got = Vec::new();
            for i in 0..=200 {
                for m in g.due(f64::from(i) * 0.01) {
                    if let Some((tc, late)) = a.feed(&m) {
                        assert_eq!(late, 2);
                        got.push(tc);
                    }
                }
            }
            assert!(got.len() >= 8, "{fps:?}: assembled {}", got.len());
            for w in got.windows(2) {
                assert_eq!(w[1].frame_count(), w[0].frame_count() + 2, "{fps:?}: block b carries frame 2b");
            }
            assert_eq!(got[0].frame_count(), 0, "{fps:?}: the first block is 00:00:00:00");
        }
    }

    /// `mode` and the outputs read from the node's own keys, with `read` the
    /// default and a generator's key naming its output.
    #[test]
    fn timecode_mode_and_writer_key_read_from_config() {
        let c = TimecodeConfig::default();
        assert_eq!(c.mode, TimecodeMode::Read);
        assert_eq!(c.writer_key(), Some("ltc-out ".into()));
        let c: TimecodeConfig =
            serde_json::from_str(r#"{"source":"mtc","mode":"generate","midi_out":"Loop"}"#).unwrap();
        assert_eq!(c.mode, TimecodeMode::Generate);
        assert_eq!(c.writer_key(), Some("mtc-out Loop".into()));
        let c: TimecodeConfig = serde_json::from_str(r#"{"source":"artnet","mode":"generate"}"#).unwrap();
        assert_eq!(c.writer_key(), None, "ArtTimeCode is not generated yet");
    }
}
