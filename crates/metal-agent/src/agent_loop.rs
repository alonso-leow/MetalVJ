//! The loop (H.3): prompt -> message + batch of tool calls -> results ->
//! repeat, terminating on a message with no tool calls.
//!
//! This pass proves the shape only. Real tools don't exist yet (H.4), so
//! [`run_turn`] calls [`ToolExecutor`] which is a trait a caller implements —
//! for this pass the only implementation is a stub that returns an error for
//! every call, which is enough to confirm the loop terminates correctly when
//! there is nothing to call and structurally supports looping when there is.
//!
//! Judgment call (flagged for review before H.4): the request/response JSON
//! shape here is a generic "OpenAI-style chat completions" shape
//! (`{model, messages, tools}` in; `choices[0].message` with optional
//! `tool_calls` out) because that is the closest thing to a lingua franca
//! across the three presets. H.3's spec says "all speak the same tool-calling dialect;
//! the per-preset adapter is the only thing that changes" — this
//! pass does NOT implement per-preset adapters (Claude's `/v1/messages` shape
//! differs enough from OpenAI's `/v1/chat/completions` shape that a real
//! adapter is real work); `send_request` always builds and parses the one
//! generic shape. That is the biggest gap for H.4 to close: either add a
//! `Preset -> Adapter` translation layer, or confirm DeepInfra/OpenAI-shaped
//! endpoints are the only ones this crate needs to hit directly and Claude
//! goes through an OpenAI-compatible endpoint.

use crate::config::ProviderConfig;
use crate::tools::ToolRegistry;
pub use crate::turn::Turn;
use serde_json::{json, Value};

#[derive(Debug, Clone, Default)]
pub struct Message {
    pub role: String,
    pub content: String,
    /// An assistant row's own tool calls. Carried so the `tool` rows after
    /// it answer something: OpenAI-shaped endpoints reject a `tool` row whose
    /// `tool_call_id` names no call in the assistant row before it, which
    /// made every turn with a tool in it fail on its *second* request.
    pub tool_calls: Vec<ToolCall>,
    /// A `tool` row's answer-to: the id of the call it is the result of.
    pub tool_call_id: Option<String>,
    /// Pixels this row shows the model. Only ever set on the `user` row the
    /// loop adds after a batch of tool results that carried images (see
    /// [`lift_images`]): a `tool` row is text-only on the chat-completions
    /// wire, so an image a tool hands back rides the next user row instead.
    pub images: Vec<(String, crate::images::TurnImage)>,
}

impl Message {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Message {
        Message { role: role.into(), content: content.into(), ..Message::default() }
    }
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// What one completed turn hands back: the loop's final message, and the
/// entire conversation it ran on as a second field — system prompt out,
/// every `(assistant, tool)` pair the loop drove and every correction the
/// gate pushed back included, the final `Message` *not* among them (it is
/// its own separate field; callers that persist the conversation append it
/// last, which is exactly what the chat does). Caller-owned conversation
/// memory: nothing in this crate persists anything anywhere.
pub struct TurnReport {
    pub final_message: Message,
    pub messages: Vec<Message>,
}

/// Executes one tool call and returns its result content. H.4's real tools
/// implement this; this pass ships no implementations, only the seam.
pub trait ToolExecutor {
    fn execute(&mut self, call: &ToolCall) -> Result<Value, String>;
}

/// A `ToolExecutor` that always fails — exists only so the loop's "no tools
/// yet" shape can be exercised in a test without a real implementation.
pub struct NoTools;
impl ToolExecutor for NoTools {
    fn execute(&mut self, call: &ToolCall) -> Result<Value, String> {
        Err(format!("no tool implementations registered yet (H.4); tried to call {}", call.name))
    }
}

/// Runs one full turn: repeatedly sends the conversation to the provider,
/// executes any tool calls the response asks for, feeds the results back,
/// and stops when a response comes back with no tool calls **and the turn
/// says it may close**.
///
/// `turn` is O.2's level of composition and is *not* closed by this function
/// — the caller closes it when the whole turn (not just one loop iteration)
/// is done, so a tool call late in the loop can still see an image captured
/// earlier in the same turn, per H.3.
///
/// # The one behaviour worth naming here
///
/// A message with no tool calls used to end the turn on the spot. It does not
/// any more: [`Turn::may_finish`] gets asked first, and a turn with an
/// unverified build or a half-finished `locate` sends the reason back as a
/// user-role message and keeps going. That is §I.4's *"the AI does not answer
/// 'done' without having looked"* implemented as a property of the loop rather
/// than as a line in the system prompt — a model that ignores the prompt
/// still cannot get past this.
pub fn run_turn(
    config: &ProviderConfig,
    messages: Vec<Message>,
    executor: &mut dyn ToolExecutor,
    turn: &mut Turn,
    registry: &ToolRegistry,
    max_iterations: usize,
) -> Result<TurnReport, String> {
    use crate::chat::runner::{Noop, RunOutcome};
    use std::sync::atomic::AtomicBool;

    // The thin shim: this is now [`crate::chat::runner::run_session_turn`] with
    // a no-op sink (the old caller has no event store) and an always-allow
    // policy (the old caller has no approval UI). Everything the old loop did
    // — provider call, tool execution, image lifting, `Turn::may_finish` —
    // happens inside the runner; only the value shapes differ.
    let provider = crate::chat::provider::for_config(config);
    let mut conversation: Vec<crate::chat::ChatMessage> =
        messages.into_iter().map(Into::into).collect();
    let policy = |_name: &str| crate::chat::policy::ToolPolicy::Auto;
    let mut sink = |_event: crate::chat::Event| {};
    let outcome = crate::chat::runner::run_session_turn(
        &*provider,
        &mut conversation,
        executor,
        turn,
        registry,
        &policy,
        &mut sink,
        &AtomicBool::new(false),
        &Noop::allow,
        max_iterations,
    )
    .map_err(|e| e.to_string())?;

    match outcome {
        RunOutcome::Final(_) => {
            // The runner keeps the whole conversation, final row included; the
            // old `TurnReport` keeps the final row separate, so pop it.
            let final_message = conversation.pop().unwrap_or_default().into();
            Ok(TurnReport {
                final_message,
                messages: conversation.into_iter().map(Into::into).collect(),
            })
        }
        RunOutcome::Canceled => Err("turn canceled".into()),
    }
}

/// The keys a tool result hands an image back under, and the mime each
/// means. `raster`, `capture_camera_frame`, `locate` and `look_venue` all
/// use one of these.
const IMAGE_KEYS: [(&str, &str); 2] = [("png_base64", "image/png"), ("jpeg_base64", "image/jpeg")];

/// Takes every image out of a tool result and into the turn's own store.
///
/// A camera frame as base64 inside a `tool` row is a megabyte of text the
/// model is asked to read as characters: it cannot see it, it costs the
/// context window, and the next tool that needs the pixels (`locate`) gets
/// them only if the model copies the megabyte back out, character for
/// character. So each `png_base64`/`jpeg_base64` anywhere in the result is
/// decoded into [`crate::images::TurnImages`] under a fresh id (`img-3`),
/// the key in the result is replaced by `"image": "img-3"`, and the ids come
/// back so the loop can show those pixels to the model as an image.
pub fn lift_images(
    mut value: Value,
    tool: &str,
    store: &mut crate::images::TurnImages,
) -> (Value, Vec<String>) {
    fn walk(v: &mut Value, tool: &str, store: &mut crate::images::TurnImages, ids: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                for (key, mime) in IMAGE_KEYS {
                    let Some(Value::String(b64)) = map.get(key) else { continue };
                    let Ok(bytes) = crate::harness::base64_decode(b64) else { continue };
                    let id = store.put(format!("img-{}", store.len() + 1), mime, bytes);
                    map.remove(key);
                    map.insert("image".into(), json!(id));
                    map.entry("image_from").or_insert_with(|| json!(tool));
                    ids.push(id);
                }
                for (_, child) in map.iter_mut() {
                    walk(child, tool, store, ids);
                }
            }
            Value::Array(items) => {
                for child in items {
                    walk(child, tool, store, ids);
                }
            }
            _ => {}
        }
    }
    let mut ids = Vec::new();
    walk(&mut value, tool, store, &mut ids);
    (value, ids)
}

/// The other half of [`lift_images`]: a call that names a turn image by id
/// (`"image": "img-3"`) and carries no pixels gets them put back as
/// `png_base64`, which is what every tool that reads an image takes. A call
/// that already carries pixels, or names no known image, is left alone.
pub fn with_turn_images(call: &ToolCall, store: &crate::images::TurnImages) -> ToolCall {
    let mut call = call.clone();
    if let Value::Object(args) = &mut call.arguments {
        let named = args.get("image").and_then(Value::as_str).and_then(|id| store.get(id));
        if let (Some(img), false) = (named, args.contains_key("png_base64")) {
            args.insert("png_base64".into(), json!(crate::harness::base64_encode(&img.bytes)));
        }
    }
    call
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_only_response_terminates_the_loop() {
        let value = json!({
            "choices": [{ "message": { "role": "assistant", "content": "hello" } }]
        });
        let msg = crate::chat::provider::openai::parse_step(&value).unwrap();
        assert_eq!(msg.content, "hello");
        assert!(msg.tool_calls.is_empty());
    }

    #[test]
    fn tool_calls_are_parsed_out_of_the_response() {
        let value = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call_1",
                        "function": { "name": "get_env", "arguments": "{}" }
                    }]
                }
            }]
        });
        let msg = crate::chat::provider::openai::parse_step(&value).unwrap();
        assert_eq!(msg.tool_calls.len(), 1);
        assert_eq!(msg.tool_calls[0].name, "get_env");
    }

    /// A fake provider that answers each request with the next scripted body
    /// and records what it was sent. `metal-agent` has no network in tests, and
    /// a local `TcpListener` is closer to the real path than stubbing
    /// `send_request` would be — it exercises the request body too.
    fn fake_provider(bodies: Vec<Value>) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            for body in bodies {
                let Ok((mut s, _)) = listener.accept() else { return };
                // Read until the whole body is in, not just whatever the
                // first segment carried: the headers and the JSON arrive
                // separately often enough that a single `read` silently
                // hands back a request with no body, and an assertion
                // against that passes or fails for the wrong reason.
                let mut got = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    let n = s.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&got);
                    let Some(head_end) = text.find("\r\n\r\n") else { continue };
                    let want: usize = text
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    if got.len() >= head_end + 4 + want {
                        break;
                    }
                }
                let _ = tx.send(String::from_utf8_lossy(&got).into_owned());
                let body = body.to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });

        (format!("http://127.0.0.1:{port}/chat"), rx)
    }

    fn said(text: &str) -> Value {
        json!({ "choices": [{ "message": { "role": "assistant", "content": text } }] })
    }

    fn config_at(url: String) -> ProviderConfig {
        let mut config =
            ProviderConfig::new(crate::config::Preset::Manual, "test-model", "sk-test1234");
        config.url = url;
        config
    }

    fn workdir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("metal-agent-loop-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn run_turn_terminates_immediately_when_there_are_no_tool_calls() {
        let (url, _rx) = fake_provider(vec![said("done")]);
        let mut turn = Turn::new(workdir("no-tools"));
        let result = run_turn(
            &config_at(url),
            vec![Message::new("user", "hello")],
            &mut NoTools,
            &mut turn,
            &ToolRegistry::new(),
            5,
        )
        .unwrap();

        assert_eq!(result.final_message.content, "done");
    }

    #[test]
    fn a_turn_with_an_unverified_build_cannot_answer_listo() {
        // §I.4 as a property of the loop: the model says "done" twice, and
        // the first one does not get to be the answer because the build never
        // went through the gate. The second does, because between them the
        // turn's own state changed — here by the gate passing.
        use metal_schema::ir::{OpCall, ShotIR};

        let dir = workdir("must-look");
        let mut turn = Turn::new(&dir);
        turn.plan(&ShotIR {
            id: "s".to_string(),
            duration_secs: 2.0,
            ops: vec![OpCall { op: "noise".to_string(), floats: vec!["seed".to_string()] }],
        })
        .unwrap();
        turn.build("shader v1").unwrap();

        // A gate that passes, run by the executor when the model calls a
        // tool — which is what "having looked" means here.
        struct GateOnCall(std::path::PathBuf);
        impl ToolExecutor for GateOnCall {
            fn execute(&mut self, _call: &ToolCall) -> Result<Value, String> {
                let outcome = metal_schema::resume::GateOutcome { passed: true, issues: vec![] };
                std::fs::write(
                    self.0.join("gate_report.json"),
                    serde_json::to_string(&outcome).unwrap(),
                )
                .unwrap();
                Ok(json!({ "ok": true }))
            }
        }

        let (url, rx) = fake_provider(vec![
            said("done"),
            json!({ "choices": [{ "message": { "role": "assistant", "content": "",
                "tool_calls": [{ "id": "c1", "function": { "name": "raster", "arguments": "{}" } }] } }] }),
            said("done, for real now"),
        ]);

        let result = run_turn(
            &config_at(url),
            vec![Message::new("user", "make the shader")],
            &mut GateOnCall(dir.clone()),
            &mut turn,
            &ToolRegistry::new(),
            5,
        )
        .unwrap();

        assert_eq!(result.final_message.content, "done, for real now");

        // And the model was told *why* its first answer was refused, in the
        let second = rx.iter().nth(1).expect("a second request");
        // The refusal text comes from Turn::may_finish (turn.rs).
        assert!(second.contains("cannot close"), "{second}");
    }

    #[test]
    fn the_request_carries_the_tool_specs_and_omits_them_when_there_are_none() {
        let spec = json!({ "type": "function", "function": { "name": "get_env" } });
        let mut registry = ToolRegistry::new();
        registry.register_native("get_env", spec.clone());

        let (url, rx) = fake_provider(vec![said("ok")]);
        let mut turn = Turn::new(workdir("with-tools"));
        run_turn(
            &config_at(url),
            vec![Message::new("user", "hello")],
            &mut NoTools,
            &mut turn,
            &registry,
            2,
        )
        .unwrap();
        let sent = rx.recv().unwrap();
        assert!(sent.contains("get_env"), "the tools never reached the provider: {sent}");

        // Empty registry: no `tools` key at all, not `tools: []`.
        let (url, rx) = fake_provider(vec![said("ok")]);
        let mut turn = Turn::new(workdir("no-tools-key"));
        run_turn(
            &config_at(url),
            vec![Message::new("user", "hello")],
            &mut NoTools,
            &mut turn,
            &ToolRegistry::new(),
            2,
        )
        .unwrap();
        let sent = rx.recv().unwrap();
        assert!(!sent.contains("\"tools\""), "{sent}");
    }

    /// A tool that hands back an image, the way `capture_camera_frame` does.
    struct Camera;
    impl ToolExecutor for Camera {
        fn execute(&mut self, call: &ToolCall) -> Result<Value, String> {
            match call.name.as_str() {
                "capture_camera_frame" => Ok(json!({
                    "ok": true,
                    "png_base64": crate::harness::base64_encode(&[0x89, b'P', b'N', b'G', 1, 2, 3]),
                })),
                // Echoes whether the pixels came back in, by length.
                "locate" => Ok(json!({
                    "got": call.arguments["png_base64"].as_str().map(|b| b.len()).unwrap_or(0),
                })),
                other => Err(format!("no {other}")),
            }
        }
    }

    fn called(id: &str, name: &str, args: Value) -> Value {
        json!({ "choices": [{ "message": {
            "role": "assistant",
            "content": "",
            "tool_calls": [{ "id": id, "type": "function",
                "function": { "name": name, "arguments": args.to_string() } }],
        } }] })
    }

    fn body_of(request: &str) -> Value {
        let at = request.find("\r\n\r\n").expect("a body") + 4;
        serde_json::from_str(&request[at..]).expect("json body")
    }

    /// The second request of a tool turn is what OpenAI-shaped endpoints
    /// validate: the assistant row names its calls, each tool row answers
    /// one of them, and the camera's pixels arrive as an image part — not
    /// as base64 text inside the tool row.
    #[test]
    fn a_tool_turn_answers_its_calls_by_id_and_shows_images_as_images() {
        let (url, rx) = fake_provider(vec![
            called("call_1", "capture_camera_frame", json!({})),
            called("call_2", "locate", json!({ "step": "grid", "image": "img-1" })),
            said("done"),
        ]);
        let mut turn = Turn::new(workdir("images"));
        let report = run_turn(
            &config_at(url),
            vec![Message::new("user", "look at the room")],
            &mut Camera,
            &mut turn,
            &ToolRegistry::new(),
            4,
        )
        .expect("the turn completes");
        assert_eq!(report.final_message.content, "done");

        let requests: Vec<Value> = rx.iter().take(3).map(|r| body_of(&r)).collect();
        let second = requests[1]["messages"].as_array().unwrap();
        let assistant = &second[1];
        assert_eq!(assistant["tool_calls"][0]["id"], "call_1");
        assert_eq!(assistant["tool_calls"][0]["function"]["name"], "capture_camera_frame");
        let tool = &second[2];
        assert_eq!(tool["role"], "tool");
        assert_eq!(tool["tool_call_id"], "call_1");
        let result: Value = serde_json::from_str(tool["content"].as_str().unwrap()).unwrap();
        assert_eq!(result["image"], "img-1");
        assert!(result.get("png_base64").is_none(), "the pixels left the tool row");
        let shown = &second[3];
        assert_eq!(shown["role"], "user");
        assert!(shown["content"][1]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));

        // `locate` named the image by id and got the pixels back.
        let third = requests[2]["messages"].as_array().unwrap();
        let locate: Value =
            serde_json::from_str(third.last().unwrap()["content"].as_str().unwrap()).unwrap();
        assert!(locate["got"].as_u64().unwrap() > 0, "{locate}");
        assert_eq!(turn.images.len(), 1);
    }

    #[test]
    fn images_nested_in_a_result_are_lifted_too() {
        let mut store = crate::images::TurnImages::new();
        let b64 = crate::harness::base64_encode(&[0xff, 0xd8, 0xff]);
        let (v, ids) = lift_images(
            json!({ "outputs": [{ "device": "a", "jpeg_base64": b64 }, { "device": "b" }] }),
            "look_venue",
            &mut store,
        );
        assert_eq!(ids, ["img-1"]);
        assert_eq!(v["outputs"][0]["image"], "img-1");
        assert_eq!(store.get("img-1").unwrap().mime, "image/jpeg");
    }
}
