//! O.8: one owner per graph, taken and released explicitly.
//!
//! metal-sync is last-writer-wins per key, which is fine for one author and
//! wrong for two harnesses (two machines) editing the same graph: each
//! save silently clobbers the other's. So a graph can be *owned*: an owner
//! record `graphs/<id>.owner` sits beside `graphs/<id>.json` and syncs like
//! it (`graphs/*.owner` is on the allowlist). While a record names another
//! peer, every graph write on this machine — the shell's routes and the
//! harness tools alike — is refused with [`Refusal`]. No record means the
//! graph is free, which is today's behaviour unchanged.
//!
//! The record lives *in the graphs directory* on purpose: whatever isolation
//! a caller applies to `graphs/` (metal-perform's per-process scratch copy in
//! tests) applies to its owners for free. The extension is `.owner`, not
//! `.json`, so nothing that lists `graphs/*.json` mistakes one for a graph.
//!
//! **Honest limits.** The record itself is synced LWW: two peers taking the
//! same free graph within one sync round both believe they won until the
//! records converge. This is a cooperative lock between harnesses that ask
//! first, not consensus.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Who owns a graph, and since when (unix seconds).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Owner {
    pub owner: String,
    pub taken_at: u64,
}

/// Why a take/release/write was refused — the clear error the spec asks for.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub graph: String,
    pub owner: String,
    pub me: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "graph {:?} is owned by {:?} (this machine is {:?}): ask them to release it first",
            self.graph, self.owner, self.me
        )
    }
}

static IDENTITY: RwLock<Option<String>> = RwLock::new(None);

/// Pin this process's identity (metal-dash's `--node`). Wins over the defaults.
pub fn set_local_identity(name: &str) {
    if let Ok(mut id) = IDENTITY.write() {
        *id = Some(name.to_string()).filter(|s| !s.is_empty());
    }
}

/// This machine's name as an owner: what [`set_local_identity`] pinned, else
/// `FW_NODE`, else the hostname (the same default as metal-perform's
/// `local_node_default`, so metal-sync — a separate process — agrees with
/// metal-dash unless `--node` was given without `FW_NODE`).
pub fn local_identity() -> String {
    if let Some(id) = IDENTITY.read().ok().and_then(|g| g.clone()) {
        return id;
    }
    if let Ok(n) = std::env::var("FW_NODE") {
        if !n.is_empty() {
            return n;
        }
    }
    if let Ok(text) = metal_vfs::read_to_string("/etc/hostname") {
        let t = text.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    match std::env::var("HOSTNAME") {
        Ok(n) if !n.is_empty() => n,
        _ => "local".to_string(),
    }
}

pub fn owner_path(graphs_dir: &Path, id: &str) -> PathBuf {
    graphs_dir.join(format!("{id}.owner"))
}

/// The current owner, or `None` for a free graph. An unreadable record is
/// treated as free rather than locking the graph forever.
pub fn status(graphs_dir: &Path, id: &str) -> Option<Owner> {
    let text = metal_vfs::read_to_string(owner_path(graphs_dir, id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Take `id` for `me`. Re-taking your own graph is a no-op; taking one
/// another peer owns is refused.
pub fn take(graphs_dir: &Path, id: &str, me: &str) -> Result<Owner, Refusal> {
    if let Some(o) = status(graphs_dir, id) {
        if o.owner != me {
            return Err(Refusal { graph: id.into(), owner: o.owner, me: me.into() });
        }
        return Ok(o);
    }
    let taken_at = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let o = Owner { owner: me.to_string(), taken_at };
    let _ = metal_vfs::create_dir_all(graphs_dir);
    let path = owner_path(graphs_dir, id);
    let tmp = path.with_extension("owner.tmp");
    let body = serde_json::to_string(&o).expect("owner serializes");
    if metal_vfs::write(&tmp, body).and_then(|_| metal_vfs::rename(&tmp, &path)).is_err() {
        let _ = metal_vfs::remove_file(&tmp);
    }
    Ok(o)
}

/// Release `id`. Only the owner may; releasing a free graph is a no-op.
pub fn release(graphs_dir: &Path, id: &str, me: &str) -> Result<(), Refusal> {
    check_write(graphs_dir, id, me)?;
    let _ = metal_vfs::remove_file(owner_path(graphs_dir, id));
    Ok(())
}

/// The write gate: `Ok` when the graph is free or owned by `me`.
pub fn check_write(graphs_dir: &Path, id: &str, me: &str) -> Result<(), Refusal> {
    match status(graphs_dir, id) {
        Some(o) if o.owner != me => Err(Refusal { graph: id.into(), owner: o.owner, me: me.into() }),
        _ => Ok(()),
    }
}

/// The JSON a status route or tool answers with.
pub fn status_json(graphs_dir: &Path, id: &str, me: &str) -> String {
    let v = match status(graphs_dir, id) {
        Some(o) => serde_json::json!({
            "graph": id, "owner": o.owner, "taken_at": o.taken_at,
            "mine": o.owner == me, "me": me,
        }),
        None => serde_json::json!({ "graph": id, "owner": null, "mine": false, "me": me }),
    };
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("metal-graph-owner-{name}-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&d);
        metal_vfs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_free_graph_is_writable_by_anyone() {
        let d = dir("free");
        assert!(status(&d, "g").is_none());
        assert!(check_write(&d, "g", "a").is_ok());
        assert!(check_write(&d, "g", "b").is_ok());
    }

    #[test]
    fn taking_an_owned_graph_is_refused() {
        let d = dir("take");
        take(&d, "g", "a").unwrap();
        let err = take(&d, "g", "b").unwrap_err();
        assert_eq!(err.owner, "a");
        assert!(err.to_string().contains("owned by \"a\""));
        // Re-taking your own is fine.
        assert_eq!(take(&d, "g", "a").unwrap().owner, "a");
    }

    #[test]
    fn the_owner_writes_and_releases_a_non_owner_cannot() {
        let d = dir("release");
        take(&d, "g", "a").unwrap();
        assert!(check_write(&d, "g", "a").is_ok());
        assert!(check_write(&d, "g", "b").is_err());
        assert!(release(&d, "g", "b").is_err());
        release(&d, "g", "a").unwrap();
        assert!(check_write(&d, "g", "b").is_ok());
    }
}
