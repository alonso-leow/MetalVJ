//! The runner: one turn of the session loop, replacing the body of
//! `agent_loop::run_turn`. It sends the conversation, streams events,
//! executes the model's tool calls (honouring policy, approval and cancel),
//! feeds the results back and stops when a call-free reply passes
//! [`Turn::may_finish`].
//!
//! Cooperative cancel (D12): the caller passes an [`AtomicBool`] checked
//! between provider deltas and between tool calls — the sync equivalent of
//! Continue's `cancelStream`/`cancelToolCall`.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::agent_loop::{ToolCall, ToolExecutor, Turn, lift_images, with_turn_images};
use crate::images::TurnImages;
use crate::tools::ToolRegistry;

use super::events::Event;
use super::message::{ChatMessage, ToolCallState};
use super::policy::ToolPolicy;
use super::provider::{Delta, Provider, Request};
use super::retry::ProviderError;

/// The operator's answer to an `ask` policy call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Allow,
    Deny,
}

/// How a turn ended.
pub enum RunOutcome {
    /// The model finished with a call-free message that passed the gate.
    Final(ChatMessage),
    /// The operator stopped the turn.
    Canceled,
}

/// Build the OpenAI-shaped tool specs a request carries from the registry.
fn specs(registry: &ToolRegistry) -> Vec<serde_json::Value> {
    registry.specs()
}

/// Run one full session turn. `messages` is mutated in place (system row in,
/// every assistant/tool pair the loop drove appended), so the caller can
/// persist the whole thing. `policy` answers a tool's `auto`/`ask`/`off`;
/// `approve` resolves an `ask`.
#[allow(clippy::too_many_arguments)]
pub fn run_session_turn(
    provider: &dyn Provider,
    messages: &mut Vec<ChatMessage>,
    executor: &mut dyn ToolExecutor,
    turn: &mut Turn,
    registry: &ToolRegistry,
    policy: &dyn Fn(&str) -> ToolPolicy,
    sink: &mut dyn FnMut(Event),
    cancel: &AtomicBool,
    approve: &dyn Fn(&ToolCallState) -> Approval,
    max_iterations: usize,
) -> Result<RunOutcome, ProviderError> {
    for _ in 0..max_iterations {
        if cancel.load(Ordering::Relaxed) {
            sink(Event::Canceled);
            return Ok(RunOutcome::Canceled);
        }
        let tools = specs(registry);
        let mut on_delta = |d: Delta| {
            if !d.text.is_empty() {
                sink(Event::Delta { text: d.text });
            }
            if let Some(call_id) = d.call_id {
                sink(Event::ToolCallGenerating {
                    call_id,
                    name: d.name.unwrap_or_default(),
                });
            }
        };
        let step = provider.send(
            &Request { system: None, messages, tools: &tools },
            &mut on_delta,
        )?;

        // The call-free case: ask the gate before closing, exactly as the old
        // loop did (§I.4 "not done without looking").
        if step.tool_calls.is_empty() {
            match turn.may_finish() {
                Ok(()) => {
                    sink(Event::Final { content: step.content.clone() });
                    messages.push(step.clone());
                    return Ok(RunOutcome::Final(step));
                }
                Err(why) => {
                    messages.push(step);
                    let correction = ChatMessage::new("user", why);
                    messages.push(correction);
                }
            }
            continue;
        }

        // A batch of calls: announce each, resolve policy/approval, execute.
        for call in &step.tool_calls {
            sink(Event::ToolCallReady {
                call_id: call.id.clone(),
                name: call.name.clone(),
                args: call.args.clone(),
            });
        }
        messages.push(step.clone());

        let mut shown: Vec<(String, crate::images::TurnImage)> = Vec::new();
        for call in &step.tool_calls {
            if cancel.load(Ordering::Relaxed) {
                sink(Event::Canceled);
                return Ok(RunOutcome::Canceled);
            }
            let tool_call = ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.args.clone(),
            };
            let tool_call = with_turn_images(&tool_call, &turn.images);

            match policy(&call.name) {
                ToolPolicy::Off => continue,
                ToolPolicy::Ask => {
                    sink(Event::AwaitApproval {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        args: call.args.clone(),
                    });
                    if approve(call) == Approval::Deny {
                        let denied = "denied by operator";
                        sink(Event::ToolError { call_id: call.id.clone(), error: denied.into() });
                        let mut row = ChatMessage::new(
                            "tool",
                            serde_json::json!({ "error": denied }).to_string(),
                        );
                        row.tool_call_id = Some(call.id.clone());
                        messages.push(row);
                        continue;
                    }
                }
                ToolPolicy::Auto => {}
            }

            sink(Event::ToolCalling { call_id: call.id.clone() });
            let content = match executor.execute(&tool_call) {
                Ok(v) => {
                    let (v, ids) = lift_images(v, &tool_call.name, &mut turn.images);
                    for id in ids {
                        if let Some(img) = turn.images.get(&id) {
                            shown.push((id.clone(), img.clone()));
                        }
                    }
                    sink(Event::ToolDone { call_id: call.id.clone(), output: v.to_string() });
                    if tool_call.name == "focus" {
                        let refs = focus_refs(&v);
                        if !refs.is_empty() {
                            sink(Event::Refs { refs });
                        }
                    }
                    v.to_string()
                }
                Err(e) => {
                    sink(Event::ToolError { call_id: call.id.clone(), error: e.clone() });
                    serde_json::json!({ "error": e }).to_string()
                }
            };
            let mut row = ChatMessage::new("tool", content);
            row.tool_call_id = Some(call.id.clone());
            messages.push(row);
        }

        if !shown.is_empty() {
            let names: Vec<&str> = shown.iter().map(|(id, _)| id.as_str()).collect();
            let mut row = ChatMessage::new(
                "user",
                format!(
                    "Images returned by the tools, in order: {}. To pass one to another tool \
                     (`locate`, …) use `\"image\": \"<id>\"` instead of `png_base64`.",
                    names.join(", ")
                ),
            );
            row.images = shown;
            messages.push(row);
        }
    }
    Err(ProviderError::new(
        super::retry::ErrorKind::Other,
        format!("loop did not terminate within {max_iterations} iterations"),
    ))
}

/// The refs a `focus` call returned: `{"refs":[{kind,id,graph?,label?}]}`.
/// Items without a string `kind` and `id` are skipped.
fn focus_refs(output: &serde_json::Value) -> Vec<super::message::Ref> {
    let Some(items) = output.get("refs").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    let text = |item: &serde_json::Value, key: &str| {
        item.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string)
    };
    items
        .iter()
        .filter_map(|item| {
            Some(super::message::Ref {
                kind: text(item, "kind")?,
                id: text(item, "id")?,
                graph: text(item, "graph"),
                label: text(item, "label"),
            })
        })
        .collect()
}

/// A runner with no live sink and an always-allow policy — the shim
/// `agent_loop::run_turn` uses, and a test convenience.
pub struct Noop;

impl Noop {
    pub fn always_auto(_name: &str) -> ToolPolicy {
        ToolPolicy::Auto
    }

    pub fn allow(_call: &ToolCallState) -> Approval {
        Approval::Allow
    }
}

/// Keep the `TurnImages` import used in this module's public docs compiling
/// even when the image path is not exercised.
#[allow(dead_code)]
type _Images = TurnImages;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::provider::{Delta, Provider, Request};
    use crate::chat::retry::{ErrorKind, ProviderError};
    use serde_json::json;

    fn workdir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fw-chat-runner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn assistant(text: &str) -> ChatMessage {
        ChatMessage::new("assistant", text)
    }

    fn call(id: &str, name: &str, args: serde_json::Value) -> ToolCallState {
        ToolCallState::new(id, name, args)
    }

    /// A scripted provider: each `send` returns the next step.
    struct Script {
        steps: std::cell::RefCell<Vec<ChatMessage>>,
    }

    impl Script {
        fn new(steps: Vec<ChatMessage>) -> Self {
            Script { steps: std::cell::RefCell::new(steps) }
        }
    }

    impl Provider for Script {
        fn send(
            &self,
            _req: &Request,
            _on_delta: &mut dyn FnMut(Delta),
        ) -> Result<ChatMessage, ProviderError> {
            Ok(self.steps.borrow_mut().remove(0))
        }
        fn wire(&self) -> &'static str {
            "script"
        }
    }

    struct Echo;
    impl ToolExecutor for Echo {
        fn execute(&mut self, call: &ToolCall) -> Result<serde_json::Value, String> {
            Ok(json!({ "ran": call.name }))
        }
    }

    fn events() -> std::rc::Rc<std::cell::RefCell<Vec<Event>>> {
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()))
    }

    #[test]
    fn a_call_free_reply_ends_the_turn() {
        let provider = Script::new(vec![assistant("done")]);
        let mut messages = vec![ChatMessage::new("user", "hello")];
        let mut turn = Turn::new(workdir("final"));
        let log = events();
        let sink = {
            let log = log.clone();
            move |e| log.borrow_mut().push(e)
        };
        let mut sink = sink;
        let out = run_session_turn(
            &provider,
            &mut messages,
            &mut Echo,
            &mut turn,
            &ToolRegistry::new(),
            &Noop::always_auto,
            &mut sink,
            &AtomicBool::new(false),
            &Noop::allow,
            5,
        )
        .unwrap();
        match out {
            RunOutcome::Final(m) => assert_eq!(m.content, "done"),
            _ => panic!("expected Final"),
        }
        assert!(matches!(log.borrow().last(), Some(Event::Final { .. })));
    }

    #[test]
    fn a_tool_call_runs_and_its_result_comes_back() {
        let mut first = assistant("");
        first.tool_calls.push(call("c1", "list_nodes", json!({})));
        let provider = Script::new(vec![first, assistant("done")]);
        let mut messages = vec![ChatMessage::new("user", "hello")];
        let mut turn = Turn::new(workdir("tool"));
        let mut sink = |_e: Event| {};
        let out = run_session_turn(
            &provider,
            &mut messages,
            &mut Echo,
            &mut turn,
            &ToolRegistry::new(),
            &Noop::always_auto,
            &mut sink,
            &AtomicBool::new(false),
            &Noop::allow,
            5,
        )
        .unwrap();
        match out {
            RunOutcome::Final(m) => assert_eq!(m.content, "done"),
            _ => panic!("expected Final"),
        }
        // The tool row answered the call by id.
        let tool_row = messages.iter().find(|m| m.role == "tool").unwrap();
        assert_eq!(tool_row.tool_call_id.as_deref(), Some("c1"));
        assert!(tool_row.content.contains("list_nodes"));
    }

    #[test]
    fn a_denied_call_returns_an_error_to_the_model() {
        let mut first = assistant("");
        first.tool_calls.push(call("c1", "delete_node", json!({ "id": "x" })));
        let provider = Script::new(vec![first, assistant("ok")]);
        let mut messages = vec![ChatMessage::new("user", "delete it")];
        let mut turn = Turn::new(workdir("deny"));
        let mut sink = |_e: Event| {};
        let deny = |_c: &ToolCallState| Approval::Deny;
        let ask = |_n: &str| ToolPolicy::Ask;
        run_session_turn(
            &provider,
            &mut messages,
            &mut Echo,
            &mut turn,
            &ToolRegistry::new(),
            &ask,
            &mut sink,
            &AtomicBool::new(false),
            &deny,
            5,
        )
        .unwrap();
        let tool_row = messages.iter().find(|m| m.role == "tool").unwrap();
        assert!(tool_row.content.contains("denied by operator"), "{}", tool_row.content);
    }

    #[test]
    fn an_ask_policy_emits_await_then_runs() {
        let mut first = assistant("");
        first.tool_calls.push(call("c1", "delete_node", json!({})));
        let provider = Script::new(vec![first, assistant("ok")]);
        let mut messages = vec![ChatMessage::new("user", "delete it")];
        let mut turn = Turn::new(workdir("ask"));
        let log = events();
        let mut sink = {
            let log = log.clone();
            move |e| log.borrow_mut().push(e)
        };
        let ask = |_n: &str| ToolPolicy::Ask;
        run_session_turn(
            &provider,
            &mut messages,
            &mut Echo,
            &mut turn,
            &ToolRegistry::new(),
            &ask,
            &mut sink,
            &AtomicBool::new(false),
            &Noop::allow,
            5,
        )
        .unwrap();
        assert!(log.borrow().iter().any(|e| matches!(e, Event::AwaitApproval { .. })));
        assert!(log.borrow().iter().any(|e| matches!(e, Event::ToolDone { .. })));
    }

    #[test]
    fn cancel_before_the_first_send_stops_the_turn() {
        let provider = Script::new(vec![assistant("never")]);
        let mut messages = vec![ChatMessage::new("user", "hello")];
        let mut turn = Turn::new(workdir("cancel"));
        let cancel = AtomicBool::new(true);
        let mut sink = |_e: Event| {};
        let out = run_session_turn(
            &provider,
            &mut messages,
            &mut Echo,
            &mut turn,
            &ToolRegistry::new(),
            &Noop::always_auto,
            &mut sink,
            &cancel,
            &Noop::allow,
            5,
        )
        .unwrap();
        assert!(matches!(out, RunOutcome::Canceled));
    }

    #[test]
    fn a_context_overflow_is_a_non_retryable_at_this_layer() {
        // The runner surfaces the error; the store decides to prune + retry.
        let e = ProviderError::new(ErrorKind::ContextOverflow, "too long");
        assert!(e.is_context_overflow());
        assert!(!e.retryable());
    }
}
