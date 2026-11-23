//! `validate(&Config) -> Result<(), Vec<Problem>>` (S.AH point 2).
//!
//! Same convention `metal-schema::ir::validate` already documents: "invalid" is
//! a list of facts, not a single bit — every problem in one pass, not
//! fail-fast, because a bad hex colour and an unknown top-level key are two
//! separate repairs a person needs to see together, not one at a time
//! across two runs.
//!
//! Unknown keys are a [`Severity::Lint`], never an error — the allowlist
//! philosophy `manifest.rs` states outright: a file nobody has thought about
//! yet (or a newer version's field an older binary doesn't know) is noticed,
//! not refused. `metal-sync --doctor` is what prints lints; nothing here ever
//! stops a load over one.

use crate::{Config, Provider};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Blocks nothing, but should be seen — an unknown key, a lint-only
    /// palette slot, a device-scope key that (incorrectly) ended up under
    /// `[studio]`.
    Lint,
    /// The document is internally inconsistent in a way nothing downstream
    /// can paper over — e.g. a palette slot that isn't a colour at all.
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub severity: Severity,
    /// Dotted path to where the problem lives, e.g. `studio.palettes.night.base`.
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tag = match self.severity {
            Severity::Lint => "lint",
            Severity::Error => "error",
        };
        write!(f, "[{tag}] {}: {}", self.path, self.message)
    }
}

/// Collects every [`Problem`] in one pass. `Ok(())` only when there are zero
/// `Error`-severity problems — lints can be present in an `Ok` result, same
/// as a clean `cargo build` can still print warnings. Callers that want the
/// lints regardless of outcome (`--doctor`) should call [`problems`]
/// directly instead of this wrapper.
pub fn validate(cfg: &Config) -> Result<(), Vec<Problem>> {
    let problems = problems(cfg);
    if problems.iter().any(|p| p.severity == Severity::Error) {
        Err(problems)
    } else {
        Ok(())
    }
}

/// Every [`Problem`] `validate` would find, lints included, regardless of
/// whether any of them is an error. `metal-sync --doctor` calls this directly
/// so a clean-but-lint-covered config still prints every lint line.
pub fn problems(cfg: &Config) -> Vec<Problem> {
    let mut out = Vec::new();

    for key in cfg.unknown.keys() {
        out.push(Problem {
            severity: Severity::Lint,
            path: key.clone(),
            message: "unknown top-level key — kept as-is, not acted on".to_string(),
        });
    }
    for key in cfg.studio.custom_nodes.keys() {
        // custom_nodes' own shape (kind/file/floats) is O.6/O.15 territory,
        // not this loader's — presence alone is not a problem.
        let _ = key;
    }

    if cfg.version != 2 {
        out.push(Problem {
            severity: Severity::Lint,
            path: "version".to_string(),
            message: format!("expected 2, found {} — loading anyway", cfg.version),
        });
    }

    for (name, def) in &cfg.studio.palette_defs {
        let path = format!("studio.palettes.{name}");
        let mut any_slot = false;
        for (slot_name, slot) in [
            ("base", &def.base),
            ("acento", &def.accent),
            ("fondo", &def.background),
            ("destello", &def.flash),
        ] {
            if let Some(hex) = slot {
                any_slot = true;
                if !is_hex_color(hex) {
                    out.push(Problem {
                        severity: Severity::Error,
                        path: format!("{path}.{slot_name}"),
                        message: format!("{hex:?} is not a #rrggbb colour"),
                    });
                }
            }
        }
        for (i, hex) in def.extra.iter().enumerate() {
            any_slot = true;
            if !is_hex_color(hex) {
                out.push(Problem {
                    severity: Severity::Error,
                    path: format!("{path}.extra[{i}]"),
                    message: format!("{hex:?} is not a #rrggbb colour"),
                });
            }
        }
        if !any_slot {
            out.push(Problem {
                severity: Severity::Lint,
                path,
                message: "palette has no slots at all".to_string(),
            });
        }
    }

    for (i, pat) in cfg.sync.allowseed.iter().enumerate() {
        if !is_safe_pattern(pat) {
            out.push(Problem {
                severity: Severity::Error,
                path: format!("sync.allowseed[{i}]"),
                message: format!("{pat:?} escapes the data root — refused"),
            });
        } else if reaches_secrets(pat) {
            // S.AL.2: `secrets/` holds a migrated provider key
            // (`secrets_dir`/`migrate_key_to_file`) — `manifest.rs::is_safe`
            // already refuses it unconditionally at the engine level, but a
            // pattern that names it is still a mistake worth a hard error
            // here rather than a lint a person could miss.
            out.push(Problem {
                severity: Severity::Error,
                path: format!("sync.allowseed[{i}]"),
                message: format!("{pat:?} would sync secrets/ — refused"),
            });
        }
    }

    if let Some(provider) = &cfg.agent.provider {
        if provider.url.is_none() && provider.preset.is_none() {
            out.push(Problem {
                severity: Severity::Lint,
                path: "agent.provider".to_string(),
                message: "neither preset nor url is set — nothing to call".to_string(),
            });
        }
        key_file_problems(provider, "agent.provider.key_file", &mut out);
    }

    out
}

/// S.AL.2: `key_file` warnings, checked live against disk every call (rule
/// 4) — never a hard error (a broken `key_file` still leaves `data/ai.json`'s
/// `key` as a fallback, per the precedence `metal_agent::config::ProviderConfig
/// ::load` documents), and never printing the key itself, only the path and
/// the OS-level facts about it. This is the one place these checks live —
/// `metal-sync --doctor` and `scripts/check-config` both call [`problems`]
/// rather than re-implementing either check (rule 6).
fn key_file_problems(provider: &Provider, path: &str, out: &mut Vec<Problem>) {
    let Some(key_file) = &provider.key_file else { return };
    let resolved = crate::resolve_key_file_path(key_file);
    match metal_vfs::metadata(&resolved) {
        Ok(meta) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = meta.permissions().mode() & 0o777;
                if mode & 0o077 != 0 {
                    out.push(Problem {
                        severity: Severity::Lint,
                        path: path.to_string(),
                        message: format!(
                            "{} is mode {mode:03o} — group/other can read it; chmod 600",
                            resolved.display()
                        ),
                    });
                }
            }
            let _ = meta; // used above under cfg(unix); avoid an unused warning elsewhere
        }
        Err(e) => {
            out.push(Problem {
                severity: Severity::Lint,
                path: path.to_string(),
                message: format!(
                    "{} cannot be read ({e}) — falling back to data/ai.json's key, if any",
                    resolved.display()
                ),
            });
        }
    }
}

fn is_hex_color(s: &str) -> bool {
    let s = s.strip_prefix('#').unwrap_or(s);
    s.len() == 6 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Same shape `manifest.rs::is_safe` checks — relative, no `..`, no root —
/// re-implemented here rather than imported because `metal-config` cannot
/// depend on `metal-sync` (that dependency runs the other way: `metal-sync`
/// depends on `metal-config`, not the reverse) and this crate's whole reason to
/// exist is having no dependency heavier than `toml`/`serde`.
fn is_safe_pattern(pat: &str) -> bool {
    !pat.is_empty()
        && !pat.starts_with('/')
        && pat.split('/').all(|seg| seg != ".." && seg != ".")
}

/// Would this pattern let `secrets/…` (or anything under it) sync? Same
/// glob vocabulary `manifest.rs::matches` implements (`*` one segment, `**`
/// the rest) — re-implemented locally for the same reason `is_safe_pattern`
/// is: this crate cannot depend on `metal-sync`. Only the first segment needs
/// checking: an exact `secrets`, or a `**` that swallows everything
/// including it.
fn reaches_secrets(pat: &str) -> bool {
    matches!(pat.split('/').next(), Some("secrets") | Some("**"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PaletteDef;

    #[test]
    fn an_unknown_key_is_a_lint_not_an_error() {
        let mut cfg = Config::default();
        cfg.unknown.insert("frobnicate".to_string(), toml::Value::Boolean(true));
        assert!(validate(&cfg).is_ok(), "a lint alone must not fail validate");
        let ps = problems(&cfg);
        assert!(ps.iter().any(|p| p.severity == Severity::Lint && p.path == "frobnicate"));
    }

    #[test]
    fn an_invalid_palette_hex_is_a_hard_error() {
        let mut cfg = Config::default();
        cfg.studio.palette_defs.insert(
            "broken".to_string(),
            PaletteDef {
                base: Some("not-a-color".to_string()),
                ..Default::default()
            },
        );
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn a_traversal_allowseed_pattern_is_refused() {
        let mut cfg = Config::default();
        cfg.sync.allowseed.push("../../etc/passwd".to_string());
        assert!(validate(&cfg).is_err());
    }

    /// S.AL.2: a hand-typed `allowseed` entry naming `secrets/` (or a
    /// wildcard that would reach it) is a hard error, not a lint — the
    /// engine-level guard in `manifest.rs::is_safe` already makes it inert,
    /// but this is the check that tells a person *why* it's wrong.
    #[test]
    fn an_allowseed_pattern_reaching_secrets_is_refused() {
        for pat in ["secrets/ai.env", "secrets/*", "secrets/**", "**"] {
            let mut cfg = Config::default();
            cfg.sync.allowseed.push(pat.to_string());
            assert!(validate(&cfg).is_err(), "{pat:?} should be refused");
        }
    }

    #[test]
    fn a_provider_with_no_preset_or_url_is_a_lint() {
        let mut cfg = Config::default();
        cfg.agent.provider = Some(Provider { preset: None, url: None, ..Default::default() });
        assert!(validate(&cfg).is_ok());
        assert!(problems(&cfg).iter().any(|p| p.path == "agent.provider"));
    }

    #[test]
    fn the_default_config_has_no_problems() {
        assert!(problems(&Config::default()).is_empty());
    }

    // --- S.AL.2: key_file --------------------------------------------------

    #[test]
    fn a_missing_key_file_is_a_lint_never_an_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut cfg = Config::default();
        cfg.agent.provider = Some(Provider {
            key_file: Some(dir.path().join("nope.env").to_string_lossy().into_owned()),
            ..Default::default()
        });
        assert!(validate(&cfg).is_ok(), "a broken key_file must not abort validate");
        let ps = problems(&cfg);
        assert!(ps.iter().any(|p| p.path == "agent.provider.key_file" && p.severity == Severity::Lint));
    }

    #[test]
    fn no_key_file_at_all_has_no_key_file_problem() {
        let mut cfg = Config::default();
        cfg.agent.provider = Some(Provider { key_file: None, ..Default::default() });
        assert!(!problems(&cfg).iter().any(|p| p.path == "agent.provider.key_file"));
    }

    #[cfg(unix)]
    #[test]
    fn a_world_readable_key_file_is_flagged() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ai.env");
        metal_vfs::write(&path, "ANTHROPIC_API_KEY=sk-test-fake-not-real\n").unwrap();
        metal_vfs::set_permissions(&path, metal_vfs::Permissions::from_mode(0o644)).unwrap();

        let mut cfg = Config::default();
        cfg.agent.provider = Some(Provider {
            key_file: Some(path.to_string_lossy().into_owned()),
            ..Default::default()
        });
        let ps = problems(&cfg);
        let hit = ps.iter().find(|p| p.path == "agent.provider.key_file").expect("flagged");
        assert_eq!(hit.severity, Severity::Lint);
        assert!(!hit.message.contains("sk-test-fake-not-real"), "never print the key");
    }

    #[cfg(unix)]
    #[test]
    fn a_0600_key_file_has_no_permission_problem() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ai.env");
        metal_vfs::write(&path, "ANTHROPIC_API_KEY=sk-test-fake-not-real\n").unwrap();
        metal_vfs::set_permissions(&path, metal_vfs::Permissions::from_mode(0o600)).unwrap();

        let mut cfg = Config::default();
        cfg.agent.provider = Some(Provider {
            key_file: Some(path.to_string_lossy().into_owned()),
            ..Default::default()
        });
        assert!(!problems(&cfg).iter().any(|p| p.path == "agent.provider.key_file"));
    }
}
