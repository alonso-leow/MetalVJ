//! The Anthropic adapter: `POST {url}` (`/v1/messages`) with
//! `{model, max_tokens, system, messages, tools}` and `x-api-key` +
//! `anthropic-version` headers. The system prompt rides a top-level `system`
//! string, tool calls are `tool_use` content blocks, and a tool result is a
//! `tool_result` block inside a **user** turn — none of which the
//! OpenAI-compatible wire shares. Moved from the Claude preset's stub, now
//! behind [`Provider`] and with SSE streaming.

use serde_json::{json, Value};

use crate::chat::message::{ChatMessage, ToolCallState};
use crate::chat::provider::{Delta, Provider, Request};
use crate::chat::retry::{ErrorKind, ProviderError};

use crate::config::ProviderConfig;

pub struct Anthropic {
    pub url: String,
    pub key: String,
    pub model: String,
    /// Ask for SSE deltas. Falls back to the non-stream body when the reply
    /// is not `text/event-stream`.
    pub stream: bool,
}

impl Anthropic {
    pub fn from_config(cfg: &ProviderConfig) -> Self {
        Anthropic {
            url: cfg.url.clone(),
            key: cfg.key.clone(),
            model: cfg.model.clone(),
            stream: true,
        }
    }

    fn headers(&self) -> Vec<(&str, String)> {
        vec![
            ("x-api-key", self.key.clone()),
            ("anthropic-version", "2023-06-01".to_string()),
        ]
    }

    fn body(&self, req: &Request) -> Value {
        let (system, messages) = split_system(req.messages);
        let mut body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "messages": messages,
        });
        // Anthropic carries the system prompt beside `messages`, not as a
        // role row, and wants it omitted entirely when there is none.
        if !system.is_empty() {
            body["system"] = json!(system);
        }
        if !req.tools.is_empty() {
            body["tools"] = Value::Array(req.tools.iter().map(tool_spec).collect());
        }
        if self.stream {
            body["stream"] = json!(true);
        }
        body
    }
}

impl Provider for Anthropic {
    fn send(
        &self,
        req: &Request,
        on_delta: &mut dyn FnMut(Delta),
    ) -> Result<ChatMessage, ProviderError> {
        let body = self.body(req);
        let owned: Vec<(&str, String)> = self.headers();
        let headers: Vec<(&str, &str)> =
            owned.iter().map(|(k, v)| (*k, v.as_str())).collect();

        if !self.stream {
            return self.send_blocking(&body, &headers);
        }

        let mut text = String::new();
        let mut calls: Vec<PartialCall> = Vec::new();
        let outcome = crate::http::post_stream(&self.url, &headers, &body, |data| {
            let Ok(event) = serde_json::from_str::<Value>(data) else { return };
            let Some(kind) = event["type"].as_str() else { return };
            match kind {
                "content_block_start" => {
                    let block = &event["content_block"];
                    if block["type"] == "tool_use" {
                        let index = event["index"].as_u64().unwrap_or(0) as usize;
                        let slot = PartialCall::at(&mut calls, index);
                        if let Some(id) = block["id"].as_str() {
                            slot.id = id.to_string();
                        }
                        if let Some(name) = block["name"].as_str() {
                            slot.name = name.to_string();
                        }
                        on_delta(Delta {
                            call_id: Some(slot.id.clone()),
                            name: Some(slot.name.clone()),
                            ..Delta::default()
                        });
                    }
                }
                "content_block_delta" => {
                    let delta = &event["delta"];
                    match delta["type"].as_str() {
                        Some("text_delta") => {
                            if let Some(t) = delta["text"].as_str() {
                                if !t.is_empty() {
                                    text.push_str(t);
                                    on_delta(Delta { text: t.to_string(), ..Delta::default() });
                                }
                            }
                        }
                        Some("input_json_delta") => {
                            let index = event["index"].as_u64().unwrap_or(0) as usize;
                            let slot = PartialCall::at(&mut calls, index);
                            if let Some(partial) = delta["partial_json"].as_str() {
                                slot.args.push_str(partial);
                                on_delta(Delta {
                                    call_id: Some(slot.id.clone()),
                                    args_delta: Some(partial.to_string()),
                                    ..Delta::default()
                                });
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        });

        match outcome {
            Ok(crate::http::StreamOrJson::Sse(_)) => Ok(assemble(&text, calls)),
            Ok(crate::http::StreamOrJson::Json(response)) => handle_response(response),
            Err(e) => Err(ProviderError::new(classify_transport(&e), e)),
        }
    }

    fn wire(&self) -> &'static str {
        "anthropic"
    }
}

impl Anthropic {
    fn send_blocking(
        &self,
        body: &Value,
        headers: &[(&str, &str)],
    ) -> Result<ChatMessage, ProviderError> {
        let response = crate::http::post_json(&self.url, headers, body)
            .map_err(|e| ProviderError::new(ErrorKind::Network, e))?;
        handle_response(response)
    }
}

fn handle_response(response: crate::http::HttpResponse) -> Result<ChatMessage, ProviderError> {
    if !(200..300).contains(&response.status) {
        let mut err = ProviderError::status(response.status, body_message(&response.body));
        err.retry_after = response
            .headers
            .get("retry-after")
            .and_then(|v| crate::chat::retry::parse_retry_after(v));
        return Err(err);
    }
    let value = response
        .json()
        .ok_or_else(|| ProviderError::new(ErrorKind::Other, "provider response was not JSON"))?;
    parse_step(&value)
}

struct PartialCall {
    id: String,
    name: String,
    args: String,
}

impl PartialCall {
    fn at(v: &mut Vec<PartialCall>, index: usize) -> &mut PartialCall {
        while v.len() <= index {
            v.push(PartialCall { id: String::new(), name: String::new(), args: String::new() });
        }
        &mut v[index]
    }
}

fn assemble(text: &str, calls: Vec<PartialCall>) -> ChatMessage {
    let mut message = ChatMessage::new("assistant", text);
    message.tool_calls = calls
        .into_iter()
        .enumerate()
        .filter(|(_, c)| !c.name.is_empty())
        .map(|(i, c)| {
            let id = if c.id.is_empty() { format!("call_{i}") } else { c.id };
            let args = serde_json::from_str(&c.args).unwrap_or(Value::Null);
            ToolCallState::new(id, c.name, args)
        })
        .collect();
    message
}

fn classify_transport(e: &str) -> ErrorKind {
    if e.contains("timed out") || e.contains("timeout") {
        ErrorKind::Timeout
    } else if let Some(code) = http_code(e) {
        crate::chat::retry::classify_status(code)
    } else {
        ErrorKind::Network
    }
}

fn http_code(e: &str) -> Option<u16> {
    e.strip_prefix("HTTP ")?.split([':', ' ']).next()?.parse().ok()
}

fn body_message(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "empty error body".to_string();
    }
    trimmed.chars().take(500).collect()
}

/// Split the conversation into the top-level `system` string and the
/// non-system rows. Anthropic has no `system` role: every system row's content
/// joins into one string, and the role rows go out shape-shifted by [`wire`].
fn split_system(messages: &[ChatMessage]) -> (String, Vec<Value>) {
    let mut system = Vec::new();
    let mut rows = Vec::new();
    for m in messages {
        if m.role == "system" {
            if !m.content.is_empty() {
                system.push(m.content.as_str());
            }
            continue;
        }
        if let Some(row) = wire(m) {
            rows.push(row);
        }
    }
    (system.join("\n\n"), rows)
}

/// One [`ChatMessage`] in the `/v1/messages` shape: a `tool` row becomes a
/// **user** turn carrying a `tool_result` block, an assistant row carries its
/// `tool_use` blocks, and an image row weaves `image` blocks (as base64
/// sources) in with the text block.
fn wire(m: &ChatMessage) -> Option<Value> {
    if m.role == "tool" {
        let id = m.tool_call_id.clone().unwrap_or_default();
        return Some(json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": id,
                "content": m.content,
            }],
        }));
    }

    let role = if m.role == "assistant" { "assistant" } else { "user" };
    let mut blocks: Vec<Value> = Vec::new();
    if !m.content.is_empty() {
        blocks.push(json!({ "type": "text", "text": m.content }));
    }
    for (_, img) in &m.images {
        blocks.push(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": img.mime,
                "data": crate::harness::base64_encode(&img.bytes),
            },
        }));
    }
    for call in &m.tool_calls {
        blocks.push(json!({
            "type": "tool_use",
            "id": call.id,
            "name": call.name,
            "input": call.args,
        }));
    }
    if blocks.is_empty() {
        return None;
    }
    Some(json!({ "role": role, "content": blocks }))
}

/// Map one tool spec to Anthropic's flat `{name, description, input_schema}`.
/// The runner hands OpenAI-shaped `{type:"function",function:{..}}` specs; a
/// spec already in Anthropic's shape is accepted too.
fn tool_spec(spec: &Value) -> Value {
    let function = spec.get("function");
    let name = function
        .and_then(|f| f.get("name"))
        .or_else(|| spec.get("name"))
        .cloned()
        .unwrap_or(Value::Null);
    let description = function
        .and_then(|f| f.get("description"))
        .or_else(|| spec.get("description"))
        .cloned()
        .unwrap_or(Value::Null);
    let input_schema = function
        .and_then(|f| f.get("parameters"))
        .or_else(|| spec.get("input_schema"))
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    json!({
        "name": name,
        "description": description,
        "input_schema": input_schema,
    })
}

/// Parse one non-streamed `/v1/messages` reply into a [`ChatMessage`].
pub fn parse_step(value: &Value) -> Result<ChatMessage, ProviderError> {
    if value.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
        return Err(ProviderError::new(ErrorKind::Refusal, "the model refused to comply"));
    }

    let blocks = value
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| ProviderError::new(ErrorKind::Other, "response missing content"))?;

    let mut text = String::new();
    let mut message = ChatMessage::new("assistant", "");
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    text.push_str(t);
                }
            }
            Some("tool_use") => {
                let id = block.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                let name = block.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                let input = block.get("input").cloned().unwrap_or(Value::Null);
                message.tool_calls.push(ToolCallState::new(id, name, input));
            }
            _ => {}
        }
    }
    message.content = text;
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Preset;
    use crate::images::TurnImage;

    fn provider(stream: bool) -> Anthropic {
        let mut p = Anthropic::from_config(&ProviderConfig::new(Preset::Claude, "claude-sonnet-5", "sk-x"));
        p.stream = stream;
        p
    }

    #[test]
    fn body_extracts_the_system_row_and_shapes_tool_use_and_tool_result() {
        let system = ChatMessage::new("system", "eres helpful");
        let user = ChatMessage::new("user", "rasteriza");
        let mut assistant = ChatMessage::new("assistant", "voy");
        assistant.tool_calls.push(ToolCallState::new("c1", "raster", json!({ "shader": "x" })));
        let mut tool = ChatMessage::new("tool", "{\"ok\":true}");
        tool.tool_call_id = Some("c1".into());
        let messages = vec![system, user, assistant, tool];

        let body = provider(false).body(&Request { system: None, messages: &messages, tools: &[] });

        assert_eq!(body["system"], "eres helpful");
        // The system row is gone; the rest keeps its order.
        let rows = body["messages"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["role"], "user");
        assert_eq!(rows[0]["content"][0]["type"], "text");
        assert_eq!(rows[0]["content"][0]["text"], "rasteriza");

        assert_eq!(rows[1]["role"], "assistant");
        assert_eq!(rows[1]["content"][1]["type"], "tool_use");
        assert_eq!(rows[1]["content"][1]["id"], "c1");
        assert_eq!(rows[1]["content"][1]["name"], "raster");
        assert_eq!(rows[1]["content"][1]["input"]["shader"], "x");

        // A tool row is a *user* turn with a `tool_result` block.
        assert_eq!(rows[2]["role"], "user");
        assert_eq!(rows[2]["content"][0]["type"], "tool_result");
        assert_eq!(rows[2]["content"][0]["tool_use_id"], "c1");
        assert_eq!(rows[2]["content"][0]["content"], "{\"ok\":true}");
    }

    #[test]
    fn no_system_row_omits_the_system_key() {
        let messages = vec![ChatMessage::new("user", "hola")];
        let body = provider(false).body(&Request { system: None, messages: &messages, tools: &[] });
        assert!(body.get("system").is_none());
    }

    #[test]
    fn tools_are_flattened_into_input_schema() {
        let tools = vec![json!({
            "type": "function",
            "function": {
                "name": "set_param",
                "description": "sets a param",
                "parameters": { "type": "object", "properties": { "name": { "type": "string" } } },
            },
        })];
        let messages: Vec<ChatMessage> = vec![];
        let body = provider(false).body(&Request { system: None, messages: &messages, tools: &tools });
        assert_eq!(body["tools"][0]["name"], "set_param");
        assert_eq!(body["tools"][0]["description"], "sets a param");
        assert_eq!(body["tools"][0]["input_schema"]["properties"]["name"]["type"], "string");
        assert!(body["tools"][0].get("function").is_none());
    }

    #[test]
    fn an_image_row_emits_a_base64_image_block() {
        let mut m = ChatMessage::new("user", "mira");
        m.images.push((
            "cam-1".into(),
            TurnImage { mime: "image/png".into(), bytes: b"abc".to_vec() },
        ));
        let messages = vec![m];
        let body = provider(false).body(&Request { system: None, messages: &messages, tools: &[] });
        let blocks = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[1]["source"]["type"], "base64");
        assert_eq!(blocks[1]["source"]["media_type"], "image/png");
        assert_eq!(blocks[1]["source"]["data"], crate::harness::base64_encode(b"abc"));
    }

    #[test]
    fn a_golden_response_parses_text_and_tool_use() {
        let value = json!({
            "content": [
                { "type": "text", "text": "claro" },
                { "type": "tool_use", "id": "toolu_1", "name": "get_env", "input": { "key": "x" } }
            ],
            "stop_reason": "tool_use"
        });
        let m = parse_step(&value).unwrap();
        assert_eq!(m.content, "claro");
        assert_eq!(m.tool_calls.len(), 1);
        assert_eq!(m.tool_calls[0].id, "toolu_1");
        assert_eq!(m.tool_calls[0].name, "get_env");
        assert_eq!(m.tool_calls[0].args["key"], "x");
    }

    #[test]
    fn a_refusal_stop_reason_is_a_refusal_error() {
        let value = json!({ "content": [], "stop_reason": "refusal" });
        let err = parse_step(&value).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Refusal);
    }

    #[test]
    fn assembled_partial_calls_parse_their_args() {
        let calls = vec![
            PartialCall { id: "toolu_1".into(), name: "set_param".into(), args: "{\"a\":1}".into() },
            PartialCall { id: String::new(), name: String::new(), args: "{}".into() },
            PartialCall { id: String::new(), name: "x".into(), args: String::new() },
        ];
        let m = assemble("hola", calls);
        assert_eq!(m.content, "hola");
        assert_eq!(m.tool_calls.len(), 2);
        assert_eq!(m.tool_calls[0].args["a"], 1);
        assert_eq!(m.tool_calls[1].id, "call_2");
    }
}
