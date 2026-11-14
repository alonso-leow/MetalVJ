//! The musical clock loops run on: `(bar, step, phase)` from the beat
//! tracker's count, never from the 125 ms analysis tick.
//!
//! **Two clocks, on purpose** (`the loops design notes`, decision 2). The analysis
//! tick (the base rung, `metal_schema::BASE_HZ`) is what the corpus, the `.npy` files and the models are
//! measured in, and it stays exactly as it is. A loop is measured in music:
//! one loop is one 4/4 bar, sixteen [`STEPS`], and a clock tick is a sixteenth
//! note — `60 / BPM / 4` seconds, the same length as every other tick at the
//! current tempo. [`Position`] is the clock's tick grid; a loop's keyframe
//! *slots* ([`STEPS_PER_BEAT`], a half beat) are the pattern's own unit
//! (`pattern.rs`). Nothing here knows the analysis tick exists.
//!
//! **What goes in is plain numbers** — the tracker's monotonic beat count, the
//! index of a beat that is a "1", the tempo, and the time — so this module
//! depends on no particular tracker (`metal-listen`'s, the stand-in's, a test's
//! arithmetic all feed it the same way) and stays pure: the same inputs in the
//! same order give the same positions, which is what lets the tests below
//! prove the grid instead of eyeballing it.
//!
//! # Inside: one "musical beat" number
//!
//! Every input is folded into one `f64`, the musical beat `m`, laid out so the
//! "1"s sit exactly on multiples of four. Everything a caller sees —
//! [`Position::bar`], [`Position::step`], [`Position::phase`] — is read off
//! `m` by division, so a step boundary can only ever land on `m * 4` being a
//! whole number: on the beat grid, whatever the tempo did.
//!
//! - **Tracked** (a beat count is present): `m = count + shift`, where `shift`
//!   is `-downbeat` plus a whole number of bars. The tracker's count already
//!   follows tempo changes, phase-locked; adding a constant keeps every step on
//!   its grid.
//! - **Free** (no count — silence, a tracker that has not locked, a loop being
//!   previewed with no music): `m` runs from an anchor at the last known BPM,
//!   or [`FALLBACK_BPM`] when there never was one. A tempo change re-anchors
//!   where `m` is *now*, so the clock bends instead of jumping. The anchor is
//!   also why ten minutes of 60 fps updates do not drift: `m` is computed from
//!   `time - anchor`, never accumulated frame by frame.
//!
//! **Never backwards.** Free → tracked picks the whole number of bars in
//! `shift` that puts the tracked `m` at or after the last one shown, so a loop
//! being previewed carries on into the music at most one bar ahead, already on
//! the music's "1". A new downbeat index (the detector changed its mind, or
//! "tap 1") re-phases the same way. A count that steps back (it should not —
//! it is monotonic — but a reset tracker would) is held at the last position
//! until it catches up rather than replaying steps.
//!
//! Salvaged from the retired `beatloop.rs` (never compiled; its
//! on/off-per-node, 8-beat-count model did not fit — decision 10): the free
//! clock at the input's tempo with a constant fallback so a loop can be tried
//! in silence, and the launch-on-the-next-"1" rule with a grace window
//! ([`next_one`], [`LAUNCH_GRACE`]). Its accent/downbeat folding moved to
//! `metal-listen` (T2) and its rate quantization is left to the player (a ×2/÷2
//! rate must land on a step boundary of the *new* step length, counted from
//! the launch — see [`Position::from_beats`] for the arithmetic it will use).

/// Slots in one loop: the 16-cell keyframe grid (§5). A loop's [`STEPS`]
/// slots are the pattern's unit; the musical [`Position`] below counts
/// [`TICKS_PER_BEAT`] per beat, so one 4/4 bar is `STEPS` ticks.
pub const STEPS: u8 = 16;
/// Beats in one bar.
pub const BEATS_PER_BAR: u32 = 4;
/// Loop slots in one beat: a keyframe slot is a half beat (§5). The musical
/// clock's own subdivision is [`TICKS_PER_BEAT`], a sixteenth.
pub const STEPS_PER_BEAT: u32 = 2;
/// Clock ticks in one beat: the sixteenth-note subdivision [`Position`]
/// counts, four to a 4/4 beat.
pub const TICKS_PER_BEAT: u32 = 4;
/// The tempo of the free clock when nothing has ever said what the tempo is.
pub const FALLBACK_BPM: f32 = 120.0;
/// How late (in beats) a launch may be and still take the "1" it was meant
/// for, instead of waiting a whole bar for the next one.
pub const LAUNCH_GRACE: f64 = 0.25;

/// Where the music is, in loop terms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    /// Bars since the clock started counting (bar 0 holds the first "1").
    pub bar: u64,
    /// `0..16`: which sixteenth of the bar.
    pub step: u8,
    /// `0..1`: how far through that step — what a ramp interpolates on.
    pub phase: f32,
    /// `0..4`: which quarter of the bar (`step / 4`).
    pub beat_in_bar: u8,
}

impl Position {
    /// The position at musical beat `m` (the "1"s on multiples of four). A
    /// negative `m` — a count from before the first "1" — reads as bar 0.
    pub fn from_beats(m: f64) -> Position {
        let m = m.max(0.0);
        let steps = m * TICKS_PER_BEAT as f64;
        // Snap a hair under a whole step up to it: f32 counts converted to
        // f64 land 1e-6 short of the integer they mean, and "step 3 at phase
        // 0.99999" for "step 4" is exactly the off-by-one a grid must not have.
        let whole = (steps + 1e-6).floor();
        let total = whole as u64;
        let step = (total % STEPS as u64) as u8;
        Position {
            bar: total / STEPS as u64,
            step,
            phase: ((steps - whole).max(0.0) as f32).min(1.0 - f32::EPSILON),
            beat_in_bar: step / TICKS_PER_BEAT as u8,
        }
    }

    /// Back to the musical beat this position is at.
    pub fn beats(&self) -> f64 {
        (self.bar as f64 * STEPS as f64 + self.step as f64 + self.phase as f64)
            / TICKS_PER_BEAT as f64
    }
}

/// The musical beat a launch pressed at `m` should start on: the next "1",
/// or the one just passed when `m` is within [`LAUNCH_GRACE`] of it.
pub fn next_one(m: f64) -> f64 {
    let bar = BEATS_PER_BAR as f64;
    ((m - LAUNCH_GRACE) / bar).ceil().max(0.0) * bar
}

/// See the module note. Feed it every frame with [`MusicClock::update`].
#[derive(Debug, Clone)]
pub struct MusicClock {
    /// The last tempo anything said, for the free clock.
    bpm: f32,
    /// Tracked: `m = count + shift`.
    shift: f64,
    /// The downbeat index `shift` was built from.
    downbeat: Option<u32>,
    /// Whether the last update was tracked.
    tracked: bool,
    /// Free: `m = anchor_m + (time - anchor_t) * bpm / 60`.
    anchor_m: f64,
    anchor_t: f64,
    /// The tempo the free anchor runs at.
    anchor_bpm: f32,
    /// The last `m` shown — the floor for never going backwards.
    last: Option<f64>,
}

impl Default for MusicClock {
    fn default() -> Self {
        MusicClock::new()
    }
}

impl MusicClock {
    pub fn new() -> MusicClock {
        MusicClock {
            bpm: FALLBACK_BPM,
            shift: 0.0,
            downbeat: None,
            tracked: false,
            anchor_m: 0.0,
            anchor_t: f64::NAN,
            anchor_bpm: FALLBACK_BPM,
            last: None,
        }
    }

    /// The position at `time` (seconds, monotonic) given what the tracker
    /// says now: its beat `count` (`None` when it has none), which beat index
    /// is a "1" (`None`: not known — beat 0 of the count is used, or the last
    /// index known), and the tempo (`None` or non-positive: keep the last).
    pub fn update(
        &mut self,
        count: Option<f32>,
        downbeat: Option<u32>,
        bpm: Option<f32>,
        time: f64,
    ) -> Position {
        if let Some(b) = bpm.filter(|b| b.is_finite() && *b > 0.0) {
            self.bpm = b;
        }
        let m = match count.filter(|c| c.is_finite()) {
            Some(count) => self.tracked_m(count as f64, downbeat),
            None => self.free_m(time),
        };
        // Never backwards: a count that stepped back holds the last position.
        let m = match self.last {
            Some(last) if m < last => last,
            _ => m,
        };
        self.last = Some(m);
        Position::from_beats(m)
    }

    /// An explicit seek (the loops design notes T15): forget the last position so the
    /// next update may land *behind* it. The only way the clock goes
    /// backwards — every other path keeps never-backwards. The tempo is kept
    /// so a seek into a song with no beat map still runs at the last BPM.
    pub fn seek(&mut self) {
        let bpm = self.bpm;
        *self = MusicClock::new();
        self.bpm = bpm;
    }

    /// The musical beat last shown (for launches: [`next_one`] of it).
    pub fn beats(&self) -> f64 {
        self.last.unwrap_or(0.0)
    }

    /// The tempo the clock is using now.
    pub fn bpm(&self) -> f32 {
        self.bpm
    }

    fn tracked_m(&mut self, count: f64, downbeat: Option<u32>) -> f64 {
        let downbeat = downbeat.or(self.downbeat);
        if !self.tracked || downbeat != self.downbeat {
            // (Re)build `shift`: `-downbeat` puts the "1"s on multiples of
            // four; then whole bars until we are not behind what was shown.
            let bar = BEATS_PER_BAR as f64;
            let base = -(downbeat.unwrap_or(0) as f64);
            let floor = self.last.unwrap_or(f64::NEG_INFINITY);
            let behind = floor - (count + base);
            let bars = if behind > 0.0 { (behind / bar).ceil() } else { 0.0 };
            // Measured against what was shown, so a re-phase never drops
            // bars the clock already counted.
            self.shift = base + bar * bars;
            self.downbeat = downbeat;
            self.tracked = true;
        }
        count + self.shift
    }

    fn free_m(&mut self, time: f64) -> f64 {
        if self.tracked || self.anchor_t.is_nan() || self.anchor_bpm != self.bpm {
            // Re-anchor where the clock is now: from tracked, at the last
            // position shown; on a tempo change, where the old tempo had got to.
            self.anchor_m = if self.anchor_t.is_nan() || self.tracked {
                self.last.unwrap_or(0.0)
            } else {
                self.anchor_m + (time - self.anchor_t) * self.anchor_bpm as f64 / 60.0
            };
            self.anchor_t = time;
            self.anchor_bpm = self.bpm;
            self.tracked = false;
        }
        self.anchor_m + (time - self.anchor_t) * self.anchor_bpm as f64 / 60.0
    }
}

/// A song's beat map, read back from its `<song>.tempo.json` sidecar
/// (`metal_listen::file::TempoMap`; plain numbers here so this module still
/// depends on no tracker). Playback at song time `t` feeds [`MusicClock`]
/// exactly what a live tracker would: [`TempoTrack::at`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TempoTrack {
    /// (start time, BPM), in time order.
    pub segments: Vec<(f64, f32)>,
    /// Every beat's time, ascending.
    pub beats: Vec<f64>,
    /// The beats that are a "1" (a subset of `beats`).
    pub downbeats: Vec<f64>,
}

impl TempoTrack {
    /// The tempo the map says at `t`: the segment `t` falls in (the first one
    /// before it starts), or the median beat spacing when there are no
    /// segments.
    pub fn bpm_at(&self, t: f64) -> Option<f32> {
        let seg = self.segments.iter().take_while(|(s, _)| *s <= t).last().or(self.segments.first());
        if let Some(&(_, bpm)) = seg {
            return Some(bpm).filter(|b| b.is_finite() && *b > 0.0);
        }
        let mut gaps: Vec<f64> = self.beats.windows(2).map(|w| w[1] - w[0]).filter(|g| *g > 0.0).collect();
        if gaps.is_empty() {
            return None;
        }
        gaps.sort_by(|a, b| a.total_cmp(b));
        Some((60.0 / gaps[gaps.len() / 2]) as f32)
    }

    /// The beat index (into `beats`) of the first "1", folded mod 4 — what
    /// [`MusicClock::update`] takes as `downbeat`.
    pub fn downbeat(&self) -> Option<u32> {
        let one = *self.downbeats.first()?;
        let i = self.beats.iter().position(|b| (b - one).abs() < 1e-3)?;
        Some(i as u32 % BEATS_PER_BAR)
    }

    /// The beat count at song time `t`: the index of the beat just before,
    /// plus how far it is to the next one. Before the first and after the
    /// last beat it extrapolates at the nearest spacing (or the tempo), so
    /// the clock keeps running through an intro or outro. `None` with fewer
    /// than two beats.
    pub fn count_at(&self, t: f64) -> Option<f32> {
        let b = &self.beats;
        if b.len() < 2 {
            return None;
        }
        let n = b.len();
        let c = if t < b[0] {
            (t - b[0]) / (b[1] - b[0]).max(1e-6)
        } else if t >= b[n - 1] {
            let period = self.bpm_at(t).map(|bpm| 60.0 / bpm as f64).unwrap_or(b[n - 1] - b[n - 2]);
            (n - 1) as f64 + (t - b[n - 1]) / period.max(1e-6)
        } else {
            let i = b.partition_point(|&x| x <= t) - 1;
            i as f64 + (t - b[i]) / (b[i + 1] - b[i]).max(1e-6)
        };
        Some(c as f32)
    }

    /// `(count, downbeat, bpm)` at song time `t`, ready for
    /// [`MusicClock::update`].
    pub fn at(&self, t: f64) -> (Option<f32>, Option<u32>, Option<f32>) {
        (self.count_at(t), self.downbeat(), self.bpm_at(t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A song at 120 BPM whose first beat is at 0.3 s and whose "1" is beat 2.
    fn song() -> TempoTrack {
        let beats: Vec<f64> = (0..400).map(|i| 0.3 + i as f64 * 0.5).collect();
        let downbeats = beats.iter().skip(2).step_by(4).copied().collect();
        TempoTrack { segments: vec![(0.0, 120.0)], beats, downbeats }
    }

    #[test]
    fn song_playback_lands_step_zero_on_the_sidecar_downbeats() {
        let track = song();
        assert_eq!(track.downbeat(), Some(2));
        let mut clock = MusicClock::new();
        for (k, &one) in track.downbeats.iter().take(40).enumerate() {
            let (c, d, b) = track.at(one);
            let p = clock.update(c, d, b, one);
            assert_eq!((p.step, p.bar), (0, k as u64), "the \"1\" at {one}");
            // Halfway to the next beat is step 2.
            let (c, d, b) = track.at(one + 0.25);
            assert_eq!(clock.update(c, d, b, one + 0.25).step, 2);
        }
        assert_eq!(clock.bpm(), 120.0);
    }

    #[test]
    fn a_seek_jumps_back_and_lands_on_the_grid() {
        let track = song();
        let mut clock = MusicClock::new();
        let one = track.downbeats[20];
        let (c, d, b) = track.at(one);
        let far = clock.update(c, d, b, one);
        assert_eq!((far.bar, far.step), (20, 0));
        // Without a seek, going back holds the position…
        let early = track.downbeats[3] + 0.5;
        let (c, d, b) = track.at(early);
        assert_eq!(clock.update(c, d, b, early), far);
        // …with one, it lands on bar 3, beat 1 (step 4).
        clock.seek();
        let p = clock.update(c, d, b, early);
        assert_eq!((p.bar, p.step), (3, 4));
        // And forwards past the end of the map it keeps the tempo.
        clock.seek();
        let late = track.beats[399] + 2.0; // 4 beats past beat 399 (a "1"+1)
        let (c, d, b) = track.at(late);
        let p = clock.update(c, d, b, late);
        assert_eq!(p.beats(), (399.0 + 4.0 - 2.0) as f64);
    }

    #[test]
    fn a_map_with_no_beats_gives_the_free_clock_its_tempo() {
        let track = TempoTrack { segments: vec![(0.0, 90.0)], ..Default::default() };
        assert_eq!(track.at(5.0), (None, None, Some(90.0)));
        let mut clock = MusicClock::new();
        let (c, d, b) = track.at(0.0);
        clock.update(c, d, b, 0.0);
        // 90 BPM: one bar (4 beats) every 8/3 s.
        let p = clock.update(None, None, None, 8.0 / 3.0);
        assert_eq!((p.bar, p.step), (1, 0));
    }

    const FPS: f64 = 60.0;

    /// A tracker at a steady tempo whose beat 1 is the "1".
    fn count_at(bpm: f32, t: f64) -> f32 {
        (t * bpm as f64 / 60.0) as f32
    }

    #[test]
    fn step_zero_lands_exactly_on_the_one_at_90_128_174() {
        for bpm in [90.0f32, 128.0, 174.0] {
            let mut clock = MusicClock::new();
            for n in 0..32u32 {
                // Beat 1 + 4n is a "1": step 0, phase 0, the start of a bar.
                let one = (1 + 4 * n) as f32;
                let p = clock.update(Some(one), Some(1), Some(bpm), one as f64 * 60.0 / bpm as f64);
                assert_eq!((p.step, p.beat_in_bar), (0, 0), "{bpm} BPM, bar {n}");
                assert!(p.phase < 1e-4, "{bpm}: phase {}", p.phase);
                assert_eq!(p.bar, n as u64);
                // A sixteenth later is step 1, a beat later beat 1 of the bar.
                let p = clock.update(Some(one + 0.25), Some(1), Some(bpm), 0.0);
                assert_eq!(p.step, 1);
                let p = clock.update(Some(one + 1.0), Some(1), Some(bpm), 0.0);
                assert_eq!((p.step, p.beat_in_bar), (4, 1));
            }
        }
    }

    #[test]
    fn a_tracked_clock_does_not_drift_over_ten_minutes() {
        for bpm in [90.0f32, 128.0, 174.0] {
            let mut clock = MusicClock::new();
            let frames = (600.0 * FPS) as u32;
            let mut p = clock.update(Some(0.0), Some(0), Some(bpm), 0.0);
            for f in 1..=frames {
                let t = f as f64 / FPS;
                p = clock.update(Some(count_at(bpm, t)), Some(0), Some(bpm), t);
            }
            // 10 min at `bpm` is exactly 10*bpm beats = 2.5*bpm bars.
            let expect = Position::from_beats(10.0 * bpm as f64);
            assert_eq!((p.bar, p.step), (expect.bar, expect.step), "{bpm}");
        }
    }

    #[test]
    fn the_free_clock_does_not_drift_over_ten_minutes_of_frames() {
        let mut clock = MusicClock::new();
        let mut p = clock.update(None, None, Some(128.0), 0.0);
        for f in 1..=(600.0 * FPS) as u32 {
            p = clock.update(None, None, None, f as f64 / FPS);
        }
        // 1280 beats = bar 320, step 0, on the nose.
        assert_eq!((p.bar, p.step), (320, 0));
        assert!(p.phase < 1e-4, "phase {}", p.phase);
    }

    #[test]
    fn silence_falls_back_to_the_constant_tempo() {
        let mut clock = MusicClock::new();
        clock.update(None, None, None, 10.0);
        // 2 s at FALLBACK_BPM (120) = 4 beats = one bar.
        let p = clock.update(None, None, None, 12.0);
        assert_eq!((p.bar, p.step), (1, 0));
    }

    #[test]
    fn a_free_tempo_change_bends_without_jumping_and_stays_on_the_grid() {
        let mut clock = MusicClock::new();
        clock.update(None, None, Some(120.0), 0.0);
        let before = clock.update(None, None, None, 1.0); // 2 beats
        assert_eq!(before.step, 8);
        // Tempo goes to 140 at t = 1: the clock carries on from beat 2.
        let at = clock.update(None, None, Some(140.0), 1.0);
        assert_eq!(at, before);
        // Each sixteenth from here is exactly 60/140/4 s.
        let sixteenth = 60.0 / 140.0 / 4.0;
        for k in 1..64u32 {
            let p = clock.update(None, None, None, 1.0 + k as f64 * sixteenth);
            assert_eq!(p.step as u32, (8 + k) % 16, "k {k}");
            assert!(p.phase < 1e-4 || p.phase > 1.0 - 1e-4, "k {k}: {}", p.phase);
        }
    }

    #[test]
    fn a_tracked_tempo_change_keeps_steps_on_the_beat_grid() {
        // 124 BPM for 30 s, then 140: the tracker's count bends; every whole
        // beat it reports is still a step multiple of four with phase 0.
        let mut clock = MusicClock::new();
        let count = |t: f64| {
            if t < 30.0 { t * 124.0 / 60.0 } else { 62.0 + (t - 30.0) * 140.0 / 60.0 }
        };
        let mut last = clock.update(Some(0.0), Some(0), Some(124.0), 0.0);
        for f in 1..(60.0 * FPS) as u32 {
            let t = f as f64 / FPS;
            let p = clock.update(Some(count(t) as f32), Some(0), Some(if t < 30.0 { 124.0 } else { 140.0 }), t);
            assert!(p.beats() >= last.beats() - 1e-6);
            last = p;
        }
        // Every whole beat after the change, fed at the time it falls on.
        let mut clock = MusicClock::new();
        for beat in 62u32..200 {
            let t = 30.0 + (beat - 62) as f64 * 60.0 / 140.0;
            let p = clock.update(Some(count(t) as f32), Some(0), Some(140.0), t);
            assert_eq!(p.step, ((beat % 4) * 4) as u8, "beat {beat}");
            assert!(p.phase < 1e-3, "beat {beat}: {}", p.phase);
        }
    }

    #[test]
    fn free_to_tracked_never_jumps_backwards_and_lands_on_the_music_one() {
        let mut clock = MusicClock::new();
        clock.update(None, None, Some(120.0), 0.0);
        let free = clock.update(None, None, None, 5.15); // m = 10.3
        assert!((free.beats() - 10.3).abs() < 1e-3);
        // The tracker locks: count 3.1, its beat 1 is a "1" → m = 2.1 + bars.
        let p = clock.update(Some(3.1), Some(1), Some(120.0), 5.16);
        assert!(p.beats() >= free.beats(), "{p:?} after {free:?}");
        assert!((p.beats() - 14.1).abs() < 1e-3, "{p:?}");
        // …and the music's "1" (count 5) is step 0.
        let p = clock.update(Some(5.0), Some(1), None, 6.0);
        assert_eq!(p.step, 0);
    }

    #[test]
    fn tracked_to_free_carries_on_from_where_it_was() {
        let mut clock = MusicClock::new();
        let a = clock.update(Some(9.5), Some(0), Some(120.0), 100.0);
        let b = clock.update(None, None, None, 100.0);
        assert_eq!(a, b);
        let c = clock.update(None, None, None, 100.5); // one beat at 120
        assert!((c.beats() - 10.5).abs() < 1e-6);
    }

    #[test]
    fn a_count_that_steps_back_holds_rather_than_replaying() {
        let mut clock = MusicClock::new();
        let a = clock.update(Some(20.0), Some(0), None, 0.0);
        let b = clock.update(Some(3.0), Some(0), None, 0.1);
        assert_eq!(a, b);
    }

    #[test]
    fn a_new_downbeat_rephases_forward_onto_the_new_one() {
        let mut clock = MusicClock::new();
        let a = clock.update(Some(10.0), Some(0), None, 0.0); // bar 2 step 8
        assert_eq!((a.bar, a.step), (2, 8));
        let b = clock.update(Some(10.0), Some(2), None, 0.0); // 10 is now a "1"
        assert_eq!(b.step, 0);
        assert!(b.beats() >= a.beats());
    }

    #[test]
    fn a_loop_is_sixteen_half_beat_slots_and_the_bar_keeps_sixteen_ticks() {
        assert_eq!(STEPS, 16);
        assert_eq!(STEPS_PER_BEAT, 2, "a loop slot is a half beat");
        assert_eq!(TICKS_PER_BEAT, 4, "the clock still counts sixteenths");
        assert_eq!(BEATS_PER_BAR * TICKS_PER_BEAT, STEPS as u32, "one bar is sixteen ticks");
        // A slot is two clock ticks: the first slot starts on the "1".
        assert_eq!(Position::from_beats(0.0).step, 0);
        assert_eq!(Position::from_beats(0.5).step, 2, "one beat is two slots");
    }

    #[test]
    fn launches_wait_for_the_next_one_with_a_grace() {
        assert_eq!(next_one(4.0), 4.0);
        assert_eq!(next_one(4.2), 4.0);
        assert_eq!(next_one(4.3), 8.0);
        assert_eq!(next_one(0.0), 0.0);
    }
}
