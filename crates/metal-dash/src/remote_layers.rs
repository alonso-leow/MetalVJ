//! Remote layers, without waiting for them.
//!
//! A layer whose `render_on` names another machine is drawn there. Each
//! composed frame asks for that layer's *next* frame and composes the newest
//! picture that has arrived, whatever its frame number: the compose never
//! blocks on a peer. One worker thread per (node, layer, size) does the
//! fetch and the JPEG decode, and coalesces its queue to the newest job, so a
//! slow peer costs a stale picture, never a stall.
//!
//! The ask carries the requester's state (model, float row, time), so the
//! peer draws what this machine resolved, not what it happens to hold.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::monitor::percent_encode;
use metal_perform::engine::RemoteAsk;

/// Seconds per frame of a layer's own clock (`call::render::LAYER_FPS`). The
/// step a prefetch assumes until two composes have been measured.
const FRAME_SECS: f32 = 1.0 / 60.0;

/// Weight of the newest compose gap in the running interval estimate.
const GAP_WEIGHT: f32 = 0.125;

/// A gap longer than this is a pause (a hidden layer, a stalled window), not
/// the compose rate, and is not learned.
const MAX_GAP: Duration = Duration::from_millis(250);

/// The compose rate of one layer, learned from the real gaps between its
/// composes (an exponential moving average). The prefetch steps the
/// generator's clock by this much per compose, times `fx.speed`, so the
/// picture composed at a frame is drawn at the clock that frame runs on, on
/// a machine whose frames arrive at 45 Hz as well as on one at 60.
#[derive(Default)]
struct ComposeClock {
    last: Option<Instant>,
    interval: Option<f32>,
}

impl ComposeClock {
    /// Records a compose at `now`, and returns the expected seconds to the next one.
    fn tick(&mut self, now: Instant) -> f32 {
        if let Some(last) = self.last {
            let gap = now.saturating_duration_since(last);
            if !gap.is_zero() && gap <= MAX_GAP {
                let dt = gap.as_secs_f32();
                self.interval = Some(match self.interval {
                    None => dt,
                    Some(e) => e + GAP_WEIGHT * (dt - e),
                });
            }
        }
        self.last = Some(now);
        self.step()
    }

    /// Seconds per compose: the measured rate, or one frame of 60 Hz before any.
    fn step(&self) -> f32 {
        self.interval.unwrap_or(FRAME_SECS)
    }
}

/// The one wait a layer's first request is allowed: long enough for a LAN
/// peer to answer the frame being composed, short enough not to stall the
/// compose. A peer slower than this still lands its picture a frame late.
pub const PRIME_WAIT: Duration = Duration::from_millis(50);

/// What a worker fetches: the layer's picture at `frame`, drawn from `row`.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub node: String,
    pub layer: String,
    pub frame: u64,
    pub w: u32,
    pub h: u32,
    pub model: usize,
    pub row: Vec<f32>,
    pub chain: Vec<metal_schema::EffectStep>,
    pub time: f32,
    /// The source's `fondo_alpha`, applied by the peer as the local one is.
    pub background: bool,
}

/// A decoded picture: `(w, h, RGB)`.
pub type Picture = (u32, u32, Vec<u8>);

/// The newest picture a worker has produced, and the frame it is for.
#[derive(Clone)]
struct Arrived {
    frame: u64,
    picture: Arc<Picture>,
}

/// The newest arrived picture, and the condvar a worker signals it on.
type Latest = Arc<(Mutex<Option<Arrived>>, Condvar)>;

struct Slot {
    jobs: Sender<Job>,
    latest: Latest,
    /// Set by the first request. That one asks for the frame being composed
    /// and waits [`PRIME_WAIT`] for it; every later one asks for the next
    /// frame (the one-frame prefetch lag) and never waits.
    primed: bool,
    clock: ComposeClock,
}

/// One per process in use (see [`global`]); tests build their own with a
/// fake `fetch`.
pub struct Prefetcher {
    fetch: fn(&Job) -> Option<Picture>,
    slots: Mutex<HashMap<String, Slot>>,
}

impl Prefetcher {
    pub fn new(fetch: fn(&Job) -> Option<Picture>) -> Prefetcher {
        Prefetcher { fetch, slots: Mutex::new(HashMap::new()) }
    }

    /// The remote layer's picture for this frame. The first request of a
    /// layer asks for this frame and waits up to [`PRIME_WAIT`], so the layer
    /// appears from frame 1 when the peer is quick. After that, each request
    /// queues the next frame (drawn from this frame's state) and returns the
    /// newest picture already arrived, or `None` while none has: the compose
    /// never blocks on a peer, and a picture is one frame behind.
    pub fn frame(&self, ask: &RemoteAsk) -> Option<Picture> {
        self.frame_at(ask, Instant::now())
    }

    /// [`Prefetcher::frame`] with the compose's own instant, so the rate it
    /// learns is the one the caller composes at.
    pub fn frame_at(&self, ask: &RemoteAsk, now: Instant) -> Option<Picture> {
        let key = slot_key(ask.node, &ask.layer.id, ask.w, ask.h);
        let (jobs, latest, first, step) = {
            let mut slots = self.slots.lock().ok()?;
            let slot = slots.entry(key).or_insert_with(|| self.spawn());
            let first = !slot.primed;
            slot.primed = true;
            let step = slot.clock.tick(now);
            (slot.jobs.clone(), Arc::clone(&slot.latest), first, step)
        };
        let job = Job {
            node: ask.node.to_string(),
            layer: ask.layer.id.clone(),
            frame: if first { ask.frame } else { ask.frame + 1 },
            w: ask.w,
            h: ask.h,
            model: ask.model,
            row: ask.row.to_vec(),
            chain: ask.chain.to_vec(),
            // The next frame's clock: the compose interval measured here times
            // the generator's rate, so an `fx.speed` below 1 is not overtaken
            // by the show clock and a compose that runs slow is not assumed fast.
            time: if first { ask.time } else { ask.time + step * ask.speed },
            background: ask.fondo_alpha,
        };
        let (held, ready) = &*latest;
        if !first {
            // What has arrived so far, read before the next frame is asked for:
            // a peer quick enough to answer inside this compose must not hand
            // back the frame one ahead of the one being composed.
            let arrived = held.lock().ok()?.clone()?;
            let _ = jobs.send(job);
            return Some((*arrived.picture).clone());
        }
        // The first request asks for this frame and waits for it, up to PRIME_WAIT.
        let _ = jobs.send(job);
        let mut held = held.lock().ok()?;
        let deadline = Instant::now() + PRIME_WAIT;
        while held.is_none() {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else { break };
            let (guard, timeout) = ready.wait_timeout(held, left).ok()?;
            held = guard;
            if timeout.timed_out() {
                break;
            }
        }
        let arrived = held.clone()?;
        Some((*arrived.picture).clone())
    }

    fn spawn(&self) -> Slot {
        let (jobs, rx) = mpsc::channel::<Job>();
        let latest: Latest = Arc::new((Mutex::new(None), Condvar::new()));
        let fetch = self.fetch;
        let sink = Arc::clone(&latest);
        std::thread::spawn(move || work(rx, fetch, sink));
        Slot { jobs, latest, primed: false, clock: ComposeClock::default() }
    }
}

/// The slot a (node, layer, size) request is served by.
fn slot_key(node: &str, layer: &str, w: u32, h: u32) -> String {
    format!("{node}\u{1}{layer}\u{1}{w}x{h}")
}

/// Process-wide remote-frame counters, surfaced by `GET /perf` through
/// [`remote_stats`]. Cumulative since the process started: how long a peer
/// fetch and its decode took, how many pictures came back, and how many frames
/// were dropped — a job the worker superseded in its queue, a picture older
/// than the one already held, or a fetch/decode that failed. Atomics: the
/// worker touches these once per frame and takes no other lock to do it.
#[derive(Default)]
struct Metrics {
    /// Fetch attempts: successful + failed. The denominator for the means.
    attempts: AtomicU64,
    fetched: AtomicU64,
    fetch_us: AtomicU64,
    decode_us: AtomicU64,
    dropped: AtomicU64,
}

impl Metrics {
    /// One fetch attempt: how long the peer GET took, how long the decode
    /// took, and whether a picture came back.
    fn record(&self, fetch: Duration, decode: Duration, ok: bool) {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        self.fetch_us.fetch_add(fetch.as_micros() as u64, Ordering::Relaxed);
        self.decode_us.fetch_add(decode.as_micros() as u64, Ordering::Relaxed);
        if ok {
            self.fetched.fetch_add(1, Ordering::Relaxed);
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// One prefetched frame that never reached a compose: a newer job replaced
    /// it in the worker's queue, or a newer picture was already held.
    fn drop_frame(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

fn metrics() -> &'static Metrics {
    static M: OnceLock<Metrics> = OnceLock::new();
    M.get_or_init(Metrics::default)
}

/// The remote-frame counters for the `GET /perf` snapshot, joined there as the
/// `remote` object: mean fetch and decode milliseconds and the fetched/dropped
/// totals since the process started. The means divide by every attempt, so a
/// failed fetch (which still cost time) is not hidden.
pub fn remote_stats() -> serde_json::Value {
    let m = metrics();
    let attempts = m.attempts.load(Ordering::Relaxed);
    let fetched = m.fetched.load(Ordering::Relaxed);
    let fetch_us = m.fetch_us.load(Ordering::Relaxed);
    let decode_us = m.decode_us.load(Ordering::Relaxed);
    let mean_ms = |total_us: u64| {
        if attempts == 0 { 0.0 } else { total_us as f64 / attempts as f64 / 1000.0 }
    };
    serde_json::json!({
        "fetched": fetched,
        "dropped": m.dropped.load(Ordering::Relaxed),
        "fetch_ms": mean_ms(fetch_us),
        "decode_ms": mean_ms(decode_us),
        "fetch_total_ms": fetch_us as f64 / 1000.0,
        "decode_total_ms": decode_us as f64 / 1000.0,
    })
}

/// The worker loop: take the newest queued job, fetch it, keep it if it is
/// newer than what is already held. Exits when the requester side is gone.
fn work(rx: Receiver<Job>, fetch: fn(&Job) -> Option<Picture>, latest: Latest) {
    while let Ok(mut job) = rx.recv() {
        while let Ok(newer) = rx.try_recv() {
            job = newer;
            // A queued job the newer one replaced: a dropped frame.
            metrics().drop_frame();
        }
        let Some(picture) = fetch(&job) else { continue };
        let (held, ready) = &*latest;
        if let Ok(mut held) = held.lock() {
            let newer = held.as_ref().is_none_or(|h| job.frame >= h.frame);
            if newer {
                *held = Some(Arrived { frame: job.frame, picture: Arc::new(picture) });
                ready.notify_all();
            } else {
                // Fetched, but a newer picture is already held: dropped.
                metrics().drop_frame();
            }
        }
    }
}

/// The process-wide prefetcher the compose path uses.
pub fn global() -> &'static Prefetcher {
    static PREFETCH: OnceLock<Prefetcher> = OnceLock::new();
    PREFETCH.get_or_init(|| Prefetcher::new(fetch_from_peer))
}

/// The codec a remote frame asks for, by the path the peer is reached on
/// (`metal-sync`'s `/peers` `via`). A direct LAN path has the bandwidth, so raw
/// RGBA: the cheapest encode and an exact copy. A relay pays for every byte,
/// so QOI (lossless, a few ms to encode). Unknown path: QOI. JPEG and the
/// other two are only used when asked, through `FW_REMOTE_FMT`.
pub fn choose_fmt(via: Option<&str>, asked: Option<&str>) -> &'static str {
    match asked {
        Some("jpeg") => "jpeg",
        Some("qoi") => "qoi",
        Some("rgba") => "rgba",
        _ => match via {
            Some("direct") => "rgba",
            _ => "qoi",
        },
    }
}

/// What the operator asked for on this machine (`FW_REMOTE_FMT`), if anything.
fn asked_fmt() -> Option<String> {
    std::env::var("FW_REMOTE_FMT").ok()
}

/// How a peer is reached now, from `metal-sync`'s `/peers`. Cached for a
/// second, so a 60 fps layer costs one lookup a second, not one a frame.
/// Called on the worker thread, never on the compose path.
fn peer_via(node: &str) -> Option<String> {
    const TTL: Duration = Duration::from_secs(1);
    type Cache = (Option<Instant>, HashMap<String, String>);
    static VIA: OnceLock<Mutex<Cache>> = OnceLock::new();
    let cache = VIA.get_or_init(|| Mutex::new((None, HashMap::new())));
    let mut cache = cache.lock().ok()?;
    if cache.0.is_none_or(|at| at.elapsed() > TTL) {
        cache.0 = Some(Instant::now());
        cache.1 = fetch_vias().unwrap_or_default();
    }
    cache.1.get(node).cloned()
}

/// Every peer's `via`, by name, from `GET /peers` on `metal-sync`.
fn fetch_vias() -> Option<HashMap<String, String>> {
    let bytes = crate::monitor::fetch_sync_bytes("/peers").ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some(
        v["peers"]
            .as_array()?
            .iter()
            .filter_map(|p| Some((p["name"].as_str()?.to_string(), p["via"].as_str()?.to_string())))
            .collect(),
    )
}

/// The `/frame` path for one job, asking for `fmt`.
pub fn frame_path(job: &Job, fmt: &str) -> String {
    let row: Vec<String> = job
        .row
        .iter()
        .map(|f| if f.is_finite() { f.to_string() } else { "0".to_string() })
        .collect();
    let chain = if job.chain.is_empty() {
        String::new()
    } else {
        format!("&chain={}", metal_schema::chain_to_query(&job.chain))
    };
    let background = if job.background { "&fondo=1" } else { "" };
    format!(
        "/frame?layer={}&frame={}&on={}&w={}&h={}&model={}&time={}&row={}{chain}{background}&fmt={fmt}",
        percent_encode(&job.layer),
        job.frame,
        percent_encode(&job.node),
        job.w,
        job.h,
        job.model,
        job.time,
        row.join(",")
    )
}

/// The rows below which splitting a frame into strips buys nothing: a short
/// frame's whole encode is cheaper than the threads that would cut it up.
const STRIP_MIN_ROWS: u32 = 48;
/// The most strips one frame is split into, whatever the core count.
const MAX_STRIPS: u32 = 16;

/// The QOI body for a frame this machine answers a peer's `GET` with: the
/// 720p encode is tens of milliseconds on an idle box and more under load
/// (`qoi::tests::bench_720p`), so it runs as independent strips encoded one
/// per core off the route's thread (`qoi::encode_parallel`). One strip, no
/// threads, for a single-core machine or a frame too short to split. A
/// direct-path `fmt=rgba` never reaches here; one GET per frame is unchanged.
pub fn encode_qoi_frame(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as u32;
    let strips = if cores < 2 || h < STRIP_MIN_ROWS { 1 } else { cores.min(h).min(MAX_STRIPS) };
    crate::qoi::encode_parallel(rgba, w, h, strips)
}

/// A picture as it came off the wire, read by the codec asked for. Raw RGBA
/// is taken as such when its length is exactly `w*h*4`; otherwise the magic
/// bytes decide, so a peer that predates the negotiation (JPEG) still composes.
///
/// A strip container (`qois`) is a QOI body too, read the same way.
pub fn decode_wire(bytes: &[u8], w: u32, h: u32, fmt: &str) -> Option<Picture> {
    if fmt == "rgba" && bytes.len() == w as usize * h as usize * 4 {
        return Some((w, h, crate::qoi::rgb_of(bytes)));
    }
    if bytes.starts_with(b"qoif") || bytes.starts_with(b"qois") {
        let (w, h, rgba) = crate::qoi::decode(bytes)?;
        return Some((w, h, crate::qoi::rgb_of(&rgba)));
    }
    crate::shell::frames::decode_jpeg_rgb(bytes)
}

/// One layer frame from the peer, through `metal-sync` (the requester's only
/// way to a peer): `GET /frame` with the layer, frame, size, state and the
/// codec the path calls for (see [`choose_fmt`]). `None` when `metal-sync` is
/// not up, the peer does not answer, or the picture does not decode at the
/// size asked for.
pub fn fetch_from_peer(job: &Job) -> Option<Picture> {
    let fmt = choose_fmt(peer_via(&job.node).as_deref(), asked_fmt().as_deref());
    let asked = Instant::now();
    let bytes = crate::monitor::fetch_sync_bytes(&frame_path(job, fmt)).ok();
    let fetch = asked.elapsed();
    let decoding = Instant::now();
    let decoded = bytes.as_deref().and_then(|b| decode_wire(b, job.w, job.h, fmt));
    let decode = decoding.elapsed();
    let picture = decoded.filter(|(w, h, _)| *w == job.w && *h == job.h);
    // A fetch or decode that failed is a dropped frame, timed all the same.
    metrics().record(fetch, decode, picture.is_some());
    picture
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn ask<'a>(layer: &'a metal_perform::layers::Layer, row: &'a [f32], frame: u64) -> RemoteAsk<'a> {
        RemoteAsk { layer, node: "stage", model: 2, row, chain: &[], time: 0.5, fondo_alpha: false, speed: 1.0, frame, w: 2, h: 1 }
    }

    /// A peer that takes 400 ms per frame: the compose must not wait for it.
    fn slow_peer(job: &Job) -> Option<Picture> {
        std::thread::sleep(Duration::from_millis(400));
        Some((2, 1, vec![job.frame as u8; 6]))
    }

    #[test]
    fn a_slow_peer_never_blocks_the_compose() {
        let layer = metal_perform::layers::Layer::new("l1", "Layer", 0);
        let row = [0.5f32; 4];
        let p = Prefetcher::new(slow_peer);
        let started = Instant::now();
        // First frame: waits at most PRIME_WAIT, then says nothing has come.
        assert_eq!(p.frame(&ask(&layer, &row, 10)), None);
        assert!(started.elapsed() < PRIME_WAIT + Duration::from_millis(50), "waited on the peer");
        // Once the worker has one back, it is what the next frame composes:
        // frame 10, the one the first request asked for, one frame late.
        std::thread::sleep(Duration::from_millis(600));
        let got = p.frame(&ask(&layer, &row, 11)).expect("the first frame arrived");
        assert_eq!(got.2[0], 10, "the frame asked for first, from the state asked with");
    }

    fn echo_row(job: &Job) -> Option<Picture> {
        // The picture is a function of the state alone: the row's first float.
        Some((2, 1, vec![(job.row[0] * 100.0) as u8; 6]))
    }

    /// A peer that answers with the time it was asked for, in milliseconds.
    fn echo_time(job: &Job) -> Option<Picture> {
        let ms = (job.time * 1000.0).round() as u32;
        Some((2, 1, vec![(ms & 0xff) as u8, ((ms >> 8) & 0xff) as u8, ((ms >> 16) & 0xff) as u8, 0, 0, 0]))
    }

    /// Waits until the worker has fetched `frame` for the 2x1 slot the drift
    /// and parity tests use (or two seconds pass), so each compose sees the
    /// picture a peer that keeps up would have sent.
    fn settle(p: &Prefetcher, w: u32, h: u32, frame: u64) {
        let key = slot_key("stage", "l1", w, h);
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            let arrived = p
                .slots
                .lock()
                .unwrap()
                .get(&key)
                .and_then(|s| s.latest.0.lock().unwrap().as_ref().map(|a| a.frame));
            if arrived.is_some_and(|f| f >= frame) || Instant::now() >= until {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Composes `frames` frames through `p`, the `f`th at the instant `gaps(f)`
    /// after the one before, at the node's clock for that instant. Returns the
    /// worst distance, in seconds, between the time a picture composed from
    /// frame 3 on was drawn at and the node's clock for the frame it is
    /// composed for. The first compose after priming is one frame behind by
    /// design, so it is not measured.
    fn worst_drift(p: &Prefetcher, speed: f32, gaps: &dyn Fn(u64) -> Duration, frames: u64) -> f32 {
        let layer = metal_perform::layers::Layer::new("l1", "Layer", 0);
        let row = [0.5f32; 4];
        let mut now = Instant::now();
        let mut elapsed = 0.0f64;
        let mut worst = 0.0f32;
        for f in 1..=frames {
            if f > 1 {
                let gap = gaps(f);
                now += gap;
                elapsed += gap.as_secs_f64();
            }
            let node = 1.0 + (speed as f64 * elapsed) as f32;
            let mut a = ask(&layer, &row, f);
            a.time = node;
            a.speed = speed;
            let got = p.frame_at(&a, now);
            settle(p, 2, 1, if f == 1 { 1 } else { f + 1 });
            if f < 3 {
                continue;
            }
            let (_, _, rgb) = got.expect("a picture has arrived");
            let ms = rgb[0] as u32 | (rgb[1] as u32) << 8 | (rgb[2] as u32) << 16;
            worst = worst.max((ms as f32 / 1000.0 - node).abs());
        }
        worst
    }

    /// The prefetch steps a layer's time by the compose interval it measured,
    /// times `fx.speed`: at 60 Hz and speed 0.5 the picture composed at frame
    /// `f` is drawn at the node's clock for `f`.
    #[test]
    fn a_prefetched_frame_follows_the_generators_clock_rate() {
        let p = Prefetcher::new(echo_time);
        let worst = worst_drift(&p, 0.5, &|_| Duration::from_nanos(16_666_667), 90);
        assert!(worst < 0.002, "the picture drifts {worst:.4} s from the generator's clock");
    }

    /// The compose clock learns the rate the composes really come at, not one
    /// frame of 60 Hz: a 45 Hz compose with jitter that averages to zero is
    /// learned to within a millisecond, and a pause does not move it.
    #[test]
    fn the_compose_clock_learns_a_jittered_rate_and_ignores_a_pause() {
        let mut clock = ComposeClock::default();
        let mut at = Instant::now();
        assert_eq!(clock.tick(at), FRAME_SECS, "no gap measured yet: one frame of 60 Hz");
        let nominal = 1.0f32 / 45.0;
        let jitter = [0.003f32, -0.003, 0.002, -0.002];
        for k in 0..400 {
            at += Duration::from_secs_f32(nominal + jitter[k % 4]);
            clock.tick(at);
        }
        assert!((clock.step() - nominal).abs() < 0.001, "learned {:.5} s, the rate is {nominal:.5} s", clock.step());
        at += Duration::from_secs(2);
        clock.tick(at);
        assert!((clock.step() - nominal).abs() < 0.001, "a 2 s pause moved the clock to {:.5} s", clock.step());
    }

    /// Jittered composes at 45 Hz, `fx.speed` 0.5: the prefetch follows the
    /// measured rate, so the picture stays on the node's clock (within the
    /// jitter), where a fixed 60 Hz step would fall ~0.1 s behind in 90 frames.
    #[test]
    fn a_prefetch_on_jittered_composes_stays_on_the_generators_clock() {
        let jitter = [0.003f64, -0.003, 0.002, -0.002];
        let gap = |f: u64| Duration::from_secs_f64(1.0 / 45.0 + jitter[(f % 4) as usize]);
        let worst = worst_drift(&Prefetcher::new(echo_time), 0.5, &gap, 90);
        assert!(worst < 0.004, "the picture drifts {worst:.4} s from the generator's clock");
    }

    /// A fake peer that draws the state it is asked with, as a peer's
    /// `GET /render` does, and answers in QOI the way a relay path would.
    fn qoi_peer(job: &Job) -> Option<Picture> {
        // Leaked, not dropped: the worker thread ends with the test, and a
        // painter torn down in a thread-local destructor aborts the process.
        thread_local! {
            static PEER: std::cell::RefCell<&'static mut dyn metal_perform::paint::Paint> =
                std::cell::RefCell::new(Box::leak(metal_perform::paint::painter()));
        }
        let (rgba, w, h) = PEER.with(|p| {
            p.borrow_mut().solo_chain(job.model, &job.row, job.time, job.w, job.h, job.background, &job.chain)
        });
        decode_wire(&crate::qoi::encode(&rgba, w, h), w, h, "qoi")
    }

    /// The prefetched path against the same layer drawn here at the same
    /// times. A fake peer answers the prefetch through the codec; from the
    /// third compose on, the picture composed is the local draw at that
    /// frame's node clock, channel for channel (within the engine's parity bound).
    #[test]
    fn a_prefetched_picture_matches_the_layer_drawn_here_at_the_same_times() {
        let layer = metal_perform::layers::Layer::new("l1", "Layer", 0);
        let mut local = metal_perform::paint::painter();
        let width = metal_perform::engine::State::new(local.as_ref()).base[0].len();
        let row = vec![0.5f32; width];
        let (w, h, speed) = (16u32, 9u32, 0.5f32);
        let node_time = |f: u64| 1.0 + speed * f as f32 / 60.0;
        let p = Prefetcher::new(qoi_peer);
        let t0 = Instant::now();
        let mut compared = 0;
        for f in 1..=6u64 {
            let now = t0 + Duration::from_nanos((f - 1) * 16_666_667);
            let a = RemoteAsk {
                layer: &layer,
                node: "stage",
                model: 0,
                row: &row,
                chain: &[],
                time: node_time(f),
                fondo_alpha: true,
                speed,
                frame: f,
                w,
                h,
            };
            let got = p.frame_at(&a, now);
            settle(&p, w, h, if f == 1 { 1 } else { f + 1 });
            if f < 3 {
                continue;
            }
            let (pw, ph, rgb) = got.expect("a prefetched picture");
            let (lrgba, lw, lh) = local.solo_chain(0, &row, node_time(f), w, h, true, &[]);
            assert_eq!((pw, ph), (lw, lh));
            let lrgb: Vec<u8> = lrgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
            let worst = rgb.iter().zip(&lrgb).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap_or(0);
            assert!(worst <= 2, "frame {f}: the prefetched picture differs from the local draw by {worst}");
            compared += 1;
        }
        assert_eq!(compared, 4);
    }

    /// A quick peer: the first frame is there from frame 1, with no lag.
    #[test]
    fn a_quick_peer_puts_the_layer_on_frame_one() {
        let layer = metal_perform::layers::Layer::new("l1", "Layer", 0);
        let p = Prefetcher::new(echo_row);
        let got = p.frame(&ask(&layer, &[0.5; 4], 1)).expect("frame 1 has its layer");
        assert_eq!(got.2[0], 50);
    }

    #[test]
    fn the_newest_picture_wins_and_is_the_one_its_state_made() {
        let layer = metal_perform::layers::Layer::new("l1", "Layer", 0);
        let p = Prefetcher::new(echo_row);
        p.frame(&ask(&layer, &[0.25; 4], 1));
        std::thread::sleep(Duration::from_millis(50));
        let got = p.frame(&ask(&layer, &[0.75; 4], 2)).expect("a picture arrived");
        // Whatever arrived last is what is composed: here the one asked with 0.25.
        assert!(got.2[0] == 25 || got.2[0] == 75);
    }

    /// The codec follows the path: raw on a direct LAN path, QOI through a
    /// relay or when the path is unknown. JPEG (or any other) only when asked.
    #[test]
    fn the_codec_follows_the_path_and_jpeg_only_when_asked() {
        assert_eq!(choose_fmt(Some("direct"), None), "rgba");
        assert_eq!(choose_fmt(Some("relay"), None), "qoi");
        assert_eq!(choose_fmt(None, None), "qoi", "an unknown path gets the lossless codec");
        assert_eq!(choose_fmt(Some("direct"), Some("jpeg")), "jpeg");
        assert_eq!(choose_fmt(Some("relay"), Some("rgba")), "rgba", "an asked codec wins over the path");
        assert_eq!(choose_fmt(Some("direct"), Some("bogus")), "rgba", "an unknown ask is ignored");
    }

    /// Each codec reads back to the picture it carried, through the path the
    /// requester picked. A JPEG from an older peer still decodes under `rgba`.
    #[test]
    fn each_codec_reads_back_its_own_picture() {
        let (w, h) = (4u32, 3u32);
        let rgba: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
        let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        assert_eq!(decode_wire(&rgba, w, h, "rgba"), Some((w, h, rgb.clone())), "raw is exact");
        assert_eq!(decode_wire(&crate::qoi::encode(&rgba, w, h), w, h, "qoi"), Some((w, h, rgb.clone())), "qoi is exact");
        // The body the frame route now sends: strip-parallel QOI, one GET.
        assert_eq!(
            decode_wire(&encode_qoi_frame(&rgba, w, h), w, h, "qoi"),
            Some((w, h, rgb.clone())),
            "a strip body is exact too"
        );
        let jpeg = crate::paint::encode_jpeg(&rgba, w, h);
        assert!(decode_wire(&jpeg, w, h, "rgba").is_some(), "a JPEG peer still composes");
        assert_eq!(decode_wire(&rgba[..10], w, h, "rgba"), None, "a short body is not a picture");
    }

    /// `GET /perf` carries the remote-frame counters, and a fetch/decode
    /// record moves them: the fetched and dropped totals and the means.
    #[test]
    fn the_perf_snapshot_reports_remote_fetch_decode_and_drops() {
        let before = crate::perf::snapshot();
        let fetched = before["remote"]["fetched"].as_u64().expect("a fetched counter");
        let dropped = before["remote"]["dropped"].as_u64().expect("a dropped counter");
        assert!(before["remote"]["fetch_ms"].as_f64().is_some(), "fetch_ms is a number");
        assert!(before["remote"]["decode_ms"].as_f64().is_some(), "decode_ms is a number");

        // What one successful and one failed fetch/decode cost.
        metrics().record(Duration::from_millis(6), Duration::from_millis(2), true);
        metrics().record(Duration::from_millis(1), Duration::from_millis(1), false);
        metrics().drop_frame();

        let after = crate::perf::snapshot();
        assert_eq!(after["remote"]["fetched"].as_u64().unwrap(), fetched + 1);
        assert_eq!(after["remote"]["dropped"].as_u64().unwrap(), dropped + 2);
        assert!(after["remote"]["fetch_ms"].as_f64().unwrap() > 0.0, "the fetch mean moved");
        assert!(after["remote"]["decode_ms"].as_f64().unwrap() > 0.0, "the decode mean moved");
    }

    /// The `/frame` path names the codec and carries the fondo mode the source has.
    #[test]
    fn the_frame_path_asks_for_the_codec_and_the_backdrop_mode() {
        let job = Job {
            node: "stage".into(),
            layer: "l1".into(),
            frame: 3,
            w: 64,
            h: 36,
            model: 1,
            row: vec![0.5],
            chain: vec![],
            time: 0.25,
            background: true,
        };
        let path = frame_path(&job, "rgba");
        assert!(path.ends_with("&fondo=1&fmt=rgba"), "{path}");
        assert!(path.contains("&on=stage&w=64&h=36&model=1&time=0.25&row=0.5"), "{path}");
        let plain = Job { background: false, ..job };
        assert!(!frame_path(&plain, "qoi").contains("background"), "no fondo flag when the source has none");
    }
}
