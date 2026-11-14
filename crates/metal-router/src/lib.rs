//! **The Router, as a model.** What goes on air, decided from what the music
//! actually is rather than from a clock and a fairness score.
//!
//! ```text
//!   the digest            this crate                    metal-dash
//!   ──────────            ──────────                    ───────
//!   (T, 16) rows  ──►  features::describe  ──►  a passage, in 12 numbers
//!                       segment::sections   ──►  where the song changes
//!                       learn::Weights      ──►  which animation, per section
//!                                                        │
//!                       take::Take  ◄── the studio ──────┘  and what it
//!                       (the dataset)      authors            should have been
//! ```
//!
//! # What was here before, and why it was not enough
//!
//! `metal-dash::router` picks four animations every five minutes by a fairness
//! score — plays, recency, pairing recency — and rotates within them every
//! sixteen beats. Nothing in that reads the music. It cannot: the only thing it
//! is given is a beat count.
//!
//! > *"depending on the musical genre some animations work better, and the
//! > current system goes more by seconds, beats and history than by the actual
//! > content of the song."*
//!
//! So the fast clock stays — sixteen beats is still a phrase and a phrase is
//! still where a switch should land — and **what** goes on air stops being a
//! function of history and becomes a function of the audio. The two clocks were
//! never the wrong idea; they were the wrong *authority*.
//!
//! # The shape of the output
//!
//! > *"a single output that varies little, only at certain important changes
//! > in the song."*
//!
//! One animation, held across a section, changing at section boundaries. That
//! is why this is two stages and not one softmax per tick: [`segment`] finds
//! the boundaries with no training at all — a boundary is a fact about the
//! audio — and [`learn`] decides what goes in each one, which is where the
//! taste lives and is the only part with an opinion in it.
//!
//! Deciding per tick and smoothing afterwards would give the same shape by
//! accident, wobble at every boundary, and make "why did it change there"
//! unanswerable.
//!
//! # Curation is the operator's, and so is the blame
//!
//! > *"the automatic selection must be curated by the user; if the visual does
//! > not suit the genre, that would be bad curation the user forced."*
//!
//! [`Brain::decide`] takes the set of animations it is **allowed** to use.
//! Forcing one is not a special case in the model — it is `Mode::Single`, and
//! the model is not consulted. An animation nobody authored for scores low
//! everywhere and is never chosen, which falls out of the fit rather than out
//! of a rule; see [`learn`].
//!
//! # Why Rust and not PyTorch
//!
//! The dance model (`docs/MODEL.md`) is a Conv1D+LSTM to be trained in Python,
//! and that is right for it: it is trained once, offline, and its output is a
//! file. This one is different in the way that decides the question — **it runs
//! at a venue, inside `metal-dash`, on a laptop with no internet**, and it is
//! retrained by an operator pressing a button in the studio. Shipping a Python
//! runtime to a booth to answer "which of twenty-two" is not a trade anybody
//! would make. The model is small enough that the whole of it — fit, cross
//! validation and inference — is a few hundred lines here.

pub mod features;
pub mod gate;
pub mod learn;
pub mod npy;
pub mod segment;
pub mod take;

use std::path::Path;

use features::{FEATURES, NAMES};
use learn::Weights;

/// Where the router's own state lives under the data directory.
pub const DIR: &str = "router";

/// One choice, and enough of the reasoning to argue with it.
#[derive(Clone, Debug)]
pub struct Decision {
    /// The animation's id.
    pub model: String,
    /// How sure, 0..1 — the softmax mass on the winner among what was allowed.
    pub confidence: f32,
    /// Everything allowed, best first, with its probability. The studio draws
    /// this so a near-miss is visible.
    pub ranked: Vec<(String, f32)>,
    /// The feature that pushed hardest toward the winner, and by how much.
    pub because: Option<(&'static str, f32)>,
}

/// The trained artefact, loaded once and asked many times.
#[derive(Clone, Debug, Default)]
pub struct Brain {
    pub weights: Weights,
}

impl Brain {
    /// Loads `data/router/model.json`, or an untrained brain if there is none.
    ///
    /// A missing file is the normal state of a fresh checkout and of a venue
    /// laptop nobody has authored on, so it is not an error — but
    /// [`Brain::trained`] is false and every caller has to say so rather than
    /// pretend. `metal-dash` refuses to enter model mode without it.
    pub fn load(dir: &Path) -> Brain {
        let weights = metal_vfs::read_to_string(take::model_path(dir))
            .ok()
            .and_then(|t| serde_json::from_str::<Weights>(&t).ok())
            .unwrap_or_default();
        Brain { weights }
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(&self.weights).map_err(|e| e.to_string())?;
        metal_vfs::write(take::model_path(dir), text).map_err(|e| e.to_string())
    }

    pub fn trained(&self) -> bool {
        self.weights.trained()
    }

    /// What should be on air over a passage described by `f`, out of `allowed`.
    ///
    /// `allowed` is the operator's curation. An empty list, or one naming
    /// nothing the model knows, gives `None` — the caller falls back to the
    /// clock-and-history router and the page says which one is deciding.
    pub fn decide(&self, f: &[f32; FEATURES], allowed: &[String]) -> Option<Decision> {
        if !self.trained() {
            return None;
        }
        let scores = self.weights.scores(f);

        // Restricted to the curated set and renormalised, so the confidence
        // reported is confidence *among what may play* — the probability mass
        // on an animation the operator has ticked off is not uncertainty about
        // this choice, it is a different question.
        let mut ranked: Vec<(String, f32)> = self
            .weights
            .classes
            .iter()
            .enumerate()
            .filter(|(_, id)| allowed.iter().any(|a| a == *id))
            .map(|(i, id)| (id.clone(), scores[i]))
            .collect();
        if ranked.is_empty() {
            return None;
        }
        let total: f32 = ranked.iter().map(|(_, p)| p).sum();
        if total > 0.0 {
            for (_, p) in ranked.iter_mut() {
                *p /= total;
            }
        }
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));

        let (model, confidence) = ranked[0].clone();
        let because = self
            .weights
            .classes
            .iter()
            .position(|c| *c == model)
            .map(|c| self.weights.why(c, f))
            .and_then(|w| w.first().map(|(i, v)| (NAMES[*i], *v)));

        Some(Decision { model, confidence, ranked, because })
    }

    /// Every section of a whole song, decided. What the studio draws under the
    /// lane so an operator can see the model's answer beside their own.
    pub fn walk(
        &self,
        matrix: &[[f32; features::COLUMNS]],
        min_seconds: f32,
        allowed: &[String],
    ) -> Vec<(segment::Section, Option<Decision>)> {
        segment::sections(matrix, min_seconds)
            .into_iter()
            .map(|s| {
                let f = features::describe(&matrix[s.from..s.to]);
                (s, self.decide(&f, allowed))
            })
            .collect()
    }
}

/// Trains from every take in `dir` against the songs in `audio`.
///
/// `classes` is the suite as it stands — every animation that could be chosen,
/// whether or not anything was authored for it. Passing only the authored ones
/// would make the model unable to say "not this one" about the rest, and
/// `learn`'s own test for that would have nothing to check.
pub fn train_from(
    dir: &Path,
    audio: &Path,
    classes: Vec<String>,
    min_seconds: f32,
) -> (Weights, Vec<String>) {
    let _ = min_seconds;
    let mut samples = Vec::new();
    let mut notes = Vec::new();
    for t in take::load_all(dir) {
        let path = crate::npy::base_path(audio, &t.song);
        match npy::load(&path) {
            Ok(matrix) => {
                let got = take::samples(&t, &matrix, &classes);
                if got.is_empty() {
                    notes.push(format!("{}: authored, but no span was long enough", t.song));
                }
                samples.extend(got);
            }
            // Named rather than skipped silently: "I authored that song and it
            // is not in the model" has to have an answer on the page.
            Err(e) => notes.push(format!("{}: {e}", t.song)),
        }
    }
    let weights = learn::train(&samples, classes, &learn::Options::default());
    (weights, notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use features::COLUMNS;

    fn brain() -> Brain {
        let mut samples = Vec::new();
        for s in 0..5 {
            let song = format!("s{s}");
            for k in 0..8 {
                let j = k as f32 * 0.01;
                let mut loud = [0.3f32; FEATURES];
                loud[0] = 0.85 + j;
                loud[7] = 0.9;
                let mut soft = [0.3f32; FEATURES];
                soft[0] = 0.2 + j;
                soft[7] = 0.1;
                samples.push(learn::Sample { song: song.clone(), f: loud, class: 0 });
                samples.push(learn::Sample { song: song.clone(), f: soft, class: 1 });
            }
        }
        Brain {
            weights: learn::train(
                &samples,
                vec!["turing".into(), "vaporgrid".into(), "oracle".into()],
                &learn::Options::default(),
            ),
        }
    }

    fn all() -> Vec<String> {
        vec!["turing".into(), "vaporgrid".into(), "oracle".into()]
    }

    #[test]
    fn an_untrained_brain_declines_to_decide_rather_than_guessing() {
        // The state of a fresh checkout and of every venue laptop before
        // anybody authors anything. `metal-dash` reads this to know it must stay
        // on the clock-and-history router and say so.
        let b = Brain::default();
        assert!(!b.trained());
        assert!(b.decide(&[0.5; FEATURES], &all()).is_none());
    }

    #[test]
    fn it_decides_and_says_why() {
        let b = brain();
        let mut f = [0.3f32; FEATURES];
        f[0] = 0.9;
        f[7] = 0.95;
        let d = b.decide(&f, &all()).expect("trained");
        assert_eq!(d.model, "turing");
        assert!(d.confidence > 0.8, "{}", d.confidence);
        let (name, _) = d.because.expect("a reason");
        assert!(name == "energy" || name == "pulse", "the reason was {name}");
    }

    /// The curation half. The model is not allowed to reach past what the
    /// operator ticked, and its confidence is confidence among those.
    #[test]
    fn it_stays_inside_what_the_operator_allowed() {
        let b = brain();
        let mut f = [0.3f32; FEATURES];
        f[0] = 0.9;
        f[7] = 0.95;
        let only = vec!["vaporgrid".to_string(), "oracle".to_string()];
        let d = b.decide(&f, &only).expect("trained");
        assert!(only.contains(&d.model), "it picked {} which was not allowed", d.model);
        assert_eq!(d.ranked.len(), 2);
        let total: f32 = d.ranked.iter().map(|(_, p)| p).sum();
        assert!((total - 1.0).abs() < 1e-4, "the restricted scores did not renormalise");
    }

    #[test]
    fn allowing_nothing_is_not_a_decision() {
        let b = brain();
        assert!(b.decide(&[0.5; FEATURES], &[]).is_none());
        assert!(b.decide(&[0.5; FEATURES], &["not-a-model".into()]).is_none());
    }

    #[test]
    fn walking_a_song_gives_one_decision_per_section_and_they_tile_it() {
        let b = brain();
        let mut matrix: Vec<[f32; COLUMNS]> = Vec::new();
        for block in 0..4 {
            for i in 0..600 {
                let mut r = [0.0f32; COLUMNS];
                if block % 2 == 0 {
                    r[0] = 0.8;
                    r[2] = if i % 4 == 0 { 0.9 } else { 0.02 };
                    r[4] = 0.9;
                } else {
                    r[0] = 0.3;
                    r[2] = 0.02;
                    r[1] = 0.7;
                    for k in 0..12 {
                        r[4 + k] = 0.5;
                    }
                }
                matrix.push(r);
            }
        }
        let walked = b.walk(&matrix, 20.0, &all());
        assert!(walked.len() >= 2, "a song of four blocks was not cut at all");
        assert_eq!(walked[0].0.from, 0);
        assert_eq!(walked.last().unwrap().0.to, matrix.len());
        for pair in walked.windows(2) {
            assert_eq!(pair[0].0.to, pair[1].0.from);
        }
        assert!(walked.iter().all(|(_, d)| d.is_some()), "a section got no decision");
    }

    #[test]
    fn the_same_passage_decides_the_same_way_twice() {
        let b = brain();
        let f = [0.44f32; FEATURES];
        let a = b.decide(&f, &all()).expect("trained");
        let c = b.decide(&f, &all()).expect("trained");
        assert_eq!(a.model, c.model);
        assert_eq!(a.confidence, c.confidence);
    }

    #[test]
    fn a_brain_survives_being_written_and_read_back() {
        let dir = std::env::temp_dir().join("metal-router-brain");
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let b = brain();
        b.save(&dir).expect("saves");
        let back = Brain::load(&dir);
        assert!(back.trained());
        let f = [0.61f32; FEATURES];
        assert_eq!(
            b.decide(&f, &all()).unwrap().model,
            back.decide(&f, &all()).unwrap().model
        );
    }

    #[test]
    fn loading_from_a_directory_with_nothing_in_it_is_an_untrained_brain() {
        let dir = std::env::temp_dir().join("metal-router-empty");
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        assert!(!Brain::load(&dir).trained());
    }
}
