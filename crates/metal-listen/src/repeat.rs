//! **The loop detector.** One beat at a time, it decides whether what is
//! playing right now is a loop of the DJ's, and how long that loop is.
//!
//! A DJ builds a loop by re-playing the same bars over and over, and the room
//! hears it as a phrase that refuses to move on. The visuals should refuse to
//! move on with it, which is the whole point of this file: nothing downstream
//! can tell "the same bars again" from "a new part that happens to sound like
//! the last one" unless something measures similarity on the beat grid. That
//! measurement is all this is.
//!
//! # What a beat looks like
//!
//! Not a sixteenth column, for the same reason a beat is not one in
//! [`crate::tempo`]: the row is sixteen wide and shared with the corpus. A
//! [`BeatVector`] is built *from* the native frames between two beats — mean
//! chroma, an onset count in four sub-beat bins, and the mean rms and centroid
//! — and is dimensionless. The caller slices the frames by beat and calls
//! [`beat_vector`]; the pooling rule lives here so the live path and the tests
//! cannot disagree about it.
//!
//! # Similarity, not equality
//!
//! A loop is never bit-identical. The DJ may sweep a filter, the room changes,
//! the limiter breathes. So the question is not "is this the same array" but
//! "is this the same *shape*", and the shape is compared with the cosine of the
//! angle between two windows of bars:
//!
//! ```text
//!   window A = beats [n-L, n)      the cycle that just played
//!   window B = beats [n-2L, n-L)   the cycle before it
//! ```
//!
//! The windows are compared **beat by beat, in order**, not as their average.
//! A mean would throw away which beat came first, so a loop whose bars had been
//! shuffled would still score `1.0`; and because a periodic signal sums to zero
//! over a whole period, an exact repeat averaged to a flat vector and scored
//! `0.0` — the opposite of certain. Keeping the sequence compares the loop
//! against the loop, hit for hit. The cosine is scale-free, so a uniform gain —
//! exactly what a filter sweep is — does not change the angle, and it is
//! bounded, so [`ENTER`] and [`EXIT`] can be fixed numbers instead of something
//! fit per song.
//!
//! # The counters before the comparison
//!
//! Cosine on raw features would mostly measure loudness: a drop's chroma is
//! larger in every bin than a breakdown's, so two *different* drops would score
//! higher than the same riff at two volumes. Each beat therefore has the
//! running [`ZSCORE_BEATS`]-beat column mean removed, and is then scaled to
//! unit length. The subtraction takes out a trend over the phrase; the scaling
//! takes out a per-beat gain and keeps a near-silent column's noise from being
//! amplified into the comparison. Both happen inside [`Repeat`] because the
//! history lives there; [`beat_vector`] stays pure.
//!
//! # Locking
//!
//! The detector tries every [`LENGTHS`] each beat and takes the **shortest one
//! that passes**. A short loop is also a long one — a 4-beat loop repeats every
//! 4, 8, 16 and 32 beats too — so if the answer were allowed to be any passing
//! length it would always be 32. Shortest-wins is what makes the answer mean
//! "the period", not "some multiple of the period".
//!
//! One score already covers two whole cycles: it compares the last `L` beats
//! against the `L` before them. A score at `>= ENTER` therefore *is* the match
//! holding for `2 * L` beats, and waiting for `2 * L` further beats would lock
//! a bar late. Release is cheap by comparison: one beat below [`EXIT`] and the
//! lock drops. A loop that has truly ended should be let go immediately, while
//! a loop that is merely wobbling should not be torn down and rebuilt.
//!
//! # Tempo is the caller's job
//!
//! This struct has no clock. [`Repeat::push`] is called once per beat by the
//! caller, and the caller only calls it while [`crate::tempo::Tempo::beats`] is
//! `Some`. The absolute beat number arrives inside the [`RepeatState`] for the
//! one thing this cannot know: where in the night the loop started. A hidden
//! gate on the tempo here would just be a second opinion about when a beat is a
//! beat.

use crate::Native;

/// Width of a [`BeatVector`]: twelve chroma, four onset bins, rms, centroid.
pub const DIMS: usize = 18;

/// How many sub-beat bins the onset column is counted into.
///
/// Four is one per sixteenth of a 4/4 bar, coarse enough to be robust to the
/// tracker's phase wobble and fine enough to tell a kick pattern from a snare
/// pattern. More bins would turn a small timing error into a large feature
/// error.
const ONSET_BINS: usize = 4;

/// The candidate loop lengths, in beats. Shortest that passes wins.
///
/// Powers of two from a bar to eight bars. Thirty-two is eight bars, about the
/// longest repeat a dance track is usually built from.
///
/// **One and two beats are deliberately absent**, and that is a measured
/// decision, not a tidiness one (rule 11). Agent 1B ran this detector over the
/// 74-song corpus with `[1, 2, 4, 8, 16, 32]`: **37 of 40 songs "locked"**,
/// essentially every hit at L=1 (2.7% of all beats). A one-beat loop compares a
/// beat against the beat before it, and any four-on-the-floor track scores near
/// 1.0 at that lag — the degenerate minimum was doing the false-positive work,
/// not the threshold. Dropping to `[4, 8, 16, 32]` cut it to **2 of 74 songs**,
/// and both are house records that genuinely loop. A DJ loop shorter than one
/// bar is not what this feature is for, so the candidate set starts at a bar.
pub const LENGTHS: [u32; 4] = [4, 8, 16, 32];

/// How many beats of history are kept — a ring, so it never grows unbounded.
///
/// Two windows at the longest length need `2 * 32 = 64` beats, so this is
/// exactly enough to score [`LENGTHS`]' longest entry and no more.
pub const HISTORY_BEATS: usize = 64;

/// Lock when the cosine score reaches this. Every score compares two whole
/// cycles, so one at this level is two cycles of evidence.
///
/// A start value, to be MEASURED against real material by agent 1B. On
/// synthetic exact repeats the score is 1.0, so anything below 1.0 locks;
/// `0.97` leaves room for the filter sweep and the noise that a real loop
/// always carries.
pub const ENTER: f32 = 0.97;

/// Release when the score drops below this for one beat.
///
/// Below [`ENTER`] on purpose: the gap is hysteresis, so a score hovering at
/// the lock threshold does not flap the state every other beat.
pub const EXIT: f32 = 0.90;

/// Running window for the per-column mean removed before the cosine, in beats.
///
/// Sixteen beats is two bars of context: long enough that the mean is the local
/// character of the music rather than of one beat, short enough that it moves
/// when the music does. The name is kept from the plan's wording ("z-score");
/// what is actually subtracted is the mean, because dividing by the spread
/// would amplify a silent column's noise (see [`flat_centered_unit`]).
pub const ZSCORE_BEATS: usize = 16;

/// One beat's feature vector, built from the native STFT frames between two
/// consecutive Tempo beats. 18 dims: mean chroma (12) + onset in 4 sub-beat
/// bins + rms + centroid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeatVector {
    pub v: [f32; DIMS],
}

/// What the detector exposes when (and only when) it is locked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RepeatState {
    /// The detected L.
    pub length_beats: u32,
    /// Absolute Tempo beat count when the loop started.
    pub since_beat: f32,
    /// The score at lock, 0..1.
    pub confidence: f32,
}

/// Build a [`BeatVector`] from the native frames that fall inside one beat.
///
/// `frames` are the [`Native`] features in time order; the caller slices by
/// beat. Pure, no I/O — the pooling rule lives here so both the live path and
/// the tests use the same one.
///
/// Chroma, rms and centroid are means, because they describe a level that
/// should not depend on how many frames a beat happened to contain. Onset is
/// the **max within each sub-beat bin**: an attack is a moment, not a level,
/// and averaging it across the beat would divide a transient by the frame
/// count and hand back a smear where there was a hit. The bin is chosen by
/// position within the beat, so where in the beat the hit landed survives.
///
/// An empty slice is all zeroes — the honest "nothing was heard here" that
/// [`crate::Digester`]'s row pooling also returns, and not a guess.
pub fn beat_vector(frames: &[Native]) -> BeatVector {
    let mut v = [0.0f32; DIMS];
    if frames.is_empty() {
        return BeatVector { v };
    }

    let n = frames.len();
    // Per-bin peaks, so a single-frame hit survives pooling. A frame lands in
    // bin `i * ONSET_BINS / n`, which spreads the frames across the bins
    // evenly whatever their count.
    let mut onset_peak = [0.0f32; ONSET_BINS];
    let mut rms = 0.0f32;
    let mut centroid = 0.0f32;
    let mut chroma = [0.0f32; crate::N_CHROMA];

    for (i, f) in frames.iter().enumerate() {
        let bin = i * ONSET_BINS / n;
        if f.onset > onset_peak[bin] {
            onset_peak[bin] = f.onset;
        }
        rms += f.rms;
        centroid += f.centroid;
        for c in 0..crate::N_CHROMA {
            chroma[c] += f.chroma[c];
        }
    }

    let inv = 1.0 / n as f32;
    for c in 0..crate::N_CHROMA {
        v[c] = chroma[c] * inv;
    }
    v[crate::N_CHROMA..crate::N_CHROMA + ONSET_BINS].copy_from_slice(&onset_peak);
    v[DIMS - 2] = rms * inv;
    v[DIMS - 1] = centroid * inv;
    BeatVector { v }
}

/// The detector.
pub struct Repeat {
    /// Ring of the last [`HISTORY_BEATS`] beat vectors, oldest first once
    /// ordered. `head` is the next slot to write; `filled` saturates at the
    /// capacity, so the count stays meaningful after the ring wraps.
    history: Vec<BeatVector>,
    head: usize,
    filled: usize,

    /// Absolute beat index of the most recent `push`, if any. Used to stamp
    /// `since_beat` when a lock begins.
    beat_index: Option<f32>,

    /// Set while locked, and returned for those beats.
    state: Option<RepeatState>,

    /// The last score computed for each candidate length, or `None` when the
    /// history is too short to form both windows.
    scores: [Option<f32>; LENGTHS.len()],
}

impl Default for Repeat {
    fn default() -> Self {
        Self::new()
    }
}

impl Repeat {
    pub fn new() -> Repeat {
        Repeat {
            history: vec![BeatVector { v: [0.0; DIMS] }; HISTORY_BEATS],
            head: 0,
            filled: 0,
            beat_index: None,
            state: None,
            scores: [None; LENGTHS.len()],
        }
    }

    /// Feed one beat vector, with the absolute beat index it covers.
    ///
    /// Returns `Some(state)` while locked, `None` otherwise. A score spans two
    /// whole cycles of its length, so the first one at `>= ENTER` locks; it
    /// releases (returns `None`) after one beat at `score < EXIT`.
    pub fn push(&mut self, beat: BeatVector) -> Option<RepeatState> {
        // The index the caller gives the beat it is handing over. Newest beat
        // is at `beat_index`; locks are stamped with the first beat of the
        // cycle that matched, so `since_beat` means "the loop's start".
        self.beat_index = match self.beat_index {
            Some(last) => Some(last + 1.0),
            // The first push has no predecessor to count from, so it is taken
            // as beat 0. The absolute origin is the caller's convention; all
            // this needs is a monotone, 1-beat-per-push number to report.
            None => Some(0.0),
        };

        self.history[self.head] = beat;
        self.head = (self.head + 1) % HISTORY_BEATS;
        self.filled = (self.filled + 1).min(HISTORY_BEATS);

        self.score_all();

        // A lock is released the moment any of its cycles falls below EXIT;
        // a detector that never lets go would miss the next loop.
        if let Some(state) = self.state {
            let released = self
                .score(state.length_beats)
                .map(|s| s < EXIT)
                .unwrap_or(true);
            if released {
                self.state = None;
            } else {
                return Some(state);
            }
        }

        // Shortest passing length wins; ties can't happen because each index
        // is distinct. A single score is already two whole cycles: it compares
        // the last `L` beats against the `L` before them, so `>= ENTER` here
        // means the match *has* held for `2 * L` beats. Waiting for `2 * L`
        // further beats would be four cycles and would lock a bar late.
        let n = self.beat_index.unwrap_or(0.0);
        for (i, &len) in LENGTHS.iter().enumerate() {
            let Some(score) = self.scores[i] else {
                continue;
            };
            if score >= ENTER {
                let start = n - (2.0 * len as f32 - 1.0);
                self.state = Some(RepeatState {
                    length_beats: len,
                    since_beat: start.max(0.0),
                    confidence: score_to_confidence(score),
                });
                return self.state;
            }
        }

        None
    }

    /// The last score for a given L (for the CLI readout / tests). `None` if
    /// there is not yet enough history.
    pub fn score(&self, len_beats: u32) -> Option<f32> {
        let i = LENGTHS.iter().position(|&l| l == len_beats)?;
        self.scores[i]
    }

    /// Reset history and lock (used when the source changes).
    pub fn reset(&mut self) {
        *self = Repeat::new();
    }

    /// Recompute every candidate's score against the current ring.
    ///
    /// Scores are cached rather than recomputed by the caller because the ring
    /// is what both the scoring and the CLI readout want, and there is exactly
    /// one ring.
    fn score_all(&mut self) {
        for (i, &len) in LENGTHS.iter().enumerate() {
            self.scores[i] = self.score_at(len);
        }
    }

    /// Cosine similarity between the last `len` beats and the `len` before
    /// those, beat by beat.
    ///
    /// `None` until the ring holds `2 * len` beats: before that there is no
    /// second window to compare against, and a partial one would score a
    /// shortened cycle against a full one and call the missing beats a change.
    ///
    /// Each beat has the running [`ZSCORE_BEATS`]-beat column mean removed and
    /// is scaled to unit length; the two windows are then compared **flattened**
    /// — beat after beat, not as their means. Two drafts that look reasonable
    /// and do not work, kept here because the temptation recurs:
    ///
    /// * Averaging *raw* beats and taking the cosine measures loudness, which
    ///   the demeaning is meant to remove.
    /// * Averaging the demeaned beats cancels a periodic signal to nothing: a
    ///   loop's lit column is above its mean on the beat it is hit and below it
    ///   on the others, and those sum to ~0 over any whole period, so an exact
    ///   repeat scored `0.0`. Worse, a mean throws away order, so a shuffled
    ///   loop scored `1.0`. A detector that cannot see order is not one.
    ///
    /// Keeping the beats in sequence survives both: `1.0` for an exact repeat,
    /// a different number when the order changes, and still blind to a gain
    /// because the per-beat unit scaling divides it out.
    fn score_at(&self, len: u32) -> Option<f32> {
        let l = len as usize;
        if self.filled < 2 * l {
            return None;
        }

        // Mean each column over the most recent ZSCORE_BEATS beats, oldest
        // first. The running window is what removes a drift — a breakdown
        // rising in level, a filter opening — so the comparison is about shape
        // and not about where in the night it happens to be.
        let ctx = self.ordered_recent(ZSCORE_BEATS);
        let mean = column_means(&ctx);
        // Total energy of the context. A context with none is silence, and
        // silence is not a loop however perfectly it repeats.
        let energy: f32 = ctx.iter().flat_map(|b| b.v.iter()).map(|x| x * x).sum();

        // Oldest first: the last `l` beats are window A; the `l` before them
        // are window B.
        let win_a = self.ordered_recent(l);
        let win_b = self.ordered_recent_range(l);

        let a = flat_centered_unit(&win_a, &mean);
        let b = flat_centered_unit(&win_b, &mean);

        // Both windows can flatten to nothing when every beat equals the
        // running mean — which is exactly what a single repeated beat looks
        // like, the simplest loop there is. Zero against zero is not an angle,
        // so read it directly: identical raw windows are the same bars and
        // score 1.0, anything else scores 0.0. Silence, which is also flat,
        // is kept out by the energy gate above.
        let na: f32 = a.iter().map(|x| x * x).sum();
        let nb: f32 = b.iter().map(|x| x * x).sum();
        if na < 1e-12 || nb < 1e-12 {
            if energy < 1e-9 {
                return Some(0.0);
            }
            let same = win_a.iter().zip(&win_b).all(|(x, y)| x.v == y.v);
            return Some(if same { 1.0 } else { 0.0 });
        }

        Some(cosine(&a, &b).clamp(-1.0, 1.0))
    }

    /// The last `n` beats, oldest first. Fewer than `n` if the ring is not
    /// full; callers that need a full window gate on `filled` first.
    fn ordered_recent(&self, n: usize) -> Vec<BeatVector> {
        let n = n.min(self.filled);
        (0..n)
            .map(|k| {
                // Oldest of the `n` newest sits `n - 1 - k` back from the head.
                let back = n - 1 - k;
                let idx = (self.head + HISTORY_BEATS - 1 - back) % HISTORY_BEATS;
                self.history[idx]
            })
            .collect()
    }

    /// The `n` beats ending `skip` beats before the newest, oldest first.
    /// Used to reach window B behind window A without copying the ring twice.
    fn ordered_recent_range(&self, skip: usize) -> Vec<BeatVector> {
        let n = skip.min(self.filled);
        (0..n)
            .map(|k| {
                let back = 2 * skip - 1 - k;
                let idx = (self.head + HISTORY_BEATS - 1 - back) % HISTORY_BEATS;
                self.history[idx]
            })
            .collect()
    }
}

/// Map a cosine in `-1..1` onto a confidence in `0..1`.
///
/// `(cos + 1) / 2` is the choice: it is monotone, exact at both ends, and
/// `0.5` for an orthogonal (unrelated) pair, which reads as "no evidence" in a
/// readout. A raw cosine would show negative confidences, which no operator
/// wants to see on a meter.
fn score_to_confidence(score: f32) -> f32 {
    ((score + 1.0) * 0.5).clamp(0.0, 1.0)
}

/// A window flattened beat-after-beat: each beat has the running mean removed,
/// then is scaled to unit length.
///
/// Two operations, each doing a job the other cannot:
///
/// * **Subtracting the running column mean** takes out the trend — a breakdown
///   rising, a filter opening over a whole phrase — so two windows are compared
///   on shape and not on where in the night they happened. It is the useful
///   half of a z-score.
/// * **Scaling each beat to unit length** takes out a per-beat gain, which is
///   precisely a filter sweep, and it keeps quiet columns from being amplified
///   into the comparison. Dividing by the running standard deviation instead
///   would do the opposite: a chroma bin with only noise in it has a tiny
///   spread, and normalising it to unit variance would promote that noise to a
///   full-scale feature and drown the two bins that carry the pattern.
///
/// The beats are kept **in order** rather than averaged, which is the whole
/// difference between a detector and a very confident coin. Averaging discards
/// which beat came first, so a shuffled loop would still score `1.0`; and
/// because a periodic signal sums to zero over a whole period, every exact
/// repeat averaged to a flat vector and scored `0.0`.
fn flat_centered_unit(window: &[BeatVector], mean: &[f32; DIMS]) -> Vec<f32> {
    let mut out = Vec::with_capacity(window.len() * DIMS);
    for v in window {
        let mut d = [0.0f32; DIMS];
        for j in 0..DIMS {
            d[j] = v.v[j] - mean[j];
        }
        let norm = d.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm < 1e-6 {
            out.resize(out.len() + DIMS, 0.0);
        } else {
            out.extend(d.iter().map(|x| x / norm));
        }
    }
    out
}

/// Per-column mean over `ctx`.
///
/// The mean is all the linear step needs. The spread is deliberately not
/// computed: dividing by it would amplify a near-silent column's noise (see
/// [`flat_centered_unit`]), which is the failure that made the filter-sweep
/// test score near zero across the board.
fn column_means(ctx: &[BeatVector]) -> [f32; DIMS] {
    let mut mean = [0.0f32; DIMS];
    let n = ctx.len();
    if n == 0 {
        return mean;
    }
    for v in ctx {
        for j in 0..DIMS {
            mean[j] += v.v[j];
        }
    }
    for m in mean.iter_mut() {
        *m /= n as f32;
    }
    mean
}

/// Cosine of the angle between two vectors, `0` when either is flat.
///
/// A zero vector has no direction, so the angle is undefined; returning `0`
/// (rather than dividing by zero or pretending `1`) makes a window with no
/// variation score as unrelated, which is the conservative reading.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (&x, &y) in a.iter().zip(b) {
        dot += (x as f64) * (y as f64);
        na += (x as f64) * (x as f64);
        nb += (y as f64) * (y as f64);
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom < 1e-12 {
        0.0
    } else {
        (dot / denom) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small LCG. Deterministic, dependency-free, and good enough to stand
    /// in for "a different beat" — the tests must fail the same way every
    /// time, which is what a seeded PRNG here is for.
    struct Lcg(u64);

    impl Lcg {
        fn new(seed: u64) -> Lcg {
            Lcg(seed.wrapping_mul(6364136223846793005).wrapping_add(1))
        }

        fn next_f32(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            // Top 24 bits, scaled into -1..1.
            let bits = (self.0 >> 40) as f32;
            (bits / (1u32 << 24) as f32) * 2.0 - 1.0
        }

        /// A vector of the given seed, with values in a small range so noise
        /// perturbs a pattern rather than drowning it.
        fn beat(&mut self) -> BeatVector {
            let mut v = [0.0f32; DIMS];
            for x in v.iter_mut() {
                *x = self.next_f32();
            }
            BeatVector { v }
        }
    }

    /// A vector with one chroma bin lit — a distinguishable pattern, so two
    /// beats can be made equal or different on purpose.
    fn pattern(bin: usize, level: f32) -> BeatVector {
        let mut v = [0.0f32; DIMS];
        v[bin % crate::N_CHROMA] = level;
        v[DIMS - 2] = level;
        BeatVector { v }
    }

    fn drive(beats: &[BeatVector]) -> Vec<Option<RepeatState>> {
        let mut r = Repeat::new();
        beats.iter().map(|&b| r.push(b)).collect()
    }

    // A 4/4 loop of four distinct beats, so its period is unambiguously four.
    fn four_beat_phrase() -> Vec<BeatVector> {
        vec![pattern(0, 1.0), pattern(3, 1.0), pattern(6, 1.0), pattern(9, 1.0)]
    }

    #[test]
    fn a_four_beat_loop_reports_four_not_eight() {
        // Sixteen beats: a phrase in 0..4, then the *same* phrase in 4..8, and
        // 8..12 again, so by beat 12 the detector has seen two whole cycles of
        // four and must say four, not eight or sixteen.
        let phrase = four_beat_phrase();
        let mut beats = Vec::new();
        for i in 0..16 {
            beats.push(phrase[i % 4]);
        }
        let out = drive(&beats);
        let locked: Vec<RepeatState> = out.iter().flatten().copied().collect();
        assert!(!locked.is_empty(), "a clean four-on-four phrase must lock");
        assert_eq!(locked.last().unwrap().length_beats, 4, "the period, not a multiple");
    }

    #[test]
    fn loops_of_four_eight_sixteen_and_thirty_two_beats_are_found() {
        // The plan named this test for {1,2,4,8}; the measured false-positive
        // run moved the candidate set to {4,8,16,32} (see `LENGTHS`), so the
        // test follows the set that actually ships.
        for &l in &LENGTHS {
            // Each cycle is a distinct phrase of `l` beats; four cycles of it
            // is enough to lock, and the last four repeat the previous four.
            let mut rng = Lcg::new(l as u64 * 7919);
            let phrase: Vec<BeatVector> = (0..l).map(|_| rng.beat()).collect();
            let mut beats = Vec::new();
            for _ in 0..8 {
                beats.extend_from_slice(&phrase);
            }
            let out = drive(&beats);
            let locked: Vec<RepeatState> = out.iter().flatten().copied().collect();
            assert!(!locked.is_empty(), "L={l} should lock on a clean repeat");
            assert_eq!(locked.last().unwrap().length_beats, l, "wrong period for L={l}");
        }
    }

    #[test]
    fn a_single_beat_repeat_does_not_lock() {
        // The regression the corpus run found: a metronomic track repeats every
        // beat, and with 1 in the candidate set that scored ~1.0 and locked the
        // visuals to a single beat (37/40 songs). The fix is the candidate set,
        // so this pins the set itself and the `score` seam that reads it.
        assert!(
            !LENGTHS.contains(&1) && !LENGTHS.contains(&2),
            "a bar is the shortest DJ loop this detects; LENGTHS={LENGTHS:?}"
        );
        let mut r = Repeat::new();
        for _ in 0..32 {
            r.push(pattern(0, 1.0));
        }
        assert_eq!(r.score(1), None, "a length that is not a candidate has no score");
        assert_eq!(r.score(2), None);
    }

    #[test]
    fn a_filter_sweep_over_a_loop_keeps_it_locked() {
        // The same loop, but window B is scaled by a slowly changing gain and
        // each beat carries a little noise. Cosine is blind to a uniform gain
        // and tolerant of the noise, so the lock must survive the sweep.
        let phrase = four_beat_phrase();
        let mut rng = Lcg::new(20240926);
        let mut r = Repeat::new();
        let mut saw_lock = false;
        let mut stayed = true;

        for i in 0..32 {
            let mut b = phrase[i % 4];
            // A gain that sweeps across the run, plus per-beat noise that is
            // small compared to the pattern.
            let gain = 1.0 + 0.4 * (i as f32 / 32.0);
            for x in b.v.iter_mut() {
                *x = *x * gain + 0.05 * rng.next_f32();
            }
            match r.push(b) {
                Some(state) => {
                    saw_lock = true;
                    if state.length_beats != 4 {
                        // A longer multiple passing is fine only if it is not
                        // picked; shortest-wins means 4 must win.
                        stayed = false;
                    }
                }
                None => {
                    if saw_lock {
                        stayed = false;
                    }
                }
            }
        }
        assert!(saw_lock, "a gain sweep must not prevent the lock");
        assert!(stayed, "the lock must hold across the whole sweep");
    }

    #[test]
    fn a_breakdown_with_no_repeat_never_locks() {
        // A vector that drifts monotonically: every beat differs from the
        // last, so no length has two matching windows.
        let mut rng = Lcg::new(7);
        let mut beats = Vec::new();
        for i in 0..48 {
            let mut b = rng.beat();
            for (j, x) in b.v.iter_mut().enumerate() {
                *x += i as f32 * 0.3 + j as f32 * 0.01;
            }
            beats.push(b);
        }
        let out = drive(&beats);
        assert!(out.iter().all(|o| o.is_none()), "a monotonically drifting signal must never lock");
    }

    #[test]
    fn an_exact_file_repeat_is_found_by_equality_and_by_score() {
        // An exact repeat: the score must be exactly 1.0 and the lock must
        // arrive at the minimum, i.e. after 2*L beats.
        let phrase = four_beat_phrase();
        let mut r = Repeat::new();
        let mut lock_at = None;
        for i in 0..16 {
            let b = phrase[i % 4];
            let out = r.push(b);
            if let Some(state) = out {
                if lock_at.is_none() {
                    lock_at = Some((i, state));
                }
            }
        }
        assert_eq!(r.score(4), Some(1.0), "an exact repeat scores 1.0");
        let (i, state) = lock_at.expect("an exact repeat must lock");
        // At beat index i, the newest beat is the one pushed; the 2*L = 8
        // beats needed are i-7..=i, so the lock can first appear at i == 7.
        assert_eq!(i, 7, "the lock must arrive at the minimum, after two cycles");
        assert_eq!(state.length_beats, 4);
        assert_eq!(state.confidence, 1.0, "score 1.0 maps to confidence 1.0");
    }

    #[test]
    fn the_detector_takes_two_cycles_to_lock() {
        let phrase = four_beat_phrase();
        let mut r = Repeat::new();
        // One whole cycle: beats 0..4. Nothing may lock yet — one match proves
        // a coincidence, not a loop.
        for i in 0..4 {
            assert!(
                r.push(phrase[i % 4]).is_none(),
                "beat {i}: one cycle is not enough to lock"
            );
        }
        // A second cycle completes at beat 7, which is 8 beats (2*L) of score.
        let mut locked = None;
        for i in 4..8 {
            if let Some(s) = r.push(phrase[i % 4]) {
                locked = Some(s);
            }
        }
        assert!(locked.is_some(), "two whole cycles must lock");
        assert_eq!(locked.unwrap().length_beats, 4);
    }

    #[test]
    fn beat_vector_pools_the_frames_it_is_given() {
        // Chroma/rms/centroid are means; onset is per-bin max, so a lone hit
        // in the second half survives even with quiet frames around it.
        let frames = vec![
            Native { rms: 0.5, centroid: 2.0, onset: 0.0, chroma: [0.2; crate::N_CHROMA], ..Default::default() },
            Native { rms: 0.5, centroid: 2.0, onset: 1.0, chroma: [0.2; crate::N_CHROMA], ..Default::default() },
            Native { rms: 0.5, centroid: 2.0, onset: 0.0, chroma: [0.2; crate::N_CHROMA], ..Default::default() },
            Native { rms: 0.5, centroid: 2.0, onset: 0.0, chroma: [0.2; crate::N_CHROMA], ..Default::default() },
        ];
        let bv = beat_vector(&frames);
        assert!((bv.v[0] - 0.2).abs() < 1e-6, "chroma is the frame mean");
        assert!((bv.v[DIMS - 2] - 0.5).abs() < 1e-6, "rms is the frame mean");
        assert!((bv.v[DIMS - 1] - 2.0).abs() < 1e-6, "centroid is the frame mean");
        // Four frames, four bins: the hit is in bin 1 and must be exactly 1.0.
        assert!((bv.v[crate::N_CHROMA + 1] - 1.0).abs() < 1e-6, "onset is the per-bin max");
        assert_eq!(beat_vector(&[]).v, [0.0; DIMS], "no frames is the honest zero");
    }

    #[test]
    fn reset_forgets_everything() {
        let phrase = four_beat_phrase();
        let mut r = Repeat::new();
        for i in 0..8 {
            r.push(phrase[i % 4]);
        }
        assert!(r.score(4).is_some());
        r.reset();
        assert!(r.score(4).is_none(), "reset must clear the history");
    }
}
