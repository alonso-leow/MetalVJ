//! `metal-sync --doctor` (S.AH point 6).
//!
//! "validate → prints what it is really going to do; every ignored/tolerated
//! knob comes out as a lint line. Nothing stays silent." Doctor never
//! mutates anything except running the one-shot migration (point 7) — it is
//! meant to be run before trusting a `config.toml` a person hand-edited, or
//! after landing on a machine for the first time.

use std::path::Path;

/// Runs the whole doctor pass and returns the lines it printed (so tests can
/// assert on content without capturing stdout). `data_root` is where
/// `data/ai.json` would live if the legacy provider file is still around —
/// used for migration only, never read for its key.
pub fn run(config_dir: &Path, data_root: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let config_path = metal_config::default_path(config_dir);
    let sync_toml = config_dir.join("sync.toml");
    let ai_json = data_root.join("ai.json");

    // S.AH point 7: migrate once, before validating, so a fresh machine's
    // first `--doctor` run is also the run that gets a config.toml.
    if !config_path.exists() {
        // `config::load` returns an empty default when `sync.toml` is
        // absent (S.AH's "missing is empty" rule) — that default is not a
        // legacy file to migrate from, so gate on the file actually
        // existing rather than on `load` succeeding.
        let legacy_sync = if sync_toml.exists() {
            crate::config::load(&sync_toml)
                .ok()
                .map(|c| metal_config::LegacySync { namespace: c.namespace })
        } else {
            None
        };
        let legacy_provider = read_legacy_provider(&ai_json);
        match metal_config::migrate(&config_path, legacy_sync, legacy_provider) {
            Ok(Some(migrated)) => {
                lines.push(format!(
                    "migrated: no config.toml found, generated {} from legacy files",
                    config_path.display()
                ));
                for note in migrated.notes {
                    lines.push(format!("migrated: {note}"));
                }
            }
            Ok(None) => {
                lines.push(format!(
                    "no config.toml and no legacy sync.toml/ai.json to migrate from — starting \
                     from defaults ({})",
                    config_path.display()
                ));
            }
            Err(e) => lines.push(format!("migration failed: {e}")),
        }
    }

    let mut cfg = match metal_config::Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            lines.push(format!("error: {} did not parse: {e}", config_path.display()));
            return lines;
        }
    };

    // S.AL.2: move a still-inline key out of `data/ai.json`'s `key` field
    // into `secrets/ai.env`, once. Unlike the bootstrap migration above
    // (gated on `config.toml` being absent), this runs on every call —
    // the common case is a machine that already has a `config.toml` from an
    // earlier S.AH migration and simply never had `key_file` set.
    // `migrate_key_to_file`'s own idempotency (a `key_file` already set is a
    // no-op) is what keeps re-running this safe on every subsequent
    // `--doctor`. The key is read here and nowhere else in this module, and
    // is never pushed into `lines` or logged.
    match metal_config::migrate_key_to_file(&config_path, read_legacy_key(&ai_json).as_deref()) {
        Ok(Some(())) => {
            lines.push(format!(
                "migrated: moved data/ai.json's key into {} (mode 0600) and set \
                 [agent.provider].key_file — the key itself is never printed",
                config_dir.join("secrets").join("ai.env").display()
            ));
            match clear_legacy_key(&ai_json) {
                Ok(()) => lines.push(format!(
                    "migrated: cleared the key field in {}",
                    ai_json.display()
                )),
                Err(e) => lines.push(format!(
                    "migrated: could not clear {}'s key field: {e} — remove it by hand once \
                     {} is confirmed working",
                    ai_json.display(),
                    config_dir.join("secrets").join("ai.env").display()
                )),
            }
            // The table on disk just changed; re-load so the problems/lines
            // below reflect it rather than the pre-migration snapshot.
            cfg = match metal_config::Config::load(&config_path) {
                Ok(c) => c,
                Err(e) => {
                    lines.push(format!(
                        "error: {} did not parse after key migration: {e}",
                        config_path.display()
                    ));
                    return lines;
                }
            };
        }
        Ok(None) => {}
        Err(e) => lines.push(format!("key migration failed: {e}")),
    }

    let problems = metal_config::problems(&cfg);
    if problems.is_empty() {
        lines.push("config.toml: no problems found".to_string());
    }
    for p in &problems {
        lines.push(p.to_string());
    }

    // "what it is really going to do" — the device-scope decisions that actually
    // steer this run, spelled out so nothing is implicit.
    lines.push(format!(
        "sync: {} peer(s) known, doc = {}",
        cfg.sync.peers.len(),
        cfg.sync.doc.as_deref().unwrap_or("(none yet — will create on join)")
    ));
    lines.push(format!("sync: allowseed has {} pattern(s)", cfg.sync.allowseed.len()));
    lines.push(format!(
        "render: engine = {}, device = {}",
        cfg.render.engine, cfg.render.device
    ));
    match &cfg.agent.provider {
        Some(p) => lines.push(format!(
            "agent.provider: preset = {}, url = {}",
            p.preset.as_deref().unwrap_or("(unset)"),
            p.url.as_deref().unwrap_or("(unset)")
        )),
        None => lines.push(
            "agent.provider: not configured in config.toml — data/ai.json's preset/url/model \
             govern until the next save through /ai-config writes them here"
                .to_string(),
        ),
    }
    if !cfg.studio.palette_defs.is_empty() {
        match metal_config::write_palettes_file(data_root, &cfg.studio.palette_defs) {
            Ok(()) => lines.push(format!(
                "studio.palettes: materialized {} palette(s) into {}",
                cfg.studio.palette_defs.len(),
                data_root.join(metal_config::PALETTES_FILE).display()
            )),
            Err(e) => lines.push(format!("studio.palettes: could not materialize: {e}")),
        }
    }
    if !cfg.studio.custom_nodes.is_empty() {
        lines.push(format!(
            "studio.custom_nodes: {} entr(y/ies) — lint only, this doctor does not validate their shape (O.6/O.15 territory)",
            cfg.studio.custom_nodes.len()
        ));
    }

    if problems.iter().any(|p| p.severity == metal_config::Severity::Error) {
        lines.push("doctor: config.toml has hard errors — fix before relying on it".to_string());
    }

    lines
}

/// Reads only `preset`/`url` out of `data/ai.json`, never the key — this is
/// migration's read of the legacy provider file, and S.AH is explicit that
/// the key stays exactly where it already lives.
fn read_legacy_provider(path: &Path) -> Option<metal_config::LegacyProvider> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(metal_config::LegacyProvider {
        preset: value.get("preset").and_then(|v| v.as_str()).map(str::to_string),
        url: value.get("url").and_then(|v| v.as_str()).map(str::to_string),
        model: value.get("model").and_then(|v| v.as_str()).map(str::to_string),
    })
}

/// S.AL.2: reads `data/ai.json`'s `key` field, and only that field — the one
/// place in this module allowed to touch it, for the one-shot move into
/// `secrets/ai.env` via [`metal_config::migrate_key_to_file`]. The value is
/// handed straight to that function and never appears in `lines`.
fn read_legacy_key(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("key").and_then(|v| v.as_str()).map(str::to_string)
}

/// S.AL.2: clears `data/ai.json`'s `key` field in place once its value has
/// been migrated into `secrets/ai.env`, leaving every other field
/// (`url`/`model`/`preset`/`stt`/`tts`) exactly as it was. A targeted edit
/// rather than a round trip through `metal_agent::config::ProviderConfig`:
/// `metal-sync` cannot depend on `metal-agent` (same dependency-direction rule
/// this whole module already follows for `LegacyProvider`).
fn clear_legacy_key(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("{} did not parse as JSON: {e}", path.display()))?;
    let Some(obj) = value.as_object_mut() else {
        return Err(format!("{} is not a JSON object", path.display()));
    };
    obj.insert("key".to_string(), serde_json::Value::String(String::new()));
    let pretty = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    std::fs::write(path, pretty).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_on_a_bare_directory_migrates_nothing_and_reports_defaults() {
        let dir = tempfile::TempDir::new().unwrap();
        let data = tempfile::TempDir::new().unwrap();
        let lines = run(dir.path(), data.path());
        assert!(lines.iter().any(|l| l.contains("no config.toml and no legacy")));
        assert!(lines.iter().any(|l| l.contains("no problems found")));
    }

    #[test]
    fn doctor_migrates_a_legacy_sync_toml_once() {
        let dir = tempfile::TempDir::new().unwrap();
        let data = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("sync.toml"), "namespace = \"abc123\"\n").unwrap();

        let lines = run(dir.path(), data.path());
        assert!(lines.iter().any(|l| l.starts_with("migrated:")));
        let config_path = metal_config::default_path(dir.path());
        assert!(config_path.exists());
        let cfg = metal_config::Config::load(&config_path).unwrap();
        assert_eq!(cfg.sync.doc.as_deref(), Some("abc123"));

        // Second run must not re-migrate or duplicate lines about it.
        let lines2 = run(dir.path(), data.path());
        assert!(!lines2.iter().any(|l| l.starts_with("migrated:")));
    }

    #[test]
    fn doctor_prints_every_lint_even_when_validate_would_pass() {
        let dir = tempfile::TempDir::new().unwrap();
        let data = tempfile::TempDir::new().unwrap();
        let config_path = metal_config::default_path(dir.path());
        std::fs::write(
            &config_path,
            "version = 2\nschema = \"fw/config/2\"\nfrobnicate = true\n",
        )
        .unwrap();

        let lines = run(dir.path(), data.path());
        assert!(lines.iter().any(|l| l.contains("[lint] frobnicate")));
    }

    #[test]
    fn doctor_reports_a_hard_error_without_silently_dropping_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let data = tempfile::TempDir::new().unwrap();
        let config_path = metal_config::default_path(dir.path());
        std::fs::write(
            &config_path,
            "version = 2\nschema = \"fw/config/2\"\n\n[studio.palettes.broken]\nbase = \"nope\"\n",
        )
        .unwrap();

        let lines = run(dir.path(), data.path());
        assert!(lines.iter().any(|l| l.contains("[error]")));
        assert!(lines.iter().any(|l| l.contains("hard errors")));
    }

    // --- S.AL.2: key migration ------------------------------------------

    const TEST_KEY: &str = "sk-test-fake-not-real";

    #[test]
    fn doctor_moves_an_inline_ai_json_key_into_secrets_and_clears_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let data = tempfile::TempDir::new().unwrap();
        let ai_json = data.path().join("ai.json");
        std::fs::write(
            &ai_json,
            format!(r#"{{"preset":"claude","url":"https://api.anthropic.com/v1/messages","model":"claude-sonnet-5","key":"{TEST_KEY}"}}"#),
        )
        .unwrap();

        let lines = run(dir.path(), data.path());
        assert!(
            lines.iter().any(|l| l.starts_with("migrated:") && l.contains("secrets/ai.env")),
            "{lines:?}"
        );
        assert!(!lines.iter().any(|l| l.contains(TEST_KEY)), "the key must never be printed");

        let config_path = metal_config::default_path(dir.path());
        let cfg = metal_config::Config::load(&config_path).unwrap();
        let provider = cfg.agent.provider.expect("carried over from ai.json");
        assert_eq!(provider.key_file.as_deref(), Some("secrets/ai.env"));

        let secret_path = dir.path().join("secrets").join("ai.env");
        let secret = std::fs::read_to_string(&secret_path).unwrap();
        assert!(secret.contains(TEST_KEY));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&secret_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let ai_after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&ai_json).unwrap()).unwrap();
        assert_eq!(ai_after.get("key").and_then(|v| v.as_str()), Some(""));
        // Everything else in ai.json survives untouched.
        assert_eq!(ai_after.get("preset").and_then(|v| v.as_str()), Some("claude"));
        assert_eq!(ai_after.get("model").and_then(|v| v.as_str()), Some("claude-sonnet-5"));
    }

    #[test]
    fn a_second_doctor_run_does_not_re_migrate_the_key() {
        let dir = tempfile::TempDir::new().unwrap();
        let data = tempfile::TempDir::new().unwrap();
        let ai_json = data.path().join("ai.json");
        std::fs::write(&ai_json, format!(r#"{{"key":"{TEST_KEY}"}}"#)).unwrap();

        run(dir.path(), data.path());
        // Simulate a person setting a *new* key by hand afterwards — a
        // second doctor run must not touch it, since key_file is already
        // set (migrate_key_to_file is a no-op once it is).
        std::fs::write(&ai_json, r#"{"key":"sk-a-different-fake-key"}"#).unwrap();
        let lines2 = run(dir.path(), data.path());
        assert!(!lines2.iter().any(|l| l.starts_with("migrated:")));

        let secret_path = dir.path().join("secrets").join("ai.env");
        let secret = std::fs::read_to_string(&secret_path).unwrap();
        assert!(secret.contains(TEST_KEY), "the original migrated secret is untouched");
    }

    #[test]
    fn doctor_with_no_key_in_ai_json_migrates_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let data = tempfile::TempDir::new().unwrap();
        let lines = run(dir.path(), data.path());
        assert!(!lines.iter().any(|l| l.contains("secrets/ai.env")));
        assert!(!dir.path().join("secrets").exists());
    }

    #[test]
    fn a_key_file_warning_appears_as_a_lint_never_an_abort() {
        let dir = tempfile::TempDir::new().unwrap();
        let data = tempfile::TempDir::new().unwrap();
        let config_path = metal_config::default_path(dir.path());
        std::fs::write(
            &config_path,
            "version = 2\nschema = \"fw/config/2\"\n\n[agent.provider]\nkey_file = \"secrets/does-not-exist.env\"\n",
        )
        .unwrap();

        let lines = run(dir.path(), data.path());
        assert!(lines.iter().any(|l| l.contains("[lint]") && l.contains("key_file")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.starts_with("error:")));
    }
}
