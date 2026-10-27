//! P-D — the timecode readers behind `clock.timecode` nodes, and the two
//! routes the shell and the AI share (rule 6): `GET /timecode` (where the
//! incoming timecode puts the sequence playhead) and `GET /artnet/nodes`
//! (ArtPoll discovery for `out.artnet`'s properties page).
//!
//! The decoding is `metal_perform::timecode`/`artnet` (pure, tested with
//! synthesized audio and packets). This file only opens what the node's
//! config names — one reader per distinct source, shared by the nodes that
//! name it, dropped when no node does — and keeps the last timecode each
//! heard. Config is a reading of the graph (rule 4): the node list is
//! re-read from the graph files at most once a second, when `GET /timecode`
//! is asked.
//!
//! Readers: **Art-Net** a UDP thread on 6454 (shared bind); **MTC** `midir`
//! behind the `midi` feature (absent → the node reports why); **LTC**
//! `parec`/`pw-record` writing the input's channels as raw floats, the
//! node's `channel` picked out — the same pipe `metal_listen::capture`
//! measured as the one that reaches every PipeWire source.

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use web_time::{Duration, Instant};

use serde_json::{json, Value};

use metal_perform::timecode::{
    Chase, Fps, LtcDecoder, LtcGen, MtcAssembler, Timecode, TimecodeConfig, TimecodeMode, TIMECODE_KIND,
};

use crate::call::{CallError, Ctx, Params};

/// What a reader last heard: the timecode, how many frames it lagged when
/// it arrived, and when.
type Heard = Arc<Mutex<Option<(Timecode, f64, Instant)>>>;

struct Reader {
    heard: Heard,
    error: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Reader {
    fn new() -> Reader {
        Reader { heard: Arc::default(), error: Arc::default(), stop: Arc::new(AtomicBool::new(false)) }
    }
}

#[derive(Clone)]
struct Bound {
    graph: String,
    node: String,
    cfg: TimecodeConfig,
}

#[derive(Default)]
struct Registry {
    readers: HashMap<String, Reader>,
    /// Generate-mode outputs, keyed by [`TimecodeConfig::writer_key`]. Their
    /// `heard` is never written; only `error` and `stop` mean anything.
    writers: HashMap<String, Reader>,
    nodes: Vec<Bound>,
    scanned: Option<Instant>,
}

fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(Mutex::default)
}

/// Every non-bypassed `clock.timecode` node in the graph files.
fn scan(root: &std::path::Path) -> Vec<Bound> {
    let dir = metal_perform::engine::graphs_dir(root);
    let Ok(entries) = metal_vfs::read_dir(&dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in entries.flatten() {
        if e.path().extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Ok(text) = metal_vfs::read_to_string(e.path()) else { continue };
        let Ok(g) = metal_graph::Graph::from_json(&text) else { continue };
        for n in g.nodes.iter().filter(|n| n.kind == TIMECODE_KIND && !n.bypass) {
            out.push(Bound { graph: g.id.clone(), node: n.id.clone(), cfg: TimecodeConfig::from_node(n) });
        }
    }
    out.sort_by(|a, b| (&a.graph, &a.node).cmp(&(&b.graph, &b.node)));
    out
}

fn set_error(slot: &Arc<Mutex<Option<String>>>, e: impl Into<String>) {
    if let Ok(mut g) = slot.lock() {
        *g = Some(e.into());
    }
}

fn hear(slot: &Heard, tc: Timecode, late: f64) {
    if let Ok(mut g) = slot.lock() {
        *g = Some((tc, late, Instant::now()));
    }
}

/// ArtTimeCode: no socket of its own. The process has one Art-Net listener
/// (`metal_perform::artnet::listen`), which dispatches every ArtTimeCode to
/// the subscribers; this reader is one of them.
fn spawn_artnet() -> Reader {
    let r = Reader::new();
    let (heard, error, stop) = (r.heard.clone(), r.error.clone(), r.stop.clone());
    metal_perform::artnet::listen();
    let rx = metal_perform::artnet::subscribe_timecode();
    let spawned = std::thread::Builder::new().name("metal-dash timecode artnet".into()).spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            if let Some(e) = metal_perform::artnet::listen_error() {
                set_error(&error, e);
            }
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(tc) => hear(&heard, tc, 0.0),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    if let Err(e) = spawned {
        set_error(&r.error, e.to_string());
    }
    r
}

/// LTC from channel `channel` (1-based) of `input` (a Pulse source; empty
/// is the default source), 48 kHz float through `parec` or `pw-record`.
fn spawn_ltc(input: &str, channel: u16) -> Reader {
    const RATE: u32 = 48_000;
    let r = Reader::new();
    let (heard, error, stop) = (r.heard.clone(), r.error.clone(), r.stop.clone());
    let source = if input.trim().is_empty() { "@DEFAULT_SOURCE@".to_string() } else { input.trim().to_string() };
    let channels = channel.max(1);
    let spawned = std::thread::Builder::new().name("metal-dash timecode ltc".into()).spawn(move || {
        use metal_schema::platform::Command;
        use std::process::Stdio;
        let attempts: [(&str, Vec<String>); 2] = [
            (
                "parec",
                vec![
                    "--raw".into(),
                    "-d".into(),
                    source.clone(),
                    "--format=float32le".into(),
                    format!("--rate={RATE}"),
                    format!("--channels={channels}"),
                    "--latency-msec=10".into(),
                ],
            ),
            (
                "pw-record",
                vec![
                    "--raw".into(),
                    "--target".into(),
                    source.clone(),
                    "--format=f32".into(),
                    format!("--rate={RATE}"),
                    format!("--channels={channels}"),
                    "-".into(),
                ],
            ),
        ];
        let mut child = None;
        for (program, args) in attempts {
            if let Ok(c) = Command::new(program).args(&args).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
                child = Some(c);
                break;
            }
        }
        let Some(mut child) = child else {
            return set_error(&error, format!("LTC {source}: neither parec nor pw-record would start"));
        };
        let Some(mut out) = child.stdout.take() else { return };
        let mut decoder = LtcDecoder::new(RATE);
        let frame_bytes = 4 * channels as usize;
        let pick = (channels as usize - 1) * 4;
        let mut buf = vec![0u8; frame_bytes * 512];
        let mut carry: Vec<u8> = Vec::new();
        let mut mono = Vec::with_capacity(512);
        while !stop.load(Ordering::Relaxed) {
            let n = match out.read(&mut buf) {
                Ok(0) | Err(_) => {
                    set_error(&error, format!("LTC {source}: the recorder stopped"));
                    break;
                }
                Ok(n) => n,
            };
            carry.extend_from_slice(&buf[..n]);
            let whole = carry.len() / frame_bytes * frame_bytes;
            mono.clear();
            for f in carry[..whole].chunks_exact(frame_bytes) {
                mono.push(f32::from_le_bytes([f[pick], f[pick + 1], f[pick + 2], f[pick + 3]]));
            }
            carry.drain(..whole);
            // The sync word ends a frame: the instant is one frame later.
            if let Some((tc, _)) = decoder.push(&mono).pop() {
                hear(&heard, tc, 1.0);
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    });
    if let Err(e) = spawned {
        set_error(&r.error, e.to_string());
    }
    r
}

/// Sample rate of generated LTC: the one the decoder reads at.
const LTC_OUT_RATE: u32 = 48_000;

/// The transport the generators follow: the lanes playhead in seconds, and
/// whether it runs. Nothing plays while the transport is stopped.
fn transport() -> (f64, bool) {
    crate::playhead::published(metal_schema::TickRate::CANONICAL.hz() as f32)
}

/// LTC out: a playback pipe (`pw-cat`, else `paplay`) fed float32 samples
/// that [`LtcGen`] writes for every frame the playhead reaches.
fn spawn_ltc_out(output: &str, fps: Fps) -> Reader {
    let r = Reader::new();
    let (error, stop) = (r.error.clone(), r.stop.clone());
    let sink = output.trim().to_string();
    let spawned = std::thread::Builder::new().name("metal-dash timecode ltc out".into()).spawn(move || {
        use metal_schema::platform::Command;
        use std::io::Write;
        use std::process::Stdio;
        let rate = LTC_OUT_RATE;
        let mut pw = vec!["--playback".into(), "--format".into(), "f32".into(), format!("--rate={rate}"), "--channels=1".into()];
        if !sink.is_empty() {
            pw.push("--target".into());
            pw.push(sink.clone());
        }
        pw.push("-".into());
        let mut pa = vec!["--raw".into(), "--format=float32le".into(), format!("--rate={rate}"), "--channels=1".into()];
        if !sink.is_empty() {
            pa.push("-d".into());
            pa.push(sink.clone());
        }
        let attempts: [(&str, Vec<String>); 2] = [("pw-cat", pw), ("paplay", pa)];
        let mut child = None;
        for (program, args) in attempts {
            if let Ok(c) = Command::new(program).args(&args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
                child = Some(c);
                break;
            }
        }
        let Some(mut child) = child else {
            return set_error(&error, "LTC out: neither pw-cat nor paplay would start");
        };
        let Some(mut pipe) = child.stdin.take() else { return };
        let mut gen = LtcGen::new(fps, rate, 0.5);
        let mut samples = Vec::new();
        let mut bytes = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            let (at, playing) = transport();
            if playing {
                samples.clear();
                gen.due(at, &mut samples);
                bytes.clear();
                for s in &samples {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
                if pipe.write_all(&bytes).is_err() {
                    set_error(&error, "LTC out: the player stopped");
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            } else {
                crate::playhead::wait_while_stopped(Duration::from_millis(100));
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    });
    if let Err(e) = spawned {
        set_error(&r.error, e.to_string());
    }
    r
}

/// MTC out: quarter-frame messages on the MIDI output whose name contains
/// `filter`, [`MtcGen`] following the transport.
#[cfg(feature = "midi")]
fn spawn_mtc_out(filter: &str, fps: Fps) -> Reader {
    let r = Reader::new();
    let (error, stop) = (r.error.clone(), r.stop.clone());
    let filter = filter.to_string();
    let spawned = std::thread::Builder::new().name("metal-dash timecode mtc out".into()).spawn(move || {
        let out = match midir::MidiOutput::new("metalvj-mtc-out") {
            Ok(o) => o,
            Err(e) => return set_error(&error, format!("MTC out: {e}")),
        };
        let port = out.ports().into_iter().find(|p| out.port_name(p).is_ok_and(|n| n.contains(filter.as_str())));
        let Some(port) = port else {
            return set_error(&error, format!("MTC out: no MIDI output matches {filter:?}"));
        };
        let Ok(mut conn) = out.connect(&port, "metalvj-mtc-out-conn") else {
            return set_error(&error, format!("MTC out: could not open {filter:?}"));
        };
        let mut gen = MtcGen::new(fps);
        while !stop.load(Ordering::Relaxed) {
            let (at, playing) = transport();
            if playing {
                for m in gen.due(at) {
                    let _ = conn.send(&m);
                }
                std::thread::sleep(Duration::from_millis(2));
            } else {
                crate::playhead::wait_while_stopped(Duration::from_millis(100));
            }
        }
    });
    if let Err(e) = spawned {
        set_error(&r.error, e.to_string());
    }
    r
}

#[cfg(not(feature = "midi"))]
fn spawn_mtc_out(_filter: &str, _fps: Fps) -> Reader {
    let r = Reader::new();
    set_error(&r.error, "MTC out: this build has no MIDI output (--features midi)");
    r
}

fn open_writer(cfg: &TimecodeConfig) -> Reader {
    let fps = cfg.fps.unwrap_or(Fps::F25);
    match cfg.source.as_str() {
        "mtc" => spawn_mtc_out(cfg.midi_out.as_deref().unwrap_or(""), fps),
        _ => spawn_ltc_out(cfg.output.as_deref().unwrap_or(""), fps),
    }
}

/// MTC from every MIDI input whose name contains `filter`.
#[cfg(feature = "midi")]
fn spawn_mtc(filter: &str) -> Reader {
    let r = Reader::new();
    let (heard, error, stop) = (r.heard.clone(), r.error.clone(), r.stop.clone());
    let filter = filter.to_string();
    let spawned = std::thread::Builder::new().name("metal-dash timecode mtc".into()).spawn(move || {
        let probe = match midir::MidiInput::new("metalvj-mtc") {
            Ok(i) => i,
            Err(e) => return set_error(&error, format!("MTC: {e}")),
        };
        let ports: Vec<_> = probe
            .ports()
            .into_iter()
            .filter(|p| probe.port_name(p).is_ok_and(|n| n.contains(filter.as_str())))
            .collect();
        drop(probe);
        let mut conns = Vec::new();
        for port in ports {
            let Ok(mut input) = midir::MidiInput::new("metalvj-mtc") else { continue };
            // midir ignores SysEx/time messages by default; MTC is both.
            input.ignore(midir::Ignore::None);
            let heard = heard.clone();
            let mut asm = MtcAssembler::new();
            if let Ok(c) = input.connect(&port, "metalvj-mtc-in", move |_, msg, _| {
                if let Some((tc, late)) = asm.feed(msg) {
                    hear(&heard, tc, f64::from(late));
                }
            }, ()) {
                conns.push(c);
            }
        }
        if conns.is_empty() {
            return set_error(&error, format!("MTC: no MIDI input matches {filter:?}"));
        }
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(200));
        }
    });
    if let Err(e) = spawned {
        set_error(&r.error, e.to_string());
    }
    r
}

#[cfg(not(feature = "midi"))]
fn spawn_mtc(_filter: &str) -> Reader {
    let _ = MtcAssembler::new;
    let r = Reader::new();
    set_error(&r.error, "MTC: this build has no MIDI input (--features midi)");
    r
}

fn open(cfg: &TimecodeConfig) -> Reader {
    match cfg.source.as_str() {
        "artnet" => spawn_artnet(),
        "mtc" => spawn_mtc(cfg.midi_port.as_deref().unwrap_or("")),
        _ => spawn_ltc(cfg.input.as_deref().unwrap_or(""), cfg.channel),
    }
}

/// One node's reading at `now`: where its chase puts the playhead.
fn reading(b: &Bound, heard: Option<(Timecode, f64, Instant)>, error: Option<String>, now: Instant) -> Value {
    let mut chase = Chase::new(b.cfg.chase, b.cfg.freewheel_frames);
    let mut tc_text = Value::Null;
    let mut fps = b.cfg.fps.map(|f| f.label());
    if let Some((mut tc, late, at)) = heard {
        // An operator-set rate wins over the detected one (LTC at 30 vs
        // 29.97 non-drop cannot be told apart by ear).
        if let Some(f) = b.cfg.fps {
            tc.fps = f;
        }
        chase.hear(tc, late, at);
        tc_text = json!(tc.text());
        fps = Some(tc.fps.label());
    }
    let at = chase.position(now).map(|p| (p - b.cfg.offset).max(0.0));
    json!({
        "graph": b.graph,
        "node": b.node,
        "source": b.cfg.source,
        "mode": b.cfg.mode,
        "chase": b.cfg.chase,
        "locked": at.is_some(),
        "at": at,
        "timecode": tc_text,
        "fps": fps,
        "error": error,
    })
}

/// Re-reads the nodes (at most once a second), opens/drops readers, and
/// answers every node's reading plus the first locked position.
fn status(root: &std::path::Path, now: Instant) -> Value {
    let mut reg = match registry().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if reg.scanned.is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_secs(1)) {
        reg.nodes = scan(root);
        reg.scanned = Some(now);
        let wanted: Vec<(String, TimecodeConfig)> = reg
            .nodes
            .iter()
            .filter(|b| b.cfg.mode == TimecodeMode::Read)
            .map(|b| (b.cfg.reader_key(), b.cfg.clone()))
            .collect();
        reg.readers.retain(|k, _| wanted.iter().any(|(w, _)| w == k));
        for (key, cfg) in wanted {
            if !reg.readers.contains_key(&key) {
                let r = open(&cfg);
                reg.readers.insert(key, r);
            }
        }
        let outs: Vec<(String, TimecodeConfig)> = reg
            .nodes
            .iter()
            .filter(|b| b.cfg.mode == TimecodeMode::Generate)
            .filter_map(|b| b.cfg.writer_key().map(|k| (k, b.cfg.clone())))
            .collect();
        reg.writers.retain(|k, _| outs.iter().any(|(w, _)| w == k));
        for (key, cfg) in outs {
            if !reg.writers.contains_key(&key) {
                let w = open_writer(&cfg);
                reg.writers.insert(key, w);
            }
        }
    }
    let nodes: Vec<Value> = reg
        .nodes
        .iter()
        .map(|b| {
            let r = match b.cfg.mode {
                TimecodeMode::Read => reg.readers.get(&b.cfg.reader_key()),
                TimecodeMode::Generate => b.cfg.writer_key().and_then(|k| reg.writers.get(&k)),
            };
            let heard = r.and_then(|r| r.heard.lock().ok().and_then(|g| *g));
            let error = r.and_then(|r| r.error.lock().ok().and_then(|g| g.clone())).or_else(|| {
                (b.cfg.mode == TimecodeMode::Generate && b.cfg.writer_key().is_none())
                    .then(|| "Art-Net: ArtTimeCode is only read here".to_string())
            });
            reading(b, heard, error, now)
        })
        .collect();
    let at = nodes.iter().find_map(|n| n["at"].as_f64());
    json!({ "at": at, "locked": at.is_some(), "nodes": nodes, "rates": Fps::ALL.map(|f| f.label()) })
}

/// Starts the once-a-half-second pass over the graph files that opens and
/// drops the readers and generators, whether or not anything asks for
/// `GET /timecode` (a generator must keep running while the show does).
pub fn start_supervisor(root: std::path::PathBuf) {
    let _ = std::thread::Builder::new().name("metal-dash timecode supervisor".into()).spawn(move || loop {
        let _ = status(&root, Instant::now());
        std::thread::sleep(Duration::from_millis(500));
    });
}

/// `GET /timecode`: `{at, locked, nodes: [{graph, node, source, chase,
/// locked, at, timecode, fps, error}]}` — `at` is the sequence position in
/// seconds the first locked `clock.timecode` node asks for.
pub fn timecode_get(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(status(&crate::output::data_root(), Instant::now()))
}

/// `GET /artnet/nodes[?ms=1500&target=ip]`: ArtPoll, then every
/// ArtPollReply heard for `ms` (default 1500, at most 5000).
pub fn artnet_nodes(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let ms = params.param("ms").and_then(|m| m.parse::<u64>().ok()).unwrap_or(1500).clamp(100, 5000);
    let target = params.param("target").and_then(|t| {
        t.parse::<std::net::SocketAddr>()
            .ok()
            .or_else(|| format!("{t}:{}", metal_perform::artnet::ARTNET_PORT).parse().ok())
    });
    match metal_perform::artnet::discover(target, Duration::from_millis(ms)) {
        Ok(nodes) => Ok(json!({ "nodes": nodes })),
        Err(e) => Err(CallError::Bad(format!("Art-Net discovery: {e}"))),
    }
}

/// F13: `GET /sacn/nodes[?ms=3000]`: listen on the E1.31 §6.2 Universe
/// Discovery multicast (`239.255.70.1:5568`) for `ms` (default 3000, at most
/// 5000) and answer every source heard with the universes it announced.
/// The Settings → Discover section reads it (rule 6: the same route the AI
/// could), the shell never opening a socket itself (rule 9).
pub fn sacn_nodes(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let ms = params.param("ms").and_then(|m| m.parse::<u64>().ok()).unwrap_or(3000).clamp(100, 5000);
    match metal_perform::sacn::discover(Duration::from_millis(ms)) {
        Ok(sources) => Ok(json!({ "sources": sources })),
        Err(e) => Err(CallError::Bad(format!("sACN discovery: {e}"))),
    }
}

/// `GET /artnet/in?net&subnet&universe&first&count`: the one listener's
/// state (`error` when it is not listening) and the row an `in.artnet` with
/// that window reads now — the properties page's live readout and the AI's
/// `artnet_in_status`.
pub fn artnet_in(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    use metal_perform::artnet::{in_row, listen, listen_error, ArtNetInConfig};
    listen();
    let num = |k: &str, d: u64| params.param(k).and_then(|v| v.parse::<u64>().ok()).unwrap_or(d);
    let cfg = ArtNetInConfig {
        net: num("net", 0) as u8,
        subnet: num("subnet", 0) as u8,
        universe: num("universe", 0) as u8,
        first: num("first", 1) as u16,
        count: num("count", 16) as u16,
    };
    let (first, count) = cfg.window();
    Ok(json!({
        "listening": listen_error().is_none(),
        "error": listen_error(),
        "port_address": cfg.port_address(),
        "first": first,
        "count": count,
        "row": in_row(&cfg),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_perform::timecode::ChaseMode;

    fn bound(cfg: TimecodeConfig) -> Bound {
        Bound { graph: "g".into(), node: "t".into(), cfg }
    }

    #[test]
    fn a_reading_applies_chase_offset_and_rate_override() {
        let now = Instant::now();
        let tc = Timecode { hours: 1, minutes: 0, seconds: 10, frames: 0, fps: Fps::F30 };
        let cfg = TimecodeConfig { offset: 3600.0, fps: Some(Fps::F25), ..TimecodeConfig::default() };
        let v = reading(&bound(cfg), Some((tc, 0.0, now)), None, now);
        assert_eq!(v["locked"], json!(true));
        assert!((v["at"].as_f64().unwrap() - 10.0).abs() < 1e-9);
        assert_eq!(v["fps"], json!("25"));
        let silent = reading(&bound(TimecodeConfig::default()), Some((tc, 0.0, now)), None, now + Duration::from_secs(1));
        assert_eq!(silent["locked"], json!(false), "follow stops");
        let cfg = TimecodeConfig { chase: ChaseMode::Freewheel, freewheel_frames: 60, ..TimecodeConfig::default() };
        let v = reading(&bound(cfg), Some((tc, 0.0, now)), None, now + Duration::from_secs(1));
        assert_eq!(v["locked"], json!(true), "freewheel 60 frames bridges a second at 30");
    }

    /// The Art-Net reader hears an ArtTimeCode that the listener dispatches.
    #[test]
    fn the_artnet_reader_hears_art_timecode() {
        let r = spawn_artnet();
        let tc = Timecode { hours: 0, minutes: 1, seconds: 2, frames: 3, fps: Fps::F25 };
        let mut got = None;
        for _ in 0..40 {
            metal_perform::artnet::dispatch(&metal_perform::artnet::timecode_packet(&tc));
            std::thread::sleep(std::time::Duration::from_millis(25));
            got = r.heard.lock().unwrap().map(|h| h.0);
            if got.is_some() {
                break;
            }
        }
        assert_eq!(got, Some(tc), "error: {:?}", r.error.lock().unwrap());
    }

    #[test]
    fn scan_finds_timecode_nodes_and_skips_bypassed_ones() {
        let root = std::env::temp_dir().join(format!("mvj-tc-scan-{}", std::process::id()));
        let dir = metal_perform::engine::graphs_dir(&root);
        metal_vfs::create_dir_all(&dir).unwrap();
        let mut g = metal_graph::Graph::new("tc", "t", metal_graph::Domain::Live);
        let a = g.add_node("tc", TIMECODE_KIND, [0.0, 0.0]);
        let b = g.add_node("tc", TIMECODE_KIND, [0.0, 0.0]);
        g.node_mut(&b).unwrap().bypass = true;
        g.node_mut(&a).unwrap().config = metal_graph::Config::from_json_str(r#"{"source":"artnet"}"#).unwrap();
        metal_vfs::write(dir.join("tc.json"), g.to_json()).unwrap();
        let found = scan(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].node, a);
        assert_eq!(found[0].cfg.source, "artnet");
        metal_vfs::remove_dir_all(&root).ok();
    }
}
