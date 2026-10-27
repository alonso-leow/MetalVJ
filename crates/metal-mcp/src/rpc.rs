//! JSON-RPC 2.0 for MCP: `initialize`, `ping`, `tools/list`, `tools/call`.
//! Transport-agnostic: one message in, at most one reply out.

use crate::catalog;
use crate::dash::Dash;
use crate::PROTOCOL_VERSION;
use serde_json::{json, Value};

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

const INSTRUCTIONS: &str = "MetalVJ's agent tools. Each tool is a thin call \
    into the running metal-dash app and answers what that app answers; read-only \
    tools are marked readOnlyHint, state-changing ones destructiveHint.";

/// Handles one line of input. `None` when no reply is due (a notification).
pub fn handle_line(line: &str, dash: &Dash) -> Option<String> {
    let message: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(error_reply(Value::Null, PARSE_ERROR, &format!("parse error: {e}")).to_string())
        }
    };
    if message.is_array() {
        return Some(
            error_reply(Value::Null, INVALID_REQUEST, "batch requests are not part of MCP 2025-06-18")
                .to_string(),
        );
    }
    handle_message(&message, dash).map(|reply| reply.to_string())
}

/// Handles one parsed JSON-RPC message. Notifications (no `id`) get `None`.
pub fn handle_message(message: &Value, dash: &Dash) -> Option<Value> {
    let id = message.get("id")?.clone();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Some(error_reply(id, INVALID_REQUEST, "request has no method"));
    };
    let outcome = match method {
        "initialize" => Ok(initialize()),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": catalog::tools() })),
        "tools/call" => call_tool(message.get("params"), dash),
        other => Err((METHOD_NOT_FOUND, format!("method not found: {other}"))),
    };
    Some(match outcome {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, text)) => error_reply(id, code, &text),
    })
}

fn error_reply(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "metal-mcp", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

fn call_tool(params: Option<&Value>, dash: &Dash) -> Result<Value, (i64, String)> {
    let name = params
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
        .ok_or((INVALID_PARAMS, "tools/call needs params.name".to_string()))?;
    if !catalog::is_exported(name) {
        return Err((INVALID_PARAMS, format!("unknown tool: {name}")));
    }
    let args = params.and_then(|p| p.get("arguments")).cloned().unwrap_or_else(|| json!({}));
    if !args.is_object() {
        return Err((INVALID_PARAMS, "tools/call arguments must be an object".to_string()));
    }
    Ok(match dash.call(name, &args) {
        Ok(value) => tool_result(value, false),
        Err(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": true }),
    })
}

/// A tool answer as MCP content. A PNG (or any image) inside the JSON is
/// lifted out as an image block so the client gets pixels, not a base64 wall
/// of text; the JSON text keeps everything else.
fn tool_result(mut value: Value, is_error: bool) -> Value {
    let mut images = Vec::new();
    if let Some(obj) = value.as_object_mut() {
        if let Some(Value::String(data)) = obj.remove("png_base64") {
            images.push(json!({ "type": "image", "data": data, "mimeType": "image/png" }));
        }
        if obj.contains_key("image_base64") && obj.contains_key("mimeType") {
            if let (Some(Value::String(data)), Some(Value::String(mime))) =
                (obj.remove("image_base64"), obj.remove("mimeType"))
            {
                images.push(json!({ "type": "image", "data": data, "mimeType": mime }));
            }
        }
        if !images.is_empty() {
            obj.insert("image".into(), json!("attached as image content"));
        }
    }
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    let mut content = vec![json!({ "type": "text", "text": text })];
    content.extend(images);
    json!({ "content": content, "isError": is_error })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dash() -> Dash {
        // Nothing listens on port 9; these tests never reach the network.
        Dash::new("http://127.0.0.1:9").unwrap()
    }

    #[test]
    fn initialize_advertises_the_protocol_and_tools_capability() {
        let reply = handle_message(
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
            &dash(),
        )
        .unwrap();
        assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
        assert!(reply["result"]["capabilities"]["tools"].is_object());
        assert_eq!(reply["result"]["serverInfo"]["name"], "metal-mcp");
    }

    #[test]
    fn notifications_get_no_reply() {
        let note = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        assert!(handle_message(&note, &dash()).is_none());
    }

    #[test]
    fn unknown_and_retired_tools_are_refused_before_any_forwarding() {
        for name in ["no_such_tool", "ai.gen"] {
            let reply = handle_message(
                &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":{}}}),
                &dash(),
            )
            .unwrap();
            assert_eq!(reply["error"]["code"], INVALID_PARAMS, "{name}");
        }
    }

    #[test]
    fn an_unreachable_dash_is_a_tool_error_not_a_protocol_error() {
        let reply = handle_message(
            &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_nodes","arguments":{}}}),
            &dash(),
        )
        .unwrap();
        assert!(reply.get("error").is_none());
        assert_eq!(reply["result"]["isError"], true);
    }

    #[test]
    fn png_is_lifted_into_image_content() {
        let result = tool_result(json!({"ok": true, "png_base64": "AAAA", "w": 4}), false);
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["mimeType"], "image/png");
        assert_eq!(content[1]["data"], "AAAA");
        assert!(!content[0]["text"].as_str().unwrap().contains("AAAA"));
    }

    #[test]
    fn garbage_is_a_parse_error() {
        let reply: Value = serde_json::from_str(&handle_line("{nope", &dash()).unwrap()).unwrap();
        assert_eq!(reply["error"]["code"], PARSE_ERROR);
    }
}
