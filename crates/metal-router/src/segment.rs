//! Where a song changes enough to be worth deciding again.
//!
//! *"a single output that varies little, only at certain important changes in
//! the song."*
//!
//! That sentence is a statement about the **shape** of the output, and there
//! are two ways to honour it. One is to classify every tick and then smooth,
//! which gives a piecewise-constant answer only by accident and wobbles at
//! every boundary. The other is to decide *where the boundaries are first* and
//! then make one decision per section, which gives it by construction. This is
//! the second.
//!
//! # The method
//!
//! Self-similarity novelty, which is the standard way and is standard because
//! it needs nothing learned: describe the `W` ticks before an instant and the
//! `W` after it, and the distance between the two descriptions is how much of a
//! change that instant is. Peaks in that curve are the boundaries. A drop, a
//! breakdown, a key change and an intro ending are all large; a fill is not.
//!
//! Nothing about it is trained, and that is deliberate — a boundary is a fact
//! about the audio, not a preference. What is trained is which animation goes
//! in each section, which is where the taste lives.

use crate::features::{describe, distance, COLUMNS, FEATURES, RATE};

/// Half the comparison window, in seconds either side — shorter and a fill
/// reads as a section, longer and a short intro is swallowed by the verse
/// after it — converted to ticks at `rate`.
pub const fn half_window(rate: metal_schema::TickRate) -> usize {
    8 * rate.hz() as usize
}

/// The base-grid half window. A fine caller passes `TickRate::Fine` to
/// [`half_window`].
pub const HALF_WINDOW: usize = half_window(metal_schema::TickRate::Base);

/// The novelty curve is only evaluated every this many ticks. A boundary
/// resolved to half a second is resolved far finer than anything downstream can
/// use, and the curve costs a `describe` of 128 rows at every point.
const STRIDE: usize = 4;

/// How much of a change a boundary has to be, on top of clearing the curve's
/// own bar.
///
/// **The relative threshold alone is not enough, and the failure is silent.**
/// `mean + one standard deviation` is the right shape for a track that does
/// change — it adapts to how eventful the music is — but over a track that
/// never changes, the curve is numerical noise around zero and *its* mean plus
/// standard deviation is still numerical noise. Four steady minutes came out
/// cut into thirteen pieces, every boundary a rounding error, and nothing about
/// the output looked wrong: thirteen plausible sections of a song.
///
/// [`crate::features::distance`] averages over twelve features, so the scale is
/// small: two passages of one track sit around 0.01, a wash against a beat
/// around 0.25. Four hundredths is far above the first and far below the second.
const FLOOR: f32 = 0.04;

/// One stretch of a song the router makes a single decision about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    /// First tick, inclusive.
    pub from: usize,
    /// Last tick, exclusive.
    pub to: usize,
}

impl Section {
    pub fn len(&self) -> usize {
        self.to.saturating_sub(self.from)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, tick: usize) -> bool {
        tick >= self.from && tick < self.to
    }
}

/// The novelty curve, one value per tick (held between strides).
///
/// Returned rather than kept private because the studio draws it: an operator
/// who can see where the model thinks the song changes can tell "the router
/// picked badly here" from "the router thinks this is still the intro", and
/// those are different problems.
pub fn novelty(rows: &[[f32; COLUMNS]]) -> Vec<f32> {
    let mut out = vec![0.0f32; rows.len()];
    if rows.len() < HALF_WINDOW * 2 {
        return out;
    }
    let mut at = HALF_WINDOW;
    while at + HALF_WINDOW <= rows.len() {
        let before = describe(&rows[at - HALF_WINDOW..at]);
        let after = describe(&rows[at..at + HALF_WINDOW]);
        let d = distance(&before, &after);
        for slot in out.iter_mut().skip(at).take(STRIDE) {
            *slot = d;
        }
        at += STRIDE;
    }
    out
}

/// Cuts `rows` into sections at its own novelty peaks.
///
/// `min_seconds` is the shortest section allowed. It exists because the ask is
/// explicit that the output should change *little*: without it, a track with a
/// busy arrangement gets a boundary every few bars and the router is back to
/// switching on a clock, only a worse one.
pub fn sections(rows: &[[f32; COLUMNS]], min_seconds: f32) -> Vec<Section> {
    let n = rows.len();
    if n == 0 {
        return Vec::new();
    }
    let min_ticks = ((min_seconds.max(2.0) * RATE) as usize).max(1);
    if n <= min_ticks * 2 {
        return vec![Section { from: 0, to: n }];
    }

    let curve = novelty(rows);
    // The bar a peak has to clear: the curve's own mean plus a standard
    // deviation. Absolute thresholds do not survive the difference between a
    // track that changes constantly and one that does not.
    let live: Vec<f32> = curve
        .iter()
        .copied()
        .skip(HALF_WINDOW)
        .take(n.saturating_sub(HALF_WINDOW * 2))
        .collect();
    if live.is_empty() {
        return vec![Section { from: 0, to: n }];
    }
    let mean = live.iter().sum::<f32>() / live.len() as f32;
    let var = live.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / live.len() as f32;
    let bar = (mean + var.sqrt()).max(FLOOR);

    // Peaks, strongest first, taken greedily so far apart ones win over a
    // cluster of neighbours around one change.
    let mut candidates: Vec<(usize, f32)> = Vec::new();
    let mut at = HALF_WINDOW + STRIDE;
    while at + HALF_WINDOW + STRIDE < n {
        let v = curve[at];
        if v > bar && v >= curve[at - STRIDE] && v >= curve[at + STRIDE] {
            candidates.push((at, v));
        }
        at += STRIDE;
    }
    candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));

    let mut cuts: Vec<usize> = Vec::new();
    for (tick, _) in candidates {
        if tick < min_ticks || n - tick < min_ticks {
            continue;
        }
        if cuts.iter().any(|c| tick.abs_diff(*c) < min_ticks) {
            continue;
        }
        cuts.push(tick);
    }
    cuts.sort_unstable();

    let mut out = Vec::with_capacity(cuts.len() + 1);
    let mut from = 0usize;
    for cut in cuts {
        out.push(Section { from, to: cut });
        from = cut;
    }
    out.push(Section { from, to: n });
    out
}

/// The live half: has the music changed enough to decide again?
///
/// `metal-dash` has no whole song to look at — it has the last minute of a rolling
/// buffer — so it compares the window just gone against the one before it and
/// asks this. Same distance, same threshold, so an operator watching the studio
/// and an operator watching the booth are looking at the same notion of
/// "changed".
pub fn changed(before: &[f32; FEATURES], after: &[f32; FEATURES], sensitivity: f32) -> bool {
    // Calibrated against what `distance` actually produces, which is small
    // because it averages over twelve features: two passages of one track are
    // around 0.01, a level change on its own about 0.03, a wash against a beat
    // about 0.25. So full sensitivity is 0.02 — a single feature moving a lot
    // counts — and zero is 0.22, which in practice means only a drop or a new
    // track. Guessing these against the *features'* range rather than against
    // the average of them is how the first pair came out ten times too high.
    let bar = 0.22 - 0.20 * sensitivity.clamp(0.0, 1.0);
    distance(before, after) > bar
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beat(ticks: usize, bpm: f32, level: f32) -> Vec<[f32; COLUMNS]> {
        let per_beat = 60.0 / bpm * RATE;
        (0..ticks)
            .map(|i| {
                let mut r = [0.0f32; COLUMNS];
                let phase = (i as f32 / per_beat).fract();
                r[2] = if phase < 1.0 / per_beat { 0.9 } else { 0.02 };
                r[0] = level;
                r[1] = 0.4;
                r[3] = 0.35;
                r[4] = 0.9;
                for k in 1..12 {
                    r[4 + k] = 0.02;
                }
                r
            })
            .collect()
    }

    fn wash(ticks: usize) -> Vec<[f32; COLUMNS]> {
        (0..ticks)
            .map(|i| {
                let mut r = [0.0f32; COLUMNS];
                r[0] = 0.30 + 0.08 * (i as f32 * 0.02).sin();
                r[1] = 0.75;
                r[2] = 0.02;
                r[3] = 0.62;
                for k in 0..12 {
                    r[4 + k] = 0.5;
                }
                r
            })
            .collect()
    }

    #[test]
    fn a_song_that_never_changes_is_one_section() {
        let rows = beat(2400, 126.0, 0.7);
        let s = sections(&rows, 20.0);
        assert_eq!(s.len(), 1, "a steady four minutes was cut into {} pieces", s.len());
        assert_eq!(s[0], Section { from: 0, to: 2400 });
    }

    #[test]
    fn a_song_that_changes_once_is_cut_once_and_near_the_change() {
        // Two minutes of wash, then two of beat. The cut has to land on the
        // join, not somewhere plausible-looking either side of it.
        let mut rows = wash(960);
        rows.extend(beat(960, 128.0, 0.75));
        let s = sections(&rows, 20.0);
        assert_eq!(s.len(), 2, "expected one cut, got {:?}", s);
        let cut = s[0].to;
        assert!(
            cut.abs_diff(960) <= HALF_WINDOW,
            "the cut landed at {cut}, {} ticks off the change",
            cut.abs_diff(960)
        );
    }

    #[test]
    fn sections_tile_the_song_with_no_gaps_and_no_overlaps() {
        // Everything downstream indexes by section, so a tick belonging to two
        // of them or to none is a decision made twice or not at all.
        let mut rows = wash(500);
        rows.extend(beat(700, 140.0, 0.8));
        rows.extend(wash(400));
        rows.extend(beat(600, 90.0, 0.5));
        let s = sections(&rows, 15.0);
        assert_eq!(s[0].from, 0);
        assert_eq!(s.last().unwrap().to, rows.len());
        for pair in s.windows(2) {
            assert_eq!(pair[0].to, pair[1].from, "a gap or an overlap: {:?}", pair);
        }
        for tick in (0..rows.len()).step_by(37) {
            assert_eq!(
                s.iter().filter(|x| x.contains(tick)).count(),
                1,
                "tick {tick} is in more or less than one section"
            );
        }
    }

    #[test]
    fn no_section_is_shorter_than_it_was_told_to_allow() {
        // The whole of *"a single output that varies little"*. Without this the
        // router is a clock again, just a worse one.
        let mut rows = Vec::new();
        for k in 0..12 {
            if k % 2 == 0 {
                rows.extend(beat(120, 100.0 + k as f32 * 12.0, 0.8));
            } else {
                rows.extend(wash(120));
            }
        }
        let min = 25.0;
        for s in sections(&rows, min) {
            // The last section may be short: it is whatever is left over.
            if s.to != rows.len() {
                assert!(
                    s.len() as f32 / RATE >= min - 0.01,
                    "a {:.1}s section slipped past a {min}s floor",
                    s.len() as f32 / RATE
                );
            }
        }
    }

    #[test]
    fn a_song_too_short_to_cut_is_one_section_rather_than_none() {
        for n in [0usize, 1, 40, 200] {
            let s = sections(&wash(n), 20.0);
            if n == 0 {
                assert!(s.is_empty());
            } else {
                assert_eq!(s.len(), 1);
                assert_eq!(s[0].to, n);
            }
        }
    }

    #[test]
    fn the_live_test_agrees_with_the_offline_one_about_what_a_change_is() {
        let a = crate::features::describe(&wash(200));
        let b = crate::features::describe(&beat(200, 130.0, 0.8));
        assert!(changed(&a, &b, 0.5), "a wash into a beat did not read as a change");
        let c = crate::features::describe(&beat(200, 130.0, 0.78));
        assert!(!changed(&b, &c, 0.5), "the same beat a little quieter read as a change");
    }

    #[test]
    fn sensitivity_does_what_it_says() {
        let a = crate::features::describe(&beat(200, 128.0, 0.70));
        let b = crate::features::describe(&beat(200, 128.0, 0.35));
        assert!(changed(&a, &b, 1.0), "at full sensitivity a halving of level is a change");
        assert!(!changed(&a, &b, 0.0), "at zero sensitivity it should take a drop");
    }
}
