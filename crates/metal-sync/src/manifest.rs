//! Which paths under `data/` cross the wire, and which never do.
//!
//! **This is an allowlist, and that is the whole design.** A denylist would
//! mean every file a future phase drops into `data/` syncs by default, and the
//! first one that holds a credential leaks it to every paired device before
//! anybody notices. `data/ai.json` already holds provider API keys, so the
//! failure mode is not hypothetical — it is the file sitting there today.
//!
//! The split between what is authored and what is derived is the same one
//! `.gitignore` already draws, and for the same reason: a derived file is
//! cheaper to recompute than to ship, and shipping it invites two peers to
//! overwrite each other's copy of something neither of them edited.
//!
//! # The list lives in `config.toml`, and is read live
//!
//! `plan-opus.md` § O.26: `[sync].allowseed` *subsumes* the `SYNCED` const
//! this module used to carry. There is no const any more. The default list
//! is [`metal_config::allowseed_default`] (one list, one place — rule 4), and
//! a running node reads `[sync].allowseed` from its `config.toml` on every
//! scan tick through [`Allowlist::load`], so a hand edit takes effect on the
//! next tick without a restart. What that means for a live engine is spelled
//! out on [`Allowlist::load`] and in `engine.rs`'s "Reconfiguration under a
//! live engine" section: a path that just left the list stops being
//! published and stops being landed from the next tick on, and is never
//! deleted anywhere; a path that just joined it is picked up like any file
//! that appeared on disk, and the document is re-scanned once so entries
//! peers already hold for it land without waiting for someone to edit them.
//!
//! [`is_safe`] is the other half of the gate and is *not* configurable: no
//! pattern, however written, can make `..` or an absolute path acceptable.

use std::path::{Component, Path};

/// The set of path patterns that sync, relative to the data root. `*` matches
/// one path segment; `**` matches the rest of the path.
///
/// A value, not a static: every scan asks [`crate::state::SyncState`] for the
/// current one, which reads it from `config.toml` — see [`Allowlist::load`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allowlist {
    patterns: Vec<String>,
}

impl Allowlist {
    /// What syncs when `config.toml` has no `[sync].allowseed` key (or no
    /// `config.toml` at all): [`metal_config::allowseed_default`], verbatim.
    pub fn builtin() -> Allowlist {
        Allowlist { patterns: metal_config::allowseed_default() }
    }

    /// Nothing syncs. What a config that cannot be read resolves to — an
    /// allowlist whose contents are unknown is an allowlist with nothing on
    /// it, by the same rule that excludes any file nobody has thought about.
    pub fn empty() -> Allowlist {
        Allowlist { patterns: Vec::new() }
    }

    pub fn from_patterns(patterns: Vec<String>) -> Allowlist {
        Allowlist { patterns }
    }

    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// Read `[sync].allowseed` from the `config.toml` at `path`, fresh.
    ///
    /// Three outcomes, mirroring `metal_config::Config::load`'s own rule
    /// ("missing is empty, corrupt is an error"):
    ///
    /// - **No file**: [`Allowlist::builtin`]. A node that has never written a
    ///   `config.toml` syncs exactly what it did before the list was
    ///   configurable.
    /// - **A file with no `allowseed` key**: the same builtin — `serde`'s
    ///   default for the field is `allowseed_default`, so an operator who
    ///   only ever set `[sync].doc` has not narrowed anything.
    /// - **A file that does not parse**: `Err`. The caller (`SyncState`)
    ///   logs it and uses [`Allowlist::empty`] for that tick — nothing is
    ///   published or landed until the file parses again, and nothing is
    ///   deleted. A person mid-edit in a text editor gets a quiet tick, not a
    ///   node that guessed at their intent; the allowlist philosophy above
    ///   says "unknown means excluded", and an unparseable list is unknown.
    pub fn load(config_toml: &Path) -> Result<Allowlist, String> {
        let cfg = metal_config::Config::load(config_toml)?;
        Ok(Allowlist { patterns: cfg.sync.allowseed })
    }

    /// Does this path, relative to the data root, cross the wire?
    ///
    /// Anything not matched is excluded, including anything this list has
    /// never heard of. Unsafe paths are excluded before any pattern is
    /// consulted, so an `allowseed` entry cannot widen the gate past the
    /// data root even if `metal-config::validate` was never run on it.
    pub fn syncable(&self, rel: &Path) -> bool {
        if !is_safe(rel) {
            return false;
        }
        let Some(text) = rel.to_str() else {
            return false;
        };
        self.patterns.iter().any(|pat| matches(pat, text))
    }

    /// Could anything syncable live under this directory? True when some
    /// pattern is deeper than `dir` and agrees with it segment by segment.
    /// `reader.rs` prunes its walk on this, which is what keeps a scan from
    /// descending into the 426 MB `digest/.venv` or the 146 MB of reel
    /// frames every three seconds. Derived from the patterns rather than a
    /// hand-kept list of directory names, so a pattern added to
    /// `config.toml` is reachable without a code change.
    pub fn worth_descending(&self, dir: &Path) -> bool {
        let Some(text) = dir.to_str() else {
            return false;
        };
        self.patterns.iter().any(|pat| prefix_matches(pat, text))
    }
}

/// Glob with exactly two features, because exactly two are needed: `*` for one
/// segment and `**` for the tail.
fn matches(pattern: &str, path: &str) -> bool {
    let mut pat = pattern.split('/');
    let mut seg = path.split('/');
    loop {
        match (pat.next(), seg.next()) {
            (Some("**"), Some(_)) => return true,
            (Some(p), Some(s)) if p == "*" || p == s => continue,
            (Some(p), Some(s)) if p.contains('*') => {
                if !wildcard(p, s) {
                    return false;
                }
            }
            (None, None) => return true,
            _ => return false,
        }
    }
}

/// Does `pattern` have more segments than `dir` and match every one `dir`
/// has? The directory-prefix half of [`matches`]: `graphs/*.json` says yes to
/// `graphs`, no to `graphs/nested`; `animations/**` says yes to any depth
/// under `animations/`.
fn prefix_matches(pattern: &str, dir: &str) -> bool {
    let mut pat = pattern.split('/');
    for s in dir.split('/') {
        match pat.next() {
            Some("**") => return true,
            Some(p) if p == "*" || p == s => continue,
            Some(p) if p.contains('*') && wildcard(p, s) => continue,
            _ => return false,
        }
    }
    // Every directory segment matched; there has to be at least one pattern
    // segment left to name a file under it.
    pat.next().is_some()
}

/// A single `*` inside one segment, as in `colourist-*.json`.
fn wildcard(pattern: &str, s: &str) -> bool {
    let Some((head, tail)) = pattern.split_once('*') else {
        return pattern == s;
    };
    s.len() >= head.len() + tail.len() && s.starts_with(head) && s.ends_with(tail)
}

/// A path is safe when it stays inside the data root by construction: relative,
/// no `..`, no root, no Windows prefix.
///
/// `reader.rs` used to `join` whatever a caller handed it, which meant
/// `POST /sync?file=../../etc/passwd` read outside `data/`. Every path that
/// enters this crate — from an HTTP query or from a peer's doc entry — goes
/// through here first.
pub fn is_safe(rel: &Path) -> bool {
    if rel.as_os_str().is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_)))
    {
        return false;
    }

    // S.AL.2: `secrets/` (`metal_config::secrets_dir`) is where a provider's
    // key gets migrated out of `data/ai.json` and into a 0600 file —
    // credential material, same class of thing this whole module exists to
    // keep off the wire. It normally lives under the *config* directory,
    // never under the data root this allowlist gates, so no scan should
    // ever hand this function a path like this in practice; the guard is
    // here anyway; same "not configurable" guarantee the rest of this
    // function already gives traversal — no `allowseed` pattern, however
    // written (`secrets/*`, `**`, a hand-typed exact path), can make
    // anything under `secrets/` syncable.
    if rel.components().next() == Some(Component::Normal(std::ffi::OsStr::new("secrets"))) {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ok(p: &str) -> bool {
        Allowlist::builtin().syncable(&PathBuf::from(p))
    }

    #[test]
    fn authored_state_syncs() {
        assert!(ok("outputs.json"));
        assert!(ok("trims.json"));
        assert!(ok("collections.json"));
        assert!(ok("sequences.json"));
        assert!(ok("switches.json"));
        assert!(ok("palettes.json"));
        assert!(ok("graphs/entrance-set.json"));
        assert!(ok("timelines/show.json"));
        assert!(ok("library/movements.json"));
        assert!(ok("tracks/a.json"));
        assert!(ok("fixtures/par64.toml"));
        assert!(ok("agent/shots.json"));
        assert!(ok("agent/roles.json"));
    }

    #[test]
    fn the_animation_corpus_syncs_both_halves() {
        assert!(ok("animations/entrance/song.json"));
        assert!(ok("animations/entrance/song.npy"));
    }

    #[test]
    fn reel_metadata_syncs_but_frames_do_not() {
        assert!(ok("reels/example-footage/index.json"));
        assert!(ok("reels/example-footage/curator.json"));
        assert!(ok("reels/example-footage/colourist-warm.json"));
        // 146 MB of derived JPEGs. Recomputed by indexing, never shipped.
        assert!(!ok("reels/example-footage/frames/000001.jpg"));
    }

    /// The one that matters most: `ai.json` holds provider API keys. A
    /// credential must never traverse the sync namespace, no matter who is
    /// paired.
    #[test]
    fn credentials_never_sync() {
        assert!(!ok("ai.json"));
    }

    /// `schema.json` is emitted by `emit-schema` from the compiled binary. Two
    /// peers on different builds overwriting each other's manifest would make
    /// every graph lint against the wrong float layout.
    #[test]
    fn build_derived_state_never_syncs() {
        assert!(!ok("schema.json"));
        assert!(!ok("router/model.json"));
    }

    /// 426 MB of Python virtualenv, with `.so` files compiled for one
    /// architecture. Shipping it from an x86 laptop to an aarch64 Pi would
    /// deliver natives that cannot load.
    #[test]
    fn the_digest_venv_never_syncs() {
        assert!(!ok("digest/.venv/lib/python3.12/site-packages/numpy/core/_multiarray.so"));
        assert!(!ok("digest/ready.8hz.npy"));
        assert!(!ok("digest/digest_audio.py"));
    }

    /// 633 MB of source media, referenced by path rather than shipped.
    #[test]
    fn audio_never_syncs() {
        assert!(!ok("audio/track.mp3"));
        assert!(!ok("audio/track.8hz.npy"));
    }

    /// An allowlist means a file nobody has thought about yet is excluded,
    /// rather than included and noticed later.
    #[test]
    fn anything_unknown_is_excluded_by_default() {
        assert!(!ok("some-future-secret.json"));
        assert!(!ok("figures/figures.html"));
        assert!(!ok("models/README.md"));
    }

    #[test]
    fn traversal_is_never_syncable() {
        assert!(!ok("../../etc/passwd"));
        assert!(!ok("/etc/passwd"));
        assert!(!ok("graphs/../../../etc/passwd"));
        assert!(!is_safe(&PathBuf::from("")));
    }

    /// S.AL.2: `secrets/ai.env` is where a provider's key lives once
    /// migrated out of `data/ai.json` — no `allowseed` pattern, however it
    /// is spelled, may make anything under `secrets/` syncable. Checked both
    /// through the builtin list and through a config that has hand-added an
    /// exact or wildcard pattern naming it — `is_safe` refuses it before any
    /// pattern is even consulted, same as traversal above.
    #[test]
    fn secrets_never_sync_no_matter_how_the_pattern_is_written() {
        assert!(!ok("secrets/ai.env"));
        assert!(!is_safe(&PathBuf::from("secrets/ai.env")));

        for allowseed in [
            vec!["secrets/ai.env".to_string()],
            vec!["secrets/*".to_string()],
            vec!["secrets/**".to_string()],
            vec!["**".to_string()],
        ] {
            let list = Allowlist::from_patterns(allowseed.clone());
            assert!(
                !list.syncable(Path::new("secrets/ai.env")),
                "allowseed {allowseed:?} must not reach secrets/ai.env"
            );
        }
    }

    /// A hand-edited `config.toml` with `[sync].allowseed = ["secrets/ai.env"]`
    /// loads without erroring (this crate never fails a load over
    /// `allowseed`'s content — see `Allowlist::load`'s doc), but the pattern
    /// is inert: `Allowlist::syncable` still refuses it. `metal_config::problems`
    /// separately flags it as a hard error for `--doctor`/`check-config` to
    /// report (`validate.rs`'s own test for that).
    #[test]
    fn a_hand_edited_config_naming_secrets_loads_but_the_pattern_is_inert() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "version = 2\nschema = \"fw/config/2\"\n\n[sync]\nallowseed = [\"secrets/ai.env\"]\n",
        )
        .unwrap();
        let list = Allowlist::load(&path).unwrap();
        assert!(!list.syncable(Path::new("secrets/ai.env")));
    }

    #[test]
    fn a_directory_prefix_does_not_match_a_deeper_file() {
        // `graphs/*.json` is one segment, not a tree.
        assert!(!ok("graphs/nested/deep.json"));
    }

    /// The scene favourites ship; their posters (`projects/hap/`) are derived
    /// and never do.
    #[test]
    fn scene_favourites_sync_and_their_posters_do_not() {
        assert!(ok("scenes/favorites.json"));
        assert!(!ok("projects/hap/clip-0.scenes/0.jpg"));
    }

    // --- the list is configuration ------------------------------------------

    /// The builtin and `metal_config`'s default are one list, not two that
    /// happen to agree today.
    #[test]
    fn the_builtin_list_is_fw_configs_default() {
        assert_eq!(Allowlist::builtin().patterns(), &metal_config::allowseed_default()[..]);
        assert_eq!(
            Allowlist::from_patterns(metal_config::Config::default().sync.allowseed),
            Allowlist::builtin()
        );
    }

    #[test]
    fn a_missing_config_toml_loads_the_builtin_list() {
        let dir = tempfile::TempDir::new().unwrap();
        let list = Allowlist::load(&dir.path().join("config.toml")).unwrap();
        assert_eq!(list, Allowlist::builtin());
    }

    #[test]
    fn a_config_toml_without_allowseed_keeps_the_builtin_list() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "version = 2\nschema = \"fw/config/2\"\n\n[sync]\ndoc = \"abc\"\n")
            .unwrap();
        assert_eq!(Allowlist::load(&path).unwrap(), Allowlist::builtin());
    }

    #[test]
    fn a_hand_edited_allowseed_narrows_and_widens_the_gate() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "version = 2\nschema = \"fw/config/2\"\n\n[sync]\nallowseed = [\"trims.json\", \"notes/*.md\"]\n",
        )
        .unwrap();
        let list = Allowlist::load(&path).unwrap();
        assert!(list.syncable(Path::new("trims.json")));
        assert!(list.syncable(Path::new("notes/today.md")), "a new pattern is honoured");
        assert!(!list.syncable(Path::new("graphs/a.json")), "a dropped pattern is gone");
        // Narrowing the list can never widen `is_safe`.
        assert!(!list.syncable(Path::new("../trims.json")));
    }

    #[test]
    fn a_corrupt_config_toml_is_an_error_not_a_guess() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is not toml = = =").unwrap();
        assert!(Allowlist::load(&path).is_err());
        assert!(!Allowlist::empty().syncable(Path::new("trims.json")));
    }

    #[test]
    fn worth_descending_follows_the_patterns_not_a_hand_kept_list() {
        let list = Allowlist::builtin();
        for dir in ["graphs", "timelines", "library", "tracks", "fixtures", "animations", "agent", "reels", "render"] {
            assert!(list.worth_descending(Path::new(dir)), "{dir}");
        }
        assert!(list.worth_descending(Path::new("reels/footage")));
        assert!(list.worth_descending(Path::new("reels/footage/verified")));
        assert!(list.worth_descending(Path::new("animations/a/b/c")), "`**` is any depth");
        for dir in ["audio", "digest", "router", "figures", "models", "presets", "graphs/nested", "reels/footage/frames"] {
            assert!(!list.worth_descending(Path::new(dir)), "{dir}");
        }
        // A configured pattern reaches a directory the builtin never named.
        let custom = Allowlist::from_patterns(vec!["notes/*.md".into()]);
        assert!(custom.worth_descending(Path::new("notes")));
        assert!(!custom.worth_descending(Path::new("graphs")));
    }
}
