//! The fixture dataset: audio ticks → authoring examples on a real patch.
//!
//! Same shape as the colourist's (`data/dataset/colourist`): per song, per *part*, at the
//! canonical 32 Hz. Each tick carries the music row (the 16 music columns) and the stage
//! floats the author (AI or human) wrote for that tick on **this patch**. The columns are
//! the patch's matrix, so an example is only valid against the patch it was cut for
//! (`patch_hash`). Per-fixture profiles come out of authored examples through
//! [`Example::lanes`].

use crate::names::{Channel, FloatName};
use crate::patch::{Matrix, Patch};
use crate::seq::{Lane, StageSeq};
use metal_schema::MUSIC;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tick {
    pub tick: u32,
    /// The first `MUSIC` columns of the digest row at this tick.
    pub music: Vec<f32>,
    /// One value per `Example::columns`; `None` until authored.
    pub stage: Option<Vec<f32>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Example {
    pub model: String,
    pub song: String,
    pub rate: f32,
    pub patch: String,
    pub patch_hash: String,
    /// The part's id in `parts.json` (e.g. `01-p1`), its kind and tick span.
    pub part: String,
    pub kind: String,
    pub span: [u32; 2],
    pub columns: Vec<String>,
    pub ticks: Vec<Tick>,
    /// Approved by a person (the `status.json` flow); drafts are not training data.
    #[serde(default)]
    pub approved: bool,
}

/// A stable fingerprint of what the columns depend on: fixture ids, types, channels,
/// points. Re-patching changes it and old examples are flagged, not silently reused.
pub fn patch_hash(patch: &Patch) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |s: &str| {
        for b in s.bytes().chain([0]) {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
    };
    for (f, p) in patch.fixtures.iter().zip(patch.placements()) {
        eat(&f.id);
        eat(f.kind.word());
        eat(&p.point.word());
        for c in f.channels() {
            eat(c.word());
        }
    }
    format!("{h:016x}")
}

/// Cut an example scaffold from digest rows (`rows[t]` = the row at tick `t`, music
/// columns first) over `start..=end`, every `stride` ticks. `stage` starts as a draft
/// ([`draft`]) so there is something to correct, not a blank page.
pub fn scaffold(
    patch: &Patch,
    song: &str,
    part: &str,
    kind: &str,
    rate: f32,
    rows: &[Vec<f32>],
    span: [u32; 2],
    stride: u32,
) -> Result<Example, String> {
    if stride == 0 || span[1] < span[0] {
        return Err("a part needs stride > 0 and end ≥ start".into());
    }
    let m = patch.matrix();
    let mut ticks = Vec::new();
    let mut t = span[0];
    while t <= span[1] {
        let row = rows.get(t as usize).ok_or_else(|| format!("tick {t} is past the digest ({} rows)", rows.len()))?;
        if row.len() < MUSIC {
            return Err(format!("digest row {t} has {} columns, need ≥ {MUSIC}", row.len()));
        }
        let music = row[..MUSIC].to_vec();
        let stage = Some(draft(&m, &music));
        ticks.push(Tick { tick: t, music, stage });
        t += stride;
    }
    Ok(Example {
        model: "stage".into(),
        song: song.into(),
        rate,
        patch: patch.name.clone(),
        patch_hash: patch_hash(patch),
        part: part.into(),
        kind: kind.into(),
        span,
        columns: m.names(),
        ticks,
        approved: false,
    })
}

/// A neutral first draft from one music row — a place to start, never an answer:
/// level follows loudness (backlights and strobes follow onsets), colour follows the
/// strongest chroma, moving heads rest centred. The author edits from here.
pub fn draft(m: &Matrix, music: &[f32]) -> Vec<f32> {
    let rms = music[0].clamp(0.0, 1.0);
    let onset = (music[2] / 4.0).clamp(0.0, 1.0);
    let hue = music[4..16]
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0.0, |(i, _)| i as f32 / 12.0);
    m.columns
        .iter()
        .map(|c| match c.channel {
            Channel::Level => match c.kind {
                crate::names::Kind::Backlight => (rms * 0.6 + onset * 0.4).clamp(0.0, 1.0),
                _ => rms,
            },
            Channel::Hue => hue,
            Channel::Sat => 0.8,
            Channel::Strobe => 0.0,
            ch => ch.get(&Channel::rest()),
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub tick: Option<u32>,
    pub what: String,
}

impl Example {
    /// Checks against the patch it will be used on.
    pub fn validate(&self, patch: &Patch) -> Vec<Problem> {
        let mut v = Vec::new();
        let m = patch.matrix();
        let mut bad = |tick, what: String| v.push(Problem { tick, what });
        if self.patch_hash != patch_hash(patch) {
            bad(None, format!("cut for another patch ({}), this one is {}", self.patch_hash, patch_hash(patch)));
        }
        if self.columns != m.names() {
            bad(None, "columns differ from the patch's matrix".into());
        }
        for c in &self.columns {
            if FloatName::parse(c).is_none() {
                bad(None, format!("`{c}` is not a stage float name"));
            }
        }
        let mut last = None;
        for t in &self.ticks {
            if last.is_some_and(|l| l >= t.tick) {
                bad(Some(t.tick), "ticks must increase".into());
            }
            last = Some(t.tick);
            if t.music.len() != MUSIC {
                bad(Some(t.tick), format!("music has {} columns, need {MUSIC}", t.music.len()));
            }
            match &t.stage {
                None if self.approved => bad(Some(t.tick), "approved but not authored".into()),
                None => {}
                Some(s) if s.len() != self.columns.len() => {
                    bad(Some(t.tick), format!("stage has {} floats, need {}", s.len(), self.columns.len()))
                }
                Some(s) => {
                    for (c, x) in self.columns.iter().zip(s) {
                        if let Some(f) = FloatName::parse(c) {
                            let [lo, hi] = f.channel.range();
                            if !(lo..=hi).contains(x) || !x.is_finite() {
                                bad(Some(t.tick), format!("{c} = {x} outside {lo}..{hi}"));
                            }
                        }
                    }
                }
            }
        }
        v
    }

    /// The authored ticks as a sequence (every authored tick becomes a full key).
    pub fn seq(&self) -> StageSeq {
        let mut s = StageSeq { rate: self.rate, columns: self.columns.clone(), keys: vec![] };
        for t in &self.ticks {
            if let Some(vals) = &t.stage {
                s.keys.push(crate::seq::Key { tick: t.tick, values: vals.iter().map(|v| Some(*v)).collect() });
            }
        }
        s
    }

    /// The per-fixture profiles authored from this example: fixture → channel → keyframes.
    pub fn lanes(&self, patch: &Patch) -> BTreeMap<String, BTreeMap<Channel, Lane>> {
        self.seq().lanes(&patch.matrix())
    }
}

/// Training pairs `(music row, stage floats)` from approved, valid examples only.
pub fn pairs(examples: &[Example], patch: &Patch) -> Vec<(Vec<f32>, Vec<f32>)> {
    examples
        .iter()
        .filter(|e| e.approved && e.validate(patch).is_empty())
        .flat_map(|e| e.ticks.iter().filter_map(|t| Some((t.music.clone(), t.stage.clone()?))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::tests::demo;
    fn rows(n: usize) -> Vec<Vec<f32>> {
        (0..n).map(|i| {
            let mut r = vec![0.0; MUSIC];
            r[0] = i as f32 / n as f32;
            r[4 + (i % 12)] = 1.0;
            r
        }).collect()
    }
    #[test]
    fn scaffold_validates_and_exports_lanes() {
        let p = demo();
        let mut e = scaffold(&p, "s", "s-p1", "drop", 32.0, &rows(100), [10, 50], 8).unwrap();
        assert_eq!(e.ticks.len(), 6);
        assert!(e.validate(&p).is_empty());
        assert!(pairs(&[e.clone()], &p).is_empty(), "drafts are not training data");
        e.approved = true;
        assert_eq!(pairs(&[e.clone()], &p).len(), 6);
        assert!(e.lanes(&p)["back-1"].contains_key(&Channel::Level));
        let mut other = p.clone();
        other.fixtures.pop();
        assert!(!e.validate(&other).is_empty(), "another patch invalidates it");
        e.ticks[0].stage.as_mut().unwrap()[0] = 9.0;
        assert!(e.validate(&p).iter().any(|x| x.what.contains("outside")));
    }
}
