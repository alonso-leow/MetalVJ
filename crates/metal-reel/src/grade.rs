//! **The Colourist** — how the footage that is on gets graded, given what is
//! playing.
//!
//! # It replaces one function, and inherits everything else
//!
//! `metal-dash::drive::row` turns a digest row into a model's float row. Almost all
//! of what it does is safety that was a bug once:
//!
//! - a float is modulated **around the operator's row**, never swept across its
//!   declared range (Turing rendered a blank sheet when it was);
//! - a structural float is damped by its own `FloatSpec::drive`;
//! - the result is clamped to the declared range;
//! - a camera is never touched (`CORRECTIONS.md` §43).
//!
//! Exactly one thing in it is arbitrary: `column_for`, which assigns each float
//! a digest column **round-robin** — float 0 gets rms, float 1 bandwidth, float
//! 2 onset, float 3 centroid, float 4 rms again. Nothing about that assignment
//! knows what the float *means*. It is a reasonable default across thirty models
//! precisely because it is not a claim.
//!
//! So the Colourist replaces `column_for` and nothing else. It produces a signed
//! [`Offsets`] row in −1..1, which `drive::row` multiplies by the same
//! `SWING × range × drive × depth` it always did and clamps the same way. **A
//! Colourist that has learnt something absurd can still only produce a row a
//! slider could have produced.** That is the property that lets an untrained
//! default ship at all.
//!
//! # What it sees: sixteen numbers, and the reason it is not twenty-eight
//!
//! The first version read the frame as well — [`crate::read`]'s twelve columns
//! stacked under the digest's sixteen — so that a grade could *compensate*, the
//! way a colourist does: lift a dark shot, pull down a bright one, drag a
//! tungsten clip back toward neutral. That was cut on 2026-08-25.
//!
//! > *"I think we over-designed the input of this new kind of model, because
//! > the music as input was enough, same as the rest of the models; the only
//! > difference is the curator model for choosing scenes."*
//!
//! Four things were wrong with it and the first is the one that matters:
//!
//! 1. **It broke the claim the whole design rests on.** A transmission is a
//!    model — same registry, same `Schema`, same lane, same Router — and the
//!    boast in `docs/TRANSMISSIONS.md` is that nothing downstream of `render`
//!    had to learn what footage is. Feeding the picture back into the model that
//!    writes the row made the *upstream* learn it instead. The rule was kept on
//!    one side of the seam and broken on the other.
//! 2. **The Curator already answered that question.** If the shot on screen is
//!    too bright, that is a choice the Curator made, and the place to fix it is
//!    the Curator — not a second model quietly undoing the first one's pick.
//! 3. **A compensation is invisible by design.** Its purpose is to make every
//!    shot land the same, which is continuity work for a cut that a viewer
//!    watches for minutes. A passage here is a few seconds long on a wall behind
//!    a DJ. Nobody is checking that shot four matches shot three.
//! 4. Twenty-eight inputs against the few hundred authored ticks anybody will
//!    realistically produce is a regime where the fit memorises. Sixteen nearly
//!    halves the parameters per float.
//!
//! **What is genuinely lost:** nothing pulls a heavily-cast clip back toward
//! neutral on its own any more. That is a real capability and it is gone on
//! purpose — a fixed property of a file belongs at index time or on the
//! operator's base row, not re-derived per frame by a model whose input changes
//! eight times a second.
//!
//! # So what is left that `column_for` was not doing
//!
//! The round robin gives each float **one** column and picks it by counting. The
//! Colourist gives each float a fitted map over **all sixteen**, and its
//! authored default gives each float the column somebody chose on purpose. That
//! difference survives the cut intact: it was never the image half that made
//! this worth having, which is most of why the image half went.
//!
//! # One consequence worth naming: `FloatSpec::smooth` stops applying per float
//!
//! `drive::row` lets a float say whether it rides the interpolated digest row or
//! the stepped one, because each float is tied to exactly one column. A
//! Colourist mixes all sixteen, so there is no single column whose staircase a
//! float could opt out of. The caller therefore blends the row **per column**,
//! by `drive::smooth_default` scaled by the operator's global `smooth`, and
//! hands over one row.
//!
//! That is not a loss in practice and it is worth saying why: `smooth_default`
//! already lands onset raw and the three continuous columns interpolated, which
//! is the distinction the per-float override exists to correct *exceptions* to.
//! A transmission has no exceptions to it — the one float that must land on the
//! hit is `tartamudeo`, and it is onset-driven.

use std::path::{Path, PathBuf};

use metal_schema::{Block, Schema};
use serde::{Deserialize, Serialize};

use crate::MUSIC;

/// How many numbers the Colourist sees: the musical digest, and nothing else.
///
/// An alias rather than a second constant, so there is no arithmetic left that
/// could disagree about the width after the image half was cut.
pub const INPUTS: usize = MUSIC;

/// The digest's four non-chroma columns, named so the table below is readable.
///
/// Chroma reaches a transmission through the colour slot, as it does everywhere
/// else in the suite: a stutter length driven by how much D# is in the chord is
/// noise wearing a feature's name. A *fitted* Colourist may use all sixteen —
/// nothing stops it, and if the twelve chroma columns carry something real the
/// fit is welcome to it. The authored table below does not guess.
pub const RMS: usize = 0;
pub const BANDWIDTH: usize = 1;
pub const ONSET: usize = 2;
pub const CENTROID: usize = 3;

/// One line of an authored default: this column moves that float this far.
///
/// Keyed by the float's **name**, not by its position. A schema is a list that
/// gets things inserted into it, and a table of indices would silently start
/// grading the wrong parameter the first time one did — which is the failure
/// `registry`'s "the order is append-only" note exists to stop, in the one
/// place appending would not be enough.
///
/// The names are shared across transmissions on purpose: `tartamudeo` means
/// the same operation in Wake (`wake`) as it did in the model that used to
/// sit beside it, so it got one line, not two. That is also why `read::NAMES`'
/// vocabulary is the vocabulary here — one word per quantity across the whole
/// crate.
///
/// The other transmission's lines were removed 2026-09-28 with it: it is a
/// tombstone in `metal-render::registry`, so no transmission owns its floats and
/// the `metal-render` gate below would (correctly) flag them as dead.
pub struct Rule {
    pub float: &'static str,
    pub from: usize,
    pub weight: f32,
    /// Where the *column* sits when nothing is playing. Subtracted before the
    /// weight is applied, so a rule reads as a push away from rest exactly the
    /// way `drive::rest_value` does — and so silence pushes nothing rather than
    /// pushing everything down. Onset rests at 0; every other column at 0.5.
    pub rest: f32,
}

const fn rule(float: &'static str, from: usize, weight: f32) -> Rule {
    Rule { float, from, weight, rest: if from == ONSET { 0.0 } else { 0.5 } }
}

/// How far off rest a column can actually get, per column.
///
/// **The one piece of arithmetic that makes the weights below readable, and it
/// was got wrong once.** Onset rests at 0 and runs to 1, so it reaches a whole
/// unit off rest and a weight of 0.9 produces an offset of 0.9. Every other
/// column rests in the *middle* of 0..1, so it only ever reaches half a unit,
/// and the same weight of 0.9 produces an offset of 0.45 — half the authority,
/// for a line that looks identical.
///
/// So: **a non-onset weight is twice the offset it buys at the rails.** The
/// table is written that way throughout, which is why its rms weights look
/// large next to its onset ones and are not.
pub const fn swing_of(column: usize) -> f32 {
    if column == ONSET {
        1.0
    } else {
        0.5
    }
}

/// The grade a transmission gets before anybody authors one.
///
/// Read it as a claim about *which column each parameter belongs to*, which is
/// the claim the round robin declines to make. Three kinds of line:
///
/// - **the hit** (`ONSET`) — anything that should snap: a held frame, a tear, a
///   channel flying apart. Onset is pooled by max upstream and lands raw, so it
///   is the only column that arrives as an edge rather than as an envelope;
/// - **the level** (`RMS`) — anything that should build and fall with the track:
///   how much of the picture is broken, how hard it is graded;
/// - **the colour of the sound** (`CENTROID`, `BANDWIDTH`) — centroid and bandwidth,
///   onto the parameters that are about hue and about how fine the damage is.
///
/// A model that does not have one of these floats simply never matches it, and
/// there is a test in `metal-render` that no line here names a float **no** model
/// has — which is how four dead rules from a rewrite were found.
pub const DEFAULT: &[Rule] = &[
    // ----- Wake: the stutter -----
    // The hit is what stutters. This is the single most important line here: an
    // effect built on holding frames that does not hold on the beat reads as
    // dropped frames rather than as an effect.
    rule("stutter", ONSET, 0.9),
    // Loud and busy wants short holds; a breakdown wants long ones.
    rule("retention", RMS, -1.0),
    rule("jump", ONSET, 0.4),
    // ----- Wake: the mosh -----
    // How much of the picture has stopped updating, building with the track.
    rule("frozen", RMS, 0.9),
    // Finer blocks under brighter music: a high centroid is detail in the sound,
    // and coarse blocks under it read as the picture lagging the track.
    rule("block", CENTROID, 0.8),
    // Loud means the keyframes land sooner, so the mosh churns instead of
    // settling into one frozen arrangement.
    rule("refresh", RMS, -0.9),
    rule("drag", RMS, 0.8),
    rule("tear", ONSET, 0.5),
    rule("indent", CENTROID, 0.8),
    // ----- Wake: the correction -----
    // These three names were shared with Chemistry and stay: Wake owns them
    // too, so the lines still fire on it.
    rule("exposure", RMS, 0.8),
    rule("contrast", ONSET, 0.5),
    rule("saturation", RMS, 0.8),
    // ----- shared: the channel gains -----
    // The channels: red and blue on centroid (the colour of the sound), green
    // on bandwidth. Green is the widest-band channel and is now the table's
    // only bandwidth line — with Chemistry's `vibrancia`/`tinte` retired
    // 2026-09-28 it carries that column alone, so its weight is the one that
    // makes bandwidth move anything visible.
    rule("r", CENTROID, -0.5),
    rule("b", CENTROID, 0.5),
    rule("g", BANDWIDTH, 0.7),
];

/// The signed push each of a model's own floats wants, in −1..1.
///
/// Same length and same order as the model's own floats, which is what
/// `metal-dash::drive::row` walks. The units are deliberately the units
/// `drive::row` already uses for `feature - rest_value(column)`, so that a
/// saturated offset moves a float exactly as far as a saturated digest column
/// could — no further. `drive.rs`'s own
/// `no_driven_float_wanders_far_from_the_row_the_operator_set` is the gate on
/// that and it did not have to change.
pub type Offsets = Vec<f32>;

// ---------------------------------------------------------------------------
// the model
// ---------------------------------------------------------------------------

/// A fitted grade for one model.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Grade {
    /// Which model this was fitted for. A grade is not transferable: the floats
    /// are a different list.
    pub model: String,
    /// The float names it was fitted against, in order. Checked on load — a
    /// model that gained a float since is a grade that would silently apply to
    /// the wrong parameters from that point on.
    pub floats: Vec<String>,
    /// One row per float, each `INPUTS + 1` long: the weights, then the bias.
    ///
    /// This is also what retires a grade fitted before the image half was cut:
    /// its rows are twenty-nine long, [`Grade::fits`] refuses it, and the model
    /// falls back to [`DEFAULT`] rather than reading sixteen music columns out
    /// of weights that were fitted for twelve picture columns first.
    ///
    /// These are the **fine (canonical, 32 Hz)** weights — the level the
    /// digest is stored on and the one the fine machine runs.
    pub w: Vec<Vec<f32>>,
    /// The **base (8 Hz)** weights, fitted from the pooled projection of the
    /// same takes. `None` for a grade written before the two-level split (a
    /// base machine then runs the fine weights, which is close but not exact).
    /// Both sets are always written together by [`train`] — a fit is a cheap
    /// least-squares solve, so there is no reason to make a machine pay to
    /// re-fit. See the voice design notes § "Two complete levels, one switch" (the
    /// Colourist row of the table).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w_base: Option<Vec<Vec<f32>>>,
    pub songs: usize,
    pub takes: usize,
    /// Held-out mean absolute error, and the same for the baseline that always
    /// answers the mean offset. `None` under [`crate::curate::MIN_SONGS`].
    pub error: Option<(f32, f32)>,
}

impl Grade {
    pub fn fits(&self, model: &str, schema: &Schema) -> bool {
        let rows_ok = |w: &Vec<Vec<f32>>| {
            w.len() == self.floats.len() && w.iter().all(|r| r.len() == INPUTS + 1)
        };
        self.model == model
            && rows_ok(&self.w)
            && self.w_base.as_ref().map_or(true, rows_ok)
            && self.floats == float_names(schema)
    }

    /// The weight rows for `rate`: the base projection's own set when one was
    /// fitted, the fine (canonical) set otherwise. One place the level selects
    /// the weights, so every reader agrees.
    pub fn weights(&self, rate: metal_schema::TickRate) -> &Vec<Vec<f32>> {
        match (rate, &self.w_base) {
            (metal_schema::TickRate::Base, Some(w)) => w,
            _ => &self.w,
        }
    }
}

/// The trained grade for a model, or the authored one.
#[derive(Clone, Debug, Default)]
pub struct Colourist {
    grade: Option<Grade>,
}

pub fn model_path(data: &Path, model: &str) -> PathBuf {
    data.join(crate::index::DIR).join(format!("colourist-{model}.json"))
}

impl Colourist {
    /// Loads the grade for `model`, or the authored default if there is none or
    /// if the one on disk was fitted against a different set of floats.
    pub fn load(data: &Path, model: &str, schema: &Schema) -> Colourist {
        let grade = metal_vfs::read_to_string(model_path(data, model))
            .ok()
            .and_then(|t| serde_json::from_str::<Grade>(&t).ok())
            .filter(|g| g.fits(model, schema));
        Colourist { grade }
    }

    pub fn from_grade(grade: Grade) -> Colourist {
        Colourist { grade: Some(grade) }
    }

    pub fn save(&self, data: &Path) -> Result<(), String> {
        let Some(grade) = &self.grade else {
            return Err("nothing fitted to save".into());
        };
        let path = model_path(data, &grade.model);
        if let Some(parent) = path.parent() {
            metal_vfs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(grade).map_err(|e| e.to_string())?;
        metal_vfs::write(path, text).map_err(|e| e.to_string())
    }

    /// Whether anybody's grade is in here. False means [`DEFAULT`].
    pub fn trained(&self) -> bool {
        self.grade.is_some()
    }

    pub fn grade(&self) -> Option<&Grade> {
        self.grade.as_ref()
    }

    /// What this music wants on the canonical (32 Hz) grid — the level the
    /// digest is stored on and the one existing callers were written against.
    /// [`Colourist::offsets_at`] selects the level explicitly.
    pub fn offsets(&self, schema: &Schema, music: &[f32; MUSIC]) -> Offsets {
        self.offsets_at(schema, music, metal_schema::TickRate::CANONICAL)
    }

    /// What this music wants at `rate`, one entry per own float of `schema`.
    ///
    /// A fitted grade carries the level's own weights (`Grade::weights`), so a
    /// base machine uses the fit made on the pooled projection and a fine one
    /// the canonical fit. The authored [`DEFAULT`] table is rate-independent
    /// (its columns are already pooled/normalised upstream), so it is used as
    /// is at either level.
    pub fn offsets_at(
        &self,
        schema: &Schema,
        music: &[f32; MUSIC],
        rate: metal_schema::TickRate,
    ) -> Offsets {
        let names = float_names(schema);
        let mut out = vec![0.0f32; names.len()];
        match &self.grade {
            Some(g) => {
                for (i, row) in g.weights(rate).iter().enumerate().take(out.len()) {
                    let mut v = row[INPUTS];
                    for k in 0..INPUTS {
                        v += row[k] * music[k];
                    }
                    out[i] = v;
                }
            }
            None => {
                for r in DEFAULT {
                    if let Some(i) = names.iter().position(|n| n == r.float) {
                        out[i] += r.weight * (music[r.from] - r.rest);
                    }
                }
            }
        }
        // The clamp is what makes the seam with `drive::row` safe: a saturated
        // offset moves a float exactly as far as a saturated digest column
        // could, and no further.
        for v in out.iter_mut() {
            *v = if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
        }
        out
    }
}

/// A model's own floats, in row order — the same walk `drive::row` does, and
/// the list a `Grade` is keyed to.
pub fn float_names(schema: &Schema) -> Vec<String> {
    let mut out = Vec::new();
    for block in schema.blocks {
        if let Block::Floats(specs) = block {
            out.extend(specs.iter().map(|s| s.name.to_string()));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// training
// ---------------------------------------------------------------------------

/// One authored moment: what was playing, and the row the operator set.
///
/// The `offsets` are the model's **own floats only**, already reduced by
/// [`offset_of`] — colour slots and cameras are not the Colourist's business and
/// never were.
#[derive(Clone, Debug)]
pub struct Take {
    pub song: String,
    pub music: [f32; MUSIC],
    pub offsets: Vec<f32>,
}

/// How much of a float's range the digest may swing it across at full depth.
///
/// **Restated from `metal-dash::drive::SWING`, not imported** — that crate depends
/// on this one, so the arrow only points one way. `metal-dash` has a test that the
/// two numbers are equal; if they drifted, a grade authored in the studio would
/// land somewhere else in the booth, which is the kind of bug that is only ever
/// found at a venue.
pub const SWING: f32 = 0.6;

/// The offset an authored value implies, given where the operator's base row
/// was and how far the float is allowed to move.
///
/// The inverse of what `drive::row` does with an offset, so a Take round-trips:
/// author a value, reduce it here, feed it back through `drive::row` at depth 1
/// and get the value back. Pinned floats (`drive == 0`) cannot be moved by any
/// signal, so an authored change to one carries no information and reduces to
/// zero rather than to infinity.
pub fn offset_of(value: f32, base: f32, min: f32, max: f32, drive: f32) -> f32 {
    let span = (max - min) * SWING * drive.clamp(0.0, 1.0);
    if span.abs() < 1e-6 {
        return 0.0;
    }
    ((value - base) / span).clamp(-1.0, 1.0)
}

/// Fits a grade for `model` from what was authored against it.
pub fn train(model: &str, schema: &Schema, takes: &[Take]) -> Grade {
    let names = float_names(schema);
    let mut songs: Vec<&str> = takes.iter().map(|t| t.song.as_str()).collect();
    songs.sort_unstable();
    songs.dedup();

    // Two fits from the one take: the canonical (fine) rows, and the base
    // projection made by pooling every four consecutive takes within a song.
    // Both are written, so a base machine runs weights fitted on the base grid
    // rather than the fine ones (which are close, but not the same — `onset`
    // is a max over ~1 frame at 32 Hz and ~5 at 8 Hz).
    let w = fit(takes, names.len());
    let w_base = fit(&pooled_takes(takes), names.len());
    let error = if songs.len() >= crate::curate::MIN_SONGS {
        Some(hold_out_by_song(takes, &songs, names.len()))
    } else {
        None
    };

    Grade {
        model: model.to_string(),
        floats: names,
        w,
        w_base: Some(w_base),
        songs: songs.len(),
        takes: takes.len(),
        error,
    }
}

/// The base (8 Hz) projection of a fine take set: every four consecutive takes
/// **within one song** pooled 4→1 by mean, for the music row and the authored
/// offsets alike. Grouped per song so a boundary never mixes two songs — the
/// takes arrive in song then tick order (`metal-dash::author::grade::takes_for`).
fn pooled_takes(takes: &[Take]) -> Vec<Take> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < takes.len() {
        let song = &takes[i].song;
        let mut j = i;
        while j < takes.len() && &takes[j].song == song {
            j += 1;
        }
        for chunk in takes[i..j].chunks(4) {
            let mut music = [0.0f32; MUSIC];
            let width = chunk.iter().map(|t| t.offsets.len()).min().unwrap_or(0);
            let mut offsets = vec![0.0f32; width];
            for t in chunk {
                for k in 0..MUSIC {
                    music[k] += t.music[k];
                }
                for (c, o) in offsets.iter_mut().enumerate() {
                    *o += t.offsets[c];
                }
            }
            let n = chunk.len() as f32;
            for k in 0..MUSIC {
                music[k] /= n;
            }
            for o in offsets.iter_mut() {
                *o /= n;
            }
            out.push(Take { song: song.clone(), music, offsets });
        }
        i = j;
    }
    out
}

fn fit(takes: &[Take], outputs: usize) -> Vec<Vec<f32>> {
    const N: usize = INPUTS + 1;
    if takes.is_empty() || outputs == 0 {
        return vec![vec![0.0; N]; outputs];
    }
    let mut xtx = vec![vec![0.0f64; N]; N];
    let mut xty = vec![vec![0.0f64; outputs]; N];

    for t in takes {
        let mut x = [0.0f64; N];
        for k in 0..INPUTS {
            x[k] = t.music[k] as f64;
        }
        x[INPUTS] = 1.0;
        for i in 0..N {
            for j in 0..N {
                xtx[i][j] += x[i] * x[j];
            }
            for c in 0..outputs.min(t.offsets.len()) {
                xty[i][c] += x[i] * t.offsets[c] as f64;
            }
        }
    }
    // Same ridge as the Curator, and heavier for the same reason plus one more:
    // the twelve chroma columns are nearly collinear with each other in any
    // tonal passage, and an unpenalised fit over them answers noise everywhere
    // it was not shown a take.
    for i in 0..INPUTS {
        xtx[i][i] += crate::curate::LAMBDA as f64 * takes.len() as f64;
    }

    match crate::curate::solve(xtx, xty) {
        Some(beta) => (0..outputs)
            .map(|c| (0..N).map(|i| beta[i][c] as f32).collect())
            .collect(),
        None => vec![vec![0.0; N]; outputs],
    }
}

fn hold_out_by_song(takes: &[Take], songs: &[&str], outputs: usize) -> (f32, f32) {
    let mut ours = 0.0f64;
    let mut base = 0.0f64;
    let mut n = 0usize;

    for held in songs {
        let train_set: Vec<Take> = takes.iter().filter(|t| t.song != *held).cloned().collect();
        let test: Vec<&Take> = takes.iter().filter(|t| t.song == *held).collect();
        if train_set.is_empty() || test.is_empty() {
            continue;
        }
        let w = fit(&train_set, outputs);
        let mut mean = vec![0.0f32; outputs];
        for t in &train_set {
            for c in 0..outputs.min(t.offsets.len()) {
                mean[c] += t.offsets[c];
            }
        }
        for v in mean.iter_mut() {
            *v /= train_set.len() as f32;
        }

        for t in test {
            for c in 0..outputs.min(t.offsets.len()) {
                let mut got = w[c][INPUTS];
                for k in 0..INPUTS {
                    got += w[c][k] * t.music[k];
                }
                ours += (got.clamp(-1.0, 1.0) - t.offsets[c]).abs() as f64;
                base += (mean[c] - t.offsets[c]).abs() as f64;
                n += 1;
            }
        }
    }
    if n == 0 {
        return (0.0, 0.0);
    }
    ((ours / n as f64) as f32, (base / n as f64) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_schema::{spec, spec_held, Block, FloatSpec, Schema};

    /// Not any real model — a stand-in with one float on each of the four
    /// columns the table reads, so that every line of [`DEFAULT`] this crate
    /// checks actually fires. The names are real ones on purpose: the table is
    /// keyed by name, so a test schema of `a`/`b`/`c` would exercise the
    /// arithmetic and none of the authoring.
    static FLOATS: &[FloatSpec] = &[
        spec("stutter", 0.0, 1.0, 0.2),
        spec("retention", 0.0, 1.0, 0.4),
        spec("frozen", 0.0, 1.0, 0.5),
        spec("exposure", 0.0, 2.0, 1.0),
        spec("contrast", 0.0, 2.0, 1.0),
        // The bandwidth column's only line in `DEFAULT` is the green channel
        // gain now that Chemistry's `vibrancia`/`tinte` are retired, so the
        // stand-in carries `g` to keep that column fired and checked; the
        // centroid column is carried by `sangrado`, its only line.
        spec("g", 0.0, 2.0, 1.0),
        spec("indent", 0.0, 1.0, 0.35),
        spec_held("pinned", 0.0, 1.0, 0.5, 0.0),
    ];
    const OWN: usize = 8;
    static SCHEMA: Schema = Schema { blocks: &[Block::Floats(FLOATS), metal_schema::color("x")] };

    fn music(rms: f32, onset: f32) -> [f32; MUSIC] {
        let mut m = [0.4f32; MUSIC];
        m[RMS] = rms;
        m[ONSET] = onset;
        m
    }

    fn at(name: &str) -> usize {
        float_names(&SCHEMA).iter().position(|n| n == name).expect("in the test schema")
    }

    #[test]
    fn the_colourist_reads_the_music_and_only_the_music() {
        // The cut, as a test: `INPUTS` is the digest's width, so a `Grade` whose
        // rows were fitted over the picture as well cannot be loaded by mistake.
        assert_eq!(INPUTS, MUSIC);
        assert_eq!(INPUTS, 16);
    }

    #[test]
    fn the_offsets_row_is_one_per_own_float_and_ignores_the_colour_slot() {
        let c = Colourist::default();
        let got = c.offsets(&SCHEMA, &music(0.5, 0.2));
        assert_eq!(got.len(), OWN, "one offset per own float, and none for the colour slot");
    }

    #[test]
    fn the_hit_is_what_stutters() {
        // The one line of the default table that an audience would notice being
        // wrong within a bar.
        let c = Colourist::default();
        let quiet = c.offsets(&SCHEMA, &music(0.5, 0.0))[at("stutter")];
        let hit = c.offsets(&SCHEMA, &music(0.5, 0.95))[at("stutter")];
        assert!(hit > quiet + 0.5, "onset moved tartamudeo by {}", hit - quiet);
    }

    #[test]
    fn the_level_builds_the_grade_and_shortens_the_holds() {
        // The other half of the table's claim: loud does not just mean *more*,
        // it means shorter holds and a harder grade at once.
        let c = Colourist::default();
        let quiet = c.offsets(&SCHEMA, &music(0.05, 0.2));
        let loud = c.offsets(&SCHEMA, &music(0.95, 0.2));
        assert!(loud[at("exposure")] > quiet[at("exposure")]);
        assert!(loud[at("frozen")] > quiet[at("frozen")]);
        assert!(loud[at("retention")] < quiet[at("retention")], "loud must shorten the hold");
    }

    #[test]
    fn silence_pushes_nothing() {
        // What `rest` is for. A digest sitting at its resting value must leave
        // the operator's row exactly where they put it — a model that grades
        // itself during a gap between tracks is a model that looks broken at the
        // one moment everybody is looking at the wall.
        let c = Colourist::default();
        let mut rest = [0.5f32; MUSIC];
        rest[ONSET] = 0.0;
        for v in c.offsets(&SCHEMA, &rest) {
            assert!(v.abs() < 1e-6, "silence pushed a float by {v}");
        }
    }

    #[test]
    fn no_offset_ever_leaves_minus_one_to_one() {
        // The gate the seam with `drive::row` rests on. Past 1 a transmission
        // could reach a state a slider could not, which is the guarantee the
        // whole suite's range declarations mean.
        let c = Colourist::default();
        for r in [0.0f32, 0.5, 1.0] {
            for o in [0.0f32, 0.5, 1.0] {
                for v in c.offsets(&SCHEMA, &music(r, o)) {
                    assert!((-1.0..=1.0).contains(&v), "{v}");
                }
            }
        }
    }

    #[test]
    fn every_rule_names_a_column_that_exists() {
        // The table is hand-written and a column index is a number. That no rule
        // has a float **no** model owns is checked in `metal-render`, where the
        // registry is.
        for r in DEFAULT {
            assert!(r.from < MUSIC, "{} reads column {}", r.float, r.from);
            assert!(r.weight.is_finite() && r.weight != 0.0, "{} weighs {}", r.float, r.weight);
            assert_eq!(r.rest, if r.from == ONSET { 0.0 } else { 0.5 }, "{}", r.float);
        }
    }

    #[test]
    fn no_rule_asks_for_more_authority_than_a_saturated_column_has() {
        // [`swing_of`]'s claim as a gate. A weight is written as *twice* the
        // offset it buys for a centred column and once for onset, so a weight
        // past that ceiling is a number that cannot do anything: `offsets`
        // clamps it, and the line reads as if it were stronger than the one
        // above it while behaving identically. That is exactly the kind of
        // hand-tuned table that quietly stops being tunable.
        for r in DEFAULT {
            let reach = (r.weight * swing_of(r.from)).abs();
            assert!(reach <= 1.0 + 1e-6, "{} reaches {reach}, past where the clamp is", r.float);
        }
    }

    #[test]
    fn a_rail_of_every_column_the_table_reads_moves_something() {
        // The other direction: a table whose weights are all so small that a
        // saturated column barely moves a float is a model that looks like it is
        // not driven at all, which is how this table was written the first time.
        let mut rest = [0.5f32; MUSIC];
        rest[ONSET] = 0.0;
        for column in [RMS, BANDWIDTH, ONSET, CENTROID] {
            let mut rail = rest;
            rail[column] = 1.0;
            let moved = Colourist::default()
                .offsets(&SCHEMA, &rail)
                .iter()
                .map(|v| v.abs())
                .fold(0.0f32, f32::max);
            assert!(moved > 0.3, "column {column} at its rail moved nothing by more than {moved}");
        }
    }

    #[test]
    fn an_authored_value_reduces_to_an_offset_and_back() {
        // The round trip training depends on: reduce an authored value here,
        // and `drive::row` at depth 1 must put it back.
        let (min, max, base) = (0.0f32, 2.0, 1.0);
        for value in [0.0f32, 0.5, 1.0, 1.4, 2.0] {
            let off = offset_of(value, base, min, max, 1.0);
            let back = (base + off * (max - min) * SWING).clamp(min, max);
            // Anything within SWING of the base comes back exactly; past that
            // the offset saturates, which is the honest answer — no signal can
            // move a float further than SWING.
            let reachable = (value - base).abs() <= (max - min) * SWING;
            if reachable {
                assert!((back - value).abs() < 1e-5, "{value} came back as {back}");
            } else {
                assert!(off.abs() == 1.0, "{value} did not saturate");
            }
        }
    }

    #[test]
    fn a_pinned_float_carries_no_information_rather_than_infinite() {
        // `drive == 0` means nothing can move it, so an authored change to one
        // is the operator using a slider, not a grade to learn.
        assert_eq!(offset_of(0.9, 0.5, 0.0, 1.0, 0.0), 0.0);
        assert!(offset_of(0.9, 0.5, 0.0, 1.0, 0.0).is_finite());
    }

    fn taught() -> Vec<Take> {
        // A grade nobody would author by hand: exposure *falls* as the track
        // gets louder, which is the opposite of the default. If the fit works,
        // it overrules the table.
        let mut out = Vec::new();
        for s in 0..4 {
            for k in 0..8 {
                let r = k as f32 / 8.0;
                let mut offsets = vec![0.0f32; OWN];
                offsets[at("exposure")] = 1.0 - r * 2.0;
                out.push(Take {
                    song: format!("s{s}"),
                    music: music(r, 0.3),
                    offsets,
                });
            }
        }
        out
    }

    /// One take set, two fits: the canonical (fine) rows and their 4→1 base
    /// projection. Both are written together and the level selects which one
    /// runs, so a base machine pays no re-fit and is not silently running the
    /// fine weights.
    #[test]
    fn colourist_fits_both_levels_from_one_take() {
        let g = train("wake", &SCHEMA, &taught());
        let w_base = g.w_base.as_ref().expect("a second weight set is fitted");
        assert_eq!(w_base.len(), g.w.len(), "one row per float at both levels");
        assert!(w_base.iter().all(|r| r.len() == INPUTS + 1));

        let m = music(0.7, 0.4);
        let c = Colourist::from_grade(g.clone());
        let direct = |w: &Vec<Vec<f32>>| -> Vec<f32> {
            w.iter()
                .map(|row| {
                    let mut v = row[INPUTS];
                    for k in 0..INPUTS {
                        v += row[k] * m[k];
                    }
                    v.clamp(-1.0, 1.0)
                })
                .collect()
        };
        assert_eq!(
            c.offsets_at(&SCHEMA, &m, metal_schema::TickRate::Fine),
            direct(&g.w),
            "fine selects the canonical fit"
        );
        assert_eq!(
            c.offsets_at(&SCHEMA, &m, metal_schema::TickRate::Base),
            direct(w_base),
            "base selects the projected fit"
        );
        // The two are genuinely different fits — a base machine would otherwise
        // just be running the fine weights under a new name.
        assert_ne!(g.w, *w_base, "the base projection fit must differ");
    }

    #[test]
    fn training_overrules_the_authored_table() {
        let g = train("wake", &SCHEMA, &taught());
        assert!(g.fits("wake", &SCHEMA));
        let c = Colourist::from_grade(g);
        assert!(c.trained());
        let i = at("exposure");
        let quiet = c.offsets(&SCHEMA, &music(0.05, 0.3))[i];
        let loud = c.offsets(&SCHEMA, &music(0.95, 0.3))[i];
        assert!(loud < quiet, "the fit did not overrule the default: {loud} vs {quiet}");
    }

    #[test]
    fn training_beats_the_baseline_and_holds_out_by_song() {
        let g = train("wake", &SCHEMA, &taught());
        let (ours, base) = g.error.expect("four songs");
        assert!(ours < base, "{ours} against {base}");
        assert_eq!(g.songs, 4);
    }

    #[test]
    fn a_grade_fitted_against_another_model_is_refused() {
        // The failure this stops is silent: a grade whose rows line up with a
        // different list of floats grades the wrong parameters and renders
        // fine.
        let g = train("wake", &SCHEMA, &taught());
        assert!(!g.fits("smoke", &SCHEMA));

        static OTHER: &[FloatSpec] = &[spec("a", 0.0, 1.0, 0.5)];
        static OTHER_SCHEMA: Schema = Schema { blocks: &[Block::Floats(OTHER)] };
        assert!(!g.fits("wake", &OTHER_SCHEMA));
    }

    #[test]
    fn a_grade_fitted_before_the_image_half_was_cut_is_refused() {
        // The migration, as a test. A `colourist-*.json` written on 2026-08-25
        // has rows of 29 weights; reading its first sixteen as music columns
        // would grade to numbers fitted for `exposicion`, `contraste`,
        // `altas`… — which renders perfectly and is nonsense.
        let mut g = train("wake", &SCHEMA, &taught());
        for row in g.w.iter_mut() {
            row.extend(std::iter::repeat_n(0.0, 12)); // the twelve picture columns
        }
        assert!(!g.fits("wake", &SCHEMA));
    }

    #[test]
    fn training_on_nothing_is_an_all_zero_grade_rather_than_a_panic() {
        let g = train("wake", &SCHEMA, &[]);
        assert!(g.fits("wake", &SCHEMA));
        let c = Colourist::from_grade(g);
        assert!(c.offsets(&SCHEMA, &music(0.5, 0.5)).iter().all(|v| *v == 0.0));
    }

    #[test]
    fn a_grade_survives_being_written_and_read_back() {
        let dir = std::env::temp_dir().join("metal-reel-colourist");
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let c = Colourist::from_grade(train("wake", &SCHEMA, &taught()));
        c.save(&dir).expect("saves");
        let back = Colourist::load(&dir, "wake", &SCHEMA);
        assert!(back.trained());
        let m = music(0.6, 0.4);
        assert_eq!(back.offsets(&SCHEMA, &m), c.offsets(&SCHEMA, &m));
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn a_grade_on_disk_for_a_schema_that_has_changed_is_ignored() {
        let dir = std::env::temp_dir().join("metal-reel-colourist-stale");
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        Colourist::from_grade(train("wake", &SCHEMA, &taught())).save(&dir).unwrap();

        static OTHER: &[FloatSpec] = &[spec("a", 0.0, 1.0, 0.5)];
        static OTHER_SCHEMA: Schema = Schema { blocks: &[Block::Floats(OTHER)] };
        assert!(!Colourist::load(&dir, "wake", &OTHER_SCHEMA).trained());
        let _ = metal_vfs::remove_dir_all(&dir);
    }
}
