//! The lanes panel's off-thread work: the preview's POSTs and the song's
//! fetch, run on one worker thread in the order they were asked for.
//!
//! The UI thread used to make these calls inline, so a frame waited for each
//! one behind the studio's lock (the `lanes` span, ~195 ms). Now a POST is
//! queued and the preview keeps its last picture until that write has landed
//! ([`LaneWorker::landed`]); a song's fetch is queued and the lane keeps its
//! old state until the answer is polled.
//!
//! One queue, one thread, FIFO. A POST whose key is still waiting in the queue
//! takes the newer body and keeps its place (latest-only), so a drag that
//! posts many rows runs at most one of them after the first.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;

use crate::call::{Call, Params};
use crate::shell::panels::lanes::{fetch_song, SongJob, SongLoad};

/// Run after each POST the worker sends, with its path: the read cache's
/// bookkeeping for a write (see `reads::CachedDoor::write_hook`).
pub(crate) type WriteHook = Arc<dyn Fn(&str) + Send + Sync>;

enum Job {
    Post {
        key: String,
        gen: u64,
        path: String,
        query: Vec<(String, String)>,
        body: Vec<u8>,
    },
    Fetch {
        job: SongJob,
        back: Sender<SongLoad>,
    },
}

/// The queue and the generations of its writes. Plain data, so the ordering
/// and coalescing rules are testable without a thread or a door.
#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    /// The generation the next write takes.
    next_gen: u64,
    /// Per key, the highest generation whose write has run.
    landed: HashMap<String, u64>,
    /// Per key, why its last write was refused; cleared by the next success.
    failed: HashMap<String, String>,
}

impl Queue {
    /// Queues a POST under `key` and returns its generation. A POST for the
    /// same key still waiting keeps its place and takes this body instead.
    fn post(&mut self, key: &str, path: &str, query: Vec<(String, String)>, body: Vec<u8>) -> u64 {
        self.next_gen += 1;
        let gen = self.next_gen;
        for job in self.jobs.iter_mut() {
            if let Job::Post { key: k, gen: g, path: p, query: q, body: b } = job {
                if k == key {
                    *g = gen;
                    *p = path.to_string();
                    *q = query;
                    *b = body;
                    return gen;
                }
            }
        }
        self.jobs.push_back(Job::Post {
            key: key.to_string(),
            gen,
            path: path.to_string(),
            query,
            body,
        });
        gen
    }

    fn fetch(&mut self, job: SongJob, back: Sender<SongLoad>) {
        self.jobs.push_back(Job::Fetch { job, back });
    }

    fn landed(&self, key: &str) -> u64 {
        self.landed.get(key).copied().unwrap_or(0)
    }

    fn next(&mut self) -> Option<Job> {
        self.jobs.pop_front()
    }

    /// Records the outcome of a write: `Some(why)` for a refusal, `None` for
    /// success. True when this is a new refusal worth logging.
    fn outcome(&mut self, key: &str, why: Option<String>) -> bool {
        match why {
            Some(w) => self.failed.insert(key.to_string(), w.clone()).as_ref() != Some(&w),
            None => {
                self.failed.remove(key);
                false
            }
        }
    }

    fn done(&mut self, key: &str, gen: u64) {
        let slot = self.landed.entry(key.to_string()).or_insert(0);
        *slot = (*slot).max(gen);
    }
}

/// The handle the panel holds. Cloning it shares the one queue and thread.
#[derive(Clone)]
pub(crate) struct LaneWorker {
    shared: Arc<(Mutex<Queue>, Condvar)>,
}

/// Why a write did not take: no answer, or an error status with its body.
fn refusal(path: &str, reply: Option<&crate::call::Reply>) -> Option<String> {
    match reply {
        None => Some(format!("{path}: no answer")),
        Some(r) if r.status >= 400 => {
            Some(format!("{path}: {} {}", r.status, String::from_utf8_lossy(&r.body).trim()))
        }
        Some(_) => None,
    }
}

fn lock(m: &Mutex<Queue>) -> MutexGuard<'_, Queue> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl LaneWorker {
    /// Starts the worker around `door`. `None` when the thread cannot spawn;
    /// the panel then runs its work inline, as it did before.
    pub(crate) fn spawn<D: Call + Send + 'static>(door: D, hook: WriteHook) -> Option<LaneWorker> {
        let shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let worker = Arc::clone(&shared);
        thread::Builder::new()
            .name("metal-dash lanes".into())
            .spawn(move || loop {
                let job = {
                    let (m, cv) = &*worker;
                    let mut q = lock(m);
                    loop {
                        if let Some(job) = q.next() {
                            break job;
                        }
                        q = cv.wait(q).unwrap_or_else(|e| e.into_inner());
                    }
                };
                match job {
                    Job::Post { key, gen, path, query, body } => {
                        let params = Params {
                            query: query.into_iter().collect(),
                            body,
                            tail: None,
                        };
                        let reply = door.call("POST", &path, params);
                        hook(&path);
                        let why = refusal(&path, reply.as_ref());
                        let (m, _) = &*worker;
                        let mut q = lock(m);
                        if q.outcome(&key, why.clone()) {
                            eprintln!("[lanes] write refused: {}", why.unwrap_or_default());
                        }
                        q.done(&key, gen);
                    }
                    Job::Fetch { job, back } => {
                        let _ = back.send(fetch_song(&door, &job));
                    }
                }
            })
            .ok()?;
        Some(LaneWorker { shared })
    }

    /// Queues a POST and returns its generation; [`Self::landed`] reaches it
    /// once the write has run.
    pub(crate) fn post(&self, key: &str, path: &str, query: Vec<(String, String)>, body: Vec<u8>) -> u64 {
        let (m, cv) = &*self.shared;
        let gen = lock(m).post(key, path, query, body);
        cv.notify_one();
        gen
    }

    /// The highest generation of `key`'s writes that has run.
    pub(crate) fn landed(&self, key: &str) -> u64 {
        let (m, _) = &*self.shared;
        lock(m).landed(key)
    }

    /// Why `key`'s latest write was refused, if it was (the picture then shows
    /// an older row than the one asked for).
    pub(crate) fn failure(&self, key: &str) -> Option<String> {
        let (m, _) = &*self.shared;
        lock(m).failed.get(key).cloned()
    }

    /// Queues a song fetch; the answer comes back on the returned channel.
    pub(crate) fn fetch(&self, job: SongJob) -> Receiver<SongLoad> {
        let (back, answer) = mpsc::channel();
        let (m, cv) = &*self.shared;
        lock(m).fetch(job, back);
        cv.notify_one();
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pop(q: &mut Queue) -> Job {
        q.next().expect("a queued job")
    }

    fn post_body(job: Job) -> (String, u64, Vec<u8>) {
        match job {
            Job::Post { key, gen, body, .. } => (key, gen, body),
            Job::Fetch { .. } => panic!("expected a POST"),
        }
    }

    /// Jobs run in the order they were queued: a write, then another key's
    /// write, then a read.
    #[test]
    fn jobs_run_in_the_order_they_were_asked_for() {
        let mut q = Queue::default();
        q.post("floats", "/a", vec![], b"a".to_vec());
        q.post("seek", "/b", vec![], b"s".to_vec());
        let (back, _answer) = mpsc::channel();
        q.fetch(SongJob { song: "s".into(), keyframes: None }, back);
        assert_eq!(post_body(pop(&mut q)).0, "floats");
        assert_eq!(post_body(pop(&mut q)).0, "seek");
        assert!(matches!(pop(&mut q), Job::Fetch { .. }));
        assert!(q.next().is_none());
    }

    /// Two writes to one key while the first still waits are one job: the
    /// newer body, at the older one's place, with the newer generation. The
    /// key has not landed until that job has run.
    #[test]
    fn a_waiting_write_is_replaced_by_the_newer_one() {
        let mut q = Queue::default();
        let first = q.post("floats", "/a", vec![], b"one".to_vec());
        q.post("seek", "/b", vec![], b"x".to_vec());
        let second = q.post("floats", "/a", vec![], b"two".to_vec());
        assert!(second > first);
        assert_eq!(q.landed("floats"), 0, "nothing has run yet");

        let (key, gen, body) = post_body(pop(&mut q));
        assert_eq!((key.as_str(), body.as_slice()), ("floats", b"two".as_slice()));
        assert_eq!(gen, second, "the job carries the newest generation");
        q.done(&key, gen);
        assert_eq!(q.landed("floats"), second);
        assert_eq!(post_body(pop(&mut q)).0, "seek");
        assert!(q.next().is_none(), "the replaced write is not run again");
    }

    /// A write that is already running is not replaced: a new write for the
    /// same key queues behind it, and `landed` reaches each in turn.
    #[test]
    fn a_running_write_is_not_replaced_and_landed_follows_each_run() {
        let mut q = Queue::default();
        let first = q.post("floats", "/a", vec![], b"one".to_vec());
        let (_, gen, _) = post_body(pop(&mut q));
        assert_eq!(gen, first);
        let second = q.post("floats", "/a", vec![], b"two".to_vec());
        q.done("floats", first);
        assert_eq!(q.landed("floats"), first);
        assert!(q.landed("floats") < second, "the second write is not landed yet");
        let (_, gen, body) = post_body(pop(&mut q));
        assert_eq!((gen, body.as_slice()), (second, b"two".as_slice()));
        q.done("floats", second);
        assert_eq!(q.landed("floats"), second);
    }

    /// A refusal is remembered until a write succeeds, and only a new reason
    /// counts as news to log.
    #[test]
    fn a_refusal_is_remembered_until_a_write_succeeds() {
        let mut q = Queue::default();
        assert!(q.outcome("floats", Some("wrong width".into())));
        assert!(!q.outcome("floats", Some("wrong width".into())), "same reason: not news");
        assert!(q.outcome("floats", Some("no model".into())));
        assert_eq!(q.failed.get("floats").map(String::as_str), Some("no model"));
        q.outcome("floats", None);
        assert!(q.failed.is_empty());
    }

    /// `landed` never goes back: a late report of an older write leaves the
    /// newer one's mark alone.
    #[test]
    fn landed_only_moves_forward() {
        let mut q = Queue::default();
        q.done("floats", 5);
        q.done("floats", 3);
        assert_eq!(q.landed("floats"), 5);
        assert_eq!(q.landed("unknown"), 0);
    }
}
