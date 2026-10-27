//! One-shot model download into the registry, with the manifest's hash as the
//! contract (D9 / step 2).
//!
//! Deliberately `std` only: an HTTP client crate would be a heavy dependency
//! fetched over a network this build may not have, when the machine's own
//! `curl` is already the tool `scripts/` uses. The function is small and the
//! failure modes are explicit — a missing `curl`, a failed transfer, or a
//! hash that does not match are all reported rather than papered over.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::registry::{path_for, ModelEntry};
use crate::{sha256_file, MlError};

/// Return the registry file for `entry`, downloading it from `url` if needed.
///
/// If the file is already present and (when the manifest records one) matches
/// its sha256, nothing is fetched. Otherwise `curl` streams `url` to a
/// sibling `.download` file, the hash is checked, and only then is it renamed
/// into place — so a partial or corrupted transfer can never masquerade as the
/// model.
pub fn fetch(entry: &ModelEntry, url: &str, dir: &Path) -> Result<PathBuf, MlError> {
    let dest = path_for(dir, entry);
    if dest.is_file() && !entry.sha256.is_empty() {
        let got = sha256_file(&dest)?;
        if got.eq_ignore_ascii_case(&entry.sha256) {
            return Ok(dest);
        }
    } else if dest.is_file() {
        // Present with no recorded hash: treat it as fetched. There is nothing
        // to verify against, which is the same state `load_checked` accepts.
        return Ok(dest);
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_extension("download");
    let status = Command::new("curl")
        .arg("-fsSL")
        .arg("-o")
        .arg(&tmp)
        .arg(url)
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => return Err(MlError::Ort(format!("curl exited with {s} fetching {url}"))),
        Err(e) => {
            return Err(MlError::Ort(format!(
                "curl is unavailable ({e}); fetch {url} manually into {}",
                dest.display()
            )))
        }
    }

    if !entry.sha256.is_empty() {
        let got = sha256_file(&tmp)?;
        if !got.eq_ignore_ascii_case(&entry.sha256) {
            let _ = std::fs::remove_file(&tmp);
            return Err(MlError::HashMismatch {
                expected: entry.sha256.clone(),
                got,
            });
        }
    }
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}
