//! Loops as authored content: what a 16-step loop says, and what it says *at
//! a position* of the musical clock ([`crate::clock`]).
//!
//! **One loop, one bar, sixteen steps** (`the loops design notes`, decision 1). A
//! [`Loop`] is aimed at one model (a source's floats and colour slots) or one
//! effect (`fx.*`), and is a list of [`Track`]s; each track takes over exactly
//! one thing — a model float ([`TrackTarget::Float`]), the effect's amount
//! ([`TrackTarget::Amount`]) or a colour slot ([`TrackTarget::Palette`]).
//! The loop's content is a 16-cell keyframe grid ([`Loop::slots`], §5): one
//! optional float row per slot, ordered like the float/amount tracks, `None`
//! holding the previous slot. The per-track `steps` the first cut authored
//! still load — [`Loop::migrate`] turns them into the grid — and still carry
//! colour [`Pick`]s, which a float row cannot. An effect loop may also hold
//! `Float` tracks over
//! any of the effect's declared params (`metal_graph::params::effect_params`;
//! the loops design notes T11), which is how `fx.move`, `fx.particles`, `fx.image-pop`,
//! `fx.forge` and `fx.warp` are looped: several params, one bar. The length is not a choice: [`Loop::validate`]
//! refuses anything but sixteen steps, so every loop in the library shares the
//! clock's bar and they all agree on the "1".
//!
//! **What a loop does not target, it does not touch** (decision 6, the core
//! requirement). [`Loop::eval`] returns only `(target, value)` pairs for its
//! own tracks; the caller writes those over the row it already has and leaves
//! every other parameter on whatever drove it before — `drive::row`'s audio
//! mapping, a fixed value, a wire. The template, not this module, is where
//! "this model wants determinism on its speed and reactivity on its colour" is
//! decided.
//!
//! # Values
//!
//! A float or amount step is `0..1` of the target's declared range, mapped by
//! [`to_range`] against the `FloatSpec` (`metal_schema`'s model floats, or
//! `EFFECTS_FLOATS` for an amount) — so a template stays right when a range is
//! retuned. `null` in a step **holds** the previous step's value (wrapping
//! round the bar: it is a loop); a track with no value at all yields nothing,
//! and its parameter stays the caller's. [`Curve::Step`] holds a value for its
//! whole step; [`Curve::Ramp`] glides to the next step's value across the
//! step's `phase`, so step 16 ramps into step 1 of the next pass.
//!
//! # Palettes
//!
//! A [`PaletteSet`] is a named, pre-tested list of 4-colour groups (skies,
//! backgrounds, psychedelia, rays…; decision 8). A palette track names a set
//! and a colour slot; each step is a [`Pick`] — which colour of the group
//! (`0..4`), and optionally which group — so colour changes land on the beat
//! inside a set somebody already looked at, instead of the chroma columns
//! free-running. Picks never ramp: a blend of two tested colours is a colour
//! nobody tested. [`Library::colour`] resolves a pick to sRGB `0..1`, the
//! same three floats per colour a `ColorSlot` block is made of.
//!
//! # The library
//!
//! [`Library::load`] reads factory templates (in the repo,
//! `data/loops/templates/`) and then the user's (the data dir), a user file
//! replacing a factory one with the same id — so a user edit of a factory
//! template is a copy that wins, and deleting the copy brings the factory one
//! back. Loops are `<dir>/*.json`, palette sets `<dir>/palettes/*.json`, one
//! per file, named by id. A file that does not parse or validate is skipped
//! and reported in [`Library::problems`] rather than failing the whole load:
//! one bad template must not take every other one off the air. Saving goes to
//! the user dir only, through [`crate::atomic::write`]; both dirs are handed
//! in, so tests point them at a scratch dir (rule 8).
//!
//! # JSON
//!
//! ```json
//! { "id": "turing-pulse", "name": "Pulse",
//!   "target": { "model": "turing" },
//!   "tracks": [
//!     { "target": { "kind": "float", "param": "speed" }, "curve": "ramp",
//!       "steps": [0, null, null, null, 1, null, null, null, 0, null, null, null, 1, null, null, null] },
//!     { "target": { "kind": "palette", "slot": "colors", "palette": "skies" },
//!       "steps": [{"colour": 0}, null, null, null, {"colour": 1}, null, null, null,
//!                 {"colour": 2, "group": 1}, null, null, null, {"colour": 3}, null, null, null] } ] }
//! ```
//!
//! ```json
//! { "id": "skies", "name": "Skies",
//!   "groups": [ [ {"oklch": [0.85, 0.06, 230]}, {"srgb": "#7fb2e5"}, {"oklch": [0.5, 0.12, 260]}, {"oklch": [0.25, 0.08, 280]} ] ] }
//! ```
//!
//! Replaces `beatloop.rs` (never compiled; on/off per node over 8-beat counts
//! did not fit — decision 10). What survived of it lives in [`crate::clock`]:
//! the silent free clock and launch on the next "1".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use metal_notation::color::{to_srgb, Oklch};
use metal_schema::FloatSpec;
use serde::{Deserialize, Serialize};

use crate::clock::{Position, STEPS};

/// What a loop is aimed at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// A model (source), by its schema name.
    Model(String),
    /// An effect, by its `fx.*` name without the prefix (`"vignette"`).
    Effect(String),
}

/// The one thing a track takes over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrackTarget {
    /// A model float — or an effect param (T11) — by name; steps are `0..1`
    /// of its declared range.
    Float { param: String },
    /// The effect's amount; steps are `0..1` of its declared range.
    Amount,
    /// A colour slot (a `ColorSlot` block's name) fed from palette set
    /// `palette`; steps are [`Pick`]s.
    Palette {
        slot: String,
        palette: String,
        /// The group picks without one of their own use.
        #[serde(default)]
        group: u16,
    },
}

/// How a value moves inside its step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Curve {
    /// Held for the whole step.
    #[default]
    Step,
    /// Glides to the next step's value across the step.
    Ramp,
}

/// One colour of a palette group, per step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pick {
    /// `0..4`: which colour of the group.
    pub colour: u8,
    /// Which group; `None` is the track's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<u16>,
}

/// One authored step: a number for floats and amounts, a pick for palettes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StepValue {
    Value(f32),
    Pick(Pick),
}

/// One track: one target, sixteen steps (`None` holds the previous one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub target: TrackTarget,
    #[serde(default)]
    pub curve: Curve,
    pub steps: Vec<Option<StepValue>>,
}

/// A loop: see the module note. `tracks` names what the loop takes over and
/// still carries each track's own `steps` for the old shape and for palette
/// picks; `slots` is the 16-cell keyframe grid (§5) — one optional float row
/// per half-beat slot, `None` holding the previous slot. The row is ordered
/// like the loop's float/amount tracks (see [`Loop::slot_tracks`]). An old
/// template with no `slots` is migrated on load by [`Loop::migrate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Loop {
    pub id: String,
    pub name: String,
    #[serde(rename = "target")]
    pub target_kind: TargetKind,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default, skip_serializing_if = "slots_empty")]
    pub slots: [Option<Vec<f32>>; STEPS as usize],
}

fn slots_empty(slots: &[Option<Vec<f32>>; STEPS as usize]) -> bool {
    slots.iter().all(Option::is_none)
}

/// What a track says at a position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    /// `0..1` of the target's declared range — [`to_range`] maps it.
    Unit(f32),
    /// A colour of the track's palette set — [`Library::colour`] resolves it.
    Pick { group: u16, colour: u8 },
}

/// Maps a `0..1` step value into `spec`'s declared range.
pub fn to_range(unit: f32, spec: &FloatSpec) -> f32 {
    spec.min + unit.clamp(0.0, 1.0) * (spec.max - spec.min)
}

/// Why a loop or palette set was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Invalid {
    #[error("id {0:?} must be non-empty [a-z0-9._-]")]
    Id(String),
    #[error("track {track}: {len} steps, a loop is always {STEPS}")]
    Length { track: usize, len: usize },
    #[error("track {track}: step {step} is out of range or the wrong kind for its target")]
    Step { track: usize, step: usize },
    #[error("track {0}: two tracks target the same thing")]
    Duplicate(usize),
    #[error("track {0}: an effect loop targets its amount or its floats, a model loop never its amount")]
    Target(usize),
    #[error("palette set has no groups")]
    Empty,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        && !id.starts_with('.')
}

impl Loop {
    /// The float/amount tracks, in order: `(track index, track)`. The order is
    /// the column order of every `slots` row — the label and curve the slot
    /// editor draws and the value it writes.
    pub fn slot_tracks(&self) -> impl Iterator<Item = (usize, &Track)> {
        self.tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| matches!(t.target, TrackTarget::Float { .. } | TrackTarget::Amount))
    }

    /// Builds the 16-slot grid from the old per-track `steps` (§5): slot `s`
    /// is `Some(row)` when any float track names a value at `s`, each column
    /// holding that track's own value there or the one it holds from before;
    /// a slot no track names is `None` (off — hold the previous slot).
    pub fn from_steps(tracks: &[Track]) -> [Option<Vec<f32>>; STEPS as usize] {
        let cols: Vec<&Track> = tracks
            .iter()
            .filter(|t| matches!(t.target, TrackTarget::Float { .. } | TrackTarget::Amount))
            .collect();
        std::array::from_fn(|s| {
            let named = cols.iter().any(|t| matches!(t.steps.get(s), Some(Some(_))));
            named.then(|| {
                cols.iter()
                    .map(|t| match t.held(s) {
                        Some(StepValue::Value(v)) => v,
                        _ => 0.0,
                    })
                    .collect()
            })
        })
    }

    /// Fills `slots` from the legacy `tracks` when the loop carries none yet;
    /// a loop that already has a grid is left alone. Called on load, so an old
    /// template plays through the new grid unchanged.
    pub fn migrate(&mut self) {
        if self.slots.iter().any(Option::is_some) {
            return;
        }
        self.slots = Loop::from_steps(&self.tracks);
    }

    /// Whether slot `step` is on (its own row), else it holds the previous.
    pub fn slot_on(&self, step: usize) -> bool {
        matches!(self.slots.get(step), Some(Some(_)))
    }

    /// Replaces slot `step` (`None` turns it off); whether it changed.
    pub fn set_slot(&mut self, step: usize, row: Option<Vec<f32>>) -> bool {
        let Some(slot) = self.slots.get_mut(step) else {
            return false;
        };
        if *slot == row {
            return false;
        }
        *slot = row;
        true
    }

    /// The row in force at `step`: its own, or the last one before it,
    /// wrapping round the bar (`None`: no slot anywhere).
    pub fn slot_held(&self, step: usize) -> Option<&Vec<f32>> {
        let n = STEPS as usize;
        (0..n).find_map(|back| self.slots[(step + n - back) % n].as_ref())
    }

    /// Column `col` of the row in force at `step`, gliding to the next slot's
    /// value across `phase` when the track ramps.
    fn slot_value(&self, step: usize, col: usize, curve: Curve, phase: f32) -> Option<f32> {
        let v = self.slot_held(step)?.get(col).copied()?;
        match curve {
            Curve::Step => Some(v),
            Curve::Ramp => {
                let next = self.slot_held((step + 1) % STEPS as usize)?.get(col).copied()?;
                Some(v + (next - v) * phase)
            }
        }
    }

    /// Sixteen slots (or, in the old per-track shape, sixteen steps), values
    /// of the right kind and in range, one track per target, targets that fit
    /// the loop's kind, an id that is safe as a file name. A track may carry
    /// no steps when the loop holds a `slots` grid instead. Range checks
    /// against the schema are the caller's (the catalog test in T6): this
    /// crate's `Loop` does not know which models exist.
    pub fn validate(&self) -> Result<(), Invalid> {
        if !valid_id(&self.id) {
            return Err(Invalid::Id(self.id.clone()));
        }
        for (i, t) in self.tracks.iter().enumerate() {
            if !t.steps.is_empty() && t.steps.len() != STEPS as usize {
                return Err(Invalid::Length { track: i, len: t.steps.len() });
            }
            let fits = match (&self.target_kind, &t.target) {
                (TargetKind::Effect(_), TrackTarget::Amount | TrackTarget::Float { .. }) => true,
                (TargetKind::Model(_), TrackTarget::Float { .. } | TrackTarget::Palette { .. }) => {
                    true
                }
                _ => false,
            };
            if !fits {
                return Err(Invalid::Target(i));
            }
            if self.tracks[..i].iter().any(|o| same_target(&o.target, &t.target)) {
                return Err(Invalid::Duplicate(i));
            }
            let palette = matches!(t.target, TrackTarget::Palette { .. });
            for (s, v) in t.steps.iter().enumerate() {
                let ok = match (v, palette) {
                    (None, _) => true,
                    (Some(StepValue::Value(v)), false) => (0.0..=1.0).contains(v),
                    (Some(StepValue::Pick(p)), true) => p.colour < 4,
                    _ => false,
                };
                if !ok {
                    return Err(Invalid::Step { track: i, step: s });
                }
            }
        }
        let cols = self.slot_tracks().count();
        for (s, row) in self.slots.iter().enumerate() {
            let Some(row) = row else { continue };
            if row.len() != cols {
                return Err(Invalid::Length { track: s, len: row.len() });
            }
            if row.iter().any(|v| !(0.0..=1.0).contains(v)) {
                return Err(Invalid::Step { track: s, step: 0 });
            }
        }
        Ok(())
    }

    /// What every track says at `pos`. The `slots` grid is the shape in force
    /// once a loop carries one (migrated or authored); without it the old
    /// per-track `steps` answer. Tracks with no value at all are left out —
    /// their targets, like every target no track names, stay the caller's.
    pub fn eval(&self, pos: Position) -> Vec<(TrackTarget, Value)> {
        if self.slots.iter().any(Option::is_some) {
            return self.slots_eval(pos);
        }
        self.tracks
            .iter()
            .filter_map(|t| Some((t.target.clone(), t.eval(pos)?)))
            .collect()
    }

    fn slots_eval(&self, pos: Position) -> Vec<(TrackTarget, Value)> {
        let step = pos.step as usize % STEPS as usize;
        let mut col = 0usize;
        let mut out = Vec::new();
        for t in &self.tracks {
            match &t.target {
                TrackTarget::Float { .. } | TrackTarget::Amount => {
                    let c = col;
                    col += 1;
                    if let Some(v) = self.slot_value(step, c, t.curve, pos.phase) {
                        out.push((t.target.clone(), Value::Unit(v)));
                    }
                }
                TrackTarget::Palette { .. } => {
                    if let Some(v) = t.eval(pos) {
                        out.push((t.target.clone(), v));
                    }
                }
            }
        }
        out
    }
}

fn same_target(a: &TrackTarget, b: &TrackTarget) -> bool {
    match (a, b) {
        (TrackTarget::Palette { slot: x, .. }, TrackTarget::Palette { slot: y, .. }) => x == y,
        _ => a == b,
    }
}

impl Track {
    /// The value in force at `step`: its own, or the last one before it,
    /// wrapping round the bar.
    fn held(&self, step: usize) -> Option<StepValue> {
        let n = self.steps.len();
        (0..n).find_map(|back| self.steps[(step + n - back) % n])
    }

    /// What this track says at `pos` (`None`: no value anywhere).
    pub fn eval(&self, pos: Position) -> Option<Value> {
        if self.steps.is_empty() {
            return None;
        }
        let step = pos.step as usize % self.steps.len();
        match self.held(step)? {
            StepValue::Value(v) => {
                let v = match self.curve {
                    Curve::Step => v,
                    Curve::Ramp => match self.held((step + 1) % self.steps.len()) {
                        Some(StepValue::Value(next)) => v + (next - v) * pos.phase,
                        _ => v,
                    },
                };
                Some(Value::Unit(v))
            }
            StepValue::Pick(p) => {
                let track_group = match self.target {
                    TrackTarget::Palette { group, .. } => group,
                    _ => 0,
                };
                Some(Value::Pick { group: p.group.unwrap_or(track_group), colour: p.colour })
            }
        }
    }
}

/// One colour as authored: OkLCh (`[l, c, h°]`) or an sRGB hex string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Colour {
    Oklch([f32; 3]),
    Srgb(String),
}

impl Colour {
    /// sRGB `0..1` (OkLCh gamut-mapped by chroma, as `drive.rs` does); `None`
    /// for a hex string that is not `#rrggbb`.
    pub fn rgb(&self) -> Option<[f32; 3]> {
        match self {
            Colour::Oklch([l, c, h]) => {
                let rgb = to_srgb(Oklch { l: *l, c: *c, h: *h });
                Some([rgb.r, rgb.g, rgb.b].map(|v| v as f32 / 255.0))
            }
            Colour::Srgb(hex) => {
                let hex = hex.strip_prefix('#')?;
                if hex.len() != 6 {
                    return None;
                }
                let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
                Some([byte(0)?, byte(2)?, byte(4)?].map(|v| v as f32 / 255.0))
            }
        }
    }
}

/// A named list of pre-tested 4-colour groups.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaletteSet {
    pub id: String,
    pub name: String,
    pub groups: Vec<[Colour; 4]>,
}

impl PaletteSet {
    pub fn validate(&self) -> Result<(), Invalid> {
        if !valid_id(&self.id) {
            return Err(Invalid::Id(self.id.clone()));
        }
        if self.groups.is_empty() {
            return Err(Invalid::Empty);
        }
        for (g, group) in self.groups.iter().enumerate() {
            if let Some(c) = group.iter().position(|c| c.rgb().is_none()) {
                return Err(Invalid::Step { track: g, step: c });
            }
        }
        Ok(())
    }

    /// Colour `colour` of group `group`, sRGB `0..1`. A group past the end
    /// wraps, so a template written for a longer set still lands somewhere
    /// tested.
    pub fn colour(&self, group: u16, colour: u8) -> Option<[f32; 3]> {
        if self.groups.is_empty() {
            return None;
        }
        self.groups[group as usize % self.groups.len()]
            .get(colour as usize)?
            .rgb()
    }
}

/// Where a template came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Factory,
    User,
}

/// Factory plus user templates — see the module note.
#[derive(Debug, Clone)]
pub struct Library {
    loops: BTreeMap<String, (Loop, Origin)>,
    palettes: BTreeMap<String, (PaletteSet, Origin)>,
    user_dir: PathBuf,
    /// Files skipped on load, and why.
    pub problems: Vec<(PathBuf, String)>,
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = metal_vfs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| metal_vfs::is_file(&p) && p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
}

impl Library {
    /// Loads `factory` then `user` (either may be missing).
    pub fn load(factory: &Path, user: &Path) -> Library {
        let mut lib = Library {
            loops: BTreeMap::new(),
            palettes: BTreeMap::new(),
            user_dir: user.to_path_buf(),
            problems: Vec::new(),
        };
        for (dir, origin) in [(factory, Origin::Factory), (user, Origin::User)] {
            for path in json_files(dir) {
                match read::<Loop>(&path)
                    .map(|mut l| {
                        l.migrate();
                        l
                    })
                    .and_then(|l| l.validate().map(|_| l).map_err(|e| e.to_string()))
                {
                    Ok(l) => {
                        lib.loops.insert(l.id.clone(), (l, origin));
                    }
                    Err(e) => lib.problems.push((path, e)),
                }
            }
            for path in json_files(&dir.join("palettes")) {
                match read::<PaletteSet>(&path)
                    .and_then(|p| p.validate().map(|_| p).map_err(|e| e.to_string()))
                {
                    Ok(p) => {
                        lib.palettes.insert(p.id.clone(), (p, origin));
                    }
                    Err(e) => lib.problems.push((path, e)),
                }
            }
        }
        lib
    }

    pub fn get(&self, id: &str) -> Option<&Loop> {
        self.loops.get(id).map(|(l, _)| l)
    }

    pub fn origin(&self, id: &str) -> Option<Origin> {
        self.loops.get(id).map(|(_, o)| *o)
    }

    /// Every loop, by id.
    pub fn loops(&self) -> impl Iterator<Item = &Loop> {
        self.loops.values().map(|(l, _)| l)
    }

    /// The loops aimed at `target`, by id — what a picker lists.
    pub fn for_target<'a>(&'a self, target: &'a TargetKind) -> impl Iterator<Item = &'a Loop> {
        self.loops().filter(move |l| &l.target_kind == target)
    }

    /// The factory loop a target plays when nothing was chosen: the first
    /// (by id) of the target's *own* templates — `<model>-…` for a model,
    /// `fx-<effect>-…` for an effect — else the first factory loop aimed at
    /// it. Own-prefix first so a themed pack (`dembow-*`, `strobe-*`,
    /// `warp-*`, the loops design notes T13/T14) never becomes a target's default just
    /// by sorting earlier.
    pub fn default_for(&self, target: &TargetKind) -> Option<&Loop> {
        let own = match target {
            TargetKind::Model(m) => format!("{m}-"),
            TargetKind::Effect(e) => format!("fx-{}-", e.replace(['_', '.'], "-")),
        };
        let factory = || {
            self.loops
                .values()
                .filter(move |(l, o)| *o == Origin::Factory && &l.target_kind == target)
                .map(|(l, _)| l)
        };
        factory().find(|l| l.id.starts_with(&own)).or_else(|| factory().next())
    }

    pub fn palette(&self, id: &str) -> Option<&PaletteSet> {
        self.palettes.get(id).map(|(p, _)| p)
    }

    pub fn palettes(&self) -> impl Iterator<Item = &PaletteSet> {
        self.palettes.values().map(|(p, _)| p)
    }

    /// The sRGB colour a palette track's value stands for.
    pub fn colour(&self, target: &TrackTarget, value: Value) -> Option<[f32; 3]> {
        match (target, value) {
            (TrackTarget::Palette { palette, .. }, Value::Pick { group, colour }) => {
                self.palette(palette)?.colour(group, colour)
            }
            _ => None,
        }
    }

    /// Validates and saves `l` to the user dir as `<id>.json`, replacing any
    /// template with that id.
    pub fn save(&mut self, l: Loop) -> std::io::Result<PathBuf> {
        l.validate().map_err(invalid)?;
        let path = self.user_dir.join(format!("{}.json", l.id));
        write_json(&path, &l)?;
        self.loops.insert(l.id.clone(), (l, Origin::User));
        Ok(path)
    }

    /// Validates and saves `p` to the user dir's `palettes/<id>.json`.
    pub fn save_palette(&mut self, p: PaletteSet) -> std::io::Result<PathBuf> {
        p.validate().map_err(invalid)?;
        let path = self.user_dir.join("palettes").join(format!("{}.json", p.id));
        write_json(&path, &p)?;
        self.palettes.insert(p.id.clone(), (p, Origin::User));
        Ok(path)
    }
}

fn invalid(e: Invalid) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
}

fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let bytes = metal_vfs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        metal_vfs::create_dir_all(dir)?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    crate::atomic::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fw-pattern-{tag}-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        dir
    }

    fn at(step: u8, phase: f32) -> Position {
        Position { bar: 3, step, phase, beat_in_bar: step / 4 }
    }

    fn steps(v: &[(usize, f32)]) -> Vec<Option<StepValue>> {
        let mut s = vec![None; 16];
        for &(i, x) in v {
            s[i] = Some(StepValue::Value(x));
        }
        s
    }

    fn model_loop() -> Loop {
        Loop {
            id: "turing-pulse".into(),
            name: "Pulse".into(),
            target_kind: TargetKind::Model("turing".into()),
            tracks: vec![
                Track {
                    target: TrackTarget::Float { param: "speed".into() },
                    curve: Curve::Ramp,
                    steps: steps(&[(0, 0.0), (4, 1.0), (8, 0.0), (12, 1.0)]),
                },
                Track {
                    target: TrackTarget::Palette { slot: "colors".into(), palette: "skies".into(), group: 0 },
                    curve: Curve::Step,
                    steps: (0..16)
                        .map(|i| {
                            (i % 4 == 0).then_some(StepValue::Pick(Pick {
                                colour: (i / 4) as u8,
                                group: (i == 8).then_some(1),
                            }))
                        })
                        .collect(),
                },
            ],
            slots: std::array::from_fn(|_| None),
        }
    }

    fn skies() -> PaletteSet {
        let c = |l: f32| Colour::Oklch([l, 0.05, 230.0]);
        PaletteSet {
            id: "skies".into(),
            name: "Skies".into(),
            groups: vec![
                [c(0.9), Colour::Srgb("#7fb2e5".into()), c(0.5), c(0.25)],
                [c(0.8), c(0.6), c(0.4), Colour::Srgb("#000000".into())],
            ],
        }
    }

    #[test]
    fn only_targeted_parameters_come_back() {
        let out = model_loop().eval(at(0, 0.0));
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|(t, _)| !matches!(t, TrackTarget::Float { param } if param != "speed")));
        // A track with nothing in it yields nothing: its param stays the caller's.
        let mut l = model_loop();
        l.tracks[0].steps = vec![None; 16];
        assert_eq!(l.eval(at(5, 0.0)).len(), 1);
    }

    #[test]
    fn null_holds_and_wraps_round_the_bar() {
        let t = Track {
            target: TrackTarget::Amount,
            curve: Curve::Step,
            steps: steps(&[(4, 0.3), (10, 0.7)]),
        };
        assert_eq!(t.eval(at(6, 0.5)), Some(Value::Unit(0.3)));
        assert_eq!(t.eval(at(10, 0.0)), Some(Value::Unit(0.7)));
        // Steps 0..4 hold step 10 from the previous pass.
        assert_eq!(t.eval(at(1, 0.0)), Some(Value::Unit(0.7)));
    }

    #[test]
    fn a_ramp_glides_to_the_next_step_across_the_phase() {
        let l = model_loop();
        let speed = |pos| match l.tracks[0].eval(pos) {
            Some(Value::Unit(v)) => v,
            other => panic!("{other:?}"),
        };
        // Step 3 holds 0 and ramps into step 4's 1.
        assert!((speed(at(3, 0.5)) - 0.5).abs() < 1e-6);
        assert_eq!(speed(at(2, 0.5)), 0.0);
        // Step 15 ramps into the next pass's step 0.
        assert!((speed(at(15, 0.25)) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn picks_step_on_the_beat_and_take_their_own_group() {
        let l = model_loop();
        let pal = &l.tracks[1];
        assert_eq!(pal.eval(at(5, 0.9)), Some(Value::Pick { group: 0, colour: 1 }));
        assert_eq!(pal.eval(at(9, 0.0)), Some(Value::Pick { group: 1, colour: 2 }));
        let set = skies();
        assert_eq!(set.colour(0, 1), Some([127.0 / 255.0, 178.0 / 255.0, 229.0 / 255.0]));
        assert_eq!(set.colour(3, 3), Some([0.0, 0.0, 0.0])); // group wraps
    }

    #[test]
    fn values_map_into_the_declared_range() {
        let spec = metal_schema::EFFECTS_FLOATS
            .iter()
            .find(|s| s.name == "zoom")
            .expect("zoom is an effect");
        assert_eq!(to_range(0.0, spec), spec.min);
        assert_eq!(to_range(1.0, spec), spec.max);
        assert_eq!(to_range(2.0, spec), spec.max);
    }

    #[test]
    fn validation_refuses_what_is_not_a_16_step_loop() {
        assert_eq!(model_loop().validate(), Ok(()));
        let mut l = model_loop();
        l.tracks[0].steps.pop();
        assert!(matches!(l.validate(), Err(Invalid::Length { len: 15, .. })));
        let mut l = model_loop();
        l.tracks[0].steps[2] = Some(StepValue::Value(1.5));
        assert!(matches!(l.validate(), Err(Invalid::Step { track: 0, step: 2 })));
        let mut l = model_loop();
        l.tracks[1].steps[2] = Some(StepValue::Value(0.5));
        assert!(matches!(l.validate(), Err(Invalid::Step { track: 1, step: 2 })));
        let mut l = model_loop();
        l.tracks.push(l.tracks[0].clone());
        assert_eq!(l.validate(), Err(Invalid::Duplicate(2)));
        let mut l = model_loop();
        l.tracks[0].target = TrackTarget::Amount;
        assert_eq!(l.validate(), Err(Invalid::Target(0)));
        let mut fx = model_loop();
        fx.target_kind = TargetKind::Effect("move".into());
        fx.tracks.truncate(1);
        assert_eq!(fx.validate(), Ok(()), "an effect loop may hold float tracks (T11)");
        fx.tracks.push(model_loop().tracks[1].clone());
        assert_eq!(fx.validate(), Err(Invalid::Target(1)), "but never a palette track");
        let mut l = model_loop();
        l.id = "../escape".into();
        assert!(matches!(l.validate(), Err(Invalid::Id(_))));
    }

    #[test]
    fn a_slot_toggles_on_and_off() {
        let mut l = model_loop();
        l.migrate();
        assert!(l.slot_on(0));
        assert!(!l.slot_on(1), "step 1 is off — it holds step 0");
        // Off holds the previous slot: step 3 still says step 0's 0.0.
        assert_eq!(l.eval(at(3, 0.0)).iter().find_map(|(t, v)| matches!(t, TrackTarget::Float { param } if param == "speed").then_some(*v)), Some(Value::Unit(0.0)));
        // Turning a slot on writes its own row; off turns it back.
        assert!(l.set_slot(1, Some(vec![0.5])));
        assert!(l.slot_on(1));
        assert_eq!(l.eval(at(1, 0.0)).iter().find_map(|(t, v)| matches!(t, TrackTarget::Float { .. }).then_some(*v)), Some(Value::Unit(0.5)));
        assert!(l.set_slot(1, None));
        assert!(!l.slot_on(1));
        assert!(!l.set_slot(1, None), "toggling off twice changes nothing");
    }

    #[test]
    fn migration_maps_old_steps_to_sixteen_slots() {
        let mut l = model_loop();
        assert!(l.slots.iter().all(Option::is_none), "the old shape carries no grid");
        l.migrate();
        assert_eq!(l.slots.len(), STEPS as usize);
        assert_eq!(l.slots[0], Some(vec![0.0]), "speed's own step 0");
        assert_eq!(l.slots[1], None, "no track names step 1");
        assert_eq!(l.slots[4], Some(vec![1.0]));
        assert_eq!(l.slot_tracks().count(), 1, "the palette track is no slot column");
        // The migrated grid says what the old steps said: ramp across phase.
        let speed = |pos| l.eval(pos).into_iter().find_map(|(t, v)| matches!(t, TrackTarget::Float { .. }).then_some(v));
        assert_eq!(speed(at(2, 0.5)), Some(Value::Unit(0.0)));
        assert_eq!(speed(at(3, 0.5)), Some(Value::Unit(0.5)));
        // The palette track is untouched by the migration.
        assert_eq!(l.tracks[1].eval(at(5, 0.9)), Some(Value::Pick { group: 0, colour: 1 }));
        assert_eq!(l.validate(), Ok(()));
    }

    #[test]
    fn steps_is_sixteen_half_beat_slots() {
        assert_eq!(STEPS, 16);
        assert_eq!(crate::clock::STEPS_PER_BEAT, 2);
    }

    #[test]
    fn old_json_without_a_grid_loads_and_migrates() {
        let text = r#"{"id":"legacy","name":"L","target":{"model":"turing"},
            "tracks":[{"target":{"kind":"float","param":"speed"},"steps":[1,null,null,null,0,null,null,null,null,null,null,null,null,null,null,null]}]}"#;
        let mut l: Loop = serde_json::from_str(text).unwrap();
        assert!(l.slots.iter().all(Option::is_none));
        l.migrate();
        assert_eq!(l.slots[0], Some(vec![1.0]));
        assert_eq!(l.slots[4], Some(vec![0.0]));
        assert_eq!(l.validate(), Ok(()));
    }

    #[test]
    fn json_round_trips_in_the_documented_shape() {
        let text = serde_json::to_string(&model_loop()).unwrap();
        assert!(text.contains(r#""target":{"model":"turing"}"#), "{text}");
        assert!(text.contains(r#""kind":"float","param":"speed""#), "{text}");
        assert!(text.contains(r#""curve":"ramp""#), "{text}");
        let back: Loop = serde_json::from_str(&text).unwrap();
        assert_eq!(back, model_loop());
        let fx: Loop = serde_json::from_str(
            r#"{"id":"vig","name":"V","target":{"effect":"vignette"},
               "tracks":[{"target":{"kind":"amount"},"steps":[1,null,null,null,0.5,null,null,null,0,null,null,null,0.25,null,null,null]}]}"#,
        )
        .unwrap();
        assert_eq!(fx.validate(), Ok(()));
        assert_eq!(fx.tracks[0].curve, Curve::Step);
        let set: PaletteSet = serde_json::from_str(&serde_json::to_string(&skies()).unwrap()).unwrap();
        assert_eq!(set, skies());
    }

    #[test]
    fn user_templates_override_factory_ones_by_id() {
        let root = scratch("lib");
        let (factory, user) = (root.join("factory"), root.join("user"));
        metal_vfs::create_dir_all(factory.join("palettes")).unwrap();
        let write = |p: PathBuf, v: String| metal_vfs::write(p, v).unwrap();
        write(factory.join("turing-pulse.json"), serde_json::to_string(&model_loop()).unwrap());
        write(factory.join("palettes/skies.json"), serde_json::to_string(&skies()).unwrap());
        write(factory.join("broken.json"), "{ not json".into());

        let mut lib = Library::load(&factory, &user);
        assert_eq!(lib.origin("turing-pulse"), Some(Origin::Factory));
        assert_eq!(lib.problems.len(), 1);
        let l = lib.get("turing-pulse").unwrap();
        let (t, v) = l.eval(at(4, 0.0)).into_iter().find(|(t, _)| matches!(t, TrackTarget::Palette { .. })).unwrap();
        assert!(lib.colour(&t, v).is_some());

        let mut mine = model_loop();
        mine.name = "Mine".into();
        let path = lib.save(mine).unwrap();
        assert!(path.starts_with(&user));
        let lib = Library::load(&factory, &user);
        assert_eq!(lib.get("turing-pulse").unwrap().name, "Mine");
        assert_eq!(lib.origin("turing-pulse"), Some(Origin::User));
        assert_eq!(lib.for_target(&TargetKind::Model("turing".into())).count(), 1);

        // Default: the target's own-prefix template beats a themed pack that
        // sorts earlier.
        let mut pack = model_loop();
        pack.id = "dembow-turing".into();
        metal_vfs::write(factory.join("dembow-turing.json"), serde_json::to_string(&pack).unwrap()).unwrap();
        metal_vfs::remove_file(user.join("turing-pulse.json")).unwrap();
        let lib = Library::load(&factory, &user);
        let turing = TargetKind::Model("turing".into());
        assert_eq!(lib.loops().find(|l| l.target_kind == turing).unwrap().id, "dembow-turing");
        assert_eq!(lib.default_for(&turing).unwrap().id, "turing-pulse".to_string());
        metal_vfs::remove_file(factory.join("turing-pulse.json")).unwrap();
        let lib = Library::load(&factory, &user);
        assert_eq!(lib.default_for(&turing).unwrap().id, "dembow-turing", "no own template: the first");

        let mut bad = model_loop();
        bad.tracks[0].steps.truncate(8);
        let mut lib = lib;
        assert!(lib.save(bad).is_err());
        let _ = metal_vfs::remove_dir_all(&root);
    }
}
