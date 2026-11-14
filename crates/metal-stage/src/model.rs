//! The `ai.stage` model: a fitted linear map from the music row to the stage floats of one
//! patch.
//!
//! Same family and fit as the Colourist (`metal_reel::grade`): one ridge least-squares row
//! per output over the music columns plus a bias, solved in closed form. It is linear on
//! purpose, as the Colourist is: a linear map trains on a few hundred authored ticks where a
//! net would memorise them. The outputs are the patch's matrix columns, so a model is per
//! patch. It records the patch name and hash, and [`StageModel::fits`] refuses any other
//! patch: a re-patch changes the hash and old weights are never silently applied to it.

use crate::names::Channel;
use crate::patch::Patch;
use metal_schema::MUSIC;
use serde::{Deserialize, Serialize};

/// The digest's music columns: the same sixteen the Colourist reads.
pub const INPUTS: usize = MUSIC;

/// The Colourist's ridge (`metal_reel::curate::LAMBDA`), restated: this crate stays free of
/// `metal-reel`, which pulls the ingest and router crates in.
pub const LAMBDA: f64 = 0.05;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StageModel {
    pub patch: String,
    pub patch_hash: String,
    /// The float names it was fitted against, in the patch matrix's order.
    pub columns: Vec<String>,
    /// One row per column: `INPUTS` weights, then the bias.
    pub w: Vec<Vec<f32>>,
    /// How many authored ticks it was fitted on.
    pub ticks: usize,
}

impl StageModel {
    /// Fits the patch's weights from `(music row, stage floats)` pairs (see
    /// [`crate::dataset::pairs`]). Each stage row carries one float per matrix column.
    pub fn train(patch: &Patch, pairs: &[(Vec<f32>, Vec<f32>)]) -> Result<StageModel, String> {
        let m = patch.matrix();
        let outputs = m.columns.len();
        if pairs.is_empty() {
            return Err("no approved, valid examples to train on".into());
        }
        if outputs == 0 {
            return Err(format!(
                "patch `{}` has no stage floats to learn",
                patch.name
            ));
        }
        const N: usize = INPUTS + 1;
        let mut xtx = vec![vec![0.0f64; N]; N];
        let mut xty = vec![vec![0.0f64; outputs]; N];
        for (music, stage) in pairs {
            if music.len() < INPUTS || stage.len() != outputs {
                return Err(format!(
                    "a pair has {} music and {} stage values; need {INPUTS}+ and {outputs}",
                    music.len(),
                    stage.len()
                ));
            }
            let mut x = [0.0f64; N];
            for k in 0..INPUTS {
                x[k] = music[k] as f64;
            }
            x[INPUTS] = 1.0;
            for i in 0..N {
                for j in 0..N {
                    xtx[i][j] += x[i] * x[j];
                }
                for c in 0..outputs {
                    xty[i][c] += x[i] * stage[c] as f64;
                }
            }
        }
        for i in 0..INPUTS {
            xtx[i][i] += LAMBDA * pairs.len() as f64;
        }
        let beta = solve(xtx, xty).ok_or("the fit is singular")?;
        let w = (0..outputs)
            .map(|c| (0..N).map(|i| beta[i][c] as f32).collect())
            .collect();
        Ok(StageModel {
            patch: patch.name.clone(),
            patch_hash: crate::dataset::patch_hash(patch),
            columns: m.names(),
            w,
            ticks: pairs.len(),
        })
    }

    /// Whether these weights were fitted for exactly this patch: name, hash and columns.
    pub fn fits(&self, patch: &Patch) -> bool {
        self.patch == patch.name
            && self.patch_hash == crate::dataset::patch_hash(patch)
            && self.columns == patch.matrix().names()
            && self.w.len() == self.columns.len()
            && self.w.iter().all(|r| r.len() == INPUTS + 1)
    }

    /// One music row → one stage float per matrix column, each clamped into its float's
    /// range. Refuses weights that were fitted for another patch.
    pub fn infer(&self, patch: &Patch, music: &[f32]) -> Result<Vec<f32>, String> {
        if !self.fits(patch) {
            return Err(format!(
                "these weights were not fitted for patch `{}`",
                patch.name
            ));
        }
        if music.len() < INPUTS {
            return Err(format!(
                "ai.stage expects {INPUTS} music columns, got {}",
                music.len()
            ));
        }
        let m = patch.matrix();
        Ok(self
            .w
            .iter()
            .zip(&m.columns)
            .map(|(row, c)| {
                let mut v = row[INPUTS];
                for k in 0..INPUTS {
                    v += row[k] * music[k];
                }
                let [lo, hi] = c.channel.range();
                if v.is_finite() {
                    v.clamp(lo, hi)
                } else {
                    c.channel.get(&Channel::rest())
                }
            })
            .collect())
    }
}

/// Gauss-Jordan elimination with partial pivoting; the same solver the Colourist's fit uses
/// (`metal_reel::curate::solve`), restated for the same reason as [`LAMBDA`].
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<Vec<f64>>) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    for col in 0..n {
        let (mut pivot, mut best) = (col, a[col][col].abs());
        for r in col + 1..n {
            if a[r][col].abs() > best {
                best = a[r][col].abs();
                pivot = r;
            }
        }
        if best < 1e-9 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let d = a[col][col];
        for j in 0..n {
            a[col][j] /= d;
        }
        for j in 0..b[col].len() {
            b[col][j] /= d;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let f = a[r][col];
            if f == 0.0 {
                continue;
            }
            for j in 0..n {
                a[r][j] -= f * a[col][j];
            }
            for j in 0..b[col].len() {
                b[r][j] -= f * b[col][j];
            }
        }
    }
    Some(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{pairs, scaffold};
    use crate::names::Kind;
    use crate::patch::tests::demo;

    /// A deterministic music row: a slow sweep on the loudness column and a moving chroma.
    fn music(i: usize) -> Vec<f32> {
        let t = i as f32;
        let mut r = vec![0.0f32; INPUTS];
        r[0] = 0.5 + 0.4 * (t * 0.37).sin();
        r[2] = 0.5 + 0.5 * (t * 0.11).cos();
        r[4 + (i % 12)] = 1.0;
        r
    }

    /// Stage floats that are a plain function of the music: level follows loudness on
    /// every float, moving heads' pan follows onsets, the rest hold their rest.
    fn target(m: &Patch, row: &[f32]) -> Vec<f32> {
        m.matrix()
            .columns
            .iter()
            .map(|c| match (c.kind, c.channel) {
                (_, Channel::Level) => row[0],
                (Kind::Moving, Channel::Pan) => 100.0 * row[2],
                _ => 0.0,
            })
            .collect()
    }

    fn training_set(p: &Patch, n: usize) -> Vec<(Vec<f32>, Vec<f32>)> {
        (0..n)
            .map(|i| {
                let m = music(i);
                let s = target(p, &m);
                (m, s)
            })
            .collect()
    }

    #[test]
    fn trains_back_a_mapping_it_was_shown() {
        let p = demo();
        let set = training_set(&p, 400);
        let model = StageModel::train(&p, &set).unwrap();
        assert!(model.fits(&p));
        assert_eq!(model.w.len(), p.matrix().columns.len());
        let probe = music(1000);
        let got = model.infer(&p, &probe).unwrap();
        let want = target(&p, &probe);
        let level = p
            .matrix()
            .columns
            .iter()
            .position(|c| c.channel == Channel::Level)
            .unwrap();
        assert!(
            (got[level] - want[level]).abs() < 0.1,
            "level {} vs {}",
            got[level],
            want[level]
        );
        // The ridge shrinks a little; the direction of the pan follows the onsets.
        let pan = p
            .matrix()
            .columns
            .iter()
            .position(|c| c.channel == Channel::Pan)
            .unwrap();
        assert!(
            got[pan] > 0.0 && (got[pan] - want[pan]).abs() < 15.0,
            "pan {} vs {}",
            got[pan],
            want[pan]
        );
    }

    #[test]
    fn the_output_is_clamped_into_each_floats_range() {
        let p = demo();
        let set: Vec<(Vec<f32>, Vec<f32>)> = training_set(&p, 200)
            .into_iter()
            .map(|(m, s)| (m, s.iter().map(|v| v * 9.0).collect()))
            .collect();
        let model = StageModel::train(&p, &set).unwrap();
        let mut hot = music(3);
        hot[0] = 1.0;
        hot[2] = 4.0;
        let out = model.infer(&p, &hot).unwrap();
        for (c, v) in p.matrix().columns.iter().zip(out) {
            let [lo, hi] = c.channel.range();
            assert!((lo..=hi).contains(&v), "{c} = {v} outside {lo}..{hi}");
        }
    }

    #[test]
    fn weights_fitted_for_another_patch_are_refused() {
        let p = demo();
        let model = StageModel::train(&p, &training_set(&p, 100)).unwrap();
        let mut other = p.clone();
        other.fixtures.pop();
        assert!(!model.fits(&other), "a re-patch changes the hash");
        assert!(model.infer(&other, &music(0)).is_err());
        let mut renamed = p.clone();
        renamed.name = "elsewhere".into();
        assert!(!model.fits(&renamed), "the patch name is part of the fit");
    }

    #[test]
    fn a_model_round_trips_through_json() {
        let p = demo();
        let model = StageModel::train(&p, &training_set(&p, 50)).unwrap();
        let text = serde_json::to_string(&model).unwrap();
        let back: StageModel = serde_json::from_str(&text).unwrap();
        assert_eq!(back, model);
        assert!(back.fits(&p));
    }

    #[test]
    fn nothing_to_learn_from_is_a_normal_error() {
        let p = demo();
        assert!(StageModel::train(&p, &[]).is_err());
        assert!(
            StageModel::train(&p, &[(music(0), vec![0.0; 2])]).is_err(),
            "a width that is not the matrix's"
        );
    }

    #[test]
    fn examples_cut_and_approved_train_the_model() {
        let p = demo();
        let rows: Vec<Vec<f32>> = (0..120)
            .map(|i| {
                let mut r = music(i);
                r.resize(metal_schema::MUSIC + 13, 0.0);
                r
            })
            .collect();
        let mut e = scaffold(&p, "s", "s-p1", "drop", 32.0, &rows, [0, 119], 2).unwrap();
        // A draft is not training data; the author approves it by hand.
        assert!(pairs(std::slice::from_ref(&e), &p).is_empty());
        e.approved = true;
        let set = pairs(std::slice::from_ref(&e), &p);
        assert_eq!(set.len(), 60);
        let model = StageModel::train(&p, &set).unwrap();
        assert_eq!(model.ticks, 60);
        assert_eq!(model.columns, p.matrix().names());
    }
}
