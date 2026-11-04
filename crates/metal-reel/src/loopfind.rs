//! **The rhythm finder** — an auxiliary of the Colourist that says whether the
//! music is looping right now, and over how many beats.
//!
//! It reads the same digest columns the Colourist reads, and nothing else (the
//! repeat detector is not a digest column, so it is not an input here). Loops
//! are beat-based: the pattern lives in the onsets and the registered bpm, so a
//! length is a beat count. Per tick it answers two things:
//!
//! - **strength** in `0..=1`: how sure the fit is that a loop is playing;
//! - **length** in [`LENGTHS`] beats (`1/8` up to `8`): which of the seven the
//!   fit thinks it is, by argmax over seven one-vs-rest rows.
//!
//! The fit is the same ridge least-squares [`crate::grade`] uses, with the same
//! solver ([`crate::curate::solve`]) and the same ridge constant, so there is no
//! new dependency and no second notion of "fitted". Training labels come from
//! the approved loops examples (`metal-dash::author::dataset::loop_takes_for`),
//! per tick: a tick labelled with a length is a positive, a tick labelled `0` is
//! an explicit negative, and an unlabelled tick contributes nothing. One song
//! may change length, or stop looping, partway through.
//!
//! Like the Colourist, a fit that is missing or stale (a different length list,
//! or a wrong row count or width, such as the tick-based fits this replaced) is
//! refused on load and the finder answers "I do not know", never a guess from
//! weights fitted for something else.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::MUSIC;

/// The loop lengths the finder can name, in beats: eighth notes up to eight
/// beats. The argmax row order. A label that is not one of these (and not `0`,
/// which is "no loop") is not a training take.
pub const LENGTHS: [f32; 7] = [0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0];

/// Rows of a fit: the strength row first, then one row per length class.
pub const ROWS: usize = 1 + LENGTHS.len();

/// The number of inputs: the musical digest, and nothing else.
pub const INPUTS: usize = MUSIC;

/// One labelled tick. `label` is `Some(beats)` for a loop of that length (must
/// be one of [`LENGTHS`], otherwise the take is ignored), and `None` for a tick
/// with no loop playing.
#[derive(Clone, Debug)]
pub struct Take {
    pub song: String,
    pub music: [f32; MUSIC],
    pub label: Option<f32>,
}

/// What the finder says about one tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guess {
    /// Loop strength, clamped to `0..=1`.
    pub strength: f32,
    /// The loop length in beats, one of [`LENGTHS`].
    pub beats: f32,
}

/// A fitted rhythm finder. `w` has [`ROWS`] rows, each `INPUTS + 1` long: the
/// weights then the bias.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LoopFinder {
    /// The lengths the rows were fitted for, in order. Checked on load.
    pub lengths: Vec<f32>,
    pub w: Vec<Vec<f32>>,
    pub songs: usize,
    pub takes: usize,
    /// Positive and negative take counts, for the report.
    pub positives: usize,
    pub negatives: usize,
}

impl LoopFinder {
    /// Whether this fit is the one this build reads: the lengths and the row
    /// shape both match. A fit written before a change to [`LENGTHS`] or
    /// [`INPUTS`] (the tick-based finder wrote `8, 16, 64`) is refused rather
    /// than applied to the wrong columns.
    pub fn fits(&self) -> bool {
        self.lengths == LENGTHS
            && self.w.len() == ROWS
            && self.w.iter().all(|r| r.len() == INPUTS + 1)
    }

    /// Whether anybody's fit is in here. False means [`LoopFinder::guess`]
    /// answers `None`.
    pub fn trained(&self) -> bool {
        self.fits() && self.takes > 0
    }

    /// The finder's answer for one tick, or `None` when there is no fit.
    pub fn guess(&self, music: &[f32; MUSIC]) -> Option<Guess> {
        if !self.trained() {
            return None;
        }
        let score = |row: &Vec<f32>| {
            let mut v = row[INPUTS];
            for k in 0..INPUTS {
                v += row[k] * music[k];
            }
            if v.is_finite() { v } else { 0.0 }
        };
        let strength = score(&self.w[0]).clamp(0.0, 1.0);
        let mut best = 0;
        let mut best_score = f32::NEG_INFINITY;
        for (c, row) in self.w[1..].iter().enumerate() {
            let s = score(row);
            if s > best_score {
                best = c;
                best_score = s;
            }
        }
        Some(Guess { strength, beats: LENGTHS[best] })
    }

    /// Fits a finder from labelled ticks. Takes whose label is neither `None`
    /// nor one of [`LENGTHS`] are ignored. No takes gives an all-zero fit
    /// that [`LoopFinder::trained`] reports as untrained.
    pub fn train(takes: &[Take]) -> LoopFinder {
        let takes: Vec<&Take> = takes
            .iter()
            .filter(|t| t.label.is_none_or(|n| LENGTHS.contains(&n)))
            .collect();
        let mut songs: Vec<&str> = takes.iter().map(|t| t.song.as_str()).collect();
        songs.sort_unstable();
        songs.dedup();
        let positives = takes.iter().filter(|t| t.label.is_some()).count();

        let w = fit(&takes);
        LoopFinder {
            lengths: LENGTHS.to_vec(),
            w,
            songs: songs.len(),
            takes: takes.len(),
            positives,
            negatives: takes.len() - positives,
        }
    }

    /// Reads the fit under `data`, or the empty (untrained) finder when there is
    /// none or it is stale.
    pub fn load(data: &Path) -> LoopFinder {
        metal_vfs::read_to_string(model_path(data))
            .ok()
            .and_then(|t| serde_json::from_str::<LoopFinder>(&t).ok())
            .filter(|f| f.fits())
            .unwrap_or_default()
    }

    pub fn save(&self, data: &Path) -> Result<(), String> {
        if !self.fits() {
            return Err("nothing fitted to save".into());
        }
        let path = model_path(data);
        if let Some(parent) = path.parent() {
            metal_vfs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        metal_vfs::write(path, text).map_err(|e| e.to_string())
    }
}

pub fn model_path(data: &Path) -> PathBuf {
    data.join(crate::index::DIR).join("loopfind.json")
}

/// The target row for a take: strength 1 or 0, then the one-hot length class.
fn targets(label: Option<f32>) -> [f64; ROWS] {
    let mut t = [0.0f64; ROWS];
    if let Some(n) = label {
        t[0] = 1.0;
        if let Some(c) = LENGTHS.iter().position(|&l| l == n) {
            t[1 + c] = 1.0;
        }
    }
    t
}

fn fit(takes: &[&Take]) -> Vec<Vec<f32>> {
    const N: usize = INPUTS + 1;
    if takes.is_empty() {
        return vec![vec![0.0; N]; ROWS];
    }
    let mut xtx = vec![vec![0.0f64; N]; N];
    let mut xty = vec![vec![0.0f64; ROWS]; N];
    for t in takes {
        let mut x = [0.0f64; N];
        for k in 0..INPUTS {
            x[k] = t.music[k] as f64;
        }
        x[INPUTS] = 1.0;
        let y = targets(t.label);
        for i in 0..N {
            for j in 0..N {
                xtx[i][j] += x[i] * x[j];
            }
            for c in 0..ROWS {
                xty[i][c] += x[i] * y[c];
            }
        }
    }
    // The Colourist's ridge, for the same reason: the chroma columns are close
    // to collinear in any tonal passage.
    for i in 0..INPUTS {
        xtx[i][i] += crate::curate::LAMBDA as f64 * takes.len() as f64;
    }
    match crate::curate::solve(xtx, xty) {
        Some(beta) => (0..ROWS)
            .map(|c| (0..N).map(|i| beta[i][c] as f32).collect())
            .collect(),
        None => vec![vec![0.0; N]; ROWS],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fw-loopfind-{name}-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A music vector that is quiet everywhere except one column, lifted.
    fn lit(column: usize, level: f32) -> [f32; MUSIC] {
        let mut m = [0.1f32; MUSIC];
        m[column] = level;
        m
    }

    /// Column 0 lit: a two-beat loop. Column 1 lit: a quarter-beat loop. Column
    /// 2 lit: no loop. Several songs so the fit is not one song's quirks.
    fn synthetic() -> Vec<Take> {
        let mut out = Vec::new();
        for s in 0..4 {
            let song = format!("song-{s}");
            for i in 0..40 {
                let level = 0.8 + 0.01 * i as f32;
                out.push(Take { song: song.clone(), music: lit(0, level), label: Some(2.0) });
                out.push(Take { song: song.clone(), music: lit(1, level), label: Some(0.25) });
                out.push(Take { song: song.clone(), music: lit(2, level), label: None });
            }
        }
        out
    }

    #[test]
    fn the_lengths_are_the_seven_beat_classes() {
        assert_eq!(LENGTHS, [0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0]);
        assert_eq!(ROWS, LENGTHS.len() + 1);
    }

    #[test]
    fn untrained_finder_answers_nothing_and_loads_from_an_empty_tree() {
        let dir = scratch("empty");
        assert!(LoopFinder::default().guess(&lit(0, 1.0)).is_none());
        let f = LoopFinder::load(&dir);
        assert!(!f.trained());
        assert!(f.guess(&lit(0, 1.0)).is_none());
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn it_finds_the_loop_and_its_length_and_says_no_loop_when_there_is_none() {
        let f = LoopFinder::train(&synthetic());
        assert!(f.trained());
        assert_eq!(f.songs, 4);
        assert_eq!(f.positives + f.negatives, f.takes);

        let two = f.guess(&lit(0, 0.9)).unwrap();
        assert_eq!(two.beats, 2.0);
        assert!(two.strength > 0.5, "strength {}", two.strength);

        let quarter = f.guess(&lit(1, 0.9)).unwrap();
        assert_eq!(quarter.beats, 0.25);
        assert!(quarter.strength > 0.5, "strength {}", quarter.strength);

        let none = f.guess(&lit(2, 0.9)).unwrap();
        assert!(none.strength < 0.5, "strength {}", none.strength);
    }

    #[test]
    fn strength_is_clamped_to_the_unit_interval() {
        let f = LoopFinder::train(&synthetic());
        for c in 0..MUSIC {
            for level in [0.0f32, 1.0, 5.0] {
                let g = f.guess(&lit(c, level)).unwrap();
                assert!((0.0..=1.0).contains(&g.strength));
                assert!(LENGTHS.contains(&g.beats));
            }
        }
    }

    #[test]
    fn labels_outside_the_lengths_are_ignored_not_fitted() {
        let mut takes = synthetic();
        let before = LoopFinder::train(&takes).takes;
        takes.push(Take { song: "x".into(), music: lit(0, 1.0), label: Some(3.0) });
        takes.push(Take { song: "x".into(), music: lit(0, 1.0), label: Some(16.0) });
        let after = LoopFinder::train(&takes);
        assert_eq!(after.takes, before);
    }

    #[test]
    fn a_fit_survives_being_written_and_read_back() {
        let dir = scratch("roundtrip");
        let f = LoopFinder::train(&synthetic());
        f.save(&dir).unwrap();
        assert!(model_path(&dir).is_file());
        let back = LoopFinder::load(&dir);
        assert!(back.trained());
        assert_eq!(back.guess(&lit(0, 0.9)), f.guess(&lit(0, 0.9)));
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_fit_is_refused_on_load() {
        let dir = scratch("stale");
        let mut f = LoopFinder::train(&synthetic());
        f.lengths = vec![8.0, 16.0];
        f.save(&dir).unwrap_err();
        // Write it past `save`'s guard, the way an older build would have.
        let text = serde_json::to_string(&f).unwrap();
        metal_vfs::create_dir_all(model_path(&dir).parent().unwrap()).unwrap();
        metal_vfs::write(model_path(&dir), text).unwrap();
        assert!(!LoopFinder::load(&dir).trained());

        // A fit the tick-based build wrote: its lengths `8, 16, 64` and its
        // four rows. The new build must not read it as beats.
        let row = format!("[{}]", vec!["0.0"; INPUTS + 1].join(","));
        let old = format!(
            r#"{{"lengths":[8,16,64],"w":[{r},{r},{r},{r}],"songs":1,"takes":1,"positives":1,"negatives":0}}"#,
            r = row
        );
        metal_vfs::write(model_path(&dir), old).unwrap();
        assert!(!LoopFinder::load(&dir).trained());
        assert!(!LoopFinder::load(&dir).fits());
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn training_on_nothing_is_untrained_not_a_panic() {
        let f = LoopFinder::train(&[]);
        assert!(!f.trained());
        assert!(f.guess(&lit(0, 1.0)).is_none());
    }
}
