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
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share")))?;
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
    // Parsing every part's whole file cost ~400 ms a call, and the panel asks
    // every refresh. Keep the last listing per root while no `*.json` under it
    // changed (name, size, mtime).
    type Sig = Vec<(std::path::PathBuf, u64, Option<std::time::SystemTime>)>;
    static MEMO: std::sync::Mutex<Option<(std::path::PathBuf, Sig, Vec<Entry>)>> = std::sync::Mutex::new(None);
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
        if let Some((r, s, entries)) = g.as_ref() {
            if r == root && *s == sig {
                return entries.clone();
            }
        }
    }
    let out = list_uncached(root);
    if let Ok(mut g) = MEMO.lock() {
        *g = Some((root.to_path_buf(), sig, out.clone()));
    }
    out
}

fn list_uncached(root: &Path) -> Vec<Entry> {
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
            let keys = v["ticks"].as_array().map(Vec::len).unwrap_or(0);
            (keys > 0).then_some(())?;
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
