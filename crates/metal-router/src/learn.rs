//! The trained half: twelve numbers describing a passage → which animation.
//!
//! # Multinomial logistic regression, and why not something bigger
//!
//! The corpus this fits is one person authoring a few dozen songs, which after
//! windowing is on the order of a thousand examples over a dozen features and
//! twenty-odd classes. That is a **small-data** problem, and the model has to
//! be chosen for the data rather than for the ask's ambition:
//!
//! - It is convex, so training either converges or the data is degenerate.
//!   There is no run-to-run variance to mistake for a curation problem.
//! - Its weights are twelve numbers per animation with the features' own names
//!   on them, so "why did it pick that" is answerable by reading them, which is
//!   what makes bad curation *visible* rather than mysterious.
//! - It extrapolates smoothly. The ask asks for this directly — *"if the user
//!   forces an animation that doesn't match the genre, it should still work
//!   relatively normally, generalising from the data it did have"* — and a
//!   linear score is the model that generalises most gently
//!   off the end of its data. A deep net asked about a genre it never saw
//!   answers confidently and arbitrarily.
//!
//! **An animation with no examples scores low everywhere**, which is exactly
//! *"the router won't put it on if it isn't its strong suit"*, and it happens
//! for free: the other classes' gradients push its logit down and nothing
//! pushes it back up. It is not a rule that had to be written, and it is why an
//! operator can author for three animations and get a router that only uses
//! those three.
//!
//! # Held out by song, never by window
//!
//! Windows out of one song are not independent examples — they are the same
//! four minutes of audio described twelve times — so a split that puts some of
//! a song's windows in train and the rest in test reports an accuracy that is
//! mostly memorisation. `docs/CORPUS.md` recorded this for the dance model and
//! it is the same trap here. [`train`] holds out **whole songs**, one at a
//! time, and reports the mean against a majority-class baseline, so a number
//! that only looks good is visibly not better than guessing.

use serde::{Deserialize, Serialize};

use crate::features::FEATURES;

/// One authored passage: what it sounded like, what went on it, and which song
/// it came out of — the last so it can be held out as a unit.
#[derive(Clone, Debug)]
pub struct Sample {
    pub song: String,
    pub f: [f32; FEATURES],
    pub class: usize,
}

/// What training produced. Serialised whole to `data/router/model.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Weights {
    /// Animation ids, in the order the rows below are in. **Ids, not indices**:
    /// the suite's order is append-only but a model can still be removed, and a
    /// file that named row 7 would then be pointing at a different animation.
    pub classes: Vec<String>,
    /// One row of [`FEATURES`] weights per class.
    pub w: Vec<Vec<f32>>,
    pub b: Vec<f32>,
    /// The standardisation the features were fitted under. Inference must apply
    /// the same one or every weight means something else.
    pub mean: Vec<f32>,
    pub scale: Vec<f32>,
    pub examples: usize,
    pub songs: usize,
    /// Mean leave-one-song-out accuracy, or `None` with fewer than three songs
    /// — where there is nothing honest to report and a number would be worse
    /// than a blank.
    pub accuracy: Option<f32>,
    /// What always guessing the commonest animation would have scored. Printed
    /// beside the accuracy, because an accuracy without it is unreadable.
    pub baseline: Option<f32>,
}

impl Weights {
    pub fn trained(&self) -> bool {
        !self.classes.is_empty() && self.examples > 0
    }

    /// The probability of each class for one passage, in `classes` order.
    ///
    /// Uniform from an untrained model rather than a panic or a zero vector: a
    /// venue laptop that has never been trained has to keep drawing, and
    /// `Brain::decide` is what turns "no opinion" into "say so and fall back".
    pub fn scores(&self, f: &[f32; FEATURES]) -> Vec<f32> {
        if !self.trained() {
            return vec![1.0 / self.classes.len().max(1) as f32; self.classes.len()];
        }
        let x = self.standardise(f);
        let mut logits: Vec<f32> = self
            .w
            .iter()
            .zip(self.b.iter())
            .map(|(row, bias)| bias + row.iter().zip(x.iter()).map(|(a, b)| a * b).sum::<f32>())
            .collect();
        softmax(&mut logits);
        logits
    }

    fn standardise(&self, f: &[f32; FEATURES]) -> [f32; FEATURES] {
        let mut out = [0.0f32; FEATURES];
        for i in 0..FEATURES {
            let m = self.mean.get(i).copied().unwrap_or(0.0);
            let s = self.scale.get(i).copied().unwrap_or(1.0);
            out[i] = (f[i] - m) / if s.abs() < 1e-6 { 1.0 } else { s };
        }
        out
    }

    /// Which feature moved a class's score most, and by how much. What the
    /// studio prints under a pick, so "why that one" has an answer.
    pub fn why(&self, class: usize, f: &[f32; FEATURES]) -> Vec<(usize, f32)> {
        if class >= self.w.len() {
            return Vec::new();
        }
        let x = self.standardise(f);
        let mut out: Vec<(usize, f32)> =
            (0..FEATURES).map(|i| (i, self.w[class][i] * x[i])).collect();
        out.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()));
        out
    }
}

fn softmax(v: &mut [f32]) {
    let top = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if !top.is_finite() {
        let n = v.len().max(1) as f32;
        v.iter_mut().for_each(|x| *x = 1.0 / n);
        return;
    }
    let mut sum = 0.0;
    for x in v.iter_mut() {
        *x = (*x - top).exp();
        sum += *x;
    }
    let sum = if sum > 0.0 { sum } else { 1.0 };
    v.iter_mut().for_each(|x| *x /= sum);
}

/// How hard to fit. Exposed so the studio's train button can be a button rather
/// than a form; nothing in the app changes them.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub epochs: usize,
    pub lr: f32,
    pub l2: f32,
}

impl Default for Options {
    fn default() -> Options {
        // Full-batch on a thousand examples of twelve features is microseconds
        // an epoch; the count is set by where the loss stops moving, not by a
        // budget.
        Options { epochs: 900, lr: 0.5, l2: 2e-3 }
    }
}

/// Fits one model, without holding anything out.
fn fit(
    samples: &[Sample],
    classes: usize,
    mean: &[f32],
    scale: &[f32],
    opt: &Options,
) -> (Vec<Vec<f32>>, Vec<f32>) {
    let mut w = vec![vec![0.0f32; FEATURES]; classes];
    let mut b = vec![0.0f32; classes];
    if samples.is_empty() || classes == 0 {
        return (w, b);
    }

    let x: Vec<[f32; FEATURES]> = samples
        .iter()
        .map(|s| {
            let mut row = [0.0f32; FEATURES];
            for i in 0..FEATURES {
                let sc = if scale[i].abs() < 1e-6 { 1.0 } else { scale[i] };
                row[i] = (s.f[i] - mean[i]) / sc;
            }
            row
        })
        .collect();

    let n = samples.len() as f32;
    let mut logits = vec![0.0f32; classes];
    for _ in 0..opt.epochs {
        let mut gw = vec![[0.0f32; FEATURES]; classes];
        let mut gb = vec![0.0f32; classes];
        for (row, s) in x.iter().zip(samples.iter()) {
            for c in 0..classes {
                logits[c] =
                    b[c] + w[c].iter().zip(row.iter()).map(|(a, v)| a * v).sum::<f32>();
            }
            softmax(&mut logits);
            for c in 0..classes {
                // dL/dlogit_c = p_c - [c == label]
                let d = logits[c] - if c == s.class { 1.0 } else { 0.0 };
                gb[c] += d;
                for i in 0..FEATURES {
                    gw[c][i] += d * row[i];
                }
            }
        }
        for c in 0..classes {
            b[c] -= opt.lr * gb[c] / n;
            for i in 0..FEATURES {
                // L2 on the weights and not on the bias: penalising the bias
                // would pull every class toward equal prior, which is the one
                // thing the model is supposed to learn from an operator who
                // uses one animation far more than another.
                w[c][i] -= opt.lr * (gw[c][i] / n + opt.l2 * w[c][i]);
            }
        }
    }
    (w, b)
}

fn standardisation(samples: &[Sample]) -> (Vec<f32>, Vec<f32>) {
    let n = samples.len().max(1) as f32;
    let mut mean = vec![0.0f32; FEATURES];
    for s in samples {
        for i in 0..FEATURES {
            mean[i] += s.f[i] / n;
        }
    }
    let mut scale = vec![0.0f32; FEATURES];
    for s in samples {
        for i in 0..FEATURES {
            let d = s.f[i] - mean[i];
            scale[i] += d * d / n;
        }
    }
    for v in scale.iter_mut() {
        *v = v.sqrt();
        if *v < 1e-6 {
            // A feature every example agrees on carries nothing; leaving the
            // scale at 1 makes it a constant zero after centring rather than a
            // division by nothing.
            *v = 1.0;
        }
    }
    (mean, scale)
}

fn predict(w: &[Vec<f32>], b: &[f32], mean: &[f32], scale: &[f32], f: &[f32; FEATURES]) -> usize {
    let mut best = (f32::NEG_INFINITY, 0usize);
    for c in 0..w.len() {
        let mut z = b[c];
        for i in 0..FEATURES {
            let sc = if scale[i].abs() < 1e-6 { 1.0 } else { scale[i] };
            z += w[c][i] * (f[i] - mean[i]) / sc;
        }
        if z > best.0 {
            best = (z, c);
        }
    }
    best.1
}

/// Trains on everything, and reports how well it would have done on a song it
/// had not seen.
pub fn train(samples: &[Sample], classes: Vec<String>, opt: &Options) -> Weights {
    if samples.is_empty() || classes.is_empty() {
        return Weights { classes, ..Weights::default() };
    }
    let (mean, scale) = standardisation(samples);

    let mut songs: Vec<&str> = samples.iter().map(|s| s.song.as_str()).collect();
    songs.sort_unstable();
    songs.dedup();

    // Leave one song out. Not one window out — windows from one song are the
    // same audio described several times, and holding one out measures nothing.
    let (accuracy, baseline) = if songs.len() >= 3 {
        let mut right = 0usize;
        let mut total = 0usize;
        let mut naive = 0usize;
        for held in &songs {
            let train_on: Vec<Sample> =
                samples.iter().filter(|s| s.song != *held).cloned().collect();
            let test: Vec<&Sample> = samples.iter().filter(|s| s.song == *held).collect();
            if train_on.is_empty() || test.is_empty() {
                continue;
            }
            let (m, sc) = standardisation(&train_on);
            let (w, b) = fit(&train_on, classes.len(), &m, &sc, opt);
            // The commonest class in what it was allowed to see.
            let mut counts = vec![0usize; classes.len()];
            for s in &train_on {
                counts[s.class] += 1;
            }
            let common = counts
                .iter()
                .enumerate()
                .max_by_key(|(_, n)| **n)
                .map(|(i, _)| i)
                .unwrap_or(0);
            for s in test {
                if predict(&w, &b, &m, &sc, &s.f) == s.class {
                    right += 1;
                }
                if s.class == common {
                    naive += 1;
                }
                total += 1;
            }
        }
        if total == 0 {
            (None, None)
        } else {
            (
                Some(right as f32 / total as f32),
                Some(naive as f32 / total as f32),
            )
        }
    } else {
        (None, None)
    };

    let (w, b) = fit(samples, classes.len(), &mean, &scale, opt);
    Weights {
        classes,
        w,
        b,
        mean,
        scale,
        examples: samples.len(),
        songs: songs.len(),
        accuracy,
        baseline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(song: &str, class: usize, loud: f32, regular: f32) -> Sample {
        let mut f = [0.3f32; FEATURES];
        f[0] = loud;
        f[7] = regular;
        Sample { song: song.into(), f, class }
    }

    /// Two obviously separable kinds of passage. If this does not fit, nothing
    /// downstream is worth debugging.
    fn separable() -> Vec<Sample> {
        let mut out = Vec::new();
        for s in 0..6 {
            let song = format!("s{s}");
            for k in 0..8 {
                let j = k as f32 * 0.01;
                out.push(sample(&song, 0, 0.80 + j, 0.90 - j));
                out.push(sample(&song, 1, 0.25 + j, 0.10 + j));
            }
        }
        out
    }

    #[test]
    fn an_untrained_model_has_no_opinion_rather_than_a_wrong_one() {
        // The state a venue laptop is in before anybody has authored anything.
        // Zeros, a panic, or a confident class 0 would each be a different
        // wrong answer; a flat distribution is the only honest one.
        let w = Weights { classes: vec!["a".into(), "b".into(), "c".into()], ..Weights::default() };
        assert!(!w.trained());
        let s = w.scores(&[0.5; FEATURES]);
        assert_eq!(s.len(), 3);
        assert!(s.iter().all(|p| (p - 1.0 / 3.0).abs() < 1e-6));
    }

    #[test]
    fn it_learns_a_separable_split() {
        let w = train(&separable(), vec!["loud".into(), "quiet".into()], &Options::default());
        assert!(w.trained());
        let loud = w.scores(&{
            let mut f = [0.3f32; FEATURES];
            f[0] = 0.85;
            f[7] = 0.92;
            f
        });
        assert!(loud[0] > 0.9, "did not learn the loud class: {loud:?}");
        let quiet = w.scores(&{
            let mut f = [0.3f32; FEATURES];
            f[0] = 0.22;
            f[7] = 0.08;
            f
        });
        assert!(quiet[1] > 0.9, "did not learn the quiet class: {quiet:?}");
    }

    #[test]
    fn probabilities_are_probabilities() {
        let w = train(&separable(), vec!["a".into(), "b".into()], &Options::default());
        for probe in [0.0f32, 0.5, 1.0] {
            let s = w.scores(&[probe; FEATURES]);
            let total: f32 = s.iter().sum();
            assert!((total - 1.0).abs() < 1e-4, "scores summed to {total}");
            assert!(s.iter().all(|p| (0.0..=1.0).contains(p) && p.is_finite()));
        }
    }

    /// *"the router won't put it on if it isn't its strong suit."* An animation
    /// nobody authored anything for must lose everywhere, and it has to fall
    /// out of the fit rather than out of a rule.
    #[test]
    fn an_animation_with_no_examples_is_never_the_answer() {
        let w = train(
            &separable(),
            vec!["loud".into(), "quiet".into(), "unused".into()],
            &Options::default(),
        );
        let mut worst: f32 = 0.0;
        for a in 0..6 {
            for b in 0..6 {
                let mut f = [0.3f32; FEATURES];
                f[0] = a as f32 / 5.0;
                f[7] = b as f32 / 5.0;
                let s = w.scores(&f);
                worst = worst.max(s[2]);
                assert!(
                    s[2] < s[0] || s[2] < s[1],
                    "an unauthored animation beat both authored ones at ({a},{b})"
                );
            }
        }
        assert!(worst < 0.2, "an unauthored animation reached {worst:.2} somewhere");
    }

    /// The other half of the same ask: forced onto music it was never trained
    /// for, it still has to produce a sane ordering rather than a nonsense one.
    #[test]
    fn it_extrapolates_gently_off_the_end_of_its_data() {
        let w = train(&separable(), vec!["loud".into(), "quiet".into()], &Options::default());
        // Louder and more regular than anything it saw.
        let mut f = [0.3f32; FEATURES];
        f[0] = 1.0;
        f[7] = 1.0;
        let s = w.scores(&f);
        assert!(s[0] > s[1], "past the end of the data it flipped its answer");
        assert!(s.iter().all(|p| p.is_finite()));
    }

    #[test]
    fn accuracy_is_held_out_by_song_and_compared_against_guessing() {
        let w = train(&separable(), vec!["a".into(), "b".into()], &Options::default());
        let acc = w.accuracy.expect("six songs is enough to hold some out");
        let base = w.baseline.expect("a baseline comes with an accuracy");
        assert!(acc > 0.95, "a separable problem scored {acc}");
        assert!(base <= 0.55, "the baseline on a balanced problem should be about half");
        assert_eq!(w.songs, 6);
    }

    #[test]
    fn two_songs_report_no_accuracy_rather_than_a_flattering_one() {
        // With two songs, leave-one-out is trained on one song. A number from
        // that would be noise wearing a percentage sign.
        let few: Vec<Sample> =
            separable().into_iter().filter(|s| s.song == "s0" || s.song == "s1").collect();
        let w = train(&few, vec!["a".into(), "b".into()], &Options::default());
        assert!(w.trained());
        assert_eq!(w.accuracy, None);
        assert_eq!(w.baseline, None);
    }

    #[test]
    fn a_model_survives_a_round_trip_through_json() {
        // It is a file on a venue laptop; if this drifted, the thing that got
        // trained and the thing that runs would be different models.
        let w = train(&separable(), vec!["a".into(), "b".into()], &Options::default());
        let text = serde_json::to_string(&w).expect("serialises");
        let back: Weights = serde_json::from_str(&text).expect("parses");
        let probe = [0.42f32; FEATURES];
        assert_eq!(w.scores(&probe), back.scores(&probe));
    }

    #[test]
    fn why_names_the_feature_that_decided_it() {
        // The whole point of a linear model here. If this were unreadable, a
        // bad pick would be unexplainable and the operator could not tell their
        // own curation from a bug.
        let w = train(&separable(), vec!["loud".into(), "quiet".into()], &Options::default());
        let mut f = [0.3f32; FEATURES];
        f[0] = 0.9;
        f[7] = 0.95;
        let why = w.why(0, &f);
        assert_eq!(why.len(), FEATURES);
        assert!(
            why[0].0 == 0 || why[0].0 == 7,
            "the two features that carry the split were not on top: {:?}",
            &why[..3]
        );
    }

    #[test]
    fn training_on_nothing_gives_a_model_that_says_it_is_untrained() {
        let w = train(&[], vec!["a".into()], &Options::default());
        assert!(!w.trained());
        assert_eq!(w.examples, 0);
    }
}
