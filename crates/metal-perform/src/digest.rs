//! The musical digest — the one input the whole suite is keyed to.
//!
//! **Nothing here analyses audio, and now something does.** The real digester
//! landed as the `metal-listen` crate; `crate::live` adapts it to [`Source`] and
//! `metal-dash --listen <source>` runs on it. This module is still what it always
//! was — the *contract*, plus a stand-in that satisfies it — and that is why
//! nothing downstream of this file changed when the real one arrived.
//!
//! [`Simulated`] was to be deleted when that happened. It was not, for two
//! reasons that only became obvious once there was something to compare it
//! against: `drive.rs` has a dozen tests that need a *deterministic* row for a
//! given instant, and a venue laptop with nothing plugged in still has to draw.
//! It is no longer the default when a real input is available, which was the
//! actual complaint behind that line.
//!
//! **The contract is not invented here.** `docs/AUDIO_FEATURES.md` fixes it:
//! the sixteen-column music prefix, the thirteen-column voice block
//! (V2, 2026-10-05) and the six-column tone block (the tone design notes D1/D2), on the
//! 8 Hz grid, one row per 125 ms, the same clock the notation is written on.
//! Every width is restated in [`COLUMNS`] from the one declaration in
//! `metal_schema`, and are what [`Source::poll`] returns. The real
//! digester implements [`Source`] and nothing downstream of this file changed
//! when it did — that is the whole point of putting the seam here, and it held.

/// The **base audio tick rate** — the bottom rung of `metal_schema::TickRate`'s
/// ladder, derived rather than written as a literal. A live source runs at the
/// active level; this is the grid the synthesised/stand-in digest is authored
/// against and the fallback when no level is plumbed.
pub const RATE: f32 = metal_schema::BASE_HZ as f32;

/// The width of a digest row: the music prefix ([`MUSIC`]), the voice block
/// ([`VOICE`]) then the tone block ([`TONE`]). Declared once in
/// `metal_schema::FEATURES`.
pub const FEATURES: usize = metal_schema::FEATURES;
/// The music prefix width — columns 0..16, the corpus and the Colourist's.
pub const MUSIC: usize = metal_schema::MUSIC;
/// The voice block width — columns 16..29.
pub const VOICE: usize = metal_schema::VOICE;
/// The tone block width — columns 29..35. The Basic Pitch mood block the
/// colour mapping reads (the tone design notes D8); older, 29-wide rows load with these
/// six columns at zero, which is what makes the mapping fall back to chroma.
pub const TONE: usize = metal_schema::TONE;
/// Where the tone block begins — the `tone_x` column, index 29.
pub const TONE_X: usize = MUSIC + VOICE;
/// `tone_y` — the circle-of-fifths vector's sine. Together with [`TONE_X`] it
/// stores a hue as a *vector*, so pooling and smoothing average it correctly
/// where an angle would wrap (the tone design notes D3).
pub const TONE_Y: usize = TONE_X + 1;
/// `tone_register` — the weighted mean MIDI pitch, 0 low .. 1 high. Drives
/// lightness (the tone design notes §4).
pub const TONE_REGISTER: usize = TONE_X + 2;
/// `tone_density` — how thick the harmony is, sparse ↔ dense. Drives chroma
/// and the lightness variety between stops.
pub const TONE_DENSITY: usize = TONE_X + 3;
/// `tone_mode` — a major/minor template match mapped to 0..1, minor ↔ major.
/// Shifts the gradient's hue by ±15°.
pub const TONE_MODE: usize = TONE_X + 4;
/// `tone_conf` — whether there is pitched content at all. The crossfade
/// control: below ~0.05 the colour mapping is exactly the chroma path of old.
pub const TONE_CONF: usize = TONE_X + 5;
/// The `hiss` column inside the voice block — pooled by max, so it rides
/// `raw` like onset. See [`crate::drive::smooth_default`].
pub const HISS: usize = MUSIC + 8;

/// What each column is, in `docs/AUDIO_FEATURES.md`'s order. Used by the
/// monitor to label the row, so the operator sees "onset" and not "f2".
///
/// Declared once in `metal_schema::DIGEST_COLUMNS` — the same names an audio
/// node's `control` variables carry in `metal_graph::params` (a binding such as
/// `control.onset`), so the label and the variable cannot drift apart.
pub const COLUMNS: [&str; FEATURES] = metal_schema::DIGEST_COLUMNS;

/// One 125 ms row. Every column is in 0..1, so consumers can treat the whole
/// row uniformly and never need to know which is which.
///
/// # That is a promise this type makes, not a fact about the digest
///
/// It was written as though it described the real digester. **It does not.**
/// Measured over the 74-song corpus in `data/audio` — 158 457 rows — fourteen
/// of the sixteen columns do arrive bounded, and two are nowhere near it:
/// bandwidth runs to 4 209 (it is in Hz) and onset to 38 (it is a decibel
/// difference). Only [`Simulated`] ever actually produced 0..1 across the board.
///
/// So the guarantee is real but it is *enforced on the way in*, by
/// `crate::live`, which scales those two columns and documents the corpus
/// percentiles it uses. Anything else constructing a `Frame` owes the same
/// scaling — a row straight out of `metal_listen` or out of a `.npy` in
/// `data/audio` will drive the renderers with numbers in the thousands, and
/// they were all tuned against this promise.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// The row interpolated across the 125 ms window — the previous row moving
    /// toward the current one. Smooth, and one row behind: it only reaches the
    /// current row as the window closes.
    pub f: [f32; FEATURES],
    /// The row as the digest actually stepped it. Instant, and a staircase.
    ///
    /// Both are carried because *which one a float should ride is a property
    /// of the float*, not of the source — a transient wants `raw` and a colour
    /// wants `f`, and the source has no idea which of the two it is feeding.
    /// See `FloatSpec::smooth` and [`crate::drive::smooth_default`].
    pub raw: [f32; FEATURES],
    /// The tick level this row is on. `Base` for the live/stand-in path today;
    /// the drive smoothing converts its per-tick fraction with
    /// `metal_schema::per_tick_decay` against this, so a fine machine smooths the
    /// same wall-clock amount. It is the runtime active level's seam into the
    /// engine.
    pub rate: metal_schema::TickRate,
    /// Whether the row carries an **analysed voice block** — the plan's "the
    /// source's row carries a voice block", the first of the two conditions
    /// decision 11 puts on `face::apply`. It is *not* "voice is present": a
    /// silent row from a voice-digesting source sets this true with every voice
    /// column at zero, and the face solver then idles (breathes, blinks,
    /// nods). A row from a 16-wide source that was never digested for voice
    /// sets it false, and the solver writes nothing — the model rests at its
    /// authored row instead of animating a singer who was never measured. See
    /// `metal_router::npy::has_voice`.
    pub voice: bool,
}

impl Frame {
    /// A row with nothing to interpolate against, so both signals are the same.
    /// The honest constructor for any source that is already a continuous
    /// function of time — it has no staircase to remove.
    pub fn step(f: [f32; FEATURES]) -> Frame {
        Frame::step_at(f, metal_schema::TickRate::Base)
    }

    /// [`Frame::step`] on an explicit tick level.
    pub fn step_at(f: [f32; FEATURES], rate: metal_schema::TickRate) -> Frame {
        Frame { f, raw: f, rate, voice: true }
    }

    /// [`Frame::step`] with no analysed voice block, so a face model leaves its
    /// rig floats alone (see [`Frame::voice`]).
    pub fn step_without_voice(f: [f32; FEATURES], rate: metal_schema::TickRate) -> Frame {
        Frame { f, raw: f, rate, voice: false }
    }

    /// Column 2. Named because the transient is the one column anything
    /// outside this module reads on its own — it is what an audience sees a
    /// visual respond to, so it is what the operator's beat light watches.
    ///
    /// Reads `raw`, not `f`. A beat light is the one readout in the app whose
    /// entire job is to agree with the hit an operator can hear, and the
    /// interpolated row reaches the hit 125 ms after it happened.
    pub fn onset(&self) -> f32 {
        self.raw[2]
    }
}

/// What the loop detector exposes when it is locked — the *second output* of
/// the same analysis [`Source::beats`] is part of. The 16-wide row is shared
/// with the corpus and the notation, so repetition is a method, not a 17th
/// column (plan decision 4).
///
/// A mirror of `metal_listen::repeat::RepeatState`, not an import: `metal-listen` is
/// an optional dependency of this crate (the venue laptop with nothing plugged
/// in must still build), so the seam cannot name its type. `crate::live` owns
/// the translation and the field names are kept identical so that conversion is
/// a field-for-field copy and cannot silently drop one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RepeatState {
    /// The detected loop length, in beats.
    pub length_beats: u32,
    /// The absolute beat count when the loop began.
    pub since_beat: f32,
    /// The similarity score at lock, 0..1.
    pub confidence: f32,
}

/// Where digest rows come from. One implementor today; the microphone and the
/// loopback capture are the two that make this a trait rather than a struct.
pub trait Source {
    /// The row for wall-clock `time` in seconds. Called once per served
    /// frame, not on a fixed clock, so an implementation must be a function
    /// of `time` rather than of how often it was asked.
    fn poll(&mut self, time: f32) -> Frame;

    /// How many beats have gone by at `time`, fractionally, or `None` from a
    /// source that cannot follow a tempo.
    ///
    /// # Why this is not a seventeenth column
    ///
    /// The Router needs to switch every sixteen beats, and **nothing in this
    /// project counted a beat**. `AUDIO_FEATURES.md` fixes sixteen columns and
    /// none of them is tempo, beat index or beat phase; column 2 is *onset*, a
    /// transient with no notion of which hit or how many. `SUITE.md` §2 already
    /// recorded the gap — the handoff asked for tempo and its stability and the
    /// sixteen do not carry it — and `docs/NEXT.md` §1 named it as the one
    /// thing blocking the Router, with three ways out.
    ///
    /// This is a fourth, and it is the cheap honest one. The digest's music
    /// **prefix** is still exactly sixteen columns, because that width is
    /// shared with the notation, the corpus and `digest_audio.py`, and putting a
    /// beat inside it would make every one of them disagree about what column 16
    /// is. But
    /// a beat is not a per-frame feature in the first place: a real tracker
    /// emits a tempo estimate and beat positions, on their own clock, which is
    /// a *second output* of the same analysis. So it is a second method.
    ///
    /// Counting onsets was the alternative and it is wrong in a way an operator
    /// sees immediately — eight onsets is two beats on a busy bar and thirty
    /// seconds on a breakdown.
    ///
    /// Must be monotonic in `time` for a given source, or a router built on it
    /// will switch backwards.
    fn beats(&self, time: f32) -> Option<f32>;

    /// Whether the music is right now repeating a phrase the DJ looped, and how
    /// long that phrase is, or `None` when nothing is repeating.
    ///
    /// # Why this is a method and not a seventeenth column
    ///
    /// Exactly [`Source::beats`]'s reason, and it is worth saying twice because
    /// the temptation is the same: the digest's music prefix is sixteen
    /// columns, shared with the corpus (`data/audio`), the notation and
    /// `digest_audio.py`, so repetition cannot become column 16 without all of
    /// them disagreeing about what column 16 is. It is also not a per-frame feature — nothing about a
    /// single 125 ms window says "these bars have played before". A loop is a
    /// statement about the last two *cycles* of the beat grid, which is a second
    /// output of the same analysis the beat clock already is. So it is a second
    /// method, mirroring [`Source::beats`] (plan decision 4).
    ///
    /// # The default is `None`, and it is honest
    ///
    /// A source that cannot follow a tempo cannot know a phrase is repeating —
    /// the detector is built *on* the beat clock, beat by beat (`metal-listen`'s
    /// `repeat::Repeat` has no clock of its own). `None` therefore means "no
    /// loop detected", not "not repeating": it is the truthful answer from a
    /// source with no ears, and the same answer a real one gives before it has
    /// heard two cycles. A stand-in that never loops simply inherits this.
    ///
    /// Allowed to flap between `Some` and `None` as the score crosses a
    /// threshold. Smoothing that is the caller's job — the Looper (Phase 3)
    /// owns the hysteresis — because how long to hold a visual on a loop is a
    /// performance decision, not a fact about the audio. What this must not do
    /// is report a loop that contradicts the row [`Source::poll`] returns for
    /// the same instant: the two are one analysis and a visual that repeats
    /// while the music has moved on is worse than no loop detection at all.
    fn repeat(&self) -> Option<RepeatState> {
        None
    }

    /// Which beat index of [`Source::beats`]'s count is a "1" (a bar's
    /// downbeat), or `None` when the source has not heard one — the musical
    /// clock (`crate::clock`) then counts bars from beat 0. Default `None`;
    /// `metal-listen`'s downbeat detector (the loops design notes T2) answers it for the
    /// live input once it is wired.
    fn downbeat(&self) -> Option<u32> {
        None
    }

    /// "Tap 1": the beat nearest now becomes the bar's first. Returns whether
    /// the source accepted it (only a real tracker can; the stand-in's "1" is
    /// fixed at beat 0).
    fn tap_downbeat(&mut self) -> bool {
        false
    }

    /// What the operator should see next to the input selector. The real
    /// sources will say the device; this one says it is not real.
    fn describe(&self) -> String;

    /// Whether this is analysing actual audio. The page puts the whole INPUT
    /// panel in a "simulated" state when it is false, because an operator who
    /// cannot tell a stand-in from the real thing will eventually take a
    /// stand-in to a venue.
    fn is_real(&self) -> bool;
}

// ---------------------------------------------------------------------------

/// A digest that behaves like music without listening to any.
///
/// Not white noise. Independent random floats per column would make every
/// model flicker into static, which would be a worse lie than a stand-in
/// needs to tell: it would misrepresent what the renderers do when driven,
/// and the renderers are what this app exists to show. So each column moves
/// the way its real counterpart moves — loudness drifts and swells, onset
/// spikes on a beat and decays, chroma holds a chord for a few bars and then
/// changes — and the result is something you can judge a model's reaction
/// against.
///
/// Deterministic in `time` and `seed`. Two polls at the same instant return
/// the same row, which is what makes it testable and what stops the preview
/// jittering when the page asks for a frame twice.
pub struct Simulated {
    seed: u32,
    /// Beats per minute the fake track runs at. The operator can change it,
    /// because "does this model read the beat" is the first question anyone
    /// asks of a visual and a fixed 120 makes it unanswerable.
    pub bpm: f32,
    /// 0..1 — how hard the simulation drives. At 0 the row is silent, which
    /// is the honest way to see a model's resting state.
    pub drive: f32,
    /// Whether the stand-in pretends a DJ has looped, and how long the loop is.
    /// `None` = the ordinary track. `Some(L)` = from the moment set, the row for
    /// beat `b` is the row for the beat `since + (b - since) % L` beats — the
    /// same arithmetic the real `Looper` uses, so a test of the Looper and a
    /// test of the stand-in exercise the same repeat.
    ///
    /// Clamped to a minimum of [`MIN_LOOP_BEATS`]: a loop shorter than a bar is
    /// not what the feature is for, and `metal_listen::repeat::LENGTHS` starts at
    /// the four-beat bar for the same measured reason (a one-beat loop is the
    /// degenerate minimum and locked on nearly every track). A bare `1` would
    /// make this stand-in agree with a detector that never produces one.
    pub loop_beats: Option<u32>,
}

/// The shortest loop the stand-in will pretend to be playing, and the length
/// `metal_listen::repeat::LENGTHS` starts at. Restated, not imported: this crate
/// has no `metal-listen` in it (see [`RepeatState`]).
pub const MIN_LOOP_BEATS: u32 = 4;

impl Simulated {
    pub fn new(seed: u32) -> Simulated {
        Simulated {
            seed,
            bpm: 124.0,
            drive: 0.8,
            loop_beats: None,
        }
    }

    /// Set the pretend loop length, clamped to [`MIN_LOOP_BEATS`]. `None` is
    /// the ordinary track.
    pub fn with_loop(mut self, loop_beats: Option<u32>) -> Simulated {
        self.loop_beats = loop_beats.map(|l| l.max(MIN_LOOP_BEATS));
        self
    }

    /// The beat whose row the track should be playing at `time`, which is the
    /// last beat before `time` folded back into the loop when a loop is set.
    ///
    /// Every row is derived from this rather than from `time` directly, so
    /// [`Simulated::poll`] and [`Simulated::repeat`] cannot disagree about
    /// whether a loop is running — which is the property the seam promises.
    ///
    /// A loop starts at beat 0, so `since_beat` is always `0.0`: this stand-in
    /// cannot be made to begin a loop part-way through the night, and pretending
    /// otherwise would be a dishonest knob no test needs.
    fn effective_beats(&self, time: f32) -> f32 {
        let beats = time * (self.bpm / 60.0);
        match self.loop_beats {
            Some(len) => beats % len.max(MIN_LOOP_BEATS) as f32,
            None => beats,
        }
    }
}

/// Smooth, bounded, deterministic noise in 0..1. Value noise rather than a
/// random walk: a walk accumulates and would need clamping that shows up as
/// the signal sticking to its rails.
fn wave(seed: u32, channel: u32, t: f32) -> f32 {
    let a = hash(seed ^ channel.wrapping_mul(0x9E37_79B9));
    let b = hash(seed ^ channel.wrapping_mul(0x85EB_CA6B) ^ 0x1234_5678);
    let (r1, r2) = (0.31 + a * 0.9, 0.13 + b * 0.5);
    // Two incommensurate rates so the pattern does not visibly repeat.
    let s = (t * r1 + a * 6.28).sin() * 0.62 + (t * r2 + b * 6.28).sin() * 0.38;
    (s * 0.5 + 0.5).clamp(0.0, 1.0)
}

/// Deterministic 32-bit hash to 0..1. Integer-in, float-out, no state.
fn hash(x: u32) -> f32 {
    let mut h = x.wrapping_add(0x9E37_79B9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    (h >> 8) as f32 / 16_777_216.0
}

impl Source for Simulated {
    fn poll(&mut self, time: f32) -> Frame {
        let mut f = [0.0f32; FEATURES];
        if self.drive <= 0.0 {
            return Frame::step(f);
        }

        // Where we are inside the current beat, 0 at the hit and 1 just
        // before the next. This is the only thing in here with a tempo, and
        // everything percussive is derived from it. `effective_beats` folds
        // the beat back when `loop_beats` is set, so the row really does
        // repeat — it is not a decoration on top of the track.
        let beats = self.effective_beats(time);
        // The clock every non-percussive column rides too: when a loop is set
        // this is `time` folded back with the beat, so the swells and the
        // chord do not keep drifting while the beat repeats. One clock for the
        // whole row is what makes a repeat a repeat.
        let local_time = beats * (60.0 / self.bpm);
        let phase = beats.fract();
        let beat_index = beats.floor() as u32;
        // Every fourth beat is the downbeat and hits harder — enough
        // structure for "does this model follow the bar" to be a real
        // question about the model rather than about the stimulus.
        let accent = if beat_index % 4 == 0 { 1.0 } else { 0.62 };

        // 2 — onset: a spike at the hit decaying over the beat. Pooled by
        // max in the real digester precisely so transients survive, so the
        // stand-in must not smear it.
        f[2] = accent * (1.0 - phase).powf(6.0);

        // 0 — RMS: a slow swell with the beat riding on top. Real loudness
        // does both and a model keyed only to the swell would look dead.
        let swell = 0.35 + 0.5 * wave(self.seed, 1, local_time * 0.09);
        f[0] = (swell * (0.72 + 0.28 * f[2])).clamp(0.0, 1.0);

        // 1, 3 — bandwidth and centroid: slow, correlated, and brighter when
        // louder, which is what happens when a mix opens up.
        f[1] = (0.25 + 0.55 * wave(self.seed, 2, local_time * 0.07) + 0.15 * f[0]).clamp(0.0, 1.0);
        f[3] = (0.20 + 0.45 * wave(self.seed, 3, local_time * 0.05) + 0.25 * f[0]).clamp(0.0, 1.0);

        // 4..15 — chroma: one chord held for four bars, then a new one. Root,
        // third and fifth lit, everything else at a low floor. Changing on a
        // bar rather than continuously is what makes a colour slot keyed to
        // harmony change when a listener would expect it to.
        let bar = (beats / 16.0).floor() as u32;
        let root = (hash(self.seed ^ bar.wrapping_mul(0x7F4A_7C15)) * 12.0) as usize % 12;
        let minor = hash(self.seed ^ bar ^ 0xABCD) > 0.5;
        let third = (root + if minor { 3 } else { 4 }) % 12;
        let fifth = (root + 7) % 12;
        for (i, slot) in f[4..MUSIC].iter_mut().enumerate() {
            *slot = 0.06 + 0.10 * wave(self.seed, 40 + i as u32, local_time * 0.3);
        }
        f[4 + root] = 0.85;
        f[4 + third] = 0.60;
        f[4 + fifth] = 0.70;

        // 16..29 — voice: a sung phrase every eight bars, silence between.
        // The stand-in has no ears, so this is what a singer *would* do, in
        // the same shape the analyser writes: presence and a moving pitch
        // while the phrase lasts, style scores soft and summing to presence.
        // Two bars of phrase, six of rest, so a model keyed to `sing` or to
        // `pitch` has something to react to on a deterministic clock.
        if bar % 8 < 2 {
            let vib = (local_time * 5.5).sin();
            f[MUSIC] = 1.0; // voice
            f[MUSIC + 1] = (0.35 + 0.4 * f[0]).clamp(0.0, 1.0); // voice level
            f[MUSIC + 2] = (0.45 + 0.08 * vib).clamp(0.0, 1.0); // pitch
            f[MUSIC + 3] = 0.85; // voicing
            f[MUSIC + 4] = 0.9; // steady
            f[MUSIC + 5] = (0.12 + 0.12 * wave(self.seed, 60, local_time * 0.5)).clamp(0.0, 1.0); // syllables
            f[MUSIC + 6] = (0.3 + 0.3 * wave(self.seed, 61, local_time * 0.4)).clamp(0.0, 1.0); // open
            f[MUSIC + 7] = (0.3 + 0.3 * wave(self.seed, 62, local_time * 0.35)).clamp(0.0, 1.0); // wide
            f[MUSIC + 8] = 0.02; // hiss
            f[MUSIC + 9] = 0.9; // sing
            f[MUSIC + 11] = 0.1; // speak
        }

        for v in f.iter_mut() {
            *v = (*v * self.drive).clamp(0.0, 1.0);
        }
        Frame::step(f)
    }

    /// The stand-in knows its own tempo exactly — it invented it. A real
    /// digester will have to estimate this, and will be wrong at a drop; that
    /// is a property of beat tracking and not of this seam.
    fn beats(&self, time: f32) -> Option<f32> {
        if self.drive <= 0.0 {
            // Silence has no beat. The router falls back to its clock and says
            // so, rather than switching to a tempo nobody is playing.
            return None;
        }
        Some(time * (self.bpm / 60.0))
    }

    /// While a loop is set and the source is driven, the stand-in reports it:
    /// `confidence` is `1.0` because the repeat is exact by construction. A
    /// silent source reports `None` even with a loop set — silence has no
    /// loop, the same way it has no beat clock (`silence_has_no_beat_clock_at_all`),
    /// and `since_beat` is `0.0` because the loop starts at beat 0 (see
    /// [`Simulated::effective_beats`]).
    fn repeat(&self) -> Option<RepeatState> {
        if self.drive <= 0.0 {
            return None;
        }
        self.loop_beats.map(|length_beats| RepeatState {
            length_beats: length_beats.max(MIN_LOOP_BEATS),
            since_beat: 0.0,
            confidence: 1.0,
        })
    }

    /// Beat 0 is the "1": `poll` accents every fourth beat from 0.
    fn downbeat(&self) -> Option<u32> {
        if self.drive <= 0.0 {
            return None;
        }
        Some(0)
    }

    fn describe(&self) -> String {
        format!("simulated · {:.0} BPM", self.bpm)
    }

    fn is_real(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_is_the_width_the_spec_fixes_and_every_column_is_named() {
        // AUDIO_FEATURES.md is the authority for the music prefix; the voice
        // block is appended after it, then the tone block (the tone design notes D1). A
        // drift here means the real digester and this stand-in disagree about
        // what column 3 is.
        assert_eq!(MUSIC, 16);
        assert_eq!(VOICE, 13);
        assert_eq!(TONE, 6);
        assert_eq!(FEATURES, MUSIC + VOICE + TONE);
        assert_eq!(COLUMNS.len(), FEATURES);
        assert_eq!(COLUMNS[2], "onset", "onset must stay at index 2");
        assert_eq!(COLUMNS[MUSIC], "voice", "the voice block starts at 16");
        // The tone block is the last six columns, 29..35, in the tone design notes D2's
        // order and starting where the block should. A consumer (the colour
        // mapping) addressing `TONE_CONF` by index would silently read a hue
        // if these moved.
        assert_eq!(TONE_X, 29, "the tone block must start at column 29");
        for (i, name) in [
            (TONE_X, "tone_x"),
            (TONE_Y, "tone_y"),
            (TONE_REGISTER, "tone_register"),
            (TONE_DENSITY, "tone_density"),
            (TONE_MODE, "tone_mode"),
            (TONE_CONF, "tone_conf"),
        ] {
            assert_eq!(COLUMNS[i], name, "tone column {i} is mislabelled");
        }
    }

    #[test]
    fn every_column_stays_inside_zero_to_one_across_ten_minutes() {
        // Consumers scale a model's declared min..max by these directly. One
        // column leaving the unit range would push a float past its declared
        // maximum and the failure would surface as a wrong-looking render,
        // not as an error.
        let mut d = Simulated::new(7);
        let mut t = 0.0;
        while t < 600.0 {
            let frame = d.poll(t);
            for (i, v) in frame.f.iter().enumerate() {
                assert!(
                    (0.0..=1.0).contains(v),
                    "column {i} ({}) left the unit range at t={t}: {v}",
                    COLUMNS[i]
                );
                assert!(v.is_finite(), "column {i} went non-finite at t={t}");
            }
            t += 1.0 / RATE;
        }
    }

    #[test]
    fn polling_the_same_instant_twice_gives_the_same_row() {
        // The page can ask for a frame twice for the same timestamp. If this
        // drifted, a paused preview would still shimmer.
        let mut d = Simulated::new(3);
        assert_eq!(d.poll(12.5).f, d.poll(12.5).f);
    }

    #[test]
    fn onset_spikes_on_the_beat_and_falls_away_between() {
        // The one property a visual operator will actually check.
        let mut d = Simulated::new(1);
        d.bpm = 120.0; // two beats a second, so beats land on exact half-seconds
        let on_beat = d.poll(4.0).onset();
        let off_beat = d.poll(4.25).onset();
        assert!(on_beat > 0.5, "no transient on the beat: {on_beat}");
        assert!(
            off_beat < on_beat * 0.2,
            "the transient smeared: {off_beat} vs {on_beat}"
        );
    }

    #[test]
    fn a_downbeat_hits_harder_than_the_beats_between() {
        let mut d = Simulated::new(1);
        d.bpm = 120.0;
        // beats 0,4,8.. are downbeats; at 120 BPM beat n is at n/2 seconds.
        assert!(
            d.poll(0.0).onset() > d.poll(0.5).onset(),
            "no accent on the bar"
        );
    }

    #[test]
    fn the_beat_clock_runs_forward_at_the_tempo_it_claims() {
        // The Router divides this by sixteen. If it were not monotonic the
        // group would switch backwards; if it did not match the BPM the
        // operator sets, the switches would visibly miss the beat light next
        // to them on the same page.
        let mut d = Simulated::new(1);
        d.bpm = 120.0;
        assert_eq!(d.beats(0.0), Some(0.0));
        assert_eq!(d.beats(30.0), Some(60.0), "120 BPM is two beats a second");
        let mut last = -1.0;
        let mut t = 0.0;
        while t < 120.0 {
            let b = d.beats(t).expect("driven");
            assert!(b > last, "the beat clock went backwards at t={t}");
            last = b;
            t += 1.0 / RATE;
        }
    }

    #[test]
    fn silence_has_no_beat_clock_at_all() {
        // Not zero, and not a stale value: `None`, so the Router can fall back
        // to its own clock and say on the page that it is doing so.
        let mut d = Simulated::new(1);
        d.drive = 0.0;
        assert_eq!(d.beats(12.0), None);
    }

    #[test]
    fn a_simulated_source_with_no_loop_reports_no_repeat() {
        // The default stand-in is an ordinary track. The trait's `repeat` is
        // `None` by default, but `Simulated` overrides it, so this pins that
        // the override still says `None` when no loop is set.
        let d = Simulated::new(1);
        assert_eq!(d.repeat(), None);
    }

    #[test]
    fn a_simulated_loop_reports_its_length_and_repeats_the_row() {
        // The seam promises `repeat` and `poll` are one analysis. Both halves
        // are checked here: the reported length, and the row actually folding
        // back on the loop arithmetic (plan step 3).
        let mut d = Simulated::new(4).with_loop(Some(4));
        d.bpm = 120.0; // two beats a second, so beats land on exact half-seconds
        assert_eq!(d.repeat().map(|r| r.length_beats), Some(4));
        assert_eq!(d.repeat(), Some(RepeatState { length_beats: 4, since_beat: 0.0, confidence: 1.0 }));
        // Beat s = 0 at t = 0; one cycle on, s + loop_beats = 4, so beat 1 and
        // beat 5 are the same beat of the loop and must draw the same row.
        let one = d.poll(0.5); // beat 1
        let five = d.poll(2.5); // beat 5
        assert_eq!(one.f, five.f, "the row did not repeat on the loop");
        assert_eq!(one.raw, five.raw, "the staircase did not repeat either");
    }

    #[test]
    fn drive_at_zero_still_has_no_repeat() {
        // Silence has no loop, for the same reason it has no beat clock: there
        // is nothing to detect a repetition in. A loop set on a silenced
        // source must not make one up.
        let mut d = Simulated::new(1).with_loop(Some(8));
        d.drive = 0.0;
        assert_eq!(d.repeat(), None);
    }

    #[test]
    fn drive_at_zero_is_actual_silence() {
        // The resting state of a model has to be observable, and "nearly
        // silent" would leave you guessing whether you were seeing the model
        // or the stimulus.
        let mut d = Simulated::new(9);
        d.drive = 0.0;
        assert_eq!(d.poll(3.3).f, [0.0; FEATURES]);
    }

    #[test]
    fn the_chord_holds_for_bars_and_then_moves() {
        // Harmony driving colour is in the handoff. If chroma changed every
        // tick, colour would strobe; if it never changed, colour would be a
        // constant. Both would be wrong about music.
        let mut d = Simulated::new(5);
        d.bpm = 120.0;
        let chroma = |d: &mut Simulated, t: f32| d.poll(t).f[4..MUSIC].to_vec();
        let a = chroma(&mut d, 0.5);
        let b = chroma(&mut d, 1.5);
        assert_eq!(
            a.iter().map(|v| (v * 10.0) as i32).collect::<Vec<_>>(),
            b.iter().map(|v| (v * 10.0) as i32).collect::<Vec<_>>(),
            "the chord changed inside a bar"
        );
        // 16 beats later is a new bar: at 120 BPM that is 8 seconds.
        let c = chroma(&mut d, 8.5);
        assert_ne!(a, c, "the chord never changes");
    }
}
