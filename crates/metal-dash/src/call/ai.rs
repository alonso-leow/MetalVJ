//! H.8: the AI-provider overlay. Neither handler touches `State` — the key
//! lives in its own file (`ai_config_path`), not in `data/`.
//!
//! `S.U14` adds this route's second half: the `tools` array
//! (`crate::ai_tools_reply`) the harness sidebar renders one control per
//! entry from — `TOOL_CATALOG`'s own catalog, read through the door rather
//! than imported by `shell/` directly (gate 1 bans `metal_agent` there; this
//! file, one layer below the door, is where that import is allowed to
//! live). `POST /ai-config` gains a `tools` object alongside its existing
//! provider fields (`preset`/`url`/`model`/`key`) — a body carrying `tools`
//! and no `preset` is a pure tool-toggle and never touches `ai.json`, so
//! flipping a tool on/off cannot accidentally blank out a saved provider
//! (or vice versa: saving a provider never touches `ai_tools.json`).

use serde_json::Value;

use crate::call::{CallError, Ctx, Params};

/// The full reply: whatever `ai_config_get_reply_at` answers about the
/// provider, plus `tools` (`crate::ai_tools_reply`) merged in — one call so
/// the sidebar's single `GET /ai-config` sees both halves in one round
/// trip, matching how `header.rs`'s own `GET /state` reads are one call for
/// several unrelated sections of that reply.
fn reply() -> Result<Value, CallError> {
    let mut v = crate::ai_config_get_reply_at(
        crate::ai_config_path(),
        &metal_agent::config::ProviderConfig::default_config_toml(),
    )
    .map_err(CallError::Bad)?;
    let tools = crate::ai_tools_reply(crate::ai_tools_path());
    if let (Some(obj), Some(tools_obj)) = (v.as_object_mut(), tools.as_object()) {
        for (k, val) in tools_obj {
            obj.insert(k.clone(), val.clone());
        }
    }
    // The presets the settings dropdown offers, each with the standard URL the
    // server assigns on save (`manual` has none: its URL is typed).
    if let Some(obj) = v.as_object_mut() {
        let presets: Vec<Value> = metal_agent::config::Preset::ALL
            .iter()
            .map(|p| serde_json::json!({ "name": p.wire(), "url": p.default_url(), "local": p.is_local() }))
            .collect();
        obj.insert("presets".to_string(), Value::Array(presets));
    }
    Ok(v)
}

pub fn get(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    reply()
}

/// A `tools`-only body (no `preset`) is the sidebar's per-tool toggle and
/// never runs the provider-config path at all — see this module's own doc
/// comment on why the two never touch each other's file.
pub fn post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    // Tool picks, capability permissions and the operator's instructions all
    // live in `ai_tools.json`; none of them carries a provider `preset`.
    let tools_file_key = ["tools", "permissions", "instructions", "focus_mode"]
        .iter()
        .any(|k| body.get(*k).is_some());
    if tools_file_key && body.get("preset").is_none() {
        return match crate::ai_tools_post_at(crate::ai_tools_path(), &body) {
            Ok(_) => reply(),
            Err(e) => Err(CallError::Bad(e)),
        };
    }
    match crate::ai_config_post_at(
        crate::ai_config_path(),
        &metal_agent::config::ProviderConfig::default_config_toml(),
        &body,
    ) {
        Ok(mut config_reply) => {
            let tools = crate::ai_tools_reply(crate::ai_tools_path());
            if let (Some(obj), Some(tools_obj)) = (config_reply.as_object_mut(), tools.as_object())
            {
                for (k, val) in tools_obj {
                    obj.insert(k.clone(), val.clone());
                }
            }
            Ok(config_reply)
        }
        Err(e) => Err(CallError::Bad(e)),
    }
}

/// `GET /ai-models` — the models the saved provider lists (`{ models: [...] }`),
/// or `{ models: [], error }` when it has no listing or cannot be reached. It
/// reads the stored key, never one from the query.
pub fn models(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let cfg = metal_agent::config::ProviderConfig::load_at(
        crate::ai_config_path(),
        &metal_agent::config::ProviderConfig::default_config_toml(),
    )
    .ok()
    .flatten();
    let Some(cfg) = cfg else {
        return Ok(serde_json::json!({ "models": [], "error": "no provider saved" }));
    };
    Ok(match metal_agent::config::list_models(cfg.preset, &cfg.url, &cfg.key) {
        Ok(models) => serde_json::json!({ "models": models }),
        Err(e) => serde_json::json!({ "models": [], "error": e }),
    })
}

/// `POST /ai/ask` — the sidebar's send button. `crate::chat::ask_at`
/// appends the prompt to its chat (`data/chats/<id>.json`), flips the
/// chat's `status` to `busy` and hands the turn to a worker thread — the
/// loop is the harness's own (`metal_agent::agent_loop::run_turn`), see
/// `crate::chat`'s module doc. Optional `context: [{kind, id, graph?, label?}]`
/// (max 8 well-formed refs) is stored on the user row as `refs` and sent to
/// the model as a `[context]` block (`crate::chat::parse_refs`). When the body carries no `chat`, the most
/// recently updated conversation takes the prompt, or a fresh one is
/// created for it.
pub fn ask(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    crate::chat::ask_at(&crate::output::data_root(), &body).map_err(CallError::Bad)
}

/// `GET /ai/log` — the sidebar's scrolling transcript, the whole array
/// `POST /ai/ask` appends to.
pub fn log(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(serde_json::json!({ "log": crate::ai_log_load(crate::ai_log_path()) }))
}

/// `GET /ai/chats` — the chat menu's read: one row per conversation, most
/// recently updated first (`crate::chat::list_chats_at`). Rows carry
/// `id`/`title`/`status`/`updated`, plus `count` (user + assistant messages)
/// and `preview` (first 80 chars of the last user prompt) — enough to paint
/// the history view and know which conversation to open; the messages
/// themselves are the prefix route below.
pub fn chats(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(serde_json::json!({ "chats": crate::chat::list_chats_at(&crate::output::data_root()) }))
}

/// `POST /ai/chats` — the chat menu's `nuevo` button. The fresh chat carries
/// no messages; its `id` is the reply, so the sidebar can switch to it.
pub fn chats_new(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    crate::chat::create_chat_at(&crate::output::data_root()).map_err(CallError::Bad)
}

/// `GET /ai/chats/<id>` — one conversation. A `?since=<n>` query returns only
/// the rows with a larger `seq` (the UI's incremental poll); absent, the whole
/// conversation. A name nobody has on disk answers an empty `idle`
/// conversation rather than a 404 — the sidebar polls in lockstep with the
/// worker's rewrites.
pub fn chat_get(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(id) = params
        .tail
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return Err(CallError::Bad("expected a chat id after /ai/chats/".into()));
    };
    let since = params.param("since").and_then(|v| v.parse().ok()).unwrap_or(0);
    Ok(crate::chat::load_chat_since(&crate::output::data_root(), id, since))
}

/// `POST /ai/chat/cancel {id}` — the stop button: flips the chat's cancel
/// flag; the runner notices between stream chunks and tool calls.
pub fn chat_cancel(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let Some(id) = body["id"].as_str().filter(|id| !id.is_empty()) else {
        return Err(CallError::Bad("expected a non-empty `id`".into()));
    };
    Ok(serde_json::json!({ "canceled": crate::chat::cancel_chat(id) }))
}

/// `POST /ai/chat/approve {call, allow}` — Allow/Deny for a call that policy
/// put in `ask`. `call` names the tool-call id, `allow` the operator's answer.
pub fn chat_approve(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let Some(call) = body["call"].as_str().filter(|c| !c.is_empty()) else {
        return Err(CallError::Bad("expected a non-empty `call`".into()));
    };
    let allow = body["allow"].as_bool().unwrap_or(false);
    Ok(serde_json::json!({ "answered": crate::chat::approve_call(call, allow) }))
}

/// `POST /ai/chat/delete` (body `{id}`) — the menu's ✕: the conversation's
/// file is removed and nothing else references it.
pub fn chat_delete(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let Some(id) = body["id"].as_str().filter(|id| !id.is_empty()) else {
        return Err(CallError::Bad("expected a non-empty `id`".into()));
    };
    crate::chat::delete_chat_at(&crate::output::data_root(), id).map_err(CallError::Bad)
}

/// `POST /ai/train { node }` (optionally `{ graph }`) — the train gesture of a
/// `gen.*` source, the route an `entrenar` button and the `train_ai` tool both
/// post.
///
/// **The source owns its fit.** The grade lives on the source: its
/// `config.weights` names the grade file (`<reels>/<name>.json`), or is absent
/// and the model's own default grade (`metal_reel::grade::model_path`) is
/// written — the same file the studio's train button writes, so training from
/// the node and from the studio produce one file, not two.
///
/// The node is found by id. A node id is unique inside one graph, so `graph`
/// is optional: without it, every graph under `data/graphs/` is searched for
/// the node.
pub fn train(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let Some(node) = body["node"].as_str().filter(|n| !n.is_empty()) else {
        return Err(CallError::Bad("expected a non-empty `node`".into()));
    };
    let graph = body["graph"].as_str().filter(|g| !g.is_empty());
    let ids = ctx.paint.ids();
    let schemas = ctx.paint.schemas();
    let (reply, trained) = train_at(&crate::output::data_root(), node, graph, &ids, &schemas)?;
    // The engine caches one Colourist per model; drop each trained model so
    // the next frame reads the file that was just written.
    for model in trained {
        ctx.state.reels.reload(&model);
    }
    Ok(reply)
}

/// The graph that contains `node`, by id when `graph` names one, otherwise the
/// first graph under `<root>/graphs` that does.
fn graph_with_node(
    root: &std::path::Path,
    node: &str,
    graph: Option<&str>,
) -> Result<metal_graph::Graph, CallError> {
    if let Some(id) = graph {
        return crate::load_graph_at(root, id).map_err(CallError::Bad);
    }
    let dir = crate::graphs_dir(root);
    let Ok(entries) = metal_vfs::read_dir(&dir) else {
        return Err(CallError::Bad(format!("no graphs directory at {}", dir.display())));
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Ok(g) = crate::load_graph_at(root, stem) {
            if g.node(node).is_some() {
                return Ok(g);
            }
        }
    }
    Err(CallError::Bad(format!("no graph contains node {node:?}")))
}

/// The core of [`train`], against an explicit data root so a test can point it
/// at a scratch tree (rule 8). Returns the reply and the model it trained, so
/// the caller can invalidate the engine's cache.
///
/// `#[cfg(feature = "gpu")]`: `crate::author` — the module that owns
/// `takes_for` and the manifest — is gpu-gated, and so is the `renderer` whose
/// schemas this needs. A build with no `gpu` cannot train, and says so.
#[cfg(feature = "gpu")]
fn train_at(
    root: &std::path::Path,
    node: &str,
    graph: Option<&str>,
    ids: &[&str],
    schemas: &[&metal_schema::Schema],
) -> Result<(Value, Vec<String>), CallError> {
    use crate::author::models::EXAMPLES_DIR;
    use crate::author::SONGS;

    let g = graph_with_node(root, node, graph)?;
    let source = g
        .node(node)
        .ok_or_else(|| CallError::Bad(format!("no node {node:?} in that graph")))?;
    let kind = source.kind.clone();
    if kind == metal_graph::AI_LOOPS_KIND {
        // The rhythm finder reads the approved loop labels, not a gen.* feed,
        // so it has no model to walk: one fit, one file.
        let reply = train_loops_at(
            root,
            node,
            std::path::Path::new(crate::author::dataset::LOOPS_DIR),
            std::path::Path::new(crate::author::SONGS),
        )?;
        return Ok((reply, Vec::new()));
    }
    let Some(model) = kind.strip_prefix("gen.") else {
        return Err(CallError::Bad(format!("{kind} is not a gen.* source; only a source owns a fit")));
    };
    let model = model.to_string();
    let weights = source
        .config
        .get("weights")
        .and_then(|v| v.as_str())
        .filter(|n| !n.is_empty())
        .map(str::to_string);
    let idx = ids
        .iter()
        .position(|i| *i == model)
        .ok_or_else(|| CallError::Bad(format!("no model {model:?} in the renderer")))?;
    let real_schema = schemas[idx];
    let manifest =
        crate::author::models::schema::Manifest::load(&crate::output::data_root().join("schema.json"))
            .map_err(CallError::Bad)?;
    let model_schema = manifest
        .model(&model)
        .ok_or_else(|| CallError::Bad(format!("{model} is not in schema.json")))?;
    let takes = crate::author::grade::takes_for(
        std::path::Path::new(EXAMPLES_DIR),
        std::path::Path::new(SONGS),
        &model,
        model_schema,
        real_schema,
    );
    // The same guard the studio button applies: an empty run must not
    // erase a grade a previous, real run produced.
    let standing = metal_reel::grade::Colourist::load(root, &model, real_schema);
    if takes.is_empty() && standing.trained() {
        return Err(CallError::Bad(format!(
            "refusing: no authored examples found for {model} and there is a \
             trained grade on disk. Nothing was overwritten."
        )));
    }
    let fitted = metal_reel::grade::train(&model, real_schema, &takes);
    let (songs, n_takes, error) = (fitted.songs, fitted.takes, fitted.error);
    let text = serde_json::to_string_pretty(&fitted).map_err(|e| CallError::Bad(e.to_string()))?;
    // `weights` names the grade file the source reads; absent, the model's
    // own default grade (the studio's file) is the one written.
    let path = match &weights {
        Some(name) => root.join(metal_reel::index::DIR).join(format!("{name}.json")),
        None => metal_reel::grade::model_path(root, &model),
    };
    if let Some(parent) = path.parent() {
        metal_vfs::create_dir_all(parent).map_err(|e| CallError::Bad(e.to_string()))?;
    }
    crate::atomic::write(path, text).map_err(|e| CallError::Bad(e.to_string()))?;
    let reply = serde_json::json!({
        "node": node,
        "kind": kind,
        "weights": weights,
        "trained": true,
        "songs": songs,
        "takes": n_takes,
        "error": error.map(|(ours, base)| serde_json::json!({ "ours": ours, "base": base })),
    });
    Ok((reply, vec![model]))
}

/// Fits the rhythm finder from the approved loop labels under `examples` and
/// writes it to `<root>/<index::DIR>/loopfind.json`. No labelled ticks is an
/// error, not an empty fit: an empty run must not replace a real one.
#[cfg(feature = "gpu")]
fn train_loops_at(
    root: &std::path::Path,
    node: &str,
    examples: &std::path::Path,
    songs: &std::path::Path,
) -> Result<Value, CallError> {
    let takes = crate::author::dataset::loop_takes_for(examples, songs);
    if takes.is_empty() {
        return Err(CallError::Bad(format!(
            "no approved loop labels with a digest under {}. Nothing was overwritten.",
            examples.display()
        )));
    }
    let fitted = metal_reel::loopfind::LoopFinder::train(&takes);
    fitted.save(root).map_err(CallError::Bad)?;
    Ok(serde_json::json!({
        "node": node,
        "kind": metal_graph::AI_LOOPS_KIND,
        "trained": {
            "songs": fitted.songs,
            "takes": fitted.takes,
            "positives": fitted.positives,
            "negatives": fitted.negatives,
        },
    }))
}

#[cfg(not(feature = "gpu"))]
fn train_at(
    _root: &std::path::Path,
    _node: &str,
    _graph: Option<&str>,
    _ids: &[&str],
    _schemas: &[&metal_schema::Schema],
) -> Result<(Value, Vec<String>), CallError> {
    Err(CallError::Bad(
        "training a source needs the gpu build (the model schemas come from the renderer)".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_agent::harness::TOOL_CATALOG;

    /// Regla 6/7's own parity mechanism, at its source: `GET /ai-config`'s
    /// `tools` array has to be *exactly* `TOOL_CATALOG`, in order, name for
    /// name — this is what lets `shell/kittest.rs`'s sidebar test compare
    /// against the door's reply instead of importing `metal_agent` itself
    /// (gate 1 bans that import in `shell/`; this module, one layer below
    /// the door, is the one place allowed to check the two actually match).
    #[test]
    fn ai_tools_reply_mirrors_the_harness_catalog_exactly() {
        let path =
            std::env::temp_dir().join(format!("fw-ai-tools-parity-{}.json", std::process::id()));
        let _ = metal_vfs::remove_file(&path);
        let reply = crate::ai_tools_reply(&path);
        let tools = reply["tools"].as_array().expect("tools array");
        assert_eq!(
            tools.len(),
            TOOL_CATALOG.len(),
            "every catalog tool needs exactly one sidebar control"
        );
        for ((name, desc, implemented), t) in TOOL_CATALOG.iter().zip(tools) {
            assert_eq!(t["name"], serde_json::json!(name));
            assert_eq!(t["description"], serde_json::json!(desc));
            assert_eq!(t["implemented"], serde_json::json!(implemented));
            // A fresh install (no `ai_tools.json` written yet) enables every
            // implemented tool and no unimplemented one.
            assert_eq!(t["enabled"], serde_json::json!(*implemented));
        }
    }

    /// `POST /ai-config {"permissions": ...}` and `{"instructions": ...}` land
    /// in `ai_tools.json` and come back on the next `GET`'s reply; a capability
    /// nobody answered reads `ask`, and a deny really removes its tool.
    #[test]
    fn permissions_and_instructions_round_trip_through_the_tools_file() {
        let path = std::env::temp_dir()
            .join(format!("fw-ai-permissions-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let reply = crate::ai_tools_reply(&path);
        assert_eq!(reply["permissions"]["dispositivos"], serde_json::json!("ask"));
        assert_eq!(reply["instructions"], serde_json::json!(""));

        let body = serde_json::json!({
            "permissions": { "dispositivos": "deny", "internet": "allow", "bogus": "x" },
            "instructions": "no uses dispositivos sin preguntarme",
        });
        let reply = crate::ai_tools_post_at(&path, &body).expect("permissions never error");
        assert_eq!(reply["permissions"]["dispositivos"], serde_json::json!("deny"));
        assert_eq!(reply["permissions"]["internet"], serde_json::json!("allow"));
        assert_eq!(reply["permissions"]["archivos"], serde_json::json!("ask"));
        assert_eq!(
            reply["instructions"],
            serde_json::json!("no uses dispositivos sin preguntarme")
        );

        let aim = reply["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "aim_fixture")
            .expect("aim_fixture is in the catalog");
        assert_eq!(aim["policy"], serde_json::json!("off"), "a denied capability removes its tool");
        assert_eq!(aim["enabled"], serde_json::json!(false));
        let _ = std::fs::remove_file(&path);
    }

    /// The inverse half of regla 6: every control the sidebar can fire maps
    /// to a route that is actually registered — never a route that only
    /// exists in this table's imagination. The documented `None` gap list
    /// (`crate::tool_route`'s own doc comment) is exempt by construction:
    /// nothing is asserted about a tool with no mapped route.
    ///
    /// `#[cfg(feature = "gpu")]`: `crate::call::author` — the module every
    /// `/author/…`-mapped tool route checks against — is itself gated
    /// behind `gpu` (`call/mod.rs`), the same fence
    /// `author::models::agent_tools` sits behind; a `--no-default-features`
    /// build has none of those routes to be out of parity with.
    #[test]
    #[cfg(feature = "gpu")]
    fn every_mapped_tool_route_is_actually_registered() {
        for (name, _desc, _implemented) in TOOL_CATALOG {
            let Some((method, path)) = crate::tool_route(name) else {
                continue;
            };
            let found = match path.strip_prefix("/author") {
                Some(rest) => crate::call::author::AUTHOR_ROUTES
                    .iter()
                    .any(|r| r.method == method && r.path == rest),
                // `POST /agent/tool` is answered by `call::agent_intercept`, ahead of ROUTES.
                None if path == crate::call::AGENT_TOOL_ROUTE => method == "POST",
                None => crate::call::ROUTES
                    .iter()
                    .any(|r| r.method == method && r.path == path),
            };
            assert!(
                found,
                "{name} maps to {method} {path}, which is not a registered route"
            );
        }
    }

    #[test]
    fn toggling_an_unknown_tool_on_is_refused_silently() {
        let path =
            std::env::temp_dir().join(format!("fw-ai-tools-refuse-{}.json", std::process::id()));
        let _ = metal_vfs::remove_file(&path);
        // E2 landed `capture_output`, the last `implemented: false` entry, so
        // the silent refusal is exercised with a name the catalog never held.
        let body = serde_json::json!({ "tools": { "no_such_tool": true } });
        let reply = crate::ai_tools_post_at(&path, &body).expect("toggling never errors");
        assert!(
            reply["tools"]
                .as_array()
                .unwrap()
                .iter()
                .all(|t| t["name"] != "no_such_tool"),
            "an unknown tool is not offered"
        );
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_default()).unwrap_or_default();
        assert!(
            saved["policy"].get("no_such_tool").is_none(),
            "an unknown tool cannot be turned on"
        );
    }

    /// E2: the last unimplemented catalog tool now lands — flipping it on is
    /// accepted, not silently refused.
    #[test]
    fn toggling_capture_output_on_is_accepted() {
        let path =
            std::env::temp_dir().join(format!("fw-ai-tools-capture-{}.json", std::process::id()));
        let _ = metal_vfs::remove_file(&path);
        let body = serde_json::json!({ "tools": { "capture_output": true } });
        let reply = crate::ai_tools_post_at(&path, &body).expect("toggling never errors");
        let capture_output = reply["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "capture_output")
            .unwrap();
        assert_eq!(
            capture_output["enabled"], true,
            "capture_output is implemented and can be turned on"
        );
    }

    #[test]
    fn toggling_an_implemented_tool_off_and_back_on_round_trips() {
        let path =
            std::env::temp_dir().join(format!("fw-ai-tools-toggle-{}.json", std::process::id()));
        let _ = metal_vfs::remove_file(&path);
        let off =
            crate::ai_tools_post_at(&path, &serde_json::json!({ "tools": { "sleep": false } }))
                .unwrap();
        let sleep_off = off["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "sleep")
            .unwrap();
        assert_eq!(sleep_off["enabled"], false);

        let on = crate::ai_tools_post_at(&path, &serde_json::json!({ "tools": { "sleep": true } }))
            .unwrap();
        let sleep_on = on["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "sleep")
            .unwrap();
        assert_eq!(sleep_on["enabled"], true);
    }

    /// `POST /ai/train { node }` on a `gen.*` source writes the very file the
    /// studio's train button writes: same takes (`takes_for`), same fit
    /// (`grade::train`), same path (`model_path`), same pretty JSON — so the
    /// graph gesture and the lane are one product, not two.
    #[test]
    #[cfg(feature = "gpu")]
    fn training_from_the_node_writes_the_same_file_as_the_studio_button() {
        use crate::author::models::EXAMPLES_DIR;
        use crate::author::SONGS;
        use std::path::Path;

        let tmp = std::env::temp_dir().join(format!("fw-ai-train-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&tmp);
        metal_vfs::create_dir_all(&tmp).unwrap();

        let paint = crate::paint::painter();
        let ids = paint.ids();
        let schemas = paint.schemas();
        let model = ids[0];
        let real_schema = schemas[0];

        let mut g = metal_graph::Graph::new("ai-train", "t", metal_graph::Domain::Live);
        let gen = g.add_node("gen", &format!("gen.{model}"), [2.0, 0.0]);
        crate::save_graph_at(&tmp, &g).expect("saves the graph");

        let (reply, trained) =
            train_at(&tmp, &gen, Some("ai-train"), &ids, &schemas).expect("trains from the source");
        assert_eq!(trained, vec![model.to_string()]);
        assert_eq!(reply["kind"], serde_json::json!(format!("gen.{model}")));
        let path = metal_reel::grade::model_path(&tmp, model);
        let from_node = metal_vfs::read(&path).expect("the node wrote a grade");

        // The studio button's own three calls, against the same root. Its
        // manifest is the same `data/schema.json` the app reads at runtime
        // (resolved from the crate, not the cwd — `output::data_root`).
        let manifest = crate::author::models::schema::Manifest::load(
            &crate::output::data_root().join("schema.json"),
        )
        .expect("schema.json");
        let model_schema = manifest.model(model).expect("the model is in the manifest");
        let takes = crate::author::grade::takes_for(
            Path::new(EXAMPLES_DIR),
            Path::new(SONGS),
            model,
            model_schema,
            real_schema,
        );
        let fitted = metal_reel::grade::train(model, real_schema, &takes);
        metal_reel::grade::Colourist::from_grade(fitted)
            .save(&tmp)
            .expect("the studio save");
        let from_studio = metal_vfs::read(&path).expect("the studio wrote a grade");

        assert_eq!(from_node, from_studio, "one gesture, one file — byte for byte");
        let _ = metal_vfs::remove_dir_all(&tmp);
    }

    /// The rhythm finder's route refuses an empty run, and writes nothing: no
    /// labelled ticks must never replace a fit a previous run produced.
    #[test]
    #[cfg(feature = "gpu")]
    fn the_loops_trainer_refuses_an_empty_run_and_writes_nothing() {
        let tmp = std::env::temp_dir().join(format!("fw-ai-loops-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&tmp);
        let examples = tmp.join("dataset").join("loops").join("examples");
        metal_vfs::create_dir_all(&examples).unwrap();
        let v = serde_json::json!({
            "model": "gradient", "song": "no-digest", "rate": 32.0, "ticks": [],
            "part": "loop-1", "span": [0, 9], "kind": "loop", "loop_beats": 2.0,
        });
        metal_vfs::write(examples.join("loop-1.json"), v.to_string()).unwrap();
        metal_vfs::write(
            tmp.join("dataset").join("loops").join("status.json"),
            r#"{"loop-1": "approved"}"#,
        )
        .unwrap();
        let songs = tmp.join("songs");
        metal_vfs::create_dir_all(&songs).unwrap();

        let err = train_loops_at(&tmp, "ai-loops-0", &examples, &songs).unwrap_err();
        assert!(matches!(err, CallError::Bad(ref m) if m.contains("Nothing was overwritten")));
        assert!(!metal_reel::loopfind::model_path(&tmp).exists());
        let _ = metal_vfs::remove_dir_all(&tmp);
    }
}

/// `POST /ai/chat/compact {id}` — the context bar's compact button
/// ([`crate::chat::compact_chat_at`]).
pub fn chat_compact(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("expected json".into()))?;
    let id = body["id"].as_str().filter(|id| !id.is_empty()).ok_or_else(|| CallError::Bad("expected a non-empty `id`".into()))?;
    crate::chat::compact_chat_at(&crate::output::data_root(), id).map_err(CallError::Bad)
}

/// `GET /ai/models` — the chat's command sheet: the provider's live model
/// list ([`metal_agent::config::list_models`], the same read as
/// `/ai-models`), plus the model in use and its context window.
pub fn chat_models(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let cfg = metal_agent::config::ProviderConfig::load_at(
        crate::ai_config_path(),
        &metal_agent::config::ProviderConfig::default_config_toml(),
    )
    .ok()
    .flatten();
    let Some(cfg) = cfg else {
        return Ok(serde_json::json!({ "models": [], "current": "", "error": "no provider saved" }));
    };
    let window = metal_agent::config::context_window(&cfg.model);
    Ok(match metal_agent::config::list_models(cfg.preset, &cfg.url, &cfg.key) {
        Ok(mut models) => {
            models.sort();
            serde_json::json!({ "models": models, "current": cfg.model, "window": window })
        }
        Err(e) => serde_json::json!({ "models": [], "current": cfg.model, "window": window, "error": e }),
    })
}
