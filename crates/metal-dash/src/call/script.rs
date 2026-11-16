//! The script code editor's route surface (`§9.4 V3`): `GET /script/<node>`
//! reads one `script.py`/`script.rs`/`script.js` node's source, and
//! `POST /script {graph, node, source}` is the **one writer** of that node's
//! `config.code` — it runs the source through `metal_perform::script`'s own
//! runtime before it writes, so a script that raises, hangs or fails to
//! compile comes back `{ok: false, error}` and **the node is left as it was**.
//!
//! The operator's editor (`shell/panels/properties/script.rs`) and the AI's
//! `read_script`/`save_script` tools both go through this route (regla 6), so a
//! source the model saves and one the operator saves pass the same gate. That
//! gate is the same [`metal_perform::script::run_lang`] the engine calls every
//! frame (`script::evaluate`), so "it saved" and "it ran last frame" cannot
//! disagree about what the runtime thinks of the source.
//!
//! **The source is not a file.** A script node's code lives in its
//! `Node.config.code` (rule 4: config is a reading of the graph), so this route
//! writes through `metal_perform::engine`'s `load_graph`/`save_graph` — the
//! same validated graph writer `POST /graph/node` uses — never a file of its
//! own.

use std::path::Path;

use serde_json::{json, Value};

use crate::call::{CallError, Ctx, Params};

/// `metal_perform::script::Lang` as the wire's short string: what the editor's
/// language badge shows and what a caller reads back. `None` for a kind that
/// is not one of the three interpreted runtimes.
fn lang_wire(lang: metal_perform::script::Lang) -> &'static str {
    match lang {
        metal_perform::script::Lang::Python => "py",
        metal_perform::script::Lang::JavaScript => "js",
        metal_perform::script::Lang::Rust => "rs",
    }
}

/// `GET /script/<node>?graph=<id>` — one script node's `{lang, source}`. The
/// node is the path tail, or `?node=` when the caller (the MCP adapter, which
/// forwards a tool's arguments as the query) names it that way.
pub fn get(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let node = params
        .tail
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| params.param("node").filter(|s| !s.is_empty()));
    let Some(node_id) = node else {
        return Err(CallError::Bad("which node".into()));
    };
    let Some(gid) = params.param("graph").filter(|s| !s.is_empty()) else {
        return Err(CallError::Bad("which graph".into()));
    };
    get_at(&metal_perform::output::data_root(), gid, node_id)
}

/// `POST /script {graph, node, source}` — validate, then write. The one
/// writer: a source the runtime refuses leaves the node's config alone.
pub fn save(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let Some(gid) = body["graph"].as_str().filter(|s| !s.is_empty()) else {
        return Err(CallError::Bad("which graph".into()));
    };
    let Some(node_id) = body["node"].as_str().filter(|s| !s.is_empty()) else {
        return Err(CallError::Bad("which node".into()));
    };
    let Some(source) = body["source"].as_str() else {
        return Err(CallError::Bad("which source".into()));
    };
    save_at(&metal_perform::output::data_root(), gid, node_id, source)
}

/// The read, over an explicit root so the route and the tests share one body
/// (rule 8: a test builds its fixture in a tempdir).
fn get_at(root: &Path, gid: &str, node_id: &str) -> Result<Value, CallError> {
    let g = metal_perform::engine::load_graph_at(root, gid).map_err(CallError::Bad)?;
    let node = g.node(node_id).ok_or_else(|| CallError::Bad(format!("no such node: {node_id}")))?;
    let lang = metal_perform::script::Lang::from_kind(&node.kind)
        .ok_or_else(|| CallError::Bad(format!("{} is not a script node", node.kind)))?;
    let source = node.config.get("code").and_then(Value::as_str).unwrap_or_default();
    Ok(json!({ "lang": lang_wire(lang), "source": source }))
}

/// The write, over an explicit root: load the graph, run the source through
/// the runtime the engine uses, and only then set `config.code` and save.
fn save_at(root: &Path, gid: &str, node_id: &str, source: &str) -> Result<Value, CallError> {
    let mut g = metal_perform::engine::load_graph_at(root, gid).map_err(CallError::Bad)?;
    let Some(node) = g.node(node_id) else {
        return Err(CallError::Bad(format!("no such node: {node_id}")));
    };
    let kind = node.kind.clone();
    let lang = metal_perform::script::Lang::from_kind(&kind)
        .ok_or_else(|| CallError::Bad(format!("{kind} is not a script node")))?;
    // The floats the script is handed, read off the node's own config the same
    // way `script::evaluate` reads them (rule 4).
    let config: metal_perform::script::ScriptConfig = serde_json::from_str(&node.config.to_json_str())
        .unwrap_or_default();
    // Run it under the real budget. A raise, a hang or a compile failure comes
    // back as this node's own runtime error, not a write.
    if let Err(e) = metal_perform::script::run_lang(lang, source, &config.values(), &[]) {
        return Ok(json!({ "ok": false, "error": e.to_string() }));
    }
    // Validated: set `code` on the node's config object, keeping every key the
    // editor does not own (floats, loop, …) — never a blind replace.
    let Some(node) = g.node_mut(node_id) else {
        return Err(CallError::Bad(format!("no such node: {node_id}")));
    };
    let mut current: serde_json::Map<String, Value> =
        serde_json::from_str::<Value>(&node.config.to_json_str())
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
    current.insert("code".to_string(), json!(source));
    node.config = metal_graph::Config::from_json_str(&Value::Object(current).to_string())
        .map_err(|e| CallError::Bad(e.to_string()))?;
    metal_perform::engine::save_graph_at(root, &g).map_err(CallError::Bad)?;
    Ok(json!({ "ok": true, "lang": lang_wire(lang), "source": source }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("metal-dash-script-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        dir
    }

    fn saved_script(root: &Path, gid: &str, node: &str, kind: &str, code: &str) -> String {
        let mut g = metal_graph::Graph::new(gid, gid, metal_graph::Domain::Live);
        let id = g.add_node(node, kind, [0.0, 0.0]);
        let Some(n) = g.node_mut(&id) else { unreachable!() };
        n.config = metal_graph::Config::from_json_str(&json!({ "code": code }).to_string()).unwrap();
        metal_perform::engine::save_graph_at(root, &g).unwrap();
        id
    }

    /// Round-trip: a `POST /script` a script that runs accepts is written, and
    /// `GET /script/<node>` reads the exact source back with its language.
    #[test]
    fn post_script_then_get_script_round_trips() {
        let root = temp_root("roundtrip");
        let good = "def process(floats, row):\n    return [1.0]\n";
        let id = saved_script(&root, "g1", "py", "script.py", "");
        let reply = save_at(&root, "g1", &id, good).expect("a normal result");
        assert_eq!(reply["ok"], json!(true), "{reply}");
        assert_eq!(reply["lang"], json!("py"), "{reply}");
        let read = get_at(&root, "g1", &id).expect("reads back");
        assert_eq!(read["lang"], json!("py"));
        assert_eq!(read["source"], json!(good));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A bad source comes back with the runtime's own error and **does not
    /// touch the node**: the graph on disk still carries the last good source.
    #[test]
    fn post_script_refuses_a_bad_source_without_corrupting_the_node() {
        let root = temp_root("bad");
        let good = "def process(floats, row):\n    return [1.0]\n";
        let id = saved_script(&root, "g1", "py", "script.py", good);
        let reply = save_at(&root, "g1", &id, "def process(:\n").expect("a normal result");
        assert_eq!(reply["ok"], json!(false), "{reply}");
        assert!(reply["error"].as_str().is_some_and(|e| !e.is_empty()), "{reply}");
        let read = get_at(&root, "g1", &id).expect("the node is intact");
        assert_eq!(read["source"], json!(good), "the bad source was never written");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A source that does not define `process` raises at call time and is
    /// refused the same way.
    #[test]
    fn post_script_refuses_a_source_that_raises() {
        let root = temp_root("raised");
        let id = saved_script(&root, "g1", "py", "script.py", "");
        let reply = save_at(&root, "g1", &id, "x = 1\n").expect("a normal result");
        assert_eq!(reply["ok"], json!(false), "{reply}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A non-script node is refused by name, not silently written.
    #[test]
    fn post_script_refuses_a_non_script_node() {
        let root = temp_root("kind");
        let mut g = metal_graph::Graph::new("g1", "g1", metal_graph::Domain::Live);
        let id = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        metal_perform::engine::save_graph_at(&root, &g).unwrap();
        assert!(matches!(
            save_at(&root, "g1", &id, "def process(floats, row): return []\n"),
            Err(CallError::Bad(_))
        ));
        let _ = std::fs::remove_dir_all(&root);
    }
}
