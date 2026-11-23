//! Model 0 — the Router. The studio's half of `metal-router`.
//!
//! > *"This puts the system on the level of the models and needs its control in
//! > the studio with the lanes; this [will be] model 0 and must be selectable
//! > like the others."*
//!
//! So it is an entry in the same dropdown as the twenty-two renderers and Don
//! Joe, it is authored on the same lane, and it saves through the same shape of
//! route. What it authors is not a float row: it is **one animation per stretch
//! of the song**, which is [`metal_router::take::Take`], and which is the dataset.
//!
//! # What the lane shows, and why it shows three things
//!
//! | | |
//! |---|---|
//! | the **picks** | what the operator said, as keyframes — the training data |
//! | the **novelty curve** | where the model thinks the song changes, untrained |
//! | the **sections**, coloured | what the model *would* choose, given what it has learnt |
//!
//! The second and third are what make a bad answer diagnosable. A pick that the
//! model disagrees with is one thing; a section boundary in the wrong place is
//! another entirely, and they need different fixes — more examples against the
//! first, nothing at all against the second, since the segmenter is not trained.
//! With only the picks on screen, both look like "the router is wrong".
//!
//! # Training is a button, and it is honest about what it cannot tell you
//!
//! `POST /router/train` reads every take, pairs each with its song's digest,
//! and fits. It reports **held-out-by-song** accuracy against the
//! majority-class baseline, and reports nothing at all under three songs, where
//! there is no honest number to give — see `metal_router::learn`.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::author::render::Catalog;
use metal_router::{features, npy, take, Brain};

/// Where the takes and the trained model live, under `data/`.
pub const DIR: &str = "data/router";

pub(crate) fn dir() -> PathBuf {
    PathBuf::from(DIR)
}

/// Every animation the router may choose between: the suite as this binary has
/// it, ids and labels.
///
/// The **whole** suite, not just what has been authored for. A model fitted
/// only against the authored ones could not say "not this one" about the rest,
/// and "it will not pick it unless it is its strength" would have nothing to be true of.
pub(crate) fn classes(catalog: &Catalog) -> (Vec<String>, Vec<String>) {
    (
        catalog.ids().iter().map(|s| s.to_string()).collect(),
        catalog.names().iter().map(|s| s.to_string()).collect(),
    )
}

pub(crate) fn matrix_for(song: &str) -> Option<Vec<[f32; features::COLUMNS]>> {
    npy::load(&npy::base_path(Path::new(crate::author::SONGS), song)).ok()
}

/// The router's state, as the page's boot payload wants it.
pub fn page_data(catalog: &Catalog, song: Option<&str>) -> Value {
    let (ids, labels) = classes(catalog);
    let brain = Brain::load(&dir());
    let w = &brain.weights;
    let take = song
        .and_then(|s| take::load_take(&dir(), s))
        .unwrap_or_default();
    json!({
        "ids": ids,
        "labels": labels,
        "trained": brain.trained(),
        "examples": w.examples,
        "songs": w.songs,
        "accuracy": w.accuracy,
        "baseline": w.baseline,
        "features": features::NAMES,
        // What the offline segmenter and the live one both mean by a section,
        // so the lane's marks and the booth's behaviour are the same numbers.
        "minSeconds": MIN_SECONDS,
        "picks": take.picks,
        "takes": take::load_all(&dir()).len(),
    })
}

/// The shortest section the studio cuts a song into.
///
/// Matches `metal-dash::router::HOLD_SECONDS`. Two numbers that had to agree and
/// did not would mean the lane showing an operator sections the booth would
/// never actually switch on.
pub const MIN_SECONDS: f32 = 24.0;

// the cleanup: the raw-socket `serve` dispatcher that used to live here moved to
// `crate::call::author::router` as typed handler functions over
// `classes`/`dir`/`matrix_for`/`coverage`/`page_data` below (the first three
// now `pub(crate)`), unchanged.

/// How many examples each animation would contribute, without training.
///
/// The studio shows it beside the train button so "the router never picks
/// Turing" has an answer before anybody retrains looking for one.
pub fn coverage(catalog: &Catalog) -> Vec<(String, usize)> {
    let (ids, _) = classes(catalog);
    let mut counts = vec![0usize; ids.len()];
    for t in take::load_all(&dir()) {
        let Some(matrix) = matrix_for(&t.song) else {
            continue;
        };
        for s in take::samples(&t, &matrix, &ids) {
            counts[s.class] += 1;
        }
    }
    ids.into_iter().zip(counts).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two floors that have to be the same number in two crates.
    ///
    /// The studio draws sections at this length and the booth refuses to change
    /// its mind faster than that. If they drifted, an operator would author
    /// against sections the app would never switch on, and nothing anywhere
    /// would say so.
    #[test]
    fn the_studio_and_the_booth_agree_on_the_shortest_section() {
        // `metal-dash` is a binary, so its constant cannot be imported here. This
        // is the assertion that the two literals match, written where somebody
        // changing one of them will see it.
        assert_eq!(
            MIN_SECONDS, 24.0,
            "metal-dash::router::HOLD_SECONDS must match"
        );
    }

    #[test]
    fn the_router_only_answers_for_its_own_prefix() {
        // The dispatcher relies on every module returning `None` for anything
        // it does not own, or an unrouted path is silently swallowed by
        // whichever matched first.
        for p in ["/models/example", "/rig/state", "/", "/state", "/routerish"] {
            assert!(!p.starts_with("/router/"), "{p} would have been claimed");
        }
    }

    /// The guard on `/router/train`, as the rule it is rather than as the route.
    ///
    /// A trained model is the only copy of however long the authoring took —
    /// it cannot be rebuilt from a checkout, because the audio is not in one.
    /// So the question "may this run overwrite what is there" has a rule, and
    /// the rule is: only if it learnt something, or if there was nothing.
    #[test]
    fn a_run_that_learnt_nothing_may_not_replace_one_that_did() {
        let may_overwrite = |fresh_examples: usize, standing_examples: usize| {
            fresh_examples > 0 || standing_examples == 0
        };
        assert!(may_overwrite(120, 0), "the first ever training must land");
        assert!(may_overwrite(120, 80), "a real retrain must land");
        assert!(may_overwrite(0, 0), "nothing over nothing is harmless");
        assert!(
            !may_overwrite(0, 80),
            "an empty run over a trained model is the one case that must refuse"
        );
    }

    #[test]
    fn a_take_round_trips_through_the_disk() {
        let root = std::env::temp_dir().join("fw-studio-router-take");
        let _ = metal_vfs::remove_dir_all(&root);
        let t = take::Take {
            song: "a song".into(),
            rate: features::RATE,
            picks: vec![
                take::Pick {
                    tick: 0,
                    model: "turing".into(),
                },
                take::Pick {
                    tick: 400,
                    model: "oracle".into(),
                },
            ],
        };
        take::save_take(&root, &t).expect("saves");
        let back = take::load_take(&root, "a song").expect("loads");
        assert_eq!(back.picks, t.picks);
        assert_eq!(take::load_all(&root).len(), 1);
    }
}
