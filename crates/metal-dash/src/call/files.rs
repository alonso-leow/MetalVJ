//! The file-transport surface: upload, list and download of media files into
//! the instance's own data directory. Added in S.U18 (15-sept-2026): the
//! headless node — the review deployment, a CI box, a one-core VPS behind
//! nginx — may have no interactive filesystem to pick footage from, but it
//! still has one writable path (`data/files/`). The one client, the egui
//! window, hands it files over the same HTTP surface it already composes
//! through.
//!
//! Names are bare filenames on purpose: no separators, no `..`, no dotfiles.
//! A file that arrives whole lands under `<data_root>/files/<name>` through
//! [`crate::atomic::write`] (temp + rename), so a cut connection can never
//! leave a partial clip behind.

use serde_json::{json, Value};

use crate::call::{Bytes, CallError, Ctx, Params};
use crate::output;

fn files_dir() -> std::path::PathBuf {
    output::data_root().join("files")
}

/// All three handlers take the real data root; the `_at` bodies take any
/// root, so a test points them at its own scratch directory and never at
/// the repo's real `data/` — the same split `output.rs` uses.

/// A bare filename or nothing. Anything with a separator, a `..`, a leading
/// dot or a trailing slash is refused before it ever reaches the disk.
fn safe_name(raw: Option<&str>) -> Result<&str, CallError> {
    let name = raw.unwrap_or_default();
    if name.is_empty()
        || name.starts_with('.')
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
    {
        return Err(CallError::Bad(format!(
            "bad file name {name:?} — a bare name, no separators, no dot prefix"
        )));
    }
    Ok(name)
}

/// `GET /files` — every stored file, with its size in bytes. The directory
/// is created on first listing: an empty instance says so rather than 500ing.
pub fn list(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    list_at(&files_dir())
}

fn list_at(dir: &std::path::Path) -> Result<Value, CallError> {
    metal_vfs::create_dir_all(&dir).map_err(|e| CallError::Internal(format!("files dir: {e}")))?;
    let mut rows = Vec::new();
    let mut names: Vec<String> = metal_vfs::read_dir(&dir)
        .map_err(|e| CallError::Internal(format!("files: {e}")))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().ok().map(|t| t.is_file()) == Some(true))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    for name in names {
        let size = metal_vfs::metadata(dir.join(&name))
            .map(|m| m.len())
            .unwrap_or(0);
        rows.push(json!({ "name": name, "size": size }));
    }
    Ok(json!({ "files": rows }))
}

/// `POST /files { name }` with the file as the raw request body. The auth
/// layer above `dispatch` is what gates who may do this; over a plain
/// loopback instance it is an ordinary write into the data directory.
pub fn upload(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let name = safe_name(params.query.get("name").map(String::as_str))?;
    if params.body.is_empty() {
        return Err(CallError::Bad(
            "an empty body is not an upload — send the file's bytes".into(),
        ));
    }
    let dir = files_dir();
    upload_at(&dir, name, &params.body)
}

fn upload_at(dir: &std::path::Path, name: &str, bytes: &[u8]) -> Result<Value, CallError> {
    metal_vfs::create_dir_all(dir).map_err(|e| CallError::Internal(format!("files dir: {e}")))?;
    crate::atomic::write(dir.join(name), bytes)
        .map_err(|e| CallError::Internal(format!("upload: {e}")))?;
    Ok(json!({ "added": name, "size": bytes.len() }))
}

/// `GET /files/download?name=` — the stored bytes back, as
/// `application/octet-stream`. A name nothing stored is the same 404 every
/// wrong-id route is.
pub fn download(_ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let name = safe_name(params.query.get("name").map(String::as_str))?;
    let bytes = metal_vfs::read(files_dir().join(name)).map_err(|_| CallError::NotFound)?;
    Ok(Bytes {
        kind: "application/octet-stream",
        body: bytes,
    })
}

#[cfg(test)]
fn download_at(dir: &std::path::Path, name: &str) -> Result<Bytes, CallError> {
    let bytes = metal_vfs::read(dir.join(name)).map_err(|_| CallError::NotFound)?;
    Ok(Bytes {
        kind: "application/octet-stream",
        body: bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// The tests point the `_at` bodies at their own scratch directories, so
    /// nothing here touches the repo's real `data/` — and no fixture needs
    /// the full `Ctx` build, because the file handlers are pure
    /// reads/writes into a named directory: what `dispatch` supplies is the
    /// root, and that is what the tests vary.
    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fw-files-{}-{tag}-{}",
            std::process::id(),
            web_time::Instant::now().elapsed().as_nanos()
        ));
        metal_vfs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn named(name: &str) -> Params {
        Params {
            query: HashMap::from([("name".to_string(), name.to_string())]),
            ..Default::default()
        }
    }

    #[test]
    fn an_upload_lands_whole_and_listing_and_download_round_trip_it() {
        let dir = scratch_dir("round-trip");
        let params = Params {
            body: b"frame-frame-frame".to_vec(),
            ..named("crash.mp4")
        };
        // The `upload` handler is a thin wrapper over `upload_at(&dir, ...)`
        // with the instance's real data root (see the fixture note) — the
        // test pins the `_at` body against its own scratch root.
        let reply = upload_at(&dir, "crash.mp4", &params.body).expect("upload");
        assert_eq!(reply["added"], "crash.mp4");
        assert_eq!(reply["size"], 17);
        let rows = list_at(&dir).expect("list");
        let row = rows["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "crash.mp4")
            .expect("the upload is listed");
        assert_eq!(row["size"], 17);
        let back = download_at(&dir, "crash.mp4").expect("download");
        assert_eq!(back.kind, "application/octet-stream");
        assert_eq!(back.body, b"frame-frame-frame");
    }

    #[test]
    fn a_name_that_should_not_reach_the_disk_is_refused_before_the_path() {
        for name in ["../crash.mp4", "/etc/passwd", "a/b.mp4", ".hidden", ""] {
            assert!(
                matches!(safe_name(Some(name)), Err(CallError::Bad(_))),
                "no {name}"
            );
        }
        // A missing `name` key is an empty name — the same 400 branch — and
        // a valid name against nothing stored is the 404 branch.
        let dir = scratch_dir("missing");
        assert!(matches!(
            download_at(&dir, "ghost.bin"),
            Err(CallError::NotFound)
        ));
    }
}
