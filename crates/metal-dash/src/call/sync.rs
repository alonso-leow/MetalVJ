//! The sync pairing and status, read and driven through the door
//! (`GET /sync/*`, `POST /sync/pair/*`).
//!
//! `metal-sync` is a separate process with its own loopback control port
//! (`:7901`, `metal-sync/src/http.rs`). The shell never dials it from a panel
//! (regla 9): a panel calls the door, and these handlers forward the one
//! request to `metal-sync` on loopback and hand back its JSON. The forward is
//! plain HTTP over a `TcpStream`, the same shape `monitor.rs` already uses
//! for `GET /frame`.
//!
//! `metal-sync` not running is an answer, not an error: every read comes back
//! with `running: false` and the empty shape of what it would have listed,
//! so the chrome indicator and the Dispositivos page can say "sin sync"
//! without a second code path.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::{json, Value};

use super::{CallError, Ctx, Params};

/// `metal-sync`'s control port — the one `monitor.rs` asks for frames on.
pub const SYNC_PORT: u16 = 7901;

/// How long a read may wait on `metal-sync`. The shell calls the door from its
/// paint thread, so a hung sync process must not hold a frame for long.
const READ_WAIT: Duration = Duration::from_secs(2);

/// Pairing actions can wait on the network (a QUIC dial to the other
/// machine), so they get longer. Still bounded: the panel must come back.
const PAIR_WAIT: Duration = Duration::from_secs(20);

/// Connecting to loopback is instant when something listens; a refused
/// connect is the "not running" case and must not be slow.
const CONNECT_WAIT: Duration = Duration::from_millis(300);

/// A pairing code is exactly six decimal digits, leading zeros kept
/// (`metal-sync`'s `pair::new_code`). Checked here so a typo never reaches the
/// wire.
pub fn valid_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())
}

/// `GET /sync/status`.
pub fn status_get(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(read_at(SYNC_PORT, "/status", json!({})))
}

/// `GET /sync/peers`: the club as this machine knows it, with liveness.
pub fn peers_get(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(read_at(SYNC_PORT, "/peers", json!({ "peers": [] })))
}

/// `GET /sync/stats`: each peer's last self-report, as `metal-sync` keeps it
/// (`metal-sync`'s `stats.rs`): status, age, and the readings, `null` where
/// unknown. Read-only.
pub fn stats_get(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(read_at(SYNC_PORT, "/peer/stats", json!({ "peers": [] })))
}

/// `GET /sync/log`: what `metal-sync` has done lately, newest last.
pub fn log_get(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(read_at(SYNC_PORT, "/log", json!({ "log": [] })))
}

/// `GET /sync/discover`: machines seen on the LAN, with `member` set for
/// the ones already in the club.
pub fn discover_get(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(read_at(SYNC_PORT, "/discover", json!({ "nearby": [] })))
}

/// `GET /sync/pair`: this machine's side of both halves of a pairing —
/// the requests it is showing codes for (`pending`) and the one it sent
/// (`outgoing`).
pub fn pair_get(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let pending = read_at(SYNC_PORT, "/pair/pending", json!({ "pending": [] }));
    let outgoing = read_at(SYNC_PORT, "/pair/outgoing", json!({ "outgoing": null }));
    Ok(json!({
        "running": pending["running"],
        "pending": pending["pending"],
        "outgoing": outgoing["outgoing"],
    }))
}

/// `POST /sync/pair/request` `{ node_id }`: dial that machine and ask to
/// pair. The reply is `awaiting_code`; the code is typed on this side later.
pub fn pair_request_post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().unwrap_or(Value::Null);
    let node_id = body["node_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CallError::Bad("expected `node_id`".into()))?;
    pair_post_at(SYNC_PORT, "/pair/request", json!({ "node_id": node_id }))
}

/// `POST /sync/pair/confirm` `{ code }`: the six digits shown on the other
/// machine. A match makes the club share its document with this one.
pub fn pair_confirm_post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().unwrap_or(Value::Null);
    let code = body["code"].as_str().unwrap_or_default();
    if !valid_code(code) {
        return Err(CallError::Bad("the code is six digits".into()));
    }
    pair_post_at(SYNC_PORT, "/pair/confirm", json!({ "code": code }))
}

/// `POST /sync/pair/reject` `{ node_id }`: refuse a request shown here.
pub fn pair_reject_post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().unwrap_or(Value::Null);
    let node_id = body["node_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CallError::Bad("expected `node_id`".into()))?;
    pair_post_at(SYNC_PORT, "/pair/reject", json!({ "node_id": node_id }))
}

/// `GET /farm`: the render farm's jobs as the conductor's `metal-sync` keeps
/// them (`metal-sync`'s `farm_net.rs`).
pub fn farm_get(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(read_at(SYNC_PORT, "/farm", json!({ "conductor": false, "jobs": [] })))
}

/// `POST /farm/submit`: an export request (a `/export/video` body) for the
/// farm to split into shards. Also what `POST /export/queue {farm:true}` does.
pub fn farm_submit_post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("expected json".into()))?;
    farm_submit_at(SYNC_PORT, body)
}

/// [`farm_submit_post`] with the body in hand.
pub(crate) fn farm_submit_post_body(body: Value) -> Result<Value, CallError> {
    farm_submit_at(SYNC_PORT, body)
}

/// The submission against an explicit control port (a test points it at a stand-in).
pub(crate) fn farm_submit_at(port: u16, body: Value) -> Result<Value, CallError> {
    pair_post_at(port, "/farm/submit", body)
}

/// A read: forward `GET path`, and answer `fallback` (the empty shape of
/// what the read lists) with `running: false` when nothing listens. A
/// reply that does arrive is `metal-sync`'s own JSON with `running: true`.
fn read_at(port: u16, path: &str, fallback: Value) -> Value {
    match forward(port, "GET", path, None, READ_WAIT) {
        Some((_, mut body)) => {
            if !body.is_object() {
                body = json!({});
            }
            body["running"] = json!(true);
            body
        }
        None => {
            let mut body = fallback;
            body["running"] = json!(false);
            body
        }
    }
}

/// A pairing action: forward `POST path` and answer `metal-sync`'s JSON on
/// success. A `4xx`/`5xx` becomes the matching [`CallError`] carrying
/// `metal-sync`'s own `error` text, so the panel can show the reason (wrong code,
/// name taken, no request in flight). Nothing listening is a `400` with
/// the reason, never a silent success.
fn pair_post_at(port: u16, path: &str, body: Value) -> Result<Value, CallError> {
    let Some((status, reply)) = forward(port, "POST", path, Some(&body), PAIR_WAIT) else {
        return Err(CallError::Bad("metal-sync is not running on this machine".into()));
    };
    if (200..300).contains(&status) {
        return Ok(reply);
    }
    let message = reply["error"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("metal-sync answered {status}"));
    Err(match status {
        404 => CallError::NotFound,
        409 => CallError::Conflict(message),
        400..=499 => CallError::Bad(message),
        _ => CallError::Internal(message),
    })
}

/// One request to `metal-sync`'s control port. `None` when nothing answers
/// there (refused connect, a timeout, a torn reply) — the caller reads that
/// as "not running". Returns the status code and the parsed JSON body; a
/// body that is not JSON is `Null`, never a panic.
fn forward(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&Value>,
    wait: Duration,
) -> Option<(u16, Value)> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_WAIT).ok()?;
    stream.set_read_timeout(Some(wait)).ok()?;
    stream.set_write_timeout(Some(wait)).ok()?;
    let payload = body.map(Value::to_string).unwrap_or_default();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(head.as_bytes()).ok()?;
    // `metal-sync` answers one request per connection and closes it, so the
    // reply is everything up to EOF.
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok()?;
    parse_reply(&raw)
}

/// Splits a raw HTTP/1.1 reply into its status and JSON body. Pure, so the
/// wire shape is tested without a socket.
fn parse_reply(raw: &[u8]) -> Option<(u16, Value)> {
    let text = std::str::from_utf8(raw).ok()?;
    let (head, body) = text.split_once("\r\n\r\n")?;
    let status = head.lines().next()?.split_whitespace().nth(1)?.parse::<u16>().ok()?;
    let body = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    Some((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    /// A stand-in `metal-sync`: answers every connection with `reply` (a full
    /// HTTP/1.1 response) and records the request line it saw. Returns its
    /// port; the thread ends after `connections` requests.
    fn fake_sync(connections: usize, reply: &'static str) -> (u16, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..connections {
                let (mut stream, _) = listener.accept().expect("accept");
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                seen.push(text.lines().next().unwrap_or_default().to_string());
                stream.write_all(reply.as_bytes()).ok();
            }
            seen
        });
        (port, handle)
    }

    fn free_port() -> u16 {
        TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
    }

    #[test]
    fn six_digits_and_nothing_else_is_a_code() {
        assert!(valid_code("048213"));
        assert!(valid_code("000000"));
        assert!(!valid_code("04821"));
        assert!(!valid_code("0482133"));
        assert!(!valid_code("04821a"));
        assert!(!valid_code(" 04821"));
        assert!(!valid_code(""));
    }

    #[test]
    fn a_reply_is_split_into_status_and_json() {
        let raw = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 27\r\n\r\n{\"error\":\"wrong code\",\"x\":1}";
        let (status, body) = parse_reply(raw).expect("parses");
        assert_eq!(status, 403);
        assert_eq!(body["error"], "wrong code");
        assert_eq!(parse_reply(b"not http"), None);
    }

    /// Nothing listening: every read says `running: false` and keeps the
    /// empty shape of its list, never an error.
    #[test]
    fn reads_without_a_sync_process_say_not_running() {
        let port = free_port();
        let peers = read_at(port, "/peers", json!({ "peers": [] }));
        assert_eq!(peers["running"], json!(false));
        assert_eq!(peers["peers"], json!([]));
        let pending = read_at(port, "/pair/pending", json!({ "pending": [] }));
        assert_eq!(pending["running"], json!(false));
    }

    /// A live `metal-sync`: its JSON comes back with `running: true`, and the
    /// request line it saw is the route the door named.
    #[test]
    fn a_read_forwards_the_route_and_marks_the_reply_running() {
        let body = "{\"name\":\"sala\",\"paired\":true}";
        let reply: &'static str = Box::leak(
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len())
                .into_boxed_str(),
        );
        let (port, server) = fake_sync(1, reply);
        let out = read_at(port, "/status", json!({}));
        assert_eq!(out["running"], json!(true));
        assert_eq!(out["name"], json!("sala"));
        assert_eq!(out["paired"], json!(true));
        let seen = server.join().unwrap();
        assert_eq!(seen, vec!["GET /status HTTP/1.1".to_string()]);
    }

    /// A pairing refusal keeps the status class: a wrong code is a 403 with
    /// metal-sync's own words, a name clash a 409 conflict.
    #[test]
    fn a_refused_pairing_carries_fw_syncs_reason() {
        let body = "{\"error\":\"wrong code\",\"attempts_left\":2}";
        let reply: &'static str = Box::leak(
            format!("HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\n\r\n{body}", body.len())
                .into_boxed_str(),
        );
        let (port, server) = fake_sync(1, reply);
        match pair_post_at(port, "/pair/confirm", json!({ "code": "000000" })) {
            Err(CallError::Bad(msg)) => assert_eq!(msg, "wrong code"),
            other => panic!("expected Bad(\"wrong code\"), got {other:?}"),
        }
        let seen = server.join().unwrap();
        assert_eq!(seen, vec!["POST /pair/confirm HTTP/1.1".to_string()]);
    }

    #[test]
    fn a_pairing_action_with_no_sync_process_is_an_error_not_a_success() {
        let port = free_port();
        assert!(matches!(
            pair_post_at(port, "/pair/request", json!({ "node_id": "abc" })),
            Err(CallError::Bad(_))
        ));
    }

    /// The four routes this module adds are rows of the one table.
    #[test]
    fn the_sync_routes_are_rows_of_the_table() {
        for (method, path) in [
            ("GET", "/sync/status"),
            ("GET", "/sync/peers"),
            ("GET", "/sync/stats"),
            ("GET", "/sync/log"),
            ("GET", "/sync/discover"),
            ("GET", "/sync/pair"),
            ("POST", "/sync/pair/request"),
            ("POST", "/sync/pair/confirm"),
            ("POST", "/sync/pair/reject"),
        ] {
            assert!(
                crate::call::ROUTES.iter().any(|r| r.method == method && r.path == path),
                "{method} {path} is not a row of ROUTES"
            );
        }
    }

    #[test]
    fn a_farm_submission_is_forwarded_and_a_missing_sync_is_a_clear_refusal() {
        let (port, seen) = fake_sync(1, "HTTP/1.1 202 Accepted\r\nContent-Length: 14\r\nConnection: close\r\n\r\n{\"job\":\"f1\"}\n\n");
        let sent = farm_submit_at(port, json!({ "device": "out-1", "range": [0, 60] })).expect("forwarded");
        assert_eq!(sent["job"], "f1");
        assert_eq!(seen.join().unwrap(), ["POST /farm/submit HTTP/1.1"]);
        match farm_submit_at(free_port(), json!({})) {
            Err(CallError::Bad(msg)) => assert!(msg.contains("not running"), "{msg}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
}
