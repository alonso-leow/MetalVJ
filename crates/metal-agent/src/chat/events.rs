//! The events the runner emits and the store folds into the chat file.
//!
//! These are the live-update seam (D8): the worker thread runs a turn and
//! calls a `FnMut(Event)` sink at every step; `metal-dash`'s chat store is the
//! one implementation that persists them, the UI reads the file back. From
//! Continue's streamed session events, reduced to what a single-operator
//! desktop chat needs.

use crate::chat::message::ToolCallState;

#[derive(Debug, Clone)]
pub enum Event {
    /// A chunk of assistant text while streaming.
    Delta { text: String },
    /// The model has started a tool call and is streaming its arguments.
    ToolCallGenerating { call_id: String, name: String },
    /// Argument streaming finished; the call is whole.
    ToolCallReady { call_id: String, name: String, args: serde_json::Value },
    /// Policy is `ask`: the call waits for Allow/Deny.
    AwaitApproval { call_id: String, name: String, args: serde_json::Value },
    /// The executor is running the call.
    ToolCalling { call_id: String },
    /// The call returned.
    ToolDone { call_id: String, output: String },
    /// The call failed (or was denied).
    ToolError { call_id: String, error: String },
    /// A retryable provider error; the wrapper is about to wait.
    Retrying { attempt: usize, wait_ms: u64, reason: String },
    /// A `focus` call completed: the elements to attach to the live assistant row.
    Refs { refs: Vec<crate::chat::message::Ref> },
    /// The turn's final assistant text.
    Final { content: String },
    /// The turn ended in a non-retryable error.
    Error { message: String },
    /// The operator stopped the turn.
    Canceled,
}

impl Event {
    /// The `notice` row a retry/error event becomes in the transcript, if any.
    pub fn notice_row(&self) -> Option<crate::chat::message::ChatMessage> {
        match self {
            Event::Retrying { attempt, wait_ms, reason } => Some(
                crate::chat::message::ChatMessage::notice(
                    "retry",
                    format!("{reason} — reintento {attempt} en {}s", wait_ms / 1000),
                ),
            ),
            Event::Error { message } => {
                Some(crate::chat::message::ChatMessage::notice("error", message.clone()))
            }
            Event::Canceled => Some(crate::chat::message::ChatMessage::notice(
                "canceled",
                "turno detenido por el operador",
            )),
            _ => None,
        }
    }
}

/// Advance a call's status from an event, so the store can patch one row
/// without re-deriving the whole machine. Returns the new state when the
/// event names a call already in `calls`.
pub fn apply_to_call(calls: &mut [ToolCallState], event: &Event) -> bool {
    use crate::chat::message::ToolCallStatus::*;
    let (id, status) = match event {
        Event::ToolCallGenerating { call_id, .. } => (call_id, Generating),
        Event::ToolCallReady { call_id, .. } => (call_id, Generated),
        Event::AwaitApproval { call_id, .. } => (call_id, AwaitingApproval),
        Event::ToolCalling { call_id } => (call_id, Calling),
        Event::ToolDone { call_id, .. } => (call_id, Done),
        Event::ToolError { call_id, .. } => (call_id, Errored),
        _ => return false,
    };
    for call in calls.iter_mut() {
        if &call.id == id {
            call.status = status;
            match event {
                Event::ToolDone { output, .. } => call.output = Some(output.clone()),
                Event::ToolError { error, .. } => call.error = Some(error.clone()),
                _ => {}
            }
            return true;
        }
    }
    false
}
