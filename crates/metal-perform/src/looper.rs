//! The Looper — the one place a digest repeat is *played*, not detected.
//!
//! Phase 1/2 gave the seam somewhere to say "the music is repeating" and
//! `metal-listen` somewhere to notice it ([`crate::digest::Source::repeat`]).
//! This module is the last step: a consumer that takes the live [`Frame`],
//! remembers the last [`RING_BEATS`] of it, and hands back a frame that is the
//! live row when nothing is engaged and a *replay of the recorded row* when
//! something is.
//!
//! # Why the replay is indexed by beat phase and not by tick
//!
//! A DJ loop is a statement about *beats*, not about wall time or frames drawn.
//! If a replay advanced one recorded row per `poll` (one per drawn frame), then
//! any tempo drift — the stand-in's own clock, a real tracker's estimate — would
//! slide the loop off the grid: the loop would run at the rate the *renderer*
//! draws, not the rate the music plays. Indexing by beat phase (`since + (now −
//! since) mod len`) means the replayed row is a function of where the music is,
//! so a tempo change moves the loop with the music (plan decision 5). This is
//! also why `poll` is safe to call several times at one `time`: the same beat
//! yields the same replayed row, which is what lets the router and every
//! renderer share one Looper and still see one frame.
//!
//! # Held in the air, never saved
//!
//! [`Mode`] and the engaged loop are *performance* state, not configuration:
//! they are the operator's hand on the machine this set, thrown away when the
//! process is. Persisting them would mean a laptop that came back from the
//! previous night already looping the previous night's phrase. So there is no
//! serde here, no file, no field on any saved struct (plan decision 6). Phase
//! 3B puts one `Looper` on `State`; when "Audio input is a node" lands it
//! becomes per `audio.live` (plan step 7).
//!
//! # Vocabulary
//!
//! `since` is the absolute beat the loop anchored at — the beat *phase* origin,
//! not a tick. [`crate::clock::MusicClock`] reads the same beat count into the
//! musical beat 16-step loops run on (it replaced the never-compiled
//! `beatloop.rs`); this module keeps its own anchor so it does not depend on it.

use std::collections::VecDeque;

use crate::digest::{Frame, RepeatState, FEATURES};
use crate::live::Input;

/// How many beats of the digest are held for replay. 32 = eight bars.
pub const RING_BEATS: usize = 32;

/// The shortest loop the Looper will engage, and the length
/// `metal_listen::repeat::LENGTHS` starts at and [`crate::digest::Simulated`]
/// clamps to. Restated rather than imported for the same reason `digest.rs`
/// restates it: this crate has no `metal-listen` in it. A one-beat loop is the
/// degenerate minimum and a detector that never produces one must not be able
/// to make the Looper engage on one either.
pub const MIN_LOOP_BEATS: u32 = 4;

/// A hard ceiling on recorded pairs, well above [`RING_BEATS`]. The beat span
/// is the real eviction rule, but a stalled beat clock (`beats()` returning the
/// same value every poll — a `Live` source between turn-overs) would otherwise
/// append the same beat forever. This is the backstop for that.
const RING_MAX_ROWS: usize = RING_BEATS * 64;

/// The mode a Looper is in. Held in the air (plan decision 6); never saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// Detector off; only a manual `lock`/`loop_beats` engages.
    Off,
    /// The detector lights up but the operator must lock (`auto` is the
    /// detector's own lock). **Default.**
    #[default]
    Arm,
    /// Lock on detection, no operator needed.
    Auto,
}

impl Mode {
    /// The wire/readout spelling. Matches the page's mode control and the
    /// eventual `/loop` route body, so one string means one mode everywhere.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Arm => "arm",
            Mode::Auto => "auto",
        }
    }

    /// Parse the spelling [`Mode::as_str`] produces. Case-insensitive on the
    /// way in because a query string or a hand-typed body should not have to
    /// know our letter case; unusable input is `None`, never a silent default.
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Mode::Off),
            "arm" => Some(Mode::Arm),
            "auto" => Some(Mode::Auto),
            _ => None,
        }
    }
}

/// A loop currently replaying.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Engaged {
    /// Absolute beat the loop started at (the beat *phase* anchor).
    pub since: f64,
    /// Loop length in beats.
    pub len: u32,
}

/// A loop leaving, crossfading back to live over 1 beat (plan decision 8).
///
/// Kept apart from [`Engaged`] because the loop is still *replaying* while it
/// fades out — the frame is still the recorded one, blended more and more with
/// live — and only when the fade completes does `engaged` become `None`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Exiting {
    /// Absolute beat the fade-out *starts* on (the next beat after `exit`).
    start: f64,
}

/// What the readout and `/loop/audio` report.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    pub mode: Mode,
    pub engaged: Option<Engaged>,
    /// 0..1 through the loop (0 at the anchor beat, →1 at the next anchor).
    pub phase: f32,
    /// Length of the engaged loop, 0 when not engaged (the readout's `L=`).
    pub length_beats: u32,
    pub detector: Option<crate::digest::RepeatState>,
}

pub struct Looper {
    /// The recorded digest, oldest first. Beats are fractional and monotone.
    ring: VecDeque<(f64, Frame)>,
    /// The loop on air, if any. `None` is the pass-through state.
    engaged: Option<Engaged>,
    /// A loop mid-exit; `engaged` stays `Some` (and keeps replaying) until the
    /// fade reaches 1.
    exiting: Option<Exiting>,
    mode: Mode,
    /// The last detector state fed in, surfaced in [`Status::detector`] even
    /// when `Arm` does not act on it.
    detector: Option<RepeatState>,
    /// Whether the current engagement was the operator's `lock` (true) or the
    /// detector's (false). A manual loop is not released by `observe(None)`.
    manual: bool,
    /// Crossfade progress 0..1: 0 fully replay/live-source, 1 fully the other
    /// side. Which side the *engine* is on is decided by `fading_in`.
    fade: f32,
    /// True while crossfading *into* the loop (live → replay); false while
    /// crossfading *out* (replay → live) or not fading.
    fading_in: bool,
    /// The last live frame seen, so a fade has something to blend against.
    live: Frame,
}

impl Looper {
    pub fn new() -> Looper {
        Looper {
            ring: VecDeque::new(),
            engaged: None,
            exiting: None,
            mode: Mode::default(),
            detector: None,
            manual: false,
            fade: 1.0,
            fading_in: false,
            live: Frame::step([0.0; FEATURES]),
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn set_mode(&mut self, m: Mode) {
        self.mode = m;
    }

    pub fn status(&self) -> Status {
        let now = self.last_beats();
        let phase = match self.engaged {
            Some(e) if e.len > 0 => {
                let p = (now - e.since).rem_euclid(e.len as f64);
                (p / e.len as f64) as f32
            }
            _ => 0.0,
        };
        Status {
            mode: self.mode,
            engaged: self.engaged,
            phase,
            length_beats: self.engaged.map(|e| e.len).unwrap_or(0),
            detector: self.detector,
        }
    }

    /// A detector lock arrived. In `Arm` this does nothing but surface it in
    /// `status().detector`; in `Auto` it engages. In `Off` nothing.
    pub fn observe(&mut self, r: Option<RepeatState>) {
        self.detector = r;
        match r {
            Some(r) if self.mode == Mode::Auto && !self.manual => {
                self.engage(r.length_beats, false);
            }
            // Detector release: only a *detected* loop lets go on `None`. A
            // manual loop is the operator's; the detector has no standing to
            // release it (brief §5). `Arm`/`Off` surface the state but do not
            // act.
            None if self.engaged.is_some() && !self.manual => self.begin_exit(),
            _ => {}
        }
    }

    /// The operator's manual loop: "loop the last N beats". `None` = exit.
    /// Works with no detector and no tempo lock (plan decision 7).
    pub fn lock(&mut self, beats: Option<u32>) {
        match beats {
            Some(l) => self.engage(l.max(MIN_LOOP_BEATS), true),
            None => self.exit(),
        }
    }

    /// Leave the loop, crossfading back to live over 1 beat (plan decision 8).
    pub fn exit(&mut self) {
        self.begin_exit();
    }

    /// Feed the live digest and get the frame every consumer should see.
    /// Replays the recorded frames, indexed by **beat phase**, when engaged;
    /// otherwise passes the live frame through. Also appends to the ring.
    pub fn poll(&mut self, input: &mut Input, time: f32) -> Frame {
        // The live row is read first, every call: it is what we append to the
        // ring and what a fade blends against.
        let live = input.poll(time);
        let beats = input.beats(time).map(|b| b as f64);

        if let Some(b) = beats {
            self.append(b, live);
        }

        self.live = live;

        let Some(curr) = self.engaged else {
            // Pass-through. Not engaged and not fading: the live frame is the
            // frame, whatever the beat clock is doing.
            return live;
        };

        // No beat clock → nothing to index a replay by. The honest answer is
        // the live frame (plan: "no tempo lock → no detector"), and the fade is
        // left frozen rather than advanced against a clock that is not there.
        let Some(now) = beats else {
            return live;
        };

        // The recorded row at this beat phase.
        let replay = self.replay(curr, now);

        // Exit fade: it begins on the beat *after* `exit` was called, so the
        // blend is scheduled, not immediate (plan decision 8). `self.fade`
        // counts 0..1 through the one beat.
        if let Some(x) = self.exiting {
            // Clamped below at 0 while the next beat has not arrived, so the
            // loop keeps playing at full level until its quantized start.
            self.fade = ((now - x.start) as f32).clamp(0.0, 1.0);
            if self.fade >= 1.0 {
                // Fade complete: fully live, and the loop is gone.
                self.engaged = None;
                self.exiting = None;
                self.fading_in = false;
                self.fade = 1.0;
                return live;
            }
            // out = replay*(1-fade) + live*fade
            return blend(replay, live, self.fade);
        }

        // Enter fade: one beat from live into the replay. Starts immediately at
        // the anchor beat, so the row is continuous at `since`.
        if self.fading_in {
            self.fade = ((now - curr.since) as f32).clamp(0.0, 1.0);
            if self.fade >= 1.0 {
                self.fading_in = false;
                self.fade = 1.0;
                return replay;
            }
            // out = live*(1-fade) + replay*fade
            return blend(replay, live, self.fade);
        }

        replay
    }

    /// Reset the ring and leave any loop when the source changes.
    pub fn reset(&mut self) {
        self.ring.clear();
        self.engaged = None;
        self.exiting = None;
        self.detector = None;
        self.manual = false;
        self.fade = 1.0;
        self.fading_in = false;
        self.live = Frame::step([0.0; FEATURES]);
    }

    // -- internals ---------------------------------------------------------

    /// The beat the last poll saw, 0.0 before any beat has arrived. Only used
    /// by [`Looper::status`], which runs off the render clock, not the audio
    /// one; `poll` keeps its own local `now` for the arithmetic that matters.
    fn last_beats(&self) -> f64 {
        self.ring.back().map(|(b, _)| *b).unwrap_or(0.0)
    }

    /// Engage (or re-anchor) a loop of `len` beats at the last seen beat.
    fn engage(&mut self, len: u32, manual: bool) {
        let len = len.max(MIN_LOOP_BEATS);
        let since = self.last_beats();
        self.engaged = Some(Engaged { since, len });
        self.exiting = None;
        self.manual = manual;
        // Crossfade-in over 1 beat from the anchor.
        self.fading_in = true;
        self.fade = 0.0;
    }

    /// Begin the exit crossfade. If nothing is engaged this is a no-op; if an
    /// exit is already running, the start does not move. The start is the next
    /// *beat* after now, so the fade is quantized (plan decision 8).
    fn begin_exit(&mut self) {
        if self.engaged.is_none() {
            return;
        }
        if self.exiting.is_some() {
            return;
        }
        let now = self.last_beats();
        // Next beat from now, half-open: `ceil`, but exactly on a beat stays
        // there rather than jumping a whole beat.
        let start = (now - 1e-9).ceil().max(now);
        self.exiting = Some(Exiting { start });
        self.fading_in = false;
    }

    /// Append `(beat, frame)` to the ring, evicting by beat span.
    fn append(&mut self, beat: f64, frame: Frame) {
        // A beat that did not advance is a stalled poll (the same `time` asked
        // twice, or a source between rows). It must not append a second pair at
        // the same phase — the replay is indexed by beat, so a duplicate would
        // only waste memory and make neighbour lookup ambiguous.
        if let Some((last, _)) = self.ring.back() {
            if beat <= *last {
                return;
            }
        }
        self.ring.push_back((beat, frame));
        // Keep the newest pair whose beat is within `RING_BEATS` of the newest,
        // plus the one before it (the neighbour the oldest phase interpolates
        // from). `>` not `>=`: a pair exactly `RING_BEATS` back is the wrap
        // neighbour of the newest phase and is still useful.
        if let Some((newest, _)) = self.ring.back() {
            let newest = *newest;
            while self.ring.len() > 2 {
                let front = self.ring[0].0;
                if newest - front > RING_BEATS as f64 {
                    self.ring.pop_front();
                } else {
                    break;
                }
            }
        }
        while self.ring.len() > RING_MAX_ROWS {
            self.ring.pop_front();
        }
    }

    /// The recorded frame at `phase` beats, interpolating `f` between the two
    /// bracketing neighbours and picking `raw` from the nearer one.
    ///
    /// If nothing brackets the phase — a loop just engaged before the ring had
    /// a cycle — the nearest recorded row is returned whole. That is the honest
    /// partial loop: replay what we have rather than invent a row.
    fn replay(&self, e: Engaged, now: f64) -> Frame {
        let phase = e.since + (now - e.since).rem_euclid(e.len as f64);
        self.frame_at(phase)
    }

    /// The interpolated recorded frame at absolute beat `phase`.
    fn frame_at(&self, phase: f64) -> Frame {
        if self.ring.is_empty() {
            return self.live;
        }
        // Find the first pair at or after `phase`. Linear scan is fine: the
        // ring is at most `RING_MAX_ROWS` and the common case is a few entries
        // ahead of the previous poll; this is not a hot inner loop.
        let mut hi = None;
        for (i, (b, _)) in self.ring.iter().enumerate() {
            if *b >= phase {
                hi = Some(i);
                break;
            }
        }
        match hi {
            Some(0) => self.ring[0].1,
            Some(i) => {
                let (b1, f1) = &self.ring[i];
                let (b0, f0) = &self.ring[i - 1];
                let span = *b1 - *b0;
                let t = if span > 0.0 {
                    ((phase - *b0) / span).clamp(0.0, 1.0) as f32
                } else {
                    1.0
                };
                let mut f = [0.0f32; FEATURES];
                for k in 0..FEATURES {
                    f[k] = f0.f[k] + (f1.f[k] - f0.f[k]) * t;
                }
                // `raw` comes from the nearer neighbour: it is the staircase,
                // and blending two steps would make a transient appear where
                // neither row had one. Nearest by `t`, ties → the later row.
                let raw = if t < 0.5 { f0.raw } else { f1.raw };
                Frame { f, raw, rate: f0.rate, voice: f0.voice }
            }
            None => self.ring.back().map(|(_, fr)| *fr).unwrap_or(self.live),
        }
    }
}

/// `replay*(1-p) + live*p` per column, for both signals: at `p == 0` the frame
/// is `replay`, at `p == 1` it is `live`. One curve for both directions, so
/// entering and leaving a loop are the same motion reversed; linear because a
/// one-beat crossfade is short enough that a curve would only be audible as a
/// coincidence with the music it is fading against.
fn blend(replay: Frame, live: Frame, p: f32) -> Frame {
    let p = p.clamp(0.0, 1.0);
    let inv = 1.0 - p;
    let mut f = [0.0f32; FEATURES];
    let mut raw = [0.0f32; FEATURES];
    for k in 0..FEATURES {
        f[k] = replay.f[k] * inv + live.f[k] * p;
        raw[k] = replay.raw[k] * inv + live.raw[k] * p;
    }
    Frame { f, raw, rate: replay.rate, voice: replay.voice }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test clock: `Simulated` at a fixed 120 BPM, so two beats a second
    /// and beat `b` is at `b/2` seconds. It is a real [`Input`], which is the
    /// point — the Looper's `poll` takes one, and no test depends on wall time.
    const BPM: f32 = 120.0;

    fn input() -> Input {
        let mut input = Input::stand_in();
        input.set_bpm(BPM);
        input
    }

    /// Seconds at which beat `b` falls on [`input`]'s clock.
    fn at(beat: f64) -> f32 {
        (beat as f32) * 60.0 / BPM
    }

    /// Poll the Looper at `time` and return the frame.
    fn poll(l: &mut Looper, input: &mut Input, time: f32) -> Frame {
        l.poll(input, time)
    }

    /// Lay down `beats` beats of history at a half-beat per poll.
    fn history(l: &mut Looper, input: &mut Input, beats: u32) {
        let mut t = 0.0f32;
        for _ in 0..beats * 2 {
            poll(l, input, t);
            t += 0.25;
        }
    }

    #[test]
    fn new_is_arm_and_empty() {
        let l = Looper::new();
        assert_eq!(l.mode(), Mode::Arm, "arm is the default (decision 6)");
        let s = l.status();
        assert_eq!(s.mode, Mode::Arm);
        assert_eq!(s.engaged, None);
        assert_eq!(s.length_beats, 0);
        assert_eq!(s.detector, None);
        assert_eq!(s.phase, 0.0);
    }

    #[test]
    fn modes_round_trip_through_their_spelling() {
        for m in [Mode::Off, Mode::Arm, Mode::Auto] {
            assert_eq!(Mode::parse(m.as_str()), Some(m));
        }
        assert_eq!(Mode::parse("AUTO"), Some(Mode::Auto), "case is not the parser's business");
        assert_eq!(Mode::parse(" live "), None, "an unknown mode is not a default");
    }

    #[test]
    fn replay_returns_the_recorded_frame_at_the_same_beat_phase() {
        // Engage L=4 after history; the row at beat s+1 must be the row
        // recorded at beat s+5 (one anchor on), because both are the same
        // phase of the same loop (plan decision 5).
        let mut l = Looper::new();
        let mut input = input();
        history(&mut l, &mut input, 16);
        l.lock(Some(4));
        let since = l.status().engaged.expect("locked").since;
        let one = poll(&mut l, &mut input, at(since + 1.0));
        let five = poll(&mut l, &mut input, at(since + 5.0));
        assert_eq!(
            one.f, five.f,
            "the row one beat into the loop must equal the row one loop later"
        );
        assert_eq!(one.raw, five.raw, "and the staircase must repeat too");
    }

    #[test]
    fn the_exit_crossfades_over_one_beat() {
        let mut l = Looper::new();
        let mut input = input();
        history(&mut l, &mut input, 16);
        l.lock(Some(4));
        let since = l.status().engaged.unwrap().since;
        // Run past a full loop length so the replay actually reaches back into
        // recorded history (inside the first cycle the phase equals now, so
        // replay and live coincide and the fade has nothing to blend).
        poll(&mut l, &mut input, at(since + 5.5));

        l.exit();
        let start = l.exiting.expect("an exit is scheduled").start;
        // Before the next beat the fade has not started — output is still the
        // loop, not live. (A quarter beat back, so the replay's phase and the
        // live instant are different rows and the two are distinguishable.)
        let t_before = at(start - 0.25);
        let before = poll(&mut l, &mut input, t_before);
        assert_eq!(l.fade, 0.0, "the fade must not start before the beat");
        let live_before = input.poll(t_before);
        assert_ne!(
            before.f, live_before.f,
            "before the exit beat the frame must still be the loop"
        );
        // Half a beat into the fade the frame must differ from both ends.
        let t_mid = at(start + 0.5);
        let mid = poll(&mut l, &mut input, t_mid);
        assert!(
            l.fade > 0.0 && l.fade < 1.0,
            "mid-fade progress must be strictly between the ends: {}",
            l.fade
        );
        let live_mid = input.poll(t_mid);
        assert_ne!(mid.f, live_mid.f, "mid-fade the frame is not yet live");
        // A full beat in, the fade is done, the loop is gone, the frame is live.
        let t_done = at(start + 1.5);
        let done = poll(&mut l, &mut input, t_done);
        assert_eq!(l.status().engaged, None, "the loop must be gone at the end");
        let live = input.poll(t_done);
        assert_eq!(done.f, live.f, "the fade must land exactly on live");
    }

    #[test]
    fn arm_mode_never_engages_by_itself() {
        let mut l = Looper::new();
        assert_eq!(l.mode(), Mode::Arm);
        let lock = RepeatState {
            length_beats: 8,
            since_beat: 0.0,
            confidence: 1.0,
        };
        l.observe(Some(lock));
        assert_eq!(
            l.status().engaged,
            None,
            "arm means the detector lights up but the operator locks"
        );
        assert_eq!(
            l.status().detector,
            Some(lock),
            "the detector must still be surfaced in Arm"
        );
    }

    #[test]
    fn auto_mode_engages_on_a_detector_lock() {
        let mut l = Looper::new();
        let mut input = input();
        history(&mut l, &mut input, 8);
        l.set_mode(Mode::Auto);
        l.observe(Some(RepeatState {
            length_beats: 8,
            since_beat: 0.0,
            confidence: 0.9,
        }));
        assert_eq!(
            l.status().engaged.map(|e| e.len),
            Some(8),
            "auto is the detector's own lock"
        );
    }

    #[test]
    fn the_manual_loop_works_without_a_detector() {
        let mut l = Looper::new();
        let mut input = input();
        history(&mut l, &mut input, 8);
        l.lock(Some(8));
        let s = l.status();
        assert_eq!(s.engaged.map(|e| e.len), Some(8));
        assert_eq!(s.detector, None, "no detector was ever fed");
        assert_eq!(s.length_beats, 8);
    }

    #[test]
    fn a_one_beat_loop_is_refused_or_clamped() {
        // Documented choice: clamp to `MIN_LOOP_BEATS` (4), mirroring
        // `digest::MIN_LOOP_BEATS` and `metal_listen::repeat::LENGTHS`. A
        // one-beat loop is the degenerate minimum the detector never emits, and
        // engaging on one would make the Looper replay a single row forever.
        let mut l = Looper::new();
        let mut input = input();
        poll(&mut l, &mut input, 0.0);
        l.lock(Some(1));
        assert_eq!(l.status().engaged.map(|e| e.len), Some(MIN_LOOP_BEATS));
        // The detector path clamps too; switch to Auto so it acts.
        l.set_mode(Mode::Auto);
        l.observe(Some(RepeatState {
            length_beats: 1,
            since_beat: 0.0,
            confidence: 1.0,
        }));
        assert_eq!(l.status().engaged.map(|e| e.len), Some(MIN_LOOP_BEATS));
    }

    #[test]
    fn replay_is_indexed_by_beat_not_by_tick() {
        // Feeding the same beat twice (a stalled poll) must not advance the
        // replay phase. Both calls are at the same `time`, so both must return
        // the same row — the property that lets the router and the renderers
        // share one Looper (3B).
        let mut l = Looper::new();
        let mut input = input();
        history(&mut l, &mut input, 16);
        l.lock(Some(4));
        let since = l.status().engaged.unwrap().since;
        let t = at(since + 2.5);
        let a = poll(&mut l, &mut input, t);
        let b = poll(&mut l, &mut input, t);
        assert_eq!(a.f, b.f, "a stalled poll must not move the replay");
        assert_eq!(a.raw, b.raw);
    }

    #[test]
    fn a_manual_loop_is_not_released_by_the_detector() {
        // `observe(None)` while engaged begins the exit (plan decision 8), but
        // only for a *detected* loop. A manual loop is the operator's and the
        // detector has no standing to release it (brief §5).
        let mut l = Looper::new();
        let mut input = input();
        history(&mut l, &mut input, 8);
        l.lock(Some(8));
        l.observe(None);
        assert_eq!(
            l.status().engaged.map(|e| e.len),
            Some(8),
            "a manual loop survives the detector going quiet"
        );
    }

    #[test]
    fn reset_clears_the_ring_and_the_loop() {
        let mut l = Looper::new();
        let mut input = input();
        history(&mut l, &mut input, 8);
        l.lock(Some(4));
        assert!(l.status().engaged.is_some());
        l.reset();
        assert_eq!(l.status().engaged, None);
        assert_eq!(l.status().detector, None);
        assert!(l.ring.is_empty(), "the ring is source-specific history");
    }
}
