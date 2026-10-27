//! Swapping the binary, without ever replacing a file that is still running.
//!
//! # Why not just rename over it
//!
//! On Linux and macOS that is exactly what happens, and it is correct: the
//! kernel keeps the old inode alive for the running process, so a rename is
//! atomic and the next launch runs the new code.
//!
//! Windows is different. An `.exe` that is running holds an exclusive lock on
//! its own image and Windows refuses to open it for writing — which is also
//! why "download an update" cannot simply close and reopen the app the way an
//! installer would. So the Windows path is the standard two-launch dance:
//!
//! 1. `apply` writes the verified download to **`<exe>.new`**, beside the real
//!    binary, and reports [`crate::Applied::Restart`]. Nothing is swapped yet.
//! 2. The operator restarts the app. The launcher shim is whatever launches the
//!    app today: nothing here spawns a process, so quitting and starting again
//!    is the whole of step 2.
//! 3. **First thing in `main`**, [`finish_pending_update`] sees `<exe>.new`,
//!    moves the just-exited binary to `<exe>.old` and the new one into place.
//!    The startup process is the *old* image, so moving it works — the lock
//!    died with the last process — and the swap is a rename, not a write.
//!
//! The startup swap runs on **every** platform, not only Windows: a POSIX
//! install that crashed between its own rename and its first frame is left with
//! the same `<exe>.new`, and picking it up on the next start is the same
//! recovery. Both paths meet here.
//!
//! # Naming
//!
//! `<exe>.new` and `<exe>.old` sit beside the binary, in the same directory, so
//! the final step is always a rename within one file system (a rename across
//! devices is a copy, and a copy of a running image is the thing this avoids).
//! The directory therefore has to be writable by the user running the app, which
//! is true for the `~/.local/bin` launcher install this app ships with and
//! false for a system-wide `/usr/bin` package — there `apply` reports the
//! permission error and changes nothing, which is the honest answer.

use std::io;
use std::path::{Path, PathBuf};

/// The pending-download name for `exe`: `<exe>.new`.
pub fn new_path(exe: &Path) -> PathBuf {
    with_suffix(exe, ".new")
}

/// The "previous binary, kept for one launch" name: `<exe>.old`.
pub fn old_path(exe: &Path) -> PathBuf {
    with_suffix(exe, ".old")
}

/// Where a download lands on its way to `dest` (which is already `<exe>.new`):
/// a `.part` beside it, so a failed download is never mistaken for a complete
/// pending update.
pub fn staging_path(dest: &Path) -> PathBuf {
    with_suffix(dest, ".part")
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// Swap `<exe>.new` into place, keeping the old binary as `<exe>.old`.
///
/// Returns `Ok(true)` when a swap happened, `Ok(false)` when there was nothing
/// pending (the normal case on every launch but the one after an update).
///
/// A zero-length `.new` is discarded: it is the fingerprint of a download that
/// was killed mid-write or of a hand-copied empty file, and installing it would
/// leave an app that cannot start.
///
/// This is the first thing `main` calls. It does no network and no config read,
/// and every failure is returned, never panicked: an unswappable install must
/// still be able to start.
pub fn finish_pending_update(exe: &Path) -> io::Result<bool> {
    let pending = new_path(exe);
    if !pending.exists() {
        return Ok(false);
    }
    if std::fs::metadata(&pending)?.len() == 0 {
        let _ = std::fs::remove_file(&pending);
        return Ok(false);
    }
    let old = old_path(exe);
    let mut kept_old = None;
    if exe.exists() {
        // A `.old` left by the previous swap is from an earlier launch, whose
        // process has exited, so the file can go now; but a `.old` that still
        // holds the previous binary is useful until a *new* swap replaces it,
        // so a missing `exe` (a recovery install) leaves it alone.
        if old.exists() {
            let _ = std::fs::remove_file(&old);
        }
        std::fs::rename(exe, &old)?;
        kept_old = Some(old.as_path());
    }
    // If the second rename fails, put the old binary back: an app that cannot
    // start is worse than one that is one version behind.
    if let Err(e) = std::fs::rename(&pending, exe) {
        if let Some(old) = kept_old {
            let _ = std::fs::rename(old, exe);
        }
        return Err(e);
    }
    // Best-effort: the new image's permissions come from the download. On
    // POSIX the executable bit is what makes it runnable at all.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(exe, std::fs::Permissions::from_mode(0o755));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    #[test]
    fn paths_sit_beside_the_binary() {
        let exe = Path::new("/opt/metalvj/metal-dash");
        assert_eq!(new_path(exe), Path::new("/opt/metalvj/metal-dash.new"));
        assert_eq!(old_path(exe), Path::new("/opt/metalvj/metal-dash.old"));
        assert_eq!(staging_path(&new_path(exe)), Path::new("/opt/metalvj/metal-dash.new.part"));
    }

    #[test]
    fn nothing_pending_is_a_no_op() {
        let dir = app_dir();
        let exe = dir.path().join("metal-dash");
        std::fs::write(&exe, b"old").unwrap();
        assert!(!finish_pending_update(&exe).unwrap());
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
        assert!(!old_path(&exe).exists());
    }

    #[test]
    fn a_pending_update_swaps_in_and_keeps_the_old_binary() {
        let dir = app_dir();
        let exe = dir.path().join("metal-dash");
        std::fs::write(&exe, b"old build").unwrap();
        std::fs::write(new_path(&exe), b"new build").unwrap();

        assert!(finish_pending_update(&exe).unwrap());
        assert_eq!(std::fs::read(&exe).unwrap(), b"new build");
        assert_eq!(std::fs::read(old_path(&exe)).unwrap(), b"old build");
        assert!(!new_path(&exe).exists(), "the pending file is consumed");

        // Second launch: nothing pending, nothing to do — the `.old` from the
        // first swap stays as the one-launch rollback copy and is replaced,
        // never accumulated, by the next swap.
        assert!(!finish_pending_update(&exe).unwrap());
        assert_eq!(std::fs::read(old_path(&exe)).unwrap(), b"old build");
        assert_eq!(std::fs::read(&exe).unwrap(), b"new build");
        std::fs::write(new_path(&exe), b"newer build").unwrap();
        assert!(finish_pending_update(&exe).unwrap());
        assert_eq!(std::fs::read(old_path(&exe)).unwrap(), b"new build", "the old .old went");
    }

    #[test]
    fn an_empty_pending_file_is_discarded() {
        let dir = app_dir();
        let exe = dir.path().join("metal-dash");
        std::fs::write(&exe, b"old build").unwrap();
        std::fs::write(new_path(&exe), b"").unwrap();
        assert!(!finish_pending_update(&exe).unwrap());
        assert_eq!(std::fs::read(&exe).unwrap(), b"old build", "never replaced by nothing");
        assert!(!new_path(&exe).exists());
    }

    #[test]
    fn a_stale_old_file_from_a_previous_run_does_not_block_the_swap() {
        let dir = app_dir();
        let exe = dir.path().join("metal-dash");
        std::fs::write(&exe, b"v2").unwrap();
        std::fs::write(old_path(&exe), b"v1, long gone").unwrap();
        std::fs::write(new_path(&exe), b"v3").unwrap();
        assert!(finish_pending_update(&exe).unwrap());
        assert_eq!(std::fs::read(&exe).unwrap(), b"v3");
        assert_eq!(std::fs::read(old_path(&exe)).unwrap(), b"v2");
    }
}
