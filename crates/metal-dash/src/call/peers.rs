//! `GET /peers`: the machines this one can compose on or render on, for the
//! properties panel's machine pickers (`Output.composed_on`, `Layer.render_on`).
//!
//! The list is `metal-sync`'s own `GET /peers` on its control port. This route
//! only relays it: the local machine is always listed (`state.local_node`,
//! online by definition), and when nothing listens on the sync port the
//! answer is `sync: false` with no peers — the panel then offers the local
//! machine alone rather than failing.
//!
//! The screens a peer shows come the same way, one hop further: `metal-sync`'s
//! `GET /peer/outputs?name=N` asks peer N over `fw/frame/1`, and that peer's
//! own `metal-dash` answers with its `outputs`. No peer address is ever dialled
//! here: a peer's `metal-dash` is only reachable through its `metal-sync`.
//! [`PeerScreens`] is the cache the device pool reads; [`refresh`] fills it
//! and is meant to run on a worker thread, never the UI thread.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::call::{CallError, Ctx, Params};

/// Connect and read budgets for one loopback request. A refused connect is
/// instant; these only bound a sync daemon that accepts and then stalls. A
/// peer's answer crosses the network, so its read gets the longer one.
const CONNECT: Duration = Duration::from_millis(200);
const READ: Duration = Duration::from_millis(800);
const PEER_READ: Duration = Duration::from_millis(1500);

/// How old the peers' screens may get before a `GET /peers` starts a new
/// background refresh. The route never waits on a peer.
const PEER_SCREENS_TTL: Duration = Duration::from_secs(5);

/// The process-wide screens cache behind `GET /peers`.
fn screens_cache() -> &'static Arc<Mutex<PeerScreens>> {
    static CACHE: OnceLock<Arc<Mutex<PeerScreens>>> = OnceLock::new();
    CACHE.get_or_init(|| Arc::new(Mutex::new(PeerScreens::default())))
}

/// Start a background [`refresh`] when the cache is stale and none is in
/// flight. Under test nothing runs: a test must never reach the real port.
fn ensure_refresh(cache: &Arc<Mutex<PeerScreens>>) {
    if cfg!(test) {
        return;
    }
    let start = {
        let mut c = cache.lock().expect("peer cache poisoned");
        let stale = c.refreshed.is_none_or(|t| t.elapsed() >= PEER_SCREENS_TTL);
        let start = stale && !c.busy;
        if start {
            c.busy = true;
        }
        start
    };
    if start {
        let cache = Arc::clone(cache);
        std::thread::spawn(move || refresh(&cache, sync_port()));
    }
}

/// `metal-sync`'s control port on loopback: `FW_SYNC_PORT` when it is set to a
/// port, else 7901 (the port `metal-sync` binds by default). A host metal-dash on a
/// non-default `metal-sync` reads it from the same variable. Lives here, not in
/// `author::playout`, because `author` is gpu-only and the monitor (glow-capable)
/// and this route both need it. `author::playout` re-exports it.
pub fn sync_port() -> u16 {
    sync_port_from(std::env::var("FW_SYNC_PORT").ok().as_deref())
}

/// [`sync_port`] for a given variable value: a number from 1 to 65535, else
/// the default.
pub fn sync_port_from(value: Option<&str>) -> u16 {
    value
        .and_then(|v| v.trim().parse::<u16>().ok())
        .filter(|p| *p != 0)
        .unwrap_or(7901)
}

pub fn peers(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let local = ctx.state.local_node.clone();
    let cache = screens_cache();
    ensure_refresh(cache);
    let known = cache.lock().expect("peer cache poisoned").by_peer.clone();
    let mut v = relay(&local, fetch_peers(sync_port()));
    v["screens"] = screens_json(&known);
    Ok(v)
}

/// The cache as the reply's `screens` field: `{name: {online, screens}}`.
fn screens_json(known: &BTreeMap<String, PeerEntry>) -> Value {
    known
        .iter()
        .map(|(n, e)| (n.clone(), json!({ "online": e.online, "screens": e.screens })))
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// The reply body, pure over what `metal-sync` answered: `None` is "nothing
/// listening (or an unreadable answer)", which reads as no sync at all.
fn relay(local: &str, peers: Option<Vec<Value>>) -> Value {
    json!({
        "local": local,
        "sync": peers.is_some(),
        "peers": peers.unwrap_or_default(),
    })
}

/// One blocking `GET path` on `127.0.0.1:port`, the raw answer. `None` when
/// nothing listens or the answer stalls past the budget.
fn get_raw(port: u16, path: &str, read: Duration) -> Option<Vec<u8>> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT).ok()?;
    stream.set_read_timeout(Some(read)).ok()?;
    stream.set_write_timeout(Some(READ)).ok()?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).ok()?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok()?;
    Some(raw)
}

/// The `peers` array `metal-sync` answered on `GET /peers`, or `None`.
pub fn fetch_peers(port: u16) -> Option<Vec<Value>> {
    parse_array_answer(&get_raw(port, "/peers", READ)?, "peers")
}

/// The screens peer `name` shows, as `metal-sync` relays them from that peer's
/// `metal-dash`, or `None` when the peer did not answer.
pub fn fetch_peer_outputs(port: u16, name: &str) -> Option<Vec<Value>> {
    let path = format!("/peer/outputs?name={}", percent_encode(name));
    parse_array_answer(&get_raw(port, &path, PEER_READ)?, "outputs")
}

/// Pure half of the fetches: a raw HTTP answer into the `key` array of its
/// JSON body. Only a 200 counts.
fn parse_array_answer(raw: &[u8], key: &str) -> Option<Vec<Value>> {
    let text = std::str::from_utf8(raw).ok()?;
    let (head, body) = text.split_once("\r\n\r\n")?;
    let status = head.lines().next()?.split_whitespace().nth(1)?;
    if status != "200" {
        return None;
    }
    let v: Value = serde_json::from_str(body).ok()?;
    v[key].as_array().cloned()
}

#[cfg(test)]
/// Pure half of [`fetch_peers`]: a raw answer into the `peers` array.
pub fn parse_peers_response(raw: &[u8]) -> Option<Vec<Value>> {
    parse_array_answer(raw, "peers")
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// One peer as the device pool last saw it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PeerEntry {
    /// Listed online by `metal-sync` and answered its outputs on the last refresh.
    pub online: bool,
    /// Its screens: the last answer while it was reachable, kept while it is not.
    pub screens: Vec<Value>,
}

/// What the device pool knows of each peer's screens. Kept across refreshes so
/// an offline peer is still listed, greyed, with its last-known screens.
#[derive(Default, Debug)]
pub struct PeerScreens {
    /// When the last refresh finished.
    pub refreshed: Option<Instant>,
    /// A refresh is in flight; a second one is not started.
    pub busy: bool,
    pub by_peer: BTreeMap<String, PeerEntry>,
}

/// One refresh: `GET /peers`, then each online peer's outputs. Network calls
/// run with no lock held. With no sync daemon every known peer goes offline
/// and keeps its last-known screens; a peer that stops answering its outputs
/// does the same.
pub fn refresh(cache: &Mutex<PeerScreens>, port: u16) {
    let previous = cache.lock().expect("peer cache poisoned").by_peer.clone();
    let mut next = BTreeMap::new();
    match fetch_peers(port) {
        Some(peers) => {
            for p in &peers {
                let Some(name) = p["name"].as_str().filter(|n| !n.is_empty()) else {
                    continue;
                };
                let last = previous.get(name).map(|e| e.screens.clone()).unwrap_or_default();
                let fresh = p["online"]
                    .as_bool()
                    .unwrap_or(false)
                    .then(|| fetch_peer_outputs(port, name))
                    .flatten();
                let entry = match fresh {
                    Some(screens) => PeerEntry { online: true, screens },
                    None => PeerEntry { online: false, screens: last },
                };
                next.insert(name.to_string(), entry);
            }
        }
        None => {
            for (name, e) in previous {
                next.insert(name, PeerEntry { online: false, screens: e.screens });
            }
        }
    }
    let mut c = cache.lock().expect("peer cache poisoned");
    c.by_peer = next;
    c.refreshed = Some(Instant::now());
    c.busy = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::Arc;

    fn answer_once(listener: TcpListener, status: &'static str, body: &'static str) {
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(reply.as_bytes());
            }
        });
    }

    #[test]
    fn a_fake_sync_daemon_answers_its_peers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        answer_once(
            listener,
            "200 OK",
            r#"{"peers":[{"name":"pi","node_id":"abc","online":true,"last_seen":1,"via":null},{"name":"laptop","node_id":"def","online":false,"last_seen":null,"via":null}]}"#,
        );
        let peers = fetch_peers(port).expect("the fake daemon answers");
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[1]["name"], "laptop");
        assert_eq!(peers[1]["online"], false);
    }

    #[test]
    fn nothing_listening_is_no_sync_and_the_local_machine_still_answers() {
        // Bind then drop: the port is free and refuses instantly.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(fetch_peers(port).is_none());
        let v = relay("casa", None);
        assert_eq!(v["local"], "casa");
        assert_eq!(v["sync"], false);
        assert_eq!(v["peers"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn a_non_200_answer_is_not_peers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        answer_once(listener, "503 Service Unavailable", r#"{"error":"the sync engine is not up"}"#);
        assert!(fetch_peers(port).is_none());
    }

    #[test]
    fn parse_reads_the_peers_array_and_rejects_garbage() {
        assert!(parse_peers_response(b"HTTP/1.1 200 OK\r\n\r\n{\"peers\":[]}").is_some());
        assert!(parse_peers_response(b"HTTP/1.1 200 OK\r\n\r\nnot json").is_none());
        assert!(parse_peers_response(b"garbage").is_none());
    }

    /// Stand-in for `metal-sync`'s control port on an ephemeral port. Its peer
    /// list and each peer's outputs are scripted and can change between
    /// refreshes; it answers `GET /peers` and `GET /peer/outputs?name=N`.
    fn fake_sync(script: Arc<Mutex<Value>>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut s = stream;
                let mut buf = [0u8; 2048];
                let n = s.read(&mut buf).unwrap_or(0);
                let line = String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or_default().to_string();
                let path = line.split_whitespace().nth(1).unwrap_or_default().to_string();
                let sc = script.lock().unwrap().clone();
                let (status, body) = if path == "/peers" {
                    ("200 OK", json!({ "peers": sc["peers"] }))
                } else if let Some(name) = path.strip_prefix("/peer/outputs?name=") {
                    match sc["outputs"].get(name) {
                        Some(o) => ("200 OK", json!({ "outputs": o })),
                        None => ("502 Bad Gateway", json!({ "error": "did not answer" })),
                    }
                } else {
                    ("404 Not Found", json!({ "error": "no such route" }))
                };
                let body = body.to_string();
                let _ = write!(
                    s,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        port
    }

    #[test]
    fn peer_screens_are_fetched_grouped_and_kept_when_a_peer_goes_offline() {
        let script = Arc::new(Mutex::new(json!({
            "peers": [
                { "name": "pi", "node_id": "aaa", "online": true, "last_seen": 1, "via": null },
                { "name": "laptop", "node_id": "bbb", "online": true, "last_seen": 1, "via": null },
            ],
            "outputs": {
                "pi": [{ "id": "wall", "label": "pared", "node": "pi" }],
                "laptop": [{ "id": "stage", "label": "escenario", "node": "laptop" }],
            },
        })));
        let port = fake_sync(script.clone());
        let cache = Mutex::new(PeerScreens::default());

        refresh(&cache, port);
        {
            let c = cache.lock().unwrap();
            assert!(!c.busy && c.refreshed.is_some());
            assert_eq!(c.by_peer["pi"], PeerEntry { online: true, screens: vec![json!({ "id": "wall", "label": "pared", "node": "pi" })] });
            assert!(c.by_peer["laptop"].online);
        }

        // The laptop leaves the club's reach: greyed, its last-known screens kept.
        script.lock().unwrap()["peers"][1]["online"] = json!(false);
        refresh(&cache, port);
        let c = cache.lock().unwrap();
        assert!(!c.by_peer["laptop"].online, "offline peers are listed, greyed");
        assert_eq!(c.by_peer["laptop"].screens[0]["id"], "stage", "last-known screens stay");
        assert!(c.by_peer["pi"].online);
    }

    #[test]
    fn with_no_sync_daemon_known_peers_go_offline_and_keep_their_screens() {
        let cache = Mutex::new(PeerScreens::default());
        cache.lock().unwrap().by_peer.insert(
            "pi".into(),
            PeerEntry { online: true, screens: vec![json!({ "id": "wall" })] },
        );
        let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        refresh(&cache, dead);
        let c = cache.lock().unwrap();
        assert_eq!(c.by_peer["pi"], PeerEntry { online: false, screens: vec![json!({ "id": "wall" })] });
    }
}
