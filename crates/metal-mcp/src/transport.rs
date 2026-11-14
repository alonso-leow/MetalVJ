//! The two wires: newline-delimited JSON on stdio, and Streamable HTTP
//! (`POST /mcp`, one JSON-RPC message per request body).

use crate::dash::Dash;
use crate::rpc;
use metal_agent::auth::tokens_match;
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Largest request body accepted over HTTP (a frame upload fits well inside).
const MAX_BODY: usize = 16 * 1024 * 1024;

/// Reads requests from stdin, one per line, and writes replies to stdout.
/// Stdout carries protocol JSON only; diagnostics go to stderr.
pub fn serve_stdio(dash: &Dash) -> io::Result<()> {
    let stdin = io::stdin();
    let mut out = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = rpc::handle_line(&line, dash) {
            out.write_all(reply.as_bytes())?;
            out.write_all(b"\n")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Serves `POST /mcp` on `bind` (`:PORT` means `127.0.0.1:PORT`). Every
/// request must carry `Authorization: Bearer <http_token>`; without it the
/// reply is 401 and nothing reaches the dash. Each connection is handled on
/// its own thread; one connection is one request.
pub fn serve_http(bind: &str, dash: Dash, http_token: &str) -> io::Result<()> {
    let bind = if bind.starts_with(':') { format!("127.0.0.1{bind}") } else { bind.to_string() };
    let listener = TcpListener::bind(&bind)?;
    eprintln!("metal-mcp: Streamable HTTP on http://{}/mcp (bearer token required)", listener.local_addr()?);
    let dash = Arc::new(dash);
    let http_token: Arc<str> = Arc::from(http_token);
    for stream in listener.incoming() {
        let stream = stream?;
        let dash = Arc::clone(&dash);
        let http_token = Arc::clone(&http_token);
        thread::spawn(move || {
            if let Err(e) = handle_connection(stream, &dash, &http_token) {
                eprintln!("metal-mcp: connection error: {e}");
            }
        });
    }
    Ok(())
}

fn handle_connection(mut stream: TcpStream, dash: &Dash, http_token: &str) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();

    let mut headers: HashMap<String, String> = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let path = target.split('?').next().unwrap_or("");
    if path != "/mcp" {
        return respond(&mut stream, 404, "text/plain", &[], b"not found");
    }
    // The client's own credential, checked before anything else about the
    // request is looked at. The comparison is constant-time.
    if !bearer_authorized(headers.get("authorization").map(String::as_str), http_token) {
        return respond(
            &mut stream,
            401,
            "text/plain",
            &[("WWW-Authenticate", "Bearer")],
            b"unauthorized: send Authorization: Bearer <token>",
        );
    }
    // Streamable HTTP: a browser page on another origin must not drive the
    // server through the user's loopback (DNS rebinding). Only local origins.
    if let Some(origin) = headers.get("origin") {
        if !local_origin(origin) {
            return respond(&mut stream, 403, "text/plain", &[], b"origin not allowed");
        }
    }
    if method != "POST" {
        return respond(&mut stream, 405, "text/plain", &[("Allow", "POST")], b"POST only");
    }
    let length: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if length > MAX_BODY {
        return respond(&mut stream, 413, "text/plain", &[], b"body too large");
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let text = String::from_utf8_lossy(&body);

    match rpc::handle_line(&text, dash) {
        Some(reply) => respond(&mut stream, 200, "application/json", &[], reply.as_bytes()),
        // Only notifications/responses arrived: accepted, nothing to say.
        None => respond(&mut stream, 202, "text/plain", &[], b""),
    }
}

/// `true` when `header` is `Bearer <token>` and the token is `expected`.
/// A blank expected token never matches, so an unconfigured server refuses.
fn bearer_authorized(header: Option<&str>, expected: &str) -> bool {
    if expected.is_empty() {
        return false;
    }
    let Some(presented) = header.and_then(|h| h.strip_prefix("Bearer ")) else {
        return false;
    };
    tokens_match(expected, presented.trim())
}

/// Whether `bind` (`HOST:PORT`, `:PORT`, or `[::1]:PORT`) names only this
/// machine. Anything that does not parse as a socket address and is not
/// `localhost` counts as remote.
pub fn is_loopback_bind(bind: &str) -> bool {
    if bind.starts_with(':') {
        return true;
    }
    if let Ok(addr) = bind.parse::<std::net::SocketAddr>() {
        return addr.ip().is_loopback();
    }
    bind.rsplit_once(':').map(|(host, _)| host == "localhost").unwrap_or(false)
}

fn local_origin(origin: &str) -> bool {
    let host = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(origin)
        .split('/')
        .next()
        .unwrap_or("");
    let host = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    extra: &[(&str, &str)],
    body: &[u8],
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::{bearer_authorized, is_loopback_bind, local_origin};

    #[test]
    fn only_local_origins_pass() {
        assert!(local_origin("http://localhost:3000"));
        assert!(local_origin("http://127.0.0.1"));
        assert!(local_origin("http://[::1]:8080"));
        assert!(!local_origin("https://evil.example"));
        assert!(!local_origin("http://localhost.evil.example"));
    }

    #[test]
    fn bearer_must_match_the_configured_token() {
        assert!(bearer_authorized(Some("Bearer s3cret"), "s3cret"));
        assert!(!bearer_authorized(Some("Bearer wrong"), "s3cret"));
        assert!(!bearer_authorized(Some("s3cret"), "s3cret"), "no Bearer scheme");
        assert!(!bearer_authorized(None, "s3cret"));
        assert!(!bearer_authorized(Some("Bearer "), "s3cret"));
        assert!(!bearer_authorized(Some("Bearer "), ""), "blank expected never matches");
    }

    #[test]
    fn loopback_binds_are_recognised() {
        assert!(is_loopback_bind(":7910"));
        assert!(is_loopback_bind("127.0.0.1:7910"));
        assert!(is_loopback_bind("[::1]:7910"));
        assert!(is_loopback_bind("localhost:7910"));
        assert!(!is_loopback_bind("0.0.0.0:7910"));
        assert!(!is_loopback_bind("192.168.1.5:7910"));
        assert!(!is_loopback_bind("[::]:7910"));
        assert!(!is_loopback_bind("example.com:7910"));
    }
}
