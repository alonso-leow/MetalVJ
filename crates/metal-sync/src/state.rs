//! What one running `metal-sync` knows: who it is, what it has seen, and the
//! content hash of every file it is responsible for.
//!
//! The hashes are the point. `iroh-docs` entries carry a hash rather than the
//! bytes, and the local half of that bargain is knowing what we last published
//! for each path so a scan can tell "changed" from "touched". Without it every
//! poll would republish the whole tree.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::land::Lander;
use crate::manifest::Allowlist;
use crate::reader::DataReader;

pub struct SyncState {
    node_id: String,
    reader: DataReader,
    lander: Lander,
    /// Where `config.toml` lives, so [`SyncState::allowlist`] can read
    /// `[sync].allowseed` fresh on every scan. `None` (tests, a state built
    /// without [`SyncState::with_config`]) means the builtin list, forever —
    /// the CLI always sets it.
    config_toml: Option<PathBuf>,
    /// Path relative to `data/` → BLAKE3 hash of what we last published.
    published: HashMap<PathBuf, String>,
    /// Files that were on disk when a joiner started, with the hash they had
    /// then. Their timestamps are older than the club's real entries, and
    /// iroh-docs stamps every write with now(), so publishing them would win
    /// LWW over the club's copy. They stay unpublished until the club's first
    /// sync lands; a file whose bytes change after the join leaves this set
    /// and publishes like any new write.
    withheld: HashMap<PathBuf, String>,
    /// Whether the club's first sync has landed since the join. Persisted with
    /// the withheld set: a restart before it keeps the hold.
    club_synced: bool,
    /// The hold's timeout ran out before the club's first sync. Persisted so a
    /// restart does not run a second full timeout for new writes; withheld
    /// files still wait for the club.
    timed_out: bool,
    /// The sidecar that carries the withheld set and `club_synced` across a
    /// restart (`join_hold.json` next to `sync.toml`). `None` keeps them in
    /// memory only.
    hold_file: Option<PathBuf>,
    peers: Vec<String>,
    last_sync: u64,
    /// A ticket for the document, generated once the endpoint is bound.
    ///
    /// It is held here rather than minted on demand because it carries *this
    /// run's* addresses. A ticket printed by a separate short-lived process
    /// names an endpoint that no longer exists the moment that process exits,
    /// which is a pairing flow that fails in a way nobody can see.
    ticket: Option<String>,
    /// `POST /pause`: nothing is published and nothing lands until
    /// `POST /resume`, which re-lands the document's latest entry per key.
    paused: bool,
    /// Node id → (online, last seen, unix seconds). Fed by the document's
    /// `NeighborUp`/`NeighborDown`; read by `GET /peers`.
    liveness: HashMap<String, (bool, u64)>,
    log: EventLog,
    /// Superseded writes: another author's entry for the same key won
    /// last-write-wins, so this one was never landed. `GET /log` reports the
    /// count and the most recent of them.
    lww: EventLog,
    lww_count: u64,
}

/// How many events `GET /log` keeps. A ring, so a node left running for a
/// season holds the last few minutes of trouble, never an unbounded file.
pub const LOG_CAPACITY: usize = 256;

/// The ring behind `GET /log`.
#[derive(Default)]
pub struct EventLog {
    ring: VecDeque<Value>,
}

impl EventLog {
    /// Append one event, dropping the oldest once the ring is full.
    pub fn push(&mut self, kind: &str, detail: Value) {
        if self.ring.len() == LOG_CAPACITY {
            self.ring.pop_front();
        }
        self.ring.push_back(json!({ "t": now(), "kind": kind, "detail": detail }));
    }

    pub fn entries(&self) -> Vec<Value> {
        self.ring.iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}

impl SyncState {
    pub fn new(node_id: String, data_root: PathBuf) -> SyncState {
        SyncState {
            node_id,
            reader: DataReader::new(data_root.clone()),
            lander: Lander::new(data_root),
            config_toml: None,
            published: HashMap::new(),
            withheld: HashMap::new(),
            club_synced: false,
            timed_out: false,
            hold_file: None,
            peers: Vec::new(),
            last_sync: now(),
            ticket: None,
            paused: false,
            liveness: HashMap::new(),
            log: EventLog::default(),
            lww: EventLog::default(),
            lww_count: 0,
        }
    }

    /// Point this state at the `config.toml` whose `[sync].allowseed`
    /// governs what syncs. Read live, never cached — see [`SyncState::allowlist`].
    pub fn with_config(mut self, config_toml: PathBuf) -> SyncState {
        self.config_toml = Some(config_toml);
        self
    }

    /// The allowlist *right now*, read from `config.toml` on every call.
    ///
    /// Rule 4: the list is state, its source is the file, and this is the
    /// only reader — nothing holds a parsed copy across ticks. A read is one
    /// small TOML file every `SCAN_INTERVAL`, which costs nothing next to
    /// hashing `data/`. The three cases are [`Allowlist::load`]'s: no file →
    /// builtin, file without the key → builtin, file that does not parse →
    /// logged and [`Allowlist::empty`] for this call, so a half-saved edit
    /// syncs nothing rather than something the operator did not write.
    pub fn allowlist(&self) -> Allowlist {
        let Some(path) = &self.config_toml else {
            return Allowlist::builtin();
        };
        match Allowlist::load(path) {
            Ok(list) => list,
            Err(e) => {
                tracing::warn!(%e, "config.toml unreadable — syncing nothing until it parses again");
                Allowlist::empty()
            }
        }
    }

    /// Where the withheld set is persisted. Set before the first hold.
    pub fn set_hold_file(&mut self, path: Option<PathBuf>) {
        self.hold_file = path;
    }

    pub fn set_ticket(&mut self, ticket: String) {
        self.ticket = Some(ticket);
    }

    pub fn ticket(&self) -> Option<&str> {
        self.ticket.as_deref()
    }

    pub fn reader(&self) -> &DataReader {
        &self.reader
    }

    pub fn lander(&self) -> &Lander {
        &self.lander
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    pub fn status(&self) -> Value {
        json!({
            "node_id": self.node_id,
            "peers": self.peers,
            "tracked_files": self.published.len(),
            "last_sync": self.last_sync,
        })
    }

    /// Hash one file and record it as published, returning what changed.
    ///
    /// Returns `changed: false` when the bytes are the same ones we published
    /// last time — the caller uses that to skip the doc write, which is what
    /// keeps an idle installation from generating sync traffic every tick.
    pub fn publish(&mut self, rel: &Path) -> Result<Value, String> {
        if !self.allowlist().syncable(rel) {
            return Err(format!("{} is not a synced path", rel.display()));
        }
        let bytes = self.reader.read_file(rel)?;
        let hash = blake3::hash(&bytes).to_hex().to_string();

        let changed = self.published.get(rel) != Some(&hash);
        if changed {
            self.published.insert(rel.to_path_buf(), hash.clone());
            self.last_sync = now();
        }

        Ok(json!({
            "file": rel.display().to_string(),
            "hash": hash,
            "size_bytes": bytes.len(),
            "changed": changed,
        }))
    }

    /// Every syncable file whose bytes differ from what we last published.
    ///
    /// "Syncable" is decided by this scan's [`SyncState::allowlist`], so a
    /// path removed from `[sync].allowseed` simply stops appearing here from
    /// the next tick — its `published` entry is left alone (harmless: it only
    /// says what hash the document last saw, and if the path is re-included
    /// an unchanged file is correctly not republished while a changed one
    /// is). Nothing is ever deleted, on disk or in the document.
    pub fn changed_since_last_scan(&mut self) -> Vec<PathBuf> {
        let mut changed = Vec::new();
        let allow = self.allowlist();
        for rel in self.reader.syncable_files(&allow) {
            let Ok(bytes) = self.reader.read_file(&rel) else { continue };
            let hash = blake3::hash(&bytes).to_hex().to_string();
            match self.withheld.get(&rel) {
                // Still the bytes that were on disk at join time: not a new
                // write, so it stays out of the document until the club lands.
                Some(at_join) if *at_join == hash => continue,
                // Edited since the join: a new local write, publish it.
                Some(_) => {
                    self.withheld.remove(&rel);
                }
                None => {}
            }
            if self.published.get(&rel) != Some(&hash) {
                self.published.insert(rel.clone(), hash);
                changed.push(rel);
            }
        }
        if !changed.is_empty() {
            self.last_sync = now();
        }
        changed
    }

    pub fn set_peers(&mut self, peers: Vec<String>) {
        let t = now();
        for (id, entry) in self.liveness.iter_mut() {
            if entry.0 && !peers.contains(id) {
                *entry = (false, t);
            }
        }
        for id in &peers {
            self.liveness.insert(id.clone(), (true, t));
        }
        self.peers = peers;
    }

    /// Is this node id a neighbour right now, and when was it last seen?
    pub fn liveness(&self, node_id: &str) -> Option<(bool, u64)> {
        self.liveness.get(node_id).copied()
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn set_paused(&mut self, paused: bool) {
        if self.paused != paused {
            self.log.push(if paused { "paused" } else { "resumed" }, Value::Null);
        }
        self.paused = paused;
    }

    pub fn log(&mut self, kind: &str, detail: Value) {
        self.log.push(kind, detail);
    }

    pub fn log_entries(&self) -> Vec<Value> {
        self.log.entries()
    }

    /// One write that last-write-wins dropped: `detail` names the path and
    /// the timestamp of the entry that was superseded.
    pub fn lww_dropped(&mut self, detail: Value) {
        self.lww_count += 1;
        self.lww.push("lww_dropped", detail);
    }

    /// The `lww_dropped` block of `GET /log`: a total, and the recent entries.
    pub fn lww_report(&self) -> Value {
        json!({ "count": self.lww_count, "entries": self.lww.entries() })
    }

    /// Do we already hold exactly these bytes for this path?
    ///
    /// This is what breaks the echo. Without it, landing a peer's file would
    /// look like a local edit to the next outbound scan, which would republish
    /// it, which would look like a remote insert to the peer, forever.
    pub fn already_have(&self, rel: &Path, hash: &str) -> bool {
        self.published.get(rel).map(String::as_str) == Some(hash)
    }

    /// Withhold every syncable file on disk right now (a joiner's pre-existing
    /// files) until [`SyncState::release_withheld`]. Replaces any earlier set,
    /// and persists the hold so a restart before the club's first sync keeps it.
    pub fn withhold_existing(&mut self) {
        let allow = self.allowlist();
        self.withheld.clear();
        self.club_synced = false;
        self.timed_out = false;
        for rel in self.reader.syncable_files(&allow) {
            if let Ok(bytes) = self.reader.read_file(&rel) {
                self.withheld.insert(rel, blake3::hash(&bytes).to_hex().to_string());
            }
        }
        self.persist_hold();
    }

    /// Reload a hold left by an earlier run. Returns `true` when that run's
    /// club had not yet synced and its timeout had not lapsed, so this run
    /// must keep holding new writes. Withheld files are reloaded either way.
    pub fn load_hold(&mut self) -> bool {
        let Some(path) = self.hold_file.clone() else { return false };
        let Ok(text) = std::fs::read_to_string(&path) else { return false };
        let record: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, path = %path.display(), "join hold unreadable — not withholding");
                return false;
            }
        };
        let synced = record["club_synced"].as_bool().unwrap_or(true);
        self.withheld = record["withheld"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(p, h)| Some((PathBuf::from(p), h.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        self.club_synced = synced;
        self.timed_out = record["timed_out"].as_bool().unwrap_or(false);
        !synced && !self.timed_out
    }

    /// The hold's timeout lapsed before the club synced: new writes publish
    /// from now on. The lapse is persisted so a restart does not hold them
    /// again; the withheld set still waits for the club's first sync.
    pub fn mark_hold_timed_out(&mut self) {
        self.timed_out = true;
        self.persist_hold();
    }

    /// Write the hold to its sidecar while the club has not synced yet.
    fn persist_hold(&self) {
        let Some(path) = &self.hold_file else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let withheld: serde_json::Map<String, Value> = self
            .withheld
            .iter()
            .map(|(p, h)| (p.display().to_string(), json!(h)))
            .collect();
        let record = json!({
            "club_synced": self.club_synced,
            "timed_out": self.timed_out,
            "withheld": withheld,
        });
        if let Err(e) = std::fs::write(path, record.to_string()) {
            tracing::warn!(%e, path = %path.display(), "could not persist the join hold");
        }
    }

    /// Paths still withheld, sorted, for the log.
    pub fn withheld(&self) -> Vec<String> {
        let mut out: Vec<String> = self.withheld.keys().map(|p| p.display().to_string()).collect();
        out.sort();
        out
    }

    pub fn has_withheld(&self) -> bool {
        !self.withheld.is_empty()
    }

    /// The club's first sync has landed: whatever is still withheld may now
    /// publish, because the club's entries were landed before this call. The
    /// sidecar goes too, so a later restart does not hold again. A leave calls
    /// this as well: a one-member club has no first sync to wait for.
    pub fn release_withheld(&mut self) -> usize {
        let n = self.withheld.len();
        self.withheld.clear();
        self.club_synced = true;
        self.timed_out = false;
        if let Some(path) = &self.hold_file {
            if let Err(e) = std::fs::remove_file(path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(%e, path = %path.display(), "could not clear the join hold");
                }
            }
        }
        n
    }

    /// Record bytes we just wrote from a peer as already published, so the
    /// next scan treats them as settled rather than as a local change.
    pub fn mark_published(&mut self, rel: &Path, hash: String) {
        self.published.insert(rel.to_path_buf(), hash);
        self.last_sync = now();
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> (tempfile::TempDir, SyncState) {
        let dir = tempfile::TempDir::new().unwrap();
        let s = SyncState::new("test-node".into(), dir.path().to_path_buf());
        (dir, s)
    }

    #[test]
    fn status_reports_the_node_id() {
        let (_d, s) = state();
        assert_eq!(s.status()["node_id"], "test-node");
    }

    #[test]
    fn the_log_is_bounded() {
        let mut log = EventLog::default();
        for i in 0..(LOG_CAPACITY + 10) {
            log.push("landed", json!(i));
        }
        assert_eq!(log.len(), LOG_CAPACITY);
        // The oldest went first: the ring starts ten events in.
        assert_eq!(log.entries()[0]["detail"], 10);
    }

    #[test]
    fn a_peer_that_drops_is_offline_with_a_last_seen() {
        let (_d, mut s) = state();
        s.set_peers(vec!["a".into()]);
        assert_eq!(s.liveness("a").map(|l| l.0), Some(true));
        s.set_peers(vec![]);
        let (online, seen) = s.liveness("a").unwrap();
        assert!(!online && seen > 0);
    }

    #[test]
    fn publishing_the_same_bytes_twice_reports_no_change() {
        let (dir, mut s) = state();
        std::fs::write(dir.path().join("trims.json"), b"{\"a\":1}").unwrap();

        let first = s.publish(Path::new("trims.json")).unwrap();
        assert_eq!(first["changed"], true);

        let second = s.publish(Path::new("trims.json")).unwrap();
        assert_eq!(second["changed"], false, "an unchanged file must not generate sync traffic");
        assert_eq!(first["hash"], second["hash"]);
    }

    #[test]
    fn editing_a_file_changes_its_hash() {
        let (dir, mut s) = state();
        std::fs::write(dir.path().join("trims.json"), b"{\"a\":1}").unwrap();
        let first = s.publish(Path::new("trims.json")).unwrap();

        std::fs::write(dir.path().join("trims.json"), b"{\"a\":2}").unwrap();
        let second = s.publish(Path::new("trims.json")).unwrap();

        assert_eq!(second["changed"], true);
        assert_ne!(first["hash"], second["hash"]);
    }

    #[test]
    fn an_excluded_path_cannot_be_published() {
        let (dir, mut s) = state();
        std::fs::write(dir.path().join("ai.json"), b"{\"key\":\"secret\"}").unwrap();
        assert!(s.publish(Path::new("ai.json")).is_err());
    }

    #[test]
    fn a_scan_reports_each_file_once_until_it_changes_again() {
        let (dir, mut s) = state();
        std::fs::create_dir_all(dir.path().join("graphs")).unwrap();
        std::fs::write(dir.path().join("trims.json"), b"{}").unwrap();
        std::fs::write(dir.path().join("graphs/one.json"), b"{}").unwrap();

        let first = s.changed_since_last_scan();
        assert_eq!(first.len(), 2);

        assert!(
            s.changed_since_last_scan().is_empty(),
            "a quiet tree must produce an empty scan, or an idle node syncs forever"
        );

        std::fs::write(dir.path().join("trims.json"), b"{\"v\":2}").unwrap();
        assert_eq!(s.changed_since_last_scan(), vec![PathBuf::from("trims.json")]);
    }

    // --- `[sync].allowseed` read live under a running state -----------------

    fn write_allowseed(config_toml: &Path, patterns: &[&str]) {
        let list: Vec<String> = patterns.iter().map(|p| format!("{p:?}")).collect();
        std::fs::write(
            config_toml,
            format!("version = 2\nschema = \"fw/config/2\"\n\n[sync]\nallowseed = [{}]\n", list.join(", ")),
        )
        .unwrap();
    }

    /// The behaviour the S.AH deferral asked to have decided: a path the
    /// operator removes from `[sync].allowseed` while the engine is running
    /// stops being published on the next scan, stays on disk, and comes back
    /// the moment the pattern is restored.
    #[test]
    fn narrowing_allowseed_mid_session_stops_publishing_without_deleting_anything() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfg_dir = tempfile::TempDir::new().unwrap();
        let config_toml = cfg_dir.path().join("config.toml");
        let mut s = SyncState::new("test-node".into(), dir.path().to_path_buf())
            .with_config(config_toml.clone());
        std::fs::create_dir_all(dir.path().join("graphs")).unwrap();
        std::fs::write(dir.path().join("trims.json"), b"{}").unwrap();
        std::fs::write(dir.path().join("graphs/one.json"), b"{}").unwrap();

        // No config.toml yet: the builtin list, both files.
        assert_eq!(s.changed_since_last_scan().len(), 2);

        // A hand edit drops graphs from the list. The next tick sees the
        // graph change and ignores it; the file is untouched.
        write_allowseed(&config_toml, &["trims.json"]);
        std::fs::write(dir.path().join("graphs/one.json"), b"{\"v\":2}").unwrap();
        assert!(s.changed_since_last_scan().is_empty(), "an excluded edit must not publish");
        assert!(dir.path().join("graphs/one.json").exists(), "never deleted locally");
        assert!(s.publish(Path::new("graphs/one.json")).is_err(), "nor on demand");

        // Restoring the pattern picks the edit up like any new file.
        write_allowseed(&config_toml, &["trims.json", "graphs/*.json"]);
        assert_eq!(s.changed_since_last_scan(), vec![PathBuf::from("graphs/one.json")]);
    }

    /// A pattern that never existed in the builtin list works the same way:
    /// added to `config.toml`, the file syncs on the next scan.
    #[test]
    fn widening_allowseed_mid_session_picks_up_new_paths_on_the_next_scan() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfg_dir = tempfile::TempDir::new().unwrap();
        let config_toml = cfg_dir.path().join("config.toml");
        let mut s = SyncState::new("test-node".into(), dir.path().to_path_buf())
            .with_config(config_toml.clone());
        std::fs::create_dir_all(dir.path().join("notes")).unwrap();
        std::fs::write(dir.path().join("notes/today.md"), b"#").unwrap();

        assert!(s.changed_since_last_scan().is_empty(), "not in the builtin list");
        write_allowseed(&config_toml, &["notes/*.md"]);
        assert_eq!(s.changed_since_last_scan(), vec![PathBuf::from("notes/today.md")]);
    }

    /// A config.toml mid-edit (unparseable) syncs nothing for that tick and
    /// resumes when it parses again — never a guess, never a delete.
    #[test]
    fn an_unparseable_config_toml_syncs_nothing_until_it_parses_again() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfg_dir = tempfile::TempDir::new().unwrap();
        let config_toml = cfg_dir.path().join("config.toml");
        let mut s = SyncState::new("test-node".into(), dir.path().to_path_buf())
            .with_config(config_toml.clone());
        std::fs::write(dir.path().join("trims.json"), b"{}").unwrap();

        std::fs::write(&config_toml, "this is not toml = = =").unwrap();
        assert!(s.changed_since_last_scan().is_empty());
        assert!(s.allowlist().patterns().is_empty());

        std::fs::remove_file(&config_toml).unwrap();
        assert_eq!(s.changed_since_last_scan(), vec![PathBuf::from("trims.json")]);
    }
}
