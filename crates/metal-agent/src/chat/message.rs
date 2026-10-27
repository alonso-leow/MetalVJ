//! The provider-neutral conversation row the chat component runs on, and the
//! per-call status the UI paints a tool-call card from.
//!
//! This is `agent_loop::Message` grown a `seq`, a `status` per tool call and
//! the fields the store persists, so `data/chats/<id>.json` can carry
//! structured calls instead of the trimmed strings it used to. The old
//! [`crate::agent_loop::Message`] stays as it was; the `From` conversions
//! below are the seam the thin shim (`agent_loop::run_turn`) crosses.
//!
//! Status lifecycle (D9, from Continue's `ToolStatus`):
//! `generating → generated → (awaiting_approval) → calling → done | errored | canceled`.

use serde_json::{json, Value};

use crate::images::TurnImage;

/// Where a tool call is in its life. Ordered as the lifecycle above so a
/// card renders one chip and the runner advances one step at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallStatus {
    /// The model is still streaming the arguments.
    Generating,
    /// Arguments are complete, nothing has run yet.
    Generated,
    /// Policy is `ask`; waiting on the operator's Allow/Deny.
    AwaitingApproval,
    /// The executor is running it right now.
    Calling,
    /// Ran and returned a result.
    Done,
    /// Ran and failed; `error` carries the reason.
    Errored,
    /// The operator stopped the turn before this call ran.
    Canceled,
}

impl ToolCallStatus {
    pub fn wire(self) -> &'static str {
        match self {
            ToolCallStatus::Generating => "generating",
            ToolCallStatus::Generated => "generated",
            ToolCallStatus::AwaitingApproval => "awaiting_approval",
            ToolCallStatus::Calling => "calling",
            ToolCallStatus::Done => "done",
            ToolCallStatus::Errored => "errored",
            ToolCallStatus::Canceled => "canceled",
        }
    }

    pub fn parse(s: &str) -> Option<ToolCallStatus> {
        Some(match s {
            "generating" => ToolCallStatus::Generating,
            "generated" => ToolCallStatus::Generated,
            "awaiting_approval" | "awaiting" => ToolCallStatus::AwaitingApproval,
            "calling" => ToolCallStatus::Calling,
            "done" => ToolCallStatus::Done,
            "errored" | "error" => ToolCallStatus::Errored,
            "canceled" | "cancelled" => ToolCallStatus::Canceled,
            _ => return None,
        })
    }
}

/// One tool call the assistant asked for: what to run, and where it got to.
#[derive(Debug, Clone)]
pub struct ToolCallState {
    pub id: String,
    pub name: String,
    pub args: Value,
    pub status: ToolCallStatus,
    /// The result body once [`ToolCallStatus::Done`] (trimmed for persistence).
    pub output: Option<String>,
    /// The failure reason once [`ToolCallStatus::Errored`].
    pub error: Option<String>,
}

impl ToolCallState {
    pub fn new(id: impl Into<String>, name: impl Into<String>, args: Value) -> Self {
        ToolCallState {
            id: id.into(),
            name: name.into(),
            args,
            status: ToolCallStatus::Generated,
            output: None,
            error: None,
        }
    }

    /// The OpenAI `tool_calls[]` shape a request carries for this call, plus
    /// the shape the store persists.
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "args": self.args,
            "status": self.status.wire(),
            "output": self.output,
            "error": self.error,
        })
    }

    pub fn from_json(v: &Value) -> Option<ToolCallState> {
        Some(ToolCallState {
            id: v["id"].as_str()?.to_string(),
            name: v["name"].as_str()?.to_string(),
            args: v.get("args").cloned().unwrap_or(Value::Null),
            status: v["status"].as_str().and_then(ToolCallStatus::parse).unwrap_or(ToolCallStatus::Done),
            output: v.get("output").and_then(Value::as_str).map(str::to_string),
            error: v.get("error").and_then(Value::as_str).map(str::to_string),
        })
    }
}

/// A focusable element the assistant "sends" (a node, a graph, …): a chip the
/// transcript shows and the user can click. `graph` names the graph a node
/// lives in; `label` is the text the chip shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ref {
    pub kind: String,
    pub id: String,
    pub graph: Option<String>,
    pub label: Option<String>,
}

impl Ref {
    /// `{kind,id,graph?,label?}` — the optional keys only when set.
    pub fn to_json(&self) -> Value {
        let mut v = json!({ "kind": self.kind, "id": self.id });
        if let Some(g) = &self.graph {
            v["graph"] = json!(g);
        }
        if let Some(l) = &self.label {
            v["label"] = json!(l);
        }
        v
    }

    pub fn from_json(v: &Value) -> Option<Ref> {
        Some(Ref {
            kind: v["kind"].as_str()?.to_string(),
            id: v["id"].as_str()?.to_string(),
            graph: v.get("graph").and_then(Value::as_str).map(str::to_string),
            label: v.get("label").and_then(Value::as_str).map(str::to_string),
        })
    }
}

/// One row of a conversation. `role` is one of `system`/`user`/`assistant`/
/// `tool`/`notice`; `notice` rows (retry banners) never ride the wire.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    /// Monotonic within a chat — the `?since=` cursor and the live-row patch
    /// key. `0` until the store assigns one.
    pub seq: u64,
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<ToolCallState>,
    pub tool_call_id: Option<String>,
    pub images: Vec<(String, TurnImage)>,
    /// Unix seconds, stamped by the store.
    pub at: u64,
    /// A `notice` row's kind (`retry`, `error`, …); otherwise null.
    pub kind: Option<String>,
    /// Focusable elements the assistant sent with this row (see [`Ref`]).
    pub refs: Vec<Ref>,
}

impl Default for ChatMessage {
    fn default() -> Self {
        ChatMessage {
            seq: 0,
            role: String::new(),
            content: String::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            images: Vec::new(),
            at: 0,
            kind: None,
            refs: Vec::new(),
        }
    }
}

impl ChatMessage {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        ChatMessage { role: role.into(), content: content.into(), ..ChatMessage::default() }
    }

    /// A `notice` row: the retry/error banners the transcript shows inline.
    pub fn notice(kind: impl Into<String>, content: impl Into<String>) -> Self {
        ChatMessage {
            kind: Some(kind.into()),
            ..ChatMessage::new("notice", content)
        }
    }

    pub fn is_wire(&self) -> bool {
        matches!(self.role.as_str(), "system" | "user" | "assistant" | "tool")
    }

    /// The persisted shape (`data/chats/<id>.json`). Fields that are empty are
    /// dropped so an old reader sees the same small rows it always did.
    pub fn to_json(&self) -> Value {
        let mut row = json!({ "seq": self.seq, "role": self.role, "content": self.content, "at": self.at });
        if !self.tool_calls.is_empty() {
            row["tool_calls"] = Value::Array(self.tool_calls.iter().map(|c| c.to_json()).collect());
        }
        if let Some(id) = &self.tool_call_id {
            row["tool_call_id"] = json!(id);
        }
        if let Some(kind) = &self.kind {
            row["kind"] = json!(kind);
        }
        if !self.refs.is_empty() {
            row["refs"] = Value::Array(self.refs.iter().map(Ref::to_json).collect());
        }
        row
    }

    /// Backward-compatible load: an old row lacks `seq`/`tool_calls`/`kind`
    /// and still parses (the plan's data-shape promise).
    pub fn from_json(v: &Value) -> Option<ChatMessage> {
        let role = v["role"].as_str()?.to_string();
        let tool_calls = v
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(ToolCallState::from_json).collect())
            .unwrap_or_default();
        Some(ChatMessage {
            seq: v["seq"].as_u64().unwrap_or(0),
            role,
            content: v["content"].as_str().unwrap_or_default().to_string(),
            tool_calls,
            tool_call_id: v["tool_call_id"].as_str().map(str::to_string),
            images: Vec::new(),
            at: v["at"].as_u64().unwrap_or(0),
            kind: v["kind"].as_str().map(str::to_string),
            refs: v
                .get("refs")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Ref::from_json).collect())
                .unwrap_or_default(),
        })
    }
}

impl From<crate::agent_loop::Message> for ChatMessage {
    fn from(m: crate::agent_loop::Message) -> Self {
        ChatMessage {
            seq: 0,
            role: m.role,
            content: m.content,
            tool_calls: m
                .tool_calls
                .into_iter()
                .map(|c| ToolCallState::new(c.id, c.name, c.arguments))
                .collect(),
            tool_call_id: m.tool_call_id,
            images: m.images,
            at: 0,
            kind: None,
            refs: Vec::new(),
        }
    }
}

impl From<ChatMessage> for crate::agent_loop::Message {
    fn from(m: ChatMessage) -> Self {
        crate::agent_loop::Message {
            role: m.role,
            content: m.content,
            tool_calls: m
                .tool_calls
                .into_iter()
                .map(|c| crate::agent_loop::ToolCall {
                    id: c.id,
                    name: c.name,
                    arguments: c.args,
                })
                .collect(),
            tool_call_id: m.tool_call_id,
            images: m.images,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips_on_the_wire() {
        for s in [
            ToolCallStatus::Generating,
            ToolCallStatus::Generated,
            ToolCallStatus::AwaitingApproval,
            ToolCallStatus::Calling,
            ToolCallStatus::Done,
            ToolCallStatus::Errored,
            ToolCallStatus::Canceled,
        ] {
            assert_eq!(ToolCallStatus::parse(s.wire()), Some(s));
        }
    }

    #[test]
    fn an_old_string_row_still_loads() {
        let row = json!({ "role": "tool", "content": "{\"ok\":true}", "at": 5 });
        let m = ChatMessage::from_json(&row).unwrap();
        assert_eq!(m.role, "tool");
        assert_eq!(m.seq, 0);
        assert!(m.tool_calls.is_empty());
    }

    #[test]
    fn a_structured_row_round_trips() {
        let mut m = ChatMessage::new("assistant", "");
        let mut call = ToolCallState::new("c1", "set_param", json!({ "name": "x" }));
        call.status = ToolCallStatus::Done;
        call.output = Some("ok".into());
        m.tool_calls.push(call);
        let back = ChatMessage::from_json(&m.to_json()).unwrap();
        assert_eq!(back.tool_calls[0].name, "set_param");
        assert_eq!(back.tool_calls[0].status, ToolCallStatus::Done);
        assert_eq!(back.tool_calls[0].output.as_deref(), Some("ok"));
    }

    #[test]
    fn conversions_preserve_tool_calls_and_images() {
        let mut old = crate::agent_loop::Message::new("assistant", "");
        old.tool_calls.push(crate::agent_loop::ToolCall {
            id: "c1".into(),
            name: "raster".into(),
            arguments: json!({}),
        });
        let new: ChatMessage = old.into();
        assert_eq!(new.tool_calls.len(), 1);
        let back: crate::agent_loop::Message = new.into();
        assert_eq!(back.tool_calls[0].name, "raster");
    }
}
