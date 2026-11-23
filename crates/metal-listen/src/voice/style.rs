//! The four soft style scores.
//!
//! `the voice design notes` decision 2: **sing, rap, speak, whisper** are floats, not a
//! category, and they sum to `voice`. The rule is a small hand-written one over
//! a ~1 s history:
//!
//! | style | what raises it |
//! |----|----|
//! | sing | high voicing, high steady, long voiced runs, few syllables |
//! | rap | voiced but not steady, many syllables, onsets on the beat grid |
//! | speak | voiced, not steady, moderate syllables, off the grid |
//! | whisper | present but unvoiced, with hiss |
//!
//! Expected to be wrong often. It is here so the face has a *plausible* mouth
//! and so `control.sing` is something an effect can bind to.

/// Everything the score needs, computed once per tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct Features {
    /// Presence, 0..1 — the sum the scores are normalised to.
    pub voice: f32,
    /// Mean YIN periodicity over voiced frames, 0..1.
    pub voicing: f32,
    /// F0 stability, 0..1.
    pub steady: f32,
    /// Syllable rate, 0..1 (rate / 12 Hz).
    pub syllables: f32,
    /// 4–8 kHz share, 0..1.
    pub hiss: f32,
    /// Longest voiced run over the history, as a fraction, 0..1.
    pub voiced_run: f32,
    /// The beat phase now, 0..1, when a clock is available.
    pub beat_phase: Option<f32>,
}

/// Sing/rap/speak/whisper. Always non-negative and summing to `Features::voice`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scores {
    pub sing: f32,
    pub rap: f32,
    pub speak: f32,
    pub whisper: f32,
}

impl Scores {
    pub fn sum(&self) -> f32 {
        self.sing + self.rap + self.speak + self.whisper
    }
}

/// Smooth gate that is 0 below `lo` and 1 above `hi`.
fn gate(x: f32, lo: f32, hi: f32) -> f32 {
    ((x - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// Score the history, then normalise so the four sum to `f.voice`.
pub fn score(f: &Features) -> Scores {
    // Rap and speak need voicing; whisper is the unvoiced branch.
    let voiced = gate(f.voicing, 0.30, 0.65);
    let unvoiced = 1.0 - gate(f.voicing, 0.20, 0.45);
    let grid = match f.beat_phase {
        Some(p) => 0.55 + 0.45 * (std::f32::consts::TAU * p).cos(),
        // No clock: a neutral grid, so offline V1 still scores rap on syllables.
        None => 0.7,
    };

    let mut s = Scores {
        sing: f.voice * voiced * f.steady * (1.0 - f.syllables) * (0.4 + 0.6 * f.voiced_run),
        rap: f.voice * voiced * (1.0 - f.steady) * f.syllables * grid,
        speak: f.voice * voiced * (1.0 - f.steady) * (1.0 - f.syllables) * (1.0 - f.hiss),
        whisper: f.voice * unvoiced * f.hiss,
    };

    let sum = s.sum();
    if sum > 1e-6 {
        let k = f.voice / sum;
        s.sing *= k;
        s.rap *= k;
        s.speak *= k;
        s.whisper *= k;
    } else if f.voice > 0.0 {
        // Presence with nothing recognisable: call it speaking, so the four
        // still sum to voice.
        s.speak = f.voice;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_scores_sum_to_presence() {
        let f = Features {
            voice: 0.8,
            voicing: 0.6,
            steady: 0.4,
            syllables: 0.5,
            hiss: 0.2,
            voiced_run: 0.7,
            beat_phase: None,
        };
        let s = score(&f);
        assert!((s.sum() - 0.8).abs() < 1e-4, "sum {} != voice 0.8", s.sum());
    }

    #[test]
    fn a_steady_voiced_history_is_sing() {
        let f = Features {
            voice: 0.9,
            voicing: 0.95,
            steady: 0.95,
            syllables: 0.05,
            hiss: 0.0,
            voiced_run: 1.0,
            beat_phase: None,
        };
        let s = score(&f);
        assert!(s.sing > s.rap && s.sing > s.speak && s.sing > s.whisper, "{s:?}");
    }

    #[test]
    fn unvoiced_hiss_is_whisper() {
        let f = Features {
            voice: 0.7,
            voicing: 0.05,
            steady: 0.0,
            syllables: 0.1,
            hiss: 0.6,
            voiced_run: 0.6,
            beat_phase: None,
        };
        let s = score(&f);
        assert!(s.whisper > s.sing && s.whisper > s.rap && s.whisper > s.speak, "{s:?}");
    }
}
