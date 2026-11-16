//! The dataset: what an operator said should be on air, and when.
//!
//! One file per song, in `data/router/takes/<song>.json`, holding a list of
//! **picks** — a tick, and the animation that should be playing from it until
//! the next one. That is precisely what the studio's lane authors, which is not
//! a coincidence: the lane already stores "a value at a tick", and the whole
//! reason the router is model 0 in the studio rather than a screen of its own
//! is that its dataset is the same shape as every other model's example.
//!
//! # A pick is a span, and a span is several examples
//!
//! A two-minute pick is one statement by the operator and it is also two
//! minutes of audio, so it becomes several overlapping windows, each described
//! and each labelled with that pick. The label genuinely holds over the whole
//! span — the operator said so — which is what makes this augmentation and not
//! invention.
//!
//! It also means windows out of one song are **not independent**, and
//! [`crate::learn::train`] holds out whole songs for exactly that reason.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::features::{describe_at, COLUMNS, RATE};
use crate::learn::Sample;

/// How long a described window is, in seconds, converted to ticks at `rate`:
/// long enough that `pulso` and `densidad` mean something, short enough that a
/// two-minute section yields more than a handful of them.
pub const fn window(rate: metal_schema::TickRate) -> usize {
    12 * rate.hz() as usize
}
/// The shortest span worth one example at all, in seconds at `rate`. Below this
/// there is not enough audio to describe.
pub const fn min_span(rate: metal_schema::TickRate) -> usize {
    4 * rate.hz() as usize
}
/// The base-grid window and minimum span. A fine caller passes
/// `TickRate::Fine` to [`window`]/[`min_span`].
pub const WINDOW: usize = window(metal_schema::TickRate::Base);
/// How far apart the windows start. Half a window, so consecutive ones overlap
/// and a section boundary never falls between two of them unexamined.
pub const STRIDE: usize = WINDOW / 2;
pub const MIN_SPAN: usize = min_span(metal_schema::TickRate::Base);

/// One statement: from this tick, this animation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Pick {
    pub tick: usize,
    /// A model **id**, not an index — see [`crate::learn::Weights::classes`].
    pub model: String,
}

/// One song's authoring.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Take {
    pub song: String,
    #[serde(default = "default_rate")]
    pub rate: f32,
    #[serde(default)]
    pub picks: Vec<Pick>,
}

fn default_rate() -> f32 {
    RATE
}

impl Take {
    /// The picks in tick order, with duplicates at the same tick resolved to
    /// the last one written. Everything below goes through this rather than
    /// trusting the file's order.
    pub fn ordered(&self) -> Vec<Pick> {
        let mut out = self.picks.clone();
        out.sort_by_key(|p| p.tick);
        out.dedup_by_key(|p| p.tick);
        out
    }

    /// What should be on air at `tick`. `None` before the first pick — which
    /// is a real state and not an error: an operator who has authored the
    /// chorus and not the intro has said nothing about the intro.
    pub fn model_at(&self, tick: usize) -> Option<String> {
        let mut found = None;
        for p in self.ordered() {
            if p.tick <= tick {
                found = Some(p.model);
            } else {
                break;
            }
        }
        found
    }

    /// The picks as spans over a song `ticks` long: `(from, to, model)`.
    pub fn spans(&self, ticks: usize) -> Vec<(usize, usize, String)> {
        let picks = self.ordered();
        let mut out = Vec::with_capacity(picks.len());
        for (i, p) in picks.iter().enumerate() {
            if p.tick >= ticks {
                break;
            }
            let to = picks.get(i + 1).map(|n| n.tick.min(ticks)).unwrap_or(ticks);
            if to > p.tick {
                out.push((p.tick, to, p.model.clone()));
            }
        }
        out
    }
}

/// Every training example this take and its digest matrix make.
///
/// `classes` is the id → index mapping the model is being fitted under; a pick
/// naming an animation that is not in it is skipped rather than fatal, because
/// an animation can be removed from the suite after somebody authored against
/// it and that must not make the whole corpus unloadable.
pub fn samples(take: &Take, matrix: &[[f32; COLUMNS]], classes: &[String]) -> Vec<Sample> {
    // The take's own grid: an authored take carries a `rate`, so the windows
    // are `12 s × rate` and `4 s × rate` and `describe_at` reads them at that
    // level. A take written before the rate field defaults to [`RATE`].
    let rate = metal_schema::TickRate::from_hz(take.rate as u32).unwrap_or(metal_schema::TickRate::Base);
    let window = window(rate);
    let min_span = min_span(rate);
    let stride = (window / 2).max(1);
    let mut out = Vec::new();
    for (from, to, model) in take.spans(matrix.len()) {
        let Some(class) = classes.iter().position(|c| *c == model) else { continue };
        let span = to - from;
        if span < min_span {
            continue;
        }
        if span < window {
            out.push(Sample {
                song: take.song.clone(),
                f: describe_at(&matrix[from..to], rate),
                class,
            });
            continue;
        }
        let mut at = from;
        while at + window <= to {
            out.push(Sample {
                song: take.song.clone(),
                f: describe_at(&matrix[at..at + window], rate),
                class,
            });
            at += stride;
        }
        // The tail, if the last stride left more than a minimum span of it —
        // otherwise the end of every section is never described, and the end of
        // a section is where an operator's choice is most deliberate.
        if to - at >= min_span && at > from {
            out.push(Sample {
                song: take.song.clone(),
                f: describe_at(&matrix[to - window.min(to - from)..to], rate),
                class,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// on disk
// ---------------------------------------------------------------------------

/// Refuses a name that would write outside the takes folder. The song name is
/// a file stem taken from `data/audio`, so this should never fire — which is
/// the reason to check it rather than not to.
pub fn safe_name(song: &str) -> bool {
    !song.is_empty()
        && song.len() < 200
        && !song.contains(['/', '\\', '\0'])
        && song != "."
        && song != ".."
}

pub fn take_path(dir: &Path, song: &str) -> PathBuf {
    dir.join("takes").join(format!("{song}.json"))
}

pub fn model_path(dir: &Path) -> PathBuf {
    dir.join("model.json")
}

pub fn load_take(dir: &Path, song: &str) -> Option<Take> {
    let text = metal_vfs::read_to_string(take_path(dir, song)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_take(dir: &Path, take: &Take) -> Result<(), String> {
    if !safe_name(&take.song) {
        return Err(format!("not a song name: {}", take.song));
    }
    let path = take_path(dir, &take.song);
    if let Some(parent) = path.parent() {
        metal_vfs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(take).map_err(|e| e.to_string())?;
    metal_vfs::write(&path, text).map_err(|e| e.to_string())
}

/// Every take on disk, in a stable order so a training run is reproducible.
pub fn load_all(dir: &Path) -> Vec<Take> {
    let mut out: Vec<Take> = metal_vfs::read_dir(dir.join("takes"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| metal_vfs::read_to_string(p).ok())
        .filter_map(|t| serde_json::from_str::<Take>(&t).ok())
        .filter(|t| !t.picks.is_empty())
        .collect();
    out.sort_by(|a, b| a.song.cmp(&b.song));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(n: usize) -> Vec<[f32; COLUMNS]> {
        (0..n)
            .map(|i| {
                let mut r = [0.0f32; COLUMNS];
                r[0] = 0.4 + 0.3 * ((i as f32) * 0.01).sin();
                r[2] = if i % 4 == 0 { 0.8 } else { 0.05 };
                r[4] = 0.7;
                r
            })
            .collect()
    }

    fn take(picks: &[(usize, &str)]) -> Take {
        Take {
            song: "song".into(),
            rate: RATE,
            picks: picks.iter().map(|(t, m)| Pick { tick: *t, model: (*m).into() }).collect(),
        }
    }

    #[test]
    fn a_pick_holds_until_the_next_one() {
        let t = take(&[(0, "a"), (400, "b"), (900, "c")]);
        assert_eq!(t.model_at(0).as_deref(), Some("a"));
        assert_eq!(t.model_at(399).as_deref(), Some("a"));
        assert_eq!(t.model_at(400).as_deref(), Some("b"));
        assert_eq!(t.model_at(5000).as_deref(), Some("c"));
    }

    #[test]
    fn nothing_authored_yet_is_nothing_rather_than_the_first_animation() {
        // An operator who has done the chorus and not the intro has said
        // nothing about the intro, and a default would put words in their mouth.
        let t = take(&[(400, "b")]);
        assert_eq!(t.model_at(0), None);
        assert_eq!(t.model_at(399), None);
        assert_eq!(t.model_at(400).as_deref(), Some("b"));
    }

    #[test]
    fn the_picks_are_read_in_tick_order_however_the_file_is_written() {
        let t = take(&[(900, "c"), (0, "a"), (400, "b")]);
        let spans = t.spans(1200);
        assert_eq!(spans.iter().map(|s| s.0).collect::<Vec<_>>(), vec![0, 400, 900]);
    }

    #[test]
    fn spans_tile_the_song_and_stop_at_its_end() {
        let t = take(&[(0, "a"), (400, "b")]);
        let spans = t.spans(1000);
        assert_eq!(spans, vec![(0, 400, "a".into()), (400, 1000, "b".into())]);
        // A pick past the end of the audio contributes nothing rather than a
        // reversed span.
        let past = take(&[(0, "a"), (5000, "b")]);
        assert_eq!(past.spans(1000), vec![(0, 1000, "a".into())]);
    }

    #[test]
    fn a_long_pick_becomes_several_overlapping_examples() {
        let m = rows(1600);
        let t = take(&[(0, "a")]);
        let s = samples(&t, &m, &["a".into()]);
        assert!(s.len() > 10, "a three-minute pick made only {} examples", s.len());
        assert!(s.iter().all(|x| x.class == 0));
        assert!(s.iter().all(|x| x.song == "song"));
    }

    #[test]
    fn a_pick_too_short_to_describe_makes_no_example_at_all() {
        // Better than a description built on two seconds of audio, which would
        // be a label attached to noise.
        let m = rows(1600);
        let t = take(&[(0, "a"), (10, "b")]);
        let s = samples(&t, &m, &["a".into(), "b".into()]);
        assert!(!s.is_empty(), "the long pick produced nothing either");
        assert!(
            s.iter().all(|x| x.class == 1),
            "the ten-tick pick at the head of the song produced an example"
        );
    }

    #[test]
    fn a_pick_naming_an_animation_that_no_longer_exists_is_skipped_not_fatal() {
        // An animation can leave the suite after somebody authored against it,
        // and the corpus has to survive that.
        let m = rows(1600);
        let t = take(&[(0, "gone"), (800, "a")]);
        let s = samples(&t, &m, &["a".into()]);
        assert!(!s.is_empty());
        assert!(s.iter().all(|x| x.class == 0));
    }

    #[test]
    fn a_take_survives_a_round_trip_through_json() {
        let t = take(&[(0, "a"), (400, "b")]);
        let text = serde_json::to_string(&t).unwrap();
        let back: Take = serde_json::from_str(&text).unwrap();
        assert_eq!(back.picks, t.picks);
        assert_eq!(back.song, t.song);
    }

    #[test]
    fn a_file_written_without_a_rate_still_loads() {
        let back: Take =
            serde_json::from_str(r#"{"song":"x","picks":[{"tick":0,"model":"a"}]}"#).unwrap();
        assert_eq!(back.rate, RATE);
    }

    #[test]
    fn a_song_name_cannot_escape_the_takes_folder() {
        assert!(safe_name("Daft Punk - One More Time"));
        assert!(!safe_name("../../etc/passwd"));
        assert!(!safe_name(""));
        assert!(!safe_name(".."));
    }
}
