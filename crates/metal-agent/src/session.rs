//! The shared agent session: `data/agent/session.jsonl`, one JSON entry per line,
//! written by the peers of an install (the `super` and the `producer` roles).
//!
//! Each entry carries a monotonically increasing `seq`. A writer names the
//! `seq` it last read (its base); the append is refused when the file's head is
//! not that base (**stale-seq refusal**), so a writer that acted on an old view
//! re-reads before it writes. A tool call marked run-once (`run_once` set to its
//! call id) runs on exactly one machine: the claim is an entry in the same file,
//! and a second claim for the same id is refused (**run-once dedupe**).
//!
//! **What this does not do.** The file ships with the rest of `data/` through
//! `metal-sync`, which converges whole files last-write-wins (see that crate's
//! module doc). Two peers that append while partitioned each keep their own
//! head, and the later file replaces the earlier one, so an entry the other
//! side wrote can be lost. The stale-seq rule protects one shared file on one
//! machine and a file that replicates before it is edited; merging two diverged
//! logs is `metal-sync`'s `session_merge`: on landing it unions both files by
//! `(author, seq)`, orders them by `seq` and keeps a run-once id once.
//! The append is serialised in this process ([`LOCK`]) and across processes by
//! a file lock on `session.jsonl.lock`, the same one `session_merge` takes.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Who wrote the entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Super,
    Producer,
}

/// One line of the session file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub seq: u64,
    /// The machine that wrote it: with `seq` it names the entry across peers,
    /// which is what `metal-sync` merges the logs by.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    pub role: Role,
    /// The run-once call id, when this entry is a run-once claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_once: Option<String>,
    pub body: Value,
}

/// Why an append did not happen. Neither refusal writes anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The writer's base `seq` is not the file's head.
    Stale { base: u64, head: u64 },
    /// A run-once claim for this id is already in the file.
    AlreadyRan { id: String, seq: u64 },
    /// The file could not be read or written.
    Io(String),
}

/// The session file under a data root: `<data>/agent/session.jsonl`.
pub fn path(data_root: &Path) -> PathBuf {
    data_root.join("agent").join("session.jsonl")
}

/// Serialises the check-then-append in this process; [`file_lock`] covers
/// the other processes.
static LOCK: Mutex<()> = Mutex::new(());

/// The cross-process lock on the log: an exclusive `flock` on the sidecar
/// `<log>.lock` (the file `metal-sync` takes when it lands a peer's copy),
/// released when the returned file is dropped.
fn file_lock(path: &Path) -> Result<std::fs::File, Refusal> {
    let io = |e: std::io::Error| Refusal::Io(e.to_string());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_file_name(name))
        .map_err(io)?;
    file.lock().map_err(io)?;
    Ok(file)
}

/// Every entry in the file, oldest first. A missing file is an empty session.
pub fn read(path: &Path) -> Result<Vec<Entry>, String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut out = Vec::new();
    for (n, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: Entry =
            serde_json::from_str(&line).map_err(|e| format!("line {}: {e}", n + 1))?;
        out.push(entry);
    }
    Ok(out)
}

/// The file's head: the highest `seq`, or 0 for an empty session.
pub fn head(entries: &[Entry]) -> u64 {
    entries.iter().map(|e| e.seq).max().unwrap_or(0)
}

/// Appends `body` as `role`, provided the file's head is still `base`. The new
/// entry's `seq` is `base + 1`.
pub fn append(path: &Path, base: u64, role: Role, body: Value) -> Result<Entry, Refusal> {
    write_entry(path, base, role, None, body)
}

/// Claims run-once call `id`: appends a claim unless the file already holds one
/// for `id`. The stale rule applies first, so a caller acting on an old view is
/// told to re-read before it learns whether the id ran.
pub fn claim_run_once(
    path: &Path,
    base: u64,
    role: Role,
    id: &str,
    body: Value,
) -> Result<Entry, Refusal> {
    write_entry(path, base, role, Some(id), body)
}

fn write_entry(
    path: &Path,
    base: u64,
    role: Role,
    run_once: Option<&str>,
    body: Value,
) -> Result<Entry, Refusal> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _file_lock = file_lock(path)?;
    let entries = read(path).map_err(Refusal::Io)?;
    let head = head(&entries);
    if head != base {
        return Err(Refusal::Stale { base, head });
    }
    if let Some(id) = run_once {
        if let Some(prev) = entries.iter().find(|e| e.run_once.as_deref() == Some(id)) {
            return Err(Refusal::AlreadyRan { id: id.to_string(), seq: prev.seq });
        }
    }
    let entry = Entry {
        seq: base + 1,
        author: metal_graph::owner::local_identity(),
        role,
        run_once: run_once.map(str::to_string),
        body,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Refusal::Io(e.to_string()))?;
    }
    let mut line = serde_json::to_string(&entry).map_err(|e| Refusal::Io(e.to_string()))?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| Refusal::Io(e.to_string()))?;
    file.write_all(line.as_bytes()).map_err(|e| Refusal::Io(e.to_string()))?;
    Ok(entry)
}

/// The refusal as the JSON a caller can act on.
pub fn refusal_json(r: &Refusal) -> Value {
    match r {
        Refusal::Stale { base, head } => {
            json!({ "ok": false, "code": "stale_seq", "base": base, "head": head })
        }
        Refusal::AlreadyRan { id, seq } => {
            json!({ "ok": false, "code": "already_ran", "id": id, "seq": seq })
        }
        Refusal::Io(e) => json!({ "ok": false, "code": "io", "error": e }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory per test, under the system temp dir (rule 8: never the
    /// real `data/`).
    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("metal-agent-session-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_session_is_empty_and_its_head_is_zero() {
        let dir = tempdir("missing");
        let p = path(&dir);
        assert_eq!(read(&p).unwrap(), Vec::new());
        assert_eq!(head(&read(&p).unwrap()), 0);
    }

    #[test]
    fn appends_grow_the_seq_by_one_and_keep_the_role() {
        let dir = tempdir("append");
        let p = path(&dir);
        let a = append(&p, 0, Role::Super, json!({"say": "one"})).expect("first");
        let b = append(&p, 1, Role::Producer, json!({"say": "two"})).expect("second");
        assert_eq!((a.seq, b.seq), (1, 2));
        let all = read(&p).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].role, Role::Super);
        assert_eq!(all[1].role, Role::Producer);
        assert_eq!(all[1].body, json!({"say": "two"}));
        assert_eq!(head(&all), 2);
    }

    #[test]
    fn a_stale_base_is_refused_and_writes_nothing() {
        let dir = tempdir("stale");
        let p = path(&dir);
        append(&p, 0, Role::Super, json!({"n": 1})).unwrap();
        append(&p, 1, Role::Super, json!({"n": 2})).unwrap();
        let before = std::fs::read_to_string(&p).unwrap();

        // A writer that last saw seq 0 is stale: the head is 2.
        let refused = append(&p, 0, Role::Producer, json!({"n": 9}));
        assert_eq!(refused, Err(Refusal::Stale { base: 0, head: 2 }));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), before, "the file is unchanged");
        assert_eq!(refusal_json(&refused.unwrap_err())["code"], json!("stale_seq"));
    }

    #[test]
    fn a_run_once_call_runs_on_one_claim_and_the_second_is_refused() {
        let dir = tempdir("once");
        let p = path(&dir);
        let first = claim_run_once(&p, 0, Role::Super, "call-7", json!({"tool": "save"}))
            .expect("the first claim runs");
        assert_eq!(first.run_once.as_deref(), Some("call-7"));

        // The second machine read the file before the first claim: its base is
        // stale, so it re-reads and does not learn anything new from the refusal.
        let stale = claim_run_once(&p, 0, Role::Producer, "call-7", json!({"tool": "save"}));
        assert_eq!(stale, Err(Refusal::Stale { base: 0, head: 1 }));

        // Re-read, and the id is already claimed.
        let base = head(&read(&p).unwrap());
        let again = claim_run_once(&p, base, Role::Producer, "call-7", json!({"tool": "save"}));
        assert_eq!(again, Err(Refusal::AlreadyRan { id: "call-7".into(), seq: 1 }));
        assert_eq!(read(&p).unwrap().len(), 1, "only the first claim is in the file");
    }

    #[test]
    fn a_different_run_once_id_is_not_a_duplicate() {
        let dir = tempdir("ids");
        let p = path(&dir);
        claim_run_once(&p, 0, Role::Super, "call-1", json!({})).unwrap();
        let second = claim_run_once(&p, 1, Role::Super, "call-2", json!({}));
        assert!(second.is_ok(), "a new id runs");
    }

    #[test]
    fn the_path_helper_names_the_agent_session_under_the_data_root() {
        let root = Path::new("/d");
        assert_eq!(path(root), Path::new("/d/agent/session.jsonl"));
    }

    /// An append names its machine, and waits for the cross-process file lock
    /// another holder has taken (the lock `metal-sync` takes to land a peer).
    #[test]
    fn an_append_carries_its_author_and_waits_for_the_file_lock() {
        let dir = tempdir("lock");
        let p = path(&dir);
        let e = append(&p, 0, Role::Super, json!({})).unwrap();
        assert_eq!(e.author, metal_graph::owner::local_identity());
        let held = file_lock(&p).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let q = p.clone();
        let t = std::thread::spawn(move || {
            let r = append(&q, 1, Role::Super, json!({}));
            tx.send(r.is_ok()).unwrap();
        });
        assert!(rx.recv_timeout(std::time::Duration::from_millis(300)).is_err(), "blocked by the lock");
        drop(held);
        assert!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), "proceeds once released");
        t.join().unwrap();
    }
}
