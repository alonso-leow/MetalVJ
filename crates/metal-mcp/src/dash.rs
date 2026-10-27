//! The forwarder: one `tools/call` becomes one HTTP request to the route
//! metal-dash names for that tool in `GET /ai-config` (`tools[].route`). The
//! adapter keeps no table of its own, so the server stays the one place a
//! tool is mapped to a route (rule 6).
//!
//! Arguments travel verbatim: a `GET` route gets them as its query string, a
//! `POST` route gets them as its JSON body. Tools whose route is `null` run
//! inside the agent process (`sleep`, `locate`, ...) and are reported as
//! errors rather than guessed at.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex;
use std::time::Duration;

/// Where metal-dash listens when neither `--dash` nor `FW_DASH_URL` says otherwise.
pub const DEFAULT_DASH_URL: &str = "http://127.0.0.1:7900";

/// Long enough for `raster` or a bake; a route that takes longer is an error.
const TIMEOUT: Duration = Duration::from_secs(120);

type Route = Option<(String, String)>;

pub struct Dash {
    /// `host:port`, as connected to.
    addr: String,
    /// The `/ai-config` route table, loaded on first use.
    routes: Mutex<Option<HashMap<String, Route>>>,
    /// `--allow-ask`: this operator lets MCP callers run `ask`-policy tools.
    allow_ask: bool,
    /// The install's bearer token, sent as `Authorization` on every request.
    /// `POST /agent/tool` refuses without it.
    token: Option<String>,
}

/// The one route an in-process tool forwards to: metal-dash runs the tool through
/// its chat executor. The body is `{name, args, allow_ask}`, not the arguments.
const AGENT_TOOL_PATH: &str = "/agent/tool";

impl Dash {
    /// `url` is `http://host:port` (a trailing slash is fine).
    pub fn new(url: &str) -> Result<Self, String> {
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| format!("--dash must be an http:// URL, got `{url}`"))?;
        let addr = rest.trim_end_matches('/');
        if addr.contains('/') || !addr.contains(':') {
            return Err(format!("--dash must be http://host:port, got `{url}`"));
        }
        Ok(Dash { addr: addr.to_string(), routes: Mutex::new(None), allow_ask: false, token: None })
    }

    /// The bearer token metal-dash issued for this install (see `metal_agent::auth`).
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token.filter(|t| !t.is_empty());
        self
    }

    /// Lets `ask`-policy tools run through `POST /agent/tool` (`--allow-ask`).
    pub fn with_allow_ask(mut self, allow: bool) -> Self {
        self.allow_ask = allow;
        self
    }

    /// Runs one tool. `Err` is a message for the MCP client, never a panic.
    pub fn call(&self, name: &str, args: &Value) -> Result<Value, String> {
        let route = {
            let mut cache = self.routes.lock().unwrap_or_else(|e| e.into_inner());
            if cache.is_none() {
                *cache = Some(self.load_routes()?);
            }
            cache
                .as_ref()
                .and_then(|table| table.get(name).cloned())
                .ok_or_else(|| format!("metal-dash does not list `{name}` in /ai-config"))?
        };
        let Some((method, path)) = route else {
            return Err(format!("metal-dash lists no route for `{name}`"));
        };
        match method.as_str() {
            "GET" => {
                let query = query_string(args);
                let target = if query.is_empty() { path } else { format!("{path}?{query}") };
                self.send("GET", &target, None)
            }
            "POST" => {
                let body = agent_body(name, &path, args, self.allow_ask);
                let bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
                let reply = self.send("POST", &path, Some(&bytes))?;
                // The agent route answers a refusal or a tool error as `{"error"}`;
                // that is the tool failing, so it becomes an MCP tool error. A
                // `code` means a structured answer (`camera_not_streaming`,
                // `needs_gpu_build`): the whole object goes out, so the code is kept.
                match (path.as_str(), reply.get("error").and_then(Value::as_str)) {
                    (AGENT_TOOL_PATH, Some(_)) if reply.get("code").is_some() => {
                        Err(reply.to_string())
                    }
                    (AGENT_TOOL_PATH, Some(message)) => Err(message.to_string()),
                    _ => Ok(reply),
                }
            }
            other => Err(format!("unsupported method {other} for `{name}`")),
        }
    }

    fn load_routes(&self) -> Result<HashMap<String, Route>, String> {
        let body = self.send("GET", "/ai-config", None)?;
        let tools = body
            .get("tools")
            .and_then(Value::as_array)
            .ok_or("/ai-config has no tools array")?;
        let mut table = HashMap::new();
        for tool in tools {
            let Some(name) = tool["name"].as_str() else { continue };
            let route = match (tool["route"]["method"].as_str(), tool["route"]["path"].as_str()) {
                (Some(m), Some(p)) => Some((m.to_string(), p.to_string())),
                _ => None,
            };
            table.insert(name.to_string(), route);
        }
        Ok(table)
    }

    /// One HTTP/1.1 request, `Connection: close`. Returns the response as a
    /// JSON value: the parsed body, a `{"text": ...}` for non-JSON text, or
    /// `{"png_base64": ...}` / `{"image_base64", "mimeType"}` for an image.
    fn send(&self, method: &str, target: &str, body: Option<&[u8]>) -> Result<Value, String> {
        let mut stream = TcpStream::connect(&self.addr)
            .map_err(|e| format!("cannot reach metal-dash at {}: {e}", self.addr))?;
        stream.set_read_timeout(Some(TIMEOUT)).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(TIMEOUT)).map_err(|e| e.to_string())?;

        let payload = body.unwrap_or(&[]);
        let mut request = format!(
            "{method} {target} HTTP/1.1\r\nHost: {}\r\nAccept: application/json, image/*\r\nConnection: close\r\n",
            self.addr
        );
        if let Some(token) = &self.token {
            request.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        if body.is_some() {
            request.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                payload.len()
            ));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
        stream.write_all(payload).map_err(|e| e.to_string())?;

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).map_err(|e| format!("metal-dash read failed: {e}"))?;
        let response = parse_response(&raw)?;
        if response.status >= 400 {
            let text = String::from_utf8_lossy(&response.body);
            if response.status == 401 {
                return Err(format!(
                    "metal-dash refused the request (401): no valid agent token; set FW_DASH_TOKEN or run metal-dash once to create {}",
                    metal_agent::auth::token_path().display()
                ));
            }
            return Err(format!("metal-dash answered HTTP {}: {}", response.status, text.trim()));
        }
        let content_type = response.headers.get("content-type").cloned().unwrap_or_default();
        if content_type.starts_with("image/") {
            return Ok(json!({
                "image_base64": base64(&response.body),
                "mimeType": content_type.split(';').next().unwrap_or("image/png").trim(),
            }));
        }
        match serde_json::from_slice::<Value>(&response.body) {
            Ok(value) => Ok(value),
            Err(_) => Ok(json!({ "text": String::from_utf8_lossy(&response.body) })),
        }
    }
}

struct Response {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn parse_response(raw: &[u8]) -> Result<Response, String> {
    let split = find(raw, b"\r\n\r\n").ok_or("malformed response from metal-dash")?;
    let head = std::str::from_utf8(&raw[..split]).map_err(|e| e.to_string())?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or("malformed status line from metal-dash")?;
    let headers: HashMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let rest = &raw[split + 4..];
    let body = if headers
        .get("transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    {
        dechunk(rest)?
    } else {
        rest.to_vec()
    };
    Ok(Response { status, headers, body })
}

fn dechunk(mut data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let end = find(data, b"\r\n").ok_or("bad chunk framing from metal-dash")?;
        let size_text = std::str::from_utf8(&data[..end]).map_err(|e| e.to_string())?;
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| "bad chunk size from metal-dash".to_string())?;
        data = &data[end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if data.len() < size + 2 {
            return Err("truncated chunk from metal-dash".into());
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

/// Arguments as a query string: strings raw, scalars and compound values as
/// their JSON text, nulls dropped. Everything outside the unreserved set is
/// percent-encoded.
fn query_string(args: &Value) -> String {
    let Some(map) = args.as_object() else { return String::new() };
    let mut parts = Vec::new();
    for (key, value) in map {
        let text = match value {
            Value::Null => continue,
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        parts.push(format!("{}={}", percent(key), percent(&text)));
    }
    parts.join("&")
}

fn percent(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The body for a `POST` route. Verbatim, except for [`AGENT_TOOL_PATH`], which
/// names the tool and carries the operator's `--allow-ask` choice.
pub fn agent_body(name: &str, path: &str, args: &Value, allow_ask: bool) -> Value {
    if path == AGENT_TOOL_PATH {
        json!({ "name": name, "args": args, "allow_ask": allow_ask })
    } else {
        args.clone()
    }
}

/// Standard base64 with padding (RFC 4648).
pub fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn query_keeps_scalars_and_encodes_the_rest() {
        let q = query_string(&json!({ "graph": "a b", "n": 3, "on": true, "x": null, "p": [1] }));
        let mut parts: Vec<&str> = q.split('&').collect();
        parts.sort();
        assert_eq!(parts, vec!["graph=a%20b", "n=3", "on=true", "p=%5B1%5D"]);
    }

    #[test]
    fn dechunks_a_chunked_body() {
        assert_eq!(dechunk(b"4\r\nWiki\r\n5;ext=1\r\npedia\r\n0\r\n\r\n").unwrap(), b"Wikipedia");
    }

    #[test]
    fn in_process_tools_are_wrapped_and_ordinary_routes_are_verbatim() {
        let args = json!({ "ms": 5 });
        assert_eq!(
            agent_body("sleep", AGENT_TOOL_PATH, &args, true),
            json!({ "name": "sleep", "args": { "ms": 5 }, "allow_ask": true })
        );
        assert_eq!(agent_body("list_nodes", "/graph", &args, false), args);
    }

    /// A one-thread fake metal-dash: answers each request in `replies` in order
    /// and hands back the raw request text it read.
    fn fake_dash(replies: Vec<(&'static str, String)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for (status, body) in replies {
                let (mut conn, _) = listener.accept().unwrap();
                // Read until the headers and the Content-Length body have both arrived.
                let mut raw = Vec::new();
                loop {
                    let mut chunk = [0u8; 4096];
                    let n = conn.read(&mut chunk).unwrap();
                    raw.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&raw).to_string();
                    if let Some(split) = text.find("\r\n\r\n") {
                        let len = text[..split]
                            .lines()
                            .find_map(|l| l.strip_prefix("Content-Length: "))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if raw.len() >= split + 4 + len {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                seen.push(String::from_utf8_lossy(&raw).to_string());
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                conn.write_all(head.as_bytes()).unwrap();
                conn.write_all(body.as_bytes()).unwrap();
            }
            seen
        });
        (url, handle)
    }

    fn ai_config() -> String {
        json!({ "tools": [
            { "name": "sleep", "route": { "method": "POST", "path": "/agent/tool" } },
            { "name": "list_nodes", "route": { "method": "GET", "path": "/graph" } },
        ]})
        .to_string()
    }

    #[test]
    fn in_process_tool_goes_to_agent_tool_wrapped_and_refusals_become_tool_errors() {
        let (url, server) = fake_dash(vec![
            ("200 OK", ai_config()),
            ("200 OK", json!({ "error": "denied by operator" }).to_string()),
        ]);
        let dash = Dash::new(&url).unwrap();
        let err = dash.call("sleep", &json!({ "ms": 1 })).unwrap_err();
        assert_eq!(err, "denied by operator");
        let seen = server.join().unwrap();
        assert!(seen[1].starts_with("POST /agent/tool HTTP/1.1"), "{}", seen[1]);
        let body = seen[1].split("\r\n\r\n").nth(1).unwrap();
        let sent: Value = serde_json::from_str(body).unwrap();
        assert_eq!(sent, json!({ "name": "sleep", "args": { "ms": 1 }, "allow_ask": false }));
    }

    #[test]
    fn allow_ask_is_sent_and_ordinary_results_pass_through() {
        let (url, server) = fake_dash(vec![
            ("200 OK", ai_config()),
            ("200 OK", json!({ "ok": true, "slept_ms": 1 }).to_string()),
        ]);
        let dash = Dash::new(&url).unwrap().with_allow_ask(true);
        let out = dash.call("sleep", &json!({ "ms": 1 })).unwrap();
        assert_eq!(out["slept_ms"], 1);
        let seen = server.join().unwrap();
        assert!(seen[1].contains("\"allow_ask\":true"), "{}", seen[1]);
    }

    #[test]
    fn the_token_is_sent_as_a_bearer_header() {
        let (url, server) = fake_dash(vec![
            ("200 OK", ai_config()),
            ("200 OK", json!({ "ok": true }).to_string()),
        ]);
        let dash = Dash::new(&url).unwrap().with_token(Some("tok123".into()));
        dash.call("sleep", &json!({ "ms": 1 })).unwrap();
        let seen = server.join().unwrap();
        assert!(seen[1].contains("\r\nAuthorization: Bearer tok123\r\n"), "{}", seen[1]);
    }

    #[test]
    fn structured_tool_errors_keep_their_code() {
        let (url, server) = fake_dash(vec![
            ("200 OK", ai_config()),
            ("200 OK", json!({ "ok": false, "code": "needs_gpu_build", "error": "needs gpu build" }).to_string()),
        ]);
        let err = Dash::new(&url).unwrap().call("sleep", &json!({})).unwrap_err();
        let parsed: Value = serde_json::from_str(&err).expect("json error");
        assert_eq!(parsed["code"], "needs_gpu_build");
        server.join().unwrap();
    }

    #[test]
    fn new_wants_an_http_host_and_port() {
        assert!(Dash::new("http://127.0.0.1:7900/").is_ok());
        assert!(Dash::new("https://127.0.0.1:7900").is_err());
        assert!(Dash::new("http://127.0.0.1").is_err());
        assert!(Dash::new("http://127.0.0.1:7900/graph").is_err());
    }
}
