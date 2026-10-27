//! `audio.live` is a node: one listener per distinct source, one digest per
//! audio node (plan "Audio input is a node", Design 2).
//!
//! Before this, a single [`Input`] per process fed every source without a
//! cable. Now each `audio.live` carries its source in its config
//! (`{"source": "<Pulse source | cpal substring>" | "stand-in" | null}`), and
//! [`Listeners`] is the pool that turns those configs into open listeners:
//!
//! - **Cached by key** ([`SourceKey`]): two nodes naming the same source share
//!   one listener; two nodes naming two sources open two.
//! - **Released when unused**: a source no node is bound to any more is
//!   dropped (its capture thread stops), except the *seed* — the source a
//!   `null` config means — which is also the musical clock (router, Looper,
//!   beat count) and lives for the whole process.
//! - **Errors counted, never blocking**: a source that will not open is
//!   remembered as failed (not retried every frame — opening spawns `pactl`),
//!   the node keeps whatever it was reading before (the previous source stays
//!   open), and with nothing before it reads a zero row. The error is what the
//!   node's readout, the graph lint and `listen_status` report.
//!
//! The pool never chooses a source on its own: `null` means the seed, which is
//! `--listen` at startup or the system default; nothing falls back to "the
//! graph's first audio node" (rule 4 — no hidden state).
//!
//! `audio.file` and `sensor.*` nodes keep reading the clock frame, as they did
//! when there was one digest per process.

use std::collections::HashMap;
use web_time::{Duration, Instant};

use crate::digest::{Frame, FEATURES};
use crate::live::{Input, STAND_IN};
use crate::looper::Looper;

/// The node kind this pool serves.
pub const AUDIO_LIVE: &str = "audio.live";

/// A binding not refreshed for this long is forgotten, and its listener
/// released if nothing else holds it — the node was deleted, or its graph is
/// no longer drawn.
pub const RELEASE_AFTER: Duration = Duration::from_secs(10);

/// What a listener is opened from. `Default` is the system default input,
/// `Named` a Pulse source / cpal substring / `app:<serial>`, `StandIn` the
/// deliberate simulated digest (`digest::Simulated`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SourceKey {
    Default,
    Named(String),
    StandIn,
}

impl SourceKey {
    /// The pick vocabulary `POST /io {input}` and the combo already use:
    /// `""` the default, [`STAND_IN`] the stand-in, anything else a name.
    pub fn from_pick(pick: &str) -> SourceKey {
        match pick {
            "" => SourceKey::Default,
            STAND_IN => SourceKey::StandIn,
            name => SourceKey::Named(name.to_string()),
        }
    }

    /// The inverse of [`SourceKey::from_pick`].
    pub fn as_pick(&self) -> &str {
        match self {
            SourceKey::Default => "",
            SourceKey::StandIn => STAND_IN,
            SourceKey::Named(n) => n,
        }
    }

    /// A short name for the chrome indicator.
    pub fn short(&self) -> String {
        match self {
            SourceKey::Default => "default".into(),
            SourceKey::StandIn => STAND_IN.into(),
            SourceKey::Named(n) => {
                let tail = n.rsplit(['.', ':', '/']).find(|s| !s.is_empty() && *s != "monitor");
                tail.unwrap_or(n).chars().take(16).collect()
            }
        }
    }

    /// Open this source for real: the stand-in, or a device via
    /// [`Input::live`] (whose signature is unchanged by this module).
    pub fn open_real(&self) -> Result<Input, String> {
        match self {
            SourceKey::StandIn => Ok(Input::stand_in()),
            SourceKey::Default => Input::live(None),
            SourceKey::Named(n) => Input::live(Some(n)),
        }
    }
}

/// What an `audio.live` node's config asks for: `Some(pick)`, or `None` for
/// `null`/absent (the seed). `device` is read when `source` is absent — the
/// key the 2026-10-01 device picker wrote — so a graph saved then keeps its
/// pick without being rewritten.
pub fn wanted(node: &metal_graph::Node) -> Option<String> {
    let v = node.config.get("source").or_else(|| node.config.get("device"))?;
    v.as_str().map(str::to_string)
}

/// How a pool opens a source. The real one is [`SourceKey::open_real`]; tests
/// inject their own so nothing touches a sound card.
pub type Opener = Box<dyn FnMut(&SourceKey) -> Result<Input, String> + Send>;

/// `(graph key, node id)`. The graph key is the device being drawn (or the
/// graph id): two device graphs both have an `audio-1`.
pub type NodeKey = (String, String);

#[derive(Clone, Debug)]
struct Binding {
    want: SourceKey,
    using: Option<SourceKey>,
    error: Option<String>,
    seen: Instant,
}

/// One row of [`Listeners::status`]: a node, what it asked for, what it
/// reads, and that listener's readout.
#[derive(Clone, Debug)]
pub struct NodeStatus {
    pub graph: String,
    pub node: String,
    pub want: SourceKey,
    pub using: Option<SourceKey>,
    pub error: Option<String>,
    pub real: bool,
    pub describe: String,
    pub bpm: f32,
    pub dropped: u64,
    /// Whether the listener's worker loaded the Basic Pitch model, carried
    /// for `listen_status`'s `tone.loaded` (the tone design notes D9). False whenever
    /// there is no input, the input is the stand-in, or the `tone` feature is
    /// off — never true just because tone is configured.
    pub tone_loaded: bool,
    /// The listener's last inference wall time in ms, for `tone.latency_ms`.
    /// 0.0 with no input, the stand-in, or before the first inference.
    pub tone_latency_ms: f32,
}

/// The pool. See the module note.
pub struct Listeners {
    seed: SourceKey,
    open: HashMap<SourceKey, Input>,
    bound: HashMap<NodeKey, Binding>,
    failed: HashMap<SourceKey, String>,
    opener: Opener,
    /// The clock when the seed itself will not open: a stand-in at drive 0,
    /// which is silence — no row, no beat clock (`Simulated`'s own contract).
    silent: Input,
    /// Successful opens since start. Tests read it to prove "once".
    pub opens: u64,
    /// Failed opens since start — counted, never blocking.
    pub errors: u64,
}

impl Listeners {
    /// The real pool: `seed` is what a `null` source means (`--listen`, else
    /// the system default). The seed is opened now, so the startup banner can
    /// say what the clock is listening to.
    pub fn new(seed: SourceKey) -> Listeners {
        Listeners::with_opener(seed, Box::new(|k: &SourceKey| k.open_real()))
    }

    /// A pool with its own opener (tests, and [`Listeners::stand_in`]).
    pub fn with_opener(seed: SourceKey, opener: Opener) -> Listeners {
        let mut silent = Input::stand_in();
        silent.set_drive(0.0);
        let mut l = Listeners {
            seed,
            open: HashMap::new(),
            bound: HashMap::new(),
            failed: HashMap::new(),
            opener,
            silent,
            opens: 0,
            errors: 0,
        };
        let seed = l.seed.clone();
        let _ = l.ensure(&seed);
        l
    }

    /// Every source is the deliberate stand-in, whatever the nodes ask for:
    /// the pool tests and headless renders use, and what an operator gets
    /// when they pick `stand-in`. Never a silent fallback.
    pub fn stand_in() -> Listeners {
        Listeners::with_opener(SourceKey::StandIn, Box::new(|_k: &SourceKey| Ok(Input::stand_in())))
    }

    /// What a `null` source means.
    pub fn seed(&self) -> &SourceKey {
        &self.seed
    }

    /// The key a node's config resolves to.
    pub fn key_of(&self, node: &metal_graph::Node) -> SourceKey {
        match wanted(node) {
            Some(pick) => SourceKey::from_pick(&pick),
            None => self.seed.clone(),
        }
    }

    /// Whether a listener for `key` is open.
    pub fn is_open(&self, key: &SourceKey) -> bool {
        self.open.contains_key(key)
    }

    /// How many listeners are open.
    pub fn open_count(&self) -> usize {
        self.open.len()
    }

    /// Open `key` unless it is open or already failed.
    fn ensure(&mut self, key: &SourceKey) -> Result<(), String> {
        if self.open.contains_key(key) {
            return Ok(());
        }
        if let Some(e) = self.failed.get(key) {
            return Err(e.clone());
        }
        match (self.opener)(key) {
            Ok(input) => {
                self.open.insert(key.clone(), input);
                self.opens += 1;
                Ok(())
            }
            Err(e) => {
                self.failed.insert(key.clone(), e.clone());
                self.errors += 1;
                Err(e)
            }
        }
    }

    /// Forget every failure, so the next frame tries those sources again —
    /// the `probar` button, or a device that has been plugged in since.
    pub fn retry(&mut self) {
        self.failed.clear();
    }

    /// Bind one `audio.live` node and answer the source it reads this frame,
    /// or `None` (a zero row) when nothing opens.
    pub fn resolve(&mut self, graph: &str, node: &metal_graph::Node) -> Option<SourceKey> {
        let want = self.key_of(node);
        let nk: NodeKey = (graph.to_string(), node.id.clone());
        let prev = self.bound.get(&nk).cloned();
        // A changed pick is the operator asking again: a source that failed
        // before gets one fresh try.
        if prev.as_ref().is_some_and(|b| b.want != want) {
            self.failed.remove(&want);
        }
        let (using, error) = match self.ensure(&want) {
            Ok(()) => (Some(want.clone()), None),
            Err(e) => {
                // The source that was open stays open, and the node keeps it.
                let keep = prev.and_then(|b| b.using).filter(|k| self.open.contains_key(k));
                (keep, Some(e))
            }
        };
        self.bound.insert(nk, Binding { want, using: using.clone(), error, seen: Instant::now() });
        self.release_unused();
        using
    }

    /// Drop stale bindings and every listener no binding holds (the seed is
    /// the clock and is never dropped).
    pub fn release_unused(&mut self) {
        let now = Instant::now();
        self.bound.retain(|_, b| now.duration_since(b.seen) < RELEASE_AFTER);
        let held: Vec<SourceKey> = self.bound.values().filter_map(|b| b.using.clone()).collect();
        let seed = self.seed.clone();
        self.open.retain(|k, _| *k == seed || held.contains(k));
    }

    /// The input the musical clock reads (router, Looper, beat count): the
    /// seed's listener, or silence when the seed will not open.
    pub fn clock(&mut self) -> &mut Input {
        let seed = self.seed.clone();
        if self.ensure(&seed).is_ok() {
            return self.open.get_mut(&seed).expect("ensured");
        }
        &mut self.silent
    }

    /// The clock's frame this instant, through the one Looper — the frame
    /// the router saw, so a loop engaged on the clock source replays for
    /// every node that reads that source.
    pub fn clock_frame(&mut self, looper: &mut Looper, time: f32) -> Frame {
        looper.poll(self.clock(), time)
    }

    /// One digest per audio node of `graph` (bound under `graph.id`, the key
    /// `/graph`'s lint reads errors back by). `clock` is
    /// [`Listeners::clock_frame`] for this instant.
    pub fn digests(&mut self, graph: &metal_graph::Graph, clock: Frame, time: f32) -> Digests {
        let graph_key = graph.id.as_str();
        let mut by_node = HashMap::new();
        for node in &graph.nodes {
            if node.kind == AUDIO_LIVE {
                let frame = match self.resolve(graph_key, node) {
                    Some(k) if k == self.seed => clock,
                    Some(k) => match self.open.get_mut(&k) {
                        Some(input) => input.poll(time),
                        None => Digests::zero(),
                    },
                    None => Digests::zero(),
                };
                by_node.insert(node.id.clone(), frame);
            } else if node.kind.starts_with("audio.") || node.kind.starts_with("sensor.") {
                by_node.insert(node.id.clone(), clock);
            }
        }
        Digests { by_node, clock }
    }

    /// The error a node's source last reported, if any.
    pub fn error_of(&self, graph: &str, node: &str) -> Option<&str> {
        self.bound.get(&(graph.to_string(), node.to_string()))?.error.as_deref()
    }

    /// Every bound node with its listener's readout — `GET /listen/status`
    /// and the `listen_status` tool. Sorted for a stable answer.
    pub fn status(&self) -> Vec<NodeStatus> {
        let mut out: Vec<NodeStatus> = self
            .bound
            .iter()
            .map(|((graph, node), b)| {
                let input = b.using.as_ref().and_then(|k| self.open.get(k));
                NodeStatus {
                    graph: graph.clone(),
                    node: node.clone(),
                    want: b.want.clone(),
                    using: b.using.clone(),
                    error: b.error.clone(),
                    real: input.is_some_and(Input::is_real),
                    describe: input.map(Input::describe).unwrap_or_else(|| "sin audio".into()),
                    bpm: input.map(Input::bpm).unwrap_or(0.0),
                    dropped: input.map(Input::dropped).unwrap_or(0),
                    tone_loaded: input.is_some_and(Input::tone_loaded),
                    tone_latency_ms: input.map(Input::tone_latency_ms).unwrap_or(0.0),
                }
            })
            .collect();
        out.sort_by(|a, b| (&a.graph, &a.node).cmp(&(&b.graph, &b.node)));
        out
    }

    /// Open `key` (or use it if open) and read its digest for `seconds` of
    /// wall time, answering the mean onset (flux) — the `probar` button.
    /// Opens and drops a listener of its own when `key` is not in the pool,
    /// so probing never changes what any node reads.
    pub fn probe(&mut self, key: &SourceKey, seconds: f32) -> Result<f32, String> {
        self.failed.remove(key);
        let mut own = None;
        if !self.open.contains_key(key) {
            own = Some((self.opener)(key)?);
        }
        let input: &mut Input = match own.as_mut() {
            Some(i) => i,
            None => self.open.get_mut(key).expect("checked above"),
        };
        let start = Instant::now();
        let (mut sum, mut n) = (0.0f32, 0u32);
        while start.elapsed().as_secs_f32() < seconds {
            let t = start.elapsed().as_secs_f32();
            sum += input.poll(t).onset();
            n += 1;
            std::thread::sleep(Duration::from_secs_f32(1.0 / crate::digest::RATE));
        }
        Ok(if n == 0 { 0.0 } else { sum / n as f32 })
    }
}

/// The plain clock input — what every path that only needs a beat count
/// (`State::on_air`, the Looper routes) used to get from `Shared.sim`.
impl std::ops::Deref for Listeners {
    type Target = Input;
    fn deref(&self) -> &Input {
        match self.open.get(&self.seed) {
            Some(i) => i,
            None => &self.silent,
        }
    }
}

impl std::ops::DerefMut for Listeners {
    fn deref_mut(&mut self) -> &mut Input {
        self.clock()
    }
}

/// This frame's digests: one per audio node of the graph being drawn, plus
/// the clock frame.
#[derive(Clone, Debug)]
pub struct Digests {
    by_node: HashMap<String, Frame>,
    clock: Frame,
}

impl Digests {
    /// No audio at all: the row a static source reads.
    pub fn zero() -> Frame {
        Frame::step([0.0; FEATURES])
    }

    /// Only a clock frame, no audio nodes — for paths with no graph.
    pub fn clock_only(clock: Frame) -> Digests {
        Digests { by_node: HashMap::new(), clock }
    }

    pub fn clock(&self) -> &Frame {
        &self.clock
    }

    /// The frame audio node `id` produced this frame.
    pub fn node(&self, id: &str) -> Option<&Frame> {
        self.by_node.get(id)
    }

    /// The frame of the audio node wired (upstream) to `node`'s `control`
    /// port, or `None` when nothing audio is — a **static** source. Walks
    /// control cables upstream (through an `ai.*`, a `script.*`) to the
    /// first audio node; never falls back to some other audio node in the
    /// graph.
    pub fn wired(&self, graph: &metal_graph::Graph, node: &str) -> Option<&Frame> {
        let mut todo: Vec<&str> = vec![node];
        let mut seen: Vec<&str> = Vec::new();
        while let Some(at) = todo.pop() {
            if seen.contains(&at) {
                continue;
            }
            seen.push(at);
            for e in graph.edges.iter().filter(|e| e.to.node == at && e.to.port == "control") {
                if let Some(f) = self.by_node.get(&e.from.node) {
                    return Some(f);
                }
                todo.push(e.from.node.as_str());
            }
        }
        None
    }

    /// The raw row for the consumers keyed by graph rather than by source
    /// (bindings, scripts, an effect's raw ingest, fixtures): the frame of
    /// the graph's first audio node in graph order, or the clock frame when
    /// it has none. Exact for a graph with one audio node — every graph the
    /// migration writes.
    pub fn graph_raw(&self, graph: &metal_graph::Graph) -> [f32; FEATURES] {
        graph
            .nodes
            .iter()
            .find_map(|n| self.by_node.get(&n.id))
            .unwrap_or(&self.clock)
            .raw
    }

    /// [`Digests::graph_raw`] as a whole frame.
    pub fn graph_frame(&self, graph: &metal_graph::Graph) -> Frame {
        *graph.nodes.iter().find_map(|n| self.by_node.get(&n.id)).unwrap_or(&self.clock)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn audio(g: &mut metal_graph::Graph, source: Option<&str>) -> String {
        let id = g.add_node("audio", AUDIO_LIVE, [0.0, 0.0]);
        let cfg = match source {
            Some(s) => format!("{{\"source\":{}}}", serde_json::to_string(s).unwrap()),
            None => "{\"source\":null}".into(),
        };
        g.node_mut(&id).unwrap().config = metal_graph::Config::from_json_str(&cfg).unwrap();
        id
    }

    /// A pool whose opener records each open and refuses names starting `bad`.
    fn pool(seed: SourceKey) -> (Listeners, Arc<Mutex<Vec<SourceKey>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let l2 = Arc::clone(&log);
        let l = Listeners::with_opener(
            seed,
            Box::new(move |k: &SourceKey| {
                if k.as_pick().starts_with("bad") {
                    return Err(format!("no such source: {}", k.as_pick()));
                }
                l2.lock().unwrap().push(k.clone());
                let mut i = Input::stand_in();
                // A different tempo per source, so frames differ by source.
                i.set_bpm(60.0 + 7.0 * k.as_pick().len() as f32);
                Ok(i)
            }),
        );
        (l, log)
    }

    #[test]
    fn two_audio_nodes_with_two_sources_open_two_listeners_once() {
        let (mut l, log) = pool(SourceKey::Default);
        let mut g = metal_graph::Graph::new("g", "g", metal_graph::Domain::Live);
        audio(&mut g, Some("mic-a"));
        audio(&mut g, Some("mic-bb"));
        let clock = Digests::zero();
        for i in 0..20 {
            l.digests(&g, clock, i as f32 * 0.1);
        }
        let opened = log.lock().unwrap().clone();
        // The seed (default) at start, then each named source exactly once.
        assert_eq!(opened.iter().filter(|k| k.as_pick() == "mic-a").count(), 1);
        assert_eq!(opened.iter().filter(|k| k.as_pick() == "mic-bb").count(), 1);
        assert_eq!(l.open_count(), 3, "seed + two sources");
    }

    #[test]
    fn switching_the_source_replaces_the_listener_without_a_restart() {
        let (mut l, _log) = pool(SourceKey::Default);
        let mut g = metal_graph::Graph::new("g", "g", metal_graph::Domain::Live);
        let a = audio(&mut g, Some("mic-a"));
        l.digests(&g, Digests::zero(), 0.0);
        assert!(l.is_open(&SourceKey::Named("mic-a".into())));
        g.node_mut(&a).unwrap().config = metal_graph::Config::from_json_str("{\"source\":\"mic-b\"}").unwrap();
        l.digests(&g, Digests::zero(), 0.1);
        assert!(l.is_open(&SourceKey::Named("mic-b".into())));
        assert!(!l.is_open(&SourceKey::Named("mic-a".into())), "the old listener is released");
        assert!(l.is_open(&SourceKey::Default), "the seed stays: it is the clock");
    }

    #[test]
    fn a_failed_open_keeps_the_current_source_and_lints() {
        let (mut l, _log) = pool(SourceKey::Default);
        let mut g = metal_graph::Graph::new("g", "g", metal_graph::Domain::Live);
        let a = audio(&mut g, Some("mic-a"));
        l.digests(&g, Digests::zero(), 0.0);
        g.node_mut(&a).unwrap().config = metal_graph::Config::from_json_str("{\"source\":\"bad-one\"}").unwrap();
        let errors_before = l.errors;
        for i in 0..5 {
            l.digests(&g, Digests::zero(), 0.1 * i as f32);
        }
        assert_eq!(l.errors, errors_before + 1, "a failed source is tried once, not every frame");
        assert!(l.is_open(&SourceKey::Named("mic-a".into())), "the open source stays open");
        let st = l.status().into_iter().find(|s| s.node == a).unwrap();
        assert_eq!(st.using, Some(SourceKey::Named("mic-a".into())));
        assert!(st.error.as_deref().is_some_and(|e| e.contains("bad-one")));
        assert!(l.error_of("g", &a).is_some());
    }

    #[test]
    fn a_failed_open_with_nothing_before_reads_a_zero_row() {
        let (mut l, _log) = pool(SourceKey::Default);
        let mut g = metal_graph::Graph::new("g", "g", metal_graph::Domain::Live);
        let a = audio(&mut g, Some("bad"));
        let d = l.digests(&g, Digests::zero(), 1.0);
        assert_eq!(d.node(&a).unwrap().raw, [0.0; FEATURES]);
    }

    #[test]
    fn the_cli_seed_fills_only_null_sources() {
        let (mut l, _log) = pool(SourceKey::Named("from-cli".into()));
        let mut g = metal_graph::Graph::new("g", "g", metal_graph::Domain::Live);
        let null = audio(&mut g, None);
        let named = audio(&mut g, Some("mic-a"));
        l.digests(&g, Digests::zero(), 0.0);
        let st = l.status();
        let of = |id: &str| st.iter().find(|s| s.node == id).unwrap().using.clone();
        assert_eq!(of(&null), Some(SourceKey::Named("from-cli".into())));
        assert_eq!(of(&named), Some(SourceKey::Named("mic-a".into())));
    }

    #[test]
    fn a_null_source_reads_the_clock_frame_through_the_looper() {
        let (mut l, _log) = pool(SourceKey::Default);
        let mut g = metal_graph::Graph::new("g", "g", metal_graph::Domain::Live);
        let a = audio(&mut g, None);
        let mut looper = Looper::new();
        let clock = l.clock_frame(&mut looper, 2.0);
        let d = l.digests(&g, clock, 2.0);
        assert_eq!(d.node(&a).unwrap().raw, clock.raw);
    }

    #[test]
    fn the_stand_in_entry_is_deliberate_and_not_real() {
        let (mut l, _log) = pool(SourceKey::Default);
        let mut g = metal_graph::Graph::new("g", "g", metal_graph::Domain::Live);
        audio(&mut g, Some(STAND_IN));
        l.digests(&g, Digests::zero(), 0.0);
        let st = l.status();
        assert_eq!(st[0].using, Some(SourceKey::StandIn));
        assert!(!st[0].real);
    }

    #[test]
    fn the_short_name_is_the_tail_of_the_source() {
        assert_eq!(SourceKey::Named("alsa_output.pci-0000.analog-stereo.monitor".into()).short(), "analog-stereo");
        assert_eq!(SourceKey::Default.short(), "default");
        assert_eq!(SourceKey::StandIn.short(), STAND_IN);
    }
}
