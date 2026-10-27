//! `S.X` (first half): the real chat layer for the AI sidebar —
//! conversations on the same filesystem everything else lives on, now backed
//! by [`metal_agent::chat`] (the provider adapters, retries, policy and the
//! session runner) instead of the old single-dialect loop.
//!
//! **One file per conversation, no second state.** `data/chats/<id>.json`
//! carries `{id, title, status, updated, seq, messages}`, where `messages`
//! is the structured transcript the plan's data shape describes: every row
//! has a `seq`, an assistant row carries its `tool_calls[]` with a `status`
//! each, and a `tool` row names its `tool_call_id`. `seq` lets the UI poll
//! `?since=` and lets the worker patch the live assistant row while it
//! streams.
//!
//! **The worker is an event sink.** `POST /ai/ask` appends the prompt and
//! flips `status` to `busy`, then spawns a thread that runs
//! [`metal_agent::chat::runner::run_session_turn`]. Every [`Event`] the runner
//! emits is written to the file incrementally, so the transcript fills while
//! the turn runs. The tools themselves are untouched (`TOOL_CATALOG` +
//! `Harness`); this module only decides which ride (per-tool policy) and
//! records what happened.
//!
//! **Cancel and approval** are process-global registries (a `OnceLock` each):
//! a per-chat `AtomicBool` the runner checks between deltas and calls, and a
//! per-call `Sender<bool>` the `POST /ai/chat/approve` route resolves. A call
//! whose answer never comes times out to a deny.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use metal_agent::chat::message::{ChatMessage, ToolCallState};
use metal_agent::chat::policy::ToolPolicy;
use metal_agent::chat::runner::{Approval, RunOutcome};
use metal_agent::chat::{Event, history};
use metal_agent::harness::TOOL_CATALOG;
use serde_json::{json, Value};

/// The loop's own safety valve in steps.
const MAX_ITERATIONS: usize = 6;

/// Tool result bodies longer than this lose their tail in the persisted
/// chat — with a visible note, never silently.
const TOOL_RESULT_CAP: usize = 1600;

/// How long an `ask`-policy call waits for the operator before it is denied.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

/// Most context refs one prompt may carry; the rest are dropped.
pub const MAX_REFS: usize = 8;

/// The chat list's `preview`: the last user prompt's first characters.
const PREVIEW_CHARS: usize = 80;

/// A new chat's auto-title: the first prompt's first characters.
const TITLE_CHARS: usize = 40;

// ---------------------------------------------------------------------------
// Cancel + approval registries
// ---------------------------------------------------------------------------

fn cancel_registry() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static R: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn approval_registry() -> &'static Mutex<HashMap<String, Sender<bool>>> {
    static R: OnceLock<Mutex<HashMap<String, Sender<bool>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Stop a chat's running turn: set its flag; the runner notices between
/// deltas and tool calls. Answers whether a turn was actually running.
pub fn cancel_chat(id: &str) -> bool {
    let reg = cancel_registry().lock().unwrap();
    if let Some(flag) = reg.get(id) {
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Answer a pending `ask`-policy call. `false` when nothing was waiting.
pub fn approve_call(call_id: &str, allow: bool) -> bool {
    let mut reg = approval_registry().lock().unwrap();
    match reg.remove(call_id) {
        Some(tx) => tx.send(allow).is_ok(),
        None => false,
    }
}

/// The policy a tool's call should obey, from the saved per-tool table with
/// legacy booleans folded in.
fn tool_policy(saved: &Value, name: &str) -> ToolPolicy {
    metal_agent::harness::effective_policy(name, saved)
}

// ---------------------------------------------------------------------------
// The chat files
// ---------------------------------------------------------------------------

/// The per-process scratch chats dir for test builds (see [`chats_dir`]).
static SCRATCH: OnceLock<std::path::PathBuf> = OnceLock::new();

/// `<root>/chats`, except in test builds, where the real data root's chats are
/// a per-process scratch dir: a test that dispatches `POST /ai/chats` (the
/// `every_route_dispatches` sweep) must never write the operator's live
/// conversations. Same guard shape as `metal_perform::engine::graphs_dir`.
fn chats_dir(root: &std::path::Path) -> std::path::PathBuf {
    let real = root.join("chats");
    if !cfg!(test) || root != crate::output::data_root() {
        return real;
    }
    SCRATCH
        .get_or_init(|| {
            let dir = std::env::temp_dir()
                .join(format!("metal-dash-test-chats-{}", std::process::id()));
            metal_vfs::create_dir_all(&dir).expect("test chats scratch dir");
            dir
        })
        .clone()
}

/// Chat ids are filenames: only `[0-9a-z-]` is a chat id.
fn find_chat_path(root: &std::path::Path, id: &str) -> Option<std::path::PathBuf> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return None;
    }
    let path = chats_dir(root).join(format!("{id}.json"));
    metal_vfs::is_file(&path).then_some(path)
}

/// A fresh conversation: `id` is nanos-since-epoch.
pub fn create_chat_at(root: &std::path::Path) -> Result<Value, String> {
    let id = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_else(|_| "0".to_string());
    let chat = json!({
        "id": id,
        "title": "new chat",
        "status": "idle",
        "updated": 0,
        "seq": 0,
        "messages": [],
    });
    save_chat_at(root, &chat)?;
    Ok(chat)
}

pub fn load_chat_at(root: &std::path::Path, id: &str) -> Option<Value> {
    metal_vfs::read_to_string(find_chat_path(root, id)?)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// `GET /ai/chats/<id>?since=<n>`: the chat with only the rows whose `seq`
/// is greater than `since` (all rows when `since` is absent). The top-level
/// `seq` is always the whole chat's latest, so the caller's next cursor is
/// the same value either way.
pub fn load_chat_since(root: &std::path::Path, id: &str, since: u64) -> Value {
    match load_chat_at(root, id) {
        Some(mut chat) => {
            if since > 0 {
                if let Some(msgs) = chat["messages"].as_array_mut() {
                    msgs.retain(|m| m["seq"].as_u64().unwrap_or(0) > since);
                }
            }
            chat
        }
        None => json!({ "id": id, "status": "idle", "seq": 0, "messages": [] }),
    }
}

fn save_chat_at(root: &std::path::Path, chat: &Value) -> Result<(), String> {
    let dir = chats_dir(root);
    metal_vfs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.json", chat["id"].as_str().unwrap_or("bad-id")));
    crate::atomic::write(
        &path,
        serde_json::to_string_pretty(chat).unwrap_or_default(),
    )
    .map_err(|e| e.to_string())
}

pub fn list_chats_at(root: &std::path::Path) -> Vec<Value> {
    let mut chats: Vec<Value> = chats_dir(root)
        .read_dir()
        .map(|entries| {
            entries
                .filter_map(|entry| {
                    let path = entry.ok()?.path();
                    if path.extension()?.ne("json") {
                        return None;
                    }
                    metal_vfs::read_to_string(&path).ok()?.parse::<Value>().ok()
                })
                .collect()
        })
        .unwrap_or_default();
    for chat in chats.iter_mut() {
        let messages = chat["messages"].as_array().cloned().unwrap_or_default();
        let count = messages
            .iter()
            .filter(|m| matches!(m["role"].as_str(), Some("user" | "assistant")))
            .count();
        let preview = messages
            .iter()
            .rev()
            .find(|m| m["role"] == "user")
            .and_then(|m| m["content"].as_str())
            .map(|s| s.chars().take(PREVIEW_CHARS).collect::<String>())
            .unwrap_or_default();
        chat["count"] = json!(count);
        chat["preview"] = json!(preview);
    }
    chats.sort_by_key(|c| std::cmp::Reverse(c["updated"].as_u64().unwrap_or(0)));
    chats
}

/// Append one plain message row (used by `ask_at` for the prompt) and stamp
/// `updated`. Structured rows are written by [`Store`].
pub fn append_message_at(
    root: &std::path::Path,
    chat_id: &str,
    role: &str,
    content: &str,
) -> Result<(), String> {
    append_row_at(root, chat_id, json!({ "role": role, "content": content }))
}

/// Append one row (any extra keys, e.g. `refs`) with the next `seq` and an
/// `at`, and stamp the chat's `updated`.
fn append_row_at(root: &std::path::Path, chat_id: &str, mut row: Value) -> Result<(), String> {
    let Some(mut chat) = load_chat_at(root, chat_id) else {
        return Err(format!("chat {chat_id} no longer exists"));
    };
    let seq = chat["seq"].as_u64().unwrap_or(0) + 1;
    let at = now_secs();
    chat["seq"] = json!(seq);
    chat["updated"] = json!(at);
    row["seq"] = json!(seq);
    row["at"] = json!(at);
    chat["messages"]
        .as_array_mut()
        .ok_or("chat has no messages")?
        .push(row);
    save_chat_at(root, &chat)
}

/// Set a stored row's `refs` (by `seq`), leaving every other key of the row
/// as it is. An empty list removes the key.
pub fn set_row_refs_at(
    root: &std::path::Path,
    chat_id: &str,
    seq: u64,
    refs: &[Value],
) -> Result<(), String> {
    let Some(mut chat) = load_chat_at(root, chat_id) else {
        return Err(format!("chat {chat_id} no longer exists"));
    };
    let row = chat["messages"]
        .as_array_mut()
        .and_then(|rows| rows.iter_mut().find(|r| r["seq"].as_u64() == Some(seq)))
        .ok_or_else(|| format!("row {seq} not in chat {chat_id}"))?;
    if refs.is_empty() {
        row.as_object_mut().map(|o| o.remove("refs"));
    } else {
        row["refs"] = json!(refs);
    }
    save_chat_at(root, &chat)
}

/// Keep at most [`MAX_REFS`] well-formed context refs. A ref needs string
/// `kind` and `id`; `graph` and `label` are optional strings. Anything else
/// is dropped silently.
pub fn parse_refs(v: &Value) -> Vec<Value> {
    let Some(items) = v.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let kind = item.get("kind")?.as_str().filter(|s| !s.is_empty())?;
            let id = item.get("id")?.as_str().filter(|s| !s.is_empty())?;
            let mut out = json!({ "kind": kind, "id": id });
            match item.get("graph") {
                None | Some(Value::Null) => {}
                Some(Value::String(g)) => out["graph"] = json!(g),
                Some(_) => return None,
            }
            let label = match item.get("label") {
                Some(Value::String(l)) => l.as_str(),
                _ => "",
            };
            out["label"] = json!(label);
            Some(out)
        })
        .take(MAX_REFS)
        .collect()
}

/// The compact `[context]` block sent to the model ahead of the prompt, one
/// line per ref. Empty when there are no refs.
pub fn context_block(refs: &[Value]) -> String {
    if refs.is_empty() {
        return String::new();
    }
    let mut out = String::from("[context]\n");
    for r in refs {
        let kind = r["kind"].as_str().unwrap_or("");
        let id = r["id"].as_str().unwrap_or("");
        let target = match r["graph"].as_str().filter(|g| !g.is_empty()) {
            Some(g) => format!("{g}/{id}"),
            None => id.to_string(),
        };
        let label = r["label"].as_str().unwrap_or("");
        if label.is_empty() {
            out.push_str(&format!("{kind} {target}\n"));
        } else {
            out.push_str(&format!("{kind} {target} ({label})\n"));
        }
    }
    out.push_str("[/context]\n");
    out
}

/// A conversation's title from its first prompt: trimmed, first
/// [`TITLE_CHARS`] characters.
pub fn title_from_prompt(prompt: &str) -> String {
    prompt.trim().chars().take(TITLE_CHARS).collect::<String>().trim().to_string()
}

pub fn set_status_at(root: &std::path::Path, chat_id: &str, status: &str) -> Result<(), String> {
    let Some(mut chat) = load_chat_at(root, chat_id) else {
        return Err(format!("chat {chat_id} no longer exists"));
    };
    chat["status"] = json!(status);
    save_chat_at(root, &chat)
}

pub fn delete_chat_at(root: &std::path::Path, id: &str) -> Result<Value, String> {
    if !(id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
    {
        return Err("bad chat id".to_string());
    }
    let path = chats_dir(root).join(format!("{id}.json"));
    if !metal_vfs::is_file(&path) {
        return Err(format!("chat {id} does not exist"));
    }
    metal_vfs::remove_file(&path).map_err(|e| e.to_string())?;
    Ok(json!({ "deleted": id }))
}

fn now_secs() -> u64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// The store: Event -> incremental file writes
// ---------------------------------------------------------------------------

/// The live writer for one turn. It owns a working copy of the chat `Value`
/// and rewrites the file after every event, so the UI's poll sees the turn
/// grow. The `seq` counter is the file's own.
struct Store<'a> {
    root: &'a std::path::Path,
    chat: Value,
    /// The assistant row currently streaming, if any.
    live: Option<u64>,
    /// True once the live assistant step's tool results have been appended,
    /// so the next text/call opens a fresh assistant row.
    step_closed: bool,
}

impl<'a> Store<'a> {
    fn open(root: &'a std::path::Path, chat_id: &str) -> Result<Store<'a>, String> {
        let chat = load_chat_at(root, chat_id)
            .ok_or_else(|| format!("chat {chat_id} no longer exists"))?;
        Ok(Store { root, chat, live: None, step_closed: false })
    }

    fn next_seq(&mut self) -> u64 {
        let seq = self.chat["seq"].as_u64().unwrap_or(0) + 1;
        self.chat["seq"] = json!(seq);
        seq
    }

    fn rows_mut(&mut self) -> &mut Vec<Value> {
        self.chat["messages"]
            .as_array_mut()
            .expect("a chat always has a messages array")
    }

    fn push_row(&mut self, mut row: Value) -> u64 {
        let seq = self.next_seq();
        let at = now_secs();
        row["seq"] = json!(seq);
        row["at"] = json!(at);
        self.rows_mut().push(row);
        seq
    }

    fn row_mut(&mut self, seq: u64) -> Option<&mut Value> {
        self.rows_mut().iter_mut().find(|r| r["seq"].as_u64() == Some(seq))
    }

    fn set_status(&mut self, status: &str) {
        self.chat["status"] = json!(status);
    }

    fn save(&mut self) {
        self.chat["updated"] = json!(now_secs());
        let _ = save_chat_at(self.root, &self.chat);
    }

    /// Make sure there is a live assistant row to write into.
    fn open_assistant(&mut self) -> u64 {
        if self.live.is_none() || self.step_closed {
            let seq = self.push_row(json!({ "role": "assistant", "content": "" }));
            self.live = Some(seq);
            self.step_closed = false;
        }
        self.live.unwrap()
    }

    /// Insert or patch one tool call on the live assistant row.
    fn upsert_call(&mut self, call_id: &str, name: &str, args: Option<Value>, status: &str) {
        let seq = self.open_assistant();
        let Some(row) = self.row_mut(seq) else { return };
        if !row["tool_calls"].is_array() {
            row["tool_calls"] = json!([]);
        }
        let calls = row["tool_calls"].as_array_mut().unwrap();
        let slot = match calls.iter_mut().find(|c| c["id"] == json!(call_id)) {
            Some(slot) => slot,
            None => {
                calls.push(json!({
                    "id": call_id, "name": name, "args": args.clone().unwrap_or(Value::Null),
                    "status": status, "output": Value::Null, "error": Value::Null,
                }));
                return;
            }
        };
        slot["status"] = json!(status);
        if let Some(a) = args {
            slot["args"] = a;
        }
    }

    fn set_call_terminal(&mut self, call_id: &str, status: &str, output: Option<&str>, error: Option<&str>) {
        if let Some(seq) = self.live {
            if let Some(row) = self.row_mut(seq) {
                if let Some(calls) = row["tool_calls"].as_array_mut() {
                    if let Some(slot) = calls.iter_mut().find(|c| c["id"] == json!(call_id)) {
                        slot["status"] = json!(status);
                        if let Some(o) = output {
                            slot["output"] = json!(o);
                        }
                        if let Some(e) = error {
                            slot["error"] = json!(e);
                        }
                    }
                }
            }
        }
    }

    fn on_event(&mut self, event: &Event) {
        match event {
            Event::Delta { text } => {
                let seq = self.open_assistant();
                if let Some(row) = self.row_mut(seq) {
                    let content = row["content"].as_str().unwrap_or("").to_string();
                    row["content"] = json!(format!("{content}{text}"));
                }
            }
            Event::ToolCallGenerating { call_id, name } => {
                self.upsert_call(call_id, name, None, "generating");
            }
            Event::ToolCallReady { call_id, name, args } => {
                self.upsert_call(call_id, name, Some(args.clone()), "generated");
            }
            Event::AwaitApproval { call_id, name, args } => {
                self.upsert_call(call_id, name, Some(args.clone()), "awaiting_approval");
                self.set_status("awaiting");
            }
            Event::ToolCalling { call_id } => {
                self.upsert_call(call_id, "", None, "calling");
                self.set_status("busy");
            }
            Event::ToolDone { call_id, output } => {
                let trimmed = trim_tool_result(output.clone());
                self.set_call_terminal(call_id, "done", Some(&trimmed), None);
                self.push_row(json!({
                    "role": "tool", "tool_call_id": call_id, "content": trimmed,
                }));
                self.step_closed = true;
            }
            Event::ToolError { call_id, error } => {
                self.set_call_terminal(call_id, "errored", None, Some(error));
                self.push_row(json!({
                    "role": "tool", "tool_call_id": call_id,
                    "content": json!({ "error": error }).to_string(),
                }));
                self.step_closed = true;
            }
            Event::Retrying { attempt, wait_ms, reason } => {
                self.push_row(json!({
                    "role": "notice", "kind": "retry",
                    "content": format!("{reason} — retry {attempt} in {}s", wait_ms / 1000),
                }));
            }
            Event::Final { content } => {
                let seq = self.open_assistant();
                if let Some(row) = self.row_mut(seq) {
                    row["content"] = json!(content);
                }
                self.step_closed = true;
            }
            Event::Refs { refs } => {
                let seq = self.open_assistant();
                let mut merged: Vec<Value> = self
                    .row_mut(seq)
                    .and_then(|row| row["refs"].as_array().cloned())
                    .unwrap_or_default();
                let key = |r: &Value| {
                    (
                        r["kind"].as_str().unwrap_or("").to_string(),
                        r["id"].as_str().unwrap_or("").to_string(),
                        r["graph"].as_str().unwrap_or("").to_string(),
                    )
                };
                for r in refs {
                    let mut new = json!({ "kind": r.kind, "id": r.id });
                    if let Some(g) = &r.graph {
                        new["graph"] = json!(g);
                    }
                    if let Some(l) = &r.label {
                        new["label"] = json!(l);
                    }
                    if !merged.iter().any(|m| key(m) == key(&new)) {
                        merged.push(new);
                    }
                }
                // Persist the working copy, then patch the refs on disk and
                // reload so the next event folds into the patched file.
                self.save();
                let chat_id = self.chat["id"].as_str().unwrap_or("").to_string();
                let _ = set_row_refs_at(self.root, &chat_id, seq, &merged);
                if let Some(fresh) = load_chat_at(self.root, &chat_id) {
                    self.chat = fresh;
                }
                // Refs annotate the live row; they do not close the step, so
                // following deltas keep appending to the same bubble.
            }
            Event::Error { message } => {
                self.push_row(json!({ "role": "notice", "kind": "error", "content": message }));
                self.set_status("error");
            }
            Event::Canceled => {
                self.push_row(json!({
                    "role": "notice", "kind": "canceled",
                    "content": "turn stopped by the operator",
                }));
            }
        }
        self.save();
    }
}

// ---------------------------------------------------------------------------
// The turn
// ---------------------------------------------------------------------------

/// `POST /ai/ask`'s handler body: append the prompt and hand the turn to a
/// worker thread.
pub fn ask_at(root: &std::path::Path, body: &Value) -> Result<Value, String> {
    let prompt = body["prompt"].as_str().unwrap_or_default();
    if prompt.trim().is_empty() {
        return Err("expected a non-empty `prompt`".to_string());
    }
    let named = body["chat"].as_str().unwrap_or_default().to_string();
    let chat_id = pick_chat(root, &named)?;
    let refs = parse_refs(&body["context"]);
    let mut row = json!({ "role": "user", "content": prompt });
    if !refs.is_empty() {
        row["refs"] = json!(refs);
    }
    append_row_at(root, &chat_id, row)?;
    if let Some(mut chat) = load_chat_at(root, &chat_id) {
        if chat["title"] == "new chat" {
            let title = title_from_prompt(prompt);
            if !title.is_empty() {
                chat["title"] = json!(title);
                save_chat_at(root, &chat)?;
            }
        }
    }
    set_status_at(root, &chat_id, "busy")?;
    let journal = json!({ "prompt": prompt });
    let _ = crate::ai_ask_post_at(crate::ai_log_path(), &journal);
    // A fresh cancel flag for this run — the previous turn's, if any, is gone.
    cancel_registry()
        .lock()
        .unwrap()
        .insert(chat_id.clone(), Arc::new(AtomicBool::new(false)));
    spawn_turn(root.to_path_buf(), chat_id.clone());
    Ok(json!({ "queued": true, "chat": chat_id }))
}

fn pick_chat(root: &std::path::Path, named: &str) -> Result<String, String> {
    if !named.is_empty() && load_chat_at(root, named).is_some() {
        return Ok(named.to_string());
    }
    if let Some(latest) = list_chats_at(root).first() {
        if let Some(id) = latest["id"].as_str() {
            return Ok(id.to_string());
        }
    }
    chat_name(create_chat_at(root)?)
}

fn chat_name(chat: Value) -> Result<String, String> {
    chat["id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "chat file without an id".to_string())
}

fn spawn_turn(root: std::path::PathBuf, chat_id: String) {
    let _ = std::thread::Builder::new()
        .name("fw-chat-turn".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_turn_at(&root, &chat_id)
            }));
            if let Err(_why) = outcome {
                let _ = append_message_at(
                    &root,
                    &chat_id,
                    "assistant",
                    "internal panic — ask again",
                );
                let _ = set_status_at(&root, &chat_id, "idle");
            }
            cancel_registry().lock().unwrap().remove(&chat_id);
        });
}

/// The worker body.
fn run_turn_at(root: &std::path::Path, chat_id: &str) -> Result<(), String> {
    let Some(config) = loaded_provider(root) else {
        let why = "no provider configured — set one in the harness settings \
                   (settings → AI)";
        append_message_at(root, chat_id, "assistant", why)?;
        set_status_at(root, chat_id, "idle")?;
        return Ok(());
    };

    let saved = crate::ai_tools_load(root.join("ai_tools.json"));
    let mut registry = metal_agent::tools::ToolRegistry::new();
    for (name, desc, implemented) in TOOL_CATALOG {
        if *implemented && tool_policy(&saved, name) != ToolPolicy::Off {
            registry.register_native(*name, tool_spec(name, desc));
        }
    }

    let mut history = chat_history(root, chat_id);
    // The model sees the last prompt with its context block in front; the
    // stored row keeps the raw prompt.
    if let Some(refs) = load_chat_at(root, chat_id).and_then(|chat| last_user_refs(&chat)) {
        if let Some(last) = history.iter_mut().rev().find(|m| m.role == "user") {
            last.content = format!("{}{}", context_block(&refs), last.content);
        }
    }
    history::prune_exchanges(&mut history, history::HISTORY_MAX_EXCHANGES);
    if let Ok(prompt) =
        metal_agent::shots::load_system_prompt(root.join("agent").join("system_prompt.md"))
    {
        let level = crate::call::settings::active_tick_level();
        let prompt = format!(
            "{prompt}\n\n## Active tick level\n\nThe active tick level is `{wire}` \
             ({hz} ticks per second). Every tick you send or receive — tool \
             arguments, replies, `?tick=` — is in this level's ticks. The \
             digest is stored canonically at 32 Hz; a base read is its pooled \
             projection.\n",
            wire = level.wire(),
            hz = level.hz(),
        );
        let instructions = saved["instructions"].as_str().unwrap_or("");
        let prompt = metal_agent::harness::with_operator_instructions(&prompt, instructions);
        history.insert(0, ChatMessage::new("system", prompt));
    }

    let mut executor = chat_executor(root)?;
    let mut turn = metal_agent::agent_loop::Turn::new(root);
    let provider = metal_agent::chat::provider::for_config(&config);

    let cancel = cancel_registry()
        .lock()
        .unwrap()
        .get(chat_id)
        .cloned()
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));

    let policy_saved = saved.clone();
    let policy = move |name: &str| tool_policy(&policy_saved, name);
    let approve = move |call: &ToolCallState| -> Approval {
        let (tx, rx) = channel();
        approval_registry().lock().unwrap().insert(call.id.clone(), tx);
        match rx.recv_timeout(APPROVAL_TIMEOUT) {
            Ok(true) => Approval::Allow,
            Ok(false) | Err(_) => Approval::Deny,
        }
    };

    let mut store = Store::open(root, chat_id)?;
    let outcome = {
        let mut sink = |event: Event| store.on_event(&event);
        metal_agent::chat::runner::run_session_turn(
            &*provider,
            &mut history,
            &mut executor,
            &mut turn,
            &registry,
            &policy,
            &mut sink,
            &cancel,
            &approve,
            MAX_ITERATIONS,
        )
    };

    match outcome {
        Ok(RunOutcome::Final(_)) => {
            store.set_status("idle");
            store.save();
        }
        Ok(RunOutcome::Canceled) => {
            store.set_status("idle");
            store.save();
        }
        Err(e) => {
            let mut store = store;
            store.on_event(&Event::Error { message: format!("error: {e}") });
            store.set_status("idle");
            store.save();
        }
    }
    Ok(())
}

fn trim_tool_result(content: String) -> String {
    if content.len() <= TOOL_RESULT_CAP {
        return content;
    }
    let head_len = content
        .char_indices()
        .nth(TOOL_RESULT_CAP)
        .map(|(byte, _)| byte)
        .unwrap_or(TOOL_RESULT_CAP);
    format!(
        "{}\n… ({} more bytes not persisted)",
        &content[..head_len],
        content.len() - head_len
    )
}

fn loaded_provider(root: &std::path::Path) -> Option<metal_agent::config::ProviderConfig> {
    metal_agent::config::ProviderConfig::load(root.join("ai.json"))
        .ok()
        .flatten()
        .filter(|config| !config.key.trim().is_empty())
}

struct WithQuestion {
    inner: Box<dyn metal_agent::agent_loop::ToolExecutor>,
}

impl metal_agent::agent_loop::ToolExecutor for WithQuestion {
    fn execute(&mut self, call: &metal_agent::agent_loop::ToolCall) -> Result<Value, String> {
        if call.name == "question" {
            return Ok(json!({
                "note": "the operator will answer in the chat's next message",
                "question": call.arguments["question"].as_str().unwrap_or("question without text"),
            }));
        }
        self.inner.execute(call)
    }
}

/// The executor a chat turn and `POST /agent/tool` both run tools through.
/// It holds the app's door ([`crate::call::agent_door`]), so the tools that
/// read or write the live state (mask grids, venues, graph ops) reach it; with
/// no door installed they answer "needs the app's door".
#[cfg(feature = "gpu")]
fn chat_executor(root: &std::path::Path) -> Result<WithQuestion, String> {
    // `save_shader` goes through the app's route (`POST /shaders`), so the
    // base harness is handed the same door the shell calls through (rule 6);
    // with no door it stays a bare harness and the tool says so.
    let base = match crate::call::agent_door() {
        Some(door) => {
            metal_agent::harness::Harness::new(root).with_routes(crate::call::door::route_caller(door))
        }
        None => metal_agent::harness::Harness::new(root),
    };
    let inner: Box<dyn metal_agent::agent_loop::ToolExecutor> =
        match crate::author::models::schema::Manifest::load(std::path::Path::new(
            crate::author::SCHEMA,
        )) {
            Ok(manifest) => {
                let mut harness = crate::author::models::agent_tools::AgentHarness::new(base, manifest);
                if let Some(door) = crate::call::agent_door() {
                    harness = harness.with_door(door);
                }
                Box::new(harness)
            }
            Err(_) => Box::new(base),
        };
    Ok(WithQuestion { inner })
}

#[cfg(not(feature = "gpu"))]
fn chat_executor(root: &std::path::Path) -> Result<WithQuestion, String> {
    Ok(WithQuestion {
        inner: Box::new(metal_agent::harness::Harness::new(root)),
    })
}

/// `POST /agent/tool {name, args?, allow_ask?}`: one tool from the agent's
/// catalog, run through the same executor a chat turn uses ([`chat_executor`]),
/// for callers that have no chat (metal-mcp, scripts). Policy applies as in a
/// turn: `off` (or unimplemented) is refused, and `ask` needs `allow_ask`,
/// because this path has no approval UI. A refusal or a tool error is a
/// `{"error": ...}` body, not an HTTP error, so an MCP client shows it as the
/// tool's text. The executor is built per call; nothing here touches `State`.
pub fn agent_tool_at(root: &std::path::Path, body: &Value) -> Result<Value, crate::call::CallError> {
    use crate::call::CallError;
    use metal_agent::agent_loop::{ToolCall, ToolExecutor as _};

    let name = body["name"]
        .as_str()
        .filter(|n| !n.is_empty())
        .ok_or_else(|| CallError::Bad("expected a non-empty `name`".into()))?;
    let args = match body.get("args") {
        None | Some(Value::Null) => json!({}),
        Some(v) if v.is_object() => v.clone(),
        Some(_) => return Err(CallError::Bad("`args` must be an object".into())),
    };
    let allow_ask = body["allow_ask"].as_bool().unwrap_or(false);
    if !TOOL_CATALOG.iter().any(|(n, _, ok)| *n == name && *ok) {
        return Ok(json!({ "error": format!("no such tool: {name}") }));
    }
    let saved = crate::ai_tools_load(root.join("ai_tools.json"));
    match tool_policy(&saved, name) {
        ToolPolicy::Off => return Ok(json!({ "error": "tool is off" })),
        ToolPolicy::Ask if !allow_ask => return Ok(json!({ "error": "denied by operator" })),
        _ => {}
    }
    let mut executor = chat_executor(root).map_err(CallError::Internal)?;
    let call = ToolCall { id: "agent-tool".into(), name: name.into(), arguments: args };
    Ok(match executor.execute(&call) {
        Ok(v) => v,
        Err(e) => json!({ "error": e }),
    })
}

/// The OpenAI-shaped tool spec the adapters speak, built from the real schema
/// table ([`metal_agent::tool_schemas`]) rather than names-only.
fn tool_spec(name: &str, desc: &str) -> Value {
    let parameters = metal_agent::tool_schemas::parameters(name)
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": desc,
            "parameters": parameters,
        }
    })
}

/// The `refs` on a chat's latest user row, when it has any.
fn last_user_refs(chat: &Value) -> Option<Vec<Value>> {
    let row = chat["messages"]
        .as_array()?
        .iter()
        .rev()
        .find(|m| m["role"] == "user")?;
    let refs = row["refs"].as_array()?;
    (!refs.is_empty()).then(|| refs.clone())
}

/// The provider-facing history of one chat: the structured rows (user /
/// assistant with its `tool_calls` / tool) rebuilt from the file. `notice`
/// rows and our own system row are excluded.
fn chat_history(root: &std::path::Path, chat_id: &str) -> Vec<ChatMessage> {
    let Some(chat) = load_chat_at(root, chat_id) else {
        return Vec::new();
    };
    chat["messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .filter_map(ChatMessage::from_json)
                .filter(|m| m.is_wire())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "metal-dash-chat-{name}-{}-{}",
            std::process::id(),
            web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        metal_vfs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// `POST /agent/tool` obeys the same policy a chat turn does: an `ask`
    /// tool is refused unless the caller allows it, `off` is refused outright,
    /// and `auto` runs.
    #[test]
    fn agent_tool_applies_policy_and_runs_in_process_tools() {
        let root = scratch("agent-tool");
        let sleep = |extra: Value| {
            let mut body = json!({ "name": "sleep", "args": { "ms": 0 } });
            body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            agent_tool_at(&root, &body).expect("well-formed body")
        };
        std::fs::write(root.join("ai_tools.json"), r#"{"policy":{"sleep":"ask"}}"#).unwrap();
        assert_eq!(sleep(json!({})), json!({ "error": "denied by operator" }));
        assert_eq!(sleep(json!({ "allow_ask": true }))["slept_ms"], 0);

        std::fs::write(root.join("ai_tools.json"), r#"{"policy":{"sleep":"off"}}"#).unwrap();
        assert_eq!(sleep(json!({ "allow_ask": true })), json!({ "error": "tool is off" }));

        std::fs::write(root.join("ai_tools.json"), r#"{"policy":{"sleep":"auto"}}"#).unwrap();
        assert_eq!(sleep(json!({}))["slept_ms"], 0);

        let unknown = agent_tool_at(&root, &json!({ "name": "no_such_tool" })).unwrap();
        assert_eq!(unknown, json!({ "error": "no such tool: no_such_tool" }));
        assert!(agent_tool_at(&root, &json!({ "args": {} })).is_err());
        assert!(agent_tool_at(&root, &json!({ "name": "sleep", "args": 3 })).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn create_then_list_shows_newest_first() {
        let root = scratch("list");
        let _first = create_chat_at(&root).expect("first");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = create_chat_at(&root).expect("second");
        let listed = list_chats_at(&root);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["id"], second["id"]);
        let _ = metal_vfs::remove_dir_all(&root);
    }

    #[test]
    fn append_persists_content_and_stamps_seq() {
        let root = scratch("append");
        let chat = create_chat_at(&root).expect("create");
        let id = chat["id"].as_str().unwrap().to_string();
        append_message_at(&root, &id, "user", "hello").expect("append");
        let reloaded = load_chat_at(&root, &id).expect("persisted");
        assert_eq!(reloaded["seq"], 1);
        let msgs = reloaded["messages"].as_array().expect("messages array");
        assert_eq!(msgs[0]["seq"], 1);
        assert_eq!(msgs[0]["content"], "hello");
        let _ = metal_vfs::remove_dir_all(&root);
    }

    #[test]
    fn chat_ids_never_escape_the_dir() {
        let root = scratch("ids");
        for bad in ["", "..", "a/b", "ABC", "hello world", "/etc/passwd"] {
            assert!(load_chat_at(&root, bad).is_none(), "id {bad:?} must resolve to nothing");
        }
    }

    #[test]
    fn delete_removes_the_file_and_only_the_file() {
        let root = scratch("delete");
        let chat = create_chat_at(&root).expect("create");
        let first = chat["id"].as_str().unwrap().to_string();
        delete_chat_at(&root, &first).expect("delete");
        assert!(load_chat_at(&root, &first).is_none());
        let _ = metal_vfs::remove_dir_all(&root);
    }

    #[test]
    fn a_turn_without_a_provider_answers_in_the_chat() {
        let root = scratch("no-provider");
        let chat = create_chat_at(&root).expect("create");
        let id = chat["id"].as_str().unwrap().to_string();
        set_status_at(&root, &id, "busy").expect("busy");
        run_turn_at(&root, &id).expect("turn completes");
        let done = load_chat_at(&root, &id).expect("chat still there");
        assert_eq!(done["status"], "idle");
        let msgs = done["messages"].as_array().expect("messages");
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0]["content"].as_str().unwrap().contains("provider"));
        let _ = metal_vfs::remove_dir_all(&root);
    }

    #[test]
    fn oversized_tool_rows_get_a_visible_cut() {
        let long = "x".repeat(TOOL_RESULT_CAP + 500);
        let trimmed = trim_tool_result(long);
        assert!(trimmed.contains("not persisted"));
        assert!(trimmed.len() < TOOL_RESULT_CAP + 120);
        assert_eq!(trim_tool_result("corto".into()), "corto");
    }

    #[test]
    fn history_keeps_structured_rows_and_drops_notices() {
        let root = scratch("history");
        let chat = create_chat_at(&root).expect("create");
        let id = chat["id"].as_str().unwrap().to_string();
        append_message_at(&root, &id, "user", "question").unwrap();
        append_message_at(&root, &id, "notice", "retry 429").unwrap();
        append_message_at(&root, &id, "assistant", "answer").unwrap();
        let history = chat_history(&root, &id);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[1].role, "assistant");
        let _ = metal_vfs::remove_dir_all(&root);
    }

    #[test]
    fn since_returns_only_newer_rows() {
        let root = scratch("since");
        let chat = create_chat_at(&root).expect("create");
        let id = chat["id"].as_str().unwrap().to_string();
        append_message_at(&root, &id, "user", "one").unwrap();
        append_message_at(&root, &id, "assistant", "two").unwrap();
        let tail = load_chat_since(&root, &id, 1);
        let msgs = tail["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["content"], "two");
        assert_eq!(tail["seq"], 2);
        let _ = metal_vfs::remove_dir_all(&root);
    }

    /// The store turns an event stream into structured rows: an assistant row
    /// with a tool call and a tool result, a retry notice, and the final text.
    #[test]
    fn the_store_folds_events_into_structured_rows() {
        let root = scratch("store");
        let chat = create_chat_at(&root).expect("create");
        let id = chat["id"].as_str().unwrap().to_string();
        {
            let mut store = Store::open(&root, &id).unwrap();
            store.on_event(&Event::Delta { text: "let me ".into() });
            store.on_event(&Event::ToolCallReady {
                call_id: "c1".into(),
                name: "set_param".into(),
                args: json!({ "name": "x" }),
            });
            store.on_event(&Event::ToolCalling { call_id: "c1".into() });
            store.on_event(&Event::ToolDone { call_id: "c1".into(), output: "ok".into() });
            store.on_event(&Event::Retrying { attempt: 1, wait_ms: 2000, reason: "429".into() });
            store.on_event(&Event::Final { content: "done".into() });
        }
        let saved = load_chat_at(&root, &id).unwrap();
        let msgs = saved["messages"].as_array().unwrap();
        let assistant = msgs.iter().find(|m| m["role"] == "assistant" && m["tool_calls"].is_array());
        let assistant = assistant.expect("an assistant row with tool calls");
        assert_eq!(assistant["tool_calls"][0]["name"], "set_param");
        assert_eq!(assistant["tool_calls"][0]["status"], "done");
        assert!(msgs.iter().any(|m| m["role"] == "tool" && m["tool_call_id"] == "c1"));
        assert!(msgs.iter().any(|m| m["kind"] == "retry"));
    }

    #[test]
    fn list_rows_carry_count_preview_and_updated() {
        let root = scratch("list-rows");
        let chat = create_chat_at(&root).expect("create");
        let id = chat["id"].as_str().unwrap().to_string();
        let long = "p".repeat(120);
        append_message_at(&root, &id, "user", "first").unwrap();
        append_message_at(&root, &id, "assistant", "answer").unwrap();
        append_message_at(&root, &id, "notice", "retry").unwrap();
        append_message_at(&root, &id, "user", &long).unwrap();
        let listed = list_chats_at(&root);
        assert_eq!(listed[0]["count"], 3);
        assert_eq!(listed[0]["preview"], json!("p".repeat(PREVIEW_CHARS)));
        assert!(listed[0]["updated"].as_u64().unwrap() > 0);
        let _ = metal_vfs::remove_dir_all(&root);
    }

    #[test]
    fn refs_are_validated_and_capped() {
        let raw = json!([
            { "kind": "node", "id": "abc", "graph": "g1", "label": "Lamp" },
            { "kind": "graph", "id": "g2" },
            { "kind": 3, "id": "x" },
            { "kind": "node" },
            { "kind": "node", "id": "y", "graph": 7 },
            "junk",
        ]);
        let refs = parse_refs(&raw);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0], json!({ "kind": "node", "id": "abc", "graph": "g1", "label": "Lamp" }));
        assert_eq!(refs[1], json!({ "kind": "graph", "id": "g2", "label": "" }));
        let many: Vec<Value> = (0..12).map(|i| json!({ "kind": "node", "id": i.to_string() })).collect();
        assert_eq!(parse_refs(&json!(many)).len(), MAX_REFS);
        assert!(parse_refs(&json!("nope")).is_empty());
    }

    #[test]
    fn context_block_is_compact() {
        let refs = parse_refs(&json!([
            { "kind": "node", "id": "abc", "graph": "g1", "label": "Label" },
            { "kind": "output", "id": "o1" },
        ]));
        assert_eq!(
            context_block(&refs),
            "[context]\nnode g1/abc (Label)\noutput o1\n[/context]\n"
        );
        assert_eq!(context_block(&[]), "");
    }

    #[test]
    fn ask_persists_raw_prompt_with_refs_and_titles_the_chat() {
        let root = scratch("ask-refs");
        let chat = create_chat_at(&root).expect("create");
        let id = chat["id"].as_str().unwrap().to_string();
        let prompt = "  haz que el foco pulse con el bajo y que sea mas lento  ";
        let body = json!({
            "chat": id,
            "prompt": prompt,
            "context": [{ "kind": "node", "id": "abc", "graph": "g1", "label": "Lamp" }],
        });
        // The worker thread would call the provider; the row is what we test.
        append_row_at(&root, &id, json!({
            "role": "user", "content": prompt,
            "refs": parse_refs(&body["context"]),
        })).unwrap();
        let saved = load_chat_at(&root, &id).unwrap();
        let row = &saved["messages"][0];
        assert_eq!(row["content"], prompt);
        assert_eq!(row["refs"][0]["id"], "abc");
        assert_eq!(
            last_user_refs(&saved).map(|r| r.len()),
            Some(1)
        );
        assert_eq!(title_from_prompt(prompt), "haz que el foco pulse con el bajo y que");
        let _ = metal_vfs::remove_dir_all(&root);
    }

    #[test]
    fn set_row_refs_patches_one_row_and_keeps_its_keys() {
        let root = scratch("row-refs");
        let chat = create_chat_at(&root).expect("create");
        let id = chat["id"].as_str().unwrap().to_string();
        {
            let mut store = Store::open(&root, &id).unwrap();
            store.on_event(&Event::Final { content: "mira esto".into() });
        }
        let seq = load_chat_at(&root, &id).unwrap()["messages"][0]["seq"].as_u64().unwrap();
        let refs = parse_refs(&json!([{ "kind": "node", "id": "abc", "graph": "g1" }]));
        set_row_refs_at(&root, &id, seq, &refs).unwrap();
        // Patching refs leaves the row's text and other keys alone.
        let row = load_chat_at(&root, &id).unwrap()["messages"][0].clone();
        assert_eq!(row["content"], "mira esto");
        assert_eq!(row["seq"].as_u64(), Some(seq));
        assert_eq!(row["refs"][0]["id"], "abc");
        set_row_refs_at(&root, &id, seq, &[]).unwrap();
        assert!(load_chat_at(&root, &id).unwrap()["messages"][0].get("refs").is_none());
        let _ = metal_vfs::remove_dir_all(&root);
    }

    #[test]
    fn refs_event_merges_into_the_live_row_without_duplicates() {
        use metal_agent::chat::message::Ref;
        let root = scratch("event-refs");
        let chat = create_chat_at(&root).expect("create");
        let id = chat["id"].as_str().unwrap().to_string();
        let node = |id: &str| Ref {
            kind: "node".into(),
            id: id.into(),
            graph: Some("g1".into()),
            label: None,
        };
        {
            let mut store = Store::open(&root, &id).unwrap();
            store.on_event(&Event::Delta { text: "mira".into() });
            store.on_event(&Event::Refs { refs: vec![node("a"), node("b")] });
            store.on_event(&Event::Refs { refs: vec![node("b"), node("c")] });
        }
        let row = load_chat_at(&root, &id).unwrap()["messages"][0].clone();
        assert_eq!(row["content"], "mira");
        let ids: Vec<&str> = row["refs"].as_array().unwrap().iter()
            .map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert!(row["refs"][0].get("label").is_none());
        let _ = metal_vfs::remove_dir_all(&root);
    }
}
