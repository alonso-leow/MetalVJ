//! The lanes panel's data: the movement scratch ([`MovementLane`], what the
//! overlays grid loads and clears) and the sequence editor's state
//! ([`IngestLane`] — the song's digest, its hits, and the stacked model
//! tracks with their keypoints, the active one in the lane's own fields so
//! the properties panel edits `row` directly).
//!
//! The painting is [`super::timeline`] (2026-09-30): one timeline for the
//! song and every track, replacing the three strips this module used to
//! paint (the scratch's cell grid, the ingest bands and the router band).
//!
//! The scratch keeps a **snapshot undo** — one full copy of the cell grid
//! per op — and the editor a grouped one (a paste or a retime is one step).
//!
//! **Regla 6.** Nothing here calls the door; the panel does.

use std::collections::BTreeMap;

use egui::Color32;

use crate::shell::classes::Theme;

// ---------------------------------------------------------------------------
// the data model
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// the authoring scratch
// ---------------------------------------------------------------------------

/// What state the lanes panel is in — the two-state decision
/// (plan-opus.md punto 7, `paneles-inferiores.md` § P2): **lanes** authors a
/// phrase offline; **overlays** performs saved ones live. The state belongs
/// to the whole panel ([`crate::shell::panels::lanes::PanelState`]), not to
/// the scratch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Lanes,
    Overlays,
}

impl Default for Mode {
    /// Overlays is the panel's state until context asks for lanes (a new
    /// sequence, or a song imported into the editor).
    fn default() -> Self {
        Mode::Overlays
    }
}

/// One undo step: the whole cell grid (and length) as it was before the op.
/// Snapshot undo, per the module note — a restore cannot be mis-applied.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    cells: BTreeMap<(usize, usize), f64>,
    len: usize,
}

/// One copied passage: cells re-based to start at tick 0, so pasting at any
/// tick is the same shift — a movement is a shape, not a position, the same
/// rule `author::library::slice` states.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub cells: BTreeMap<(usize, usize), f64>,
    pub span: usize,
}

/// The lanes panel's movement scratch — everything the authoring state
/// remembers between frames. Held by `lanes.rs`'s `PanelState` as `mv`; the
/// panel's own [`Mode`] lives on `PanelState`, not here.
#[derive(Debug, Clone, Default)]
pub struct MovementLane {
    /// The scratch movement's name — what "save as sequence" posts.
    pub name: String,
    /// Which model the scratch's rows fit (a schema id; `""` = none picked).
    pub model: String,
    /// The phrase's length in ticks.
    pub len: usize,
    /// The scratch's cells: `(tick, lane)` → value. f64, not f32: the save
    /// route's body is f64, and an f32 0.4 serializes as
    /// 0.4000000059604645 — the value the operator typed is the value the
    /// library file keeps, so the scratch stores the body's own width.
    pub cells: BTreeMap<(usize, usize), f64>,
    /// The picked cell, if any — what "write"/"erase" act on.
    pub selected: Option<(usize, usize)>,
    /// The value field the operator is mid-typing (never sent alone).
    #[cfg(test)]
    pub value: String,
    /// The span (passage) box's `[from, to)` span and its destination —
    /// the edit-half fields the copy/clear/move buttons act on, remembered
    /// between frames so an operator can adjust one bound and retry.
    #[cfg(test)]
    pub span_from: usize,
    #[cfg(test)]
    pub span_to: usize,
    #[cfg(test)]
    pub span_dest: usize,
    undo: Vec<Snapshot>,
    #[cfg(test)]
    pub clipboard: Option<Clip>,
    pub note: Option<String>,
    /// The arm target the overlays mode posts to.
    pub device: Option<String>,
    pub layer: Option<String>,
    #[cfg(test)]
    pub looping: bool,
}

impl MovementLane {
    #[cfg(test)]
    /// Writes (or overwrites) one cell, pushing an undo snapshot.
    pub fn write(&mut self, tick: usize, lane: usize, value: f64) {
        self.push_undo();
        self.cells.insert((tick, lane), value);
        self.len = self.len.max(tick + 1);
    }

    #[cfg(test)]
    /// Clears one cell, pushing an undo snapshot **before** the removal —
    /// the snapshot is the state to go back *to*, so it has to be taken
    /// before the clear happens, not after (the same order every other
    /// op keeps). Clearing an absent cell changes nothing and pushes
    /// nothing.
    pub fn clear(&mut self, tick: usize, lane: usize) {
        if self.cells.contains_key(&(tick, lane)) {
            self.push_undo();
            self.cells.remove(&(tick, lane));
        }
    }

    #[cfg(test)]
    /// Clears a range `[from, to)` of one lane, pushing one snapshot for
    /// the whole clear — one op, one undo.
    pub fn clear_range(&mut self, lane: usize, from: usize, to: usize) {
        let had = self
            .cells
            .keys()
            .any(|(t, l)| *l == lane && *t >= from && *t < to);
        if had {
            self.push_undo();
            self.cells
                .retain(|(t, l), _| !(*l == lane && *t >= from && *t < to));
        }
    }

    #[cfg(test)]
    /// Clears the span `[from, to)` of *every* lane — the span box's
    /// "clear span". One snapshot for the whole span, the same one-op-
    /// one-undo rule `clear_range` keeps per lane.
    pub fn clear_span(&mut self, from: usize, to: usize) {
        if from >= to {
            return;
        }
        let had = self.cells.keys().any(|(t, _)| *t >= from && *t < to);
        if had {
            self.push_undo();
            self.cells.retain(|t, _| t.0 < from || t.0 >= to);
        }
    }

    #[cfg(test)]
    /// Moves the span `[from, to)` of *every* lane to start at `dest` —
    /// the span box's "move span": the passage lifts out and lands
    /// re-based, cells already at the destination are overwritten, and one
    /// snapshot covers the whole move. A backwards move (`dest` inside the
    /// span) is refused, the same refusal `paste` implies by just
    /// overwriting — an overlapping move is a copy the operator can still
    /// undo, but inventing a wraparound or a fold would not be.
    pub fn move_span(&mut self, from: usize, to: usize, dest: usize) {
        if from >= to || (dest >= from && dest < to) {
            return;
        }
        let moved: Vec<((usize, usize), f64)> = self
            .cells
            .iter()
            .filter(|((t, _), _)| *t >= from && *t < to)
            .map(|((t, l), v)| ((t - from + dest, *l), *v))
            .collect();
        if moved.is_empty() {
            return;
        }
        self.push_undo();
        self.cells.retain(|t, _| t.0 < from || t.0 >= to);
        for ((t, l), v) in moved {
            self.cells.insert((t, l), v);
            self.len = self.len.max(t + 1);
        }
    }

    #[cfg(test)]
    /// Copies the range `[from, to)` of *every* lane into the clipboard,
    /// re-based to tick 0.
    pub fn copy(&mut self, from: usize, to: usize) {
        let mut cells = BTreeMap::new();
        for ((t, l), v) in &self.cells {
            if *t >= from && *t < to {
                cells.insert((t - from, *l), *v);
            }
        }
        if !cells.is_empty() {
            self.clipboard = Some(Clip {
                cells,
                span: to - from,
            });
        }
    }

    #[cfg(test)]
    /// Pastes the clipboard at `tick`, pushing one snapshot.
    pub fn paste(&mut self, tick: usize) {
        let Some(clip) = self.clipboard.clone() else {
            return;
        };
        self.push_undo();
        for ((t, l), v) in clip.cells {
            let t = t + tick;
            self.cells.insert((t, l), v);
            self.len = self.len.max(t + 1);
        }
    }

    #[cfg(test)]
    /// Restores the last snapshot, if there is one.
    pub fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.cells = s.cells;
            self.len = s.len;
        }
    }

    #[cfg(test)]
    fn push_undo(&mut self) {
        self.undo.push(Snapshot {
            cells: self.cells.clone(),
            len: self.len,
        });
        // A scratch is one phrase; the stack grows only with edits, but a
        // cap keeps a long session from hoarding copies.
        if self.undo.len() > 128 {
            self.undo.remove(0);
        }
    }

    #[cfg(test)]
    /// The scratch as the ticks array `POST /author/models/library` takes:
    /// one row of `width` floats per tick that has any cell, in tick order,
    /// missing lanes at 0.0 — exactly the shape `author::library::check`
    /// holds a model movement to (a row of the schema's width, numbers
    /// only). `None` when the scratch is empty or nameless: the panel turns
    /// that into the route's own refusal text without posting.
    pub fn ticks_json(&self, width: usize) -> Option<serde_json::Value> {
        let name = self.name.trim();
        if name.is_empty() || self.cells.is_empty() || width == 0 {
            return None;
        }
        let ticks: Vec<serde_json::Value> = (0..self.len)
            .filter_map(|t| {
                let any = self.cells.keys().any(|(tt, _)| *tt == t);
                if !any {
                    return None;
                }
                let row: Vec<f64> = (0..width)
                    .map(|l| self.cells.get(&(t, l)).copied().unwrap_or(0.0))
                    .collect();
                Some(serde_json::json!(row))
            })
            .collect();
        if ticks.is_empty() {
            return None;
        }
        Some(serde_json::json!({
            "name": name,
            "model": self.model,
            "ticks": ticks,
        }))
    }

    #[cfg(test)]
    /// The scratch as the body `POST /author/models/example` takes —
    /// `S.AP`'s Colourist lane, the same cells [`Self::ticks_json`] reads,
    /// wrapped for the *other* save target: sparse `{tick, floats}` objects
    /// (`example::Tick`'s own shape) under `{model, song, rate}`, rather than
    /// a bare dense array of rows under `{name, model}`. `None` for the same
    /// reasons `ticks_json` is: nothing authored, no model, or (here) no
    /// song picked — a dataset with no song is not a dataset, it is a
    /// keyframe with nowhere to train against.
    pub fn example_ticks_json(
        &self,
        width: usize,
        song: &str,
        rate: f32,
        target: Option<(&str, &str)>,
    ) -> Option<serde_json::Value> {
        let song = song.trim();
        if self.model.is_empty() || self.cells.is_empty() || width == 0 || song.is_empty() {
            return None;
        }
        let ticks: Vec<serde_json::Value> = (0..self.len)
            .filter_map(|t| {
                let any = self.cells.keys().any(|(tt, _)| *tt == t);
                if !any {
                    return None;
                }
                let row: Vec<f64> = (0..width)
                    .map(|l| self.cells.get(&(t, l)).copied().unwrap_or(0.0))
                    .collect();
                Some(serde_json::json!({ "tick": t, "floats": row }))
            })
            .collect();
        if ticks.is_empty() {
            return None;
        }
        let mut body = serde_json::json!({
            "model": self.model,
            "song": song,
            "rate": rate,
            "ticks": ticks,
        });
        if let Some((device, layer)) = target {
            body["device"] = serde_json::json!(device);
            body["layer"] = serde_json::json!(layer);
        }
        Some(body)
    }

    /// Loads a movement off the library into the scratch — the "open" of
    /// the overlays mode. Clears undo (the opened phrase is a new editing
    /// session, not a continuation of the last one) and returns false for
    /// anything that is not a model movement's shape (rig ticks are frames;
    /// authoring them needs rig capture, which the native shell has not got
    /// a write path for yet — the canvas reads them, [`lane_rows_from_movement`]
    /// paints them, but the scratch edits floats only).
    pub fn load(&mut self, movement: &serde_json::Value) -> bool {
        let Some(model) = movement["model"].as_str().map(str::to_string) else {
            return false;
        };
        let mut cells = BTreeMap::new();
        for (i, tick) in movement["ticks"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            for (lane, v) in tick
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                if let Some(v) = v.as_f64() {
                    cells.insert((i, lane), v);
                }
            }
        }
        if cells.is_empty() {
            return false;
        }
        self.name = movement["name"].as_str().unwrap_or_default().to_string();
        self.model = model;
        self.len = movement["ticks"].as_array().map(Vec::len).unwrap_or(0);
        self.cells = cells;
        self.undo.clear();
        self.selected = None;
        self.note = None;
        true
    }
}

// ---------------------------------------------------------------------------
// the ingest columns lane — `S.AP` rebuilt against `page/suite.js`'s own
// three canvases, not `MovementLane`'s blank cell grid
// ---------------------------------------------------------------------------

/// One song's digest, as `GET /author/grade/matrix` hands it back —
/// [`crate::author::grade::Matrix`]'s shape, mirrored here so this module
/// stays usable from a glow-only build the way [`MovementLane`] already is
/// (`crate::author` is `gpu`-gated; parsing the door's JSON reply is not).
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct DigestMatrix {
    pub song: String,
    /// Read by the tests' fixtures only; the lane draws from `ticks`.
    #[cfg(test)]
    pub rate: f32,
    pub ticks: usize,
    pub rms: Vec<f32>,
    pub bandwidth: Vec<f32>,
    pub onset: Vec<f32>,
    pub centroid: Vec<f32>,
    pub chroma: Vec<[f32; 12]>,
    /// The thirteen voice columns, one row per tick — zeroed when the digest
    /// carries no voice block, so [`Self::has_voice`] is what the timeline
    /// trusts. `#[serde(default)]` lets a pre-V3 door reply (no `voice` key)
    /// still parse.
    #[serde(default)]
    pub voice: Vec<[f32; metal_schema::VOICE]>,
    /// The digest's own file carried the voice block (`npy::has_voice`).
    /// `false` is "not digested for voice", not "silent".
    #[serde(default)]
    pub has_voice: bool,
    /// The only digest is an 8 Hz base file with no canonical sibling, held as
    /// steps at the fine level — the "base-only" badge (`npy::is_base_only`).
    #[serde(default)]
    pub base_only: bool,
}

/// The row at `tick`, each float eased between its own keys
/// (`metal_schema::keys`, the same as `dense::interpolate`): a keyframe that
/// names other floats does not stall it. Flat past either end, `None` when
/// nothing is authored yet, so the caller leaves whatever the operator last
/// dialled alone (`suite.js`'s own rule: *"an unauthored model keeps whatever
/// the operator dialled in"*, not the defaults dense playback would fall
/// back to).
pub fn interpolate_row(keyframes: &BTreeMap<usize, Vec<f32>>, tick: usize) -> Option<Vec<f32>> {
    let width = keyframes.values().next()?.len();
    let tracks = metal_schema::keys::tracks(keyframes.iter().map(|(t, f)| (*t, f.as_slice())), width);
    metal_schema::keys::row_at(&tracks, tick as f32)
}

/// F6: the canonical order a fixture page's lane columns appear in — the
/// eleven selectors a profile may declare, in the order the plan lists them.
pub const FIXTURE_COLUMN_ORDER: &[&str] = &[
    "level", "hue", "sat", "strobe", "pan", "tilt", "cct", "zoom", "focus", "iris", "frost",
];

/// F6: `selectors` (a profile's declared selector names, any order) filtered
/// into [`FIXTURE_COLUMN_ORDER`] — the columns a fixture page's lane offers.
/// A profile that declares none (or a legacy lane) yields an empty list, the
/// single-value row the lane had before F6.
pub fn fixture_columns(selectors: &[String]) -> Vec<String> {
    FIXTURE_COLUMN_ORDER
        .iter()
        .filter(|c| selectors.iter().any(|s| s == *c))
        .map(|c| (*c).to_string())
        .collect()
}

/// F6: the row a fixture lane drives at `tick`, one float per column. Linear
/// per column through [`interpolate_row`], except that a `hue` column is
/// unwrapped first so interpolation takes the **short way** round the colour
/// wheel (`0.9` → `0.1` passes through `1.0`/`0`, never through `0.5`), then
/// wrapped back into `0..1`. An empty `columns` is the legacy single-value row
/// (`interpolate_row` unchanged).
pub fn fixture_row_at(
    keyframes: &BTreeMap<usize, Vec<f32>>,
    columns: &[String],
    tick: usize,
) -> Option<Vec<f32>> {
    let hues: Vec<usize> = columns.iter().enumerate().filter(|(_, c)| c.as_str() == "hue").map(|(i, _)| i).collect();
    if hues.is_empty() {
        return interpolate_row(keyframes, tick);
    }
    let mut unwrapped = keyframes.clone();
    for &c in &hues {
        let mut prev: Option<f32> = None;
        for row in unwrapped.values_mut() {
            if c >= row.len() {
                continue;
            }
            if let Some(p) = prev {
                let mut d = row[c] - p;
                if d > 0.5 {
                    d -= 1.0;
                } else if d < -0.5 {
                    d += 1.0;
                }
                row[c] = p + d;
            }
            prev = Some(row[c]);
        }
    }
    let mut out = interpolate_row(&unwrapped, tick)?;
    for &c in &hues {
        if c < out.len() {
            out[c] = out[c].rem_euclid(1.0);
        }
    }
    Some(out)
}

/// What one layer of a page carries beyond its clips — the compositing
/// state shared by every clip stacked on that layer (plan §1). Keyed in
/// [`IngestLane::layer_meta`] by `(device, mask, layer)`, so a clip beside
/// another on the same layer reads and writes the same blend.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerMeta {
    pub blend: String,
    pub opacity: f32,
    pub muted: bool,
    pub solo: bool,
}

impl Default for LayerMeta {
    fn default() -> Self {
        LayerMeta {
            blend: "normal".to_string(),
            opacity: 1.0,
            muted: false,
            solo: false,
        }
    }
}

impl LayerMeta {
    /// The parsed blend, defaulting to `normal` for an unknown name — the
    /// same fallback [`metal_perform::layers::Blend::parse`] gives a fresh
    /// layer.
    pub fn blend(&self) -> metal_perform::layers::Blend {
        metal_perform::layers::Blend::parse(&self.blend).unwrap_or_default()
    }

    /// Advances to the next blend in [`metal_perform::layers::Blend::all`], the
    /// chip's click.
    pub fn cycle_blend(&mut self) {
        let all = metal_perform::layers::Blend::all();
        let cur = self.blend();
        let i = all.iter().position(|b| *b == cur).unwrap_or(0);
        self.blend = all[(i + 1) % all.len()].as_str().to_string();
    }
}

/// G3: how a fixture/rig lane drives its value. `Keyframe` interpolates the
/// lane's authored keyframes at the playhead (`interpolate_row`, the same read
/// a model lane makes); `Constant` holds one value for the whole lane while
/// its grid button is on, the live-value path `Layer::base` takes. The default
/// is `Keyframe`, so a lane saved before G3 (and every model/video lane, which
/// never reads this) is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FixtureValue {
    #[default]
    Keyframe,
    Constant,
}

impl FixtureValue {
    /// The tag the project file carries.
    pub fn word(self) -> &'static str {
        match self {
            FixtureValue::Keyframe => "keyframe",
            FixtureValue::Constant => "constant",
        }
    }

    /// Parses the project file's tag; anything else is the default.
    pub fn parse(s: &str) -> FixtureValue {
        match s {
            "constant" => FixtureValue::Constant,
            _ => FixtureValue::Keyframe,
        }
    }
}

/// The map key for one layer: its page's device, mask, fixture and group,
/// and the layer index.
pub type LayerKey = (Option<String>, Option<String>, Option<String>, Option<String>, usize);

/// One clip of the sequence editor (2026-09-30, clips 2026-10-01): a model
/// authored over the song — or a video file — aimed at its own output
/// device and mask (its **page**), on a **layer** of that page, between its
/// mark-in and mark-out. The active clip lives in [`IngestLane`]'s own
/// fields (so the properties panel keeps editing `ingest.row`); the others
/// wait here.
#[derive(Debug, Clone, Default)]
pub struct Track {
    pub model: String,
    pub device: Option<String>,
    pub mask: Option<String>,
    /// G2: a fixture lane targets one venue fixture; a rig lane its
    /// `Fixture.group` — the page the lane lives on, exactly as
    /// `device`/`mask` name an output page. Only one family is ever set.
    pub fixture: Option<String>,
    pub group: Option<String>,
    /// G3: `Keyframe` (interpolate `keyframes` at the playhead) or `Constant`
    /// (hold `value`). Read only for a fixture/rig lane.
    pub value_mode: FixtureValue,
    /// G3: the constant a `Constant` fixture/rig lane holds, in `0..1`.
    pub value: f32,
    /// F6: the columns a fixture page's lane offers — its profile's declared
    /// selectors, in [`FIXTURE_COLUMN_ORDER`]. One float per column in
    /// `keyframes`; empty means the legacy single-value row (a lane saved
    /// before F6, or a model/video lane).
    pub columns: Vec<String>,
    pub keyframes: BTreeMap<usize, Vec<f32>>,
    /// Which layer of its page — higher plays over lower.
    pub layer: usize,
    /// Mark-in/mark-out, inclusive; `None` follows the keypoints' extent.
    pub bounds: Option<(usize, usize)>,
    /// A video clip's file — `None` is a model clip.
    pub video: Option<String>,
    /// An fx clip's effect kind (`fx.shake`…): the layer applies that effect
    /// over the layers under it. `None` for a model or video clip.
    pub effect: Option<String>,
    /// Expanded shows one row per float with its keypoints; collapsed only
    /// where the model starts and ends in the song.
    pub expanded: bool,
    row: Vec<f32>,
    loaded_song: Option<String>,
}

impl Track {
    /// The page this track lives on — its output (device/mask) or its G2
    /// fixture/rig.
    pub fn page(&self) -> Page {
        Page {
            device: self.device.clone(),
            mask: self.mask.clone(),
            fixture: self.fixture.clone(),
            group: self.group.clone(),
        }
    }
}

/// What the timeline paints for one track — the active one read off the
/// live fields, the rest off their stash.
#[derive(Debug, Clone, Copy)]
pub struct TrackView<'a> {
    pub index: usize,
    pub model: &'a str,
    pub device: Option<&'a str>,
    pub mask: Option<&'a str>,
    pub fixture: Option<&'a str>,
    pub group: Option<&'a str>,
    /// G3: the lane's value mode (see [`FixtureValue`]); a no-op outside the
    /// fixture/rig family.
    pub value_mode: FixtureValue,
    /// G3: the constant a `Constant` lane holds.
    pub value: f32,
    /// F6: the lane's columns (see [`Track::columns`]).
    pub columns: &'a [String],
    pub keyframes: &'a BTreeMap<usize, Vec<f32>>,
    pub expanded: bool,
    pub active: bool,
    pub layer: usize,
    /// Mark-in/mark-out as played — the explicit marks, else the extent.
    pub bounds: Option<(usize, usize)>,
    pub video: Option<&'a str>,
    /// The fx effect kind of an fx clip (see [`Track::effect`]).
    pub effect: Option<&'a str>,
}

impl TrackView<'_> {
    /// The page this clip belongs to.
    pub fn page(&self) -> Page {
        Page {
            device: self.device.map(str::to_string),
            mask: self.mask.map(str::to_string),
            fixture: self.fixture.map(str::to_string),
            group: self.group.map(str::to_string),
        }
    }

    /// What the clip is called on the timeline.
    pub fn title(&self) -> String {
        match self.video {
            Some(v) => format!(" {}", std::path::Path::new(v).file_name().and_then(|n| n.to_str()).unwrap_or(v)),
            None if self.model.is_empty() => "— no model —".to_string(),
            None => self.model.to_string(),
        }
    }
}

/// One page of the sequence: a final output — a whole device, or one mask
/// of it — with its own layers of clips. G2 adds two more page kinds beside
/// those: a **fixture** page (one venue fixture) and a **rig** page (every
/// fixture of a `Fixture.group`), each with its own lanes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    pub device: Option<String>,
    pub mask: Option<String>,
    /// A venue fixture's page — the lane targets one fixture (G2).
    pub fixture: Option<String>,
    /// A venue rig's page — the lane targets a whole `Fixture.group` (G2).
    pub group: Option<String>,
}

impl Page {
    pub fn label(&self) -> String {
        if let Some(f) = &self.fixture {
            return format!("◉ {f}");
        }
        if let Some(g) = &self.group {
            return format!("▣ {g}");
        }
        match (&self.device, &self.mask) {
            (Some(d), Some(m)) => format!("{d} / {m}"),
            (Some(d), None) => d.clone(),
            (None, Some(m)) => format!("— / {m}"),
            (None, None) => "— all".to_string(),
        }
    }
}

/// One undo step for the whole sequence panel: the active track's live
/// fields, every stacked [`Track`] with its clips/keyframes/bounds, and the
/// panel-wide state an edit can touch — the per-layer meta, the song's hits,
/// the marked range, and the playhead. The snapshot is the state to go back
/// *to*, taken before a mutation — the same shape [`MovementLane`]'s own
/// undo keeps, scaled up from one cell grid to the entire sequence, so a
/// restore cannot be mis-applied: every field is copied back verbatim.
#[derive(Debug, Clone)]
struct SeqSnapshot {
    model: String,
    device: Option<String>,
    mask: Option<String>,
    keyframes: BTreeMap<usize, Vec<f32>>,
    row: Vec<f32>,
    columns: Vec<String>,
    expanded: bool,
    layer: usize,
    bounds: Option<(usize, usize)>,
    video: Option<String>,
    effect: Option<String>,
    loaded_song: Option<String>,
    tracks: Vec<Track>,
    active: usize,
    layer_meta: BTreeMap<LayerKey, LayerMeta>,
    song_hits: std::collections::BTreeSet<usize>,
    mark: Option<(usize, usize)>,
    playhead: usize,
}

/// How many whole-sequence snapshots the undo stack keeps before the oldest
/// is dropped — a few hundred steps bounds the memory a long session hoards.
const HISTORY_DEPTH: usize = 256;

/// The first and last authored tick — where a model starts and ends.
pub fn track_extent(keyframes: &BTreeMap<usize, Vec<f32>>) -> Option<(usize, usize)> {
    Some((*keyframes.keys().next()?, *keyframes.keys().next_back()?))
}

/// A clip's played span: its marks when set, else its keypoints' extent.
pub fn clip_bounds(bounds: Option<(usize, usize)>, keyframes: &BTreeMap<usize, Vec<f32>>) -> Option<(usize, usize)> {
    bounds.or_else(|| track_extent(keyframes))
}

/// Ticks per second on the sequence editor's grid. The matrix it paints
/// (`GET /author/grade/matrix`), the ruler and every saved keyframe are on the
/// canonical grid since the 8 → 32 Hz move, so playback and the audio/video
/// seeks count in canonical ticks too — at the base 8 Hz rate a part opened
/// at 45 s played four times slow and sought the song to 3 min.
pub const EDITOR_RATE: f32 = metal_schema::TickRate::CANONICAL.hz() as f32;


/// A video clip's default length when its file's duration is unknown.
pub const VIDEO_DEFAULT_TICKS: usize = 80;

/// `S.AP` rebuilt: the Colourist take lane, `page/suite.js`'s `SUITE`
/// ported. A scrolling window of the song's digested ingest
/// (`the build reference`'s sixteen columns) with the authored keyframes overlaid
/// as tick marks, and one live float row — [`Self::row`] — a schema-driven
/// properties panel edits. The row and the keyframe are the same object,
/// exactly `suite.js`'s own doc comment: *"the sliders write it, the
/// picture is it, and `k` files it at a tick"* — here, a button does what
/// `k` did.
#[derive(Debug, Clone)]
pub struct IngestLane {
    /// Which model's schema `row` fits — `""` until a model is picked.
    pub model: String,
    pub song: String,
    /// The song [`Self::load`] last actually fetched for — `None` (or a
    /// name that no longer matches [`Self::song`]) is what
    /// [`Self::needs_load`] reads as "fetch again": opening a different
    /// song, or switching model (which clears this too, since keyframes
    /// are per `(model, song)`), makes the panel auto-load instead of
    /// waiting on a button.
    loaded_song: Option<String>,
    /// `R22.7` B3: a fetch landed but the song has no matrix yet
    /// (`ticks == 0`), so it must not count as loaded. The panel reads the
    /// song's status off `GET /author/songs` while this is set and only
    /// lets [`Self::needs_load`] fire again once the digest is done —
    /// otherwise every frame would re-ask for a matrix that is not there.
    pub waiting_digest: bool,
    /// The song's digest, fetched by "load" — `None` before a load, or
    /// when the song has no matrix yet (the route hands back `ticks: 0`
    /// rather than an error either way).
    pub matrix: Option<DigestMatrix>,
    /// The song's own beat hits — shared by every model, `GET`/`POST
    /// /models/song-hits`, the same reason `suite.js`'s `songTicks` is keyed
    /// by song alone (`laneS`'s doc comment).
    pub song_hits: std::collections::BTreeSet<usize>,
    /// The model's authored ticks for this song/device — sparse, `tick ->
    /// floats`, exactly `example::Example.ticks`' own shape.
    pub keyframes: BTreeMap<usize, Vec<f32>>,
    /// The live row the properties panel edits — what the picture shows,
    /// what a keyframe commit files. Its length is the current model's
    /// schema width; a model switch resets it to that model's defaults.
    pub row: Vec<f32>,
    pub playhead: usize,
    /// How many ticks the window shows at once — `suite.js`'s `SPAN`, made
    /// adjustable rather than the constant 160 it was, the same
    /// "longitud (ticks)" idiom `timeline_mode`'s own scratch already uses.
    pub span: usize,
    /// A loop's length in ticks — `Some(8 | 16 | 64)` for a sequence view
    /// made by `+ › add loop`. Such a view has no ingestion (no song, no
    /// digest rows) and every clip and the window stop at this many ticks.
    /// `None` is an ordinary sequence.
    pub loop_ticks: Option<usize>,
    pub note: Option<String>,
    /// `R22.8`: which mask of the chosen output cuts the preview — `None`
    /// is the `—` (no cut). It rides with a saved sequence (`Movement.mask`)
    /// and, while one runs, is what the real preview shows.
    pub mask: Option<String>,
    /// `R22.8`: the floating preview over the ingest canvas, and the row it
    /// last pushed (`POST /author/render/floats` fires only when this
    /// changes — never per frame).
    pub preview: crate::shell::preview::FloatingPreview,
    pub preview_posted: Option<(usize, Vec<f32>, Option<String>)>,
    /// The whole-sequence undo stack — one snapshot per user mutation.
    history: Vec<SeqSnapshot>,
    /// 2026-09-30, the sequence editor: the output device the active track
    /// is aimed at (was the panel's `take_device`).
    pub device: Option<String>,
    /// G2: the venue fixture / `Fixture.group` the active track's page is
    /// aimed at — the fixture-lane counterpart of `device`/`mask`.
    pub fixture: Option<String>,
    pub group: Option<String>,
    /// G3: the active fixture/rig lane's value mode and held constant (live
    /// fields, stashed into the track like the rest).
    pub value_mode: FixtureValue,
    pub value: f32,
    /// F6: the active fixture page's lane columns (see [`Track::columns`]).
    pub columns: Vec<String>,
    /// Whether the active track shows its float rows.
    pub expanded: bool,
    /// Every track, the active one's slot stale until [`Self::stash`].
    pub tracks: Vec<Track>,
    pub active: usize,
    /// The range dragged on the ruler, `[from, to]` inclusive.
    pub mark: Option<(usize, usize)>,
    /// Copied keyframes, as `(offset from the range start, floats)`.
    pub clip: Vec<(usize, Vec<f32>)>,
    /// The left edge of the view, in ticks (horizontal scroll).
    pub view_from: f32,
    /// The rows' vertical scroll, in points.
    pub scroll_y: f32,
    /// The rows' height scale, from ctrl+wheel (1.0 = the default heights).
    pub row_zoom: f32,
    /// The canvas rect, as of the last frame it painted — the kittest
    /// pointer-click tests need a point inside it, the same trade
    /// `MovementLane::canvas_rect` makes.
    pub canvas_rect: Option<egui::Rect>,
    /// 2026-10-01: playback — `(egui time, tick)` it started from; `None`
    /// is stopped. See [`Self::advance`].
    pub playing: Option<(f64, usize)>,
    /// The active clip's layer, marks and video (see [`Track`]).
    pub layer: usize,
    pub bounds: Option<(usize, usize)>,
    pub video: Option<String>,
    /// The active clip's fx effect kind (see [`Track::effect`]).
    pub effect: Option<String>,
    /// Per-layer compositing state, keyed by `(device, mask, layer)` so
    /// clips on the same layer share it (plan §1). Absent means the
    /// defaults ([`LayerMeta::default`]).
    pub layer_meta: BTreeMap<LayerKey, LayerMeta>,
    /// The song [`Self::matrix`] was fetched for — independent of any model,
    /// so the audio rows show as soon as a song is picked.
    matrix_for: Option<String>,
    /// Where the playhead and the view go once the song's matrix lands — a
    /// dataset part opened on a song not loaded yet ([`Self::adopt_part`]);
    /// [`Self::load_matrix`] would otherwise start the view over at 0.
    pending_view: Option<usize>,
}

/// `span`'s default — `page/suite.js`'s `SPAN` constant — everything else
/// starts empty/zero, which `#[derive(Default)]` would already give.
impl Default for IngestLane {
    fn default() -> Self {
        IngestLane {
            model: String::new(),
            song: String::new(),
            loaded_song: None,
            waiting_digest: false,
            matrix: None,
            song_hits: std::collections::BTreeSet::new(),
            keyframes: BTreeMap::new(),
            row: Vec::new(),
            playhead: 0,
            span: 160,
            loop_ticks: None,
            note: None,
            mask: None,
            preview: crate::shell::preview::FloatingPreview::default(),
            preview_posted: None,
            history: Vec::new(),
            canvas_rect: None,
            device: None,
            fixture: None,
            group: None,
            value_mode: FixtureValue::Keyframe,
            value: 0.0,
            columns: Vec::new(),
            expanded: true,
            tracks: Vec::new(),
            active: 0,
            mark: None,
            clip: Vec::new(),
            view_from: 0.0,
            scroll_y: 0.0,
            row_zoom: 1.0,
            playing: None,
            layer: 0,
            bounds: None,
            video: None,
            effect: None,
            layer_meta: BTreeMap::new(),
            matrix_for: None,
            pending_view: None,
        }
    }
}

impl IngestLane {
    /// Adopts a freshly loaded song/model: the matrix, the song's own beat
    /// hits, and the model's authored keyframes for this song (and device,
    /// if scoped) — `suite.js`'s `activate`/`loadExample`/`loadSongTicks`,
    /// all at once. Records [`Self::song`] as loaded so [`Self::needs_load`]
    /// goes quiet until the song (or the model) changes again — 22-sept-2026:
    /// the fetch itself now fires automatically off that flag once the
    /// panel is open with both a model and a song set, rather than behind
    /// an explicit "load" button. Clears undo — a freshly opened take is
    /// a new editing session, not a continuation — and returns the
    /// playhead to 0, interpolating the row from whatever was just loaded
    /// when there is something to interpolate from.
    ///
    /// `R22.7` B3: a reply with `ticks: 0` means the song is not digested
    /// yet, so it does **not** count as loaded ([`Self::waiting_digest`]
    /// carries that), and the panel polls until the matrix exists.
    pub fn load(
        &mut self,
        matrix: Option<DigestMatrix>,
        song_hits: std::collections::BTreeSet<usize>,
        keyframes: BTreeMap<usize, Vec<f32>>,
    ) {
        let ticks = matrix.as_ref().map(|m| m.ticks).unwrap_or(0);
        // Another track of the song already open keeps the playhead and the
        // view: only a new song starts over at 0.
        let same_song = ticks > 0
            && self.matrix.as_ref().is_some_and(|m| m.ticks == ticks && m.song == matrix.as_ref().map(|m| m.song.as_str()).unwrap_or_default());
        self.matrix = matrix;
        self.song_hits = song_hits;
        self.keyframes = keyframes;
        self.history.clear();
        self.loaded_song = if ticks == 0 {
            None
        } else {
            Some(self.song.trim().to_string())
        };
        self.waiting_digest = ticks == 0;
        if ticks > 0 {
            self.matrix_for = self.loaded_song.clone();
        }
        if same_song {
            self.goto(self.playhead);
        } else {
            self.view_from = 0.0;
            self.mark = None;
            self.goto(0);
        }
    }

    /// Whether the panel should fetch: a model and a song are both set, and
    /// either nothing has been loaded yet or it was loaded for a different
    /// song than the one now typed — including right after a model switch,
    /// which clears [`Self::loaded_song`] itself (keyframes are per
    /// `(model, song)`, so a new model means the old load no longer answers
    /// for what is on screen even if the song string is unchanged). A song
    /// still waiting on its digest ([`Self::waiting_digest`]) is not
    /// fetched again: the panel polls the digest status instead.
    pub fn needs_load(&self) -> bool {
        let song = self.song.trim();
        !self.waiting_digest
            && !self.model.is_empty()
            && !song.is_empty()
            && self.loaded_song.as_deref() != Some(song)
    }

    /// Whether the song's digest should be fetched — a song is set and the
    /// matrix on hand is not its own. No model needed: the audio rows are
    /// the song's.
    pub fn needs_matrix(&self) -> bool {
        let song = self.song.trim();
        !self.waiting_digest && !song.is_empty() && self.matrix_for.as_deref() != Some(song)
    }

    /// Adopts the song's digest and hits alone, leaving every clip as it is;
    /// a new song starts the view over at 0.
    pub fn load_matrix(&mut self, matrix: Option<DigestMatrix>, song_hits: std::collections::BTreeSet<usize>) {
        let ticks = matrix.as_ref().map(|m| m.ticks).unwrap_or(0);
        self.matrix = matrix;
        self.song_hits = song_hits;
        self.waiting_digest = ticks == 0;
        self.matrix_for = (ticks > 0).then(|| self.song.trim().to_string());
        self.view_from = 0.0;
        self.mark = None;
        self.goto(0);
        if ticks > 0 {
            if let Some(tick) = self.pending_view.take() {
                self.view_to(tick);
            }
        }
    }

    /// Puts the playhead on `tick` with a little lead-in on screen.
    fn view_to(&mut self, tick: usize) {
        self.view_from = tick.saturating_sub(self.span / 8) as f32;
        self.goto(tick);
    }

    /// A Colourist dataset part (`GET /author/dataset/example`,
    /// `author::dataset`) opened as a clip: its model and keypoints, its span
    /// as mark-in/out, the window zoomed to hold it and the playhead on its
    /// first tick. A part of another song starts the editor over on that song
    /// (one part, one clip); a part of the open song stacks as a new layer.
    /// Counted as loaded for the song, so the auto-load does not replace its
    /// keypoints with the model's whole-song corpus.
    pub fn adopt_part(&mut self, song: &str, model: &str, keyframes: BTreeMap<usize, Vec<f32>>, span: (usize, usize)) {
        if self.song.trim() != song {
            let canvas_rect = self.canvas_rect;
            *self = IngestLane { canvas_rect, ..IngestLane::default() };
            self.song = song.to_string();
        } else if !self.is_blank() {
            self.add_track();
        }
        self.model = model.to_string();
        self.row = keyframes.values().next().cloned().unwrap_or_default();
        self.keyframes = keyframes;
        self.bounds = Some(span);
        self.expanded = true;
        self.loaded_song = Some(song.to_string());
        let wanted = (span.1.saturating_sub(span.0) * 5 / 4).max(1);
        self.span = wanted.clamp(crate::shell::timeline::SPAN_MIN, crate::shell::timeline::SPAN_MAX);
        if self.matrix_for.as_deref() == Some(song) {
            self.view_to(span.0);
        } else {
            self.pending_view = Some(span.0);
            self.goto(span.0);
        }
    }

    /// The tick playback stops at: the song's length, else one past the last
    /// keyframe; `0` with neither — nothing to play.
    pub fn end_tick(&self) -> usize {
        match self.matrix.as_ref().map(|m| m.ticks).filter(|t| *t > 0) {
            Some(t) => t,
            None => self
                .track_views()
                .iter()
                .filter_map(|t| t.bounds.map(|(_, b)| b + 1))
                .max()
                .unwrap_or(0),
        }
    }

    /// Starts playback from the playhead (from 0 when it sits at the end), or
    /// stops it.
    pub fn toggle_play(&mut self, now: f64) {
        if self.playing.take().is_some() {
            return;
        }
        let end = self.end_tick();
        // An undigested song has no ticks yet, but its audio plays: the audio
        // clock is the length (`advance_to`). Nothing at all to play refuses.
        if end == 0 && self.song.trim().is_empty() {
            self.note = Some("nothing to play: load a song or pin keypoints".into());
            return;
        }
        let from = if self.playhead + 1 >= end { 0 } else { self.playhead };
        self.playing = Some((now, from));
    }

    #[cfg(test)]
    /// Moves the playhead to where playback is at `now`, at the digest's own
    /// rate, keeping it in view; stops at [`Self::end_tick`]. True while
    /// playing (the caller keeps repainting).
    pub fn advance(&mut self, now: f64) -> bool {
        self.advance_to(now, None)
    }

    /// As [`Self::advance`], but when the song is sounding on the device
    /// (`audible`: seconds into the song the speakers are at, from the audio
    /// clock) the playhead follows that instead of the wall clock. The wall
    /// clock is still what stops the playback when there is no audio clock.
    pub fn advance_to(&mut self, now: f64, audible: Option<f64>) -> bool {
        let Some((t0, from)) = self.playing else {
            return false;
        };
        let end = self.end_tick();
        if end == 0 {
            // No digest: the playhead is the audio clock while it sounds and
            // stays put otherwise; the panel stops the playback when the
            // audio ends.
            if let Some(secs) = audible {
                self.goto((secs.max(0.0) * f64::from(EDITOR_RATE)) as usize);
            }
            return true;
        }
        let tick = match audible {
            Some(secs) => (secs.max(0.0) * f64::from(EDITOR_RATE)) as usize,
            None => from + ((now - t0).max(0.0) * f64::from(EDITOR_RATE)) as usize,
        };
        if end == 0 || tick >= end {
            self.playing = None;
            self.goto(end.saturating_sub(1));
            return false;
        }
        if tick != self.playhead {
            self.goto(tick);
            let span = self.span.max(1) as f32;
            if (tick as f32) < self.view_from || tick as f32 >= self.view_from + span {
                self.view_from = tick as f32;
            }
        }
        true
    }

    /// Jumps the playhead to `tick` from a click: while playing, playback
    /// carries on from there at once (the audio follows, `sync_audio`).
    pub fn seek(&mut self, tick: usize, now: f64) {
        if self.playing.is_some() {
            self.playing = Some((now, tick));
        }
        self.goto(tick);
    }

    /// Moves the playhead and, if any keyframes are authored, re-interpolates
    /// [`Self::row`] — `suite.js`'s `goTick`. With nothing authored the row
    /// is left exactly as the operator dialled it.
    pub fn goto(&mut self, tick: usize) {
        self.playhead = tick;
        if let Some(row) = interpolate_row(&self.keyframes, tick) {
            self.row = row;
        }
    }

    /// Files [`Self::row`] as the keyframe at the current playhead —
    /// `suite.js`'s `k`.
    pub fn write_keyframe(&mut self) {
        self.history_push();
        self.keyframes.insert(self.playhead, self.row.clone());
        if let Some((a, b)) = self.bounds {
            self.bounds = Some((a.min(self.playhead), b.max(self.playhead)));
        }
    }

    /// Clears the keyframe at the current playhead, if there is one.
    pub fn clear_keyframe(&mut self) {
        if self.keyframes.contains_key(&self.playhead) {
            self.history_push();
            self.keyframes.remove(&self.playhead);
        }
    }

    /// Takes the state to go back *to* before a mutation and pushes it on the
    /// undo stack, dropping the oldest snapshot past [`HISTORY_DEPTH`] so a
    /// long session cannot hoard copies. Every mutating op calls this first;
    /// read-only moves (seek, play, selection) never do.
    fn history_push(&mut self) {
        self.history.push(self.capture());
        if self.history.len() > HISTORY_DEPTH {
            self.history.remove(0);
        }
    }

    /// The current state as a restorable snapshot, cloned verbatim.
    fn capture(&self) -> SeqSnapshot {
        SeqSnapshot {
            model: self.model.clone(),
            device: self.device.clone(),
            mask: self.mask.clone(),
            keyframes: self.keyframes.clone(),
            row: self.row.clone(),
            columns: self.columns.clone(),
            expanded: self.expanded,
            layer: self.layer,
            bounds: self.bounds,
            video: self.video.clone(),
            effect: self.effect.clone(),
            loaded_song: self.loaded_song.clone(),
            tracks: self.tracks.clone(),
            active: self.active,
            layer_meta: self.layer_meta.clone(),
            song_hits: self.song_hits.clone(),
            mark: self.mark,
            playhead: self.playhead,
        }
    }

    /// Copy `s`'s whole state back, re-interpolating the row from the
    /// restored keyframes at the restored playhead.
    fn restore(&mut self, s: SeqSnapshot) {
        self.model = s.model;
        self.device = s.device;
        self.mask = s.mask;
        self.keyframes = s.keyframes;
        self.row = s.row;
        self.columns = s.columns;
        self.expanded = s.expanded;
        self.layer = s.layer;
        self.bounds = s.bounds;
        self.video = s.video;
        self.effect = s.effect;
        self.loaded_song = s.loaded_song;
        self.tracks = s.tracks;
        self.active = s.active.min(self.tracks.len().saturating_sub(1));
        self.layer_meta = s.layer_meta;
        self.song_hits = s.song_hits;
        self.mark = s.mark;
        self.playhead = s.playhead;
        self.preview_posted = None;
        self.goto(self.playhead);
    }

    /// Restores the last whole-sequence snapshot — one gesture (a paste, a
    /// track add, the 🗑 delete) is one step — if there is one.
    pub fn undo(&mut self) {
        if let Some(s) = self.history.pop() {
            self.restore(s);
        }
    }

    /// Sets (`Some`) or clears (`None`) several ticks with no history of its
    /// own — the caller takes the one snapshot the whole batch undoes as.
    fn apply(&mut self, changes: Vec<(usize, Option<Vec<f32>>)>) {
        for (tick, next) in changes {
            match next {
                Some(f) => {
                    self.keyframes.insert(tick, f);
                }
                None => {
                    self.keyframes.remove(&tick);
                }
            }
        }
    }

    /// The marked range, else the playhead alone.
    pub fn range(&self) -> (usize, usize) {
        self.mark.unwrap_or((self.playhead, self.playhead))
    }

    /// Copies the keyframes in the range; returns how many.
    pub fn copy_range(&mut self) -> usize {
        let (a, b) = self.range();
        self.clip = self.keyframes.range(a..=b).map(|(t, f)| (t - a, f.clone())).collect();
        self.clip.len()
    }

    /// Clears the keyframes in the range, one undo step.
    pub fn clear_range(&mut self) -> usize {
        // The snapshot is the state to go back *to*, so it has to be taken
        // before the removal; clearing an absent range changes nothing and
        // pushes nothing.
        let (a, b) = self.range();
        if self.keyframes.range(a..=b).next().is_some() {
            self.history_push();
        }
        self.clear_range_quiet()
    }

    /// Clears the range with no snapshot of its own — the caller owns the one
    /// snapshot the whole gesture (a clear, or the 🗑 delete) undoes as.
    fn clear_range_quiet(&mut self) -> usize {
        let (a, b) = self.range();
        let ticks: Vec<usize> = self.keyframes.range(a..=b).map(|(t, _)| *t).collect();
        let n = ticks.len();
        self.apply(ticks.into_iter().map(|t| (t, None)).collect());
        self.goto(self.playhead);
        n
    }

    /// Pastes the clipboard with its start at the playhead, one undo step.
    pub fn paste(&mut self) -> usize {
        let at = self.playhead;
        let changes: Vec<_> = self.clip.iter().map(|(o, f)| (at + o, Some(f.clone()))).collect();
        let n = changes.len();
        if n > 0 {
            self.history_push();
            self.apply(changes);
        }
        self.goto(self.playhead);
        n
    }

    /// Moves one keyframe to another tick (a drag of its grip) — replacing
    /// whatever was there, one undo step. False when there is nothing to move.
    pub fn move_keyframe(&mut self, from: usize, to: usize) -> bool {
        if from == to {
            return false;
        }
        let Some(floats) = self.keyframes.get(&from).cloned() else {
            return false;
        };
        self.history_push();
        self.apply(vec![(from, None), (to, Some(floats))]);
        self.goto(to);
        true
    }

    /// The previous/next authored tick from the playhead, on this track.
    pub fn neighbour_keyframe(&self, forward: bool) -> Option<usize> {
        if forward {
            self.keyframes.range(self.playhead + 1..).next().map(|(t, _)| *t)
        } else {
            self.keyframes.range(..self.playhead).next_back().map(|(t, _)| *t)
        }
    }

    /// Writes the live fields into the active track's slot.
    fn stash(&mut self) {
        if self.tracks.is_empty() {
            self.tracks.push(Track::default());
            self.active = 0;
        }
        self.tracks[self.active] = Track {
            model: self.model.clone(),
            device: self.device.clone(),
            mask: self.mask.clone(),
            fixture: self.fixture.clone(),
            group: self.group.clone(),
            value_mode: self.value_mode,
            value: self.value,
            columns: self.columns.clone(),
            keyframes: self.keyframes.clone(),
            expanded: self.expanded,
            row: self.row.clone(),
            loaded_song: self.loaded_song.clone(),
            layer: self.layer,
            bounds: self.bounds,
            video: self.video.clone(),
            effect: self.effect.clone(),
        };
    }

    fn adopt(&mut self, i: usize) {
        let t = self.tracks[i].clone();
        self.model = t.model;
        self.device = t.device;
        self.mask = t.mask;
        self.fixture = t.fixture;
        self.group = t.group;
        self.value_mode = t.value_mode;
        self.value = t.value;
        self.columns = t.columns;
        self.keyframes = t.keyframes;
        self.expanded = t.expanded;
        self.row = t.row;
        self.loaded_song = t.loaded_song;
        self.layer = t.layer;
        self.bounds = t.bounds;
        self.video = t.video;
        self.effect = t.effect;
        self.active = i;
        self.preview_posted = None;
        self.goto(self.playhead);
    }

    /// The live track has no model and no keypoints — reused, not stacked on.
    pub fn is_blank(&self) -> bool {
        self.model.is_empty() && self.keyframes.is_empty() && self.video.is_none() && self.effect.is_none() && self.bounds.is_none()
    }

    /// How many tracks the editor stacks — at least the live one.
    pub fn track_count(&self) -> usize {
        self.tracks.len().max(1)
    }

    /// Makes track `i` the one the toolbars and the properties panel edit.
    pub fn select_track(&mut self, i: usize) {
        if i == self.active || i >= self.track_count() {
            return;
        }
        self.stash();
        self.adopt(i);
    }

    /// The loop lengths the overlays grid's `add loop` offers, in ticks.
    pub const LOOP_TICKS: [usize; 3] = [8, 16, 64];

    /// `+ › add loop`: replaces the view with an empty loop of `ticks` — no
    /// song, so no digest rows — whose window is `ticks` wide. The canvas
    /// keeps its rect so the scroll and zoom the panel already has survive.
    pub fn start_loop(&mut self, ticks: usize) {
        let canvas_rect = self.canvas_rect;
        *self = IngestLane { canvas_rect, loop_ticks: Some(ticks.max(1)), span: ticks.max(1), ..IngestLane::default() };
    }

    /// The window's and every clip's end, clamped to a loop's length.
    pub fn clamp_to_loop(&self, tick: usize) -> usize {
        match self.loop_ticks {
            Some(n) => tick.min(n.saturating_sub(1)),
            None => tick,
        }
    }

    /// Stacks a new, empty model clip on a new layer over the page's others
    /// (aimed at the same page) and selects it.
    pub fn add_track(&mut self) {
        let layer = self.top_layer().map_or(0, |l| l + 1);
        self.push_clip(Track { layer, expanded: true, ..Track::default() });
    }

    /// A new, empty model clip beside the active one — same layer, from the
    /// playhead — so one model stops where the next starts.
    pub fn add_clip(&mut self) {
        let span = self.span.max(1) / 4;
        let at = self.playhead;
        self.push_clip(Track { layer: self.layer, bounds: Some((at, at + span)), expanded: true, ..Track::default() });
    }

    /// An fx effect (`fx.<name>`) as a clip from the playhead for a quarter of
    /// the sequence, on a new top layer: the layer applies that effect over
    /// the layers under it. The gallery's `fx` layer card is the same thing
    /// without an effect yet ([`Self::add_track`]).
    pub fn add_effect_at_playhead(&mut self, effect: &str) {
        let len = (self.span.max(1) / 4).max(1);
        let at = self.playhead;
        let layer = self.top_layer().map_or(0, |l| l + 1);
        let end = at + len - 1;
        self.push_clip(Track { layer, bounds: Some((at, end)), effect: Some(effect.to_string()), expanded: true, ..Track::default() });
    }

    /// The sequence's song becomes `name` (a digested library song), from
    /// the playhead: the lanes that follow it read its digest.
    pub fn set_song_at_playhead(&mut self, name: &str) {
        self.song = name.to_string();
        self.loaded_song = None;
    }

    /// A video file as a clip on the active page, from the playhead for a
    /// quarter of the sequence (the length [`Self::add_clip`] gives a new clip).
    pub fn add_video_at_playhead(&mut self, path: &str) {
        let len = (self.span.max(1) / 4).max(1);
        self.add_video_clip(path, self.playhead, len, None);
    }

    /// A video file as a clip on the active page, from `at` for `len` ticks,
    /// on `layer` (a new top layer when `None`); it becomes the active clip.
    pub fn add_video_clip(&mut self, path: &str, at: usize, len: usize, layer: Option<usize>) {
        let layer = layer.unwrap_or_else(|| self.top_layer().map_or(0, |l| l + 1));
        let end = at + len.max(1) - 1;
        self.push_clip(Track { layer, bounds: Some((at, end)), video: Some(path.to_string()), ..Track::default() });
    }

    /// Appends `t` on the active page (a blank live clip is replaced) and
    /// selects it.
    fn push_clip(&mut self, mut t: Track) {
        self.history_push();
        if let Some((a, b)) = t.bounds {
            t.bounds = Some((self.clamp_to_loop(a), self.clamp_to_loop(b)));
        }
        t.device = self.device.clone();
        t.mask = self.mask.clone();
        t.fixture = self.fixture.clone();
        t.group = self.group.clone();
        t.value_mode = self.value_mode;
        t.value = self.value;
        t.columns = self.columns.clone();
        t.loaded_song = (!self.song.trim().is_empty()).then(|| self.song.trim().to_string());
        self.stash();
        if self.is_blank() && t.layer == self.layer + 1 && self.page_views().len() == 1 {
            t.layer = self.layer;
        }
        if self.is_blank() {
            self.tracks[self.active] = t;
            let i = self.active;
            self.adopt(i);
        } else {
            self.tracks.push(t);
            self.adopt(self.tracks.len() - 1);
        }
    }

    /// The highest layer on the active page.
    fn top_layer(&self) -> Option<usize> {
        self.page_views().iter().map(|t| t.layer).max()
    }

    /// The active clip's page.
    pub fn current_page(&self) -> Page {
        Page {
            device: self.device.clone(),
            mask: self.mask.clone(),
            fixture: self.fixture.clone(),
            group: self.group.clone(),
        }
    }

    /// The key one layer's meta lives under.
    fn layer_key(page: &Page, layer: usize) -> LayerKey {
        (page.device.clone(), page.mask.clone(), page.fixture.clone(), page.group.clone(), layer)
    }

    /// `page`'s `layer` meta, or the defaults when nothing was set yet — an
    /// old sequence with no blend loads as `normal / 1.0 / false / false`.
    pub fn layer_meta(&self, page: &Page, layer: usize) -> LayerMeta {
        self.layer_meta.get(&Self::layer_key(page, layer)).cloned().unwrap_or_default()
    }

    /// Stores `page`'s `layer` meta.
    pub fn set_layer_meta(&mut self, page: &Page, layer: usize, meta: LayerMeta) {
        self.history_push();
        self.layer_meta.insert(Self::layer_key(page, layer), meta);
    }

    /// Every page, in the order its first clip was stacked.
    pub fn pages(&self) -> Vec<Page> {
        let mut out: Vec<Page> = Vec::new();
        for t in self.track_views() {
            let p = t.page();
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    }

    /// The active page's clips.
    pub fn page_views(&self) -> Vec<TrackView<'_>> {
        let page = self.current_page();
        self.track_views().into_iter().filter(|t| t.page() == page).collect()
    }

    /// The active page's layers, top first.
    pub fn layers(&self) -> Vec<usize> {
        let mut out: Vec<usize> = self.page_views().iter().map(|t| t.layer).collect();
        out.sort_unstable_by(|a, b| b.cmp(a));
        out.dedup();
        out
    }

    /// Switches to `page`'s first clip — or opens the page with an empty clip.
    pub fn select_page(&mut self, page: &Page) {
        if let Some(i) = self.track_views().iter().find(|t| &t.page() == page).map(|t| t.index) {
            self.select_track(i);
            return;
        }
        self.stash();
        self.tracks.push(Track {
            device: page.device.clone(),
            mask: page.mask.clone(),
            fixture: page.fixture.clone(),
            group: page.group.clone(),
            expanded: true,
            loaded_song: (!self.song.trim().is_empty()).then(|| self.song.trim().to_string()),
            ..Track::default()
        });
        self.adopt(self.tracks.len() - 1);
    }

    /// G2: adds a lane aimed at a venue fixture (`fixture`) or a rig
    /// (`group`) — exactly one of the two is ever set. Switches to that
    /// fixture's/rig's page first (opening it when it has none), then stacks
    /// an empty lane on it, the same placement `+ layer` gives a model layer.
    pub fn add_fixture_lane(&mut self, fixture: Option<String>, group: Option<String>, columns: Vec<String>) {
        let page = Page { device: None, mask: None, fixture: fixture.clone(), group: group.clone() };
        if let Some(i) = self.track_views().iter().find(|t| t.page() == page).map(|t| t.index) {
            self.select_track(i);
        } else {
            self.stash();
            self.tracks.push(Track {
                fixture: fixture.clone(),
                group: group.clone(),
                columns,
                expanded: true,
                loaded_song: (!self.song.trim().is_empty()).then(|| self.song.trim().to_string()),
                ..Track::default()
            });
            self.adopt(self.tracks.len() - 1);
        }
        self.add_track();
    }

    /// Aims the whole active page at another output (device and mask). A
    /// fixture/rig page retargeted to an output drops its fixture/group, and
    /// vice versa, so the page stays one kind.
    pub fn retarget_page(&mut self, device: Option<String>, mask: Option<String>) {
        let old = self.current_page();
        self.stash();
        for t in &mut self.tracks {
            if t.device == old.device && t.mask == old.mask && t.fixture == old.fixture && t.group == old.group {
                t.device = device.clone();
                t.mask = mask.clone();
                t.fixture = None;
                t.group = None;
            }
        }
        let i = self.active;
        self.adopt(i);
    }

    /// Moves clip `i` so it starts at `to` (its keypoints with it) onto
    /// `layer`. Returns whether anything changed.
    pub fn move_clip(&mut self, i: usize, to: usize, layer: usize) -> bool {
        self.stash();
        let Some(t) = self.tracks.get_mut(i) else { return false };
        let Some((a, b)) = clip_bounds(t.bounds, &t.keyframes) else { return false };
        let low = t.keyframes.keys().next().copied().unwrap_or(a).min(a) as i64;
        let d = (to as i64 - a as i64).max(-low);
        if d == 0 && t.layer == layer {
            return false;
        }
        let shift = |x: usize| (x as i64 + d) as usize;
        t.keyframes = std::mem::take(&mut t.keyframes).into_iter().map(|(k, f)| (shift(k), f)).collect();
        if t.bounds.is_some() {
            t.bounds = Some((shift(a), shift(b)));
        }
        t.layer = layer;
        let active = self.active;
        self.adopt(active);
        true
    }

    /// Drags clip `i`'s mark-in (`start`) or mark-out to `tick`.
    pub fn trim_clip(&mut self, i: usize, start: bool, tick: usize) -> bool {
        self.stash();
        let Some(t) = self.tracks.get_mut(i) else { return false };
        let Some((a, b)) = clip_bounds(t.bounds, &t.keyframes) else { return false };
        let next = if start { (tick.min(b), b) } else { (a, tick.max(a)) };
        if next == (a, b) && t.bounds.is_some() {
            return false;
        }
        t.bounds = Some(next);
        let active = self.active;
        self.adopt(active);
        true
    }

    /// The clip that plays at `tick` on the active page: the highest layer
    /// whose marks hold it (the later-stacked one on a tie).
    pub fn top_at(&self, tick: usize) -> Option<usize> {
        self.page_views()
            .iter()
            .filter(|t| t.bounds.is_some_and(|(a, b)| a <= tick && tick <= b))
            .max_by_key(|t| (t.layer, t.index))
            .map(|t| t.index)
    }

    /// Clip `i`'s row at `tick` — the live row for the active clip.
    pub fn row_of(&self, i: usize, tick: usize) -> Option<Vec<f32>> {
        if i == self.active {
            return Some(self.row.clone());
        }
        interpolate_row(&self.tracks.get(i)?.keyframes, tick)
    }

    /// G3: sets the active fixture/rig lane's value mode. A no-op read for a
    /// model/video lane (its row comes from `keyframes`/`row` regardless).
    pub fn set_value_mode(&mut self, mode: FixtureValue) {
        self.value_mode = mode;
    }

    /// G3: sets the active fixture/rig lane's held constant (clamped `0..1`).
    pub fn set_fixture_value(&mut self, value: f32) {
        self.value = value.clamp(0.0, 1.0);
    }

    /// G3: the value fixture/rig lane `i` drives at `tick`. `Constant` holds
    /// the lane's own `value` — the live path `Layer::base` takes; `Keyframe`
    /// reads `Track.keyframes`/`interpolate_row`, the same interpolation a
    /// model lane makes. `None` for a lane that is not a fixture/rig, or a
    /// keyframe lane with nothing authored yet.
    pub fn fixture_value_at(&self, i: usize, tick: usize) -> Option<Vec<f32>> {
        let view = self.track_views().into_iter().find(|t| t.index == i)?;
        if view.fixture.is_none() && view.group.is_none() {
            return None;
        }
        let width = view.columns.len().max(1);
        match view.value_mode {
            FixtureValue::Constant => Some(vec![view.value; width]),
            FixtureValue::Keyframe => fixture_row_at(view.keyframes, view.columns, tick),
        }
    }

    /// The whole sequence as one file — every page's clips — the body `POST
    /// /author/project` saves. `None` without a song.
    pub fn project_body(&mut self) -> Option<serde_json::Value> {
        let song = self.song.trim().to_string();
        if song.is_empty() {
            return None;
        }
        self.stash();
        let clips: Vec<serde_json::Value> = self
            .tracks
            .iter()
            .filter(|t| !t.model.is_empty() || t.video.is_some() || t.effect.is_some() || t.fixture.is_some() || t.group.is_some())
            .map(|t| {
                let keys: Vec<serde_json::Value> =
                    t.keyframes.iter().map(|(k, f)| serde_json::json!({ "tick": k, "floats": f })).collect();
                let page = t.page();
                let meta = self.layer_meta.get(&Self::layer_key(&page, t.layer)).cloned().unwrap_or_default();
                let mut clip = serde_json::json!({
                    "model": t.model,
                    "device": t.device,
                    "mask": t.mask,
                    "fixture": t.fixture,
                    "group": t.group,
                    "layer": t.layer,
                    "bounds": t.bounds.map(|(a, b)| [a, b]),
                    "video": t.video,
                    "effect": t.effect,
                    "keyframes": keys,
                    "blend": meta.blend,
                    "opacity": meta.opacity,
                    "muted": meta.muted,
                    "solo": meta.solo,
                });
                // G3: only a fixture/rig lane carries a value mode, so an
                // output page's clips stay byte-identical to their pre-G3 file.
                if t.fixture.is_some() || t.group.is_some() {
                    clip["value_mode"] = serde_json::json!(t.value_mode.word());
                    clip["value"] = serde_json::json!(t.value);
                    // F6: only a fixture/rig lane carries columns, and only
                    // when its profile declared them, so a pre-F6 file (and
                    // every output page's clip) stays byte-identical.
                    if !t.columns.is_empty() {
                        clip["columns"] = serde_json::json!(t.columns);
                    }
                }
                clip
            })
            .collect();
        Some(serde_json::json!({ "song": song, "clips": clips }))
    }

    /// Replaces every clip with a saved project's; returns how many. An
    /// empty or foreign reply changes nothing.
    pub fn adopt_project(&mut self, project: &serde_json::Value) -> usize {
        let song = self.song.trim().to_string();
        let loaded: Vec<(Track, LayerKey, LayerMeta)> = project["clips"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| {
                let s = |k: &str| c[k].as_str().map(str::to_string);
                let keyframes = c["keyframes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|k| {
                        let floats = k["floats"].as_array()?.iter().filter_map(|v| v.as_f64()).map(|v| v as f32).collect();
                        Some((k["tick"].as_u64()? as usize, floats))
                    })
                    .collect();
                let bounds = c["bounds"]
                    .as_array()
                    .and_then(|b| Some((b.first()?.as_u64()? as usize, b.get(1)?.as_u64()? as usize)));
                let device = s("device");
                let mask = s("mask");
                let fixture = s("fixture");
                let group = s("group");
                let layer = c["layer"].as_u64().unwrap_or(0) as usize;
                // G3: an old clip, or an output page's clip, carries no value
                // mode — `keyframe`/`0.0` is what the fields default to.
                let value_mode = FixtureValue::parse(c["value_mode"].as_str().unwrap_or("keyframe"));
                let value = c["value"].as_f64().unwrap_or(0.0) as f32;
                // F6: a pre-F6 clip carries no columns — the legacy
                // single-value row.
                let columns: Vec<String> = c["columns"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect();
                // An old clip carries no blend fields: the defaults are what
                // a sequence saved before §1 loads as.
                let meta = LayerMeta {
                    blend: c["blend"].as_str().unwrap_or("normal").to_string(),
                    opacity: c["opacity"].as_f64().unwrap_or(1.0) as f32,
                    muted: c["muted"].as_bool().unwrap_or(false),
                    solo: c["solo"].as_bool().unwrap_or(false),
                };
                let track = Track {
                    model: s("model").unwrap_or_default(),
                    device,
                    mask,
                    fixture,
                    group,
                    value_mode,
                    value,
                    columns,
                    keyframes,
                    layer,
                    bounds,
                    video: s("video"),
                    effect: s("effect"),
                    expanded: false,
                    loaded_song: Some(song.clone()),
                    ..Track::default()
                };
                let key = Self::layer_key(&track.page(), layer);
                (track, key, meta)
            })
            .filter(|(t, _, _)| !t.model.is_empty() || t.video.is_some() || t.effect.is_some() || t.fixture.is_some() || t.group.is_some())
            .collect();
        let n = loaded.len();
        if n == 0 {
            return 0;
        }
        let mut metas = BTreeMap::new();
        self.tracks = loaded
            .into_iter()
            .map(|(t, key, meta)| {
                metas.insert(key, meta);
                t
            })
            .collect();
        self.layer_meta = metas;
        self.active = 0;
        self.adopt(0);
        n
    }

    /// Removes the active track; the last one is emptied instead. One undo
    /// step.
    pub fn remove_track(&mut self) {
        self.history_push();
        self.remove_track_quiet();
    }

    /// Removes the active track with no snapshot of its own — the caller owns
    /// the one snapshot the whole gesture undoes as.
    fn remove_track_quiet(&mut self) {
        self.stash();
        if self.tracks.len() <= 1 {
            self.tracks.clear();
            self.tracks.push(Track {
                device: self.device.clone(),
                mask: self.mask.clone(),
                fixture: self.fixture.clone(),
                group: self.group.clone(),
                expanded: true,
                ..Track::default()
            });
        } else {
            self.tracks.remove(self.active);
        }
        self.adopt(self.active.min(self.tracks.len() - 1));
    }

    /// The 🗑 delete: clears the marked range and removes the active track as
    /// **one** undo step (two ops the operator reads as one gesture), returns
    /// how many keypoints the clear took. The keyboard's Delete/Backspace
    /// call this too.
    pub fn delete_range_and_track(&mut self) -> usize {
        self.history_push();
        let n = self.clear_range_quiet();
        self.remove_track_quiet();
        n
    }

    /// A library sequence as a new track: its model, its rows as keypoints
    /// (row *i* at tick *i*), its mask. Counted as loaded for the open song,
    /// so the auto-load does not replace it with the model's dataset.
    pub fn adopt_sequence(&mut self, model: &str, keyframes: BTreeMap<usize, Vec<f32>>, mask: Option<String>) {
        if !self.is_blank() {
            self.add_track();
        }
        self.model = model.to_string();
        self.row = keyframes.values().next().cloned().unwrap_or_default();
        self.keyframes = keyframes;
        self.mask = mask;
        let song = self.song.trim();
        self.loaded_song = (!song.is_empty()).then(|| song.to_string());
        self.goto(self.playhead);
    }

    /// Expands or collapses track `i`'s float rows.
    pub fn set_expanded(&mut self, i: usize, expanded: bool) {
        if i == self.active {
            self.expanded = expanded;
        } else if let Some(t) = self.tracks.get_mut(i) {
            t.expanded = expanded;
        }
    }

    /// Every track, in stack order, the active one read live.
    pub fn track_views(&self) -> Vec<TrackView<'_>> {
        let live = TrackView {
            index: self.active,
            model: &self.model,
            device: self.device.as_deref(),
            mask: self.mask.as_deref(),
            fixture: self.fixture.as_deref(),
            group: self.group.as_deref(),
            value_mode: self.value_mode,
            value: self.value,
            columns: &self.columns,
            keyframes: &self.keyframes,
            expanded: self.expanded,
            active: true,
            layer: self.layer,
            bounds: clip_bounds(self.bounds, &self.keyframes),
            video: self.video.as_deref(),
            effect: self.effect.as_deref(),
        };
        if self.tracks.is_empty() {
            return vec![live];
        }
        self.tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if i == self.active {
                    live
                } else {
                    TrackView {
                        index: i,
                        model: &t.model,
                        device: t.device.as_deref(),
                        mask: t.mask.as_deref(),
                        fixture: t.fixture.as_deref(),
                        group: t.group.as_deref(),
                        value_mode: t.value_mode,
                        value: t.value,
                        columns: &t.columns,
                        keyframes: &t.keyframes,
                        expanded: t.expanded,
                        active: false,
                        layer: t.layer,
                        bounds: clip_bounds(t.bounds, &t.keyframes),
                        video: t.video.as_deref(),
                        effect: t.effect.as_deref(),
                    }
                }
            })
            .collect()
    }

    /// Every track's save body (`POST /author/models/example`), the active
    /// one included — one dataset per track, aimed at its own device.
    pub fn all_example_bodies(&mut self, rate: f32, layer: Option<&str>) -> Vec<serde_json::Value> {
        self.stash();
        let song = self.song.trim().to_string();
        self.tracks
            .iter()
            .filter(|t| !t.model.is_empty() && !t.keyframes.is_empty() && !song.is_empty())
            .map(|t| {
                let ticks: Vec<serde_json::Value> = t
                    .keyframes
                    .iter()
                    .map(|(k, f)| serde_json::json!({ "tick": k, "floats": f }))
                    .collect();
                let mut body = serde_json::json!({ "model": t.model, "song": song, "rate": rate, "ticks": ticks });
                if let (Some(d), Some(l)) = (&t.device, layer) {
                    body["device"] = serde_json::json!(d);
                    body["layer"] = serde_json::json!(l);
                }
                body
            })
            .collect()
    }

    /// Toggles the song's own beat hit at `tick` — `suite.js`'s
    /// `toggleSongHit`, minus the immediate save: the panel's "save
    /// hits" button posts the whole set, the same explicit
    /// load/save discipline the router pick lane already keeps.
    pub fn toggle_song_hit(&mut self, tick: usize) {
        self.history_push();
        if !self.song_hits.remove(&tick) {
            self.song_hits.insert(tick);
        }
    }

    /// Resets the live row and clears the authored keyframes for a model
    /// switch — the old model's keyframes fit a schema this row no longer
    /// has, so they are not carried over. Clears [`Self::loaded_song`] too,
    /// so [`Self::needs_load`] fires again and the new model's own ticks
    /// for the song already open load automatically.
    pub fn switch_model(&mut self, model: &str, defaults: Vec<f32>) {
        self.model = model.to_string();
        self.row = defaults;
        self.keyframes.clear();
        self.history.clear();
        self.playhead = 0;
        self.loaded_song = None;
    }

    #[cfg(test)]
    /// The body `POST /author/models/example` takes — `S.AP`'s dataset
    /// shape, sourced from [`Self::keyframes`] directly rather than a cell
    /// grid. `None` for the same reasons `MovementLane::example_ticks_json`
    /// refuses: nothing authored, no model, or no song.
    pub fn example_body(&self, rate: f32, target: Option<(&str, &str)>) -> Option<serde_json::Value> {
        let song = self.song.trim();
        if self.model.is_empty() || self.keyframes.is_empty() || song.is_empty() {
            return None;
        }
        let ticks: Vec<serde_json::Value> = self
            .keyframes
            .iter()
            .map(|(t, floats)| serde_json::json!({ "tick": t, "floats": floats }))
            .collect();
        let mut body = serde_json::json!({
            "model": self.model,
            "song": song,
            "rate": rate,
            "ticks": ticks,
        });
        if let Some((device, layer)) = target {
            body["device"] = serde_json::json!(device);
            body["layer"] = serde_json::json!(layer);
        }
        Some(body)
    }

    /// The body `POST /author/models/library` takes — saving the same
    /// keyframes as a `Movement`, `S.AP`'s "convert to an overlay button":
    /// one dense row per authored tick, in tick order, under `{name,
    /// model}` — `MovementLane::ticks_json`'s shape, sourced from
    /// [`Self::keyframes`]. `None` when there is nothing to save or no name
    /// was given.
    pub fn movement_body(&self, name: &str) -> Option<serde_json::Value> {
        let name = name.trim();
        if name.is_empty() || self.model.is_empty() || self.keyframes.is_empty() {
            return None;
        }
        let ticks: Vec<serde_json::Value> = self
            .keyframes
            .values()
            .map(|floats| serde_json::json!(floats))
            .collect();
        // `R22.8` point 4: the chosen mask rides with the sequence. Left out
        // when there is none, so an unmasked save is byte-identical to what
        // this route always wrote.
        let mut body = serde_json::json!({
            "name": name,
            "model": self.model,
            "ticks": ticks,
        });
        if let Some(mask) = &self.mask {
            body["mask"] = serde_json::json!(mask);
        }
        Some(body)
    }
}

// ---------------------------------------------------------------------------
// the router band — S.AP's "share the same visual context" as the ingest
// lane above it: one contiguous colour per model, "from this tick, this
// model" made visible instead of only listed.
// ---------------------------------------------------------------------------

/// A stable colour per model id — an FNV-ish hash into a hue, so the band
/// stays legible without a fixed model→colour table that would need
/// maintaining as models are added (`ModelSchema`'s own point: a model is
/// just an id in a manifest, not a hand-kept list). The hash is pure; only
/// [`Theme::hue`] (gate 2's `Hsva::` ban) turns it into a `Color32`.
pub(crate) fn model_color(theme: &Theme, id: &str) -> Color32 {
    let mut h: u32 = 2166136261;
    for b in id.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    let hue = (h % 360) as f32 / 360.0;
    theme.hue(hue)
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A dataset part opens as one clip of its song: span = marks, the
    /// playhead waits for the matrix, then lands on the part's first tick.
    #[test]
    fn a_dataset_part_opens_as_a_clip_on_its_span() {
        let mut lane = IngestLane { song: "old".into(), model: "aurora".into(), ..IngestLane::default() };
        lane.keyframes.insert(3, vec![0.0]);
        let keys: BTreeMap<usize, Vec<f32>> = [(865, vec![0.2]), (1454, vec![0.9])].into_iter().collect();
        lane.adopt_part("magalenha", "rings", keys, (865, 1911));
        assert_eq!(lane.song, "magalenha");
        assert_eq!(lane.model, "rings");
        assert_eq!(lane.bounds, Some((865, 1911)));
        assert_eq!(lane.track_count(), 1, "another song starts over: one part, one clip");
        assert!(!lane.needs_load(), "the corpus auto-load must not replace the part");
        assert!(lane.needs_matrix());
        assert!(lane.span >= 1911 - 865, "the window holds the whole part");

        let matrix = DigestMatrix { song: "magalenha".into(), ticks: 2000, ..DigestMatrix::default() };
        lane.load_matrix(Some(matrix), Default::default());
        assert_eq!(lane.playhead, 865, "the playhead lands on the part, not on 0");
        assert!(lane.view_from <= 865.0 && lane.view_from + lane.span as f32 > 1911.0);
        assert!((lane.row[0] - 0.2).abs() < 1e-6);

        // A second part of the same song stacks as a new layer.
        let keys: BTreeMap<usize, Vec<f32>> = [(3000, vec![0.5])].into_iter().collect();
        lane.adopt_part("magalenha", "aurora", keys, (3000, 3400));
        assert_eq!(lane.track_count(), 2);
        assert_eq!(lane.playhead, 3000, "same song: the view moves at once");
    }

    /// 2026-10-01: playback walks the playhead at the editor's (canonical)
    /// rate, stops at the last keyframe, and refuses with nothing to play.
    #[test]
    fn playback_walks_the_playhead_and_stops_at_the_end() {
        let mut lane = IngestLane::default();
        lane.toggle_play(0.0);
        assert!(lane.playing.is_none(), "nothing authored, nothing to play");

        lane.keyframes.insert(0, vec![0.0]);
        lane.keyframes.insert(64, vec![1.0]);
        lane.toggle_play(10.0);
        assert!(lane.advance(11.0));
        assert_eq!(lane.playhead, 32, "one second is 32 canonical ticks");
        assert!((lane.row[0] - 0.5).abs() < 1e-6, "the row interpolates as it plays");
        assert!(!lane.advance(13.0), "past the end it stops");
        assert_eq!(lane.playhead, 64);
        assert!(lane.playing.is_none());
        lane.toggle_play(20.0);
        assert_eq!(lane.playing, Some((20.0, 0)), "from the end it starts over");
    }

    /// Native audio: the playhead is the audio clock's position, not the wall
    /// clock's, and the wall clock does not move it while the audio is audible.
    #[test]
    fn the_playhead_follows_the_audio_clock_when_it_has_one() {
        let mut lane = IngestLane::default();
        lane.keyframes.insert(0, vec![0.0]);
        lane.keyframes.insert(640, vec![1.0]);
        lane.toggle_play(10.0);
        // Half a second of wall clock would be tick 16; the speakers are at 1 s.
        assert!(lane.advance_to(10.5, Some(1.0)));
        assert_eq!(lane.playhead, 32, "the audio clock wins");
        // Without an audio clock the wall clock drives it, as before.
        assert!(lane.advance_to(11.0, None));
        assert_eq!(lane.playhead, 32);
        // The audio clock past the end stops the playback at the last tick.
        assert!(!lane.advance_to(11.0, Some(30.0)));
        assert!(lane.playing.is_none());
        assert_eq!(lane.playhead, 640);
    }

    /// No digest yet: a song's audio is the length. The playhead is the audio
    /// clock's while it sounds and stays put without one; nothing to play and
    /// no song still refuses.
    #[test]
    fn an_undigested_song_plays_on_its_audio_clock() {
        let mut lane = IngestLane::default();
        lane.toggle_play(0.0);
        assert!(lane.playing.is_none(), "no song, no keypoints: refused");
        lane.song = "tone".into();
        lane.toggle_play(0.0);
        assert_eq!(lane.playing, Some((0.0, 0)));
        assert!(lane.advance_to(5.0, None), "no clock yet: keep playing in place");
        assert_eq!(lane.playhead, 0, "the wall clock does not move it");
        assert!(lane.advance_to(5.5, Some(3.0)));
        assert_eq!(lane.playhead, 96, "three seconds of song");
        assert!(lane.advance_to(6.0, Some(3.5)), "no end to stop at");
        assert_eq!(lane.playhead, 112);
    }

    /// 2026-10-06: a click while playing jumps there and keeps playing from
    /// it; a click while stopped only moves the playhead.
    #[test]
    fn a_seek_while_playing_plays_on_from_the_click() {
        let mut lane = IngestLane::default();
        lane.keyframes.insert(0, vec![0.0]);
        lane.keyframes.insert(640, vec![1.0]);
        lane.seek(100, 0.0);
        assert!(lane.playing.is_none());
        assert_eq!(lane.playhead, 100);
        lane.toggle_play(1.0);
        assert!(lane.advance(2.0));
        lane.seek(400, 2.5);
        assert_eq!(lane.playing, Some((2.5, 400)));
        assert!(lane.advance(3.5));
        assert_eq!(lane.playhead, 432, "playing on from the click");
    }
    use serde_json::json;

    /// 2026-10-01: clips. A clip beside another shares its layer; a new
    /// layer stacks over; the higher layer is what plays where both hold.
    #[test]
    fn the_top_layer_plays_and_a_clip_beside_shares_its_layer() {
        let mut lane = IngestLane { model: "a".into(), ..IngestLane::default() };
        lane.keyframes.insert(0, vec![0.0]);
        lane.keyframes.insert(20, vec![1.0]);
        assert_eq!(lane.top_at(10), Some(0));

        lane.goto(30);
        lane.add_clip();
        assert_eq!(lane.layer, 0, "beside, on the same layer");
        assert_eq!(lane.bounds, Some((30, 30 + lane.span / 4)));
        assert_eq!(lane.top_at(31), Some(1));

        lane.add_video_clip("/v.mp4", 5, 10, None);
        assert_eq!(lane.layer, 1, "a new layer over the others");
        assert_eq!(lane.top_at(8), Some(2), "the higher layer wins");
        assert_eq!(lane.top_at(16), Some(0), "past the video's mark-out, the model under it");
        assert_eq!(lane.layers(), vec![1, 0]);
        assert_eq!(lane.top_at(100), None);
    }

    /// Moving a clip carries its keypoints and can change its layer;
    /// dragging a mark only moves that edge.
    #[test]
    fn moving_and_trimming_a_clip() {
        let mut lane = IngestLane { model: "a".into(), ..IngestLane::default() };
        lane.keyframes.insert(2, vec![0.0]);
        lane.keyframes.insert(6, vec![1.0]);
        lane.add_video_clip("/v.mp4", 0, 8, None);
        assert!(lane.move_clip(0, 10, 3));
        lane.select_track(0);
        assert_eq!(lane.keyframes.keys().copied().collect::<Vec<_>>(), vec![10, 14]);
        assert_eq!(lane.layer, 3);
        assert!(lane.trim_clip(0, false, 12));
        assert_eq!(lane.bounds, Some((10, 12)));
        assert!(lane.trim_clip(0, true, 99), "a mark-in never passes the mark-out");
        assert_eq!(lane.bounds, Some((12, 12)));
        assert!(lane.move_clip(1, 0, 1) || lane.video.is_none());
        assert!(!lane.move_clip(0, 12, 3), "nowhere to go");
    }

    /// Pages: one per output; switching opens the page's own clips, and a
    /// retarget moves the whole page.
    #[test]
    fn pages_keep_their_own_layers() {
        let mut lane = IngestLane { model: "a".into(), device: Some("tv".into()), ..IngestLane::default() };
        lane.keyframes.insert(0, vec![0.0]);
        let wall = Page { device: Some("wall".into()), mask: Some("m1".into()), ..Page::default() };
        lane.select_page(&wall);
        assert_eq!(lane.current_page(), wall);
        assert_eq!(lane.page_views().len(), 1);
        lane.model = "b".into();
        lane.keyframes.insert(3, vec![1.0]);
        assert_eq!(lane.pages().len(), 2);
        let tv = Page { device: Some("tv".into()), mask: None, ..Page::default() };
        lane.select_page(&tv);
        assert_eq!(lane.model, "a");
        lane.retarget_page(Some("tv".into()), Some("left".into()));
        assert_eq!(lane.current_page().mask.as_deref(), Some("left"));
        assert_eq!(lane.pages().len(), 2);
    }

    /// G2: a lane aimed at a venue fixture opens its own page and stacks on
    /// it; a rig lane does the same for a `Fixture.group`; both round-trip
    /// through the project file beside the output pages. `retarget_page`
    /// moves a fixture page back onto an output, dropping the fixture.
    #[test]
    fn a_fixture_lane_opens_its_page_and_round_trips() {
        let mut lane = IngestLane { model: "a".into(), song: "s".into(), ..IngestLane::default() };
        lane.keyframes.insert(0, vec![0.0]);
        lane.add_fixture_lane(Some("wash-1".into()), None, Vec::new());
        assert_eq!(lane.current_page().fixture.as_deref(), Some("wash-1"));
        assert_eq!(lane.device, None, "a fixture page is not an output page");
        assert_eq!(lane.page_views().len(), 1);
        assert_eq!(lane.pages().len(), 2, "the output page and the fixture's");

        // Authoring the lane lets a second one stack beside it, and the page
        // is reused rather than duplicated.
        lane.model = "b".into();
        lane.keyframes.insert(2, vec![1.0]);
        lane.add_fixture_lane(Some("wash-1".into()), None, Vec::new());
        assert_eq!(lane.pages().len(), 2, "the same fixture keeps one page");
        assert_eq!(lane.page_views().len(), 2, "a second lane stacks on it");

        // A rig opens a page of its own.
        lane.add_fixture_lane(None, Some("front-wash".into()), Vec::new());
        let rig = Page { group: Some("front-wash".into()), ..Page::default() };
        assert_eq!(lane.current_page(), rig);
        assert_eq!(lane.pages().len(), 3);

        let body = lane.project_body().expect("a song is set");
        let clips = body["clips"].as_array().unwrap().clone();
        assert!(clips.iter().any(|c| c["fixture"] == "wash-1"));
        assert!(clips.iter().any(|c| c["group"] == "front-wash"));

        let mut back = IngestLane { song: "s".into(), ..IngestLane::default() };
        assert_eq!(back.adopt_project(&body), clips.len());
        assert!(back.pages().iter().any(|p| p.fixture.as_deref() == Some("wash-1")));
        assert!(back.pages().iter().any(|p| p.group.as_deref() == Some("front-wash")));

        // Retargeting the rig page onto an output drops the group, moving
        // its lanes to the output page.
        let rig_page = Page { group: Some("front-wash".into()), ..Page::default() };
        back.select_page(&rig_page);
        assert_eq!(back.current_page().group.as_deref(), Some("front-wash"));
        back.retarget_page(Some("tv".into()), None);
        assert_eq!(back.current_page().group, None);
        assert_eq!(back.current_page().device.as_deref(), Some("tv"));
    }

    /// G3: a `constant` fixture lane holds its one value; a `keyframe` fixture
    /// lane interpolates its authored ticks. `fixture_value_at` is the single
    /// read both take, and `None` outside the fixture/rig family.
    #[test]
    fn a_fixture_lane_holds_a_constant_or_interpolates_its_keyframes() {
        let mut lane = IngestLane { model: "a".into(), song: "s".into(), ..IngestLane::default() };
        lane.add_fixture_lane(Some("wash-1".into()), None, Vec::new());
        let i = lane.active;
        // Author two keyframes on the fixture lane itself.
        lane.keyframes.insert(0, vec![0.0]);
        lane.keyframes.insert(10, vec![1.0]);
        // Default is `Keyframe`: the row at a tick is the interpolation.
        assert_eq!(lane.fixture_value_at(i, 5), Some(vec![0.5]));
        lane.set_value_mode(FixtureValue::Constant);
        lane.set_fixture_value(0.75);
        assert_eq!(lane.fixture_value_at(i, 0), Some(vec![0.75]));
        assert_eq!(lane.fixture_value_at(i, 5), Some(vec![0.75]), "held, no interpolation");
        assert_eq!(lane.fixture_value_at(i, 99), Some(vec![0.75]), "held past the end too");
        // A model lane is not a fixture/rig: the read declines.
        let mut model = IngestLane { model: "a".into(), ..IngestLane::default() };
        model.keyframes.insert(0, vec![0.2]);
        assert_eq!(model.fixture_value_at(0, 0), None);
    }

    /// F6: `fixture_columns` keeps the profile's declared selectors in the
    /// canonical order and drops everything else — an unknown name, a
    /// selector the profile does not have. A pan/tilt head offers pan and
    /// tilt; a wash, which declares neither, does not.
    #[test]
    fn fixture_columns_are_the_profiles_selectors_in_canonical_order() {
        let head = fixture_columns(&["tilt".into(), "level".into(), "pan".into()]);
        assert_eq!(head, vec!["level", "pan", "tilt"]);
        let wash = fixture_columns(&["sat".into(), "level".into(), "hue".into()]);
        assert_eq!(wash, vec!["level", "hue", "sat"]);
        assert!(!wash.iter().any(|c| c == "pan" || c == "tilt"), "a wash has no pan/tilt: {wash:?}");
        assert!(fixture_columns(&["nope".into()]).is_empty());
    }

    /// F6: a fixture lane drives a row of N floats, one per profile column,
    /// and the columns survive a project round-trip. An output lane carries
    /// no columns key.
    #[test]
    fn a_fixture_lane_drives_a_column_row_and_round_trips_the_columns() {
        let mut lane = IngestLane { model: "a".into(), song: "s".into(), device: Some("tv".into()), ..IngestLane::default() };
        lane.add_fixture_lane(Some("head-1".into()), None, fixture_columns(&["level".into(), "pan".into(), "tilt".into()]));
        let i = lane.active;
        lane.keyframes.insert(0, vec![0.0, -1.0, 0.5]);
        lane.keyframes.insert(10, vec![1.0, 1.0, -0.5]);
        let row = lane.fixture_value_at(i, 5).expect("a row");
        assert_eq!(row.len(), 3, "one float per column");
        assert!((row[0] - 0.5).abs() < 1e-6);

        let body = lane.project_body().expect("a song is set");
        let clip = body["clips"].as_array().unwrap().iter().find(|c| c["fixture"] == "head-1").unwrap().clone();
        assert_eq!(clip["columns"], serde_json::json!(["level", "pan", "tilt"]));
        let out_clip = body["clips"].as_array().unwrap().iter().find(|c| c["device"] == "tv");
        if let Some(c) = out_clip {
            assert!(c.get("columns").is_none(), "an output lane carries no columns");
        }

        let mut back = IngestLane { song: "s".into(), ..IngestLane::default() };
        assert!(back.adopt_project(&body) > 0);
        let view = back.track_views().into_iter().find(|t| t.fixture == Some("head-1")).unwrap();
        assert_eq!(view.columns, &["level".to_string(), "pan".to_string(), "tilt".to_string()]);
    }

    /// F6: a `keyframe` interpolation across a `hue` column takes the short
    /// way round the wheel (`0.9` → `0.1` passes through `0`, not `0.5`); a
    /// linear column is untouched.
    #[test]
    fn a_hue_column_interpolates_the_short_way() {
        let columns = fixture_columns(&["hue".into(), "level".into()]);
        assert_eq!(columns, vec!["level", "hue"], "canonical order puts level first");
        let mut kf = BTreeMap::new();
        kf.insert(0, vec![0.0, 0.9]);
        kf.insert(10, vec![1.0, 0.1]);
        let mid = fixture_row_at(&kf, &columns, 5).expect("a row");
        // The short way from 0.9 to 0.1 wraps through 1.0/0.0: at the
        // midpoint hue is 0.0, never the long way's 0.5.
        assert!(mid[1] < 0.5, "hue took the short way, got {}", mid[1]);
        assert!((mid[1]).abs() < 1e-6 || (mid[1] - 1.0).abs() < 1e-6, "midpoint hue {} ~ 0", mid[1]);
        assert!((mid[0] - 0.5).abs() < 1e-6, "level is linear");
    }

    /// G3: both the value mode and the held constant survive a project
    /// round-trip; an output page's clip carries neither key, so its file is
    /// byte-identical to a pre-G3 save.
    #[test]
    fn a_fixture_lanes_value_mode_round_trips_and_output_lanes_stay_unchanged() {
        let mut lane = IngestLane { model: "a".into(), song: "s".into(), device: Some("tv".into()), ..IngestLane::default() };
        lane.keyframes.insert(0, vec![0.0]);
        lane.add_fixture_lane(None, Some("front-wash".into()), Vec::new());
        lane.set_value_mode(FixtureValue::Constant);
        lane.set_fixture_value(0.4);
        let body = lane.project_body().expect("a song is set");
        let fixture_clip = body["clips"].as_array().unwrap().iter().find(|c| c["group"] == "front-wash").unwrap().clone();
        assert_eq!(fixture_clip["value_mode"], "constant");
        assert!((fixture_clip["value"].as_f64().unwrap() - 0.4).abs() < 1e-6);
        let output_clip = body["clips"].as_array().unwrap().iter().find(|c| c["device"] == "tv").unwrap().clone();
        assert!(output_clip.get("value_mode").is_none(), "an output lane carries no value mode");
        assert!(output_clip.get("value").is_none(), "nor a value");

        let mut back = IngestLane { song: "s".into(), ..IngestLane::default() };
        assert!(back.adopt_project(&body) > 0);
        let rig = back.track_views().into_iter().find(|t| t.group == Some("front-wash")).unwrap();
        assert_eq!(rig.value_mode, FixtureValue::Constant);
        assert!((rig.value - 0.4).abs() < 1e-6);
        // A non-fixture lane loads as the default `Keyframe`, unchanged.
        let tv = back.track_views().into_iter().find(|t| t.device == Some("tv")).unwrap();
        assert_eq!(tv.value_mode, FixtureValue::Keyframe);
    }

    /// The project file round-trips every clip.
    #[test]
    fn a_project_round_trips_its_clips() {
        let mut lane = IngestLane { model: "a".into(), song: "s".into(), device: Some("tv".into()), ..IngestLane::default() };
        lane.keyframes.insert(4, vec![0.25, 0.5]);
        lane.add_video_clip("/v.mp4", 8, 4, Some(2));
        let body = lane.project_body().unwrap();
        let mut back = IngestLane { song: "s".into(), ..IngestLane::default() };
        assert_eq!(back.adopt_project(&json!({ "clips": [] })), 0);
        assert_eq!(back.adopt_project(&body), 2);
        let views = back.track_views();
        assert_eq!(views[0].model, "a");
        assert_eq!(views[0].keyframes.get(&4), Some(&vec![0.25, 0.5]));
        assert_eq!(views[1].video, Some("/v.mp4"));
        assert_eq!(views[1].bounds, Some((8, 11)));
        assert_eq!(views[1].layer, 2);
        assert_eq!(views[1].device, Some("tv"));
        assert!(!back.needs_load(), "an adopted project is not replaced by the auto-load");
    }

    /// Write / clear / undo: a snapshot restores the grid verbatim, and
    /// clearing an absent cell pushes nothing.
    #[test]
    fn write_then_undo_restores_the_grid_exactly() {
        let mut mv = MovementLane {
            name: "x".into(),
            ..Default::default()
        };
        mv.write(2, 0, 0.5);
        mv.write(4, 1, 0.7);
        assert_eq!(mv.cells.get(&(4, 1)), Some(&0.7));
        assert_eq!(mv.len, 5, "a write past tick 3 grows the length");
        mv.clear(5, 0);
        assert_eq!(
            mv.undo.len(),
            2,
            "clearing an absent cell pushes no snapshot"
        );
        mv.clear(2, 0);
        assert_eq!(mv.cells.get(&(2, 0)), None);
        mv.undo();
        assert_eq!(
            mv.cells.get(&(2, 0)),
            Some(&0.5),
            "undo restored the clear's before, which still had the write"
        );
        mv.undo();
        assert_eq!(
            mv.cells.get(&(4, 1)),
            None,
            "undo restored the state before the second write"
        );
        mv.undo();
        assert!(mv.cells.is_empty(), "undo to the very start");
    }

    /// Copy is re-based to tick 0 and paste shifts by the target tick —
    /// a movement is a shape, not a position.
    #[test]
    fn copy_then_paste_rebases_and_shifts() {
        let mut mv = MovementLane {
            name: "x".into(),
            ..Default::default()
        };
        mv.write(5, 0, 0.3);
        mv.write(6, 2, 0.8);
        mv.copy(5, 7);
        let clip = mv.clipboard.clone().expect("copied something");
        assert_eq!(clip.span, 2);
        assert!(clip.cells.contains_key(&(0, 0)), "re-based to tick 0");
        assert!(clip.cells.contains_key(&(1, 2)));
        mv.paste(10);
        assert_eq!(mv.cells.get(&(10, 0)), Some(&0.3));
        assert_eq!(mv.cells.get(&(11, 2)), Some(&0.8));
        assert_eq!(mv.len, 12);
        mv.undo();
        assert!(mv.cells.get(&(10, 0)).is_none(), "one paste, one undo");
    }

    /// Clear range is one op, one undo, and only for the lane asked.
    #[test]
    fn clear_range_takes_one_undo_and_only_one_lane() {
        let mut mv = MovementLane {
            name: "x".into(),
            ..Default::default()
        };
        mv.write(2, 0, 0.1);
        mv.write(3, 0, 0.2);
        mv.write(3, 1, 0.9);
        mv.clear_range(0, 2, 4);
        assert!(mv.cells.get(&(2, 0)).is_none() && mv.cells.get(&(3, 0)).is_none());
        assert_eq!(
            mv.cells.get(&(3, 1)),
            Some(&0.9),
            "the other lane is untouched"
        );
        assert_eq!(
            mv.undo.len(),
            4,
            "one snapshot for the whole clear (3 writes + the clear)"
        );
        mv.undo();
        assert_eq!(
            mv.cells.get(&(2, 0)),
            Some(&0.1),
            "the whole clear undoes at once"
        );
    }

    #[test]
    fn clear_span_takes_every_lanes_passage_in_one_undo() {
        let mut mv = MovementLane {
            name: "x".into(),
            ..Default::default()
        };
        mv.write(2, 0, 0.1);
        mv.write(2, 1, 0.9);
        mv.write(5, 0, 0.5);
        mv.clear_span(2, 4);
        assert_eq!(mv.cells.get(&(5, 0)), Some(&0.5), "outside the span stays");
        assert!(
            mv.cells.keys().all(|(t, _)| *t != 2),
            "both lanes' span is gone"
        );
        assert_eq!(mv.undo.len(), 4, "one snapshot for the whole span clear");
        mv.undo();
        assert_eq!(mv.cells.get(&(2, 1)), Some(&0.9));
    }

    #[test]
    fn move_span_rebases_the_passage_and_refuses_overlaps() {
        let mut mv = MovementLane {
            name: "x".into(),
            ..Default::default()
        };
        mv.write(2, 0, 0.1);
        mv.write(3, 1, 0.9);
        mv.move_span(2, 4, 6);
        assert_eq!(mv.cells.get(&(6, 0)), Some(&0.1), "re-based to dest");
        assert_eq!(mv.cells.get(&(7, 1)), Some(&0.9));
        assert!(
            mv.cells.keys().all(|(t, _)| *t != 2 && *t != 3),
            "the origin is vacated"
        );
        assert_eq!(mv.len, 8, "the length grew to hold the moved passage");
        assert_eq!(mv.undo.len(), 3, "one snapshot for the whole move");
        mv.undo();
        assert_eq!(mv.cells.get(&(2, 0)), Some(&0.1));
        // A destination inside the span itself is refused outright.
        let before = mv.cells.clone();
        mv.move_span(2, 4, 3);
        assert_eq!(mv.cells, before, "an overlapping move changes nothing");
    }

    /// `ticks_json`: one row of `width` floats per written tick, missing
    /// lanes at 0.0, nothing for empty/nameless scratches.
    #[test]
    fn ticks_json_is_one_width_row_per_written_tick() {
        let mut mv = MovementLane {
            name: "drop".into(),
            model: "geoterrain".into(),
            ..Default::default()
        };
        assert!(mv.ticks_json(2).is_none(), "an empty scratch saves nothing");
        mv.write(0, 0, 0.4);
        mv.write(2, 1, 0.6);
        let body = mv.ticks_json(2).expect("saved");
        assert_eq!(body["name"], "drop");
        assert_eq!(body["model"], "geoterrain");
        let ticks = body["ticks"].as_array().expect("rows");
        assert_eq!(ticks.len(), 2, "tick 1 has no cells, so no row");
        assert_eq!(ticks[0], serde_json::json!([0.4, 0.0]));
        assert_eq!(ticks[1], serde_json::json!([0.0, 0.6]));
        mv.name = "  ".into();
        assert!(
            mv.ticks_json(2).is_none(),
            "a nameless movement is refused, not posted"
        );
    }

    /// `example_ticks_json`: sparse `{tick, floats}` objects under
    /// `{model, song, rate}` — `S.AP`'s dataset shape, distinct from
    /// `ticks_json`'s dense array under `{name, model}` — and `device`/
    /// `layer` present only when a target was given.
    #[test]
    fn example_ticks_json_is_sparse_ticks_keyed_by_song_not_name() {
        let mut take = MovementLane {
            model: "wake".into(),
            ..Default::default()
        };
        assert!(
            take.example_ticks_json(2, "click-100", 8.0, None).is_none(),
            "an empty scratch saves nothing"
        );
        take.write(0, 0, 0.2);
        take.write(3, 1, 0.9);
        assert!(
            take.example_ticks_json(2, "  ", 8.0, None).is_none(),
            "no song, no dataset"
        );
        let body = take
            .example_ticks_json(2, "click-100", 8.0, None)
            .expect("saved");
        assert_eq!(body["model"], "wake");
        assert_eq!(body["song"], "click-100");
        assert_eq!(body["rate"], 8.0);
        assert!(body.get("device").is_none(), "no target, no device key");
        let ticks = body["ticks"].as_array().expect("ticks");
        assert_eq!(ticks.len(), 2, "tick 1 and 2 have no cells");
        assert_eq!(ticks[0], serde_json::json!({ "tick": 0, "floats": [0.2, 0.0] }));
        assert_eq!(ticks[1], serde_json::json!({ "tick": 3, "floats": [0.0, 0.9] }));

        let scoped = take
            .example_ticks_json(2, "click-100", 8.0, Some(("wall", "gen-0")))
            .expect("saved");
        assert_eq!(scoped["device"], "wall");
        assert_eq!(scoped["layer"], "gen-0");
    }

    /// `load` opens a model movement into the scratch and clears undo; a
    /// rig movement (no `model` key) or an empty one is refused.
    #[test]
    fn load_opens_a_model_movement_and_refuses_the_rest() {
        let mut mv = MovementLane::default();
        mv.write(9, 9, 1.0);
        assert!(
            !mv.load(&json!({ "name": "reach", "ticks": [{ "turns": { "3": [0.0,0.0,0.0] } }] })),
            "a rig movement has no model key, so the scratch refuses it"
        );
        assert!(!mv.load(&json!({ "name": "empty", "model": "turing", "ticks": [] })));
        assert!(mv.load(&json!({
            "name": "drop", "model": "geoterrain", "ticks": [[0.1, 0.2], [0.3, 0.0]]
        })));
        assert_eq!(mv.name, "drop");
        assert_eq!(mv.len, 2);
        assert_eq!(
            mv.cells.get(&(1, 1)),
            Some(&0.0),
            "a 0.0 float is still a cell"
        );
        assert!(
            mv.undo.is_empty(),
            "an opened phrase is a new editing session"
        );
        mv.write(0, 0, 0.9);
        mv.undo();
        assert_eq!(
            mv.cells.get(&(0, 0)),
            Some(&0.1),
            "undo only sees post-open edits"
        );
    }

    /// `model_color` is deterministic (the same id always paints the same
    /// band colour across frames) and distinguishes two different ids —
    /// not a proof no two ids ever collide, but enough to catch a
    /// regression that made every model the same colour.
    #[test]
    fn model_color_is_deterministic_and_tells_models_apart() {
        let theme = crate::shell::classes::Theme::phosphor();
        assert_eq!(model_color(&theme, "turing"), model_color(&theme, "turing"));
        assert_ne!(model_color(&theme, "turing"), model_color(&theme, "oracle"));
    }

    // -----------------------------------------------------------------
    // the ingest lane
    // -----------------------------------------------------------------

    /// `interpolate_row`: flat before the first keyframe, flat past the
    /// last, eased between two, and `None` with nothing authored — the
    /// bracket/ease pair `floatsAt` was.
    #[test]
    fn interpolate_row_holds_flat_past_either_end_and_eases_between() {
        let empty = BTreeMap::new();
        assert_eq!(interpolate_row(&empty, 5), None, "nothing authored, no row");

        let mut kf = BTreeMap::new();
        kf.insert(10, vec![0.0]);
        kf.insert(20, vec![1.0]);
        assert_eq!(interpolate_row(&kf, 0), Some(vec![0.0]), "before the first holds flat");
        assert_eq!(interpolate_row(&kf, 30), Some(vec![1.0]), "past the last holds flat");
        assert_eq!(interpolate_row(&kf, 10), Some(vec![0.0]), "exactly on a keyframe");
        let mid = interpolate_row(&kf, 15).unwrap();
        assert!((mid[0] - 0.5).abs() < 1e-6, "the midpoint of a smoothstep ease is still 0.5: {mid:?}");
        let quarter = interpolate_row(&kf, 12).unwrap();
        // ease(0.2) = 0.2*0.2*(3-0.4) = 0.104
        assert!((quarter[0] - 0.104).abs() < 1e-4, "{quarter:?}");
    }

    /// `IngestLane::load` adopts a freshly fetched matrix/song-hits/
    /// keyframes trio at once, clears any stale undo history, and lands the
    /// playhead back at 0 with the row interpolated from what was just
    /// loaded.
    #[test]
    fn load_adopts_matrix_song_hits_and_keyframes_and_resets_the_playhead() {
        let mut lane = IngestLane {
            playhead: 40,
            ..Default::default()
        };
        // A stale edit from before the load: `write_keyframe` records a
        // snapshot on the undo stack that `load` must then clear.
        lane.playhead = 1;
        lane.row = vec![0.9];
        lane.keyframes.insert(1, vec![0.9]);
        lane.write_keyframe();

        let mut hits = std::collections::BTreeSet::new();
        hits.insert(4);
        let mut fresh = BTreeMap::new();
        fresh.insert(0, vec![0.3]);
        lane.load(None, hits.clone(), fresh.clone());

        assert_eq!(lane.playhead, 0);
        assert_eq!(lane.song_hits, hits);
        assert_eq!(lane.keyframes, fresh);
        assert_eq!(lane.row, vec![0.3], "interpolated from the freshly loaded keyframe at 0");
        lane.undo(); // the stale undo from before the load must be gone
        assert_eq!(lane.keyframes, fresh, "undo has nothing pre-load left to restore");
    }

    /// `IngestLane::goto` re-interpolates the row when keyframes exist, and
    /// leaves it untouched otherwise — the "an unauthored model keeps
    /// whatever the operator dialled in" rule.
    #[test]
    fn goto_reinterpolates_only_once_something_is_authored() {
        let mut lane = IngestLane {
            row: vec![0.9],
            ..Default::default()
        };
        lane.goto(3);
        assert_eq!(lane.row, vec![0.9], "nothing authored: the dialled value stays");
        lane.keyframes.insert(0, vec![0.1]);
        lane.goto(0);
        assert_eq!(lane.row, vec![0.1], "now it tracks the keyframe");
    }

    /// Write, then undo, restores the tick to what it was before — including
    /// back to "no keyframe at all" when the write created it.
    #[test]
    fn write_keyframe_then_undo_restores_the_prior_tick() {
        let mut lane = IngestLane {
            model: "wake".into(),
            row: vec![0.4, 0.6],
            playhead: 5,
            ..Default::default()
        };
        lane.write_keyframe();
        assert_eq!(lane.keyframes.get(&5), Some(&vec![0.4, 0.6]));
        lane.row = vec![0.9, 0.1];
        lane.write_keyframe(); // overwrite the same tick
        assert_eq!(lane.keyframes.get(&5), Some(&vec![0.9, 0.1]));
        lane.undo();
        assert_eq!(lane.keyframes.get(&5), Some(&vec![0.4, 0.6]), "back to the first write");
        lane.undo();
        assert_eq!(lane.keyframes.get(&5), None, "back to no keyframe at all");
    }

    /// Clearing an absent tick does nothing and pushes no undo step.
    #[test]
    fn clear_keyframe_is_a_no_op_on_an_absent_tick() {
        let mut lane = IngestLane {
            playhead: 2,
            ..Default::default()
        };
        lane.clear_keyframe();
        lane.undo(); // must not panic or restore anything spurious
        assert!(lane.keyframes.is_empty());
    }

    #[test]
    fn toggle_song_hit_flips_membership() {
        let mut lane = IngestLane::default();
        lane.toggle_song_hit(7);
        assert!(lane.song_hits.contains(&7));
        lane.toggle_song_hit(7);
        assert!(!lane.song_hits.contains(&7));
    }

    /// A model switch resets the row to the new model's defaults and drops
    /// the old model's keyframes — they describe a schema this row no
    /// longer has.
    #[test]
    fn switch_model_resets_row_and_drops_keyframes() {
        let mut lane = IngestLane {
            model: "wake".into(),
            ..Default::default()
        };
        lane.keyframes.insert(3, vec![0.5]);
        lane.playhead = 9;
        lane.switch_model("turing", vec![0.1, 0.2, 0.3]);
        assert_eq!(lane.model, "turing");
        assert_eq!(lane.row, vec![0.1, 0.2, 0.3]);
        assert!(lane.keyframes.is_empty());
        assert_eq!(lane.playhead, 0);
    }

    /// `needs_load`: quiet with no model or no song, quiet again right
    /// after a matching `load`, and loud again either when the song
    /// changes or when a model switch invalidates the load even though the
    /// song string itself did not move.
    #[test]
    fn needs_load_fires_on_a_new_song_and_on_a_model_switch() {
        let mut lane = IngestLane::default();
        assert!(!lane.needs_load(), "no model, no song: nothing to load");
        lane.model = "wake".into();
        assert!(!lane.needs_load(), "still no song");
        lane.song = "click-100".into();
        assert!(lane.needs_load(), "a model and a song, never loaded");

        lane.load(matrix_with_ticks(), Default::default(), Default::default());
        assert!(!lane.needs_load(), "just loaded for this exact song");

        lane.song = "click-200".into();
        assert!(lane.needs_load(), "a different song needs its own load");

        lane.load(matrix_with_ticks(), Default::default(), Default::default());
        assert!(!lane.needs_load());
        lane.switch_model("turing", vec![0.0]);
        assert!(
            lane.needs_load(),
            "a model switch needs a fresh load even though the song did not change"
        );
    }

    /// `R22.7` B3: a matrix with `ticks: 0` is the route's "this song is
    /// not digested" reply, not a load — [`IngestLane::load`] must not
    /// record the song as loaded, and [`IngestLane::needs_load`] must stay
    /// quiet while the panel polls the digest instead.
    #[test]
    fn a_matrix_with_no_ticks_does_not_count_as_loaded() {
        let mut lane = IngestLane {
            model: "wake".into(),
            song: "fresh".into(),
            ..Default::default()
        };
        lane.load(
            Some(DigestMatrix {
                song: "fresh".into(),
                ticks: 0,
                ..Default::default()
            }),
            Default::default(),
            Default::default(),
        );
        assert!(lane.waiting_digest, "an empty matrix means waiting on a digest");
        assert!(
            !lane.needs_load(),
            "waiting on a digest must not re-fetch the matrix every frame"
        );

        // Once the digest lands, the poll clears the wait and the very next
        // `needs_load` asks for the matrix again.
        lane.waiting_digest = false;
        assert!(lane.needs_load(), "a digested song loads on the next frame");
        lane.load(matrix_with_ticks(), Default::default(), Default::default());
        assert!(!lane.waiting_digest);
        assert!(!lane.needs_load(), "and then it is quiet again");
    }

    /// The smallest `DigestMatrix` a load can accept: one tick, which is
    /// what `R22.7` reads as "the digest is on disk".
    fn matrix_with_ticks() -> Option<DigestMatrix> {
        Some(DigestMatrix {
            song: "click-100".into(),
            ticks: 1,
            ..Default::default()
        })
    }

    /// `example_body`: sparse `{tick, floats}` objects under `{model, song,
    /// rate}`, `device`/`layer` present only when a target was given —
    /// `MovementLane::example_ticks_json`'s shape, sourced from keyframes.
    #[test]
    fn example_body_is_sparse_ticks_with_an_optional_target() {
        let mut lane = IngestLane {
            model: "wake".into(),
            song: "click-100".into(),
            ..Default::default()
        };
        assert!(lane.example_body(8.0, None).is_none(), "nothing authored yet");
        // 0.25/0.75, not 0.2/0.9: exact in both f32 and f64, so the JSON
        // round-trip has nothing to lose — the interesting behaviour here is
        // the shape, not float precision (`MovementLane`'s own tests make
        // the same choice for `f64` cells; `row` is `f32`, one more reason).
        lane.keyframes.insert(0, vec![0.25, 0.0]);
        lane.keyframes.insert(3, vec![0.0, 0.75]);
        let body = lane.example_body(8.0, None).expect("saved");
        assert_eq!(body["model"], "wake");
        assert_eq!(body["song"], "click-100");
        assert_eq!(body["rate"], 8.0);
        assert!(body.get("device").is_none());
        let ticks = body["ticks"].as_array().unwrap();
        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[0], serde_json::json!({ "tick": 0, "floats": [0.25, 0.0] }));

        let scoped = lane.example_body(8.0, Some(("wall", "gen-0"))).expect("saved");
        assert_eq!(scoped["device"], "wall");
        assert_eq!(scoped["layer"], "gen-0");
    }

    /// `movement_body`: one dense row per authored tick, in tick order,
    /// under `{name, model}` — refuses a blank name or an empty lane.
    #[test]
    fn movement_body_is_one_row_per_authored_tick_in_order() {
        let mut lane = IngestLane {
            model: "wake".into(),
            ..Default::default()
        };
        assert!(lane.movement_body("drop").is_none(), "nothing authored yet");
        lane.keyframes.insert(5, vec![0.75, 0.25]);
        lane.keyframes.insert(0, vec![0.25, 0.75]);
        assert!(lane.movement_body("  ").is_none(), "a blank name is refused");
        let body = lane.movement_body("drop").expect("saved");
        assert_eq!(body["model"], "wake");
        let ticks = body["ticks"].as_array().unwrap();
        assert_eq!(ticks, &vec![
            serde_json::json!([0.25, 0.75]),
            serde_json::json!([0.75, 0.25]),
        ], "tick order, not insertion order");
    }

    /// Tracks keep their own model, device and keypoints: switching away and
    /// back restores them, and the save posts one body per track, each
    /// aimed at its own device (2026-09-30, the stacked models).
    #[test]
    fn stacked_tracks_keep_their_own_keypoints_and_save_one_body_each() {
        let mut lane = IngestLane { song: "s".into(), ..IngestLane::default() };
        lane.model = "a".into();
        lane.device = Some("screen-1".into());
        lane.keyframes.insert(0, vec![0.1]);
        lane.add_track();
        assert_eq!(lane.track_count(), 2);
        assert!(lane.model.is_empty() && lane.keyframes.is_empty(), "a new track starts empty");
        assert_eq!(lane.device.as_deref(), Some("screen-1"), "aimed at the same device");
        lane.model = "b".into();
        lane.device = Some("screen-2".into());
        lane.keyframes.insert(8, vec![0.5]);

        lane.select_track(0);
        assert_eq!(lane.model, "a");
        assert_eq!(lane.keyframes.get(&0), Some(&vec![0.1]));
        let views = lane.track_views();
        assert_eq!(views[1].model, "b");
        assert_eq!(views[1].keyframes.get(&8), Some(&vec![0.5]));
        assert!(views[0].active && !views[1].active);

        let bodies = lane.all_example_bodies(8.0, Some("gen-1"));
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0]["device"], "screen-1");
        assert_eq!(bodies[1]["device"], "screen-2");
        assert_eq!(bodies[1]["ticks"], serde_json::json!([{ "tick": 8, "floats": [0.5] }]));

        lane.remove_track();
        assert_eq!(lane.track_count(), 1);
        assert_eq!(lane.model, "b", "the remaining track becomes the live one");
    }

    /// `+ layer` stacks a track and makes it the active one — the layer the
    /// properties panel edits.
    #[test]
    fn add_track_stacks_and_focuses_a_new_layer() {
        let mut lane = IngestLane { model: "a".into(), song: "s".into(), ..IngestLane::default() };
        lane.keyframes.insert(0, vec![0.5]);
        lane.add_track();
        assert_eq!(lane.track_count(), 2);
        assert_eq!(lane.active, 1, "the new layer is focused");
        assert_eq!(lane.layer, 1, "and it is a new layer");
        assert!(lane.model.is_empty() && lane.keyframes.is_empty(), "a new layer starts empty");
    }

    /// The whole-sequence undo covers track structure, not just keyframes:
    /// `add_track` and `remove_track` each restore the exact stack — the
    /// active index, the live fields and every clip's keypoints with them.
    #[test]
    fn undo_restores_a_track_add_and_a_track_remove() {
        let mut lane = IngestLane { model: "a".into(), song: "s".into(), ..IngestLane::default() };
        lane.keyframes.insert(0, vec![0.5]);
        lane.goto(3);

        // add_track grows the stack and focuses the new layer; undo puts the
        // count, the active index and the live clip back.
        lane.add_track();
        lane.model = "b".into();
        lane.keyframes.insert(8, vec![0.9]);
        assert_eq!(lane.track_count(), 2);
        assert_eq!(lane.active, 1);
        lane.undo();
        assert_eq!(lane.track_count(), 1, "undo drops the added track");
        assert_eq!(lane.active, 0);
        assert_eq!(lane.model, "a", "and restores the live track");
        assert_eq!(lane.keyframes.get(&0), Some(&vec![0.5]));
        assert_eq!(lane.playhead, 3, "the playhead rides the snapshot");

        // remove_track deletes the active clip; undo restores the whole stack.
        lane.remove_track();
        assert_ne!(lane.model, "a", "the track was removed");
        lane.undo();
        assert_eq!(lane.model, "a");
        assert_eq!(lane.keyframes.get(&0), Some(&vec![0.5]));
    }

    /// The 🗑 delete is one gesture: clearing the marked range and removing
    /// the active track undo together, and removing the last track empties it
    /// rather than leaving nothing.
    #[test]
    fn delete_range_and_track_is_one_undo_step() {
        let mut lane = IngestLane { model: "a".into(), song: "s".into(), ..IngestLane::default() };
        lane.keyframes.insert(0, vec![0.1]);
        lane.keyframes.insert(5, vec![0.5]);
        lane.add_track();
        lane.model = "b".into();
        lane.keyframes.insert(2, vec![0.9]);
        lane.select_track(0);
        lane.goto(0);
        lane.mark = Some((0, 0));

        let n = lane.delete_range_and_track();
        assert_eq!(n, 1, "the mark held one keypoint");
        assert_eq!(lane.track_count(), 1, "the active track was removed");
        assert_eq!(lane.model, "b", "the remaining track becomes live");
        assert_eq!(lane.keyframes.get(&2), Some(&vec![0.9]));

        lane.undo();
        assert_eq!(lane.track_count(), 2, "both halves undo as one step");
        assert_eq!(lane.model, "a");
        assert_eq!(lane.keyframes.get(&0), Some(&vec![0.1]), "the cleared tick is back");
        assert_eq!(lane.keyframes.get(&5), Some(&vec![0.5]));
    }

    /// Two layers keep independent `row`/`model` editing state: editing B
    /// never writes A's row.
    #[test]
    fn two_layers_keep_independent_row_and_model_state() {
        let mut lane = IngestLane { song: "s".into(), ..IngestLane::default() };
        lane.model = "a".into();
        lane.row = vec![0.25, 0.75];
        lane.add_track();
        lane.model = "b".into();
        lane.row = vec![0.5];

        lane.select_track(0);
        assert_eq!(lane.model, "a");
        assert_eq!(lane.row, vec![0.25, 0.75], "layer A kept its row");
        lane.select_track(1);
        assert_eq!(lane.model, "b");
        assert_eq!(lane.row, vec![0.5], "layer B kept its row");
    }

    /// Layer meta round-trips through `project_body`/`adopt_project`, keyed
    /// by `(device, mask, layer)`.
    #[test]
    fn layer_meta_round_trips_through_a_project() {
        let mut lane = IngestLane {
            model: "a".into(),
            song: "s".into(),
            device: Some("tv".into()),
            ..IngestLane::default()
        };
        lane.keyframes.insert(0, vec![0.5]);
        let page = lane.current_page();
        lane.set_layer_meta(
            &page,
            0,
            LayerMeta { blend: "multiply".into(), opacity: 0.4, muted: true, solo: true },
        );
        let body = lane.project_body().unwrap();
        assert_eq!(body["clips"][0]["blend"], "multiply");
        assert_eq!(body["clips"][0]["muted"], true);

        let mut back = IngestLane { song: "s".into(), ..IngestLane::default() };
        assert_eq!(back.adopt_project(&body), 1);
        let meta = back.layer_meta(&back.current_page(), 0);
        assert_eq!(meta.blend, "multiply");
        assert!((meta.opacity - 0.4).abs() < 1e-6);
        assert!(meta.muted && meta.solo);
    }

    /// An old clip JSON with no blend fields loads as `normal / 1.0 / false /
    /// false`.
    #[test]
    fn a_blend_absent_from_an_old_clip_loads_as_normal() {
        let old = json!({
            "song": "s",
            "clips": [{
                "model": "a", "device": "tv", "mask": null, "layer": 0,
                "bounds": [0, 3], "video": null,
                "keyframes": [{ "tick": 0, "floats": [0.5] }]
            }]
        });
        let mut lane = IngestLane { song: "s".into(), ..IngestLane::default() };
        assert_eq!(lane.adopt_project(&old), 1);
        let meta = lane.layer_meta(&lane.current_page(), 0);
        assert_eq!(meta.blend, "normal");
        assert_eq!(meta.opacity, 1.0);
        assert!(!meta.muted && !meta.solo);
    }

    /// `LayerMeta::cycle_blend` walks the four modes and wraps.
    #[test]
    fn cycle_blend_walks_the_four_modes() {
        let mut meta = LayerMeta::default();
        let mut seen = vec![meta.blend.clone()];
        for _ in 0..4 {
            meta.cycle_blend();
            seen.push(meta.blend.clone());
        }
        assert_eq!(seen, vec!["normal", "add", "multiply", "screen", "normal"]);
    }

    /// Collapsed, a track is where its model starts and ends.
    #[test]
    fn a_tracks_extent_is_its_first_and_last_keypoint() {
        let mut k = BTreeMap::new();
        assert_eq!(track_extent(&k), None);
        k.insert(12, vec![]);
        k.insert(4, vec![]);
        k.insert(30, vec![]);
        assert_eq!(track_extent(&k), Some((4, 30)));
    }

    /// Copy the marked range, paste it at the playhead, clear a range, retime
    /// a keypoint — each one undo step.
    #[test]
    fn range_edits_and_retime_are_one_undo_step_each() {
        let mut lane = IngestLane::default();
        lane.keyframes.insert(2, vec![0.2]);
        lane.keyframes.insert(4, vec![0.4]);
        lane.mark = Some((2, 4));
        assert_eq!(lane.copy_range(), 2);
        lane.mark = None;
        lane.playhead = 10;
        assert_eq!(lane.paste(), 2);
        assert_eq!(lane.keyframes.keys().copied().collect::<Vec<_>>(), vec![2, 4, 10, 12]);
        lane.undo();
        assert_eq!(lane.keyframes.keys().copied().collect::<Vec<_>>(), vec![2, 4], "the paste is one step");

        assert!(lane.move_keyframe(4, 7));
        assert_eq!(lane.keyframes.get(&7), Some(&vec![0.4]));
        assert_eq!(lane.playhead, 7, "the playhead follows the moved keypoint");
        lane.undo();
        assert_eq!(lane.keyframes.keys().copied().collect::<Vec<_>>(), vec![2, 4], "the retime is one step");
        assert!(!lane.move_keyframe(5, 6), "nothing at 5");

        lane.mark = Some((0, 3));
        assert_eq!(lane.clear_range(), 1);
        assert_eq!(lane.keyframes.keys().copied().collect::<Vec<_>>(), vec![4]);
        lane.playhead = 0;
        assert_eq!(lane.neighbour_keyframe(true), Some(4));
        assert_eq!(lane.neighbour_keyframe(false), None);
    }

    /// A library sequence opens as its own track, row *i* at tick *i*, and
    /// counts as loaded so the auto-load leaves it alone.
    #[test]
    fn a_sequence_is_adopted_as_a_track_that_the_auto_load_keeps() {
        let mut lane = IngestLane { song: "s".into(), ..IngestLane::default() };
        let keys: BTreeMap<usize, Vec<f32>> = [(0, vec![0.5]), (1, vec![0.7])].into_iter().collect();
        lane.adopt_sequence("geo", keys.clone(), Some("m1".into()));
        assert_eq!(lane.track_count(), 1, "a blank live track is reused");
        lane.adopt_sequence("geo", keys, None);
        assert_eq!(lane.track_count(), 2, "a second one stacks");
        lane.select_track(0);
        assert_eq!(lane.model, "geo");
        assert_eq!(lane.mask.as_deref(), Some("m1"));
        assert!(!lane.needs_load(), "already loaded for this song");
    }

}

#[cfg(test)]
mod loop_tests {
    use super::*;

    /// `start_loop` opens an empty view with no song (so no digest) whose
    /// window is the loop's length, and keeps the canvas rect.
    #[test]
    fn a_loop_is_an_empty_view_with_no_song_and_its_own_window() {
        let mut lane = IngestLane { song: "s".into(), span: 40, ..IngestLane::default() };
        lane.start_loop(16);
        assert_eq!(lane.loop_ticks, Some(16));
        assert!(lane.song.is_empty(), "a loop has no ingestion");
        assert!(lane.matrix.is_none());
        assert_eq!(lane.span, 16);
        assert!(lane.is_blank());
    }

    /// Every clip is clamped to the loop's ticks: a 100-tick video clip on
    /// an 8-tick loop ends on tick 7.
    #[test]
    fn clips_stop_at_the_loop_length() {
        let mut lane = IngestLane::default();
        lane.start_loop(8);
        lane.add_video_clip("loop.mp4", 0, 100, None);
        let bounds = lane.track_views().iter().find_map(|t| t.bounds);
        assert_eq!(bounds, Some((0, 7)));
        assert_eq!(lane.clamp_to_loop(200), 7);
        assert_eq!(IngestLane::default().clamp_to_loop(200), 200, "an ordinary sequence is unlimited");
    }

    /// The menu's lengths are exactly the three the ask names.
    #[test]
    fn the_loop_lengths_are_8_16_and_64() {
        assert_eq!(IngestLane::LOOP_TICKS, [8, 16, 64]);
    }
}
