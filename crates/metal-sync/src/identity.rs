//! This node's cryptographic identity, persisted.
//!
//! A peer is addressed by its public key, not by an IP — that is the whole
//! premise of dialling over iroh. So the key has to outlive the process: an
//! identity that changes on restart is not an identity, it is a new stranger
//! every boot, and every peer that trusted the old one has to be re-paired.
//!
//! This replaces the previous `format!("{hostname}-{timestamp}")` node id,
//! which was regenerated on every start and so could never have been dialled
//! by anyone.

use std::path::{Path, PathBuf};

use iroh::SecretKey;

/// Where the key lives when nothing overrides it: `~/.config/metalvj/node.key`.
pub fn default_path() -> PathBuf {
    config_dir().join("node.key")
}

/// `~/.config/metalvj` (or `$XDG_CONFIG_HOME/metalvj`). The lookup
/// itself moved to [`metal_config::config_dir`] so that `metal-agent` and
/// `check-config` resolve the same directory this crate does; this is kept
/// as the name every caller in `metal-sync` already uses.
pub fn config_dir() -> PathBuf {
    metal_config::config_dir()
}

/// Read the key at `path`, generating and saving one if it is not there yet.
///
/// First run writes a key; every run after that reads the same one, which is
/// what makes a paired peer stay paired across a reboot.
pub fn load_or_create(path: &Path) -> Result<SecretKey, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(text.trim())
            .ok_or_else(|| format!("{} is not a 64-character hex key", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = SecretKey::generate();
            save(path, &key)?;
            tracing::info!(node_id = %key.public(), path = %path.display(), "generated a node identity");
            Ok(key)
        }
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn parse(text: &str) -> Option<SecretKey> {
    if text.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(SecretKey::from_bytes(&bytes))
}

fn save(path: &Path, key: &SecretKey) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let hex: String = key.to_bytes().iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(path, &hex).map_err(|e| format!("cannot write {}: {e}", path.display()))?;

    // This is a private key. A default-umask file in a config directory is
    // world-readable on plenty of systems, and the namespace credential is the
    // only thing standing between a stranger and the doc store.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("cannot restrict {}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_survives_a_restart() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("node.key");

        let first = load_or_create(&path).unwrap();
        let second = load_or_create(&path).unwrap();

        assert_eq!(
            first.public(),
            second.public(),
            "the node id must not change between runs, or every paired peer is lost"
        );
    }

    #[test]
    fn two_nodes_get_different_identities() {
        let dir = tempfile::TempDir::new().unwrap();
        let a = load_or_create(&dir.path().join("a.key")).unwrap();
        let b = load_or_create(&dir.path().join("b.key")).unwrap();
        assert_ne!(a.public(), b.public());
    }

    #[test]
    fn the_key_file_is_created_with_a_parent_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nested").join("deeper").join("node.key");
        load_or_create(&path).unwrap();
        assert!(path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_private_key_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("node.key");
        load_or_create(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "group/other must have no access to a secret key");
    }

    #[test]
    fn a_corrupt_key_file_is_an_error_rather_than_a_silent_new_identity() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("node.key");
        std::fs::write(&path, "not a key").unwrap();
        // Silently regenerating here would look like a working node that every
        // peer has stopped recognising, which is far harder to diagnose.
        assert!(load_or_create(&path).is_err());
    }
}
