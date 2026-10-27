//! `M1`: temp-file + rename saves.
//!
//! A show is a process that can be killed mid-request, and `metal_vfs::write`
//! truncates the destination *first* — a crash between the truncate and the
//! last byte leaves the venue reading a half-written `outputs.json` /
//! `collections.json` / graph file, which is worse than the old file the save
//! was replacing. Writing the bytes to a sibling temp and renaming it over the
//! destination makes the swap atomic on the same filesystem: a reader sees the
//! old file or the new one, never a torn one.
//!
//! The scratch name is unique per process *and* per call, so two saves racing
//! in one directory cannot clobber each other's temp — the reason this is not
//! the single fixed `.{name}.tmp` the earlier chat-only writer used.
//! `chat::save_chat_at` is the original of this shape (and now routes through
//! here); every whole-file data save in the crate goes through [`write`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic per-process suffix for the scratch name — pid alone is not
/// enough when two frames/requests save the same file back to back.
static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

/// A per-process tag for scratch names. The browser has no process ids (`std::process::id`
/// panics there), and its single thread never races, so the counter alone does.
pub fn pid_tag() -> u32 {
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::process::id()
    }
}

fn scratch_for(path: &Path) -> PathBuf {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("save");
    let scratch = format!(
        ".{name}.{}.{}.tmp",
        pid_tag(),
        NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
    );
    match path.parent() {
        Some(dir) => dir.join(scratch),
        None => PathBuf::from(scratch),
    }
}

/// Write `bytes` to `path` through a sibling temp file and a rename.
///
/// The caller owns `create_dir_all` (every existing save site already does it);
/// this only needs the destination's directory to exist. A failed write or
/// rename removes the scratch, so a refused save never leaves a `.tmp` behind.
pub fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    let path = path.as_ref();
    let scratch = scratch_for(path);
    if let Err(e) = metal_vfs::write(&scratch, bytes) {
        let _ = metal_vfs::remove_file(&scratch);
        return Err(e);
    }
    if let Err(e) = metal_vfs::rename(&scratch, path) {
        let _ = metal_vfs::remove_file(&scratch);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fw-atomic-{}-{}-{tag}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        metal_vfs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// The save lands whole, overwrites in place, and leaves no `.tmp`
    /// sibling — the scratch is either renamed onto the destination or (on a
    /// failure) removed.
    #[test]
    fn write_lands_the_bytes_and_leaves_no_scratch() {
        let dir = scratch_dir("lands");
        let path = dir.join("outputs.json");

        write(&path, b"one").expect("first write");
        assert_eq!(metal_vfs::read(&path).expect("read"), b"one");

        write(&path, b"two").expect("overwrite");
        assert_eq!(metal_vfs::read(&path).expect("read"), b"two");

        let leftovers: Vec<String> = metal_vfs::read_dir(&dir)
            .expect("read_dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "scratch left behind: {leftovers:?}");
        metal_vfs::remove_dir_all(&dir).ok();
    }

    /// A save whose destination directory does not exist fails rather than
    /// silently succeeding, and creates nothing — the caller's own
    /// `create_dir_all` is what makes that directory; this helper does not
    /// invent it.
    #[test]
    fn a_missing_destination_directory_is_an_error_not_a_silent_creation() {
        let dir = scratch_dir("missing");
        let path = dir.join("no-such-dir").join("graph.json");
        assert!(write(&path, b"x").is_err());
        assert!(!metal_vfs::exists(&path));
        metal_vfs::remove_dir_all(&dir).ok();
    }
}
