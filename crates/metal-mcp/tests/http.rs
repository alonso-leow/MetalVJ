//! Streamable HTTP's bearer gate: the binary, spawned for real. Every case
//! points `FW_AGENT_TOKEN_FILE` at a path that does not exist, so the only
//! token in play is the one the test passes (or none).

use serde_json::json;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TOKEN: &str = "test-http-token-0123456789abcdef";

/// A port that was free a moment ago. Racy in principle, fine for one test run.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn no_token_file() -> PathBuf {
    std::env::temp_dir().join(format!("metal-mcp-no-token-{}", std::process::id()))
}

fn spawn(args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_metal-mcp"))
        .args(["--dash", "http://127.0.0.1:9"])
        .args(args)
        .env("FW_AGENT_TOKEN_FILE", no_token_file())
        .env_remove("FW_DASH_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn metal-mcp")
}

/// Waits until `addr` accepts a connection, so the request does not race the bind.
fn wait_for(addr: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(addr).is_err() {
        assert!(Instant::now() < deadline, "metal-mcp never listened on {addr}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One raw POST /mcp, returning the whole reply (status line, headers, body).
fn post_mcp(addr: &str, authorization: Option<&str>) -> String {
    let body = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},
        "clientInfo":{"name":"test","version":"0"}}})
    .to_string();
    let auth = authorization.map(|a| format!("Authorization: {a}\r\n")).unwrap_or_default();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream.write_all(request.as_bytes()).unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply
}

fn status_of(reply: &str) -> u16 {
    reply.split_whitespace().nth(1).and_then(|s| s.parse().ok()).expect("status line")
}

/// Kills the child on drop, so a failing assertion never leaks a server.
struct Guard(Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn post_mcp_needs_the_bearer_token() {
    let addr = format!("127.0.0.1:{}", free_port());
    let _server = Guard(spawn(&["--http", &addr, "--http-token", TOKEN]));
    wait_for(&addr);

    let none = post_mcp(&addr, None);
    assert_eq!(status_of(&none), 401, "{none}");
    assert!(none.contains("WWW-Authenticate: Bearer"), "{none}");

    let wrong = post_mcp(&addr, Some("Bearer not-the-token"));
    assert_eq!(status_of(&wrong), 401, "{wrong}");

    let no_scheme = post_mcp(&addr, Some(TOKEN));
    assert_eq!(status_of(&no_scheme), 401, "{no_scheme}");

    let ok = post_mcp(&addr, Some(&format!("Bearer {TOKEN}")));
    assert_eq!(status_of(&ok), 200, "{ok}");
    assert!(ok.contains("\"protocolVersion\":\"2025-06-18\""), "{ok}");
}

#[test]
fn the_token_file_is_the_default_token() {
    let file = std::env::temp_dir().join(format!("metal-mcp-token-file-{}", std::process::id()));
    std::fs::write(&file, format!("{TOKEN}\n")).unwrap();
    let addr = format!("127.0.0.1:{}", free_port());
    let child = Command::new(env!("CARGO_BIN_EXE_metal-mcp"))
        .args(["--dash", "http://127.0.0.1:9", "--http", &addr])
        .env("FW_AGENT_TOKEN_FILE", &file)
        .env_remove("FW_DASH_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn metal-mcp");
    let _server = Guard(child);
    wait_for(&addr);
    assert_eq!(status_of(&post_mcp(&addr, None)), 401);
    assert_eq!(status_of(&post_mcp(&addr, Some(&format!("Bearer {TOKEN}")))), 200);
    let _ = std::fs::remove_file(&file);
}

/// No token from either source: the server does not start, on any address.
#[test]
fn http_without_a_token_refuses_to_start() {
    for bind in [format!("127.0.0.1:{}", free_port()), format!("0.0.0.0:{}", free_port())] {
        let mut child = spawn(&["--http", &bind]);
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(s) = child.try_wait().unwrap() {
                break s;
            }
            assert!(Instant::now() < deadline, "{bind}: metal-mcp kept running without a token");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(status.code(), Some(2), "{bind}");
    }
}

/// A remote bind is allowed with a token, and the token is then the guard.
#[test]
fn remote_bind_starts_with_a_token() {
    let port = free_port();
    let _server = Guard(spawn(&["--http", &format!("0.0.0.0:{port}"), "--http-token", TOKEN]));
    let local = format!("127.0.0.1:{port}");
    wait_for(&local);
    assert_eq!(status_of(&post_mcp(&local, None)), 401);
    assert_eq!(status_of(&post_mcp(&local, Some(&format!("Bearer {TOKEN}")))), 200);
}
