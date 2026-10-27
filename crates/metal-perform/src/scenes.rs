//! `data/scenes/favorites.json` as `reel.batch` reads it: the format and its
//! writer live in [`metal_reel::scenes`]; this is only the per-frame cache.
//!
//! [`live`] re-reads the file when its modification time changes, so a group
//! edited while a batch plays is what the batch steps through from its next
//! tick on. A missing or malformed file reads as empty: a batch that names a
//! scene the file does not have simply skips it.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use web_time::SystemTime;

pub use metal_reel::scenes::{Scene, SceneBook as Favorites};

/// `<root>/scenes/favorites.json`.
pub fn path(root: &Path) -> PathBuf {
    root.join(metal_reel::scenes::BOOK)
}

/// Reads the file; anything missing or unreadable is the empty set.
pub fn load(root: &Path) -> Favorites {
    Favorites::load(&path(root))
}

/// [`load`], reused until the file's modification time (or the root) changes
/// — cheap enough to call once per frame.
pub fn live(root: &Path) -> Favorites {
    type Cached = (PathBuf, Option<SystemTime>, Favorites);
    static CACHE: OnceLock<Mutex<Option<Cached>>> = OnceLock::new();
    let path = path(root);
    let stamp = metal_vfs::metadata(&path).and_then(|m| m.modified()).ok();
    let mut slot = CACHE.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(|e| e.into_inner());
    if let Some((p, s, f)) = slot.as_ref() {
        if *p == path && *s == stamp && stamp.is_some() {
            return f.clone();
        }
    }
    let fresh = Favorites::load(&path);
    *slot = Some((path, stamp, fresh.clone()));
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_empty() {
        let dir = std::env::temp_dir().join("fw-scenes-missing");
        assert!(load(&dir).scenes.is_empty());
    }
}
