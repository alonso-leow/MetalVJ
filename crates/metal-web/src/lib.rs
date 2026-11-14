//! The hand-rolled HTTP server, once.
//!
//! MetalVJ's apps serve their UI as a page over a plain `TcpListener` —
//! no async runtime, no framework, the page does the interesting work. That
//! was the right call and it stays. What was not right is that the three
//! studios which merged into `fw-studio` each had their own copy of it:
//! three request parsers, three response writers, three percent-decoders,
//! three slightly different ideas about `Cache-Control`.
//!
//! So this crate is the plumbing all of that shared, plus the static assets
//! (the theme and its fonts) that every MetalVJ surface needs and none of
//! them should embed separately.
//!
//! Scope is deliberately small. It parses exactly as much HTTP as a page
//! talking to `127.0.0.1` over `fetch()` produces: a request line, headers,
//! a `Content-Length` body. No chunked encoding, no keep-alive, no TLS.
//! Anything more would be a framework, and the reason there isn't one is
//! that this fits in a file you can read.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

use serde_json::Value;

pub mod assets;

/// One parsed request. Borrowed from nothing — the stream is done with by the
/// time a handler sees this, so a handler can hold it across a `&mut self`
/// call on the renderer without fighting the borrow checker.
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    /// Reads one request off `stream`. `Ok(None)` means the peer closed
    /// without sending anything, which browsers do routinely when they
    /// pre-open a connection they end up not using — it is not an error and
    /// must not be logged as one.
    pub fn read(stream: &TcpStream) -> std::io::Result<Option<Request>> {
        let mut reader = BufReader::new(stream.try_clone()?);

        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("/");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let (path, query) = (path.to_string(), parse_query(query));

        let mut headers = HashMap::new();
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                headers.insert(k.trim().to_lowercase(), v.trim().to_owned());
            }
        }

        let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut body = vec![0u8; len];
        if len > 0 {
            reader.read_exact(&mut body)?;
        }

        Ok(Some(Request { method, path, query, headers, body }))
    }

    /// `(method, path)`, for matching. Spelled as a method because every
    /// handler in both apps opens with it.
    pub fn route(&self) -> (&str, &str) {
        (&self.method, &self.path)
    }

    /// A query parameter as a number, with a fallback. The clamp is the
    /// caller's job — the ranges are per-endpoint and belong next to them.
    pub fn num(&self, key: &str, fallback: f32) -> f32 {
        self.query.get(key).and_then(|v| v.parse().ok()).unwrap_or(fallback)
    }

    pub fn param(&self, key: &str) -> Option<&str> {
        self.query.get(key).map(String::as_str)
    }

    /// The body as JSON, or `None` if it is not JSON. Handlers treat that as
    /// a 400 rather than a panic: the body came off a socket.
    pub fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

/// Writes a response and closes. Every reply this app makes is small enough
/// to buffer, and `Connection: close` is what makes a one-thread-per-request
/// loop correct without a keep-alive timeout to get wrong.
///
/// `no-store` on everything: the dynamic routes are live frames and live
/// state, where a cached reply is a preview that has quietly stopped moving.
/// The static assets opt out of that with [`cached`].
pub fn reply(
    stream: &mut TcpStream,
    status: &str,
    kind: &str,
    body: &[u8],
) -> std::io::Result<()> {
    write_out(stream, status, kind, body, "no-store")
}

/// A response the browser may keep. Only for the fonts, which are content
/// addressed by the binary that served them and never change under a running
/// server — re-fetching 47 kB of woff2 on every poll would be absurd.
pub fn cached(stream: &mut TcpStream, kind: &str, body: &[u8]) -> std::io::Result<()> {
    write_out(stream, "200 OK", kind, body, "public, max-age=604800, immutable")
}

fn write_out(
    stream: &mut TcpStream,
    status: &str,
    kind: &str,
    body: &[u8],
    cache: &str,
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
         Cache-Control: {cache}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

pub fn html(stream: &mut TcpStream, page: &str) -> std::io::Result<()> {
    reply(stream, "200 OK", "text/html; charset=utf-8", page.as_bytes())
}

pub fn json(stream: &mut TcpStream, value: &Value) -> std::io::Result<()> {
    reply(stream, "200 OK", "application/json", value.to_string().as_bytes())
}

/// `{"ok": true}` — the reply to every command endpoint that has nothing to
/// say beyond having worked.
pub fn ok(stream: &mut TcpStream) -> std::io::Result<()> {
    json(stream, &serde_json::json!({ "ok": true }))
}

pub fn bad(stream: &mut TcpStream, message: &str) -> std::io::Result<()> {
    reply(stream, "400 Bad Request", "text/plain; charset=utf-8", message.as_bytes())
}

/// A same-origin redirect. `location` is relative — every caller resolves it
/// against a request path that has already had a reverse-proxy prefix
/// stripped by the framework, and a root-absolute `Location` would put the
/// browser back at the proxy's own root instead of the app's (see `dash.html`
/// §`ROOT`/`api`, and `check.mjs` §15).
pub fn redirect(stream: &mut TcpStream, location: &str) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 303 See Other\r\nLocation: {location}\r\nContent-Length: 0\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes())?;
    stream.flush()
}

pub fn not_found(stream: &mut TcpStream) -> std::io::Result<()> {
    reply(stream, "404 Not Found", "text/plain; charset=utf-8", b"not found")
}

pub fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_string(), percent_decode(v)))
        .collect()
}

/// Enough of a decoder for what a browser puts in a query string: percent
/// escapes and `+` for space. Invalid escapes are passed through rather than
/// dropped, so a stray `%` in a preset name is a stray `%` and not a
/// truncated name.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};

    #[test]
    fn a_query_string_decodes_escapes_and_plus() {
        let q = parse_query("name=drop+%232&w=320");
        assert_eq!(q.get("name").map(String::as_str), Some("drop #2"));
        assert_eq!(q.get("w").map(String::as_str), Some("320"));
    }

    #[test]
    fn a_stray_percent_survives_rather_than_truncating() {
        // A preset called "100% wet" must come back whole. Dropping the tail
        // would silently save over a different preset.
        assert_eq!(percent_decode("100%25 wet"), "100% wet");
        assert_eq!(percent_decode("100% wet"), "100% wet");
        assert_eq!(percent_decode("trailing%"), "trailing%");
    }

    #[test]
    fn a_post_with_a_body_parses_whole() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();

        std::thread::spawn(move || {
            let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
            let body = br#"{"model":"turing","floats":[0.5]}"#;
            write!(
                s,
                "POST /live?seq=7 HTTP/1.1\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n",
                body.len()
            )
            .expect("head");
            s.write_all(body).expect("body");
        });

        let (stream, _) = listener.accept().expect("accept");
        let req = Request::read(&stream).expect("read").expect("a request");
        assert_eq!(req.route(), ("POST", "/live"));
        assert_eq!(req.num("seq", 0.0), 7.0);
        let value = req.json().expect("json body");
        assert_eq!(value["model"], "turing");
    }

    #[test]
    fn a_peer_that_opens_and_closes_is_not_an_error() {
        // Browsers pre-open connections they never use. If this returned Err
        // the server log would fill with noise that means nothing.
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            let _ = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        });
        let (stream, _) = listener.accept().expect("accept");
        assert!(Request::read(&stream).expect("no error").is_none());
    }
}
