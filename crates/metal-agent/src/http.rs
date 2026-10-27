//! A hand-rolled *outbound* blocking HTTP/1.1 client, in the same spirit as
//! `metal-web`'s hand-rolled inbound server: this repo avoids an async runtime
//! and an HTTP client dependency. The one thing the hand-rolled request
//! layer does not do is TLS — the provider presets are all `https://`, so
//! the TLS leg is delegated to `rustls` (the same crate the workspace
//! already carries for iroh's QUIC transport) and the same hand-rolled
//! request/response parsing runs over the encrypted stream unchanged:
//! the only extra is [`tls_stream`], which hands back a `Read + Write`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;
use std::sync::Arc;

use rustls::pki_types::ServerName;

/// A minimal parsed URL: scheme, host, port, and path+query as one string.
/// Not a general URL parser — just enough to open a socket and write a
/// request line, matching how much HTTP `metal-web::Request` bothers to parse
/// on the inbound side.
pub struct Url {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub tls: bool,
}

impl Url {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let (scheme, rest) = raw.split_once("://").ok_or_else(|| format!("no scheme in {raw}"))?;
        let tls = match scheme {
            "http" => false,
            "https" => true,
            other => return Err(format!("unsupported scheme {other}")),
        };
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let path = format!("/{path}");
        let (host, port) = match authority.split_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().map_err(|_| format!("bad port in {raw}"))?),
            None => (authority.to_string(), if tls { 443 } else { 80 }),
        };
        Ok(Url { host, port, path, tls })
    }
}

/// A response with just enough structure for JSON-speaking callers: status
/// line split into a code, headers lowercased, and the body as raw bytes.
#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

/// One blocking, non-streaming POST. Enough for the provider loop (single
/// chat/completions call per turn-step) and for MCP-over-HTTP requests.
pub fn post_json(
    url: &str,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
) -> Result<HttpResponse, String> {
    let payload = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    post_bytes(url, headers, "application/json", &payload)
}

/// One blocking, non-streaming POST with an arbitrary body and
/// `Content-Type` — the shared primitive under [`post_json`] and H.6's
/// multipart STT upload (see [`post_multipart`]), which need a body that
/// isn't `application/json`.
///
/// `https://` URLs run through [`tls_stream`] — rustls over the same hand
/// request this module writes for `http://`, so both legs share the exact
/// request building and response parsing.
pub fn post_bytes(
    url: &str,
    headers: &[(&str, &str)],
    content_type: &str,
    payload: &[u8],
) -> Result<HttpResponse, String> {
    let parsed = Url::parse(url)?;

    let mut head = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        parsed.path,
        parsed.host,
        content_type,
        payload.len()
    );
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");

    if parsed.tls {
        let mut stream = tls_stream(&parsed)?;
        return round_trip(&mut stream, &head, payload);
    }

    let mut stream = TcpStream::connect((parsed.host.as_str(), parsed.port))
        .map_err(|e| format!("connect {}:{}: {e}", parsed.host, parsed.port))?;
    set_socket_timeouts(&mut stream);
    round_trip(&mut stream, &head, payload)
}

/// Writes the prepared request head + payload once and reads one response
/// back — the one place `post_bytes` does its I/O, generic over the socket
/// kind (plain `TcpStream` or the rustls wrapper, same trait bound).
fn round_trip<S: Read + Write>(
    stream: &mut S,
    head: &str,
    payload: &[u8],
) -> Result<HttpResponse, String> {
    stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    stream.write_all(payload).map_err(|e| e.to_string())?;
    read_response(stream)
}

/// A TLS stream for the parsed URL: TCP, then the rustls client handshake on
/// top, then wrapped as `Read + Write` (`StreamOwned`) so the caller keeps
/// using the exact same request/response parsing the plain path runs.
///
/// The trust store is `webpki-roots` — a static, tree-carried set of the
/// public CA roots; there is no per-OS story (system root parsing), and a
/// provider not chaining to a public CA (a self-signed endpoint, a proxy)
/// fails with rustls' own certificate error rather than being silently
/// downgraded to plaintext.
fn tls_stream(parsed: &Url) -> Result<rustls::StreamOwned<rustls::ClientConnection, TcpStream>, String> {
    let root_store = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.iter().cloned().collect(),
    };
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let server_name = ServerName::try_from(parsed.host.clone())
        .map_err(|e| format!("bad server name: {e}"))?;

    let conn = rustls::ClientConnection::new(Arc::new(config), server_name)
        .map_err(|e| format!("tls connect {}: {e}", parsed.host))?;
    let mut tcp = TcpStream::connect((parsed.host.as_str(), parsed.port))
        .map_err(|e| format!("connect {}:{}: {e}", parsed.host, parsed.port))?;
    set_socket_timeouts(&mut tcp);

    Ok(rustls::StreamOwned::new(conn, tcp))
}

fn set_socket_timeouts(tcp: &mut TcpStream) {
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(120)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(30)));
}

/// Builds a `multipart/form-data` body: a run of string fields followed by
/// one file field, matching the shape every OpenAI-compatible
/// `/audio/transcriptions` endpoint expects (`model` as a plain field,
/// `file` as the upload). Returns `(content_type, body)` — the caller passes
/// both straight to [`post_bytes`].
///
/// Hand-rolled rather than pulled in, same rationale as the rest of this
/// module: one caller, one fixed shape, no need for a general multipart
/// crate.
pub fn build_multipart(
    fields: &[(&str, &str)],
    file_field: &str,
    file_name: &str,
    file_bytes: &[u8],
) -> (String, Vec<u8>) {
    let boundary = format!("metal-agent-boundary-{}", std::process::id());
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"{file_field}\"; filename=\"{file_name}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(file_bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// The two shapes a provider reply can take when streaming was requested:
/// a real `text/event-stream`, or a plain JSON body from an endpoint that
/// ignored `stream`. One request decides which — never a second call.
pub enum StreamOrJson {
    /// Events were delivered through `on_event`; the status was 2xx.
    Sse(u16),
    /// Not an event stream; the whole reply is here to parse as usual.
    Json(HttpResponse),
}

/// One blocking POST that either streams SSE `data:` payloads to `on_event`
/// or hands back the whole JSON reply, decided by the response `Content-Type`.
/// Used by the provider adapters for token streaming; a non-SSE endpoint
/// degrades to [`HttpResponse`] without a second request.
pub fn post_stream(
    url: &str,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
    mut on_event: impl FnMut(&str),
) -> Result<StreamOrJson, String> {
    let parsed = Url::parse(url)?;
    let payload = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let mut head = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        parsed.path,
        parsed.host,
        payload.len()
    );
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");

    if parsed.tls {
        let mut stream = tls_stream(&parsed)?;
        let _ = stream.sock.set_read_timeout(Some(Duration::from_secs(300)));
        stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
        stream.write_all(&payload).map_err(|e| e.to_string())?;
        return read_stream_or_json(stream, &mut on_event);
    }

    let mut stream = TcpStream::connect((parsed.host.as_str(), parsed.port))
        .map_err(|e| format!("connect {}:{}: {e}", parsed.host, parsed.port))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(300)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    stream.write_all(&payload).map_err(|e| e.to_string())?;
    read_stream_or_json(stream, &mut on_event)
}

fn read_stream_or_json<S: Read>(
    stream: S,
    on_event: &mut impl FnMut(&str),
) -> Result<StreamOrJson, String> {
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).map_err(|e| e.to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad status line: {status_line}"))?;

    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }

    let is_sse = headers
        .get("content-type")
        .map(|c| c.contains("text/event-stream"))
        .unwrap_or(false);

    if !is_sse {
        let mut body = Vec::new();
        reader.read_to_end(&mut body).map_err(|e| e.to_string())?;
        return Ok(StreamOrJson::Json(HttpResponse { status, headers, body }));
    }

    if status != 200 {
        let mut body = String::new();
        let _ = reader.read_to_string(&mut body);
        return Err(format!("HTTP {status}: {}", body.trim()));
    }

    let mut data = String::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            if !data.trim().is_empty() {
                on_event(data.trim_end());
            }
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            if !data.trim().is_empty() {
                on_event(data.trim_end());
            }
            data.clear();
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        }
    }
    Ok(StreamOrJson::Sse(status))
}

fn read_response(stream: impl Read) -> Result<HttpResponse, String> {
    let mut reader = BufReader::new(stream);

    let mut status_line = String::new();
    reader.read_line(&mut status_line).map_err(|e| e.to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad status line: {status_line}"))?;

    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }

    let mut body = Vec::new();
    if let Some(len) = headers.get("content-length").and_then(|v| v.parse::<usize>().ok()) {
        body.resize(len, 0);
        reader.read_exact(&mut body).map_err(|e| e.to_string())?;
    } else {
        // No Content-Length and `Connection: close` (which is what this
        // client always sends) means "read until EOF" — same convention
        // metal-web relies on for the inverse direction.
        reader.read_to_end(&mut body).map_err(|e| e.to_string())?;
    }

    Ok(HttpResponse { status, headers, body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn parses_http_url() {
        let u = Url::parse("http://localhost:8080/mcp").unwrap();
        assert_eq!(u.host, "localhost");
        assert_eq!(u.port, 8080);
        assert_eq!(u.path, "/mcp");
        assert!(!u.tls);
    }

    #[test]
    fn parses_https_url() {
        let u = Url::parse("https://api.anthropic.example/v1/messages").unwrap();
        assert_eq!(u.host, "api.anthropic.example");
        assert_eq!(u.port, 443);
        assert_eq!(u.path, "/v1/messages");
        assert!(u.tls);
    }

    #[test]
    fn posts_and_parses_a_json_response_over_plain_tcp() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            // Use metal-web's own request parser to read the request off the
            // socket, since a single `read` isn't guaranteed to return the
            // whole thing in one TCP segment (the client keeps its write
            // half open waiting for a reply, so there's no EOF to read to).
            let req = metal_web::Request::read(&s).unwrap().unwrap();
            let mut s = s;
            assert_eq!(req.route(), ("POST", "/chat"));
            assert!(String::from_utf8_lossy(&req.body).contains("hello"));

            let body = br#"{"ok":true}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            s.write_all(resp.as_bytes()).unwrap();
            s.write_all(body).unwrap();
        });

        let resp = post_json(
            &format!("http://127.0.0.1:{port}/chat"),
            &[("Authorization", "Bearer test")],
            &serde_json::json!({"msg": "hello"}),
        )
        .unwrap();

        assert_eq!(resp.status, 200);
        assert_eq!(resp.json().unwrap()["ok"], true);
    }

    #[test]
    fn multipart_body_contains_fields_and_file_between_boundaries() {
        let (content_type, body) =
            build_multipart(&[("model", "whisper-1")], "file", "clip.wav", b"RIFF....fake");
        let boundary = content_type.split("boundary=").nth(1).unwrap();
        let text = String::from_utf8_lossy(&body);

        assert!(content_type.starts_with("multipart/form-data; boundary="));
        assert!(text.contains(&format!("--{boundary}\r\n")));
        assert!(text.contains("name=\"model\""));
        assert!(text.contains("whisper-1"));
        assert!(text.contains("name=\"file\"; filename=\"clip.wav\""));
        assert!(text.contains(&format!("--{boundary}--\r\n")));
        // The raw audio bytes land in the body untouched.
        assert!(body.windows(12).any(|w| w == b"RIFF....fake"));
    }
}
