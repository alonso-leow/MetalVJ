//! Provider config: `[agent.provider]` in `config.toml` plus `data/ai.json`.
//!
//! Non-negotiable rule (H.3): "La clave no se escribe en el grafo ni en los
//! logs y la UI la muestra enmascarada." Concretely, that means:
//!
//! - `ProviderConfig` is the *only* struct allowed to hold the raw key.
//! - Its `Debug` impl is hand-written and never prints the raw key — every
//!   log line, panic message, or `{:?}` a caller reaches for gets the masked
//!   form instead. There is no derive here on purpose: deriving `Debug`
//!   would silently start leaking the key again the day a field gets
//!   reordered.
//! - Nothing in this crate copies the key into a graph/state file other than
//!   `data/ai.json` itself. `config.toml` never holds it (O.26: "las claves
//!   viven fuera") — `metal_config::Provider` has no `key` field at all.
//!
//! # S.AH: two files, one precedence
//!
//! `plan-sonnet.md` § S.AH: this reader "toma sus campos" from the central
//! `~/.config/metalvj/config.toml`. S.AH itself deferred the wiring
//! because the data root (`--data`, where `ai.json` lives) has no path
//! relationship to the config dir; the resolution is that it needs none —
//! the config dir is resolved on its own by [`metal_config::config_dir`]
//! (`$XDG_CONFIG_HOME/metalvj`, else `~/.config/metalvj`), the same
//! lookup `metal-sync` and `check-config` use, and [`ProviderConfig::load`] /
//! [`ProviderConfig::save`] call it themselves so no caller had to grow an
//! argument. `load_at`/`save_at` take the `config.toml` path explicitly for
//! tests and for anything that already knows a different one.
//!
//! What lives where, and who wins:
//!
//! | field            | `config.toml` `[agent.provider]` | `data/ai.json`     |
//! |------------------|----------------------------------|--------------------|
//! | `preset`/`url`/`model` | **authority** when the table exists | fallback only |
//! | `key`            | never — `key_file` is a *pointer* to one | fallback only |
//! | `stt`/`tts`      | never (each carries its own key) | **only place**     |
//!
//! `load` reads `ai.json` (missing → `None`, as before) and then, if
//! `config.toml` has an `[agent.provider]` table, overrides `preset`/`url`/
//! `model` from it — a table with no `ai.json` beside it still loads, as a
//! provider with an empty key, so the UI can show *which* provider is chosen
//! and say the key is missing rather than pretend nothing is configured.
//! `save` writes `ai.json` whole (the pre-S.AH shape, so a machine with no
//! `config.toml` and the one-shot `metal_config::migrate` keep working
//! unchanged) and, when `config.toml` exists, writes `preset`/`url`/`model`
//! into its `[agent.provider]` too, leaving `key_file` and every other
//! section untouched. It does not *create* `config.toml`: that is
//! migration's job (`metal-sync --doctor`), and until it has run `ai.json`
//! alone governs — the same rule `metal_sync::config::save` follows for the
//! namespace. Once the table exists, `ai.json`'s copy of those three fields
//! is never read again; it is rewritten on every save only so the file stays
//! a complete provider on its own.
//!
//! # S.AL.2: the key itself has its own, narrower precedence
//!
//! `key_file` (S.AL, `plan-opus.md` § O.30: "las claves nunca viven inline
//! en config.toml") is a pointer, not the key, and unlike `preset`/`url`/
//! `model` it does not simply win outright — a `key_file` that's set but
//! broken (file missing, unreadable) must not brick the provider when
//! `ai.json` still has a perfectly good key. So: **`key_file` (via
//! [`metal_config::read_key_file`], read live off disk every `load`, never
//! cached) wins when it resolves to a non-empty key; otherwise `ai.json`'s
//! own `key` field; otherwise empty.** `key_file` is read *after* `ai.json`
//! is loaded — `ai.json`'s key is the starting value `key_from_file`
//! overrides, exactly the same shape as the `preset`/`url`/`model`
//! precedence above. Once a `key_file` is migrated in
//! (`metal_config::migrate_key_to_file`), [`ProviderConfig::save`] stops
//! mirroring the resolved key back into `ai.json` — see its doc comment —
//! so the plaintext copy this precedence still tolerates as a *fallback*
//! does not silently reappear as the *authority*.
//!
//! A `config.toml` that exists but does not parse is an error here, as it is
//! everywhere else (`metal_config::Config::load`'s rule) — the chat says "cannot
//! read config.toml", not "no provider configured".

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Which provider preset a config was built from. `Manual` leaves `url` and
/// `model` fully user-specified; the others prefill `url` from
/// [`Preset::default_url`] but the user can still override it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Claude,
    Openai,
    Deepinfra,
    Manual,
}

impl Preset {
    fn manual() -> Preset {
        Preset::Manual
    }

    /// The base URL a preset prefills. `Manual` has none — the whole point
    /// of manual is that the user supplies it.
    pub fn default_url(self) -> Option<&'static str> {
        match self {
            Preset::Claude => Some("https://api.anthropic.com/v1/messages"),
            Preset::Openai => Some("https://api.openai.com/v1/chat/completions"),
            Preset::Deepinfra => Some("https://api.deepinfra.com/v1/openai/chat/completions"),
            Preset::Manual => None,
        }
    }
}

/// `{ url, model, key, preset, stt?, tts? }`, persisted at `data/ai.json`.
///
/// `key` is `pub` because the loop needs the real value to authenticate a
/// request — hiding it behind a getter wouldn't stop a careless `println!`,
/// only a masked `Debug` does that. Treat `Debug`/`{:?}` as the safe path and
/// reach for `.key` directly only at the one call site that builds the
/// outbound request.
///
/// `stt`/`tts` (H.6) are their own optional [`VoiceConfig`] entries in the
/// same file, not a reuse of `url`/`model`/`key` above — the doc is explicit
/// that voice "pueden ser un endpoint distinto" from the text model, so a
/// setup can point text at Claude and STT/TTS at an OpenAI-compatible
/// endpoint (or vice versa) without the two colliding. Absent means "voice
/// not configured", which is the normal case, not an error.
#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// `#[serde(default)]` on `url`/`model`/`preset`: once `config.toml`
    /// governs them an `ai.json` trimmed by hand to just `{ key }` is a
    /// legal file, not a parse error.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub model: String,
    pub key: String,
    #[serde(default = "Preset::manual")]
    pub preset: Preset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt: Option<VoiceConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tts: Option<VoiceConfig>,
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("url", &self.url)
            .field("model", &self.model)
            .field("key", &mask_key(&self.key))
            .field("preset", &self.preset)
            .field("stt", &self.stt)
            .field("tts", &self.tts)
            .finish()
    }
}

/// A standalone STT or TTS endpoint: its own `url`/`model`/`key`, independent
/// of the text provider above. Same masking discipline as `ProviderConfig` —
/// no derived `Debug`, so a reordered field can't silently start leaking the
/// key again.
#[derive(Clone, Serialize, Deserialize)]
pub struct VoiceConfig {
    pub url: String,
    pub model: String,
    pub key: String,
}

impl std::fmt::Debug for VoiceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceConfig")
            .field("url", &self.url)
            .field("model", &self.model)
            .field("key", &mask_key(&self.key))
            .finish()
    }
}

/// `sk-***last4`, or `***` for anything too short to have a safe tail.
/// Never returns a substring longer than the last 4 characters of the
/// original key.
fn mask_key(key: &str) -> String {
    const TAIL: usize = 4;
    if key.len() <= TAIL {
        return "***".to_string();
    }
    let tail = &key[key.len() - TAIL..];
    format!("***{tail}")
}

impl ProviderConfig {
    pub fn new(preset: Preset, model: impl Into<String>, key: impl Into<String>) -> Self {
        let url = preset.default_url().unwrap_or("").to_string();
        Self { url, model: model.into(), key: key.into(), preset, stt: None, tts: None }
    }

    /// `~/.config/metalvj/config.toml` — the one lookup, see
    /// [`metal_config::config_dir`]. What [`ProviderConfig::load`] and
    /// [`ProviderConfig::save`] read and mirror.
    pub fn default_config_toml() -> PathBuf {
        metal_config::default_path(&metal_config::config_dir())
    }

    /// Loads the provider: `data/ai.json` at `path`, with `[agent.provider]`
    /// from the default `config.toml` on top. `Ok(None)` when neither has
    /// anything — the UI treats that as "no provider configured", not an
    /// error. See the module doc for which file wins for which field.
    pub fn load(path: impl AsRef<Path>) -> std::io::Result<Option<Self>> {
        Self::load_at(path, &Self::default_config_toml())
    }

    /// [`ProviderConfig::load`] against an explicit `config.toml` path.
    pub fn load_at(
        path: impl AsRef<Path>,
        config_toml: &Path,
    ) -> std::io::Result<Option<Self>> {
        let path = path.as_ref();
        let stored: Option<Self> = if path.exists() {
            let raw = fs::read_to_string(path)?;
            Some(
                serde_json::from_str(&raw)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
            )
        } else {
            None
        };

        let central = metal_config::Config::load(config_toml)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let Some(provider) = central.agent.provider else {
            return Ok(stored);
        };

        // S.AL.2 precedence: `key_file` (read live, never cached) wins over
        // `ai.json`'s own `key` when it's set *and* readable; a `key_file`
        // that's set but broken (missing file, bad permissions) falls back
        // to `ai.json`'s key rather than failing the whole load — `metal-sync
        // --doctor`/`check-config` are where a broken `key_file` gets
        // surfaced, not here. Computed before the moves below, since a
        // partial move out of `provider`'s other fields would make `&provider`
        // unavailable afterwards.
        let key_from_file =
            metal_config::read_key_file(&provider).ok().flatten().filter(|k| !k.is_empty());

        // The table exists: it is the authority for preset/url/model. Start
        // from whatever `ai.json` holds (the key, voice endpoints, and a
        // fallback for any of the three the table leaves unset), or from an
        // empty provider when there is no `ai.json` yet.
        let mut cfg = stored.unwrap_or_else(|| ProviderConfig {
            url: String::new(),
            model: String::new(),
            key: String::new(),
            preset: Preset::Manual,
            stt: None,
            tts: None,
        });
        if let Some(name) = provider.preset {
            cfg.preset = serde_json::from_value(serde_json::Value::String(name.clone()))
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "{}: [agent.provider].preset = {name:?} is not one of \
                             claude/openai/deepinfra/manual",
                            config_toml.display()
                        ),
                    )
                })?;
        }
        if let Some(url) = provider.url {
            cfg.url = url;
        } else if cfg.url.is_empty() {
            // Preset named, url not: the preset's own endpoint, exactly what
            // `ProviderConfig::new` would have prefilled.
            cfg.url = cfg.preset.default_url().unwrap_or("").to_string();
        }
        if let Some(model) = provider.model {
            cfg.model = model;
        }
        if let Some(key) = key_from_file {
            cfg.key = key;
        }
        Ok(Some(cfg))
    }

    /// Writes `data/ai.json` at `path` — the one and only place the raw key
    /// is allowed to land on disk *while `key_file` is not yet in effect* —
    /// and mirrors `preset`/`url`/`model` into the default `config.toml`'s
    /// `[agent.provider]` when that file exists.
    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        self.save_at(path, &Self::default_config_toml())
    }

    /// [`ProviderConfig::save`] against an explicit `config.toml` path.
    ///
    /// Nothing else in `metal-agent` serializes this struct to a file. The
    /// mirror into `config.toml` carries no key: `metal_config::Provider` has
    /// no field for one, so there is no way to write it there by accident.
    ///
    /// S.AL.2: when `config_toml`'s `[agent.provider].key_file` is set *and*
    /// currently resolves to a real key (checked live, never cached — a
    /// `key_file` that's set but broken is not "in effect", so `ai.json`
    /// keeps being written as the fallback the load-side precedence needs),
    /// the copy of `key` written to `ai.json` is cleared instead of mirroring
    /// `self.key`. Otherwise this is a no-op change: the key is written
    /// exactly as before. This is what stops the plaintext key from
    /// reappearing in a file that might get synced once it has been migrated
    /// out to `secrets/ai.env`.
    pub fn save_at(&self, path: impl AsRef<Path>, config_toml: &Path) -> std::io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let key_file_in_effect = config_toml.exists()
            && metal_config::Config::load(config_toml)
                .ok()
                .and_then(|c| c.agent.provider)
                .and_then(|p| metal_config::read_key_file(&p).ok())
                .flatten()
                .is_some_and(|k| !k.is_empty());

        let body = if key_file_in_effect {
            let mut redacted = self.clone();
            redacted.key = String::new();
            serde_json::to_string_pretty(&redacted)
        } else {
            serde_json::to_string_pretty(self)
        }
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(path, body)?;

        // Mirror into an existing config.toml only — creating one is
        // migration's job (`metal-sync --doctor`), and until then `ai.json`
        // alone governs, so a later `load` still sees exactly what was saved.
        if !config_toml.exists() {
            return Ok(());
        }
        let invalid = |e: String| std::io::Error::new(std::io::ErrorKind::InvalidData, e);
        let mut central = metal_config::Config::load(config_toml).map_err(invalid)?;
        let mut provider = central.agent.provider.take().unwrap_or_default();
        provider.preset = Some(
            serde_json::to_value(self.preset)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "manual".to_string()),
        );
        provider.url = Some(self.url.clone());
        provider.model = Some(self.model.clone());
        central.agent.provider = Some(provider);
        central.save(config_toml).map_err(invalid)
    }

    /// The masked form the UI shows — same mask as `Debug`, exposed
    /// explicitly so the UI layer doesn't have to parse `{:?}` output to get
    /// it.
    pub fn masked_key(&self) -> String {
        mask_key(&self.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `config.toml` path that does not exist: every test below that
    /// exercises the `ai.json` half alone goes through `load_at`/`save_at`
    /// with this, never through `load`/`save`, which would read (and on save,
    /// write) the developer's real `~/.config/metalvj/config.toml`.
    fn no_config(dir: &Path) -> PathBuf {
        dir.join("absent-config.toml")
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("metal-agent-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ai.json");
        let cfg_toml = no_config(&dir);

        let cfg = ProviderConfig::new(Preset::Claude, "claude-sonnet-5", "sk-abcd1234wxyz");
        cfg.save_at(&path, &cfg_toml).unwrap();
        assert!(!cfg_toml.exists(), "save never creates config.toml — migration does");

        let loaded = ProviderConfig::load_at(&path, &cfg_toml).unwrap().expect("file exists");
        assert_eq!(loaded.url, cfg.url);
        assert_eq!(loaded.model, cfg.model);
        assert_eq!(loaded.key, cfg.key);
        assert_eq!(loaded.preset, cfg.preset);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_loads_as_none() {
        let path = std::env::temp_dir().join("metal-agent-test-does-not-exist.json");
        std::fs::remove_file(&path).ok();
        let cfg_toml = no_config(&std::env::temp_dir());
        assert!(ProviderConfig::load_at(&path, &cfg_toml).unwrap().is_none());
    }

    #[test]
    fn debug_never_prints_the_raw_key() {
        let cfg = ProviderConfig::new(Preset::Openai, "gpt-5", "sk-supersecretvalue9999");
        let printed = format!("{cfg:?}");
        assert!(!printed.contains("sk-supersecretvalue9999"));
        assert!(printed.contains("9999"), "masked form should keep the last 4 chars");
        assert!(printed.contains("***"));
    }

    #[test]
    fn masked_key_matches_ui_expectation() {
        let cfg = ProviderConfig::new(Preset::Deepinfra, "some-model", "abcd");
        // A 4-char key is entirely a "tail" — masking still hides it fully.
        assert_eq!(cfg.masked_key(), "***");

        let cfg = ProviderConfig::new(Preset::Deepinfra, "some-model", "0123456789");
        assert_eq!(cfg.masked_key(), "***6789");
    }

    #[test]
    fn presets_prefill_url_and_manual_does_not() {
        assert_eq!(
            Preset::Claude.default_url(),
            Some("https://api.anthropic.com/v1/messages")
        );
        assert_eq!(
            Preset::Openai.default_url(),
            Some("https://api.openai.com/v1/chat/completions")
        );
        assert_eq!(
            Preset::Deepinfra.default_url(),
            Some("https://api.deepinfra.com/v1/openai/chat/completions")
        );
        assert_eq!(Preset::Manual.default_url(), None);

        let manual = ProviderConfig::new(Preset::Manual, "whatever", "key");
        assert_eq!(manual.url, "");

        let claude = ProviderConfig::new(Preset::Claude, "claude-sonnet-5", "key");
        assert_eq!(claude.url, "https://api.anthropic.com/v1/messages");
    }

    // --- H.6: voz -----------------------------------------------------

    #[test]
    fn a_provider_config_with_no_stt_or_tts_still_parses() {
        // Pre-H.6 files on disk have no `stt`/`tts` keys at all — `#[serde
        // (default)]` has to make that parse as `None`, not a hard error.
        let raw = r#"{"url":"https://api.openai.com/v1/chat/completions","model":"gpt-5","key":"sk-old","preset":"openai"}"#;
        let cfg: ProviderConfig = serde_json::from_str(raw).unwrap();
        assert!(cfg.stt.is_none());
        assert!(cfg.tts.is_none());
    }

    #[test]
    fn stt_and_tts_round_trip_as_independent_endpoints() {
        let dir = std::env::temp_dir().join(format!("metal-agent-voice-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ai.json");

        let cfg_toml = no_config(&dir);

        let mut cfg = ProviderConfig::new(Preset::Claude, "claude-sonnet-5", "sk-text-key");
        cfg.stt = Some(VoiceConfig {
            url: "https://api.openai.com/v1/audio/transcriptions".to_string(),
            model: "whisper-1".to_string(),
            key: "sk-stt-key".to_string(),
        });
        cfg.tts = Some(VoiceConfig {
            url: "https://api.deepinfra.com/v1/openai/audio/speech".to_string(),
            model: "some-tts-model".to_string(),
            key: "sk-tts-key".to_string(),
        });
        cfg.save_at(&path, &cfg_toml).unwrap();

        let loaded = ProviderConfig::load_at(&path, &cfg_toml).unwrap().expect("file exists");
        let stt = loaded.stt.expect("stt survives the round trip");
        let tts = loaded.tts.expect("tts survives the round trip");
        assert_eq!(stt.url, "https://api.openai.com/v1/audio/transcriptions");
        assert_eq!(stt.model, "whisper-1");
        assert_eq!(stt.key, "sk-stt-key");
        assert_eq!(tts.model, "some-tts-model");
        // Voice endpoints are independent of the text provider's url/key.
        assert_ne!(stt.url, loaded.url);
        assert_ne!(tts.key, loaded.key);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn voice_config_debug_never_prints_the_raw_key() {
        let mut cfg = ProviderConfig::new(Preset::Claude, "claude-sonnet-5", "sk-text-secret1234");
        cfg.stt = Some(VoiceConfig {
            url: "https://api.openai.com/v1/audio/transcriptions".to_string(),
            model: "whisper-1".to_string(),
            key: "sk-stt-supersecret9999".to_string(),
        });
        let printed = format!("{cfg:?}");
        assert!(!printed.contains("sk-text-secret1234"));
        assert!(!printed.contains("sk-stt-supersecret9999"));
        assert!(printed.contains("9999"));
    }

    // --- S.AH: `[agent.provider]` in config.toml is the authority ------------

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("metal-agent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn central_with(preset: Option<&str>, url: Option<&str>, model: Option<&str>) -> metal_config::Config {
        let mut c = metal_config::Config::default();
        c.agent.provider = Some(metal_config::Provider {
            preset: preset.map(str::to_string),
            url: url.map(str::to_string),
            model: model.map(str::to_string),
            key_file: Some("./secrets/ai.env".to_string()),
        });
        c
    }

    /// The core of the rewiring: with `[agent.provider]` present, what
    /// `ai.json` says about preset/url/model is not consulted; the key and
    /// the voice endpoints still come from `ai.json` and only from there.
    #[test]
    fn config_toml_wins_for_preset_url_model_and_ai_json_keeps_the_key() {
        let dir = scratch("central-wins");
        let ai = dir.join("ai.json");
        let cfg_toml = dir.join("config.toml");

        let mut on_disk = ProviderConfig::new(Preset::Claude, "claude-sonnet-5", "sk-the-real-key");
        on_disk.stt = Some(VoiceConfig {
            url: "https://stt.example/v1".to_string(),
            model: "whisper-1".to_string(),
            key: "sk-stt".to_string(),
        });
        on_disk.save_at(&ai, &no_config(&dir)).unwrap();
        central_with(Some("openai"), Some("https://api.openai.com/v1/chat/completions"), Some("gpt-5"))
            .save(&cfg_toml)
            .unwrap();

        let loaded = ProviderConfig::load_at(&ai, &cfg_toml).unwrap().unwrap();
        assert_eq!(loaded.preset, Preset::Openai);
        assert_eq!(loaded.url, "https://api.openai.com/v1/chat/completions");
        assert_eq!(loaded.model, "gpt-5");
        assert_eq!(loaded.key, "sk-the-real-key", "the key only ever comes from ai.json");
        assert_eq!(loaded.stt.unwrap().key, "sk-stt");

        // And config.toml still holds no key, in any form.
        let text = std::fs::read_to_string(&cfg_toml).unwrap();
        assert!(!text.contains("sk-the-real-key"));
        assert!(!text.contains("sk-stt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A table naming only a preset resolves the url the way
    /// `ProviderConfig::new` would; `ai.json`'s url is not what governs.
    #[test]
    fn a_preset_without_a_url_in_config_toml_takes_the_presets_own_endpoint() {
        let dir = scratch("preset-only");
        let ai = dir.join("ai.json");
        let cfg_toml = dir.join("config.toml");
        ProviderConfig::new(Preset::Manual, "m", "sk-k").save_at(&ai, &no_config(&dir)).unwrap();
        central_with(Some("deepinfra"), None, None).save(&cfg_toml).unwrap();

        let loaded = ProviderConfig::load_at(&ai, &cfg_toml).unwrap().unwrap();
        assert_eq!(loaded.preset, Preset::Deepinfra);
        assert_eq!(loaded.model, "m", "model falls back to ai.json when the table leaves it unset");
        // `ai.json` had a manual (empty) url; nothing to fall back to, so
        // the preset's endpoint fills it.
        assert_eq!(loaded.url, Preset::Deepinfra.default_url().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `config.toml` alone (no `ai.json` yet) is a configured provider with
    /// no key — visible as such, rather than invisible.
    #[test]
    fn config_toml_without_ai_json_loads_as_a_keyless_provider() {
        let dir = scratch("no-ai-json");
        let cfg_toml = dir.join("config.toml");
        central_with(Some("claude"), None, Some("claude-sonnet-5")).save(&cfg_toml).unwrap();

        let loaded = ProviderConfig::load_at(dir.join("ai.json"), &cfg_toml).unwrap().unwrap();
        assert_eq!(loaded.preset, Preset::Claude);
        assert_eq!(loaded.model, "claude-sonnet-5");
        assert!(loaded.key.is_empty());
        assert_eq!(loaded.masked_key(), "***");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Saving through the UI route keeps the two files agreeing: the key
    /// lands in `ai.json`, the three public fields land in `config.toml`,
    /// and `key_file` survives the write untouched. This fixture's
    /// `key_file` points nowhere real, so it is not "in effect" (S.AL.2) —
    /// `ai.json` still gets the key, same as before that precedence existed.
    /// See the `key_file`-specific tests below for the in-effect case.
    #[test]
    fn save_mirrors_public_fields_into_an_existing_config_toml_and_never_the_key() {
        let dir = scratch("save-mirror");
        let ai = dir.join("ai.json");
        let cfg_toml = dir.join("config.toml");
        central_with(Some("claude"), None, None).save(&cfg_toml).unwrap();

        ProviderConfig::new(Preset::Openai, "gpt-5-mini", "sk-secret-value-4242")
            .save_at(&ai, &cfg_toml)
            .unwrap();

        let central = metal_config::Config::load(&cfg_toml).unwrap();
        let p = central.agent.provider.unwrap();
        assert_eq!(p.preset.as_deref(), Some("openai"));
        assert_eq!(p.url.as_deref(), Some("https://api.openai.com/v1/chat/completions"));
        assert_eq!(p.model.as_deref(), Some("gpt-5-mini"));
        assert_eq!(p.key_file.as_deref(), Some("./secrets/ai.env"), "untouched");
        assert!(!std::fs::read_to_string(&cfg_toml).unwrap().contains("sk-secret-value-4242"));

        // A round trip through the two files comes back whole.
        let loaded = ProviderConfig::load_at(&ai, &cfg_toml).unwrap().unwrap();
        assert_eq!(loaded.preset, Preset::Openai);
        assert_eq!(loaded.model, "gpt-5-mini");
        assert_eq!(loaded.key, "sk-secret-value-4242");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Once the table governs, an `ai.json` trimmed by hand to just the key
    /// is a legal file — the three fields it dropped are the ones it no
    /// longer owns.
    #[test]
    fn an_ai_json_holding_only_the_key_parses_once_config_toml_governs() {
        let dir = scratch("key-only");
        let ai = dir.join("ai.json");
        let cfg_toml = dir.join("config.toml");
        std::fs::write(&ai, r#"{"key":"sk-only"}"#).unwrap();
        central_with(Some("claude"), None, Some("claude-sonnet-5")).save(&cfg_toml).unwrap();

        let loaded = ProviderConfig::load_at(&ai, &cfg_toml).unwrap().unwrap();
        assert_eq!(loaded.key, "sk-only");
        assert_eq!(loaded.preset, Preset::Claude);
        assert_eq!(loaded.url, Preset::Claude.default_url().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Hand-editing a preset name wrong is an error that names the file,
    /// not a silent fallback to whatever `ai.json` had.
    #[test]
    fn an_unknown_preset_in_config_toml_is_an_error_not_a_guess() {
        let dir = scratch("bad-preset");
        let cfg_toml = dir.join("config.toml");
        central_with(Some("gemini"), None, None).save(&cfg_toml).unwrap();
        let err = ProviderConfig::load_at(dir.join("ai.json"), &cfg_toml).unwrap_err();
        assert!(err.to_string().contains("gemini"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- S.AL.2: key_file precedence and save's mirror rule ------------------

    fn central_with_key_file(key_file: &str) -> metal_config::Config {
        let mut c = metal_config::Config::default();
        c.agent.provider = Some(metal_config::Provider {
            preset: Some("claude".to_string()),
            url: None,
            model: Some("claude-sonnet-5".to_string()),
            key_file: Some(key_file.to_string()),
        });
        c
    }

    #[test]
    fn key_file_wins_over_ai_jsons_key_when_it_resolves() {
        let dir = scratch("key-file-wins");
        let ai = dir.join("ai.json");
        let cfg_toml = dir.join("config.toml");
        let secret = dir.join("ai.env");
        std::fs::write(&secret, "ANTHROPIC_API_KEY=sk-from-key-file-fake\n").unwrap();

        std::fs::write(&ai, r#"{"key":"sk-from-ai-json-fake"}"#).unwrap();
        central_with_key_file(&secret.to_string_lossy()).save(&cfg_toml).unwrap();

        let loaded = ProviderConfig::load_at(&ai, &cfg_toml).unwrap().unwrap();
        assert_eq!(loaded.key, "sk-from-key-file-fake");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `key_file` set but pointing nowhere real: the provider still loads,
    /// falling back to whatever `ai.json` has, rather than erroring or
    /// silently going keyless.
    #[test]
    fn a_broken_key_file_falls_back_to_ai_jsons_key() {
        let dir = scratch("key-file-broken");
        let ai = dir.join("ai.json");
        let cfg_toml = dir.join("config.toml");
        let missing = dir.join("does-not-exist.env");

        std::fs::write(&ai, r#"{"key":"sk-from-ai-json-fake"}"#).unwrap();
        central_with_key_file(&missing.to_string_lossy()).save(&cfg_toml).unwrap();

        let loaded = ProviderConfig::load_at(&ai, &cfg_toml).unwrap().unwrap();
        assert_eq!(loaded.key, "sk-from-ai-json-fake");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Once `key_file` actually resolves to a key, `save` must stop writing
    /// the plaintext key into `ai.json` — that mirror is exactly what
    /// migrating to `secrets/ai.env` was meant to end. The key is not lost:
    /// it is simply not this crate's job to write it into `key_file` too
    /// (nothing here does), so `ai.json`'s copy is cleared, not replaced.
    #[test]
    fn save_does_not_mirror_the_key_when_key_file_is_in_effect() {
        let dir = scratch("save-no-mirror");
        let ai = dir.join("ai.json");
        let cfg_toml = dir.join("config.toml");
        let secret = dir.join("ai.env");
        std::fs::write(&secret, "ANTHROPIC_API_KEY=sk-from-key-file-fake\n").unwrap();
        central_with_key_file(&secret.to_string_lossy()).save(&cfg_toml).unwrap();

        ProviderConfig::new(Preset::Claude, "claude-sonnet-5", "sk-should-not-land-in-ai-json")
            .save_at(&ai, &cfg_toml)
            .unwrap();

        let raw = std::fs::read_to_string(&ai).unwrap();
        assert!(!raw.contains("sk-should-not-land-in-ai-json"), "ai.json: {raw}");
        let on_disk: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(on_disk.get("key").and_then(|v| v.as_str()), Some(""));

        // The provider still resolves correctly on the next load — the key
        // comes from key_file, not from the now-empty ai.json field.
        let loaded = ProviderConfig::load_at(&ai, &cfg_toml).unwrap().unwrap();
        assert_eq!(loaded.key, "sk-from-key-file-fake");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The inverse of the test above: with no working `key_file`, `save`
    /// keeps writing the real key into `ai.json` exactly as it always has —
    /// this is the only place the key is allowed to persist to when nothing
    /// else can serve it back.
    #[test]
    fn save_still_mirrors_the_key_when_key_file_is_broken() {
        let dir = scratch("save-mirror-fallback");
        let ai = dir.join("ai.json");
        let cfg_toml = dir.join("config.toml");
        let missing = dir.join("does-not-exist.env");
        central_with_key_file(&missing.to_string_lossy()).save(&cfg_toml).unwrap();

        ProviderConfig::new(Preset::Claude, "claude-sonnet-5", "sk-mirrors-as-usual")
            .save_at(&ai, &cfg_toml)
            .unwrap();

        let raw = std::fs::read_to_string(&ai).unwrap();
        assert!(raw.contains("sk-mirrors-as-usual"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
