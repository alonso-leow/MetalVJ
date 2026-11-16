//! The append-merge for the shared agent session log, `agent/session.jsonl`.
//!
//! Every other synced file converges whole-file last-write-wins. The session
//! log is an append log written by several machines, so two peers that append
//! while partitioned would lose each other's entries that way. Landing a peer's
//! copy of it merges instead: the union of both files' entries by
//! `(author, seq)`, ordered by `seq` (ties by author), with a run-once id kept
//! once (the earliest claim). The result is a pure function of the two entry
//! sets, so every peer that has seen the same entries holds the same bytes and
//! the log settles.
//!
//! Appenders take the same cross-process lock ([`lock`]) around their
//! check-then-append, so a landing never interleaves with a local append.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The synced path of the log, under the data root.
pub const SESSION_REL: &str = "agent/session.jsonl";

/// Whether `rel` is the shared session log.
pub fn is_session(rel: &Path) -> bool {
    rel == Path::new(SESSION_REL)
}

/// The sidecar the lock is taken on. It is not an allowlisted path, so it is
/// never synced.
pub fn lock_path(log: &Path) -> PathBuf {
    let mut name = log.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".lock");
    log.with_file_name(name)
}

/// An exclusive cross-process lock on the log, released when dropped. Blocks
/// until it is free.
pub struct Guard(std::fs::File);

pub fn lock(log: &Path) -> std::io::Result<Guard> {
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(lock_path(log))?;
    file.lock()?;
    Ok(Guard(file))
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn entries(text: &str) -> impl Iterator<Item = Value> + '_ {
    text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["seq"].is_u64())
}

/// The merge of two copies of the log (see the module doc). A line that is not
/// an entry (no numeric `seq`) is dropped.
pub fn merge(a: &str, b: &str) -> String {
    let mut by_key: BTreeMap<(u64, String), Value> = BTreeMap::new();
    for e in entries(a).chain(entries(b)) {
        let key = (e["seq"].as_u64().unwrap_or(0), e["author"].as_str().unwrap_or("").to_string());
        by_key.entry(key).or_insert(e);
    }
    let mut ran = std::collections::HashSet::new();
    let mut out = String::new();
    for e in by_key.into_values() {
        if let Some(id) = e["run_once"].as_str() {
            if !ran.insert(id.to_string()) {
                continue;
            }
        }
        out.push_str(&e.to_string());
        out.push('\n');
    }
    out
}

/// Lands `incoming` over the log at `path`: merged with what is there, under
/// the lock, written atomically.
pub fn land(path: &Path, incoming: &str) -> std::io::Result<()> {
    let _guard = lock(path)?;
    let local = std::fs::read_to_string(path).unwrap_or_default();
    let merged = merge(&local, incoming);
    let tmp = path.with_extension("jsonl.metal-sync-merge");
    std::fs::write(&tmp, merged)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(author: &str, seq: u64, run_once: Option<&str>) -> String {
        let mut v = serde_json::json!({ "seq": seq, "role": "super", "author": author, "body": {} });
        if let Some(id) = run_once {
            v["run_once"] = id.into();
        }
        format!("{v}\n")
    }

    fn keys(text: &str) -> Vec<(u64, String)> {
        entries(text).map(|e| (e["seq"].as_u64().unwrap(), e["author"].as_str().unwrap().to_string())).collect()
    }

    #[test]
    fn partitioned_appends_are_both_kept_in_seq_order() {
        let a = line("a", 1, None) + &line("a", 2, None);
        let b = line("a", 1, None) + &line("b", 2, None) + &line("b", 3, None);
        let m = merge(&a, &b);
        assert_eq!(keys(&m), [(1, "a".into()), (2, "a".into()), (2, "b".into()), (3, "b".into())]);
    }

    #[test]
    fn the_merge_is_commutative_idempotent_and_drops_what_is_not_an_entry() {
        let a = line("a", 1, None) + "not json\n" + &line("a", 3, None);
        let b = line("b", 2, None) + "{\"no\":\"seq\"}\n";
        assert_eq!(merge(&a, &b), merge(&b, &a));
        let m = merge(&a, &b);
        assert_eq!(merge(&m, &m), m);
        assert_eq!(merge(&m, &a), m);
        assert_eq!(keys(&m).len(), 3);
    }

    #[test]
    fn a_run_once_id_claimed_on_two_machines_stays_once() {
        let a = line("a", 2, Some("call-7"));
        let b = line("b", 2, Some("call-7")) + &line("b", 3, Some("call-8"));
        let m = merge(&a, &b);
        assert_eq!(keys(&m), [(2, "a".into()), (3, "b".into())], "the earliest claim (ties by author) wins");
    }

    #[test]
    fn landing_merges_into_the_file_on_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = dir.path().join(SESSION_REL);
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, line("a", 1, None) + &line("a", 2, None)).unwrap();
        land(&log, &(line("b", 2, None) + &line("b", 3, None))).unwrap();
        assert_eq!(keys(&std::fs::read_to_string(&log).unwrap()).len(), 4);
        assert!(!log.with_extension("jsonl.metal-sync-merge").exists());
        assert!(is_session(Path::new(SESSION_REL)) && !is_session(Path::new("agent/roles.json")));
    }

    #[test]
    fn the_lock_excludes_a_second_holder_until_released() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = dir.path().join(SESSION_REL);
        let first = lock(&log).unwrap();
        let probe = std::fs::OpenOptions::new().write(true).open(lock_path(&log)).unwrap();
        assert!(matches!(probe.try_lock(), Err(std::fs::TryLockError::WouldBlock)), "held by the first guard");
        drop(first);
        assert!(probe.try_lock().is_ok(), "free once the guard is dropped");
    }
}
