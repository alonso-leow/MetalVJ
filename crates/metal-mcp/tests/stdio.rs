//! Spawns the real binary over stdio and runs initialize + tools/list. No
//! metal-dash is needed: neither method touches it.

use metal_agent::harness::TOOL_CATALOG;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn stdio_initialize_then_tools_list() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_metal-mcp"))
        .args(["--dash", "http://127.0.0.1:9"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn metal-mcp");

    let mut stdin = child.stdin.take().unwrap();
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},
            "clientInfo":{"name":"test","version":"0"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    ];
    for request in &requests {
        writeln!(stdin, "{request}").unwrap();
    }
    drop(stdin);

    let replies: Vec<Value> = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).expect("stdout is JSON only"))
        .collect();
    assert!(child.wait().unwrap().success());

    // The notification gets no reply: exactly two replies, ids 1 and 2.
    assert_eq!(replies.len(), 2, "{replies:?}");
    let init = &replies[0]["result"];
    assert_eq!(replies[0]["id"], 1);
    assert_eq!(init["protocolVersion"], "2025-06-18");
    assert_eq!(init["serverInfo"]["name"], "metal-mcp");

    let tools = replies[1]["result"]["tools"].as_array().expect("tools array");
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort();
    let mut want: Vec<&str> = TOOL_CATALOG
        .iter()
        .filter(|(n, _, ok)| *ok && !n.starts_with("ai."))
        .map(|(n, _, _)| *n)
        .collect();
    want.sort();
    assert_eq!(names, want);
    assert!(names.iter().all(|n| !n.starts_with("ai.")));
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], "object");
    }
}
