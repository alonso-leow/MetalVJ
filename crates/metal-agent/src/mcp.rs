//! MCP client. `metal-agent` is a client only, never a server (H.3): it connects
//! *out* to MCP servers over stdio or HTTP and folds their tools into the
//! registry, prefixed with the server's name.
//!
//! Synchronous by design, matching the rest of the repo — no async runtime
//! anywhere, so this uses blocking child-process pipes for stdio and the
//! blocking [`crate::http`] client for HTTP, the same way `metal-web` hand-rolls
//! its server instead of pulling in an async HTTP stack.
//!
//! Judgment call (flagged for review before H.4): MCP's real wire format is
//! JSON-RPC 2.0 batches over these transports, with an initialize handshake,
//! capability negotiation, and notifications. This pass implements the
//! narrow subset needed to prove the transport shape and the tool-merge
//! rule: a single JSON-RPC request/response round trip per call
//! (`initialize`, `tools/list`, `tools/call`), no batching, no
//! notifications, no capability negotiation beyond a fixed minimal
//! `initialize` payload. That is very likely not a complete MCP client and
//! will need revisiting once H.4 talks to a real server.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use metal_schema::platform::Command;

/// One configured MCP server, as it would be added from the config overlay
/// (H.8).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct McpServerConfig {
    /// Short name, used as the tool-name prefix (`"blender.render"`).
    pub name: String,
    pub transport: McpTransport,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum McpTransport {
    Stdio { command: String, args: Vec<String> },
    Http { url: String },
}

/// A live connection to one MCP server. Owns the child process for stdio, or
/// just the URL for HTTP — either way, calls block until the server
/// responds, same as everything else in this crate.
pub enum McpConnection {
    Stdio { child: Child, stdin: ChildStdin, stdout: BufReader<ChildStdout> },
    Http { url: String },
}

pub struct McpClient {
    pub config: McpServerConfig,
    connection: McpConnection,
    next_id: u64,
}

impl McpClient {
    /// Connects and runs the `initialize` handshake. For stdio this spawns
    /// the child process; for HTTP nothing is opened yet (the client is
    /// stateless per-request), so `connect` just records the URL and probes
    /// it with `initialize`.
    pub fn connect(config: McpServerConfig) -> Result<Self, String> {
        let connection = match &config.transport {
            McpTransport::Stdio { command, args } => {
                let mut child = Command::new(command)
                    .args(args)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .map_err(|e| format!("spawn {command}: {e}"))?;
                let stdin = child.stdin.take().ok_or("no stdin")?;
                let stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
                McpConnection::Stdio { child, stdin, stdout }
            }
            McpTransport::Http { url } => McpConnection::Http { url: url.clone() },
        };

        let mut client = McpClient { config, connection, next_id: 1 };
        client.call("initialize", json!({}))?;
        Ok(client)
    }

    /// Lists the server's tools, each returned with `{server}.` prefixed
    /// onto its name — see [`crate::tools::ToolRegistry`] for why the prefix
    /// matters (it's what lets a native tool of the same bare name win).
    pub fn list_tools(&mut self) -> Result<Vec<McpTool>, String> {
        let result = self.call("tools/list", json!({}))?;
        let raw_tools = result.get("tools").and_then(Value::as_array).cloned().unwrap_or_default();
        let prefix = self.config.name.clone();
        Ok(raw_tools
            .into_iter()
            .filter_map(|t| {
                let bare_name = t.get("name")?.as_str()?.to_string();
                Some(McpTool { prefixed_name: format!("{prefix}.{bare_name}"), bare_name, spec: t })
            })
            .collect())
    }

    pub fn call_tool(&mut self, bare_name: &str, arguments: Value) -> Result<Value, String> {
        self.call("tools/call", json!({ "name": bare_name, "arguments": arguments }))
    }

    /// One JSON-RPC 2.0 request/response round trip.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let response = match &mut self.connection {
            McpConnection::Stdio { stdin, stdout, .. } => {
                let mut line = serde_json::to_string(&request).map_err(|e| e.to_string())?;
                line.push('\n');
                stdin.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
                stdin.flush().map_err(|e| e.to_string())?;

                let mut reply_line = String::new();
                let n = stdout.read_line(&mut reply_line).map_err(|e| e.to_string())?;
                if n == 0 {
                    return Err(format!("mcp server {} closed stdout", self.config.name));
                }
                serde_json::from_str::<Value>(&reply_line).map_err(|e| e.to_string())?
            }
            McpConnection::Http { url } => {
                let resp = crate::http::post_json(url, &[], &request)?;
                resp.json().ok_or_else(|| format!("mcp server {} sent non-JSON reply", self.config.name))?
            }
        };

        if let Some(err) = response.get("error") {
            return Err(format!("mcp error from {}: {err}", self.config.name));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// A tool as advertised by an MCP server: its bare name (as the server knows
/// it), the name prefixed for the registry, and the raw spec JSON (schema
/// etc.) passed through untouched.
pub struct McpTool {
    pub bare_name: String,
    pub prefixed_name: String,
    pub spec: Value,
}

/// The set of connected MCP servers, keyed by name — what the config overlay
/// (H.8) adds to and what the loop consults to find a server for a
/// `{server}.{tool}` call.
#[derive(Default)]
pub struct McpRegistry {
    clients: HashMap<String, McpClient>,
}

impl McpRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a new MCP server to the running config, connecting immediately
    /// so a bad command/URL fails at add-time rather than at first use.
    pub fn add_server(&mut self, config: McpServerConfig) -> Result<(), String> {
        let name = config.name.clone();
        let client = McpClient::connect(config)?;
        self.clients.insert(name, client);
        Ok(())
    }

    pub fn get_mut(&mut self, server: &str) -> Option<&mut McpClient> {
        self.clients.get_mut(server)
    }

    pub fn servers(&self) -> impl Iterator<Item = &str> {
        self.clients.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny fake MCP server over stdio: reads one JSON-RPC line, replies
    /// with a canned `tools/list` result, using only `python3` (present in
    /// most dev environments) so this test doesn't need a real MCP SDK.
    /// If python3 isn't available the test is skipped rather than failed —
    /// this is exercising metal-agent's client plumbing, not the environment.
    #[test]
    fn stdio_transport_round_trips_initialize_and_tools_list() {
        if Command::new("python3").arg("--version").output().is_err() {
            eprintln!("skipping: python3 not available");
            return;
        }

        let script = r#"
import sys, json
for line in sys.stdin:
    req = json.loads(line)
    if req["method"] == "initialize":
        result = {}
    elif req["method"] == "tools/list":
        result = {"tools": [{"name": "render"}]}
    else:
        result = {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result}) + "\n")
    sys.stdout.flush()
"#;
        let config = McpServerConfig {
            name: "blender".to_string(),
            transport: McpTransport::Stdio {
                command: "python3".to_string(),
                args: vec!["-c".to_string(), script.to_string()],
            },
        };

        let mut client = McpClient::connect(config).expect("connect");
        let tools = client.list_tools().expect("list_tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].bare_name, "render");
        assert_eq!(tools[0].prefixed_name, "blender.render");
    }
}
