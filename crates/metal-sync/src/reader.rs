//! Reading the local `data/` tree.
//!
//! # There is no cross-process file lock, and pretending otherwise was worse
//! # than not having one
//!
//! This type used to hold a `Mutex<()>` described as matching "metal-dash's
//! file-locking discipline". It did not, on two counts: `metal-dash`'s `Mutex`
//! guards its in-memory `State` and not any file, and an in-process mutex here
//! cannot exclude a *different* process anyway. There is no advisory locking
//! (`flock`, `fcntl`) anywhere in the workspace.
//!
//! The real answer to the race is on the write side, not the read side — see
//! [`crate::land`], where every write is a rename. A rename is atomic at the
//! filesystem level, so a reader gets the whole old file or the whole new one.
//! That is a guarantee a lock between two cooperating processes could not give
//! us against a third.

use std::fs;
use std::path::{Path, PathBuf};

use crate::manifest::{self, Allowlist};

/// Blocking reads of `data/`, rooted at a directory nothing can escape.
pub struct DataReader {
    data_root: PathBuf,
}

impl DataReader {
    pub fn new(data_root: PathBuf) -> Self {
        DataReader { data_root }
    }

    pub fn root(&self) -> &Path {
        &self.data_root
    }

    /// Read a file by its path relative to the data root.
    ///
    /// The path is checked before it is joined. This used to be a bare
    /// `data_root.join(caller_string)`, which meant the HTTP route
    /// `POST /sync?file=../../etc/passwd` read straight out of the tree.
    pub fn read_file(&self, rel: &Path) -> Result<Vec<u8>, String> {
        if !manifest::is_safe(rel) {
            return Err(format!("{} is not a path inside data/", rel.display()));
        }
        let path = self.data_root.join(rel);
        fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))
    }

    pub fn read_json(&self, rel: &Path) -> Result<serde_json::Value, String> {
        let bytes = self.read_file(rel)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| format!("{} did not parse as json: {e}", rel.display()))
    }

    /// Every path under `data/` that `allow` says syncs, relative to the root.
    ///
    /// This walks the tree rather than listing one directory, because the
    /// allowlist covers nested paths (`graphs/`, `animations/`, `reels/*/`).
    /// It prunes at directories nothing syncable can live under
    /// ([`Allowlist::worth_descending`]), which is what keeps it from
    /// descending into the 426 MB `digest/.venv` or the 146 MB of reel frames
    /// on every scan. The list is a parameter, not a global, because it is
    /// read from `config.toml` per scan — see `SyncState::allowlist`.
    pub fn syncable_files(&self, allow: &Allowlist) -> Vec<PathBuf> {
        let mut out = Vec::new();
        walk(&self.data_root, Path::new(""), allow, &mut out);
        out.sort();
        out
    }
}

fn walk(root: &Path, rel: &Path, allow: &Allowlist, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root.join(rel)) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let child = rel.join(entry.file_name());
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if allow.worth_descending(&child) {
                walk(root, &child, allow, out);
            }
        } else if allow.syncable(&child) {
            out.push(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(body).unwrap();
    }

    #[test]
    fn can_read_file() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "trims.json", b"{}");
        let reader = DataReader::new(dir.path().to_path_buf());
        assert_eq!(reader.read_file(Path::new("trims.json")).unwrap(), b"{}");
    }

    #[test]
    fn missing_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let reader = DataReader::new(dir.path().to_path_buf());
        assert!(reader.read_file(Path::new("nonexistent.json")).is_err());
    }

    #[test]
    fn read_json_parses_correctly() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "trims.json", b"{\"key\": \"value\"}");
        let reader = DataReader::new(dir.path().to_path_buf());
        assert_eq!(reader.read_json(Path::new("trims.json")).unwrap()["key"], "value");
    }

    /// The defect this module shipped with: a caller-supplied path escaping the
    /// data root. Reachable from `POST /sync?file=`.
    #[test]
    fn traversal_cannot_escape_the_data_root() {
        let dir = TempDir::new().unwrap();
        let outside = dir.path().parent().unwrap().join("metal-sync-secret.txt");
        fs::write(&outside, b"secret").unwrap();

        let reader = DataReader::new(dir.path().to_path_buf());
        assert!(reader.read_file(Path::new("../metal-sync-secret.txt")).is_err());
        assert!(reader.read_file(Path::new("/etc/passwd")).is_err());

        let _ = fs::remove_file(&outside);
    }

    #[test]
    fn syncable_files_finds_nested_authored_state_and_skips_the_rest() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "trims.json", b"{}");
        write(dir.path(), "graphs/one.json", b"{}");
        write(dir.path(), "animations/entrance/song.npy", b"x");
        write(dir.path(), "reels/footage/index.json", b"{}");
        // None of these sync.
        write(dir.path(), "ai.json", b"{}");
        write(dir.path(), "schema.json", b"{}");
        write(dir.path(), "reels/footage/frames/000001.jpg", b"x");
        write(dir.path(), "digest/.venv/lib/numpy.so", b"x");
        write(dir.path(), "audio/track.mp3", b"x");

        let reader = DataReader::new(dir.path().to_path_buf());
        let found = reader.syncable_files(&Allowlist::builtin());

        assert_eq!(
            found,
            vec![
                PathBuf::from("animations/entrance/song.npy"),
                PathBuf::from("graphs/one.json"),
                PathBuf::from("reels/footage/index.json"),
                PathBuf::from("trims.json"),
            ]
        );
    }

    #[test]
    fn an_empty_data_root_yields_nothing_rather_than_failing() {
        let dir = TempDir::new().unwrap();
        assert!(DataReader::new(dir.path().to_path_buf())
            .syncable_files(&Allowlist::builtin())
            .is_empty());
    }

    /// A pattern the builtin list never had reaches a directory the walk
    /// would otherwise prune — the prune follows the list, not a fixed set
    /// of directory names.
    #[test]
    fn a_configured_pattern_reaches_a_directory_the_builtin_would_prune() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "notes/today.md", b"#");
        write(dir.path(), "trims.json", b"{}");
        let reader = DataReader::new(dir.path().to_path_buf());
        assert!(reader.syncable_files(&Allowlist::builtin()).iter().all(|p| p != Path::new("notes/today.md")));
        let custom = Allowlist::from_patterns(vec!["notes/*.md".into()]);
        assert_eq!(reader.syncable_files(&custom), vec![PathBuf::from("notes/today.md")]);
    }
}
