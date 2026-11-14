//! The graph writes the panels fire without waiting for the reply: a
//! `POST /graph/node` queued here runs on one worker thread, in the order it
//! was asked for, so a frame never waits the ~35 ms a write takes.
//!
//! The UI thread submits a body and gets a ticket. A caller that needs the
//! answer polls [`GraphWriter::take_result`] each frame and applies it when it
//! lands; a caller that ignores the answer just submits. A failed write is kept
//! until [`GraphWriter::take_errors`] drains it into the status line.
//!
//! No coalescing: `config` and `layer` merge their fields into a node, so two
//! writes to one node are not interchangeable and both must run.
//!
//! Read-back: once the queue drains, the worker re-reads `GET /graph` for the
//! graph the last write named, straight from the door (not the read cache),
//! puts it into the cache, and keeps it for the canvas. The canvas takes it
//! with [`GraphWriter::take_refresh`] and replaces its optimistic copy; a
//! refresh that a later write has overtaken is dropped rather than applied.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;

use serde_json::Value;

use crate::call::{Call, Params, Reply};
use crate::shell::lane_worker::WriteHook;

const PATH: &str = "/graph/node";

/// Receives each graph the worker reads back, keyed by its id: the read
/// cache's `put` (see `reads::CachedDoor::graph_put`).
pub(crate) type GraphPut = Arc<dyn Fn(&str, &Reply) + Send + Sync>;

/// A graph read back after the writes had settled: `upto` is the last ticket
/// it covers, which is done once the read-back is taken.
struct Refresh {
    upto: u64,
    graph: String,
    value: Value,
}

/// The queue and the outcomes. Plain data, so ordering and error reporting
/// are testable without a thread or a door.
#[derive(Default)]
struct State {
    jobs: VecDeque<(u64, Vec<u8>)>,
    /// The last ticket handed out; tickets are 1-based.
    next: u64,
    /// Every ticket at or below this has run.
    done_upto: u64,
    /// Failed writes not yet taken, by ticket.
    failed: HashMap<u64, String>,
    /// The last graph read back, if the queue was empty when it was read.
    refreshed: Option<Refresh>,
}

impl State {
    fn submit(&mut self, body: Vec<u8>) -> u64 {
        self.next += 1;
        self.jobs.push_back((self.next, body));
        self.next
    }

    fn next_job(&mut self) -> Option<(u64, Vec<u8>)> {
        self.jobs.pop_front()
    }

    fn finish(&mut self, ticket: u64, outcome: Result<(), String>) {
        self.done_upto = self.done_upto.max(ticket);
        if let Err(e) = outcome {
            self.failed.insert(ticket, e);
        }
    }

    /// `None` while the write is still queued or running; otherwise its
    /// outcome. A failure is forgotten once taken here or by `take_errors`.
    fn take_result(&mut self, ticket: u64) -> Option<Result<(), String>> {
        if ticket > self.done_upto {
            return None;
        }
        Some(match self.failed.remove(&ticket) {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    /// Every failure not yet taken, oldest first.
    fn take_errors(&mut self) -> Vec<String> {
        let mut tickets: Vec<u64> = self.failed.keys().copied().collect();
        tickets.sort_unstable();
        tickets.into_iter().filter_map(|t| self.failed.remove(&t)).collect()
    }
}

/// The handle the panels and the shell hold. Cloning shares the one queue and
/// thread.
#[derive(Clone)]
pub(crate) struct GraphWriter {
    shared: Arc<(Mutex<State>, Condvar)>,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a non-200 means as text: the body the server sent, the same as
/// [`Call::json`] reports for it.
fn outcome(reply: Option<Reply>) -> Result<(), String> {
    match reply {
        Some(r) if r.status == 200 => Ok(()),
        Some(r) => Err(String::from_utf8_lossy(&r.body).into_owned()),
        None => Err(format!("POST {PATH} matched no route")),
    }
}

impl GraphWriter {
    /// Starts the worker around `door`, running `hook` after each write (the
    /// read cache's bookkeeping, see `reads::CachedDoor::write_hook`). `None`
    /// when the thread cannot spawn; callers then write inline.
    pub(crate) fn spawn<D: Call + Send + 'static>(door: D, hook: WriteHook, put: GraphPut) -> Option<GraphWriter> {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let worker = Arc::clone(&shared);
        thread::Builder::new()
            .name("metal-dash graph writes".into())
            .spawn(move || loop {
                let (ticket, body) = {
                    let (m, cv) = &*worker;
                    let mut s = lock(m);
                    loop {
                        if let Some(job) = s.next_job() {
                            break job;
                        }
                        s = cv.wait(s).unwrap_or_else(|e| e.into_inner());
                    }
                };
                let graph = serde_json::from_slice::<Value>(&body)
                    .ok()
                    .and_then(|v| v["graph"].as_str().map(str::to_string));
                let params = Params { query: Default::default(), body, tail: None };
                let result = outcome(door.call("POST", PATH, params));
                hook(PATH);
                // The read-back lands in the cache before the ticket reads as
                // done, so a read after the ticket is the fresh answer.
                let idle = lock(&worker.0).jobs.is_empty();
                if let (true, Some(graph)) = (idle, graph) {
                    read_back(&door, &put, &worker, &graph, ticket);
                }
                lock(&worker.0).finish(ticket, result);
            })
            .ok()?;
        Some(GraphWriter { shared })
    }

    /// The graph read back by the last settled write, once: `None` while
    /// writes are queued or running, and for a read that a newer write has
    /// overtaken. The canvas applies the returned graph in place of its own.
    pub(crate) fn take_refresh(&self) -> Option<(String, Value)> {
        let mut s = lock(&self.shared.0);
        let r = s.refreshed.take()?;
        (r.upto == s.done_upto && s.jobs.is_empty()).then_some((r.graph, r.value))
    }

    /// Queues a write of `body` and returns its ticket.
    pub(crate) fn submit(&self, body: &Value) -> u64 {
        let (m, cv) = &*self.shared;
        let ticket = lock(m).submit(body.to_string().into_bytes());
        cv.notify_one();
        ticket
    }

    /// The outcome of `ticket` once it has run (`None` before that).
    pub(crate) fn take_result(&self, ticket: u64) -> Option<Result<(), String>> {
        let (m, _) = &*self.shared;
        lock(m).take_result(ticket)
    }

    /// Every failed write not yet reported, for the status line.
    pub(crate) fn take_errors(&self) -> Vec<String> {
        let (m, _) = &*self.shared;
        lock(m).take_errors()
    }
}

/// `GET /graph?id=` from the door itself, as the worker's own read: the reply
/// goes to the cache through `put` and is kept for the canvas.
fn read_back<D: Call>(door: &D, put: &GraphPut, shared: &(Mutex<State>, Condvar), graph: &str, ticket: u64) {
    let mut query = HashMap::new();
    query.insert("id".to_string(), graph.to_string());
    let params = Params { query, body: Vec::new(), tail: None };
    let Some(reply) = door.call("GET", "/graph", params).filter(|r| r.status == 200) else {
        return;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&reply.body) else {
        return;
    };
    put(graph, &reply);
    let mut s = lock(&shared.0);
    if s.jobs.is_empty() {
        s.refreshed = Some(Refresh { upto: ticket, graph: graph.to_string(), value });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// A door that records each POST body, and answers 400 with the body's
    /// `"fail"` text when it has one.
    struct FakeDoor {
        seen: Arc<Mutex<Vec<Value>>>,
    }

    impl Call for FakeDoor {
        fn call(&self, method: &str, path: &str, params: Params) -> Option<Reply> {
            if method == "GET" {
                let body = format!("{{\"graph\":{{\"id\":\"{}\"}}}}", params.query.get("id").map_or("", String::as_str));
                return Some(Reply { status: 200, kind: "application/json", body: body.into_bytes() });
            }
            assert_eq!((method, path), ("POST", PATH));
            let body: Value = serde_json::from_slice(&params.body).unwrap();
            self.seen.lock().unwrap().push(body.clone());
            if let Some(msg) = body.get("fail").and_then(Value::as_str) {
                return Some(Reply { status: 400, kind: "text/plain", body: msg.as_bytes().to_vec() });
            }
            Some(Reply { status: 200, kind: "application/json", body: b"{}".to_vec() })
        }
    }

    fn spawn(seen: &Arc<Mutex<Vec<Value>>>) -> GraphWriter {
        let hook: WriteHook = Arc::new(|_| {});
        GraphWriter::spawn(FakeDoor { seen: Arc::clone(seen) }, hook, Arc::new(|_, _| {})).expect("thread")
    }

    fn wait_for(w: &GraphWriter, ticket: u64) -> Result<(), String> {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(r) = w.take_result(ticket) {
                return r;
            }
            assert!(Instant::now() < until, "ticket {ticket} never ran");
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Writes run in the order they were submitted.
    #[test]
    fn writes_run_in_submission_order() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let w = spawn(&seen);
        let a = w.submit(&serde_json::json!({ "n": 1 }));
        let b = w.submit(&serde_json::json!({ "n": 2 }));
        let c = w.submit(&serde_json::json!({ "n": 3 }));
        assert!(a < b && b < c);
        assert_eq!(wait_for(&w, c), Ok(()));
        let order: Vec<i64> = seen.lock().unwrap().iter().map(|v| v["n"].as_i64().unwrap()).collect();
        assert_eq!(order, vec![1, 2, 3]);
    }

    /// Same-node writes are not merged: `config` merges fields, so both run,
    /// in order.
    #[test]
    fn writes_to_one_node_are_not_coalesced() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let w = spawn(&seen);
        let a = w.submit(&serde_json::json!({ "node": "n", "config": { "a": 1 } }));
        let b = w.submit(&serde_json::json!({ "node": "n", "config": { "b": 2 } }));
        assert_eq!(wait_for(&w, b), Ok(()));
        assert_eq!(wait_for(&w, a), Ok(()));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0]["config"], serde_json::json!({ "a": 1 }));
        assert_eq!(seen[1]["config"], serde_json::json!({ "b": 2 }));
    }

    /// A ticket is pending until its write has run; a success leaves nothing
    /// behind and reads back as Ok.
    #[test]
    fn a_ticket_is_pending_until_it_runs() {
        let mut s = State::default();
        let t = s.submit(b"{}".to_vec());
        assert!(s.take_result(t).is_none());
        let (ticket, _) = s.next_job().unwrap();
        s.finish(ticket, Ok(()));
        assert_eq!(s.take_result(t), Some(Ok(())));
        assert!(s.take_errors().is_empty());
    }

    /// A failure reaches its ticket once, and `take_errors` once for the
    /// status line, in submission order.
    #[test]
    fn failures_reach_the_ticket_and_the_status_line() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let w = spawn(&seen);
        let ok = w.submit(&serde_json::json!({}));
        let bad = w.submit(&serde_json::json!({ "fail": "no such node" }));
        let worse = w.submit(&serde_json::json!({ "fail": "bad graph" }));
        assert_eq!(wait_for(&w, bad), Err("no such node".to_string()));
        assert_eq!(wait_for(&w, ok), Ok(()));
        assert_eq!(wait_for(&w, worse), Err("bad graph".to_string()));
        assert!(w.take_errors().is_empty(), "both were taken through their tickets");

        let late = w.submit(&serde_json::json!({ "fail": "dropped" }));
        let again = w.submit(&serde_json::json!({ "fail": "shown" }));
        assert_eq!(wait_for(&w, again), Err("shown".to_string()));
        assert_eq!(wait_for(&w, late), Err("dropped".to_string()));
        assert_eq!(w.take_errors(), Vec::<String>::new());
    }

    /// Unreported failures come out of `take_errors` oldest first, then are gone.
    #[test]
    fn take_errors_drains_unreported_failures_in_order() {
        let mut s = State::default();
        let a = s.submit(b"1".to_vec());
        let b = s.submit(b"2".to_vec());
        let c = s.submit(b"3".to_vec());
        s.finish(b, Err("second".into()));
        s.finish(c, Err("third".into()));
        s.finish(a, Err("first".into()));
        assert_eq!(s.take_errors(), vec!["first", "second", "third"]);
        assert!(s.take_errors().is_empty());
        assert_eq!(s.take_result(b), Some(Ok(())), "drained failures are not reported twice");
    }

    /// The write hook runs after each write, with the node path.
    #[test]
    fn the_hook_runs_after_each_write() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel::<String>();
        let tx = Mutex::new(tx);
        let hook: WriteHook = Arc::new(move |p: &str| {
            let _ = tx.lock().unwrap().send(p.to_string());
        });
        let w = GraphWriter::spawn(FakeDoor { seen }, hook, Arc::new(|_, _| {})).expect("thread");
        let t = w.submit(&serde_json::json!({}));
        wait_for(&w, t).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), PATH);
    }

    /// Once the queue has drained, the worker reads the written graph back:
    /// `take_refresh` hands it over once, and `put` gets the same reply.
    #[test]
    fn a_settled_write_reads_its_graph_back_once() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let puts = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&puts);
        let put: GraphPut = Arc::new(move |gid: &str, _: &Reply| record.lock().unwrap().push(gid.to_string()));
        let hook: WriteHook = Arc::new(|_| {});
        let w = GraphWriter::spawn(FakeDoor { seen }, hook, put).expect("thread");
        let t = w.submit(&serde_json::json!({ "op": "flag", "graph": "g", "node": "n", "field": "bypass" }));
        assert_eq!(wait_for(&w, t), Ok(()));
        let until = Instant::now() + Duration::from_secs(5);
        let refresh = loop {
            if let Some(r) = w.take_refresh() {
                break r;
            }
            assert!(Instant::now() < until, "no read-back landed");
            thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(refresh.0, "g");
        assert_eq!(refresh.1["graph"]["id"], "g");
        assert_eq!(puts.lock().unwrap().as_slice(), ["g"]);
        assert!(w.take_refresh().is_none(), "a read-back is handed over once");
    }

}
