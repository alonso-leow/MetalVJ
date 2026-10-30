//! `~/.config/metalvj/config.toml` — one file, two scopes.
//!
//! `plan-opus.md` § O.26 draws one line through every section of this file:
//! `scope = "device"` (this machine — identity, keys, capacity) never
//! syncs, and `scope = "studio"` (shared work — overlays, skills, roles,
//! palettes) does. That line is why this is one file and not several: a
//! reader that wants "what does the club look like" and a reader that wants
//! "what is this laptop" are looking at the same document, just different
//! tables in it.
//!
//! This crate is deliberately the same shape as `metal-schema`: no dependency
//! beyond `serde`/`toml`/`serde_json` (the last only for writing
//! `data/palettes.json`, which is JSON, not TOML), so it can sit underneath
//! `metal-sync --doctor` and `scripts/check-config` without dragging a runtime
//! or a GPU stack along. See `Cargo.toml` for why that restriction is
//! load-bearing, not just tidy.
//!
//! # Missing is empty, corrupt is an error
//!
//! Same rule as `metal-sync/src/config.rs`'s `sync.toml` reader, which this
//! crate is meant to grow into replacing: a `config.toml` that has never
//! been written parses as every section defaulted, because a fresh install
//! doing that is normal. A `config.toml` that exists and fails to parse as
//! TOML is a hard error — silently starting over would orphan a paired node
//! or drop a chosen provider without saying so.
//!
//! # Unknown keys are a lint, never an error
//!
//! Same allowlist philosophy as `[sync].allowseed` / `manifest.rs`: a key this crate
//! doesn't recognise (a newer version wrote it, a person hand-edited it) is
//! surfaced by [`validate`] as a [`Problem`], never a parse failure. Refusing
//! to start over a typo would make the file too dangerous to hand-edit,
//! which is the opposite of what a *central* config is for.

mod palettes;
mod validate;

pub use palettes::{materialize_palettes, shift, write_palettes_file, Palette};
pub use validate::{problems, validate, Problem, Severity};

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The whole document. Every field is `#[serde(default)]`: a section absent
/// from disk is a section at its default, not a missing config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_schema")]
    pub schema: String,

    #[serde(default, skip_serializing_if = "is_default")]
    pub meta: Meta,
    #[serde(default, skip_serializing_if = "is_default")]
    pub node: Node,
    #[serde(default, skip_serializing_if = "is_default")]
    pub sync: Sync,
    #[serde(default, skip_serializing_if = "is_default")]
    pub paths: Paths,
    #[serde(default, skip_serializing_if = "is_default")]
    pub agent: Agent,
    #[serde(default, skip_serializing_if = "is_default")]
    pub mcp: Mcp,
    #[serde(default, skip_serializing_if = "is_default")]
    pub render: Render,
    #[serde(default, skip_serializing_if = "is_default")]
    pub farm: Farm,
    #[serde(default, skip_serializing_if = "is_default")]
    pub studio: Studio,
    /// The shell's remembered layout (panel sizes, graph canvas view). Absent
    /// from a config written before it existed, which then loads as "nothing
    /// remembered" — every panel at its theme default.
    #[serde(default, skip_serializing_if = "is_default")]
    pub layout: Layout,

    /// Keys this version of the crate doesn't know about, kept verbatim so a
    /// round trip through an older binary doesn't drop a newer field —
    /// `validate` turns these into lints, never a hard error.
    #[serde(flatten)]
    pub unknown: std::collections::BTreeMap<String, toml::Value>,
}

/// The shell's layout memory, written by the dashboard itself as the operator
/// drags splitters and pans the graph canvas. Every field defaults, so a
/// config with no `[layout]` table is a config with nothing remembered.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Layout {
    /// Panel sizes in points, keyed by the shell's stable panel names
    /// (`bottom-row`, `properties-col`, ...). Width for a left panel, height
    /// for the bottom row — whichever axis the panel is resized along.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub panels: std::collections::BTreeMap<String, f32>,
    /// The graph canvas's pan (screen points) and zoom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas: Option<CanvasView>,
    /// Whether the AI harness (IA) sidebar was open. `None` means never saved,
    /// so the shell's default applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_open: Option<bool>,
    /// Whether the library's left sidebar was open. `None` means never saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_sidebar_open: Option<bool>,
}

/// The graph canvas's view transform: screen = origin + pan + graph * zoom.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct CanvasView {
    #[serde(default)]
    pub pan: [f32; 2],
    #[serde(default = "default_zoom")]
    pub zoom: f32,
}

fn default_zoom() -> f32 {
    1.0
}

fn default_version() -> u32 {
    2
}
fn default_schema() -> String {
    "fw/config/2".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: default_version(),
            schema: default_schema(),
            meta: Meta::default(),
            node: Node::default(),
            sync: Sync::default(),
            paths: Paths::default(),
            agent: Agent::default(),
            mcp: Mcp::default(),
            render: Render::default(),
            farm: Farm::default(),
            studio: Studio::default(),
            layout: Layout::default(),
            unknown: std::collections::BTreeMap::new(),
        }
    }
}
fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// `scope = device`. Never synced.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Meta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub define: Option<String>,
}

/// `scope = device`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Node {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub first_super: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soul: Option<String>,
}

/// `scope = device`. Subsumes `sync.toml` (`doc` is the namespace field
/// `SyncConfig::namespace` already reads) and `allowseed`, the runtime
/// allowlist `metal-sync/src/manifest.rs` reads on every scan — see
/// [`allowseed_default`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Sync {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    #[serde(default)]
    pub peers: Vec<Peer>,
    #[serde(default = "allowseed_default")]
    pub allowseed: Vec<String>,
}

impl Default for Sync {
    fn default() -> Self {
        Sync { doc: None, peers: Vec::new(), allowseed: allowseed_default() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Peer {
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<String>,
}

/// **The** allowlist: what a `config.toml` with no `[sync].allowseed` key
/// syncs. This is the one list — `metal-sync/src/manifest.rs` no longer carries
/// a `SYNCED` const of its own; its `Allowlist::builtin()` is this function,
/// and a running `SyncEngine` re-reads `[sync].allowseed` from `config.toml`
/// on every scan tick (rule 4: read live from the source, never a second
/// cached copy). Keeping the list here rather than in `metal-sync` is what lets
/// `metal-sync --doctor` and `scripts/check-config` report on it without
/// linking the engine.
///
/// The list is the `plan-sonnet.md` § S.AH skeleton plus the two patterns
/// `manifest.rs::SYNCED` had grown since the skeleton was written
/// (`switches.json` from S.AK, `fixtures/*.toml` from S.AC/O.20) — the union
/// of both, so that folding the two lists into one dropped nothing either
/// side was already syncing. A few entries name files whose writers have not
/// landed yet (`render/queue.json`, `reels/*/verified/**`); an allowlist
/// pattern with nothing on disk to match is inert, and listing it now means
/// the feature syncs the day it ships instead of the day someone remembers.
///
/// Same pattern language as `manifest.rs::matches`: `*` is one path
/// segment, `**` is the rest of the path. Every path is relative to the
/// data root.
pub fn allowseed_default() -> Vec<String> {
    [
        // Top-level authored state.
        "outputs.json", "trims.json", "collections.json", "sequences.json",
        "switches.json", "palettes.json",
        // One file per graph / timeline / library page / track.
        "graphs/*.json", "graphs/*.owner", "timelines/*.json", "library/*.json", "tracks/*.json",
        // The global scene favourites and their groups (`metal_reel::scenes`):
        // authored, and what `reel.batch` plays on every peer.
        "scenes/favorites.json",
        // The scene posters the ingest cut for each scene (one JPEG per cut),
        // named data-root relative in the cut list so every peer reads its own copy.
        "projects/hap/*.scenes/*.jpg",
        // The global gallery favourites and recents (`metal-dash` `shell::favorites`):
        // one list for the install, the same on every peer.
        "favorites.json",
        // O.20/S.AC: fixture profiles are authored in the studio and the same
        // head should read the same on every peer.
        "fixtures/*.toml",
        // The suite animation corpus: both halves (`.npy` matrix + `.json`
        // index) are authored, neither is derivable from the other.
        "animations/**",
        // The agent's authored corpus and identity — never `ai.json`, which
        // holds the provider key (see `manifest.rs`'s module doc).
        "agent/shots.json", "agent/session.jsonl", "agent/roles.json", "agent/soul.md",
        // Reel *metadata* only; `reels/*/frames/` is derived and never ships.
        "reels/*/index.json", "reels/*/curator.json", "reels/*/colourist-*.json",
        "reels/*/verified/**",
        "render/queue.json",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// `scope = device`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Paths {
    #[serde(default = "default_data_path")]
    pub data: String,
    #[serde(default = "default_import_path")]
    pub import: String,
}

impl Default for Paths {
    fn default() -> Self {
        Paths { data: default_data_path(), import: default_import_path() }
    }
}
fn default_data_path() -> String {
    "./data".to_string()
}
fn default_import_path() -> String {
    "./import".to_string()
}

/// `scope = device` (the whole `[agent]`/`[agent.provider]` pair — a key is
/// a credential, and two paired machines publishing one would fight over
/// whose wins, the same argument `manifest.rs` makes for `ai.json` never
/// syncing).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Agent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soul: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills_dir: Option<String>,
    #[serde(default)]
    pub skills: Vec<Skill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Skill {
    pub file: String,
    #[serde(default)]
    pub default: bool,
}

/// Mirrors `metal_agent::config::ProviderConfig`'s public fields, minus the raw
/// key: `config.toml` never holds a secret inline, only a pointer to one
/// (`key_file`, per O.26 — "keys live outside"). The actual key stays
/// exactly where it already lived, `data/ai.json`, until a later lot decides
/// to move it under `~/.config/metalvj/secrets/`.
///
/// **This table is the runtime source of truth for `preset`/`url`/`model`**
/// once it is present: `metal_agent::config::ProviderConfig::load` reads it
/// through [`default_path`]`(`[`config_dir`]`())` and lets it override
/// whatever `data/ai.json` says, and `ProviderConfig::save` writes these
/// three fields back here whenever `config.toml` exists. `data/ai.json`
/// keeps only what this table cannot hold — the key, and the optional
/// `stt`/`tts` voice endpoints (each of which carries its own key). See the
/// module doc of `metal-agent/src/config.rs` for the exact precedence.
///
/// This struct has no `key` field, so unlike `metal_agent::config::ProviderConfig`
/// it needs no hand-written `Debug` — there is nothing here to leak. Don't
/// add one without re-reading that file's doc comment on why.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Provider {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The model name sent with each request. Not in the S.AH skeleton (an
    /// omission — the struct was meant to mirror `ProviderConfig` minus the
    /// key, and `model` is not a secret); added when `[agent.provider]`
    /// became the runtime authority, because a person editing `preset`/`url`
    /// by hand here and `model` in `data/ai.json` would be editing one
    /// provider in two files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Path to a file holding the raw key, never the key itself — see
    /// [`read_key_file`] for how it's resolved and read, and
    /// `metal_agent::config::ProviderConfig::load`'s module doc for where this
    /// slots into the preset/url/model/key precedence (S.AL.2). Relative
    /// paths resolve against [`config_dir`] (matching [`secrets_dir`], the
    /// default the one-shot migration writes to); an absolute path is used
    /// as-is, for a person who keeps secrets somewhere else entirely (a
    /// password manager's file mount, say).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_file: Option<String>,
}

/// `scope = device` (`S.S`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Mcp {
    #[serde(default)]
    pub client: Vec<String>,
    #[serde(default)]
    pub serve: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// `scope = device`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Render {
    #[serde(default = "default_engine")]
    pub engine: String,
    #[serde(default = "default_device")]
    pub device: String,
    #[serde(default = "default_compose")]
    pub compose_default: String,
    /// Which tick level this machine runs — `base` (8 Hz) or `fine` (32 Hz).
    ///
    /// A machine capability, not something a node, sequence or file carries
    /// (rule 4): the stored grid is canonical 32 Hz, and a base machine pools
    /// on read. It is `metal_schema::TickRate`, the one ladder type, so the
    /// setting, the digester and the notation cannot drift. Default `base`,
    /// the minimum a fresh install can always afford; moving to `fine` is an
    /// explicit choice.
    #[serde(default)]
    pub tick_level: metal_schema::TickRate,
}
impl Default for Render {
    fn default() -> Self {
        Render {
            engine: default_engine(),
            device: default_device(),
            compose_default: default_compose(),
            tick_level: metal_schema::TickRate::Base,
        }
    }
}
fn default_engine() -> String {
    "gpu".to_string()
}
fn default_device() -> String {
    "default".to_string()
}
fn default_compose() -> String {
    "same".to_string()
}

/// `scope = device` (`O.24`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Farm {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conductor: Option<String>,
}

/// `scope = studio` — a *view* of the files sync already carries, per O.26:
/// "it lists them, it does not define their content; the file is the
/// authority and the row is regenerated from disk (`scripts/reindex.py`),
/// never by hand." Nothing here is authored by hand except
/// `[studio.palettes]` and `[studio.custom_nodes]`, which are genuinely
/// config, not an index.
///
/// **Deviation from the `plan-sonnet.md` § S.AH skeleton, flagged in the
/// commit and the dispatch report**: the skeleton has both a scalar
/// `[studio] palettes = "palettes.json"` *and* a subtable `[studio.palettes]`
/// — the same key twice, which is not valid TOML (`toml`/every parser
/// rejects "Cannot overwrite a value"). This is a typo in the plan, not a
/// design either loader can honour. Resolution taken: drop the scalar
/// `palettes` index field — `data/palettes.json` is the fixed materialize
/// target O.26's prose already names outright ("`[studio.palettes]` is
/// materialized into `data/palettes.json`"), so there was nothing the scalar
/// field would have said that isn't already a constant. `graphs`,
/// `timelines`, `collections`, `sequences`, `roles` are untouched.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Studio {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphs: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timelines: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collections: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequences: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "std::collections::BTreeMap::is_empty",
        rename = "palettes"
    )]
    pub palette_defs: std::collections::BTreeMap<String, PaletteDef>,
    #[serde(
        default,
        skip_serializing_if = "std::collections::BTreeMap::is_empty",
        rename = "custom_nodes"
    )]
    pub custom_nodes: std::collections::BTreeMap<String, toml::Value>,
}

/// Fixed materialize target for `[studio.palettes]` (S.AH point 3). Not a
/// config field — see the deviation note on [`Studio`] for why the skeleton's
/// scalar `palettes` key was dropped rather than kept alongside the subtable.
pub const PALETTES_FILE: &str = "palettes.json";

/// One named palette as authored in `[studio.palettes]`: the behavioural
/// slots (`base`, `acento`, `fondo`, `destello`), plus `extra` for anything
/// past those four. [`materialize_palettes`] turns this into the ordered
/// `c_0..c_n` list `data/palettes.json` stores.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PaletteDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, alias = "acento", skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(default, alias = "fondo", skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, alias = "destello", skip_serializing_if = "Option::is_none")]
    pub flash: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<String>,
}

impl Config {
    /// Missing file = every section at its default, never an error — same
    /// rule `metal-sync/src/config.rs::load` already enforces for `sync.toml`.
    /// A file that exists and fails to parse as TOML *is* an error: this
    /// document is about to be treated as authoritative for a namespace and
    /// a provider, so silently starting over would be the same class of bug
    /// `a_corrupt_config_is_an_error_rather_than_a_silent_new_document`
    /// guards against there.
    pub fn load(path: &Path) -> Result<Config, String> {
        match metal_vfs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| format!("{} did not parse: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            metal_vfs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        metal_vfs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }
}

/// `<config_dir>/config.toml`. Callers that already know their config
/// directory (`metal-sync --config`, `--doctor`, the tests) hand it in; the
/// rest resolve it with [`config_dir`].
pub fn default_path(config_dir: &Path) -> PathBuf {
    config_dir.join("config.toml")
}

/// `$XDG_CONFIG_HOME/metalvj`, else `~/.config/metalvj` (see [`app_dir_in`] for the
/// pre-rename fallback).
///
/// **The one lookup.** This used to live in `metal_sync::identity::config_dir`
/// with a byte-for-byte copy in `check-config.rs`, and `metal-agent` had none —
/// which is why S.AH left `data/ai.json` unwired: "the data root has no fixed
/// relationship with `~/.config/metalvj/`". The relationship settled on is
/// that there *isn't* a path relationship, and there doesn't need to be one:
/// the data root and the config dir are resolved independently (data root by
/// `--data`, config dir by this function), exactly as `metal-sync --doctor`
/// already takes them as two separate arguments and as `config.toml`'s own
/// `[paths].data` already implies (the config knows where data is, never the
/// reverse). Anything that wants `config.toml` calls this; anything that wants
/// to relocate it for a test or a second profile sets `XDG_CONFIG_HOME`, the
/// override every reader in the workspace already honours. No new flag, no new
/// env var.
///
/// `metal_sync::identity::config_dir` and `check-config.rs` now delegate here
/// (rule 4: one source).
pub fn config_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return app_dir_in(Path::new(&x));
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    app_dir_in(&PathBuf::from(home).join(".config"))
}

/// `~/.cache/metalvj` (the digest venv, the vision models, the song data),
/// falling back to `~/.cache/furtherwave` through [`app_dir_in`].
pub fn cache_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    app_dir_in(&PathBuf::from(home).join(".cache"))
}

/// The app's directory name under a base directory (`~/.config`, `~/.cache`).
pub const APP_DIR: &str = "metalvj";

/// The directory name before the rename. Read only as a fallback.
const LEGACY_APP_DIR: &str = "furtherwave";

/// `base/metalvj`, unless it is missing and `base/furtherwave` exists: then the
/// pre-rename directory, so an install made before the MetalVJ rename keeps its
/// config, token, keys and caches. Nothing is moved or copied. Every reader of
/// `~/.config` or `~/.cache` goes through here, so this is the one fallback.
pub fn app_dir_in(base: &Path) -> PathBuf {
    let new = base.join(APP_DIR);
    let old = base.join(LEGACY_APP_DIR);
    if !new.exists() && old.exists() {
        old
    } else {
        new
    }
}

/// `<config_dir>/secrets` — where a `key_file` a relative path points into
/// by default, and where [`migrate_key_to_file`] writes `ai.env` the one
/// time it moves a key out of `data/ai.json` (S.AL.2, `plan-opus.md` § O.30:
/// "keys never live inline in config.toml; once migrated to
/// `secrets/ai.env` (mode 0600)…"). Not created by this function — callers
/// that need it to exist (`migrate_key_to_file`) create it themselves.
pub fn secrets_dir() -> PathBuf {
    config_dir().join("secrets")
}

/// Resolves a `key_file` value the way [`Provider::key_file`] documents:
/// absolute as-is, relative against [`config_dir`]. `pub(crate)` so
/// `validate.rs`'s key-file checks resolve the exact same path
/// `read_key_file` reads, rather than recomputing it a second way.
pub(crate) fn resolve_key_file_path(key_file: &str) -> PathBuf {
    let p = Path::new(key_file);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        config_dir().join(p)
    }
}

/// One line of `ANTHROPIC_API_KEY=...`, or a bare key with nothing else on
/// the line — either is legal. Whitespace and trailing newlines around the
/// whole thing, and around the value half of a `KEY=value` line, are
/// trimmed. A line "looks like" `KEY=value` when it contains `=` and
/// everything before the first `=` is non-empty and all
/// uppercase/underscore; anything else is treated as a bare key, `=`
/// included (a raw key that happens to contain `=` is vanishingly unlikely
/// but should still round-trip rather than being mis-split).
fn parse_key_file_contents(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some((name, value)) = trimmed.split_once('=') {
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
            return value.trim().to_string();
        }
    }
    trimmed.to_string()
}

/// Reads the key `provider.key_file` points at, live, every call (rule 4 —
/// nothing here is cached). `Ok(None)` means there is no `key_file` to read,
/// which is normal and not a problem; `Err` means one is set but couldn't be
/// read (missing file, permission denied, …) — `metal-sync --doctor` and
/// `check-config` surface that as a warning (see `validate::problems`),
/// callers that just want the key for a request ([`ProviderConfig`] in
/// `metal-agent`) treat it the same as "not set" and fall back to `ai.json`.
///
/// The returned value is the raw key. It must never be wrapped in something
/// that reaches a `Debug`/`Display` impl — this function's own `Err`
/// carries only the file path and the OS error, never file contents.
pub fn read_key_file(provider: &Provider) -> Result<Option<String>, String> {
    let Some(key_file) = provider.key_file.as_deref() else {
        return Ok(None);
    };
    let path = resolve_key_file_path(key_file);
    let raw = metal_vfs::read_to_string(&path)
        .map_err(|e| format!("cannot read key_file {}: {e}", path.display()))?;
    Ok(Some(parse_key_file_contents(&raw)))
}

/// **Migration (S.AH point 7).** If `config.toml` doesn't exist yet but a
/// legacy file does, build a config from it and say so — but never delete
/// the legacy file in this lot; a config.toml that turned out wrong should
/// not have also destroyed the only other copy of the truth.
///
/// Callers pass in whatever they already parsed from the legacy file (this
/// crate does not know `sync.toml`'s or `ai.json`'s shapes — that would be
/// exactly the dependency inversion `metal-schema`'s doc comment warns against);
/// `metal-sync`/`metal-agent` are the ones who read this crate and reach across.
#[derive(Debug, Clone, Default)]
pub struct LegacySync {
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct LegacyProvider {
    pub preset: Option<String>,
    pub url: Option<String>,
    pub model: Option<String>,
}

pub struct Migrated {
    pub config: Config,
    /// Human-readable lines describing what got carried over — `--doctor`
    /// and the CLI print these, migration is never silent.
    pub notes: Vec<String>,
}

/// Runs once: only when `config.toml` is absent. If it already exists, this
/// returns `None` untouched rather than overwriting a file a person may have
/// hand-edited since.
pub fn migrate(
    config_path: &Path,
    legacy_sync: Option<LegacySync>,
    legacy_provider: Option<LegacyProvider>,
) -> Result<Option<Migrated>, String> {
    if metal_vfs::exists(&config_path) {
        return Ok(None);
    }
    if legacy_sync.is_none() && legacy_provider.is_none() {
        return Ok(None);
    }

    let mut cfg = Config::default();
    let mut notes = Vec::new();

    if let Some(sync) = legacy_sync {
        if sync.namespace.is_some() {
            notes.push(format!(
                "carried sync.toml's namespace into [sync].doc ({})",
                config_path.display()
            ));
        }
        cfg.sync.doc = sync.namespace;
    }
    if let Some(provider) = legacy_provider {
        notes.push(
            "carried ai.json's preset/url/model into [agent.provider] — from here on \
             config.toml governs those three; the key itself stays in data/ai.json, \
             untouched, migration never moves key material"
                .to_string(),
        );
        cfg.agent.provider = Some(Provider {
            preset: provider.preset,
            url: provider.url,
            model: provider.model,
            key_file: None,
        });
    }

    cfg.save(config_path)?;
    notes.push(format!("wrote {} — the old files were not deleted", config_path.display()));
    Ok(Some(Migrated { config: cfg, notes }))
}

/// **Key migration (S.AL.2).** Unlike [`migrate`] above, which only fires
/// while `config.toml` is still absent, this runs on every call — the
/// common case on a real machine is one that already has a `config.toml`
/// from an earlier S.AH migration and simply never had `key_file` set, so
/// gating on "config.toml absent" would mean this never fires at all.
/// Idempotency instead comes from checking `[agent.provider].key_file`
/// itself: once it's set (by this function, or by a person who pointed it
/// somewhere on their own), this is a no-op no matter what `raw_key` says —
/// a key rotated by hand afterwards is [`read_key_file`]'s job to notice on
/// the next read, not this function's to re-migrate.
///
/// `raw_key` is whatever the caller already read out of `data/ai.json`'s
/// `key` field — this crate does not parse that file (same
/// dependency-direction rule [`LegacyProvider`] follows: `metal-sync` reaches
/// across, not the other way). `None` or an empty/whitespace-only value is
/// "nothing to migrate" and returns `Ok(None)`.
///
/// On success, writes `secrets/ai.env` (relative to `config_path`'s own
/// directory — the same directory [`secrets_dir`] names when `config_path`
/// is the default one) as `ANTHROPIC_API_KEY=<key>`, mode `0600` on unix,
/// sets `[agent.provider].key_file = "secrets/ai.env"`, and returns
/// `Ok(Some(()))` so the caller knows to go clear the sidecar's own copy —
/// this function never touches `data/ai.json` itself, for the same
/// dependency-direction reason it doesn't parse it.
pub fn migrate_key_to_file(config_path: &Path, raw_key: Option<&str>) -> Result<Option<()>, String> {
    let Some(key) = raw_key.map(str::trim).filter(|k| !k.is_empty()) else {
        return Ok(None);
    };

    let mut cfg = Config::load(config_path)?;
    let mut provider = cfg.agent.provider.clone().unwrap_or_default();
    if provider.key_file.is_some() {
        return Ok(None);
    }

    let secrets = config_path.parent().unwrap_or_else(|| Path::new(".")).join("secrets");
    metal_vfs::create_dir_all(&secrets)
        .map_err(|e| format!("cannot create {}: {e}", secrets.display()))?;
    let secret_path = secrets.join("ai.env");
    metal_vfs::write(&secret_path, format!("ANTHROPIC_API_KEY={key}\n"))
        .map_err(|e| format!("cannot write {}: {e}", secret_path.display()))?;
    restrict_to_owner(&secret_path)?;

    provider.key_file = Some("secrets/ai.env".to_string());
    cfg.agent.provider = Some(provider);
    cfg.save(config_path)?;
    Ok(Some(()))
}

/// A key file is a credential — a default-umask file in a config directory
/// is world-readable on plenty of systems, same reasoning
/// `metal_sync::identity::save` already documents for the node's private key.
#[cfg(unix)]
fn restrict_to_owner(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    metal_vfs::set_permissions(path, metal_vfs::Permissions::from_mode(0o600))
        .map_err(|e| format!("cannot restrict {}: {e}", path.display()))
}
#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_def_loads_the_old_spanish_slot_keys() {
        let old: PaletteDef =
            serde_json::from_str(r##"{"base":"#000000","acento":"#ff5c8a","fondo":"#05070d","destello":"#ffe08a"}"##)
                .expect("a palette written with the Spanish slot keys loads");
        assert_eq!(old.base.as_deref(), Some("#000000"));
        assert_eq!(old.accent.as_deref(), Some("#ff5c8a"));
        assert_eq!(old.background.as_deref(), Some("#05070d"));
        assert_eq!(old.flash.as_deref(), Some("#ffe08a"));
        let out = serde_json::to_string(&old).unwrap();
        assert!(out.contains("\"accent\"") && !out.contains("acento"));
    }

    #[test]
    fn layout_round_trips_panel_sizes_and_canvas_view() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let mut cfg = Config::default();
        cfg.layout.panels.insert("bottom-row".to_string(), 412.5);
        cfg.layout.panels.insert("library-dirs".to_string(), 180.0);
        cfg.layout.canvas = Some(CanvasView { pan: [-64.0, 32.5], zoom: 1.75 });
        cfg.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.layout, cfg.layout);
        assert_eq!(loaded.layout.panels["library-dirs"], 180.0);
        assert_eq!(loaded.layout.canvas.unwrap().zoom, 1.75);
    }

    #[test]
    fn an_old_config_without_layout_loads_with_nothing_remembered() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        metal_vfs::write(&path, "version = 2\nschema = \"fw/config/2\"\n[render]\ntick_level = \"fine\"\n").unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.layout, Layout::default());
        assert!(cfg.layout.canvas.is_none());
        // And an empty layout is not written back, so the file stays as it was.
        cfg.save(&path).unwrap();
        assert!(!metal_vfs::read_to_string(&path).unwrap().contains("layout"));
    }

    #[test]
    fn a_partial_canvas_table_defaults_its_zoom() {
        let cfg: Config = toml::from_str("[layout.canvas]\npan = [10.0, 20.0]\n").unwrap();
        let canvas = cfg.layout.canvas.unwrap();
        assert_eq!(canvas.pan, [10.0, 20.0]);
        assert_eq!(canvas.zoom, 1.0);
    }

    #[test]
    fn tick_level_defaults_to_base_and_round_trips_as_a_name() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        // A file with no `tick_level` reads as base — the minimum.
        metal_vfs::write(&path, "version = 2\nschema = \"fw/config/2\"\n").unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.render.tick_level, metal_schema::TickRate::Base);

        // The machine can move to fine, and it survives a save/load as `fine`.
        let mut cfg = cfg;
        cfg.render.tick_level = metal_schema::TickRate::Fine;
        cfg.save(&path).unwrap();
        assert!(metal_vfs::read_to_string(&path).unwrap().contains("tick_level = \"fine\""));
        assert_eq!(Config::load(&path).unwrap().render.tick_level, metal_schema::TickRate::Fine);
    }

    #[test]
    fn missing_sections_load_as_defaults() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfg = Config::load(&dir.path().join("absent.toml")).unwrap();
        assert_eq!(cfg.version, 2);
        assert_eq!(cfg.render.engine, "gpu");
        assert_eq!(cfg.paths.data, "./data");
        assert!(cfg.sync.doc.is_none());
        assert!(cfg.studio.palette_defs.is_empty());
        // The allowlist default still loads even with no file on disk — a
        // doctor run on a fresh install should see the same list the
        // skeleton documents, not an empty one.
        assert!(cfg.sync.allowseed.contains(&"palettes.json".to_string()));
    }

    #[test]
    fn a_corrupt_config_is_an_error_rather_than_a_silent_new_document() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        metal_vfs::write(&path, "this is not toml = = =").unwrap();
        assert!(Config::load(&path).is_err());
    }

    #[test]
    fn round_trip_preserves_every_scope() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        let mut cfg = Config::default();
        cfg.meta.name = Some("workstation".to_string());
        cfg.node.hostname = Some("workstation".to_string());
        cfg.sync.doc = Some("abc123".to_string());
        cfg.agent.provider = Some(Provider {
            preset: Some("openai".to_string()),
            url: Some("https://api.openai.com/v1".to_string()),
            model: Some("gpt-5".to_string()),
            key_file: Some("./secrets/ai.env".to_string()),
        });
        cfg.studio.palette_defs.insert(
            "night".to_string(),
            PaletteDef {
                base: Some("#0b0f1a".to_string()),
                accent: Some("#ff5c8a".to_string()),
                background: Some("#05070d".to_string()),
                flash: Some("#ffe08a".to_string()),
                extra: vec![],
            },
        );
        cfg.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn an_unknown_top_level_key_round_trips_without_erroring() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        metal_vfs::write(&path, "version = 2\nschema = \"fw/config/2\"\nfrobnicate = true\n")
            .unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.unknown.get("frobnicate"), Some(&toml::Value::Boolean(true)));
    }

    #[test]
    fn migration_runs_once_and_never_deletes_the_legacy_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let sync_path = dir.path().join("sync.toml");
        metal_vfs::write(&sync_path, "namespace = \"abc123\"\n").unwrap();

        let migrated = migrate(
            &config_path,
            Some(LegacySync { namespace: Some("abc123".to_string()) }),
            None,
        )
        .unwrap()
        .expect("migration runs when config.toml is absent");
        assert_eq!(migrated.config.sync.doc.as_deref(), Some("abc123"));
        assert!(!migrated.notes.is_empty());
        assert!(metal_vfs::exists(&sync_path), "the legacy file must survive migration");
        assert!(metal_vfs::exists(&config_path));

        // A second run must not clobber a config.toml that already exists.
        metal_vfs::write(&config_path, "version = 2\nschema = \"fw/config/2\"\n").unwrap();
        let again = migrate(
            &config_path,
            Some(LegacySync { namespace: Some("different".to_string()) }),
            None,
        )
        .unwrap();
        assert!(again.is_none());
    }

    // --- S.AL.2: key_file ------------------------------------------------

    const TEST_KEY: &str = "sk-test-fake-not-real";

    #[test]
    fn secrets_dir_is_config_dir_slash_secrets() {
        assert_eq!(secrets_dir(), config_dir().join("secrets"));
    }

    #[test]
    fn missing_key_file_field_is_ok_none_not_an_error() {
        let provider = Provider::default();
        assert_eq!(read_key_file(&provider), Ok(None));
    }

    #[test]
    fn a_present_but_unreadable_key_file_is_an_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let provider = Provider {
            key_file: Some(dir.path().join("does-not-exist.env").to_string_lossy().into_owned()),
            ..Default::default()
        };
        assert!(read_key_file(&provider).is_err());
    }

    #[test]
    fn reads_a_bare_key_with_whitespace_trimmed() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("bare.env");
        metal_vfs::write(&path, format!("  {TEST_KEY}  \n")).unwrap();
        let provider = Provider {
            key_file: Some(path.to_string_lossy().into_owned()),
            ..Default::default()
        };
        assert_eq!(read_key_file(&provider).unwrap(), Some(TEST_KEY.to_string()));
    }

    #[test]
    fn reads_a_dotenv_style_key_value_line() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("dotenv.env");
        metal_vfs::write(&path, format!("ANTHROPIC_API_KEY={TEST_KEY}\n")).unwrap();
        let provider = Provider {
            key_file: Some(path.to_string_lossy().into_owned()),
            ..Default::default()
        };
        assert_eq!(read_key_file(&provider).unwrap(), Some(TEST_KEY.to_string()));
    }

    #[test]
    fn a_lowercase_left_hand_side_is_not_treated_as_a_dotenv_line() {
        // "not=a=key" doesn't look like `UPPER_CASE=value`, so the whole
        // trimmed line is the key, `=` and all.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("weird.env");
        metal_vfs::write(&path, "not=a=key\n").unwrap();
        let provider = Provider {
            key_file: Some(path.to_string_lossy().into_owned()),
            ..Default::default()
        };
        assert_eq!(read_key_file(&provider).unwrap(), Some("not=a=key".to_string()));
    }

    /// An absolute `key_file` is used as-is; this test would fail if it
    /// accidentally got joined onto `config_dir()`.
    #[test]
    fn an_absolute_key_file_is_used_as_is() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("abs.env");
        metal_vfs::write(&path, TEST_KEY).unwrap();
        assert!(path.is_absolute());
        let provider = Provider {
            key_file: Some(path.to_string_lossy().into_owned()),
            ..Default::default()
        };
        assert_eq!(read_key_file(&provider).unwrap(), Some(TEST_KEY.to_string()));
    }

    /// A relative `key_file` resolves against `config_dir()` — exercised via
    /// `XDG_CONFIG_HOME` (the one override every reader in the workspace
    /// honours) pointed at a scratch directory, never the real
    /// `~/.config/metalvj`. Single-threaded test runs only
    /// (`--test-threads=1`), since this mutates a process-global env var.
    #[test]
    fn a_relative_key_file_resolves_against_config_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let secrets = dir.path().join("metalvj").join("secrets");
        metal_vfs::create_dir_all(&secrets).unwrap();
        metal_vfs::write(secrets.join("ai.env"), format!("ANTHROPIC_API_KEY={TEST_KEY}\n")).unwrap();

        let previous = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        let provider = Provider {
            key_file: Some("secrets/ai.env".to_string()),
            ..Default::default()
        };
        let result = read_key_file(&provider);
        match previous {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        assert_eq!(result.unwrap(), Some(TEST_KEY.to_string()));
    }

    /// H.3-style guarantee for this module: the resolved key must never
    /// surface through a `Debug`/`Display` impl anywhere in this crate.
    /// `Provider` has no field that can hold one (only `key_file`, a path),
    /// so this is a regression guard as much as a spec test — the day
    /// someone adds a field that could hold a resolved key, this is the
    /// test that should start failing until it's redacted.
    #[test]
    fn provider_debug_and_key_file_errors_never_contain_the_key() {
        let provider = Provider {
            key_file: Some("./secrets/ai.env".to_string()),
            ..Default::default()
        };
        assert!(!format!("{provider:?}").contains(TEST_KEY));

        let dir = tempfile::TempDir::new().unwrap();
        let broken = Provider {
            key_file: Some(dir.path().join("missing.env").to_string_lossy().into_owned()),
            ..Default::default()
        };
        let err = read_key_file(&broken).unwrap_err();
        assert!(!err.contains(TEST_KEY));
    }

    #[test]
    fn migrate_key_to_file_writes_secrets_and_points_key_file_at_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        Config::default().save(&config_path).unwrap();

        let migrated = migrate_key_to_file(&config_path, Some(TEST_KEY)).unwrap();
        assert!(migrated.is_some());

        let secret_path = dir.path().join("secrets").join("ai.env");
        let contents = metal_vfs::read_to_string(&secret_path).unwrap();
        assert_eq!(contents.trim(), format!("ANTHROPIC_API_KEY={TEST_KEY}"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metal_vfs::metadata(&secret_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let cfg = Config::load(&config_path).unwrap();
        let provider = cfg.agent.provider.expect("migration creates the table");
        assert_eq!(provider.key_file.as_deref(), Some("secrets/ai.env"));
    }

    #[test]
    fn migrate_key_to_file_is_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        Config::default().save(&config_path).unwrap();

        migrate_key_to_file(&config_path, Some(TEST_KEY)).unwrap();
        let after_first = metal_vfs::read_to_string(&config_path).unwrap();

        // A second run, even with a different (or absent) raw_key, must not
        // touch anything: key_file is already set.
        let second = migrate_key_to_file(&config_path, Some("sk-a-different-fake-key")).unwrap();
        assert!(second.is_none());
        let after_second = metal_vfs::read_to_string(&config_path).unwrap();
        assert_eq!(after_first, after_second);

        let secret_path = dir.path().join("secrets").join("ai.env");
        let contents = metal_vfs::read_to_string(&secret_path).unwrap();
        assert!(contents.contains(TEST_KEY), "the original secret file is untouched");
    }

    #[test]
    fn migrate_key_to_file_is_a_no_op_with_no_key_to_migrate() {
        let dir = tempfile::TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        Config::default().save(&config_path).unwrap();

        assert!(migrate_key_to_file(&config_path, None).unwrap().is_none());
        assert!(migrate_key_to_file(&config_path, Some("")).unwrap().is_none());
        assert!(migrate_key_to_file(&config_path, Some("   ")).unwrap().is_none());
        assert!(!metal_vfs::exists(&dir.path().join("secrets")));
    }

    #[test]
    fn migrate_key_to_file_preserves_an_existing_preset_url_model() {
        let dir = tempfile::TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let mut cfg = Config::default();
        cfg.agent.provider = Some(Provider {
            preset: Some("openai".to_string()),
            url: Some("https://api.openai.com/v1".to_string()),
            model: Some("gpt-5".to_string()),
            key_file: None,
        });
        cfg.save(&config_path).unwrap();

        migrate_key_to_file(&config_path, Some(TEST_KEY)).unwrap();

        let reloaded = Config::load(&config_path).unwrap();
        let provider = reloaded.agent.provider.unwrap();
        assert_eq!(provider.preset.as_deref(), Some("openai"));
        assert_eq!(provider.url.as_deref(), Some("https://api.openai.com/v1"));
        assert_eq!(provider.model.as_deref(), Some("gpt-5"));
        assert_eq!(provider.key_file.as_deref(), Some("secrets/ai.env"));

        assert!(!metal_vfs::read_to_string(&config_path).unwrap().contains(TEST_KEY));
    }
}

#[cfg(test)]
mod app_dir_tests {
    use super::*;

    #[test]
    fn the_pre_rename_dir_is_used_only_while_the_new_one_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        assert_eq!(app_dir_in(base), base.join("metalvj"));
        std::fs::create_dir(base.join("furtherwave")).unwrap();
        assert_eq!(app_dir_in(base), base.join("furtherwave"));
        std::fs::create_dir(base.join("metalvj")).unwrap();
        assert_eq!(app_dir_in(base), base.join("metalvj"));
    }
}
