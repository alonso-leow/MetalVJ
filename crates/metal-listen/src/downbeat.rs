//! Hearing the "1" — which beat of the 4/4 bar is the downbeat.
//!
//! [`crate::tempo::Tempo`] counts beats but has no idea where the bar starts:
//! its count begins wherever it happened to lock. Loops are one bar long
//! (`the loops design notes` decision 1), so a loop launched on the tracker's beat 0
//! lands on the "1" one time in four. This module supplies the missing offset.
//!
//! # How
//!
//! A downbeat is where the music *leans*: the kick that opens the bar, the
//! crash on the one, the bass that changes on it. So each beat is reduced to
//! one number — its **accent**, the strongest onset heard within
//! [`HIT_WINDOW`] of it — and the accent is judged against the music's own
//! history rather than an absolute level: a beat *stands out* when its accent
//! is above the [`PERCENTILE`] of the last [`BARS`] bars of beats. Standing out
//! is then folded by beat position mod 4, and the position that stands out
//! most often is the "1". Comparing against a percentile of the history (not
//! a fixed threshold, not the mean) is what makes it work equally on a quiet
//! intro and a drop, and makes a four-on-the-floor with identical kicks give
//! *no* answer instead of a random one: nothing stands out, so confidence is
//! zero.
//!
//! Salvaged from `metal-perform/src/beatloop.rs` (`accent`, `Downbeat::observe`):
//! the nearest-beat window, and the rule that asking twice at one instant
//! changes nothing.
//!
//! # Tap
//!
//! "Tap 1" is the operator's correction: the beat nearest the tap becomes the
//! "1", and stays so until the next tap — what is heard keeps being computed
//! (and reported in [`Downbeat::heard`]) but never overrides a tap.

use std::collections::VecDeque;

/// Beats per bar. The plan is 4/4 only.
pub const BEATS_PER_BAR: usize = 4;
/// Bars of beat accents kept as the history a beat is judged against.
pub const BARS: usize = 8;
/// A beat "stands out" above this quantile of the history's accents.
pub const PERCENTILE: f32 = 0.75;
/// How far from a beat (in beats) an onset still counts as that beat's.
const HIT_WINDOW: f64 = 0.3;
/// Bars of history needed before anything is claimed.
const MIN_BARS: usize = 2;

/// The downbeat detector. Feed it [`Downbeat::observe`] once per analysis
/// frame with the beat clock and the onset strength.
#[derive(Clone, Debug, Default)]
pub struct Downbeat {
    /// (beat index, accent) of the last [`BARS`] bars of finished beats.
    history: VecDeque<(i64, f32)>,
    /// The beat being heard now and its loudest hit so far.
    current: Option<(i64, f32)>,
    /// The position "tap 1" made the first of the bar.
    tapped: Option<u8>,
    /// Last computed (position, confidence), refreshed as each beat closes.
    heard: Option<(u8, f32)>,
}

impl Downbeat {
    pub fn new() -> Downbeat {
        Downbeat::default()
    }

    /// One reading of `accent` (onset strength, any non-negative scale) at
    /// `beats` on the beat clock. Readings within [`HIT_WINDOW`] of a beat
    /// keep that beat's loudest; a beat is closed into the history once the
    /// clock is nearer the next one. A clock that went backwards (it should
    /// not) drops the open beat rather than corrupting the history.
    pub fn observe(&mut self, beats: f64, accent: f32) {
        if !beats.is_finite() {
            return;
        }
        let nearest = beats.round();
        let beat = nearest as i64;
        if let Some((cur, peak)) = self.current {
            if beat > cur {
                self.close(cur, peak);
                self.current = None;
            } else if beat < cur {
                self.current = None;
            }
        }
        if (beats - nearest).abs() <= HIT_WINDOW && accent.is_finite() {
            let peak = match self.current {
                Some((cur, p)) if cur == beat => p.max(accent),
                _ => accent.max(0.0),
            };
            self.current = Some((beat, peak));
        }
    }

    fn close(&mut self, beat: i64, peak: f32) {
        self.history.push_back((beat, peak));
        while self.history.len() > BARS * BEATS_PER_BAR {
            self.history.pop_front();
        }
        self.heard = self.judge();
    }

    /// Fold "stands out against the history" by position and pick the
    /// position that does it most. Confidence is the margin between the best
    /// and second-best position's stand-out rate, 0..1.
    fn judge(&self) -> Option<(u8, f32)> {
        if self.history.len() < MIN_BARS * BEATS_PER_BAR {
            return None;
        }
        let mut sorted: Vec<f32> = self.history.iter().map(|&(_, a)| a).collect();
        sorted.sort_by(f32::total_cmp);
        let idx = ((sorted.len() - 1) as f32 * PERCENTILE).round() as usize;
        let threshold = sorted[idx];

        let mut out = [0u32; BEATS_PER_BAR];
        let mut seen = [0u32; BEATS_PER_BAR];
        for &(b, a) in &self.history {
            let pos = b.rem_euclid(BEATS_PER_BAR as i64) as usize;
            seen[pos] += 1;
            // Strictly above (with a 5 % margin): a bar of identical hits
            // stands out nowhere.
            if a > threshold * 1.05 + 1e-6 {
                out[pos] += 1;
            }
        }
        let rate: [f32; BEATS_PER_BAR] =
            std::array::from_fn(|i| if seen[i] == 0 { 0.0 } else { out[i] as f32 / seen[i] as f32 });
        let best = (0..BEATS_PER_BAR).max_by(|&a, &b| rate[a].total_cmp(&rate[b]))?;
        let second = (0..BEATS_PER_BAR)
            .filter(|&i| i != best)
            .map(|i| rate[i])
            .fold(0.0f32, f32::max);
        let confidence = (rate[best] - second).clamp(0.0, 1.0);
        (confidence > 0.0).then_some((best as u8, confidence))
    }

    /// "Tap 1": the beat nearest `beats` becomes the "1" until the next tap.
    /// Returns its position (beat index mod 4).
    pub fn tap(&mut self, beats: f64) -> u8 {
        let pos = (beats.round() as i64).rem_euclid(BEATS_PER_BAR as i64) as u8;
        self.tapped = Some(pos);
        pos
    }

    /// Whether a tap is currently in force.
    pub fn tapped(&self) -> bool {
        self.tapped.is_some()
    }

    /// The "1" as heard: (beat index mod 4, confidence 0..1), ignoring taps.
    /// `None` until two bars have been heard and one position stands out.
    pub fn heard(&self) -> Option<(u8, f32)> {
        self.heard
    }

    /// The "1" in force: which beat index mod 4 is a downbeat. A tap wins
    /// over what is heard.
    pub fn phase(&self) -> Option<u8> {
        self.tapped.or(self.heard.map(|h| h.0))
    }

    /// Confidence of [`Downbeat::phase`]: 1 for a tap, the heard margin
    /// otherwise, 0 when there is no answer.
    pub fn confidence(&self) -> f32 {
        if self.tapped.is_some() {
            1.0
        } else {
            self.heard.map_or(0.0, |h| h.1)
        }
    }

    /// Whether integer beat `beat` of the clock is a "1".
    pub fn is_downbeat(&self, beat: i64) -> bool {
        self.phase()
            .is_some_and(|p| beat.rem_euclid(BEATS_PER_BAR as i64) == p as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempo::FRAME_RATE;

    /// Drive a detector with a perfect beat clock at `bpm` and a flux that
    /// hits every beat, louder on every beat ≡ `one` (mod 4).
    fn run(bpm: f64, one: i64, secs: f64) -> Downbeat {
        let mut d = Downbeat::new();
        let per = FRAME_RATE * 60.0 / bpm;
        for i in 0..(FRAME_RATE * secs) as usize {
            let beats = i as f64 / per;
            let into = (beats - beats.floor()) * per;
            let loud = if (beats.floor() as i64).rem_euclid(4) == one { 1.0 } else { 0.45 };
            d.observe(beats, (loud * (-into / 1.5).exp()) as f32);
        }
        d
    }

    #[test]
    fn the_louder_fourth_beat_is_the_one() {
        for bpm in [90.0, 128.0, 174.0] {
            for one in [0, 1, 3] {
                let d = run(bpm, one, 20.0);
                let (pos, conf) = d.heard().unwrap_or_else(|| panic!("{bpm} BPM: nothing heard"));
                assert_eq!(pos as i64, one, "{bpm} BPM");
                assert!(conf > 0.5, "{bpm} BPM: confidence {conf}");
                assert!(d.is_downbeat(one + 8));
            }
        }
    }

    #[test]
    fn identical_hits_claim_no_downbeat() {
        let mut d = Downbeat::new();
        let per = FRAME_RATE * 60.0 / 120.0;
        for i in 0..(FRAME_RATE * 20.0) as usize {
            let beats = i as f64 / per;
            let into = (beats - beats.floor()) * per;
            d.observe(beats, (-into / 1.5).exp() as f32);
        }
        assert_eq!(d.phase(), None);
        assert_eq!(d.confidence(), 0.0);
    }

    #[test]
    fn a_tap_overrides_until_the_next_tap() {
        let mut d = run(128.0, 0, 20.0);
        assert_eq!(d.phase(), Some(0));
        assert_eq!(d.tap(42.1), 2);
        assert_eq!(d.phase(), Some(2));
        assert_eq!(d.confidence(), 1.0);
        // More music saying "0" does not undo the tap…
        let per = FRAME_RATE * 60.0 / 128.0;
        for i in 0..(FRAME_RATE * 10.0) as usize {
            let beats = 50.0 + i as f64 / per;
            let loud = if (beats.round() as i64) % 4 == 0 { 1.0 } else { 0.4 };
            d.observe(beats, loud);
        }
        assert_eq!(d.phase(), Some(2));
        assert_eq!(d.heard().unwrap().0, 0, "hearing carries on under a tap");
        // …the next tap does.
        assert_eq!(d.tap(47.9), 0);
        assert_eq!(d.phase(), Some(0));
    }
}
