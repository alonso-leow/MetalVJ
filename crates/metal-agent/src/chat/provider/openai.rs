//! The OpenAI-compatible adapter: `POST {url}` with `{model, messages, tools}`
//! and `Authorization: Bearer`. Covers OpenAI, DeepInfra and any `Manual`
//! endpoint that speaks the chat-completions dialect. Moved from
//! `agent_loop.rs`'s `wire`/`parse_step`, now behind [`Provider`] and with SSE
//! streaming.

use serde_json::{json, Value};

use crate::chat::message::{ChatMessage, ToolCallState};
use crate::chat::provider::{Delta, Provider, Request};
use crate::chat::retry::{ErrorKind, ProviderError};

use crate::config::ProviderConfig;

pub struct OpenAiCompat {
    pub url: String,
    pub key: String,
    pub model: String,
    /// Ask for SSE deltas. Falls back to the non-stream body when the reply
    /// is not `text/event-stream`.
    pub stream: bool,
}

impl OpenAiCompat {
    pub fn from_config(cfg: &ProviderConfig) -> Self {
        OpenAiCompat {
            url: cfg.url.clone(),
            key: cfg.key.clone(),
            model: cfg.model.clone(),
            stream: true,
        }
    }

    fn headers(&self) -> Vec<(&str, String)> {
        vec![("Authorization", format!("Bearer {}", self.key))]
    }

    fn body(&self, req: &Request) -> Value {
        let mut body = json!({
            "model": self.model,
            "messages": req.messages.iter().map(wire).collect::<Vec<_>>(),
        });
        if self.stream {
            body["stream"] = json!(true);
        }
        if !req.tools.is_empty() {
            body["tools"] = Value::Array(req.tools.to_vec());
        }
        body
    }
}

impl Provider for OpenAiCompat {
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
            if data == "[DONE]" {
                return;
            }
            let Ok(chunk) = serde_json::from_str::<Value>(data) else { return };
            let Some(delta) = chunk["choices"].get(0).and_then(|c| c.get("delta")) else {
                return;
            };
            if let Some(t) = delta["content"].as_str() {
                if !t.is_empty() {
                    text.push_str(t);
                    on_delta(Delta { text: t.to_string(), ..Delta::default() });
                }
            }
            if let Some(tool_calls) = delta["tool_calls"].as_array() {
                for tc in tool_calls {
                    let index = tc["index"].as_u64().unwrap_or(0) as usize;
                    let slot = PartialCall::at(&mut calls, index);
                    if let Some(id) = tc["id"].as_str() {
                        slot.id = id.to_string();
                    }
                    if let Some(name) = tc["function"]["name"].as_str() {
                        slot.name = name.to_string();
                    }
                    if let Some(args) = tc["function"]["arguments"].as_str() {
                        slot.args.push_str(args);
                    }
                    on_delta(Delta {
                        call_id: Some(slot.id.clone()),
                        name: Some(slot.name.clone()),
                        args_delta: tc["function"]["arguments"].as_str().map(str::to_string),
                        ..Delta::default()
                    });
                }
            }
        });

        match outcome {
            Ok(crate::http::StreamOrJson::Sse(_)) => Ok(assemble(&text, calls)),
            Ok(crate::http::StreamOrJson::Json(response)) => handle_response(response),
            Err(e) => Err(ProviderError::new(classify_transport(&e), e)),
        }
    }

    fn wire(&self) -> &'static str {
        "openai"
    }
}

impl OpenAiCompat {
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

/// Parse a non-streamed reply, mapping a non-2xx to a classified error (with
/// `Retry-After` when the provider sent one).
fn handle_response(
    response: crate::http::HttpResponse,
) -> Result<ChatMessage, ProviderError> {
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

/// One [`ChatMessage`] in the chat-completions shape: an assistant row carries
/// its `tool_calls`, a tool row its `tool_call_id`, and a row with images
/// becomes a content array of one text part and one `image_url` part per
/// image, as a data URL.
pub fn wire(m: &ChatMessage) -> Value {
    let mut row = json!({ "role": m.role, "content": m.content });
    if !m.tool_calls.is_empty() {
        row["tool_calls"] = m
            .tool_calls
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "type": "function",
                    "function": { "name": c.name, "arguments": c.args.to_string() },
                })
            })
            .collect();
        if m.content.is_empty() {
            row["content"] = Value::Null;
        }
    }
    if let Some(id) = &m.tool_call_id {
        row["tool_call_id"] = json!(id);
    }
    if !m.images.is_empty() {
        let mut parts = vec![json!({ "type": "text", "text": m.content })];
        for (_, img) in &m.images {
            parts.push(json!({
                "type": "image_url",
                "image_url": {
                    "url": format!("data:{};base64,{}", img.mime, crate::harness::base64_encode(&img.bytes)),
                },
            }));
        }
        row["content"] = Value::Array(parts);
    }
    row
}

/// Parse one non-streamed reply into a [`ChatMessage`].
pub fn parse_step(value: &Value) -> Result<ChatMessage, ProviderError> {
    let choice = value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .ok_or_else(|| {
            ProviderError::new(ErrorKind::Other, "response missing choices[0].message")
        })?;

    let content = choice.get("content").and_then(Value::as_str).unwrap_or("").to_string();
    let role = choice.get("role").and_then(Value::as_str).unwrap_or("assistant").to_string();
    let mut message = ChatMessage::new(role, content);

    if let Some(calls) = choice.get("tool_calls").and_then(Value::as_array) {
        message.tool_calls = calls
            .iter()
            .filter_map(|c| {
                let id = c.get("id")?.as_str()?.to_string();
                let function = c.get("function")?;
                let name = function.get("name")?.as_str()?.to_string();
                let args = function
                    .get("arguments")
                    .and_then(Value::as_str)
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(Value::Null);
                Some(ToolCallState::new(id, name, args))
            })
            .collect();
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_only_response_terminates() {
        let value = json!({ "choices": [{ "message": { "role": "assistant", "content": "hola" } }] });
        let m = parse_step(&value).unwrap();
        assert_eq!(m.content, "hola");
        assert!(m.tool_calls.is_empty());
    }

    #[test]
    fn tool_calls_are_parsed_out_of_the_response() {
        let value = json!({
            "choices": [{ "message": {
                "role": "assistant", "content": "",
                "tool_calls": [{ "id": "call_1", "function": { "name": "get_env", "arguments": "{}" } }]
            }}]
        });
        let m = parse_step(&value).unwrap();
        assert_eq!(m.tool_calls.len(), 1);
        assert_eq!(m.tool_calls[0].name, "get_env");
    }

    #[test]
    fn the_wire_shape_names_calls_and_answers_by_id() {
        let mut a = ChatMessage::new("assistant", "");
        a.tool_calls.push(ToolCallState::new("c1", "raster", json!({ "shader": "x" })));
        let w = wire(&a);
        assert_eq!(w["tool_calls"][0]["id"], "c1");
        assert_eq!(w["tool_calls"][0]["function"]["name"], "raster");
        assert_eq!(w["content"], Value::Null);

        let mut t = ChatMessage::new("tool", "{}");
        t.tool_call_id = Some("c1".into());
        assert_eq!(wire(&t)["tool_call_id"], "c1");
    }

    #[test]
    fn assembled_partial_calls_parse_their_args() {
        let calls = vec![
            PartialCall { id: "c1".into(), name: "set_param".into(), args: "{\"a\":1}".into() },
            PartialCall { id: String::new(), name: "x".into(), args: String::new() },
        ];
        let m = assemble("hola", calls);
        assert_eq!(m.content, "hola");
        assert_eq!(m.tool_calls[0].args["a"], 1);
        assert_eq!(m.tool_calls[1].id, "call_1");
    }
}
