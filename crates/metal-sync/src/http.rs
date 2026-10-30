//! The local control surface on :7901.
//!
//! This is not the sync transport — peers talk QUIC over iroh, never this.
//! This is how an operator (and `metal-dash`'s own UI, later) asks a running
//! `metal-sync` what it thinks is going on: which node am I, who am I paired
//! with, what landed last.
//!
//! It is hand-rolled for the same reason `metal-web` is: the whole surface is a
//! handful of routes serving JSON to localhost, and a framework would be a
//! dependency with a runtime attached.
//!
//! The pairing and frame routes need the network, which is async. These
//! threads are not runtime threads, so they `block_on` the runtime's
//! [`Handle`] for exactly the one request — the same blocking shape as every
//! other route, with the QUIC work done where iroh lives.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::runtime::Handle;

use crate::frame::{Frame, FrameAsk};
use crate::pair::Net;
use crate::state::SyncState;

/// Requests are small — a status poll or a one-line sync trigger. The cap
/// exists so a malformed or hostile client cannot make us allocate without
/// bound; it is not a guess at what a real request looks like.
const MAX_BODY: usize = 64 * 1024;

/// What the control surface can reach. Without `net` (no engine yet, or a
/// test) the network routes answer 503.
#[derive(Clone)]
pub struct Control {
    pub state: Arc<Mutex<SyncState>>,
    pub net: Option<(Net, Handle)>,
    pub dash_port: u16,
}

pub fn handle_connection(stream: TcpStream, state: Arc<Mutex<SyncState>>) -> std::io::Result<()> {
    handle(stream, &Control { state, net: None, dash_port: 7900 })
}

pub fn handle(stream: TcpStream, ctl: &Control) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);

    // Read the request line and headers properly, to the blank line that ends
    // them. The previous version read a single fixed 1024-byte buffer, which
    // silently truncated anything longer and mis-parsed anything that arrived
    // in more than one packet.
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    if content_length > MAX_BODY {
        return write_json(stream, 413, json!({ "error": "body too large" }));
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if let Some(resp) = network_route(ctl, method, path, query, &body) {
        return match resp {
            Reply::Json(status, v) => write_json(stream, status, v),
            Reply::Frame(f) => write_frame(stream, f),
        };
    }
    route(stream, method, path, query, &ctl.state)
}

enum Reply {
    Json(u16, Value),
    Frame(Frame),
}

/// A field from the JSON body, or the query string as a fallback.
fn arg(body: &Value, query: &str, key: &str) -> Option<String> {
    body[key].as_str().map(str::to_string).or_else(|| param(query, key))
}

/// The pairing, club and frame routes. `None` hands the request on to
/// [`route`].
fn network_route(ctl: &Control, method: &str, path: &str, query: &str, body: &Value) -> Option<Reply> {
    let state = &ctl.state;
    let r = match (method, path) {
        ("GET", "/status") => {
            let mut v = state.lock().expect("sync state mutex poisoned").status();
            v["paused"] = json!(state.lock().expect("poisoned").paused());
            v["dash_reachable"] = json!(dash_reachable(ctl.dash_port));
            if let Some((net, _)) = &ctl.net {
                let book = net.club.book.lock().expect("pair book poisoned");
                v["name"] = json!(book.name());
                v["paired"] = json!(!book.peers().is_empty());
                drop(book);
                let doc = net.club.doc.read().expect("doc lock poisoned");
                v["namespace"] = json!(doc.as_ref().map(|d| d.id().to_string()));
            }
            Reply::Json(200, v)
        }
        ("GET", "/log") => {
            let s = state.lock().expect("poisoned");
            Reply::Json(200, json!({ "log": s.log_entries(), "lww_dropped": s.lww_report() }))
        }
        ("POST", "/pause") | ("POST", "/resume") => {
            state.lock().expect("poisoned").set_paused(path == "/pause");
            Reply::Json(200, json!({ "paused": path == "/pause" }))
        }
        ("GET", "/discover") | ("GET", "/peers") | ("GET", "/pair/pending")
        | ("GET", "/pair/outgoing") | ("GET", "/frame") | ("GET", "/peer/outputs") | ("GET", "/peer/stats")
        | ("GET", "/song/pull") | ("POST", "/song/pull")
        | ("POST", "/pair/request")
        | ("POST", "/pair/confirm") | ("POST", "/pair/reject") | ("POST", "/leave") => {
            let Some((net, rt)) = &ctl.net else {
                return Some(Reply::Json(503, json!({ "error": "the sync engine is not up" })));
            };
            networked(net, rt, state, method, path, query, body)
        }
        _ => return None,
    };
    Some(r)
}

fn networked(
    net: &Net,
    rt: &Handle,
    state: &Arc<Mutex<SyncState>>,
    method: &str,
    path: &str,
    query: &str,
    body: &Value,
) -> Reply {
    let club = &net.club;
    let now = std::time::Instant::now();
    let (status, v) = match (method, path) {
        ("GET", "/discover") => {
            let book = club.book.lock().expect("pair book poisoned");
            let seen = club.discovered.lock().expect("discovered poisoned");
            let list: Vec<Value> = seen
                .iter()
                .map(|(id, s)| {
                    json!({
                        "node_id": id,
                        "name": s.name,
                        "addrs": s.addrs,
                        "last_seen": s.last_seen,
                        "member": book.is_member(id),
                    })
                })
                .collect();
            (200, json!({ "nearby": list }))
        }
        ("GET", "/peers") => {
            let peers: Vec<(String, String)> = club
                .book
                .lock()
                .expect("pair book poisoned")
                .peers()
                .iter()
                .map(|(name, id)| (name.clone(), id.clone()))
                .collect();
            let s = state.lock().expect("poisoned");
            let list: Vec<Value> = peers
                .iter()
                .map(|(name, id)| {
                    let (online, last_seen) = s.liveness(id).unwrap_or((false, 0));
                    // `direct` or `relay`, from the endpoint's active path to
                    // the peer; null when no path is in use right now.
                    let via = id
                        .parse::<iroh::EndpointId>()
                        .ok()
                        .and_then(|id| rt.block_on(net.via(id)));
                    json!({
                        "name": name,
                        "node_id": id,
                        "online": online,
                        "last_seen": if last_seen == 0 { Value::Null } else { json!(last_seen) },
                        "via": if online { via.map_or(Value::Null, |v| json!(v)) } else { Value::Null },
                    })
                })
                .collect();
            (200, json!({ "peers": list }))
        }
        ("GET", "/pair/pending") => {
            (200, json!({ "pending": club.book.lock().expect("pair book poisoned").pending(now) }))
        }
        ("GET", "/pair/outgoing") => {
            let mut book = club.book.lock().expect("pair book poisoned");
            let out = book.outgoing(now).map(|o| json!({ "node_id": o.addr.id.to_string() }));
            (200, json!({ "outgoing": out }))
        }
        ("POST", "/pair/reject") => match arg(body, query, "node_id") {
            Some(id) => {
                let had = club.book.lock().expect("pair book poisoned").reject(&id);
                (if had { 200 } else { 404 }, json!({ "rejected": had }))
            }
            None => (400, json!({ "error": "missing node_id" })),
        },
        ("POST", "/pair/request") => match arg(body, query, "node_id").map(|s| s.parse::<iroh::EndpointId>()) {
            Some(Ok(id)) => {
                let addr = crate::pair::addr_for(club, id);
                rt.block_on(net.request(addr))
            }
            Some(Err(_)) => (400, json!({ "error": "not a node id" })),
            None => (400, json!({ "error": "missing node_id" })),
        },
        ("POST", "/pair/confirm") => match arg(body, query, "code") {
            Some(code) => rt.block_on(net.confirm(&code)),
            None => (400, json!({ "error": "missing code" })),
        },
        ("POST", "/leave") => rt.block_on(net.leave()),
        ("GET", "/song/pull") => match arg(body, query, "song") {
            Some(song) => (200, net.pull_status(&song)),
            None => (400, json!({ "error": "missing ?song=" })),
        },
        ("POST", "/song/pull") => match arg(body, query, "song") {
            Some(song) => {
                let (net, task) = (net.clone(), song.clone());
                rt.spawn(async move { net.pull_song(&task).await });
                (202, json!({ "pulling": song }))
            }
            None => (400, json!({ "error": "missing song" })),
        },
        ("GET", "/frame") => {
            let ask = if let Some(d) = param(query, "device") {
                FrameAsk::Device(d)
            } else if let (Some(layer), Some(frame)) =
                (param(query, "layer"), param(query, "frame").and_then(|f| f.parse().ok()))
            {
                let size = |k: &str, default: u32| {
                    param(query, k).and_then(|v| v.parse().ok()).unwrap_or(default)
                };
                FrameAsk::Layer {
                    layer,
                    frame,
                    on: param(query, "on"),
                    w: size("w", crate::frame::DEFAULT_LAYER_W),
                    h: size("h", crate::frame::DEFAULT_LAYER_H),
                    state: crate::frame::LayerState::from_query(|k| param(query, k)),
                    fmt: param(query, "fmt"),
                    q: param(query, "q").and_then(|q| q.parse().ok()),
                }
            } else {
                return Reply::Json(400, json!({ "error": "missing ?device= (or ?layer=&frame=)" }));
            };
            return Reply::Frame(rt.block_on(net.frame(ask)));
        }
        ("GET", "/peer/stats") => {
            let book = club.stats.lock().expect("stats poisoned");
            (200, json!({ "peers": book.view(now) }))
        }
        ("GET", "/peer/outputs") => {
            let Some(name) = param(query, "name") else {
                return Reply::Json(400, json!({ "error": "missing ?name=" }));
            };
            let f = rt.block_on(net.peer_outputs(&name));
            let body = serde_json::from_slice(&f.body).unwrap_or_else(|_| json!({ "error": "not JSON" }));
            return Reply::Json(f.status, body);
        }
        _ => (404, json!({ "error": "no such route" })),
    };
    Reply::Json(status, v)
}

/// Is `metal-dash` answering on loopback? A connect, nothing more.
fn dash_reachable(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(200)).is_ok()
}

fn route(
    stream: TcpStream,
    method: &str,
    path: &str,
    query: &str,
    state: &Arc<Mutex<SyncState>>,
) -> std::io::Result<()> {
    match (method, path) {
        // Pairing asks the *running* node, so the ticket names addresses that
        // are live right now.
        ("GET", "/ticket") => {
            let s = state.lock().expect("sync state mutex poisoned");
            match s.ticket() {
                Some(t) => write_json(stream, 200, json!({ "ticket": t })),
                None => write_json(stream, 503, json!({ "error": "not paired yet" })),
            }
        }
        ("GET", "/files") => {
            let s = state.lock().expect("sync state mutex poisoned");
            let allow = s.allowlist();
            let files: Vec<String> =
                s.reader().syncable_files(&allow).iter().map(|p| p.display().to_string()).collect();
            write_json(stream, 200, json!({ "files": files }))
        }
        ("POST", "/sync") => {
            let Some(file) = param(query, "file") else {
                return write_json(stream, 400, json!({ "error": "missing ?file=" }));
            };
            let mut s = state.lock().expect("sync state mutex poisoned");
            match s.publish(Path::new(&file)) {
                Ok(v) => write_json(stream, 200, v),
                Err(e) => write_json(stream, 400, json!({ "error": e })),
            }
        }
        _ => write_json(stream, 404, json!({ "error": "no such route" })),
    }
}

/// One query parameter, percent-decoded.
///
/// Decoding matters here because the value is a path: without it, `%2e%2e%2f`
/// would sail past a textual `..` check and only become a traversal after the
/// filesystem saw it. The path is validated again downstream — this just means
/// the two layers are looking at the same string.
fn param(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| percent_decode(v))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The reason phrase used to be hardcoded to `OK`, so an error came back as
/// `HTTP/1.1 404 OK` — a malformed status line that a strict client is
/// entitled to reject outright.
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        409 => "Conflict",
        502 => "Bad Gateway",
        404 => "Not Found",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

fn write_json(mut stream: TcpStream, status: u16, body: Value) -> std::io::Result<()> {
    let text = body.to_string();
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        reason(status),
        text.len(),
        text
    )?;
    stream.flush()
}

/// A frame passes through with `metal-dash`'s status: the picture on 200, its
/// JSON error otherwise.
fn write_frame(mut stream: TcpStream, f: Frame) -> std::io::Result<()> {
    // A picture's codec is the one the ask named (`fmt`); its magic bytes
    // would be a guess (raw RGBA has no header), so the label stays generic.
    let ctype = if f.status == 200 { "application/octet-stream" } else { "application/json" };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        f.status,
        reason(f.status),
        ctype,
        f.body.len()
    )?;
    stream.write_all(&f.body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(ctl: &Control, method: &str, path: &str) -> (u16, Value) {
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        match network_route(ctl, method, path, query, &Value::Null) {
            Some(Reply::Json(s, v)) => (s, v),
            Some(Reply::Frame(f)) => (f.status, Value::Null),
            None => (0, Value::Null),
        }
    }

    fn offline() -> (tempfile::TempDir, Control) {
        let dir = tempfile::TempDir::new().unwrap();
        let state = Arc::new(Mutex::new(SyncState::new("n".into(), dir.path().to_path_buf())));
        (dir, Control { state, net: None, dash_port: 1 })
    }

    #[test]
    fn status_reports_paused_and_an_unreachable_dash() {
        let (_d, ctl) = offline();
        let (s, v) = ask(&ctl, "GET", "/status");
        assert_eq!(s, 200);
        assert_eq!(v["paused"], false);
        assert_eq!(v["dash_reachable"], false);
    }

    #[test]
    fn pause_and_resume_flip_the_flag_and_log_it() {
        let (_d, ctl) = offline();
        assert_eq!(ask(&ctl, "POST", "/pause").1["paused"], true);
        assert!(ctl.state.lock().unwrap().paused());
        assert_eq!(ask(&ctl, "POST", "/resume").1["paused"], false);
        let log = ask(&ctl, "GET", "/log").1;
        assert_eq!(log["log"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn network_routes_without_an_engine_are_503_not_404() {
        let (_d, ctl) = offline();
        for (m, p) in [
            ("GET", "/discover"),
            ("GET", "/peers"),
            ("GET", "/pair/pending"),
            ("GET", "/pair/outgoing"),
            ("POST", "/pair/request"),
            ("POST", "/pair/confirm"),
            ("POST", "/pair/reject"),
            ("POST", "/leave"),
            ("GET", "/song/pull?song=x"),
            ("POST", "/song/pull"),
            ("GET", "/frame?device=x"),
            ("GET", "/peer/outputs?name=pi"),
        ] {
            assert_eq!(ask(&ctl, m, p).0, 503, "{m} {p}");
        }
        // Everything else falls through to the older routes.
        assert_eq!(ask(&ctl, "GET", "/files").0, 0);
    }

    #[test]
    fn status_lines_carry_their_real_reason_phrase() {
        assert_eq!(reason(200), "OK");
        assert_eq!(reason(404), "Not Found");
        assert_eq!(reason(400), "Bad Request");
    }

    #[test]
    fn query_parameters_are_percent_decoded() {
        assert_eq!(param("file=trims.json", "file").unwrap(), "trims.json");
        assert_eq!(param("a=1&file=graphs%2Fx.json", "file").unwrap(), "graphs/x.json");
        assert!(param("other=1", "file").is_none());
    }

    /// An encoded traversal has to become a visible `..` before anything
    /// downstream tries to decide whether it is safe.
    #[test]
    fn an_encoded_traversal_decodes_before_it_is_checked() {
        assert_eq!(param("file=%2e%2e%2fpasswd", "file").unwrap(), "../passwd");
    }
}
