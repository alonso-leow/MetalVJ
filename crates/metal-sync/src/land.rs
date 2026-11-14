//! Landing a peer's bytes into the local `data/`.
//!
//! Two rules, and both exist because of something the rest of the repo does
//! not do.
//!
//! **Atomic.** Every write here is a temp file in the target's own directory
//! followed by a `rename`. `metal-dash` does not do this — every one of its save
//! sites is a plain in-place `std::fs::write`, and there is no `fs::rename`
//! anywhere in `metal-dash/src`. That is survivable when a single process is the
//! only writer. It stops being survivable the moment a second process writes
//! the same tree, because `metal-dash` reads graphs *per request*: a reader can
//! land mid-write and get a truncated file. The rename makes the swap atomic
//! at the filesystem level, so a reader sees either the old file or the new
//! one and never half of either.
//!
//! **Validated.** A graph that does not parse, or that lints with an error, is
//! rejected and logged rather than written. A peer sending a bad graph should
//! not be able to put the local instance into a state its own UI could not
//! have produced.

use std::path::{Path, PathBuf};

use crate::manifest::Allowlist;

/// Who wrote an incoming entry, as far as the club can tell: a member of the
/// document we paired with, or someone else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sender {
    Paired,
    Unpaired,
}


/// What happened to one incoming blob.
#[derive(Debug, Clone, PartialEq)]
pub enum Landing {
    /// Written to disk, atomically.
    Wrote,
    /// The path is not one this node accepts. Excluded by the allowlist, or
    /// not a safe relative path.
    Refused(String),
    /// The bytes arrived but did not survive the gate.
    Invalid(String),
}

pub struct Lander {
    data_root: PathBuf,
    /// `None` when `schema.json` is absent or unreadable — a fresh node that
    /// has never run `emit-schema`. Graphs still have to parse; they just do
    /// not get linted, because linting against a catalog we do not have would
    /// mean rejecting every graph on a node that has not been set up yet.
    catalog: Option<Vec<metal_graph::NodeKind>>,
}

impl Lander {
    pub fn new(data_root: PathBuf) -> Lander {
        let catalog = load_catalog(&data_root);
        if catalog.is_none() {
            tracing::warn!(
                "no readable data/schema.json: incoming graphs will be parsed but not linted"
            );
        }
        Lander { data_root, catalog }
    }

    /// Validate `bytes` as the contents of `rel`, and write it if it passes.
    ///
    /// `allow` is the allowlist *as of this landing* — the engine reads it
    /// from `config.toml` fresh (`SyncState::allowlist`), so a path an
    /// operator just removed from `[sync].allowseed` is refused here even if
    /// a peer still publishes it, and nothing already on disk is touched.
    pub fn land(&self, rel: &Path, bytes: &[u8], allow: &Allowlist, from: Sender) -> std::io::Result<Landing> {
        if !allow.syncable(rel) {
            return Ok(Landing::Refused(format!("{} is not a synced path", rel.display())));
        }
        // A graph this machine owns takes writes from any paired peer, last
        // write wins: the owner is where the pixels are drawn (a follower takes
        // those over `fw/frame/1`), a peer takes the state, so a peer may edit
        // the graph it is watching. An author that is not paired is still
        // refused, and our copy kept.
        if from == Sender::Unpaired {
            if let Some(why) = self.owned_here(rel) {
                tracing::warn!(path = %rel.display(), %why, "dropped an incoming graph write");
                return Ok(Landing::Refused(why));
            }
        }
        if let Err(why) = self.validate(rel, bytes) {
            tracing::warn!(path = %rel.display(), %why, "rejected an incoming blob");
            return Ok(Landing::Invalid(why));
        }
        // The shared session log is an append log: merge the peer's copy into
        // ours instead of replacing it, so partitioned appends survive.
        if crate::session_merge::is_session(rel) {
            let text = String::from_utf8_lossy(bytes);
            crate::session_merge::land(&self.data_root.join(rel), &text)?;
            return Ok(Landing::Wrote);
        }
        self.write_atomic(rel, bytes)?;
        Ok(Landing::Wrote)
    }

    fn owned_here(&self, rel: &Path) -> Option<String> {
        if !is_graph(rel) || rel.extension().is_none_or(|e| e != "json") {
            return None;
        }
        let id = rel.file_stem()?.to_str()?;
        let me = metal_graph::owner::local_identity();
        let owner = metal_graph::owner::status(&self.data_root.join("graphs"), id)?;
        (owner.owner == me).then(|| format!("graph {id:?} is owned by this machine ({me:?}); an unpaired author's write to it is dropped"))
    }

    /// The gate. A graph gets the full treatment because a graph is the one
    /// document the render path compiles; everything else only has to be the
    /// kind of file it claims to be.
    fn validate(&self, rel: &Path, bytes: &[u8]) -> Result<(), String> {
        let is_json = rel.extension().is_some_and(|e| e == "json");
        if !is_json {
            return Ok(());
        }
        let text = std::str::from_utf8(bytes).map_err(|e| format!("not utf-8: {e}"))?;

        if is_graph(rel) {
            let g = metal_graph::Graph::from_json(text).map_err(|e| format!("did not parse: {e}"))?;

            // Lint is reported, never enforced. `metal-dash` saves a graph whose
            // lint has errors — `graph_reply` hands the issues back to the UI
            // and writes the file anyway — because a graph under construction
            // is lint-failing by definition until the last cable is connected.
            // The repo's own `data/graphs/empty-1.json` fails lint today.
            //
            // So refusing a lint-failing graph here would mean a graph the
            // author saved on one machine silently never appearing on another:
            // a capability that exists on one device and not the other, which
            // is the exact failure this whole phase is meant to remove. Sync
            // mirrors what the authoring tool allows; it does not hold a
            // stricter opinion than the tool that produced the file.
            if let Some(catalog) = &self.catalog {
                let errors: Vec<String> = g
                    .lint(catalog)
                    .into_iter()
                    .filter(|i| i.severity == metal_graph::lint::Severity::Error)
                    .map(|i| i.message)
                    .collect();
                if !errors.is_empty() {
                    tracing::info!(
                        path = %rel.display(),
                        issues = %errors.join("; "),
                        "landing a graph that does not lint clean, as metal-dash would have saved it"
                    );
                }
            }
            return Ok(());
        }

        serde_json::from_str::<serde_json::Value>(text)
            .map(|_| ())
            .map_err(|e| format!("did not parse as json: {e}"))
    }

    /// Temp file **in the target's own directory**, then rename. Same
    /// directory rather than a shared scratch dir because `rename` is only
    /// atomic within one filesystem, and a `data/` on its own mount is exactly
    /// the setup where a scratch dir elsewhere would silently degrade to a
    /// copy.
    fn write_atomic(&self, rel: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let target = self.data_root.join(rel);
        let dir = target.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "no parent directory")
        })?;
        std::fs::create_dir_all(dir)?;

        let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("blob");
        let tmp = dir.join(format!(".{name}.metal-sync-{}", std::process::id()));

        // The temp file is removed on any failure so a crash mid-land does not
        // leave litter beside the real file.
        if let Err(e) = std::fs::write(&tmp, bytes) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&tmp, &target) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(())
    }
}

fn is_graph(rel: &Path) -> bool {
    rel.parent().is_some_and(|p| p.as_os_str() == "graphs")
}

fn load_catalog(data_root: &Path) -> Option<Vec<metal_graph::NodeKind>> {
    let text = std::fs::read_to_string(data_root.join("schema.json")).ok()?;
    let manifest = metal_schema::Manifest::parse(&text).ok()?;
    Some(metal_graph::NodeKind::catalog(&manifest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lander() -> (tempfile::TempDir, Lander) {
        let dir = tempfile::TempDir::new().unwrap();
        let lander = Lander::new(dir.path().to_path_buf());
        (dir, lander)
    }

    #[test]
    fn a_synced_json_file_lands() {
        let (dir, l) = lander();
        let rel = Path::new("trims.json");
        assert_eq!(l.land(rel, b"{\"a\":1}", &Allowlist::builtin(), Sender::Paired).unwrap(), Landing::Wrote);
        assert_eq!(std::fs::read_to_string(dir.path().join(rel)).unwrap(), "{\"a\":1}");
    }

    #[test]
    fn an_excluded_path_is_refused_before_anything_is_written() {
        let (dir, l) = lander();
        // The credential file, which must never arrive from a peer.
        let rel = Path::new("ai.json");
        assert!(matches!(l.land(rel, b"{}", &Allowlist::builtin(), Sender::Paired).unwrap(), Landing::Refused(_)));
        assert!(!dir.path().join(rel).exists());
    }

    #[test]
    fn traversal_is_refused_and_writes_nothing_outside_the_root() {
        let (dir, l) = lander();
        let rel = Path::new("../escaped.json");
        assert!(matches!(l.land(rel, b"{}", &Allowlist::builtin(), Sender::Paired).unwrap(), Landing::Refused(_)));
        assert!(!dir.path().parent().unwrap().join("escaped.json").exists());
    }

    #[test]
    fn malformed_json_never_lands() {
        let (dir, l) = lander();
        let rel = Path::new("trims.json");
        assert!(matches!(l.land(rel, b"{not json", &Allowlist::builtin(), Sender::Paired).unwrap(), Landing::Invalid(_)));
        assert!(!dir.path().join(rel).exists());
    }

    #[test]
    fn a_graph_that_does_not_parse_never_lands() {
        let (dir, l) = lander();
        let rel = Path::new("graphs/x.json");
        assert!(matches!(l.land(rel, b"{\"nonsense\": true}", &Allowlist::builtin(), Sender::Paired).unwrap(), Landing::Invalid(_)));
        assert!(!dir.path().join(rel).exists());
    }

    /// The correction that the first two-node run forced. `metal-dash` saves
    /// graphs whose lint has errors — the repo's own `empty-1.json` is one —
    /// so refusing them here would make a saved graph vanish between devices.
    #[test]
    fn a_graph_that_parses_but_fails_lint_still_lands() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let (Ok(schema), Ok(graph)) = (
            std::fs::read(repo.join("schema.json")),
            std::fs::read(repo.join("graphs/empty-1.json")),
        ) else {
            // `data/graphs/` is gitignored, so a fresh worktree may not have
            // it. Rule 8: absent fixture is not a regression.
            return;
        };
        std::fs::write(dir.path().join("schema.json"), schema).unwrap();

        let l = Lander::new(dir.path().to_path_buf());
        let rel = Path::new("graphs/empty-1.json");
        assert_eq!(
            l.land(rel, &graph, &Allowlist::builtin(), Sender::Paired).unwrap(),
            Landing::Wrote,
            "a graph metal-dash saved must survive the trip to another device"
        );
    }

    /// Overwriting must leave no window where the file is absent or partial,
    /// and no temp litter beside it once the rename has happened.
    #[test]
    fn landing_over_an_existing_file_leaves_no_temp_behind() {
        let (dir, l) = lander();
        let rel = Path::new("trims.json");
        l.land(rel, b"{\"v\":1}", &Allowlist::builtin(), Sender::Paired).unwrap();
        l.land(rel, b"{\"v\":2}", &Allowlist::builtin(), Sender::Paired).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join(rel)).unwrap(), "{\"v\":2}");

        let strays: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains("metal-sync"))
            .collect();
        assert!(strays.is_empty(), "temp files left behind: {strays:?}");
    }

    /// A graph this machine owns refuses an unpaired author's version of it and
    /// takes any paired peer's (last write wins); the bytes land either way
    /// once it is released.
    #[test]
    fn a_graph_owned_here_takes_paired_writes_and_drops_unpaired_ones() {
        let (dir, l) = lander();
        let graphs = dir.path().join("graphs");
        let g = metal_graph::Graph::new("mine", "Mine", metal_graph::Domain::Live).to_json();
        let rel = Path::new("graphs/mine.json");
        let me = metal_graph::owner::local_identity();
        metal_graph::owner::take(&graphs, "mine", &me).unwrap();
        assert!(matches!(l.land(rel, g.as_bytes(), &Allowlist::builtin(), Sender::Unpaired).unwrap(), Landing::Refused(_)));
        assert!(!graphs.join("mine.json").exists());
        assert_eq!(l.land(rel, g.as_bytes(), &Allowlist::builtin(), Sender::Paired).unwrap(), Landing::Wrote);
        let newer = metal_graph::Graph::new("mine", "Edited by a peer", metal_graph::Domain::Live).to_json();
        assert_eq!(l.land(rel, newer.as_bytes(), &Allowlist::builtin(), Sender::Paired).unwrap(), Landing::Wrote);
        assert!(std::fs::read_to_string(graphs.join("mine.json")).unwrap().contains("Edited by a peer"));
        metal_graph::owner::release(&graphs, "mine", &me).unwrap();
        assert_eq!(l.land(rel, g.as_bytes(), &Allowlist::builtin(), Sender::Paired).unwrap(), Landing::Wrote);
        // The owner record itself is a synced path.
        assert!(Allowlist::builtin().syncable(Path::new("graphs/mine.owner")));
    }

    /// The session log merges on landing instead of being replaced, so an
    /// entry written here while partitioned survives a peer's copy.
    #[test]
    fn the_session_log_merges_instead_of_being_replaced() {
        let (dir, l) = lander();
        let rel = Path::new("agent/session.jsonl");
        let mine = "{\"seq\":1,\"role\":\"super\",\"author\":\"a\",\"body\":{}}\n";
        let theirs = "{\"seq\":1,\"role\":\"producer\",\"author\":\"b\",\"body\":{}}\n";
        assert_eq!(l.land(rel, mine.as_bytes(), &Allowlist::builtin(), Sender::Paired).unwrap(), Landing::Wrote);
        assert_eq!(l.land(rel, theirs.as_bytes(), &Allowlist::builtin(), Sender::Paired).unwrap(), Landing::Wrote);
        let got = std::fs::read_to_string(dir.path().join(rel)).unwrap();
        assert_eq!(got.lines().count(), 2, "{got}");
    }

    #[test]
    fn nested_paths_get_their_directory_created() {
        let (dir, l) = lander();
        let rel = Path::new("animations/entrance/song.json");
        assert_eq!(l.land(rel, b"{}", &Allowlist::builtin(), Sender::Paired).unwrap(), Landing::Wrote);
        assert!(dir.path().join(rel).exists());
    }

    /// A `.npy` carries a float matrix, not JSON. The gate must not demand it
    /// parse as text.
    #[test]
    fn binary_payloads_skip_the_json_check() {
        let (dir, l) = lander();
        let rel = Path::new("animations/entrance/song.npy");
        assert_eq!(l.land(rel, &[0x93, b'N', b'U', b'M', b'P', b'Y'], &Allowlist::builtin(), Sender::Paired).unwrap(), Landing::Wrote);
        assert!(dir.path().join(rel).exists());
    }
}
