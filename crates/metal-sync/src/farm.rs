//! The render farm's conductor state: an export split into frame-range
//! shards, handed to workers, taken back when a worker goes quiet.
//!
//! This file is the pure half (no network, no clock of its own: every call
//! is handed `now` in milliseconds), so assign / heartbeat / timeout / requeue
//! / delivery / join order are tested without a node. `farm_net.rs` is the
//! `fw/farm/1` protocol and the two loops that drive it.
//!
//! # The life of a shard
//!
//! ```text
//! Queued --claim(worker)--> Claimed --deliver--> Done
//!    ^                         |
//!    +---- expire / fail ------+     (3 missed 2 s heartbeats, or the
//!                                     worker said it failed)
//! ```
//!
//! A worker holds at most one shard. A heartbeat from anyone but the shard's
//! current worker is refused, which is how a worker learns its shard was given
//! away while it was away: it stops rendering. A delivery from a worker that no
//! longer holds the shard is refused for the same reason, so a late finisher
//! cannot overwrite the shard's second render.
//!
//! The whole [`Farm`] is `data/render/queue.json` (synced). A `Claimed` shard
//! read back from disk is requeued: the heartbeats that justified the claim
//! died with the process that wrote it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Workers beat this often.
pub const BEAT_MS: u64 = 2000;
/// Shard size when `[farm].shard_frames` says nothing.
pub const DEFAULT_SHARD_FRAMES: u64 = 300;
/// Beats a worker may miss when `[farm].missed_beats` says nothing.
pub const DEFAULT_MISSED_BEATS: u32 = 3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ShardState {
    Queued,
    Claimed { worker: String, beat_ms: u64 },
    /// Delivered: `rel` is the file under the data root
    /// (`reels/<reel>/verified/<job>-<index>.mp4`).
    Done { worker: String, rel: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shard {
    pub index: usize,
    /// Frames `[from, to)` of the export.
    pub from: u64,
    pub to: u64,
    #[serde(flatten)]
    pub state: ShardState,
    /// How many times it has been claimed.
    #[serde(default)]
    pub attempts: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    /// Where the pieces land and the joined file is named after.
    pub reel: String,
    /// The `POST /export/video` request this job was split from; a shard runs
    /// it with `range` and `path` replaced.
    pub body: Value,
    pub shards: Vec<Shard>,
}

/// What a claim hands a worker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Assignment {
    pub job: String,
    pub reel: String,
    pub index: usize,
    pub from: u64,
    pub to: u64,
    pub body: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FarmError {
    NoSuchJob,
    NoSuchShard,
    /// The shard is not (or no longer) this worker's.
    NotYours,
    /// A job needs at least one frame and a name that is a plain path segment.
    BadJob(String),
}

impl std::fmt::Display for FarmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FarmError::NoSuchJob => write!(f, "no such job"),
            FarmError::NoSuchShard => write!(f, "no such shard"),
            FarmError::NotYours => write!(f, "that shard is not yours"),
            FarmError::BadJob(why) => write!(f, "{why}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Farm {
    /// Oldest first: a worker is served the oldest job that has a queued shard.
    pub jobs: Vec<Job>,
    #[serde(default = "default_missed")]
    pub missed_beats: u32,
}

fn default_missed() -> u32 {
    DEFAULT_MISSED_BEATS
}

impl Default for Farm {
    fn default() -> Farm {
        Farm { jobs: Vec::new(), missed_beats: DEFAULT_MISSED_BEATS }
    }
}

/// A plain name: it becomes a path segment under `reels/` and a file stem.
pub fn is_plain(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `reels/<reel>/verified/<job>-<index>.mp4`.
pub fn verified_rel(reel: &str, job: &str, index: usize) -> String {
    format!("reels/{reel}/verified/{job}-{index}.mp4")
}

impl Farm {
    pub fn new(missed_beats: u32) -> Farm {
        Farm { jobs: Vec::new(), missed_beats: missed_beats.max(1) }
    }

    /// Splits frames `[from, to)` of `body` into shards of `shard_frames`
    /// (the last one takes the remainder) and queues the job.
    pub fn submit(&mut self, id: &str, reel: &str, body: Value, from: u64, to: u64, shard_frames: u64) -> Result<(), FarmError> {
        if !is_plain(id) || !is_plain(reel) {
            return Err(FarmError::BadJob("a job and a reel are plain names".into()));
        }
        if to <= from {
            return Err(FarmError::BadJob("a job needs at least one frame".into()));
        }
        if self.jobs.iter().any(|j| j.id == id) {
            return Err(FarmError::BadJob(format!("job {id:?} already exists")));
        }
        let step = shard_frames.max(1);
        let mut shards = Vec::new();
        let mut at = from;
        while at < to {
            let end = (at + step).min(to);
            shards.push(Shard { index: shards.len(), from: at, to: end, state: ShardState::Queued, attempts: 0 });
            at = end;
        }
        self.jobs.push(Job { id: id.to_string(), reel: reel.to_string(), body, shards });
        Ok(())
    }

    /// The oldest queued shard goes to `worker`, who holds at most one.
    pub fn claim(&mut self, worker: &str, now: u64) -> Option<Assignment> {
        let holds = self
            .jobs
            .iter()
            .flat_map(|j| &j.shards)
            .any(|s| matches!(&s.state, ShardState::Claimed { worker: w, .. } if w == worker));
        if holds {
            return None;
        }
        for job in &mut self.jobs {
            if let Some(shard) = job.shards.iter_mut().find(|s| s.state == ShardState::Queued) {
                shard.state = ShardState::Claimed { worker: worker.to_string(), beat_ms: now };
                shard.attempts += 1;
                return Some(Assignment {
                    job: job.id.clone(),
                    reel: job.reel.clone(),
                    index: shard.index,
                    from: shard.from,
                    to: shard.to,
                    body: job.body.clone(),
                });
            }
        }
        None
    }

    fn shard_mut(&mut self, job: &str, index: usize) -> Result<&mut Shard, FarmError> {
        let job = self.jobs.iter_mut().find(|j| j.id == job).ok_or(FarmError::NoSuchJob)?;
        job.shards.get_mut(index).ok_or(FarmError::NoSuchShard)
    }

    /// `worker` is alive and still rendering. `Err(NotYours)` tells it to stop.
    pub fn heartbeat(&mut self, worker: &str, job: &str, index: usize, now: u64) -> Result<(), FarmError> {
        let shard = self.shard_mut(job, index)?;
        match &mut shard.state {
            ShardState::Claimed { worker: w, beat_ms } if w == worker => {
                *beat_ms = now;
                Ok(())
            }
            _ => Err(FarmError::NotYours),
        }
    }

    /// Puts back every shard whose worker has been quiet for more than
    /// `missed_beats` beats. Returns `(job, index, worker)` for each.
    pub fn expire(&mut self, now: u64) -> Vec<(String, usize, String)> {
        let limit = BEAT_MS * u64::from(self.missed_beats);
        let mut back = Vec::new();
        for job in &mut self.jobs {
            for shard in &mut job.shards {
                if let ShardState::Claimed { worker, beat_ms } = &shard.state {
                    if now.saturating_sub(*beat_ms) > limit {
                        back.push((job.id.clone(), shard.index, worker.clone()));
                        shard.state = ShardState::Queued;
                    }
                }
            }
        }
        back
    }

    /// The worker gave up (its render failed): the shard goes straight back.
    pub fn fail(&mut self, worker: &str, job: &str, index: usize) -> Result<(), FarmError> {
        let shard = self.shard_mut(job, index)?;
        match &shard.state {
            ShardState::Claimed { worker: w, .. } if w == worker => {
                shard.state = ShardState::Queued;
                Ok(())
            }
            _ => Err(FarmError::NotYours),
        }
    }

    /// The worker's file is in place under `rel`: the shard is done.
    pub fn deliver(&mut self, worker: &str, job: &str, index: usize, rel: &str) -> Result<(), FarmError> {
        let shard = self.shard_mut(job, index)?;
        match &shard.state {
            ShardState::Claimed { worker: w, .. } if w == worker => {
                shard.state = ShardState::Done { worker: worker.to_string(), rel: rel.to_string() };
                Ok(())
            }
            _ => Err(FarmError::NotYours),
        }
    }

    /// Whether `worker` may deliver this shard now (checked before the bytes
    /// are taken, so a stale worker's file is never written).
    pub fn may_deliver(&self, worker: &str, job: &str, index: usize) -> bool {
        self.jobs
            .iter()
            .find(|j| j.id == job)
            .and_then(|j| j.shards.get(index))
            .is_some_and(|s| matches!(&s.state, ShardState::Claimed { worker: w, .. } if w == worker))
    }

    /// The shard files in frame order once every shard is done.
    pub fn join_order(&self, job: &str) -> Option<Vec<String>> {
        let job = self.jobs.iter().find(|j| j.id == job)?;
        let mut shards: Vec<&Shard> = job.shards.iter().collect();
        shards.sort_by_key(|s| s.from);
        shards
            .into_iter()
            .map(|s| match &s.state {
                ShardState::Done { rel, .. } => Some(rel.clone()),
                _ => None,
            })
            .collect()
    }

    /// Jobs whose every shard is done, oldest first.
    pub fn finished(&self) -> Vec<String> {
        self.jobs.iter().filter(|j| self.join_order(&j.id).is_some()).map(|j| j.id.clone()).collect()
    }

    /// The job joined and delivered: it leaves the queue.
    pub fn remove(&mut self, job: &str) -> bool {
        let before = self.jobs.len();
        self.jobs.retain(|j| j.id != job);
        self.jobs.len() != before
    }

    /// Queued / claimed / done counts of a job, for `GET /farm`.
    pub fn progress(&self, job: &str) -> Option<(usize, usize, usize)> {
        let job = self.jobs.iter().find(|j| j.id == job)?;
        let count = |f: fn(&ShardState) -> bool| job.shards.iter().filter(|s| f(&s.state)).count();
        Some((
            count(|s| matches!(s, ShardState::Queued)),
            count(|s| matches!(s, ShardState::Claimed { .. })),
            count(|s| matches!(s, ShardState::Done { .. })),
        ))
    }

    /// A farm read back from disk: claims lose their heartbeats with the
    /// process that made them, so they go back in the queue.
    pub fn requeue_claims(&mut self) {
        for shard in self.jobs.iter_mut().flat_map(|j| j.shards.iter_mut()) {
            if matches!(shard.state, ShardState::Claimed { .. }) {
                shard.state = ShardState::Queued;
            }
        }
    }
}

/// `data/render/queue.json`.
pub fn queue_path(data_root: &std::path::Path) -> std::path::PathBuf {
    data_root.join("render").join("queue.json")
}

/// Reads the queue (empty when the file is missing or unreadable); claims are
/// requeued ([`Farm::requeue_claims`]).
pub fn load(data_root: &std::path::Path, missed_beats: u32) -> Farm {
    let mut farm: Farm = std::fs::read_to_string(queue_path(data_root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| Farm::new(missed_beats));
    farm.missed_beats = missed_beats.max(1);
    farm.requeue_claims();
    farm
}

/// Writes the queue (via a temp file and a rename).
pub fn save(data_root: &std::path::Path, farm: &Farm) -> Result<(), String> {
    let path = queue_path(data_root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(farm).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn farm_of(frames: u64, shard: u64) -> Farm {
        let mut f = Farm::default();
        f.submit("j1", "show", json!({ "device": "out-1" }), 0, frames, shard).unwrap();
        f
    }

    #[test]
    fn a_job_splits_into_ordered_shards_with_the_remainder_last() {
        let f = farm_of(700, 300);
        let spans: Vec<(u64, u64)> = f.jobs[0].shards.iter().map(|s| (s.from, s.to)).collect();
        assert_eq!(spans, [(0, 300), (300, 600), (600, 700)]);
        let mut f = f;
        assert!(f.submit("j1", "show", json!({}), 0, 10, 5).is_err(), "a job id is unique");
        assert!(f.submit("../x", "show", json!({}), 0, 10, 5).is_err());
        assert!(f.submit("j2", "show", json!({}), 10, 10, 5).is_err());
    }

    #[test]
    fn claims_go_to_the_oldest_queued_shard_and_a_worker_holds_one() {
        let mut f = farm_of(600, 300);
        let a = f.claim("w1", 0).unwrap();
        assert_eq!((a.index, a.from, a.to), (0, 0, 300));
        assert!(f.claim("w1", 10).is_none(), "one shard per worker");
        let b = f.claim("w2", 10).unwrap();
        assert_eq!(b.index, 1);
        assert!(f.claim("w3", 20).is_none(), "nothing left to claim");
    }

    #[test]
    fn a_heartbeat_keeps_a_shard_and_three_missed_ones_lose_it() {
        let mut f = farm_of(300, 300);
        f.claim("w1", 0).unwrap();
        assert!(f.expire(BEAT_MS * 3).is_empty(), "exactly three beats late is still alive");
        f.heartbeat("w1", "j1", 0, 5000).unwrap();
        assert!(f.expire(5000 + BEAT_MS * 3).is_empty(), "the beat moved the deadline");
        let back = f.expire(5000 + BEAT_MS * 3 + 1);
        assert_eq!(back, [("j1".to_string(), 0, "w1".to_string())]);
        assert_eq!(f.progress("j1"), Some((1, 0, 0)));
    }

    #[test]
    fn a_requeued_shard_goes_to_another_worker_and_the_first_is_told_to_stop() {
        let mut f = farm_of(300, 300);
        f.claim("w1", 0).unwrap();
        f.expire(100_000);
        let again = f.claim("w2", 100_000).unwrap();
        assert_eq!(again.index, 0);
        assert_eq!(f.jobs[0].shards[0].attempts, 2);
        assert_eq!(f.heartbeat("w1", "j1", 0, 100_001), Err(FarmError::NotYours), "w1 must stop");
        assert!(!f.may_deliver("w1", "j1", 0));
        assert_eq!(f.deliver("w1", "j1", 0, "x"), Err(FarmError::NotYours), "a late finisher does not win");
        assert!(f.may_deliver("w2", "j1", 0));
        assert_eq!(f.heartbeat("w2", "nope", 0, 0), Err(FarmError::NoSuchJob));
        assert_eq!(f.heartbeat("w2", "j1", 9, 0), Err(FarmError::NoSuchShard));
    }

    #[test]
    fn a_failed_shard_is_requeued_at_once_and_only_by_its_worker() {
        let mut f = farm_of(300, 300);
        f.claim("w1", 0).unwrap();
        assert_eq!(f.fail("w2", "j1", 0), Err(FarmError::NotYours));
        f.fail("w1", "j1", 0).unwrap();
        assert!(f.claim("w2", 1).is_some());
    }

    #[test]
    fn delivery_joins_in_frame_order_whatever_order_they_finish_in() {
        let mut f = farm_of(900, 300);
        for w in ["w1", "w2", "w3"] {
            f.claim(w, 0).unwrap();
        }
        assert!(f.join_order("j1").is_none(), "not until every shard is done");
        // Finish out of order: 2, 0, 1.
        for (w, i) in [("w3", 2), ("w1", 0), ("w2", 1)] {
            f.deliver(w, "j1", i, &verified_rel("show", "j1", i)).unwrap();
        }
        assert_eq!(f.finished(), ["j1"]);
        assert_eq!(
            f.join_order("j1").unwrap(),
            ["reels/show/verified/j1-0.mp4", "reels/show/verified/j1-1.mp4", "reels/show/verified/j1-2.mp4"]
        );
        assert!(f.remove("j1"));
        assert!(f.jobs.is_empty());
    }

    #[test]
    fn the_queue_file_round_trips_and_a_restart_requeues_claims() {
        let dir = std::env::temp_dir().join(format!("fw-farm-queue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut f = farm_of(600, 300);
        f.claim("w1", 5).unwrap();
        f.claim("w2", 5).unwrap();
        f.deliver("w2", "j1", 1, "reels/show/verified/j1-1.mp4").unwrap();
        save(&dir, &f).unwrap();
        let back = load(&dir, 3);
        assert_eq!(back.progress("j1"), Some((1, 0, 1)), "the claim was requeued, the delivery kept");
        assert!(queue_path(&dir).ends_with("render/queue.json"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
