//! The real digest, wired to the app — and the scaling that has to happen on
//! the way in.
//!
//! `digest::Source` is the seam this arrives at. Everything downstream of it —
//! the mapping, the monitor, seventeen renderers — was built and judged against
//! `digest::Simulated`, and none of it changes here. That was the point of
//! putting the seam where it is.
//!
//! # The one thing that is not a straight pass-through
//!
//! `digest::Frame` says every column is already in 0..1 and that consumers can
//! treat the row uniformly. **That is true of the stand-in and false of the
//! real digester**, and the gap is not small. Measured over the 74-song corpus
//! in `data/audio` — 158 457 rows, five and a half hours:
//!
//! | column | median | p95 | max |
//! |---|---:|---:|---:|
//! | rms | 0.181 | 0.381 | 0.610 |
//! | **bandwidth** | **2619.9** | **3226.3** | **4208.5** |
//! | **onset** | **3.63** | **11.23** | **37.99** |
//! | centroid | 0.226 | 0.365 | 0.733 |
//! | chroma ×12 | ~0.35 | ~0.90 | 1.000 |
//!
//! Fourteen of the sixteen are already bounded. Two are not, and feeding those
//! straight in would drive every parameter in the suite with numbers up to four
//! thousand — not subtly wrong, *completely* wrong, on every model at once.
//!
//! So the scaling happens **here, in the adapter**, and deliberately not in
//! `metal-listen`:
//!
//! - `metal-listen` produces what `digest_audio.py` produces, because the corpus
//!   and the model are defined on those numbers and `AUDIO_FEATURES.md`'s
//!   "one pipeline, not two" is the rule that keeps training and serving
//!   honest. Normalising there would fork the format.
//! - `metal-dash` needs 0..1 to drive visuals. That is a *presentation* concern
//!   and it belongs on the presentation side of the seam.
//!
//! Bandwidth is divided by Nyquist, which is what `AUDIO_FEATURES.md` open item
//! 1 already proposes for it ("a plausible one is dividing bandwidth by Nyquist
//! as well") and is exactly the treatment centroid already gets — so the two
//! frequency-shaped columns end up on the same footing. Onset is divided by the
//! corpus 95th percentile and clipped, which puts a normal transient around 0.3
//! and a hard one at the top without a compressor curve nobody can reason about.

use crate::digest::{Frame, RepeatState, Simulated, Source, FEATURES, RATE};

/// The 95th percentile of the onset column across the corpus (11.23, rounded
/// up). Above this is the loudest 5% of transients and clips to 1.0, which for
/// a visual is the right answer: harder than "as hard as it gets" is still
/// as hard as it gets.
#[cfg(feature = "listen")]
const ONSET_FULL_SCALE: f32 = 12.0;

/// Where the app's digest rows come from — the stand-in, or a real device.
///
/// `Simulated` is kept rather than deleted, which `digest.rs` anticipated it
/// would not be. Two reasons, both practical: `drive.rs` has a dozen tests that
/// need a *deterministic* row for a given instant, and a venue laptop whose
/// interface has not been plugged in yet needs something to draw. It is no
/// longer the default when a real input is available, which was the actual
/// complaint.
/// The `POST /io {input}` value that selects the simulated digest.
pub const STAND_IN: &str = "stand-in";

pub enum Input {
    Simulated(Simulated),
    #[cfg(feature = "listen")]
    Live(Live),
}

impl Input {
    /// The deliberate stand-in: the `stand-in` entry of an `audio.live`'s
    /// source combo, and the input tests drive. Never a silent fallback for
    /// a device that will not open (that is a zero row and a red node —
    /// `crate::listeners`).
    pub fn stand_in() -> Input {
        Input::Simulated(Simulated::new(0x5EED))
    }

    /// Open a real device. `pick` is what `devices::inputs()` calls an input —
    /// a PulseAudio source name.
    #[cfg(feature = "listen")]
    pub fn live(pick: Option<&str>) -> Result<Input, String> {
        Live::open(pick).map(Input::Live)
    }

    #[cfg(not(feature = "listen"))]
    pub fn live(_pick: Option<&str>) -> Result<Input, String> {
        Err("this build has no capture — rebuild with the `listen` feature".into())
    }

    fn as_source(&self) -> &dyn Source {
        match self {
            Input::Simulated(s) => s,
            #[cfg(feature = "listen")]
            Input::Live(l) => l,
        }
    }

    fn as_source_mut(&mut self) -> &mut dyn Source {
        match self {
            Input::Simulated(s) => s,
            #[cfg(feature = "listen")]
            Input::Live(l) => l,
        }
    }

    pub fn poll(&mut self, time: f32) -> Frame {
        self.as_source_mut().poll(time)
    }

    pub fn beats(&self, time: f32) -> Option<f32> {
        self.as_source().beats(time)
    }

    /// Whether the music is repeating a DJ's loop right now, and how long it
    /// is. Read fresh from the source every call — see why there is no
    /// `last_repeat` cell below.
    pub fn repeat(&self) -> Option<RepeatState> {
        self.as_source().repeat()
    }

    /// The beat index of the last heard "1" — see [`Source::downbeat`].
    pub fn downbeat(&self) -> Option<u32> {
        self.as_source().downbeat()
    }

    /// "Tap 1" on the input — see [`Source::tap_downbeat`].
    pub fn tap_downbeat(&mut self) -> bool {
        self.as_source_mut().tap_downbeat()
    }

    pub fn describe(&self) -> String {
        self.as_source().describe()
    }

    pub fn is_real(&self) -> bool {
        self.as_source().is_real()
    }

    /// Samples the capture dropped since it opened; always 0 for the stand-in.
    pub fn dropped(&self) -> u64 {
        match self {
            Input::Simulated(_) => 0,
            #[cfg(feature = "listen")]
            Input::Live(l) => l.inner.dropped(),
        }
    }

    /// The tempo. A control on the stand-in; an *observation* on a real input,
    /// which is why the page has to know which it is looking at rather than
    /// offering a slider that would do nothing.
    pub fn bpm(&self) -> f32 {
        match self {
            Input::Simulated(s) => s.bpm,
            #[cfg(feature = "listen")]
            Input::Live(l) => l.inner.bpm().unwrap_or(0.0),
        }
    }

    pub fn set_bpm(&mut self, v: f32) {
        match self {
            Input::Simulated(s) => s.bpm = v.clamp(60.0, 200.0),
            // Not an error and not silent-by-accident: you cannot ask a room to
            // play at a different tempo. The page greys the control instead.
            #[cfg(feature = "listen")]
            Input::Live(_) => {}
        }
    }

    /// How hard the stand-in drives, 0..1. A real input answers with how hard
    /// the *music* is driving, which is the nearest true thing.
    pub fn drive(&self) -> f32 {
        match self {
            Input::Simulated(s) => s.drive,
            #[cfg(feature = "listen")]
            Input::Live(l) => l.last_level.get(),
        }
    }

    pub fn set_drive(&mut self, v: f32) {
        match self {
            Input::Simulated(s) => s.drive = v.clamp(0.0, 1.0),
            #[cfg(feature = "listen")]
            Input::Live(_) => {}
        }
    }

    /// Whether the tempo and level readouts are things to be set or things to
    /// be read. The page needs this to decide what to grey out.
    pub fn controllable(&self) -> bool {
        matches!(self, Input::Simulated(_))
    }

    /// How confident the beat clock is, 0..1. Always 0 for the stand-in, whose
    /// beat is not an estimate.
    pub fn confidence(&self) -> f32 {
        match self {
            Input::Simulated(_) => 0.0,
            #[cfg(feature = "listen")]
            Input::Live(l) => l.inner.confidence(),
        }
    }

    /// Whether this input's listener actually loaded the Basic Pitch model —
    /// what `GET /listen/status` reports as `tone.loaded` (the tone design notes D9). The
    /// stand-in runs no model, so it is always false there; a real listener
    /// answers from its worker's last published analysis, one mutex read, and
    /// stays false with the `tone` feature off or the model absent.
    pub fn tone_loaded(&self) -> bool {
        match self {
            Input::Simulated(_) => false,
            #[cfg(feature = "listen")]
            Input::Live(l) => l.inner.tone_loaded(),
        }
    }

    /// This input's last Basic Pitch inference wall time in ms — the
    /// `tone.latency_ms` of the same status (0.0 for the stand-in and before
    /// the first inference). Same cheap read as [`Input::tone_loaded`].
    pub fn tone_latency_ms(&self) -> f32 {
        match self {
            Input::Simulated(_) => 0.0,
            #[cfg(feature = "listen")]
            Input::Live(l) => l.inner.tone_latency_ms(),
        }
    }
}

#[cfg(feature = "listen")]
pub struct Live {
    inner: metal_listen::capture::Listener,
    /// The beat count last handed out. `Source::beats` must be monotonic —
    /// the Router switches on it, and a count that went backwards would send
    /// it back to the animation it just left.
    last_beats: std::cell::Cell<f32>,
    last_level: std::cell::Cell<f32>,
    /// The row before the one showing now, and the row showing now, and the
    /// wall-clock instant the current one first appeared. Together these are
    /// what lets `poll` interpolate: without the previous row there is nothing
    /// to come *from*, and without the turnover instant there is no phase.
    prev: [f32; FEATURES],
    cur: [f32; FEATURES],
    turned: f32,
    /// Whether a row has ever arrived. The first one must not be interpolated
    /// from a row of zeros — that is a 125 ms fade-in from black on every
    /// device open, which reads as a bug in the renderer.
    seen: bool,
}

#[cfg(feature = "listen")]
impl Live {
    pub fn open(pick: Option<&str>) -> Result<Live, String> {
        Ok(Live {
            inner: metal_listen::capture::Listener::open(pick)?,
            last_beats: std::cell::Cell::new(0.0),
            last_level: std::cell::Cell::new(0.0),
            prev: [0.0; FEATURES],
            cur: [0.0; FEATURES],
            turned: 0.0,
            seen: false,
        })
    }
}

#[cfg(feature = "listen")]
impl Source for Live {
    fn poll(&mut self, time: f32) -> Frame {
        // Reading the device is not a function of `time` — the device is
        // already a function of real time, and polling faster than 8 Hz returns
        // the same row, which is what the contract asks for.
        //
        // **Interpolating it is a function of `time`, and that is why this
        // takes one again.** The held row is the staircase: the digest steps
        // every 125 ms, every float downstream steps with it, and drawing that
        // at 60/s draws the same step sixty times rather than removing it. So
        // the row is carried in both forms — stepped and interpolated — and
        // each float picks. See `Frame` and `drive::smooth_default`.
        let row = self.inner.poll();
        let mut f = row;
        f[1] = (f[1] / (metal_listen::SR as f32 / 2.0)).clamp(0.0, 1.0);
        f[2] = (f[2] / ONSET_FULL_SCALE).clamp(0.0, 1.0);
        self.last_level.set(f[0].clamp(0.0, 1.0));

        // A new row is one that differs from the one held. Exact comparison is
        // right here and a tolerance would be wrong: the listener hands back
        // the *identical* row until it has a new one, so anything that differs
        // at all is a turnover, and a tolerance would swallow a genuinely
        // quiet passage into one long frozen row.
        if !self.seen {
            self.prev = f;
            self.cur = f;
            self.turned = time;
            self.seen = true;
        } else if f != self.cur {
            self.prev = self.cur;
            self.cur = f;
            self.turned = time;
        }

        // Where we are through the current row, 0 at its arrival and 1 by the
        // time the next is due. Clamped, so a row that overstays — a stalled
        // device, a laptop that slept — parks at the current value rather than
        // extrapolating away from it.
        let phase = ((time - self.turned) * RATE).clamp(0.0, 1.0);
        let mut lerped = [0.0f32; FEATURES];
        for i in 0..FEATURES {
            lerped[i] = self.prev[i] + (self.cur[i] - self.prev[i]) * phase;
        }
        Frame {
            f: lerped,
            raw: self.cur,
            rate: metal_schema::TickRate::Base,
            // metal-listen digests the voice block, so a live row always carries
            // one — even a silent passage, which is what makes the face idle.
            voice: true,
        }
    }

    fn beats(&self, _time: f32) -> Option<f32> {
        let b = self.inner.beats()?;
        let guarded = b.max(self.last_beats.get());
        self.last_beats.set(guarded);
        Some(guarded)
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }

    /// Read straight off the listener's analysis thread, and *not* cached here
    /// the way `beats` caches into `last_beats`.
    ///
    /// The reasons are opposite. `beats` is cached because it must be monotonic
    /// and the raw count can jitter down; a repeat has no such requirement —
    /// `Source::repeat` is explicitly allowed to flap as the detector's score
    /// crosses its threshold, and Phase 3's Looper owns the hysteresis. The
    /// listener already holds the value (`metal_listen::capture::Listener::repeat`
    /// reads its `Shared`/`Analysis`), so a `last_repeat` cell here would be a
    /// second copy of state that already has one home.
    ///
    /// The two types carry identical fields on purpose; this is a field-for-
    /// field copy and cannot silently drop one.
    fn repeat(&self) -> Option<crate::digest::RepeatState> {
        self.inner.repeat().map(|r| crate::digest::RepeatState {
            length_beats: r.length_beats,
            since_beat: r.since_beat,
            confidence: r.confidence,
        })
    }

    /// The detector's phase (beat mod 4 of the `beats` count). The clock
    /// only needs which residue is the "1", so the phase is the index.
    fn downbeat(&self) -> Option<u32> {
        self.inner.downbeat().map(u32::from)
    }

    fn tap_downbeat(&mut self) -> bool {
        if self.inner.beats().is_none() {
            return false;
        }
        self.inner.tap_downbeat();
        true
    }

    fn is_real(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stand_in_still_answers_every_control() {
        let mut input = Input::stand_in();
        assert!(!input.is_real());
        assert!(input.controllable());
        input.set_bpm(128.0);
        assert_eq!(input.bpm(), 128.0);
        input.set_drive(0.5);
        assert_eq!(input.drive(), 0.5);
        // Out of range is clamped rather than accepted, the same as it was
        // when the page wrote these fields directly.
        input.set_bpm(9000.0);
        assert_eq!(input.bpm(), 200.0);
        input.set_drive(-1.0);
        assert_eq!(input.drive(), 0.0);
    }

    #[test]
    fn a_simulated_row_is_still_inside_zero_to_one() {
        // The guarantee the whole suite is built on. If this ever fails the
        // renderers are being driven with numbers they were never tuned for —
        // which is exactly the thing the real digester would have done without
        // the scaling in this module.
        let mut input = Input::stand_in();
        for i in 0..400 {
            let frame = input.poll(i as f32 * metal_schema::TickRate::Base.seconds_per_tick() as f32);
            for (c, &v) in frame.f.iter().enumerate() {
                assert!(
                    (0.0..=1.0).contains(&v),
                    "column {c} left the unit range at {v}"
                );
            }
        }
    }

    #[test]
    fn the_stand_in_reports_no_repeat_by_default() {
        // `Source`'s default is `None`, and the stand-in sets no loop, so an
        // operator opening a fresh input is told the truth: nothing is
        // repeating. `Some` here would be a loop nobody is playing.
        assert_eq!(Input::stand_in().repeat(), None);
    }

    #[test]
    fn a_simulated_loop_is_visible_through_input() {
        // The stand-in's loop must reach the app through the same `Input`
        // method a real one does, or the Looper would work on a device and
        // silently do nothing on the stand-in every test is written against.
        let input = Input::Simulated(Simulated::new(0x5EED).with_loop(Some(8)));
        let state = input.repeat().expect("a set loop must be visible");
        assert_eq!(state.length_beats, 8);
    }

    #[cfg(feature = "listen")]
    #[test]
    fn the_corpus_percentiles_this_module_scales_by_are_the_ones_documented() {
        // A guard on the table in the module docs, not on any audio: if
        // somebody retunes ONSET_FULL_SCALE they should have to come here and
        // say why.
        assert_eq!(ONSET_FULL_SCALE, 12.0);
        // Nyquist for the corpus rate. Bandwidth is scaled by this, so a change
        // in metal-listen::SR would silently rescale a column.
        assert_eq!(metal_listen::SR, 22_050);
    }
}
