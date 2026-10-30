//! `catalogue` — the two generated authoring manifests, from the sources of
//! truth.
//!
//! `cargo run -p metal-dash --bin catalogue` writes:
//!
//! - `data/songs/catalogue.json`, from the songs in `data/audio/`;
//! - `tools/song-gate/models.json`, from `metal_render::registry::all_models()`.
//!
//! Generated, never hand-typed (rule 7): the folder is the manifest for the
//! songs, the registry table is the manifest for the models. This is a
//! separate bin rather than a subcommand because `metal-dash` has no library
//! target — `main.rs`'s modules are private to the `metal-dash` binary, so a
//! subcommand would mean growing one. The song listing here is deliberately
//! tiny: there is no second state to keep in step, only the folder.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// What librosa opens — the same set `author::audio::SOUND` lists.
const SOUND: [&str; 7] = [".mp3", ".wav", ".flac", ".m4a", ".ogg", ".opus", ".aiff"];

/// The repo root, from this crate rather than the working directory, so the
/// output lands in the same place from a test, a binary, or a different cwd.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// One row per numbered sound file in `dir`, sorted by id.
///
/// `id` is the leading run of digits (`"01"`), `stem` is the file stem the
/// `/author/*` routes key on, `digested` is the sibling `.8hz.npy`, and
/// `genre` is `null` here — the gate app fills it. A sound file with no
/// leading number is not one of the corpus's numbered songs (the folder also
/// holds a `click-100.wav` utility), so it is skipped. The `.npy` check is
/// plain file existence; `author::audio::matrix_ticks` would be the richer
/// test but lives in the `metal-dash` binary and is not reachable from this one.
fn catalogue(dir: &Path) -> Vec<Value> {
    let mut rows: Vec<Value> = metal_vfs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| metal_vfs::is_file(&p))
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            SOUND.iter().any(|e| n.to_lowercase().ends_with(e))
        })
        .filter_map(|audio| {
            let name = audio
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            let stem = audio
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&name)
                .to_owned();
            let id: String = stem.chars().take_while(|c| c.is_ascii_digit()).collect();
            if id.is_empty() {
                return None;
            }
            let matrix = dir.join(metal_schema::digest_file_name(&stem, metal_schema::TickRate::Base));
            Some(json!({
                "id": id,
                "stem": stem,
                "file": format!("data/audio/{name}"),
                "digested": metal_vfs::is_file(&matrix),
                "genre": Value::Null,
            }))
        })
        .collect();
    rows.sort_by(|a, b| {
        let key = |v: &Value| {
            let id = v["id"].as_str().unwrap_or_default();
            // Numeric first, so "9" sorts before "10" even if the corpus ever
            // loses its zero padding; the string breaks ties.
            (id.parse::<u32>().unwrap_or(u32::MAX), id.to_owned())
        };
        key(a).cmp(&key(b))
    });
    rows
}

/// The suite's models, minus tombstones, as the gate app reads them.
///
/// The registry is append-only and a retired model stays as a tombstone that
/// keeps its table position (see `registry.rs`) but must not be offered; the
/// gate app would otherwise carry a row for an animation that draws nothing.
/// `Model::retired` is the marker.
fn models() -> Vec<Value> {
    metal_render::registry::all_models()
        .iter()
        .filter(|m| !m.retired())
        .map(|m| json!({ "id": m.id(), "name": m.name(), "look": metal_render::model::look_line(m.as_ref()) }))
        .collect()
}

fn write_json(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        metal_vfs::create_dir_all(parent).expect("create the output directory");
    }
    let mut text = serde_json::to_string_pretty(value).expect("serialise the manifest");
    text.push('\n');
    metal_vfs::write(path, text).expect("write the manifest");
}

fn main() {
    let root = repo_root();

    let songs = catalogue(&root.join("data/audio"));
    let songs_path = root.join("data/songs/catalogue.json");
    write_json(&songs_path, &Value::Array(songs.clone()));
    println!("wrote {} songs to {}", songs.len(), songs_path.display());

    let models = models();
    let models_path = root.join("tools/song-gate/models.json");
    write_json(&models_path, &Value::Array(models.clone()));
    println!("wrote {} models to {}", models.len(), models_path.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_reads_the_folder_and_sorts_by_id() {
        let dir = std::env::temp_dir().join("fw-catalogue-test");
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        metal_vfs::write(dir.join("10 - Ten.mp3"), b"x").unwrap();
        metal_vfs::write(dir.join(metal_schema::digest_file_name("10 - Ten", metal_schema::TickRate::Base)), b"x").unwrap();
        metal_vfs::write(dir.join("02 - Two.wav"), b"x").unwrap();
        metal_vfs::write(dir.join("click-100.wav"), b"x").unwrap();
        metal_vfs::write(dir.join("notes.txt"), b"x").unwrap();

        let rows = catalogue(&dir);
        assert_eq!(rows.len(), 2, "only the numbered sound files are songs");
        assert_eq!(rows[0]["id"], "02", "sorted by id");
        assert_eq!(rows[1]["id"], "10");
        assert_eq!(rows[0]["stem"], "02 - Two");
        assert_eq!(rows[0]["file"], "data/audio/02 - Two.wav");
        assert_eq!(rows[0]["digested"], false);
        assert_eq!(rows[1]["digested"], true);
        assert!(rows[0]["genre"].is_null());
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// Rule 7: the count is computed from the registry, never written down.
    #[test]
    fn models_json_is_all_models_minus_tombstones() {
        let live: Vec<&str> = metal_render::registry::all_models()
            .iter()
            .filter(|m| !m.retired())
            .map(|m| m.id())
            .collect();
        let rows = models();
        assert_eq!(rows.len(), live.len(), "one row per live model");
        let ids: Vec<&str> = rows.iter().filter_map(|r| r["id"].as_str()).collect();
        assert_eq!(ids, live, "in registry order");
        for row in &rows {
            assert!(
                row["name"].as_str().is_some_and(|s| !s.is_empty()),
                "{} has no label",
                row["id"]
            );
            assert!(
                row["look"].as_str().is_some_and(|s| !s.is_empty()),
                "{} has no look",
                row["id"]
            );
        }
    }
}
