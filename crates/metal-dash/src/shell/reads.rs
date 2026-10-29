//! A short-lived read cache in front of the door — the shell's own reads,
//! not the images.
//!
//! Every panel reads its state fresh every frame (regla 5), which was
//! affordable while a read was cheap. It stopped being cheap the moment the
//! preview worker landed: the studio answers `/author/render/grid.jpg` by
//! rendering thirty-six models under its own lock, and every other
//! `/author/*` read a panel makes in that window queues behind it. A frame
//! is 16.6 ms; a render is not.
//!
//! So reads go through this: a `GET` whose reply is JSON is served from the
//! last answer for [`TTL`], and **any write clears the whole cache** — a
//! read is never older than the last `POST`, which is what keeps a drag, a
//! toggle or a selection from being answered with the world before it.
//!
//! Two things are deliberately not cached:
//!
//! * **Bytes replies** (a JPEG, the `/out/` page, a downloaded file). They
//!   are what the panels and the monitor actually paint; each of those has
//!   its own cadence (`frames::live_gap()`, the monitor's own loop), and a
//!   second cadence here would only fight it. The filter is the reply's own
//!   `kind`, not a path list — a list would rot the first time a route
//!   changed its content type.
//! * **Anything in tests.** `Shell::new` is handed the door directly by the
//!   kittest harness and the restyle drill, so `FakeDoor`'s recording keeps
//!   every call it always recorded; only `window::run` wraps the real door
//!   in one of these.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use web_time::{Duration, Instant};

use crate::call::{Call, Params, Reply};

/// How long one JSON read is reused. Short enough that a panel which
/// changed under the operator's hand is answered by the world as it is now
/// on the next frame or two, long enough that a render behind the studio's
/// lock is not joined by four more readers queueing for the same answer.
pub(crate) const TTL: Duration = Duration::from_millis(250);

/// The longest a read is reused before a refresh is due, however slow it
/// was: a read that took 200 ms behind a render is not re-asked four times a
/// second, which is what kept the author lock busy for the whole frame.
pub(crate) const TTL_MAX: Duration = Duration::from_secs(1);

/// A read that changes only with the writes its own predicate names, not
/// with every write in its first segment. Each costs 20–70 ms to rebuild
/// (the graph is parsed from disk and laid out on every call), so it is
/// reused for [`TTL_STATIC`] and cleared only by the writes that can change
/// it.
struct Scoped {
    path: &'static str,
    affected_by: fn(&str) -> bool,
}

const SCOPED_READS: &[Scoped] = &[
    Scoped { path: "/graph", affected_by: graph_write },
    Scoped { path: "/catalog", affected_by: graph_write },
    Scoped { path: "/author/models/state", affected_by: model_write },
];

/// Whether `write` is `prefix` itself or a path under it (`/graphs` is not
/// under `/graph`).
fn under(write: &str, prefix: &str) -> bool {
    write == prefix
        || write
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with(['/', '\u{1}']))
}

/// A write that can change the node graph: the graph itself, its script, or
/// its loops. Lane writes (`/author/render/floats`, `/author/video/seek`) and
/// playback do not.
fn graph_write(write: &str) -> bool {
    ["/graph", "/script", "/loops"].iter().any(|p| under(write, p))
}

/// A write that can change the model list or the songs behind it.
fn model_write(write: &str) -> bool {
    ["/author/models", "/author/songs", "/author/add", "/author/digest", "/author/trim"]
        .iter()
        .any(|p| under(write, p))
}

/// The per-dependency rule for `path`, if it is a [`SCOPED_READS`] read.
fn scoped(path: &str) -> Option<&'static Scoped> {
    SCOPED_READS.iter().find(|s| s.path == path)
}

/// How long a reused read stays fresh for: [`TTL_STATIC`] for a scoped read,
/// otherwise four times what the last fetch cost, between [`TTL`] and
/// [`TTL_MAX`].
fn fresh_for(path: &str, cost: Duration) -> Duration {
    if scoped(path).is_some() {
        return TTL_STATIC;
    }
    (cost * 4).clamp(TTL, TTL_MAX)
}

/// How long a scoped read is reused until a write that affects it. An edit
/// through the app clears it at once; only a change made outside the app
/// waits this long.
pub(crate) const TTL_STATIC: Duration = Duration::from_secs(2);

/// The reads the shell makes on every frame with no query. They are fetched
/// at startup so no panel waits on them inline.
const PREWARM: &[&str] = &[
    "/state",
    "/author/models/state",
    "/author/dataset",
    "/author/songs",
    "/graph",
    "/graph/io",
    "/graph/live",
    "/catalog",
    "/venue",
    "/listen/status",
];

/// Reads that are never answered from the cache: the audio clock, which the
/// lanes playhead follows frame by frame.
const LIVE_READS: &[&str] = &["/author/play"];

/// One cached reply: when it was taken, and the reply itself (a `Reply` is
/// not `Clone` — its `kind` is `&'static str` and its body is the only
/// owned half, so storing the three fields is the whole of it).
struct Entry {
    at: Instant,
    /// How long the fetch that made this reply took.
    cost: Duration,
    /// A write has changed what this read describes. The reply is still
    /// served (the panel keeps its last answer) while a refresh runs behind
    /// the frame; it is never fetched inline again.
    stale: bool,
    status: u16,
    kind: &'static str,
    body: Vec<u8>,
}

impl Entry {
    fn reply(&self) -> Reply {
        Reply {
            status: self.status,
            kind: self.kind,
            body: self.body.clone(),
        }
    }

    fn of(reply: &Reply, cost: Duration, stale: bool) -> Entry {
        Entry {
            at: Instant::now(),
            cost,
            stale,
            status: reply.status,
            kind: reply.kind,
            body: reply.body.clone(),
        }
    }
}

/// One fetch in flight (a background refresh or an inline read). A write that
/// invalidates its key while it runs marks it dirty, and its reply then lands
/// stale. Tracking the fetch, not a global write counter, means a write that
/// does not touch the key never makes its reply stale.
struct Fetch {
    key: String,
    dirty: bool,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<String, Entry>,
    /// Keys a background refresh is already fetching.
    refreshing: HashSet<String>,
    /// Fetches in flight, by token.
    fetches: HashMap<u64, Fetch>,
    next_token: u64,
    /// Reads whose reply is not JSON, or that no route answers. A miss on one
    /// of these is answered inline: a picture is not waited for, and an
    /// unrouted path costs nothing.
    inline_only: HashSet<String>,
    /// Woken when a read the UI was not waiting on, or a read that cannot be
    /// deferred, lands (see [`Call::set_wake`]).
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Cache {
    /// Registers a fetch of `key` and returns its token.
    fn begin(&mut self, key: &str) -> u64 {
        self.next_token += 1;
        let token = self.next_token;
        self.fetches.insert(token, Fetch { key: key.to_string(), dirty: false });
        token
    }

    /// Ends the fetch `token`; `true` when a write invalidated it meanwhile.
    fn end(&mut self, token: u64) -> bool {
        self.fetches.remove(&token).is_none_or(|f| f.dirty)
    }
}

type Job = (String, String, Params, u64);

pub(crate) struct CachedDoor {
    inner: Box<dyn Call>,
    cache: Arc<Mutex<Cache>>,
    /// The background refresher, when the door can be shared with a thread.
    refresh: Option<Sender<Job>>,
    /// The background graph writer ([`Call::post_async`] goes through it),
    /// when one was started with [`Self::with_writes`].
    writes: Option<crate::shell::graph_writer::GraphWriter>,
}

impl CachedDoor {
    pub(crate) fn new(inner: Box<dyn Call>) -> CachedDoor {
        CachedDoor {
            inner,
            cache: Arc::new(Mutex::new(Cache::default())),
            refresh: None,
            writes: None,
        }
    }

    /// Routes [`Call::post_async`] through `writes`, a writer on a second
    /// handle to this door (see `GraphWriter::spawn`).
    pub(crate) fn with_writes(mut self, writes: crate::shell::graph_writer::GraphWriter) -> CachedDoor {
        self.writes = Some(writes);
        self
    }

    /// [`Self::new`] plus a refresher thread that walks `background` (a
    /// second handle to the same door) for expired reads.
    pub(crate) fn with_background<D: Call + Send + 'static>(inner: Box<dyn Call>, background: D) -> CachedDoor {
        let mut door = CachedDoor::new(inner);
        let cache = Arc::clone(&door.cache);
        let (tx, rx) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name("metal-dash reads".into())
            .spawn(move || {
                while let Ok((key, path, params, token)) = rx.recv() {
                    let started = Instant::now();
                    let reply = background.call("GET", &path, params);
                    let cost = started.elapsed();
                    if let Ok(mut c) = cache.lock() {
                        c.refreshing.remove(&key);
                        let stale = c.end(token);
                        // A first answer is what the UI is waiting on: wake it
                        // so the panel adopts it. A refresh of an answer it
                        // already has is picked up on its next frame anyway.
                        let landed = match reply {
                            Some(reply) if reply.kind == "application/json" => {
                                c.entries.insert(key, Entry::of(&reply, cost, stale)).is_none()
                            }
                            _ => {
                                c.inline_only.insert(key);
                                true
                            }
                        };
                        if landed {
                            if let Some(wake) = &c.wake {
                                wake();
                            }
                        }
                    }
                }
            });
        if spawned.is_ok() {
            // Warm the reads the panels make on every frame, so the first
            // focus switch is answered from the cache, not from an inline
            // fetch on the UI thread.
            if let Ok(mut c) = door.cache.lock() {
                for path in PREWARM {
                    c.refreshing.insert(Self::key(path, &Params::default()));
                }
            }
            for path in PREWARM {
                let _ = tx.send((Self::key(path, &Params::default()), path.to_string(), Params::default(), 0));
            }
            door.refresh = Some(tx);
        }
        door
    }

    /// The query is part of the identity — `/fs?path=a` and `/fs?path=b`
    /// are different reads — so it is folded into the key in a sorted,
    /// unambiguous shape rather than trusted to a `HashMap`'s iteration
    /// order.
    fn key(path: &str, params: &Params) -> String {
        let mut query: Vec<(&str, &str)> = params
            .query
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        query.sort_unstable();
        let mut key = String::from(path);
        for (k, v) in query {
            key.push('\u{1}');
            key.push_str(k);
            key.push('\u{2}');
            key.push_str(v);
        }
        key
    }
}

impl CachedDoor {
    /// What a writer that is not this door (the lanes worker) runs after each
    /// POST it sends, so the reads this door serves see that write the same
    /// way a write through [`Call::call`] would.
    /// What the graph writer runs with each graph it reads back after its
    /// writes settled: the reply becomes this cache's entry for that graph,
    /// so a read after the write is the fresh answer, not the stale one the
    /// write left behind.
    pub(crate) fn graph_put(&self) -> crate::shell::graph_writer::GraphPut {
        let cache = Arc::clone(&self.cache);
        Arc::new(move |graph: &str, reply: &Reply| {
            let mut params = Params::default();
            params.query.insert("id".to_string(), graph.to_string());
            let key = Self::key("/graph", &params);
            if let Ok(mut cache) = crate::perf::locked(&cache, "reads.cache") {
                cache.entries.insert(key, Entry::of(reply, Duration::ZERO, false));
            }
        })
    }

    pub(crate) fn write_hook(&self) -> crate::shell::lane_worker::WriteHook {
        let cache = Arc::clone(&self.cache);
        Arc::new(move |path: &str| {
            if let Ok(mut cache) = crate::perf::locked(&cache, "reads.cache") {
                mark_write(&mut cache, path);
            }
        })
    }
}

/// A `GET` answered as a shared parsed `Value` (`json_memo::parse_shared`):
/// for a large reply a panel only reads, so a frame never deep-copies it.
pub(crate) fn get_shared(door: &dyn Call, path: &str, query: &[(&str, String)]) -> Option<std::rc::Rc<serde_json::Value>> {
    let params = Params {
        query: query.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect(),
        ..Params::default()
    };
    match door.call("GET", path, params) {
        Some(reply) if reply.status == 200 => {
            crate::perf::span("reads.parse", || crate::json_memo::parse_shared(&reply.body))
        }
        _ => None,
    }
}

/// A write to `path` makes every read it could have changed a statement
/// about a world that no longer exists: keep the reply for the panel to show,
/// but mark it for a background refresh.
fn mark_write(cache: &mut Cache, path: &str) {
    for (k, e) in cache.entries.iter_mut() {
        if invalidated_by(k, path) {
            e.stale = true;
        }
    }
    for f in cache.fetches.values_mut() {
        if invalidated_by(&f.key, path) {
            f.dirty = true;
        }
    }
}

/// The first path segment — `/graph/node` → `graph`, `/author/songs` →
/// `author` — the scope one write can change.
fn segment(path: &str) -> &str {
    path.trim_start_matches('/').split(['/', '\u{1}']).next().unwrap_or("")
}

/// Whether a write to `write` can have changed the cached read `key`. A
/// scoped read answers only to its own dependency ([`SCOPED_READS`]). Any
/// other read is cleared by a write in its first segment, and the global
/// `/state` by every write.
fn invalidated_by(key: &str, write: &str) -> bool {
    let path = key.split('\u{1}').next().unwrap_or(key);
    if let Some(s) = scoped(path) {
        return (s.affected_by)(write);
    }
    segment(key) == segment(write) || key == "/state" || key.starts_with("/state\u{1}")
}

impl Call for CachedDoor {
    fn graph_refresh(&self) -> Option<(String, serde_json::Value)> {
        self.writes.as_ref()?.take_refresh()
    }

    fn is_pending(&self, path: &str) -> bool {
        let Ok(cache) = crate::perf::locked(&self.cache, "reads.cache") else {
            return false;
        };
        cache
            .refreshing
            .iter()
            .any(|k| k.split('\u{1}').next() == Some(path) && !cache.entries.contains_key(k))
    }

    fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut cache) = crate::perf::locked(&self.cache, "reads.cache") {
            cache.wake = Some(wake);
        }
    }

    fn post_async(&self, path: &str, body: serde_json::Value) {
        match &self.writes {
            Some(writes) if path == "/graph/node" => {
                writes.submit(&body);
            }
            _ => {
                let _ = self.json("POST", path, Some(body));
            }
        }
    }

    fn call(&self, method: &str, path: &str, params: Params) -> Option<Reply> {
        let get = method == "GET";
        // `/sync` is never cached: the pairing overlay's own poll is its
        // cadence, and a pairing POST on its worker thread does not come
        // through this cache, so a cached answer could outlive it.
        // The audio clock is read live: a cached or stale position is the
        // playhead's lag behind the speakers (measured on the emulator, 0.6 s).
        let readable = get && segment(path) != "sync" && !LIVE_READS.contains(&path);
        let key = readable.then(|| Self::key(path, &params));
        if let Some(key) = &key {
            if let Ok(mut cache) = crate::perf::locked(&self.cache, "reads.cache") {
                if let Some(entry) = cache.entries.get(key) {
                    let age = entry.at.elapsed();
                    if !entry.stale && age < fresh_for(path, entry.cost) {
                        return Some(entry.reply());
                    }
                    // A reply the UI already has is never fetched inline again:
                    // the refresh runs behind the frame, however long it waits
                    // for the locks. Only a read with no reply yet blocks.
                    if let Some(tx) = self.refresh.as_ref() {
                        let reply = entry.reply();
                        if cache.refreshing.insert(key.clone()) {
                            let token = cache.begin(key);
                            let _ = tx.send((key.clone(), path.to_string(), params.clone(), token));
                        }
                        return Some(reply);
                    }
                }
                // No reply yet: the first read of a path is not fetched on the
                // UI thread either. It goes to the refresher, and this call
                // answers `None` (the panel's empty state) until it lands and
                // wakes the UI. A path that cannot be deferred (see
                // `inline_only`) still runs inline below.
                if let Some(tx) = self.refresh.as_ref() {
                    if !cache.inline_only.contains(key) {
                        if cache.refreshing.insert(key.clone()) {
                            let token = cache.begin(key);
                            let _ = tx.send((key.clone(), path.to_string(), params.clone(), token));
                        }
                        return None;
                    }
                }
            }
        }

        // Registered before the fetch, so a write that lands while it runs
        // marks the reply stale rather than being cached as fresh.
        let token = match (&key, get) {
            (Some(key), true) => crate::perf::locked(&self.cache, "reads.cache").ok().map(|mut c| c.begin(key)),
            _ => None,
        };
        let started = Instant::now();
        let reply = {
            let _span = crate::perf::Span::new("reads.inner");
            self.inner.call(method, path, params)
        };
        let cost = started.elapsed();

        if let Ok(mut cache) = crate::perf::locked(&self.cache, "reads.cache") {
            if !get {
                mark_write(&mut cache, path);
            } else if let (Some(key), Some(reply)) = (key, &reply) {
                let stale = token.is_none_or(|t| cache.end(t));
                if reply.kind == "application/json" {
                    cache.entries.insert(key, Entry::of(reply, cost, stale));
                }
            } else if let Some(t) = token {
                cache.end(t);
            }
        }
        reply
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// Counts what actually reached the inner door, and answers whatever
    /// `kind` the test asked for — the one knob the cache's own rule turns
    /// on.
    struct CountDoor {
        calls: Cell<usize>,
        kind: &'static str,
    }

    impl CountDoor {
        fn new(kind: &'static str) -> Rc<CountDoor> {
            Rc::new(CountDoor {
                calls: Cell::new(0),
                kind,
            })
        }
    }

    impl Call for CountDoor {
        fn call(&self, _method: &str, _path: &str, _params: Params) -> Option<Reply> {
            self.calls.set(self.calls.get() + 1);
            Some(Reply {
                status: 200,
                kind: self.kind,
                body: b"{}".to_vec(),
            })
        }
    }

    /// `Call` is this crate's trait, so a shared counter can implement it
    /// the same way `shell::FakeDoor`'s `Rc` impl does — that is what lets a
    /// test keep reading the count after the cache has taken the box.
    impl Call for Rc<CountDoor> {
        fn call(&self, method: &str, path: &str, params: Params) -> Option<Reply> {
            CountDoor::call(self, method, path, params)
        }
    }

    fn get(door: &CachedDoor, path: &str) -> Option<Reply> {
        door.call("GET", path, Params::default())
    }

    #[test]
    fn one_path_read_twice_is_one_call_and_a_new_path_is_two() {
        let counting = CountDoor::new("application/json");
        let door = CachedDoor::new(Box::new(Rc::clone(&counting)));
        let _ = get(&door, "/state");
        let _ = get(&door, "/state");
        assert_eq!(counting.calls.get(), 1, "the second read is the cache's");
        let _ = get(&door, "/graphs");
        assert_eq!(
            counting.calls.get(),
            2,
            "a different path is a different read"
        );
    }

    /// The audio clock is never a cached answer, and reading it does not
    /// throw away the other reads either.
    #[test]
    fn the_audio_clock_is_read_live() {
        let counting = CountDoor::new("application/json");
        let door = CachedDoor::new(Box::new(Rc::clone(&counting)));
        let _ = get(&door, "/author/songs");
        let _ = get(&door, "/author/play");
        let _ = get(&door, "/author/play");
        assert_eq!(counting.calls.get(), 3, "the clock is asked every time");
        let _ = get(&door, "/author/songs");
        assert_eq!(counting.calls.get(), 3, "its read survives the clock reads");
    }

    #[test]
    fn the_query_is_part_of_the_key() {
        let counting = CountDoor::new("application/json");
        let door = CachedDoor::new(Box::new(Rc::clone(&counting)));
        let one = Params {
            query: [("path".to_string(), "/a".to_string())]
                .into_iter()
                .collect(),
            ..Params::default()
        };
        let two = Params {
            query: [("path".to_string(), "/b".to_string())]
                .into_iter()
                .collect(),
            ..Params::default()
        };
        let _ = door.call("GET", "/fs", one.clone());
        let _ = door.call("GET", "/fs", one);
        assert_eq!(counting.calls.get(), 1);
        let _ = door.call("GET", "/fs", two);
        assert_eq!(counting.calls.get(), 2);
    }

    #[test]
    fn a_write_keeps_reads_it_cannot_have_changed() {
        let counting = CountDoor::new("application/json");
        let door = CachedDoor::new(Box::new(Rc::clone(&counting)));
        let _ = get(&door, "/author/songs");
        let _ = get(&door, "/graph");
        let _ = door.call("POST", "/graph/node", Params::default());
        let _ = get(&door, "/author/songs");
        assert_eq!(counting.calls.get(), 3, "a graph write keeps the song list");
        let _ = get(&door, "/graph");
        assert_eq!(counting.calls.get(), 4, "and re-reads the graph");
    }

    #[test]
    fn a_write_clears_every_cached_read() {
        let counting = CountDoor::new("application/json");
        let door = CachedDoor::new(Box::new(Rc::clone(&counting)));
        let _ = get(&door, "/state");
        let _ = door.call("POST", "/transport", Params::default());
        let _ = get(&door, "/state");
        assert_eq!(counting.calls.get(), 3, "the read after the write is fresh");
    }

    /// A write reported by the lanes worker clears the same reads a write
    /// through this door does.
    #[test]
    fn the_write_hook_marks_reads_like_a_write_does() {
        let counting = CountDoor::new("application/json");
        let door = CachedDoor::new(Box::new(Rc::clone(&counting)));
        let hook = door.write_hook();
        let _ = get(&door, "/graph");
        let _ = get(&door, "/graph");
        assert_eq!(counting.calls.get(), 1);
        hook("/graph/node");
        let _ = get(&door, "/graph");
        assert_eq!(counting.calls.get(), 2, "the graph is re-read after the hook");
    }

    /// The graph and catalogue are reused for the static TTL, every other
    /// JSON read keeps the cost-scaled one — `/state` moves with the clock.
    #[test]
    fn static_reads_are_reused_longer() {
        assert_eq!(fresh_for("/graph", Duration::from_millis(60)), TTL_STATIC);
        assert_eq!(fresh_for("/catalog", Duration::from_millis(21)), TTL_STATIC);
        assert_eq!(fresh_for("/state", Duration::from_millis(20)), TTL);
        assert_eq!(fresh_for("/author/songs", Duration::from_millis(1)), TTL);
    }

    /// A static read is reused until a write, and any write clears it — a
    /// script save changes the graph without touching `/graph`'s segment.
    #[test]
    fn a_static_read_survives_until_a_write() {
        let counting = CountDoor::new("application/json");
        let door = CachedDoor::new(Box::new(Rc::clone(&counting)));
        let _ = get(&door, "/graph");
        let _ = get(&door, "/graph");
        let _ = get(&door, "/catalog");
        let _ = get(&door, "/catalog");
        assert_eq!(counting.calls.get(), 2, "each static read is reused");
        let _ = door.call("POST", "/script", Params::default());
        let _ = get(&door, "/graph");
        let _ = get(&door, "/catalog");
        assert_eq!(counting.calls.get(), 5, "a script write clears both, once each");
    }

    #[test]
    fn a_bytes_reply_is_never_cached() {
        let counting = CountDoor::new("image/jpeg");
        let door = CachedDoor::new(Box::new(Rc::clone(&counting)));
        let _ = get(&door, "/frame");
        let _ = get(&door, "/frame");
        assert_eq!(counting.calls.get(), 2);
    }

    /// The pairing reads are never cached, so a POST on the pairing worker
    /// (which skips this cache) can never leave a stale `/sync` answer behind.
    #[test]
    fn a_sync_read_is_never_cached() {
        let counting = CountDoor::new("application/json");
        let door = CachedDoor::new(Box::new(Rc::clone(&counting)));
        let _ = get(&door, "/sync/pair");
        let _ = get(&door, "/sync/pair");
        assert_eq!(counting.calls.get(), 2);
    }

    /// The playback stream the lanes worker and the preview send: float
    /// posts, seeks, and the transport. None of them can change the graph or
    /// the model list, so neither read is re-fetched, however many arrive.
    #[test]
    fn an_author_write_stream_keeps_graph_and_models_state() {
        let counting = CountDoor::new("application/json");
        let door = CachedDoor::new(Box::new(Rc::clone(&counting)));
        let hook = door.write_hook();
        let _ = get(&door, "/graph");
        let _ = get(&door, "/author/models/state");
        assert_eq!(counting.calls.get(), 2);
        for _ in 0..200 {
            let _ = door.call("POST", "/author/render/floats", Params::default());
            hook("/author/render/floats");
            let _ = door.call("POST", "/author/video/seek", Params::default());
            hook("/author/video/seek");
            let _ = door.call("POST", "/author/play", Params::default());
            let _ = get(&door, "/graph");
            let _ = get(&door, "/author/models/state");
        }
        assert_eq!(counting.calls.get(), 2 + 200 * 3, "only the three POSTs reached the door");
        let _ = get(&door, "/graph");
        let _ = get(&door, "/author/models/state");
        assert_eq!(counting.calls.get(), 2 + 200 * 3, "both reads are still the cache's");
    }

    /// A model or song write clears `/author/models/state` and leaves the
    /// graph alone; a graph write does the reverse.
    #[test]
    fn model_and_graph_writes_clear_only_their_own_read() {
        let counting = CountDoor::new("application/json");
        let door = CachedDoor::new(Box::new(Rc::clone(&counting)));
        let _ = get(&door, "/graph");
        let _ = get(&door, "/author/models/state");
        let _ = door.call("POST", "/author/models/example", Params::default());
        let _ = get(&door, "/graph");
        assert_eq!(counting.calls.get(), 3, "the graph survives a model write");
        let _ = get(&door, "/author/models/state");
        assert_eq!(counting.calls.get(), 4, "the model list is re-read after it");
        let _ = door.call("POST", "/graph/node", Params::default());
        let _ = get(&door, "/author/models/state");
        assert_eq!(counting.calls.get(), 5, "the POST reached the door; the model list is kept");
        let _ = get(&door, "/graph");
        assert_eq!(counting.calls.get(), 6, "and the graph is re-read");
    }

    #[test]
    fn a_song_write_clears_models_state() {
        let counting = CountDoor::new("application/json");
        let door = CachedDoor::new(Box::new(Rc::clone(&counting)));
        let _ = get(&door, "/author/models/state");
        let _ = door.call("POST", "/author/songs", Params::default());
        let _ = get(&door, "/author/models/state");
        assert_eq!(counting.calls.get(), 3);
    }

    /// The dependency predicates: a prefix matches its own segment only.
    #[test]
    fn graph_writes_are_the_graph_script_and_loops() {
        assert!(graph_write("/graph"));
        assert!(graph_write("/graph/node"));
        assert!(graph_write("/script"));
        assert!(graph_write("/loops/save"));
        assert!(!graph_write("/graphs"), "a longer segment is not the graph");
        assert!(!graph_write("/author/render/floats"));
        assert!(!graph_write("/author/video/seek"));
        assert!(!graph_write("/author/play"));
        assert!(!model_write("/author/render/floats"));
        assert!(!model_write("/author/video/seek"));
        assert!(!model_write("/author/play"));
        assert!(model_write("/author/models/timeline/segment"));
    }

    /// A fetch that a write invalidates while it runs lands stale; one that
    /// only an unrelated write overlaps does not.
    #[test]
    fn a_refresh_is_stale_only_when_its_own_dependency_wrote() {
        let mut cache = Cache::default();
        let token = cache.begin("/graph");
        mark_write(&mut cache, "/author/render/floats");
        assert!(!cache.end(token), "a lane write does not dirty the graph fetch");

        let token = cache.begin("/graph");
        mark_write(&mut cache, "/graph/node");
        assert!(cache.end(token), "a graph write does dirty it");

        let token = cache.begin("/author/models/state");
        mark_write(&mut cache, "/author/render/floats");
        assert!(!cache.end(token), "a lane write does not dirty the model list");
    }

    /// A read the graph writer read back after a write is the answer a later
    /// read gets — not the entry the write left stale, and no inner read.
    #[test]
    fn a_graph_read_back_after_a_write_is_the_fresh_answer() {
        let counting = CountDoor::new("application/json");
        let door = CachedDoor::new(Box::new(Rc::clone(&counting)));
        let graph = |door: &CachedDoor| {
            let mut params = Params::default();
            params.query.insert("id".to_string(), "g".to_string());
            door.call("GET", "/graph", params).unwrap().body
        };
        assert_eq!(graph(&door), b"{}");
        (door.write_hook())("/graph/node");
        (door.graph_put())("g", &Reply { status: 200, kind: "application/json", body: b"{\"fresh\":1}".to_vec() });
        assert_eq!(graph(&door), b"{\"fresh\":1}", "the read after the write is the read-back");
        assert_eq!(counting.calls.get(), 1, "no inner read was needed");
    }

}
