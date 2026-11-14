//! `~/.config/metalvj/sync.toml` — which document this node belongs to.
//!
//! The namespace id is the club membership. A node that has one rejoins the
//! same document on every start; a node that does not creates one and becomes
//! the first member. Pairing a second device means handing it a ticket, which
//! carries the namespace and a way to reach us — see `SyncEngine::ticket`.
//!
//! # S.AH: sourced from `config.toml` when it exists
//!
//! `plan-sonnet.md` § S.AH: this reader is meant to "take its fields" from
//! the new central `~/.config/metalvj/config.toml` (`metal-config`), with
//! `sync.toml` kept as the fallback and the field name (`namespace`)
//! unchanged. [`load`] and [`save`] keep their exact signatures and their
//! exact "missing is empty / corrupt is an error" behaviour for `sync.toml`
//! itself — every existing test here still exercises that path directly, on
//! a lone temp file with no `config.toml` next to it. The central file only
//! enters the picture when it sits in the *same directory* as the `sync.toml`
//! path a caller hands in (true for every real caller: both default to
//! `identity::config_dir()`), which is how `load`/`save` find it without a
//! new argument.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    /// Hex `NamespaceId` of the document this node syncs. `None` until the
    /// node either creates a document or joins one from a ticket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// What this machine is called in the club — the name `Output.node` and
    /// `Output.composed_on` use. `None` falls back to the hostname (see
    /// `pair::default_name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The `name ↔ node_id` table: every machine this one has paired with,
    /// by the name it announced at `hello`. Frames between machines resolve
    /// a device's `node`/`composed_on` through it, and `fw/frame/1` answers
    /// only the ids in it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub peers: BTreeMap<String, String>,
    /// Seconds a joiner holds its own publishes waiting for the club's first
    /// sync before publishing anyway. `None` uses the engine default (30).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_hold_secs: Option<u64>,
}

pub fn default_path() -> PathBuf {
    crate::identity::config_dir().join("sync.toml")
}

/// Missing `sync.toml` = empty default. A `config.toml` next to it, if one
/// exists, wins — its `[sync].doc` is the same field under a new name
/// (`doc` in the skeleton, `namespace` here; see the module doc comment).
/// A corrupt `sync.toml` is still a hard error when no `config.toml` is
/// there to source from instead — silently starting a fresh namespace would
/// orphan the node from every peer it was paired with.
pub fn load(path: &Path) -> Result<SyncConfig, String> {
    if let Some(dir) = path.parent() {
        let central = metal_config::default_path(dir);
        if central.exists() {
            let cfg = metal_config::Config::load(&central)?;
            // The namespace is sourced centrally; the name and the peer
            // table have no `config.toml` field yet, so they still come from
            // `sync.toml` — a missing or corrupt one simply contributes none.
            let local: SyncConfig = std::fs::read_to_string(path)
                .ok()
                .and_then(|t| toml::from_str(&t).ok())
                .unwrap_or_default();
            return Ok(SyncConfig { namespace: cfg.sync.doc, ..local });
        }
    }
    match std::fs::read_to_string(path) {
        Ok(text) => {
            toml::from_str(&text).map_err(|e| format!("{} did not parse: {e}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SyncConfig::default()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// Writes `sync.toml` as before, and mirrors the namespace into
/// `config.toml`'s `[sync].doc` when that file already exists next to it —
/// otherwise a later `load` (which prefers the central file once it exists)
/// would keep reading the namespace this call just wrote to `sync.toml` and
/// never see it.
pub fn save(path: &Path, cfg: &SyncConfig) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let text = toml::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;

    if let Some(dir) = path.parent() {
        let central = metal_config::default_path(dir);
        if central.exists() {
            let mut central_cfg = metal_config::Config::load(&central)?;
            central_cfg.sync.doc = cfg.namespace.clone();
            central_cfg.save(&central)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_config_is_an_empty_one_rather_than_an_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfg = load(&dir.path().join("absent.toml")).unwrap();
        assert!(cfg.namespace.is_none());
    }

    #[test]
    fn the_namespace_survives_a_restart() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sync.toml");
        save(&path, &SyncConfig { namespace: Some("abc123".into()), ..Default::default() }).unwrap();
        assert_eq!(load(&path).unwrap().namespace.as_deref(), Some("abc123"));
    }

    #[test]
    fn a_corrupt_config_is_an_error_rather_than_a_silent_new_document() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sync.toml");
        std::fs::write(&path, "this is not toml = = =").unwrap();
        // Falling back to a fresh namespace here would quietly orphan the node
        // from every peer it was paired with.
        assert!(load(&path).is_err());
    }

    #[test]
    fn the_join_hold_timeout_is_read_from_sync_toml() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sync.toml");
        std::fs::write(&path, "join_hold_secs = 5\n").unwrap();
        assert_eq!(load(&path).unwrap().join_hold_secs, Some(5));
        assert_eq!(load(&dir.path().join("absent.toml")).unwrap().join_hold_secs, None);
    }

    #[test]
    fn the_name_table_survives_a_restart() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sync.toml");
        let mut cfg = SyncConfig { name: Some("bar".into()), ..Default::default() };
        cfg.peers.insert("stage".into(), "abc".into());
        save(&path, &cfg).unwrap();
        let back = load(&path).unwrap();
        assert_eq!(back.name.as_deref(), Some("bar"));
        assert_eq!(back.peers.get("stage").map(String::as_str), Some("abc"));
    }

    #[test]
    fn a_central_config_toml_next_to_sync_toml_wins_on_load() {
        let dir = tempfile::TempDir::new().unwrap();
        let sync_path = dir.path().join("sync.toml");
        save(&sync_path, &SyncConfig { namespace: Some("from-sync-toml".into()), ..Default::default() }).unwrap();

        let central_path = metal_config::default_path(dir.path());
        let mut central = metal_config::Config::default();
        central.sync.doc = Some("from-config-toml".into());
        central.save(&central_path).unwrap();

        assert_eq!(load(&sync_path).unwrap().namespace.as_deref(), Some("from-config-toml"));
    }

    #[test]
    fn saving_mirrors_into_an_existing_central_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let sync_path = dir.path().join("sync.toml");
        let central_path = metal_config::default_path(dir.path());
        metal_config::Config::default().save(&central_path).unwrap();

        save(&sync_path, &SyncConfig { namespace: Some("mirrored".into()), ..Default::default() }).unwrap();

        let central = metal_config::Config::load(&central_path).unwrap();
        assert_eq!(central.sync.doc.as_deref(), Some("mirrored"));
    }
}
