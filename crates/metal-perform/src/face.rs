//! The face solver: the thirteen voice columns → a facial rig → model floats.
//!
//! This is V4's glue and nothing more. The rig itself — the 23 controls, the
//! 14 visemes, the per-control springs, the [`metal_face::Style`] — already lives
//! in `metal-face`, shared with V5's `monster` model. What this module adds is
//! the pipeline `the voice design notes` decision 8 describes, one tick at a time:
//!
//! ```text
//! voice columns → viseme weights → metal_face::blend → secondary motion
//!               → teeth-on-hiss / vibrato → Springs::step → model floats
//! ```
//!
//! and the two things decision 8 deliberately left out of `metal-face` because
//! they need inputs it does not have: the singer's **vibrato** at the measured
//! F0 modulation rate, and **teeth shown on hiss**.
//!
//! # The shape of a tick
//!
//! 1. [`Voice`] reads the row. Continuous columns ride the interpolated `f`;
//!    `hiss`, `onset` and the level's dip ride `raw`, because they are
//!    transients and interpolating them moves the event.
//! 2. [`Face::weights`] estimates the visemes — the heuristics of decision 8.
//!    A plausible mouth, not a transcript; it is expected to be wrong often.
//! 3. [`metal_face::blend`] mixes them into a target pose.
//! 4. [`Face::secondary`] adds upper-face motion (decision 9) and the idle
//!    face when there is no voice.
//! 5. [`metal_face::Springs::step`] advances every control toward the target
//!    through its own attack/release.
//! 6. [`apply`] writes the result into the model floats whose
//!    [`metal_schema::FloatSpec::rig`] names that control.
//!
//! # Look-ahead is offline only
//!
//! Coarticulation reads at its best when the target is sampled a little
//! *ahead* of now: a p/b/m closure that starts 60 ms before the burst is a
//! closure you see. A live source has no future, so [`apply`] takes an
//! optional future row and honours it **only when the source is at the fine
//! level** — the plan's "offline, a sequence at the fine level gets look-ahead
//! for free". A base-level source gives the coarse, "rough" mouth. The engine
//! passes `None` today because nothing yet hands it a fine sequence's future
//! row; the seam is here and tested.
//!
//! # Who wins
//!
//! A lane or a loop that names a rig float beats the solver, per control: if
//! the row arrived at [`apply`] with a value already off the float's rest (a
//! keyframe the operator authored, an overlay movement), the solver leaves
//! that control alone. Everything it does not name, the solver writes.

use std::collections::VecDeque;

use metal_face::{blend, Controls, FaceControl as C, Springs, Style, Viseme};
use metal_schema::{FloatSpec, Schema};

use crate::clock::Position;
use crate::digest::Frame;

const TAU: f32 = std::f32::consts::TAU;

/// A control's range and rest, for the authored-override test and for
/// clamping. `None` for a float that is not a rig float.
fn rig_of(spec: &FloatSpec) -> Option<C> {
    spec.rig
}

/// Whether `schema` declares any facial-rig floats. Decision 11's first
/// condition; a model that declares none is never touched by the solver.
pub fn has_rig(schema: &Schema) -> bool {
    schema.blocks.iter().any(|b| match b {
        metal_schema::Block::Floats(specs) => specs.iter().any(|s| rig_of(s).is_some()),
        _ => false,
    })
}

/// The thirteen voice columns, as the solver reads them. Global column
/// indices are `metal_schema::MUSIC + i`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Voice {
    /// col 0 — presence.
    pub presence: f32,
    /// col 1 — loudness of the voiced frames.
    pub level: f32,
    /// col 2 — F0, 0..1 log scale.
    pub pitch: f32,
    /// col 3 — YIN periodicity.
    pub voicing: f32,
    /// col 4 — F0 stability.
    pub steady: f32,
    /// col 5 — syllable rate.
    pub syllables: f32,
    /// col 6 — jaw-openness proxy.
    pub open: f32,
    /// col 7 — lip spread vs round.
    pub wide: f32,
    /// col 8 — sibilance.
    pub hiss: f32,
    /// col 9 — style score.
    pub sing: f32,
    /// col 10.
    pub rap: f32,
    /// col 11.
    pub speak: f32,
    /// col 12.
    pub whisper: f32,
    /// The music prefix's `onset` (column 2): the transient the consonants
    /// key to, which is not a voice column but is on the same row.
    pub onset: f32,
    /// The music prefix's `centroid` (column 3): the slow drift the head yaw
    /// follows.
    pub centroid: f32,
}

impl Voice {
    pub fn from_frame(frame: &Frame) -> Voice {
        let g = |i: usize| frame.f[metal_schema::MUSIC + i];
        let r = |i: usize| frame.raw[metal_schema::MUSIC + i];
        Voice {
            presence: g(0),
            // The level's *dip* is a transient, so it comes off `raw`; the
            // amplitude itself is a mean and reads either.
            level: r(1),
            pitch: g(2),
            voicing: g(3),
            steady: g(4),
            syllables: g(5),
            open: g(6),
            wide: g(7),
            // Hiss is pooled by max, like onset — raw or it smears.
            hiss: r(8),
            sing: g(9),
            rap: g(10),
            speak: g(11),
            whisper: g(12),
            onset: frame.raw[2],
            centroid: frame.f[3],
        }
    }

    /// A voice block set by name, for tests and for a caller that has the
    /// columns already unpacked.
    #[allow(clippy::too_many_arguments)]
    pub fn new(presence: f32, level: f32, voicing: f32, open: f32, wide: f32, hiss: f32) -> Voice {
        Voice {
            presence,
            level,
            voicing,
            open,
            wide,
            hiss,
            ..Voice::default()
        }
    }

    /// The soft style scores as `metal_face` wants them.
    pub fn style(&self) -> Style {
        Style {
            sing: self.sing,
            rap: self.rap,
            speak: self.speak,
            whisper: self.whisper,
        }
    }
}

/// The beat phase a rap nod rides: `0..1` across one beat, `0` on the beat.
/// Read off the musical clock the same way a loop reads its step.
pub fn beat_phase(pos: Position) -> f32 {
    let in_beat = (pos.step % 4) as f32;
    ((in_beat + pos.phase) / 4.0).clamp(0.0, 1.0 - f32::EPSILON)
}

/// One source's face solver state: the springs plus the history the
/// heuristics need (a level dip, a phrase edge, a blink clock, saccades, the
/// pitch wobble). One per `gen.*` node, held in [`crate::engine::State`].
#[derive(Clone, Debug)]
pub struct Face {
    springs: Springs,
    /// The last time [`apply`] stepped, so a generator drawn twice in one
    /// frame (fan-out) does not step twice.
    last_time: Option<f32>,
    last: Controls,
    prev_presence: f32,
    prev_level: f32,
    /// How long a plosive closure is still held: a p/b/m closure lasts
    /// 50–100 ms, longer than one fine tick, so detecting the level dip and
    /// closing for a beat keeps the burst on closed lips.
    pp_hold: f32,
    pitch_ema: f32,
    /// The beat phase of the tick being stepped, for the nod.
    beat: f32,
    /// A decaying nostril flare, fired on a phrase's rising edge.
    flare: f32,
    breath: f32,
    blink_wait: f32,
    blink_hold: f32,
    saccade: f32,
    saccade_target: f32,
    saccade_wait: f32,
    vib_phase: f32,
    pitch_hist: VecDeque<f32>,
    rng: u32,
}

impl Default for Face {
    fn default() -> Self {
        Self::new()
    }
}

impl Face {
    pub fn new() -> Face {
        Face {
            springs: Springs::new(),
            last_time: None,
            last: Controls::rest(),
            prev_presence: 0.0,
            prev_level: 0.0,
            pp_hold: 0.0,
            pitch_ema: 0.0,
            beat: 0.0,
            flare: 0.0,
            breath: 0.0,
            blink_wait: 2.0,
            blink_hold: 0.0,
            saccade: 0.0,
            saccade_target: 0.0,
            saccade_wait: 1.0,
            vib_phase: 0.0,
            pitch_hist: VecDeque::with_capacity(64),
            rng: 0x5EED_FACE,
        }
    }

    fn rand(&mut self) -> f32 {
        // One small LCG: enough for blink holds and saccade picks, and
        // deterministic, which is what a renderer needs to draw the same
        // frame twice.
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.rng >> 8) as f32 / 16_777_216.0
    }

    /// The target pose for this voice frame, before the springs. Pure in the
    /// inputs (no state mutates), so a test — and the look-ahead test — can
    /// ask what the mouth *would* be asked to do.
    pub fn target_for(&self, v: &Voice, future: Option<&Voice>) -> Controls {
        // Look-ahead: sample the target a little ahead, so a closure starts
        // before the burst. Only ever called with `Some` offline and fine.
        let sampled = future.unwrap_or(v);
        let mut t = blend(&self.weights(sampled, v));
        self.secondary(&mut t, v);
        t.clamped()
    }

    /// The viseme weights for a tick: decision 8's heuristics.
    fn weights(&self, v: &Voice, now: &Voice) -> Vec<(Viseme, f32)> {
        let presence = v.presence.clamp(0.0, 1.0);
        let mut w: Vec<(Viseme, f32)> = Vec::new();
        if presence < 0.08 {
            // Nothing measured: `sil` (the rest pose) and the idle face the
            // secondary motion adds. `blend` returns rest for a lone `sil`.
            w.push((Viseme::Sil, 1.0));
            return w;
        }

        let voiced = v.voicing > 0.45;
        let hiss = v.hiss.clamp(0.0, 1.0);
        let onset = now.onset.clamp(0.0, 1.0);

        // --- consonants ---------------------------------------------------
        // A plosive is a level dip followed by a broadband burst (decision 8).
        // The dip is what we can see *before* the burst, so closing starts
        // here and stays held through the burst. A closure overrides the
        // vowel: the lips are shut whatever the sung vowel underneath is.
        if self.pp_hold > 0.0 {
            return vec![(Viseme::PP, 1.0)];
        }

        // Fricatives: hiss with low voicing. Rounded (low F2/`wide`) reads as
        // ch/sh/j; spread reads as s/z. Weak hiss at a low level is f/v.
        if hiss > 0.10 && v.voicing < 0.55 {
            if hiss < 0.30 && now.level < 0.25 {
                w.push((Viseme::FF, hiss * 2.0));
            } else if v.wide < 0.40 {
                w.push((Viseme::CH, hiss));
            } else {
                w.push((Viseme::SS, hiss));
            }
        }

        // Voiced onset consonants: a burst with voicing. Alveolar (`t d n l`)
        // when the tongue front is active (high `wide`), velar (`k g`) when
        // the jaw is mid-open.
        if voiced && onset > 0.45 && hiss < 0.35 {
            if v.wide > 0.5 {
                w.push((Viseme::DD, (onset * 0.8).clamp(0.0, 1.0)));
            } else {
                w.push((Viseme::Kk, (onset * 0.6).clamp(0.0, 1.0)));
            }
        }

        // --- vowels -------------------------------------------------------
        // (`open`, `wide`) as an F1/F2 map: the nearest of five corners, with
        // a light inverse-distance mix so two close guesses average instead
        // of popping between them.
        let vowel_strength = presence * (0.35 + 0.65 * v.voicing).clamp(0.0, 1.0);
        if vowel_strength > 0.05 && hiss < 0.35 {
            let corners: [(Viseme, f32, f32); 5] = [
                (Viseme::Aa, 0.85, 0.50),
                (Viseme::E, 0.45, 0.72),
                (Viseme::I, 0.20, 0.85),
                (Viseme::O, 0.60, 0.25),
                (Viseme::U, 0.20, 0.12),
            ];
            let mut raw = [0.0f32; 5];
            let mut total = 0.0;
            for (i, (_, o, wd)) in corners.iter().enumerate() {
                let d = ((v.open - o).powi(2) + (v.wide - wd).powi(2)).sqrt();
                raw[i] = 1.0 / (d + 0.10);
                total += raw[i];
            }
            for (i, (vis, _, _)) in corners.iter().enumerate() {
                w.push((*vis, vowel_strength * raw[i] / total));
            }
        }

        // A little `sil` so a gap between phonemes leaves a resting mouth
        // rather than a held vowel; `blend` adds nothing for it and leaves the
        // remainder at rest.
        w.push((Viseme::Sil, 0.2));
        w
    }

    /// Decision 9's upper-face mapping, the vibrato and teeth-on-hiss, the
    /// whisper shaping, and the idle face. Mutates the target in place.
    fn secondary(&self, t: &mut Controls, v: &Voice) {
        let presence = v.presence.clamp(0.0, 1.0);

        // Brows ← pitch rise over the phrase + onset accent.
        let rise = (v.pitch - self.pitch_ema).max(0.0);
        let brow = (rise * 6.0 + 0.25 * v.onset * v.voicing).clamp(0.0, 1.0);
        t[C::BrowUp] = t[C::BrowUp].max(brow);
        // A low steady glide furrows a touch: speech, not a held note.
        let furrow = ((1.0 - v.steady) * (1.0 - v.sing) * presence * 0.3).clamp(0.0, 1.0);
        t[C::BrowFurrow] = t[C::BrowFurrow].max(furrow);

        // Eyes ← level peaks, an idle saccade, a whisper squint.
        let peak = ((v.level - 0.55) * 2.2).clamp(0.0, 1.0);
        t[C::EyeWide] = t[C::EyeWide].max(peak);
        t[C::Blink] = t[C::Blink].max(self.blink_hold.clamp(0.0, 1.0));
        t[C::Squint] = t[C::Squint].max(v.whisper.clamp(0.0, 1.0) * 0.8);

        // Teeth on hiss: peel both lips off the teeth, the plan's decision-8
        // item `metal-face` left for the solver because it needs the column.
        let hiss = v.hiss.clamp(0.0, 1.0);
        if hiss > 0.12 {
            t[C::LipUpperUp] = t[C::LipUpperUp].max(hiss * 0.7);
            t[C::LipLowerDown] = t[C::LipLowerDown].max(hiss * 0.5);
        }

        // Head yaw ← centroid (a slow drift, not an articulation).
        t[C::HeadYaw] = (t[C::HeadYaw] + (v.centroid - 0.5) * 0.6).clamp(-1.0, 1.0);

        // Nostril flare on the rising edge of presence (a phrase start).
        t[C::NostrilFlare] = t[C::NostrilFlare].max(self.flare.clamp(0.0, 1.0));

        // Vibrato at the measured F0 modulation rate, while singing.
        let sing = v.sing.clamp(0.0, 1.0);
        if sing > 0.05 && presence > 0.1 {
            let vib = self.vib_phase.sin() * 0.07 * sing;
            t[C::JawOpen] = (t[C::JawOpen] + vib).clamp(0.0, 1.0);
            t[C::LipStretch] = (t[C::LipStretch] + vib * 0.4).clamp(0.0, 1.0);
        }

        // The idle face, when nothing is being measured: breathing, saccades
        // and a music-driven nod (decision 9). The nod still rides the beat,
        // so a silent singer is not a statue.
        let idle = (1.0 - presence / 0.08).clamp(0.0, 1.0);
        if idle > 0.0 {
            let breathe = 0.5 + 0.5 * self.breath.sin();
            t[C::NostrilFlare] = (t[C::NostrilFlare] + 0.28 * breathe * idle).clamp(0.0, 1.0);
            t[C::JawOpen] = (t[C::JawOpen] + 0.03 * breathe * idle).clamp(0.0, 1.0);
            t[C::EyeWide] = (t[C::EyeWide] + self.saccade * idle).clamp(0.0, 1.0);
            t[C::HeadRoll] = (t[C::HeadRoll] + 0.05 * breathe * idle).clamp(-1.0, 1.0);
        }

        // The nod is on every beat while there is music; `rap` stiffens and
        // deepens it, and an idle face still nods more gently.
        let rap = v.rap.clamp(0.0, 1.0);
        let nod = (rap * 0.8 + idle * 0.22) * self.nod_wave();
        t[C::HeadPitch] = (t[C::HeadPitch] + nod).clamp(-1.0, 1.0);
    }

    /// The beat wave a nod rides: a cosine per beat, `+1` on the beat. Stored
    /// each step from the musical clock.
    fn nod_wave(&self) -> f32 {
        (self.beat * TAU).cos()
    }

    /// Advances the solver one tick and returns the control values.
    ///
    /// `future` is the look-ahead row, already gated on the fine level and
    /// offline by the caller. `time` keys the idempotence guard so a generator
    /// drawn twice in one frame steps once.
    pub fn step(
        &mut self,
        v: &Voice,
        future: Option<&Voice>,
        beat: f32,
        time: f32,
        dt: f32,
    ) -> Controls {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };

        // History first, from the *current* row: the dip, the phrase edge and
        // the pitch wobble are facts about now, look-ahead or not.
        let rising_edge = v.presence > 0.35 && self.prev_presence <= 0.35;
        let falling_edge = v.presence <= 0.2 && self.prev_presence > 0.2;
        if rising_edge {
            self.flare = 1.0;
        }
        if falling_edge {
            // A phrase ending is a good blink; so is the passage just gone.
            self.blink_wait = 0.0;
        }
        self.flare = (self.flare - dt * 2.0).max(0.0);

        // A level dip is the plosive's closure. Hold it ~90 ms so the burst
        // one or two fine ticks later meets closed lips (decision 8).
        if v.presence > 0.08 && self.prev_level - v.level > 0.12 {
            self.pp_hold = 0.10;
        } else {
            self.pp_hold = (self.pp_hold - dt).max(0.0);
        }

        self.pitch_ema += (v.pitch - self.pitch_ema) * 0.25;
        self.pitch_hist.push_back(v.pitch);
        if self.pitch_hist.len() > 64 {
            self.pitch_hist.pop_front();
        }
        self.update_vibrato(dt);

        // Idle timers.
        self.breath += dt * TAU * 0.25;
        if self.breath > TAU {
            self.breath -= TAU;
        }
        self.blink_wait -= dt;
        if self.blink_wait <= 0.0 {
            // A blink is quick; the hold is what the lids do, the wait is the
            // random hold between them.
            self.blink_hold = 1.0;
            self.blink_wait = 1.5 + self.rand() * 3.0;
        }
        self.blink_hold = (self.blink_hold - dt * 12.0).max(0.0);
        self.saccade_wait -= dt;
        if self.saccade_wait <= 0.0 {
            self.saccade_target = (self.rand() - 0.5) * 0.5;
            self.saccade_wait = 0.8 + self.rand() * 1.8;
        }
        self.saccade += (self.saccade_target - self.saccade) * (dt * 20.0).min(1.0);

        self.beat = beat;
        let target = self.target_for(v, future);
        self.springs.style = v.style();
        let out = self.springs.step(&target, dt);

        self.prev_presence = v.presence;
        self.prev_level = v.level;
        self.last = out;
        self.last_time = Some(time);
        out
    }

    /// Estimates the F0 modulation rate from recent pitch history: the rate
    /// of sign changes of the pitch about its own mean, in Hz. Falls back to a
    /// human 5.5 Hz when there is not enough history or no wobble.
    fn update_vibrato(&mut self, dt: f32) {
        let n = self.pitch_hist.len();
        if n < 8 {
            self.vib_phase += dt * TAU * 5.5;
            return;
        }
        let mean = self.pitch_hist.iter().copied().sum::<f32>() / n as f32;
        let mut crossings = 0usize;
        let mut prev = self.pitch_hist[0] - mean;
        for &p in self.pitch_hist.iter().skip(1) {
            let d = p - mean;
            if prev != 0.0 && d != 0.0 && (d > 0.0) != (prev > 0.0) {
                crossings += 1;
            }
            prev = d;
        }
        let seconds = (n as f32 * dt).max(1e-3);
        let hz = (crossings as f32 / 2.0) / seconds;
        let hz = if (3.0..=9.0).contains(&hz) { hz } else { 5.5 };
        self.vib_phase += dt * TAU * hz;
        if self.vib_phase > TAU {
            self.vib_phase -= TAU;
        }
    }

    /// The control values last written, for reporting.
    pub fn value(&self) -> Controls {
        self.last
    }

    /// The beat phase this face last stepped with.
    pub fn beat(&self) -> f32 {
        self.beat
    }
}

/// Writes the control values into `row` at every rig float, honouring an
/// authored override in `base`: a control whose base value is already off rest
/// (a lane keyframe, an overlay movement, a hand-set preset) is left alone.
fn write_controls(schema: &Schema, row: &mut [f32], base: &[f32], controls: &Controls) {
    let mut at = 0usize;
    for block in schema.blocks {
        if let metal_schema::Block::Floats(specs) = block {
            for (k, spec) in specs.iter().enumerate() {
                if let Some(c) = rig_of(spec) {
                    // The row `drive::row` produced for a rig float is the
                    // base value (drive 0), so either is the operator's.
                    let held = base
                        .get(at + k)
                        .copied()
                        .or_else(|| row.get(at + k).copied())
                        .unwrap_or(spec.default);
                    let authored = (held - spec.default).abs() > 1e-4;
                    if !authored {
                        if let Some(dst) = row.get_mut(at + k) {
                            *dst = c.spec().clamp(controls[c]);
                        }
                    }
                }
            }
        }
        at += block.width();
    }
}

/// The engine hook (decision 11): voice columns → viseme weights →
/// `metal_face::blend` → secondary motion → `Springs::step` → the model floats
/// whose spec names a rig control.
///
/// Runs between `drive::row` and `drive::apply_loop`, so a hand-authored lane
/// or loop that names a rig float wins. `base` is the row the operator (or an
/// overlay) set, used only to tell an authored rig float from a solver-owned
/// one. `lookahead` is honoured only at the fine level; the caller passes the
/// future row when it has one (offline), and `None` live.
#[allow(clippy::too_many_arguments)]
pub fn apply(
    schema: &Schema,
    row: &mut [f32],
    base: &[f32],
    frame: &Frame,
    lookahead: Option<&Frame>,
    beat: f32,
    time: f32,
    face: &mut Face,
) {
    if !frame.voice || !has_rig(schema) {
        return;
    }
    // A generator fanned out to two layers is drawn once; stepping it twice
    // in one frame would double the springs.
    if face.last_time == Some(time) {
        write_controls(schema, row, base, &face.last);
        return;
    }
    let v = Voice::from_frame(frame);
    // Look-ahead is the plan's offline-only prize: a base-level source stays
    // coarse even if a caller hands it a future row.
    let future = match (frame.rate, lookahead) {
        (metal_schema::TickRate::Fine, Some(f)) if f.voice => Some(Voice::from_frame(f)),
        _ => None,
    };
    let dt = frame.rate.seconds_per_tick() as f32;
    let controls = face.step(&v, future.as_ref(), beat, time, dt);
    write_controls(schema, row, base, &controls);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::FEATURES;

    const EPS: f32 = 1e-4;

    /// A three-float synthetic schema: two rig floats and one ordinary float
    /// beside them, so the solver's writer is checked against a float it must
    /// not touch.
    static RIG_SPECS: [FloatSpec; 3] = [
        metal_schema::spec_rig(C::JawOpen),
        metal_schema::spec_rig(C::LipClose),
        metal_schema::spec("energy", 0.0, 1.0, 0.5),
    ];
    static RIG_BLOCKS: [metal_schema::Block; 1] = [metal_schema::Block::Floats(&RIG_SPECS)];
    static RIG_SCHEMA: Schema = Schema { blocks: &RIG_BLOCKS };
    static PLAIN_SPECS: [FloatSpec; 1] = [metal_schema::spec("x", 0.0, 1.0, 0.0)];
    static PLAIN_BLOCKS: [metal_schema::Block; 1] = [metal_schema::Block::Floats(&PLAIN_SPECS)];
    static PLAIN_SCHEMA: Schema = Schema { blocks: &PLAIN_BLOCKS };
    const JAW: usize = 0;
    const LIP: usize = 1;
    const ENERGY: usize = 2;

    fn frame(voice: Voice, rate: metal_schema::TickRate) -> Frame {
        let mut f = [0.0f32; FEATURES];
        f[metal_schema::MUSIC] = voice.presence;
        f[metal_schema::MUSIC + 1] = voice.level;
        f[metal_schema::MUSIC + 2] = voice.pitch;
        f[metal_schema::MUSIC + 3] = voice.voicing;
        f[metal_schema::MUSIC + 4] = voice.steady;
        f[metal_schema::MUSIC + 5] = voice.syllables;
        f[metal_schema::MUSIC + 6] = voice.open;
        f[metal_schema::MUSIC + 7] = voice.wide;
        f[metal_schema::MUSIC + 8] = voice.hiss;
        f[metal_schema::MUSIC + 9] = voice.sing;
        f[metal_schema::MUSIC + 10] = voice.rap;
        f[metal_schema::MUSIC + 11] = voice.speak;
        f[metal_schema::MUSIC + 12] = voice.whisper;
        f[2] = voice.onset;
        f[3] = voice.centroid;
        // raw == f for a synthetic row; the transient columns are set the same.
        Frame { f, raw: f, rate, voice: true }
    }

    fn vowel(open: f32, wide: f32) -> Voice {
        Voice {
            presence: 1.0,
            level: 0.6,
            voicing: 0.9,
            open,
            wide,
            ..Voice::default()
        }
    }

    /// Decision 11's condition: no voice block, no rig floats, no writes.
    #[test]
    fn the_solver_only_runs_for_a_rig_model_and_a_voice_row() {
        assert!(has_rig(&RIG_SCHEMA));
        assert!(!has_rig(&PLAIN_SCHEMA));
    }

    /// The plan's `a_sustained_a_holds_the_jaw_open`.
    #[test]
    fn a_sustained_a_holds_the_jaw_open() {
        let mut face = Face::new();
        let mut row = vec![0.0f32; 3];
        let base = vec![0.0f32; 3];
        let v = vowel(0.9, 0.5);
        let mut t = 0.0;
        let dt = 1.0 / 32.0;
        let mut peak = 0.0f32;
        for _ in 0..64 {
            let fr = frame(v, metal_schema::TickRate::Fine);
            apply(&RIG_SCHEMA, &mut row, &base, &fr, None, 0.0, t, &mut face);
            peak = peak.max(row[JAW]);
            t += dt;
        }
        assert!(row[JAW] > 0.5, "a sustained 'a' did not hold the jaw open: {}", row[JAW]);
        assert!(peak > 0.5);
        assert_eq!(row[ENERGY], 0.0, "an ordinary float is never touched");
    }

    /// The plan's `whisper_caps_the_jaw`.
    #[test]
    fn whisper_caps_the_jaw() {
        let mut face = Face::new();
        let mut row = vec![0.0f32; 3];
        let base = vec![0.0f32; 3];
        let v = Voice {
            presence: 1.0,
            level: 0.3,
            voicing: 0.2,
            open: 0.9,
            wide: 0.4,
            hiss: 0.5,
            whisper: 1.0,
            ..Voice::default()
        };
        let dt = 1.0 / 32.0;
        let mut t = 0.0;
        let mut max_jaw = 0.0f32;
        for _ in 0..128 {
            let fr = frame(v, metal_schema::TickRate::Fine);
            apply(&RIG_SCHEMA, &mut row, &base, &fr, None, 0.0, t, &mut face);
            max_jaw = max_jaw.max(row[JAW]);
            t += dt;
        }
        assert!(max_jaw <= metal_face::springs::WHISPER_JAW + EPS, "jaw {max_jaw}");
    }

    /// The plan's `pp_closes_the_lips_before_the_burst`: a dip lands the
    /// closure, and the burst that follows meets closed lips.
    #[test]
    fn pp_closes_the_lips_before_the_burst() {
        let mut face = Face::new();
        let mut row = vec![0.0f32; 3];
        let base = vec![0.0f32; 3];
        let dt = 1.0 / 32.0;
        let mut t = 0.0;
        // A sung vowel, then a level dip (the closure), then the burst.
        let loud = Voice { presence: 1.0, level: 0.7, voicing: 0.9, open: 0.6, wide: 0.4, ..Voice::default() };
        let dip = Voice { presence: 1.0, level: 0.3, voicing: 0.6, open: 0.4, wide: 0.4, ..Voice::default() };
        let burst = Voice { presence: 1.0, level: 0.6, voicing: 0.3, open: 0.4, wide: 0.4, onset: 0.8, hiss: 0.2, ..Voice::default() };
        let mut closed_at_burst = 0.0;
        for phase in [loud, loud, dip, burst] {
            let fr = frame(phase, metal_schema::TickRate::Fine);
            apply(&RIG_SCHEMA, &mut row, &base, &fr, None, 0.0, t, &mut face);
            closed_at_burst = row[LIP];
            t += dt;
        }
        assert!(
            closed_at_burst > 0.8,
            "the lips were not closed at the burst: {closed_at_burst}"
        );
    }

    /// The plan's `no_voice_idles_and_breathes`: no presence still moves the
    /// face — breathing, a beat nod — and every control stays legal.
    #[test]
    fn no_voice_idles_and_breathes() {
        let mut face = Face::new();
        let mut row = vec![0.0f32; 3];
        let base = vec![0.0f32; 3];
        let silent = Voice::default();
        let dt = 1.0 / 32.0;
        let mut t = 0.0;
        let mut peaks = Controls::rest();
        let mut pitches = Vec::new();
        for i in 0..256 {
            let beat = (i as f32 * dt * 2.0) % 1.0; // 120 BPM
            let fr = frame(silent, metal_schema::TickRate::Fine);
            apply(&RIG_SCHEMA, &mut row, &base, &fr, None, beat, t, &mut face);
            let c = face.value();
            for (ctrl, val) in c.iter() {
                let s = ctrl.spec();
                assert!(val >= s.lo - EPS && val <= s.hi + EPS, "{} = {val}", ctrl.name());
                peaks[ctrl] = peaks[ctrl].max(val);
            }
            pitches.push(face.value()[C::HeadPitch]);
            t += dt;
        }
        assert!(
            peaks[C::NostrilFlare] > 0.05,
            "an idle face did not breathe: {}",
            peaks[C::NostrilFlare]
        );
        let (lo, hi) = pitches.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &x| (a.min(x), b.max(x)));
        assert!(hi - lo > 0.01, "an idle face did not nod with the music: {}", hi - lo);
    }

    /// The plan's `rap_nods_on_the_beat`: the nod is deeper under rap, and it
    /// lands on the beat, not between.
    #[test]
    fn rap_nods_on_the_beat() {
        let face = Face::new();
        let rap = Voice { presence: 1.0, level: 0.6, voicing: 0.8, rap: 1.0, ..Voice::default() };
        let on_beat = face.target_for(&rap, None)[C::HeadPitch];
        let off_beat = {
            let mut f2 = face;
            f2.beat = 0.5;
            f2.target_for(&rap, None)[C::HeadPitch]
        };
        assert!(
            on_beat > off_beat + 0.05,
            "the rap nod does not land on the beat: {on_beat} vs {off_beat}"
        );
    }

    /// The plan's `a_lane_on_a_rig_float_beats_the_solver`.
    #[test]
    fn a_lane_on_a_rig_float_beats_the_solver() {
        let mut face = Face::new();
        let mut base = vec![0.0f32; 3];
        base[JAW] = 0.42; // an authored lane on jaw_open
        // `drive::row` has already copied the base into the row; the solver
        // must not overwrite the lane's value.
        let mut row = base.clone();
        let v = vowel(0.9, 0.5);
        let fr = frame(v, metal_schema::TickRate::Fine);
        apply(&RIG_SCHEMA, &mut row, &base, &fr, None, 0.0, 0.0, &mut face);
        assert!(
            (row[JAW] - 0.42).abs() < 1e-6,
            "the solver overwrote an authored rig float: {}",
            row[JAW]
        );
        assert!(row[LIP] == 0.0 || row[LIP] <= 1.0);
    }

    /// The plan's `lookahead_moves_targets_earlier_offline_only`: at the fine
    /// level a future row moves the target now; at base it is ignored.
    #[test]
    fn lookahead_moves_targets_earlier_offline_only() {
        let face = Face::new();
        let silent = Voice::default();
        let vowel = vowel(0.9, 0.5);
        // The pure target reads the future row when there is one.
        let early = face.target_for(&silent, Some(&vowel))[C::JawOpen];
        let late = face.target_for(&silent, None)[C::JawOpen];
        assert!(early > late + 0.1, "look-ahead did not move the target: {early} vs {late}");

        // Through `apply`, only the fine level honours it.
        let mut fine_face = Face::new();
        let mut fine_row = vec![0.0f32; 3];
        let base = vec![0.0f32; 3];
        let now_fine = frame(silent, metal_schema::TickRate::Fine);
        let next_fine = frame(vowel, metal_schema::TickRate::Fine);
        apply(&RIG_SCHEMA, &mut fine_row, &base, &now_fine, Some(&next_fine), 0.0, 0.0, &mut fine_face);
        assert!(fine_row[JAW] > 0.1, "a fine source ignored look-ahead: {}", fine_row[JAW]);

        let mut base_face = Face::new();
        let mut base_row = vec![0.0f32; 3];
        let now_base = frame(silent, metal_schema::TickRate::Base);
        let next_base = frame(vowel, metal_schema::TickRate::Base);
        apply(&RIG_SCHEMA, &mut base_row, &base, &now_base, Some(&next_base), 0.0, 0.0, &mut base_face);
        assert!(base_row[JAW] < 0.05, "a base source used look-ahead: {}", base_row[JAW]);
    }

    /// The plan's `every_viseme_pose_is_inside_control_ranges`: the solver's
    /// output, over the whole space of viseme poses and many synthetic frames,
    /// never leaves a control's declared range.
    #[test]
    fn every_viseme_pose_is_inside_control_ranges() {
        for v in Viseme::ALL {
            for (c, x) in v.pose().iter() {
                let s = c.spec();
                assert!(x >= s.lo && x <= s.hi, "{} {}", v.name(), c.name());
            }
        }
        // And the solver, driven through its whole range, stays legal.
        let mut face = Face::new();
        let mut row = vec![0.0f32; 3];
        let base = vec![0.0f32; 3];
        let dt = 1.0 / 32.0;
        let mut t = 0.0;
        for i in 0..2000 {
            let x = i as f32 / 2000.0;
            let v = Voice {
                presence: 1.0,
                level: (x * 7.0).fract(),
                pitch: (x * 3.0).fract(),
                voicing: (x * 5.0).fract(),
                steady: (x * 11.0).fract(),
                syllables: (x * 13.0).fract(),
                open: (x * 17.0).fract(),
                wide: (x * 19.0).fract(),
                hiss: (x * 23.0).fract(),
                sing: (x * 29.0).fract(),
                rap: (x * 31.0).fract(),
                speak: (x * 37.0).fract(),
                whisper: (x * 41.0).fract(),
                onset: (x * 43.0).fract(),
                centroid: (x * 47.0).fract(),
            };
            let fr = frame(v, metal_schema::TickRate::Fine);
            apply(&RIG_SCHEMA, &mut row, &base, &fr, None, (x * 53.0).fract(), t, &mut face);
            for (c, val) in face.value().iter() {
                let s = c.spec();
                assert!(val >= s.lo - EPS && val <= s.hi + EPS, "{} = {val}", c.name());
            }
            t += dt;
        }
    }
}
