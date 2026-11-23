//! The updater's settings: one `[update]` table in the workspace's one config
//! file (`~/.config/metalvj/config.toml`, `metal_config::default_path`).
//!
//! No second file. `metal-config` is the crate that owns that document, and
//! its rule is "missing is empty, unknown keys are a lint": an old install with
//! no `[update]` table loads here as every field defaulted, which is the
//! correct answer (channel `main`, enabled, the public repo).
//!
//! ```toml
//! [update]
//! channel = "main"                 # "main" | "dev" (any branch name) | "off"
//! repo = "alonso-leow/metalvj"     # owner/name on GitHub
//! enabled = true
//! # token_file = "secrets/github.token"   # preferred: a file, not the secret
//! # token = ""                            # inline, for a scripted install
//! ```
//!
//! A token is a credential, so it is never in the `[update]` table by default —
//! and `metal-config`'s own rule is that `config.toml` holds *pointers* to
//! secrets, not secrets. It is read from, in order:
//!
//! 1. `update.token` in `config.toml` (a bare token, for a scripted install
//!    with no secret store);
//! 2. `update.token_file` — a path resolved like `[agent.provider].key_file`,
//!    i.e. absolute as-is, relative against the config directory;
//! 3. `METALVJ_GITHUB_TOKEN` in the environment;
//! 4. `GITHUB_TOKEN` in the environment.
//!
//! `token_file` is the recommended one: it keeps the credential out of a file
//! `metal-sync` could ever be asked to travel.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Release;

/// The channel that means "do not check at all".
pub const OFF: &str = "off";
/// The channel that tracks the release line (`v1.2.3` tags, no prereleases).
pub const MAIN: &str = "main";
/// The public repo the app is published from.
pub const DEFAULT_REPO: &str = "alonso-leow/metalvj";

/// Everything `check`/`apply` need. `Default` is the fresh-install answer:
/// channel `main`, the public repo, enabled, no token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    /// `main`, a branch name such as `dev`, or `off`. The channel is the ref,
    /// not a separate concept — see the crate docs on the tag convention.
    pub channel: String,
    /// `owner/name` on GitHub.
    pub repo: String,
    pub enabled: bool,
    /// A bare token. Prefer [`UpdateConfig::token_file`]; this exists for a
    /// scripted install that has nowhere else to put one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// A file holding the token (same resolution rule as a `key_file`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_file: Option<String>,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        UpdateConfig {
            channel: MAIN.to_string(),
            repo: DEFAULT_REPO.to_string(),
            enabled: true,
            token: None,
            token_file: None,
        }
    }
}

impl UpdateConfig {
    /// The config file's `[update]` table, or all defaults when the file (or
    /// the table) is absent. An unparseable file is an error, matching
    /// `Config::load`'s rule — a broken config is never silently replaced.
    ///
    /// `metal-config` keeps a table it does not model in `Config::unknown`
    /// (verbatim, "a key this version doesn't know about"), which is exactly
    /// where `[update]` lands: this crate owns its keys, `metal-config` owns
    /// the file.
    pub fn load() -> Result<UpdateConfig, String> {
        let path = metal_config::default_path(&metal_config::config_dir());
        let cfg = metal_config::Config::load(&path)?;
        Ok(cfg.unknown.get("update").and_then(table_of).unwrap_or_default())
    }

    /// The `[update]` table read out of a TOML document's text — the shape the
    /// tests and `GET /update`'s `--updates` override use. Every key this crate
    /// does not name defaults, so a partial table is normal.
    pub fn from_toml(text: &str) -> Result<UpdateConfig, String> {
        let doc: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
        Ok(doc.get("update").and_then(table_of).unwrap_or_default())
    }

    /// `true` when the channel is `off` (or `enabled = false`): `check`
    /// answers [`crate::Check::Off`] and `apply` refuses.
    pub fn is_off(&self) -> bool {
        !self.enabled || self.channel.trim().eq_ignore_ascii_case(OFF)
    }

    /// The token to send, from the four places above, first one present.
    pub fn token(&self) -> Option<String> {
        if let Some(t) = self.token.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            return Some(t.to_string());
        }
        if let Some(file) = self.token_file.as_deref().filter(|f| !f.trim().is_empty()) {
            let path = resolve_token_file(file);
            let raw = metal_vfs::read_to_string(&path).ok()?;
            let value = parse_token_file(&raw);
            if !value.is_empty() {
                return Some(value);
            }
        }
        for var in ["METALVJ_GITHUB_TOKEN", "GITHUB_TOKEN"] {
            if let Ok(v) = std::env::var(var) {
                let v = v.trim().to_string();
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
        None
    }

    /// Pick the best release for this channel out of an already-fetched list.
    /// See [`crate::select`] — the same decision, with the channel and the
    /// running version already applied.
    pub fn select<'a>(&self, releases: &'a [Release], current: &crate::Version) -> crate::Selection<'a> {
        crate::select(&self.channel, releases, current)
    }
}

/// `NAME=value` or a bare value, the same one-line rule
/// `metal_config::read_key_file` applies: whitespace trimmed, and a line whose
/// left half is an upper-case variable name treated as an assignment.
fn parse_token_file(raw: &str) -> String {
    let trimmed = raw.trim();
    match trimmed.split_once('=') {
        Some((name, value))
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_uppercase() || c == '_') =>
        {
            value.trim().to_string()
        }
        _ => trimmed.to_string(),
    }
}

fn table_of(value: &toml::Value) -> Option<UpdateConfig> {
    value.clone().try_into().ok()
}

/// Mirrors `metal_config`'s `key_file` resolution: absolute as-is, relative
/// against the config directory.
fn resolve_token_file(file: &str) -> PathBuf {
    let p = Path::new(file);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        metal_config::config_dir().join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_main_enabled_public() {
        let c = UpdateConfig::default();
        assert_eq!(c.channel, MAIN);
        assert_eq!(c.repo, DEFAULT_REPO);
        assert!(c.enabled);
        assert_eq!(c.token, None);
        assert!(!c.is_off());
    }

    #[test]
    fn the_table_is_read_and_missing_keys_default() {
        let c = UpdateConfig::from_toml(
            r#"
                [paths]
                data = "./elsewhere"
                [update]
                channel = "dev"
                repo = "someone/fork"
                enabled = false
            "#,
        )
        .unwrap();
        assert_eq!(c.channel, "dev");
        assert_eq!(c.repo, "someone/fork");
        assert!(!c.enabled);
        assert!(c.is_off(), "disabled is off");
        assert_eq!(UpdateConfig::from_toml("[update]\nchannel = \"dev\"\n").unwrap().repo, DEFAULT_REPO);
        assert_eq!(UpdateConfig::from_toml("").unwrap(), UpdateConfig::default());
    }

    #[test]
    fn off_is_off_either_way() {
        let mut c = UpdateConfig::default();
        c.channel = "off".into();
        assert!(c.is_off());
        c.channel = " OFF ".into();
        assert!(c.is_off(), "trimmed and case-insensitive");
        c.channel = "dev".into();
        assert!(!c.is_off());
    }

    #[test]
    fn a_bare_token_wins_and_whitespace_does_not() {
        let mut c = UpdateConfig { token: Some("  tok  ".into()), ..UpdateConfig::default() };
        assert_eq!(c.token().as_deref(), Some("tok"));
        c.token = Some("   ".into());
        c.token_file = Some("/nonexistent/gh.env".into());
        // No env var is set in the test process, so a blank inline token and a
        // missing file are both "absent".
        if std::env::var("METALVJ_GITHUB_TOKEN").is_err() && std::env::var("GITHUB_TOKEN").is_err() {
            assert_eq!(c.token(), None);
        }
    }

    #[test]
    fn token_file_resolves_relative_to_the_config_dir() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("gh.env");
        std::fs::write(&file, "GITHUB_TOKEN=abc123\n").unwrap();
        let c = UpdateConfig {
            token_file: Some(file.to_string_lossy().into_owned()),
            ..UpdateConfig::default()
        };
        assert_eq!(c.token().as_deref(), Some("abc123"));
        std::fs::write(&file, "rawtoken\n").unwrap();
        assert_eq!(c.token().as_deref(), Some("rawtoken"));
    }

    #[test]
    fn the_token_file_parser_takes_both_shapes() {
        assert_eq!(parse_token_file("GITHUB_TOKEN=ghp_1\n"), "ghp_1");
        assert_eq!(parse_token_file("  ghp_2  "), "ghp_2");
        assert_eq!(parse_token_file("a=b=c"), "a=b=c", "lower-case left half is a bare value");
    }
}
