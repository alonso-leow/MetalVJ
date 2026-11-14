//! The Colourist dataset: authored **parts** of songs, not whole songs.
//!
//! `data/animations/<model>/<song>.json` (the timeline's own corpus,
//! [`super::models::example`]) is one example per `(model, song)` covering the
//! whole song: before its first keyframe and after its last, the row holds
//! flat, and every one of those held ticks becomes a training take. That is
//! right for a take authored across a song, and wrong for the dataset built on
//! 2026-10-06, which deliberately cuts each song down to 1–5 parts around its
//! anomalies (intro, build→drop, break, bridge…; `scripts/dataset_parts.py`,
//! `data/dataset/colourist/parts.json`). So the parts live in their own folder,
//! [`DIR`], one file per part, and each carries its `span`:
//!
//! ```json
//! { "model": "rings", "song": "<stem>", "rate": 32.0, "ticks": [...],
//!   "part": "67-p2", "span": [865, 1911], "kind": "build-drop",
//!   "keywords": ["samba"], "title": "...", "story": "...", "keys": [...] }
//! ```
//!
//! The first four fields are an [`example::Example`] exactly (serde ignores
//! the rest), so the same `dense::interpolate` fills it; [`takes_for`] then
//! keeps **only the ticks inside the span**. Two models of one song never share
//! a file, and a part never claims the rest of its song.
//!
//! The sequence editor opens a part as a clip whose mark-in/out is the span
//! (`GET /author/dataset`, `GET /author/dataset/example`).

use std::collections::HashMap;
use std::path::Path;

use metal_render::model::{Block as RenderBlock, Schema};
use metal_reel::grade;
use metal_router::npy;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::author::models::example::{self, Example};
use crate::author::models::schema::ModelSchema;

/// One file per authored part.
pub const DIR: &str = "data/dataset/colourist/examples";

/// The Fixtures dataset: one light-fixture-control example per song, beside
/// the pixel parts (`data/dataset/fixtures/examples`, its status beside it).
pub const FIXTURES_DIR: &str = "data/dataset/fixtures/examples";

/// One file per loop example: a whole song with the `gradient` model, its
/// rows still empty for the author. Its status sits in `data/dataset/loops/status.json`.
pub const LOOPS_DIR: &str = "data/dataset/loops/examples";
/// Loop ids are `loop-<catalogue id>`; a part id never starts with it.
pub const LOOP_PREFIX: &str = "loop-";
/// Inputs of [`generate_loops`], relative to the checkout.
pub const GATE: &str = "data/router/gate.json";
pub const CATALOGUE: &str = "data/songs/catalogue.json";
pub const AUDIO: &str = "data/audio";
/// The model every loop example is authored in.
pub const LOOP_MODEL: &str = "gradient";

/// The folder a request's `set` names: `loops` or (default) the colourist parts.
pub fn dir_for(set: Option<&str>) -> &'static str {
    match set {
        Some("loops") => LOOPS_DIR,
        Some("fixtures") => FIXTURES_DIR,
        _ => DIR,
    }
}

/// The loop lengths a label may name, in beats, and `0` for "no loop". Any
/// other value is refused: the example is not a label at all.
fn beats_ok(n: f64) -> bool {
    n == 0.0 || metal_reel::loopfind::LENGTHS.iter().any(|&l| l as f64 == n)
}

/// One approved loop label: the song, the span it covers, and the rhythm in
/// force along it. `keys` are `(tick, beats)` sorted by tick: the label holds
/// from its tick until the next key. `0` beats is an explicit "no loop here";
/// any other value is one of [`metal_reel::loopfind::LENGTHS`]. Ticks before
/// the first key are unlabelled.
#[derive(Clone, Debug, PartialEq)]
pub struct LoopLabel {
    pub part: String,
    pub song: String,
    pub span: (usize, usize),
    pub keys: Vec<(usize, f32)>,
}

impl LoopLabel {
    /// The label in force at `tick`: `None` when no key is in force yet (the
    /// tick is unlabelled), else the beats (`0` for no loop).
    pub fn beats_at(&self, tick: usize) -> Option<f32> {
        self.keys.iter().rev().find(|(t, _)| *t <= tick).map(|(_, b)| *b)
    }
}

/// The approved loop labels under `root` (the loops examples folder), by part.
///
/// An example's `loop_beats` (top level) labels the whole song from tick 0. A
/// tick may carry its own `loop_beats` (`ticks[i].loop_beats`), which takes
/// over from that tick on, so one song can change length or stop looping. A
/// `null` or absent value is unlabelled. A value that is neither `0` nor one of
/// the finder's lengths refuses the whole example, never a silent guess.
/// An example with no label at all is not returned.
pub fn loop_labels(root: &Path) -> Vec<LoopLabel> {
    let statuses = statuses(root);
    let mut out: Vec<LoopLabel> = metal_vfs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| read(&p))
        .filter_map(|v| {
            if v["kind"].as_str() != Some("loop") {
                return None;
            }
            let part = v["part"].as_str()?.to_string();
            if statuses.get(&part).and_then(Value::as_str) != Some("approved") {
                return None;
            }
            let span = span_of(&v)?;
            let song = v["song"].as_str()?.to_string();
            let mut keys = Vec::new();
            match &v["loop_beats"] {
                Value::Null => {}
                n => keys.push((0, valid_beats(n)?)),
            }
            for t in v["ticks"].as_array().into_iter().flatten() {
                match &t["loop_beats"] {
                    Value::Null => {}
                    n => keys.push((t["tick"].as_u64()? as usize, valid_beats(n)?)),
                }
            }
            if keys.is_empty() {
                return None;
            }
            // Stable: the whole-song default (tick 0) sorts before an authored
            // tick 0 and is overridden by it.
            keys.sort_by_key(|k| k.0);
            Some(LoopLabel { part, song, span, keys })
        })
        .collect();
    out.sort_by(|a, b| a.part.cmp(&b.part));
    out
}

/// A `loop_beats` JSON value as beats, or `None` when it is not `0` or a loop
/// length.
fn valid_beats(v: &Value) -> Option<f32> {
    let n = v.as_f64().filter(|n| beats_ok(*n))?;
    Some(n as f32)
}

/// The rhythm finder's training takes: every labelled tick inside an approved
/// loop label's span, with the digest row of that tick. The label is per tick
/// ([`LoopLabel::beats_at`]): a length is a positive, `0` is an explicit
/// negative, and a tick with no key in force yet is not a take. A song that
/// has no digest on disk contributes nothing.
pub fn loop_takes_for(root: &Path, songs_root: &Path) -> Vec<metal_reel::loopfind::Take> {
    let mut out = Vec::new();
    for l in loop_labels(root) {
        let Ok(matrix) = npy::load_song(songs_root, &l.song, metal_schema::TickRate::CANONICAL) else {
            continue;
        };
        if matrix.is_empty() {
            continue;
        }
        let (a, b) = (l.span.0, l.span.1.min(matrix.len() - 1));
        for tick in a..=b {
            let Some(beats) = l.beats_at(tick) else {
                continue;
            };
            let label = (beats > 0.0).then_some(beats);
            let mut music = [0.0f32; metal_reel::MUSIC];
            music.copy_from_slice(&matrix[tick][..metal_reel::MUSIC]);
            out.push(metal_reel::loopfind::Take { song: l.song.clone(), music, label });
        }
    }
    out
}

/// The review status of each part, `{ "<part>": "approved" | "pending" }`,
/// next to the parts folder (`<DIR>/../status.json`). A part not in it is
/// pending.
pub const STATUSES: [&str; 2] = ["pending", "approved"];

fn status_path(root: &Path) -> std::path::PathBuf {
    root.parent().unwrap_or(root).join("status.json")
}

/// Reviews also live outside the checkout (`~/.local/share/metalvj/review-status.json`),
/// since the launcher resets its clone to GitHub on every start. Only the real
/// dataset folder is mirrored; the mirror wins over the tracked file.
fn mirror_path(root: &Path) -> Option<std::path::PathBuf> {
    if !root.ends_with(DIR) {
        return None;
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| metal_config::home_dir().join(".local/share"));
    Some(base.join("metalvj/review-status.json"))
}

fn statuses(root: &Path) -> serde_json::Map<String, Value> {
    let mut all = read(&status_path(root)).and_then(|v| v.as_object().cloned()).unwrap_or_default();
    if let Some(m) = mirror_path(root).and_then(|p| std::fs::read_to_string(p).ok()) {
        if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(&m) {
            all.extend(o);
        }
    }
    all
}

/// Files `part`'s review status (one of [`STATUSES`]).
pub fn set_status(root: &Path, part: &str, status: &str) -> Result<(), String> {
    let part = example::safe_component(part)?;
    if !STATUSES.contains(&status) {
        return Err(format!("status {status:?}: one of {}", STATUSES.join(", ")));
    }
    if !metal_vfs::is_file(&root.join(format!("{part}.json"))) {
        return Err(format!("no dataset part {part:?}"));
    }
    let mut all = statuses(root);
    all.insert(part.to_string(), Value::String(status.to_string()));
    let text = serde_json::to_string_pretty(&Value::Object(all)).map_err(|e| e.to_string())?;
    if let Some(m) = mirror_path(root) {
        if let Some(d) = m.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        std::fs::write(&m, &text).map_err(|e| format!("review mirror: {e}"))?;
    }
    metal_vfs::write(status_path(root), text + "\n").map_err(|e| e.to_string())
}

/// What the picker lists for one part — the example without its rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub part: String,
    pub model: String,
    pub song: String,
    pub span: [usize; 2],
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// How many keyframes it has.
    #[serde(default)]
    pub keys: usize,
    /// Its review status: `pending` until someone approves it.
    #[serde(default)]
    pub status: String,
    /// The gate's primary genre (loops only).
    #[serde(default)]
    pub genre: Option<String>,
}

/// A part on disk: the example, and the span it answers for.
#[derive(Debug, Clone)]
pub struct Part {
    pub example: Example,
    pub span: (usize, usize),
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_str(&metal_vfs::read_to_string(path).ok()?).ok()
}

fn span_of(v: &Value) -> Option<(usize, usize)> {
    let s = v["span"].as_array()?;
    let (a, b) = (s.first()?.as_u64()? as usize, s.get(1)?.as_u64()? as usize);
    (a <= b).then_some((a, b))
}

/// Every part under `root`, sorted by id. A file that is not a part (no
/// `part`, no `span`, no keyframes) is skipped, never an error.
pub fn list(root: &Path) -> Vec<Entry> {
    list_in(root, 1)
}

/// [`list`] keeping the examples with at least `min_keys` keyframes: parts need
/// one; a loop is listed while still empty (`0`).
pub fn list_in(root: &Path, min_keys: usize) -> Vec<Entry> {
    // Parsing every part's whole file cost ~400 ms a call, and the panel asks
    // every refresh. Keep the last listing per root while no `*.json` under it
    // changed (name, size, mtime).
    type Sig = Vec<(std::path::PathBuf, u64, Option<std::time::SystemTime>)>;
    static MEMO: std::sync::Mutex<Option<(std::path::PathBuf, usize, Sig, Vec<Entry>)>> = std::sync::Mutex::new(None);
    let sig: Sig = {
        let mut v: Sig = metal_vfs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .map(|e| {
                let m = e.metadata().ok();
                (e.path(), m.as_ref().map_or(0, |m| m.len()), m.and_then(|m| m.modified().ok()))
            })
            .collect();
        // The statuses file sits beside the folder, not in it.
        let sp = status_path(root);
        let m = metal_vfs::metadata(&sp).ok();
        v.push((sp, m.as_ref().map_or(0, |m| m.len()), m.and_then(|m| m.modified().ok())));
        v.sort();
        v
    };
    if let Ok(g) = MEMO.lock() {
        if let Some((r, m, s, entries)) = g.as_ref() {
            if r == root && *m == min_keys && *s == sig {
                return entries.clone();
            }
        }
    }
    let out = list_uncached(root, min_keys);
    if let Ok(mut g) = MEMO.lock() {
        *g = Some((root.to_path_buf(), min_keys, sig, out.clone()));
    }
    out
}

fn list_uncached(root: &Path, min_keys: usize) -> Vec<Entry> {
    let statuses = statuses(root);
    let mut out: Vec<Entry> = metal_vfs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| read(&p))
        .filter_map(|v| {
            let span = span_of(&v)?;
            // A Fixtures example counts its scene clips instead of ticks.
            let keys = v["ticks"].as_array().or_else(|| v["clips"].as_array()).map(Vec::len).unwrap_or(0);
            (keys >= min_keys).then_some(())?;
            Some(Entry {
                part: v["part"].as_str()?.to_string(),
                model: v["model"].as_str()?.to_string(),
                song: v["song"].as_str()?.to_string(),
                span: [span.0, span.1],
                kind: v["kind"].as_str().map(str::to_string),
                keywords: v["keywords"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|k| k.as_str().map(str::to_string)).collect())
                    .unwrap_or_default(),
                title: v["title"].as_str().map(str::to_string),
                keys,
                status: statuses.get(v["part"].as_str()?).and_then(Value::as_str).unwrap_or("pending").to_string(),
                genre: v["genre"].as_str().map(str::to_string),
            })
        })
        .collect();
    out.sort_by(|a, b| a.part.cmp(&b.part));
    out
}

/// One part's whole file, by id. The id is a path component, so it is
/// checked the way a device/layer id is.
pub fn load(root: &Path, part: &str) -> Result<Value, String> {
    let part = example::safe_component(part)?;
    read(&root.join(format!("{part}.json"))).ok_or_else(|| format!("no dataset part {part:?}"))
}

/// Every part authored for `model`, with its span.
pub fn parts_for(root: &Path, model: &str) -> Vec<Part> {
    let mut out: Vec<(String, Part)> = metal_vfs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| read(&p))
        .filter(|v| v["model"].as_str() == Some(model))
        .filter_map(|v| {
            let span = span_of(&v)?;
            let id = v["part"].as_str().unwrap_or_default().to_string();
            let mut ex: Example = serde_json::from_value(v).ok()?;
            ex.upgrade_to_canonical();
            (!ex.ticks.is_empty()).then_some((id, Part { example: ex, span }))
        })
        .collect();
    // Song then part order: `grade::train` pools consecutive takes per song.
    out.sort_by(|a, b| (a.1.example.song.as_str(), a.0.as_str()).cmp(&(b.1.example.song.as_str(), b.0.as_str())));
    out.into_iter().map(|(_, p)| p).collect()
}

/// The training takes `model`'s parts make: one per dense tick **inside each
/// part's span**, reduced exactly as [`crate::author::grade::takes_for`] reduces
/// a whole-song example (same `offset_of`, same name lookup).
pub fn takes_for(
    root: &Path,
    songs_root: &Path,
    model: &str,
    model_schema: &ModelSchema,
    real_schema: &Schema,
) -> Vec<grade::Take> {
    let names = grade::float_names(real_schema);
    let mut specs: HashMap<&str, &metal_render::model::FloatSpec> = HashMap::new();
    for block in real_schema.blocks {
        if let RenderBlock::Floats(fields) = block {
            for f in fields.iter() {
                specs.insert(f.name, f);
            }
        }
    }
    let mut out = Vec::new();
    for part in parts_for(root, model) {
        let ex = &part.example;
        let Ok(matrix) = npy::load_song(songs_root, &ex.song, metal_schema::TickRate::CANONICAL) else {
            continue;
        };
        if matrix.is_empty() {
            continue;
        }
        let rows = crate::author::models::dense::interpolate(ex, model_schema.width(), matrix.len());
        let (a, b) = (part.span.0, part.span.1.min(matrix.len() - 1));
        for tick in a..=b {
            let values: HashMap<String, f32> =
                crate::author::grade::own_float_values(model_schema, &rows[tick]).into_iter().collect();
            let offsets = names
                .iter()
                .map(|n| {
                    let v = values.get(n).copied().unwrap_or(0.0);
                    match specs.get(n.as_str()) {
                        Some(s) => grade::offset_of(v, s.default, s.min, s.max, s.drive),
                        None => 0.0,
                    }
                })
                .collect();
            out.push(grade::Take {
                song: ex.song.clone(),
                music: matrix[tick][..metal_reel::MUSIC]
                    .try_into()
                    .expect("a digest row is at least the music prefix"),
                offsets,
            });
        }
    }
    out
}

/// The primary genre of a gate `genre` text ("reggaeton, romantic" ->
/// "reggaeton"). Untagged songs and the gate's `NOTE: …` flags (duplicates)
/// have none.
pub fn primary_genre(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.to_ascii_uppercase().starts_with("NOTE") {
        return None;
    }
    let first = t.split(',').next()?.trim().to_lowercase();
    (!first.is_empty()).then_some(first)
}

/// The title a song is compared by for duplicates: the stem without its
/// catalogue number, its last ` - ` field (the title, not the artists),
/// the parenthesised tags and the version words, as letters only.
///
/// This is a simplification of "same rhythm": it finds the same track under
/// another version name (remix, video edit), not two songs that merely share a
/// tempo. The beat-level comparison (`metal_listen::repeat`) needs the decoded
/// audio, which the endpoint does not read.
pub fn rhythm_key(stem: &str) -> String {
    const FILLER: [&str; 10] = ["remix", "remaster", "remastered", "version", "mix", "edit", "official", "video", "audio", "live"];
    let stem = stem.trim();
    let numbered = stem.split_once(" - ").filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    let rest = numbered.map_or(stem, |(_, r)| r);
    let title = rest.rsplit(" - ").next().unwrap_or(rest);
    let mut bare = String::new();
    let mut depth = 0;
    for c in title.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => bare.push(c),
            _ => {}
        }
    }
    bare.split_whitespace()
        .map(|w| w.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>())
        .filter(|w| !w.is_empty() && !FILLER.contains(&w.as_str()))
        .collect()
}

/// What [`generate_loops`] did.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct LoopReport {
    /// Loop ids written now.
    pub written: Vec<String>,
    /// Loop ids whose file already exists: left as they are, authored or not.
    pub kept: Vec<String>,
    /// `(stem, reason)` for every catalogue song not picked.
    pub skipped: Vec<(String, String)>,
}

/// Picks one whole song per primary genre from the catalogue and writes an
/// empty `gradient` loop example for each into `root`.
///
/// Songs are walked in catalogue order. A song is skipped when its genre
/// already has a pick, when it has no primary genre (untagged or `NOTE`),
/// when its [`rhythm_key`] matches a pick, or when `length_of` finds no digest
/// for it (`length_of` returns the song's length in canonical ticks). An
/// existing loop file is never rewritten, so authored rows survive a re-run.
pub fn generate_loops(
    root: &Path,
    gate: &Path,
    catalogue: &Path,
    length_of: impl Fn(&str) -> Option<usize>,
) -> Result<LoopReport, String> {
    let gate: Value = read(gate).ok_or_else(|| format!("no gate {}", gate.display()))?;
    let songs = gate["songs"].as_object().cloned().unwrap_or_default();
    let mut rows: Vec<(String, String)> = read(catalogue)
        .and_then(|v| v.as_array().cloned())
        .ok_or_else(|| format!("no catalogue {}", catalogue.display()))?
        .iter()
        .filter_map(|r| Some((r["id"].as_str()?.to_string(), r["stem"].as_str()?.to_string())))
        .collect();
    rows.sort();

    let mut report = LoopReport::default();
    let mut picked: Vec<(String, String, String)> = Vec::new(); // (genre, stem, id)
    for (id, stem) in rows {
        let genre_text = songs.get(&stem).and_then(|s| s["genre"].as_str()).unwrap_or_default();
        let Some(genre) = primary_genre(genre_text) else {
            report.skipped.push((stem, "no primary genre".into()));
            continue;
        };
        if let Some((_, other, _)) = picked.iter().find(|(g, _, _)| *g == genre) {
            report.skipped.push((stem.clone(), format!("genre {genre} already has {other}")));
            continue;
        }
        let key = rhythm_key(&stem);
        if let Some((_, other, _)) = picked.iter().find(|(_, s, _)| rhythm_key(s) == key) {
            report.skipped.push((stem.clone(), format!("same track as {other}")));
            continue;
        }
        let Some(len) = length_of(&stem).filter(|n| *n > 0) else {
            report.skipped.push((stem.clone(), "no digest".into()));
            continue;
        };
        let part = format!("{LOOP_PREFIX}{id}");
        picked.push((genre.clone(), stem.clone(), part.clone()));
        let path = root.join(format!("{part}.json"));
        if metal_vfs::is_file(&path) {
            report.kept.push(part);
            continue;
        }
        let example = serde_json::json!({
            "model": LOOP_MODEL, "song": stem, "rate": 32.0, "ticks": [],
            "part": part, "span": [0, len - 1], "kind": "loop",
            "genre": genre, "keywords": [genre], "title": stem,
            "loop_beats": null,
        });
        metal_vfs::create_dir_all(root).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(&example).map_err(|e| e.to_string())?;
        metal_vfs::write(&path, text + "\n").map_err(|e| e.to_string())?;
        report.written.push(part);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fw-dataset-{name}-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        dir
    }

    fn part(dir: &Path, id: &str, model: &str, span: [usize; 2]) {
        let v = serde_json::json!({
            "model": model, "song": "s", "rate": 32.0,
            "ticks": [{ "tick": span[0], "floats": [0.0] }, { "tick": span[1], "floats": [1.0] }],
            "part": id, "span": span, "kind": "drop", "keywords": ["samba"],
        });
        metal_vfs::write(dir.join(format!("{id}.json")), v.to_string()).unwrap();
    }

    #[test]
    fn list_reads_every_part_sorted_and_skips_what_is_not_one() {
        let dir = scratch("list");
        part(&dir, "02-p1", "rings", [10, 20]);
        part(&dir, "01-p2", "aurora", [0, 5]);
        metal_vfs::write(dir.join("stray.json"), r#"{"model":"rings"}"#).unwrap();
        let got = list(&dir);
        assert_eq!(got.iter().map(|e| e.part.as_str()).collect::<Vec<_>>(), ["01-p2", "02-p1"]);
        assert_eq!(got[1].span, [10, 20]);
        assert_eq!(got[1].keys, 2);
        assert_eq!(got[0].keywords, ["samba"]);
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn a_status_is_filed_per_part_and_listed() {
        // status.json sits next to the parts folder, so give it its own parent.
        let dir = scratch("status").join("examples");
        metal_vfs::create_dir_all(&dir).unwrap();
        part(&dir, "01-p1", "domain", [0, 10]);
        part(&dir, "02-p1", "rings", [0, 10]);
        assert_eq!(list(&dir)[0].status, "pending");
        set_status(&dir, "01-p1", "approved").unwrap();
        let l = list(&dir);
        assert_eq!((l[0].status.as_str(), l[1].status.as_str()), ("approved", "pending"));
        assert!(set_status(&dir, "01-p1", "maybe").is_err());
        assert!(set_status(&dir, "09-p9", "approved").is_err());
    }

    #[test]
    fn load_refuses_a_part_id_that_walks_out_of_the_folder() {
        let dir = scratch("load");
        part(&dir, "01-p1", "rings", [0, 4]);
        assert_eq!(load(&dir, "01-p1").unwrap()["span"], serde_json::json!([0, 4]));
        assert!(load(&dir, "../01-p1").is_err());
        assert!(load(&dir, "nope").is_err());
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn parts_for_keeps_only_the_models_own_parts_with_their_spans() {
        let dir = scratch("parts");
        part(&dir, "01-p1", "rings", [3, 9]);
        part(&dir, "01-p2", "aurora", [0, 5]);
        let got = parts_for(&dir, "rings");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].span, (3, 9));
        assert_eq!(got[0].example.ticks.len(), 2);
        let _ = metal_vfs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod loop_tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fw-loops-{name}-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A gate and a catalogue in `dir`, with the stems and genres given.
    fn inputs(dir: &Path, rows: &[(&str, &str, &str)]) -> (std::path::PathBuf, std::path::PathBuf) {
        let mut songs = serde_json::Map::new();
        let mut cat = Vec::new();
        for (id, stem, genre) in rows {
            songs.insert((*stem).into(), serde_json::json!({ "genre": genre, "allow": {} }));
            cat.push(serde_json::json!({ "id": id, "stem": stem, "genre": null, "digested": true }));
        }
        let gate = dir.join("gate.json");
        let catalogue = dir.join("catalogue.json");
        metal_vfs::write(&gate, serde_json::json!({ "version": 2, "songs": songs }).to_string()).unwrap();
        metal_vfs::write(&catalogue, serde_json::Value::Array(cat).to_string()).unwrap();
        (gate, catalogue)
    }

    #[test]
    fn only_approved_loops_with_a_label_are_rhythm_finder_labels() {
        let dir = scratch("labels");
        let root = dir.join("examples");
        metal_vfs::create_dir_all(&root).unwrap();
        let ex = |part: &str, kind: &str, loop_beats: serde_json::Value| {
            let v = serde_json::json!({
                "model": LOOP_MODEL, "song": "song-a", "rate": 32.0, "ticks": [],
                "part": part, "span": [4, 9], "kind": kind, "loop_beats": loop_beats,
            });
            metal_vfs::write(root.join(format!("{part}.json")), v.to_string()).unwrap();
        };
        ex("loop-a", "loop", serde_json::json!(2.0));
        ex("loop-b", "loop", serde_json::json!(0));
        ex("loop-c", "loop", serde_json::Value::Null);
        ex("loop-d", "loop", serde_json::json!(1.0));
        ex("loop-e", "drop", serde_json::json!(1.0));
        ex("loop-f", "loop", serde_json::json!(3.0));
        let status = serde_json::json!({ "loop-a": "approved", "loop-b": "approved", "loop-c": "approved",
            "loop-d": "pending", "loop-e": "approved", "loop-f": "approved" });
        metal_vfs::write(dir.join("status.json"), status.to_string()).unwrap();

        // Not a length (3 beats), not 0: refused whole, not guessed.
        let got = loop_labels(&root);
        assert_eq!(
            got,
            vec![
                LoopLabel { part: "loop-a".into(), song: "song-a".into(), span: (4, 9), keys: vec![(0, 2.0)] },
                LoopLabel { part: "loop-b".into(), song: "song-a".into(), span: (4, 9), keys: vec![(0, 0.0)] },
            ]
        );
        // No digest on disk for the song: a label with no row contributes no take.
        assert!(loop_takes_for(&root, &dir.join("no-songs")).is_empty());
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tick_can_change_the_label_partway_through_a_song() {
        let dir = scratch("per-tick");
        let root = dir.join("examples");
        metal_vfs::create_dir_all(&root).unwrap();
        let write = |part: &str, top: serde_json::Value, ticks: serde_json::Value| {
            let v = serde_json::json!({
                "model": LOOP_MODEL, "song": "song-b", "rate": 32.0, "ticks": ticks,
                "part": part, "span": [4, 14], "kind": "loop", "loop_beats": top,
            });
            metal_vfs::write(root.join(format!("{part}.json")), v.to_string()).unwrap();
        };
        // Two beats from the top, no loop from tick 6, half a beat from tick 10.
        write(
            "loop-g",
            serde_json::json!(2.0),
            serde_json::json!([
                { "tick": 6, "floats": [0.0], "loop_beats": 0.0 },
                { "tick": 10, "floats": [0.0], "loop_beats": 0.5 },
                { "tick": 12, "floats": [0.0], "loop_beats": null },
            ]),
        );
        // Unlabelled until its first tick: nothing before tick 8.
        write("loop-h", serde_json::Value::Null, serde_json::json!([{ "tick": 8, "floats": [0.0], "loop_beats": 1.0 }]));
        // One value that is not a length refuses the whole example.
        write("loop-i", serde_json::json!(2.0), serde_json::json!([{ "tick": 5, "floats": [0.0], "loop_beats": 3.0 }]));
        let status = serde_json::json!({ "loop-g": "approved", "loop-h": "approved", "loop-i": "approved" });
        metal_vfs::write(dir.join("status.json"), status.to_string()).unwrap();

        let got = loop_labels(&root);
        let ids: Vec<&str> = got.iter().map(|l| l.part.as_str()).collect();
        assert_eq!(ids, ["loop-g", "loop-h"]);

        let g = &got[0];
        assert_eq!(g.keys, vec![(0, 2.0), (6, 0.0), (10, 0.5)]);
        assert_eq!(g.beats_at(4), Some(2.0));
        assert_eq!(g.beats_at(9), Some(0.0), "no loop from tick 6");
        assert_eq!(g.beats_at(10), Some(0.5), "half a beat from tick 10");
        assert_eq!(g.beats_at(14), Some(0.5), "and it holds to the end");

        let h = &got[1];
        assert_eq!(h.beats_at(7), None, "unlabelled before its first tick");
        assert_eq!(h.beats_at(8), Some(1.0));
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn the_primary_genre_is_the_first_tag_and_flags_have_none() {
        assert_eq!(primary_genre("reggaeton, romantic").as_deref(), Some("reggaeton"));
        assert_eq!(primary_genre(" Pop ").as_deref(), Some("pop"));
        assert_eq!(primary_genre("NOTE: duplicated"), None);
        assert_eq!(primary_genre("  "), None);
    }

    #[test]
    fn a_remix_and_its_original_share_a_rhythm_key() {
        let original = rhythm_key("09 - Kybba - BA BA BAD");
        assert_eq!(rhythm_key("01 - Kybba, Ryan Castro, Sean Paul & Busy Signal - BA BA BAD REMIX (Video Oficial)"), original);
        assert_ne!(rhythm_key("02 - Sean Paul - I'm Still In Love With You (Official Video)"), original);
        assert_eq!(rhythm_key("04 - Pa Curubandé Yo Voy"), "pacurubandéyovoy");
    }

    #[test]
    fn one_whole_song_per_genre_skipping_duplicates_flags_and_undigested() {
        let dir = scratch("gen");
        let (gate, cat) = inputs(
            &dir,
            &[
                ("01", "01 - A - Track One", "reggaeton, romantic"),
                ("02", "02 - B - Track One (Remix)", "edm"),
                ("03", "03 - C - Ballad", "slow, relax"),
                ("04", "04 - D - Ballad Copy", "NOTE: duplicated"),
                ("05", "05 - E - Samba", "samba"),
                ("06", "06 - F - Pop Song", "pop"),
                ("07", "07 - G - No Digest", "jazz"),
                ("08", "08 - H - Pop Again", "pop"),
            ],
        );
        let root = dir.join("examples");
        let lengths = |stem: &str| match stem {
            "07 - G - No Digest" => None,
            "01 - A - Track One" => Some(1000),
            _ => Some(500),
        };
        let report = generate_loops(&root, &gate, &cat, lengths).unwrap();
        assert_eq!(report.written, ["loop-01", "loop-03", "loop-05", "loop-06"]);
        assert!(report.skipped.iter().any(|(s, r)| s == "08 - H - Pop Again" && r.contains("already has")));
        assert!(report.skipped.iter().any(|(s, r)| s == "02 - B - Track One (Remix)" && r.contains("same track")));
        assert!(report.skipped.iter().any(|(s, _)| s == "04 - D - Ballad Copy"));
        assert!(report.skipped.iter().any(|(s, r)| s == "07 - G - No Digest" && r == "no digest"));

        let got = list_in(&root, 0);
        assert_eq!(got.len(), 4);
        let first = got.iter().find(|e| e.part == "loop-01").unwrap();
        assert_eq!((first.model.as_str(), first.span), ("gradient", [0, 999]));
        assert_eq!((first.genre.as_deref(), first.kind.as_deref(), first.keys, first.status.as_str()), (Some("reggaeton"), Some("loop"), 0, "pending"));
        // A loop is listed while empty; a part still needs keyframes.
        assert!(list(&root).is_empty());
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rerun_keeps_the_authored_file_and_approve_is_per_loop() {
        let dir = scratch("keep");
        let (gate, cat) = inputs(&dir, &[("01", "01 - A - Track", "pop"), ("02", "02 - B - Other", "rock")]);
        let root = dir.join("examples");
        generate_loops(&root, &gate, &cat, |_| Some(64)).unwrap();
        // The author fills loop-01; a re-run must not wipe it.
        let mut ex = load(&root, "loop-01").unwrap();
        ex["ticks"] = serde_json::json!([{ "tick": 0, "floats": [0.5, 0.0, 0.0, 0.3] }]);
        metal_vfs::write(root.join("loop-01.json"), ex.to_string()).unwrap();
        let again = generate_loops(&root, &gate, &cat, |_| Some(64)).unwrap();
        assert_eq!(again.kept, ["loop-01", "loop-02"]);
        assert_eq!(load(&root, "loop-01").unwrap()["ticks"].as_array().unwrap().len(), 1);

        set_status(&root, "loop-02", "approved").unwrap();
        let got = list_in(&root, 0);
        assert_eq!((got[0].status.as_str(), got[1].status.as_str()), ("pending", "approved"));
        assert!(set_status(&root, "loop-09", "approved").is_err());
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn the_set_name_picks_the_folder() {
        assert_eq!(dir_for(Some("loops")), LOOPS_DIR);
        assert_eq!(dir_for(None), DIR);
        assert_eq!(dir_for(Some("anything")), DIR);
    }
}
