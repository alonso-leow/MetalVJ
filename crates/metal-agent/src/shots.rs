//! H.7: the few-shot corpus (`data/agent/shots.json`) and the system prompt
//! (`data/agent/system_prompt.md`), loaded and validated against the real
//! tool surface — [`harness::TOOL_CATALOG`] plus the `question` tool, which
//! H.3 treats as "a tool like any other" but which is not in
//! [`harness::TOOL_CATALOG`] because [`harness::Harness`] never executes it
//! (`agent_loop`'s own doc: it is "up to the caller... to special-case it").
//!
//! **Why validate here and not just eyeball the JSON.** H.9's own
//! verification list is explicit: *"every tool in the registry has ≥2 shots;
//! every shot validates against its tool's schema; a shader with no declared
//! floats is rejected; an incompatible `connect_nodes` is rejected."*
//! This module implements the first two checks in Rust so they run under
//! `cargo test` instead of a separate `check-agent.mjs` — the corpus and its
//! loader live in the same crate as the tools it demonstrates, so a
//! signature change (a renamed argument, say) fails the build here instead of
//! silently drifting from a JS-side checker nobody remembers to update.
//!
//! **Per-tool argument shapes.** `metal-agent` has no formal JSON Schema for its
//! tools (I.3 calls for one — see that section's "typed IR" — but nothing
//! in this pass builds it). What exists instead is `harness.rs`'s own
//! `require_str`/`.get(...)` calls, which *are* the schema, just not
//! reified. [`tool_schema`] below is a hand-transcription of exactly those
//! calls (plus H.4's own table, `the design notes`, for the five tools
//! `harness.rs` does not implement yet) — required/optional keys only, no
//! type checking beyond "is this key present". That is deliberately the
//! same weight as `harness.rs`'s own validation: this module should reject
//! what the real tool would reject, not more.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::harness::TOOL_CATALOG;

#[derive(Debug, Deserialize)]
pub struct Corpus {
    #[serde(default, rename = "_comment")]
    pub _comment: String,
    pub shots: Vec<Shot>,
}

#[derive(Debug, Deserialize)]
pub struct Shot {
    pub id: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub user: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
pub struct Step {
    #[serde(default)]
    pub assistant: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallShot>,
}

#[derive(Debug, Deserialize)]
pub struct ToolCallShot {
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

/// Loads and parses `path` (normally `data/agent/shots.json`). Parsing alone
/// is not validation — call [`validate`] on the result before trusting it.
pub fn load(path: impl AsRef<Path>) -> Result<Corpus, String> {
    let text = fs::read_to_string(path.as_ref())
        .map_err(|e| format!("cannot read {}: {e}", path.as_ref().display()))?;
    serde_json::from_str(&text)
        .map_err(|e| format!("{} did not parse: {e}", path.as_ref().display()))
}

/// `question` is H.3's third response shape, learned by example like any
/// other tool call (`agent_loop`'s own doc comment), but it is deliberately
/// absent from `harness::TOOL_CATALOG` because `Harness` never executes it —
/// see that module's doc. It still needs a schema here so shots that use it
/// validate.
const QUESTION_TOOL: &str = "question";

/// Required argument keys per tool, transcribed from `harness.rs`'s own
/// `require_str`/`.get(...)` calls for the twelve implemented tools, and
/// from H.4's table (`the design notes`) for the five that are not
/// implemented yet but do have a documented shape. `graph` is omitted from
/// tools that accept it under either `graph` or `path` (`list_nodes`) —
/// those are handled specially in [`check_required_keys`].
/// The argument names a tool requires, as [`check_required_keys`] reads
/// them — public so the chat's tool specs name them too.
pub fn tool_args(name: &str) -> Option<&'static [&'static str]> {
    tool_schema(name)
}

fn tool_schema(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "list_tools" => Some(&[]),
        "get_env" => Some(&["graph"]),
        "list_nodes" => Some(&[]), // path|graph, checked specially
        "search_nodes" => Some(&["q"]),
        "create_node" => Some(&["graph", "kind"]),
        "delete_node" => Some(&["graph", "id"]),
        "connect_nodes" => Some(&["graph", "from", "to"]),
        "get_params" => Some(&["graph", "id"]),
        "list_kinds" => Some(&[]), // kind|filter|domain, all optional
        "focus" => Some(&["items"]),
        "set_params" => Some(&["graph", "id", "patch"]),
        "read_shader" => Some(&[]), // id|name, checked specially
        "raster" => Some(&[]),      // shader|shader_id, checked specially
        "sleep" => Some(&["ms"]),
        // H.4's table, for the five not implemented by `Harness` yet:
        "capture_camera_frame" => Some(&[]),
        // H.5c: `step` discriminates grid/crop/resolve/verify; each step's
        // own required keys are checked inside `harness::Harness::locate`
        // itself (`png_base64`+`n` for grid, `region` for crop/verify,
        // `n`+`cols`+`rows` for resolve) — same weight this module already
        // gives `read_shader`/`raster`'s either-of shapes, just not worth a
        // fifth `check_either_of` arm for one required common key.
        "locate" => Some(&["step"]),
        "create_mask_grid" => Some(&["outputs", "rows", "cols"]),
        "test_mask_grid" => Some(&["outputs"]),
        // O.1's authoring half (`POST /outputs { op: "mask" }`): the address
        // is the device; `mask_id` names an existing mask (absent creates
        // one) and every field is optional — the tool refuses a call that
        // names none, same as `set_layer_mix`.
        "edit_mask" => Some(&["device_id"]),
        // G1: graph effects name their graph, node and effect; a fixed amount
        // and the source are optional. `edit_output` names its op and id,
        // `resolve_draft` its venue and decision.
        "add_effect" => Some(&["graph", "node", "effect", "state"]),
        "remove_effect" => Some(&["graph", "node", "effect"]),
        "edit_output" => Some(&["op", "id"]),
        "resolve_draft" => Some(&["venue", "decision"]),
        // P3: a venue surface is named by its venue and surface; every other
        // key (name/points/origin/u/v/material/delete) is optional. A pin set
        // names the projector too, and carries the four corners.
        "edit_venue_surface" => Some(&["venue", "surface"]),
        // Venue 3D: the object/screen/projector id is the only required key;
        // the venue is the active one. A swept form names its surface and
        // form; a move names its item; the reads take a projector or nothing.
        "edit_venue_object" => Some(&["object"]),
        "add_screen" => Some(&["screen"]),
        "add_projection" => Some(&["projector"]),
        "set_surface_form" => Some(&["surface", "form"]),
        "set_venue_sectors" => Some(&["surface"]),
        "move_venue_item" => Some(&["item"]),
        "projection_area" => Some(&["projector"]),
        "venue_scene" => Some(&[]),
        // Venue 3D second batch: the object id (stage/truss/dummy/spline) or
        // the two operands; a hit needs an origin plus a dir or target.
        "add_stage" => Some(&["stage"]),
        "add_truss" => Some(&["truss"]),
        "add_dummy" => Some(&["dummy"]),
        "venue_spline" => Some(&["object", "path"]),
        "place_mask_in_space" => Some(&["mask", "on"]), // points optional on delete
        "venue_boolean" => Some(&["a", "b", "operation"]),
        "venue_hit" => Some(&["origin"]), // dir|target checked in the handler
        "venue_report" | "venue_view" => Some(&[]),
        "venue_assets_search" => Some(&[]),
        "venue_asset_import" => Some(&["object", "path", "licence"]),
        // D1: a fixture is named by its venue; `id` (absent adds) and every
        // mapping field are optional — the same "which venue" shape the
        // surface tool has.
        "set_fixture" => Some(&["venue"]),
        // F9: the per-fixture node is named by its fixture (the venue is the
        // active one); `colour_from`/`model`/`pick`/`offset` are optional.
        "add_fixture_node" => Some(&["fixture"]),
        "calibrate_projector" => Some(&["venue", "projector", "surface"]),
        // P-F: a surface (every projector pinned to it) or one projector.
        "blend_edges" => Some(&["venue"]),
        // S.AK/O.28 — the grid's switches: `list_switches` takes nothing
        // (it lists every saved switch, read fresh off the graph);
        // `set_switch` names which one and which side.
        "list_switches" => Some(&[]),
        "set_switch" => Some(&["id", "op"]),
        // R22.4 (regla 6) — the authoring half: `add_switch` carries the
        // whole body the grid's popover writes (`name`/`unset` optional),
        // `remove_switch` names the one to drop.
        "add_switch" => Some(&["id", "kind", "graph", "set"]),
        "remove_switch" => Some(&["id"]),
        // V2b — the machine tick level: the read takes nothing, the write names
        // the level (`base` | `fine`).
        "get_tick_level" => Some(&[]),
        "set_tick_level" => Some(&["tick_level"]),
        // The sync read tools (plan §5 item 6): `metal-sync`'s status, peers and
        // log, all read-only and all take nothing.
        "sync_status" | "sync_peers" | "sync_log" => Some(&[]),
        // P-D: both optional-only reads.
        "artnet_nodes" | "artnet_in_status" | "timecode_status" => Some(&[]),
        // P-E: takes nothing.
        "list_transitions" => Some(&[]),
        "capture_output" => Some(&["node"]),
        "bake_clip" => Some(&["source", "duration"]),
        // S.K, transcribed from `POST /timeline/segment`'s own body shape
        // (`metal-dash::author::models::mod::edit_segment`) and `set_keyframe`'s
        // read-modify-write over `POST /example`.
        "import_segment" => Some(&["timeline", "track", "segment"]),
        "move_segment" => Some(&["timeline", "track", "id", "from", "to"]),
        "remove_segment" => Some(&["timeline", "track", "id"]),
        "set_keyframe" => Some(&["model", "song", "tick", "floats"]),
        // S.AM — the train gesture of an `ai.*` node: the node id (the graph
        // is optional; `POST /ai/train` finds it when absent).
        "train_ai" => Some(&["node"]),
        // A1 — the embedded reaction: the source it lands in, which `ai.*`
        // it is, and `on` (the same body `POST /graph/node {op:"ai.embed"}`
        // takes; `on` decides embed vs. the `separar` wire value).
        "embed_ai" => Some(&["graph", "node", "ai", "on"]),
        // Surface sources: `attach` needs the node and its port; `insert`
        // needs the node it goes after and the kind.
        "attach_source" => Some(&["graph", "node", "port"]),
        "insert_node" => Some(&["graph", "after", "kind"]),
        // Decided 2026-09-05: runs the digest over the clip's
        // own audio, not over the timeline's song.
        "attach_digest" => Some(&["timeline", "segment_id"]),
        // S.O.1 — Overlay blackout: the room's off switch, `{on: bool}` —
        // matches `POST /overlay/blackout`'s own shape (no timer; see
        // `agent_tools.rs::blackout`'s doc for why not `duration`).
        "blackout" => Some(&["on"]),
        // S.O.2 — Solid color overlay: fixed color for N seconds
        "solid_color" => Some(&["rgb", "duration_seconds"]),
        "play_sequence" => Some(&["op"]),
        "edit_loops" => Some(&["op"]),
        "transport" => Some(&["action"]),
        "fire_trigger" => Some(&["id"]),
        "overlay_control" => Some(&["op"]),
        "colour_apply" => Some(&["graph"]),
        // S.D — one segment's `SegmentMix` (O.3): `mask`/`blend`/`opacity`
        // are each optional and only the keys present are written, so the
        // required shape is just "which segment"; the tool itself refuses a
        // call that names none of the three
        // (`metal_dash::author::models::agent_tools::AgentHarness::set_layer_mix`).
        "set_layer_mix" => Some(&["timeline", "segment_id"]),
        // S.O.5 — Rasterize timeline segment to overlay
        "rasterize_overlay" => Some(&["timeline", "track", "segment_id"]),
        // 3D venues: which venue (a file under `data/venues/` or an example
        // in `data/agent/venues/`); `device`, `projector`, `rgb`, `seconds`
        // and `monitor` are optional.
        "project_venue" => Some(&["venue"]),
        // Every argument optional: the venue projected last, its first
        // shot with a camera, the first camera that answers.
        "look_venue" => Some(&[]),
        // M2 of the pixel map: which fixture and the point to aim it at;
        // `venue` is optional (the one projected last).
        "aim_fixture" => Some(&["fixture", "target"]),
        // D2 — the GDTF importer: `path`/`bytes` are either-of (the tool
        // refuses a call that names neither); `name`/`mode` are optional.
        "import_gdtf" => Some(&[]),
        // M5 of the pixel map: every tool optional — the venue defaults to
        // the one projected last, the camera to the first that answers.
        "scan_venue" | "discover_fixture" => Some(&[]),
        // D5: solve_head needs the fixture and the point to aim it at; the
        // venue is optional (the one projected last).
        "solve_head" => Some(&["fixture", "target"]),
        "listen_status" => Some(&[]),
        // A runtime shader: its id (the file name) and its WGSL.
        "save_shader" => Some(&["id", "wgsl"]),
        // §9.4 V3 — a script node's own source: which graph, which node, and
        // (to write) the source itself.
        "read_script" => Some(&["graph", "node"]),
        "save_script" => Some(&["graph", "node", "source"]),
        // `POST /layers`: which op on which device; the rest is the op's.
        "edit_layer" => Some(&["op", "device"]),
        // Param edges (`metal_graph::bind`), the route bodies verbatim:
        // `GET /graph/io?id&node`, `POST /graph/node {op: "bind" | "param"}`.
        // `from` may be `null` (unbind) and `value` may be `null` (release),
        // but the key is always there.
        "get_io" => Some(&["graph", "node"]),
        "bind_param" => Some(&["graph", "node", "param", "from"]),
        "set_param" => Some(&["graph", "node", "name", "value"]),
        // M4 of the materials lote: which graph and which `mat.*` node; the
        // `part` and every override key are optional, so "just name the node"
        // is a legal call.
        "set_material" => Some(&["graph", "node"]),
        // K — the palette node's write: which stored palette, which slot, the
        // new [r, g, b], and whether the whole palette moves with it
        // (`relative`, optional and false by default).
        "set_palette" => Some(&["id", "slot", "value"]),
        // Video scenes: `clip` is optional on the read (none lists the book);
        // the write's other keys are the op's own.
        "list_scenes" => Some(&[]),
        // `part` is optional: none lists every part, one opens it.
        "list_dataset" => Some(&[]),
        "edit_scene" => Some(&["op"]),
        // H5 — the head-tracking gesture: `on` is required (`strength` is
        // optional, so "just turn it off" is a legal call).
        "set_head_tracking" => Some(&["on"]),
        // P-G — `node`/`dir` optional: no node is the device's own frame.
        "export_frame" => Some(&["device"]),
        // E1 — an offline export: the device (its graph), the frame range and
        // where the file goes; `node`/`fps`/`codec`/`bitrate` fall back to the
        // graph's first generator and the default preset.
        "export_video" => Some(&["device", "range", "path"]),
        // P-C — `target` optional (a default name / the first loopback).
        "list_video_io" => Some(&[]),
        "perf_report" => Some(&[]),
        "get_grid_layouts" => Some(&[]),
        "set_grid_layouts" => Some(&["grid_layouts"]),
        "export_status" => Some(&[]),
        "export_control" => Some(&["op"]),
        "list_palettes" => Some(&[]),
        "set_output_sink" => Some(&["device_id", "kind"]),
        // D3 — the universe bus's merge priority/mode: every key optional.
        "dmx_bus" => Some(&[]),
        // O.8 — one owner per graph: take, release, ask who.
        "take_graph" | "release_graph" | "graph_owner" => Some(&["graph"]),
        QUESTION_TOOL => Some(&["question", "options"]),
        _ => None,
    }
}

/// Tools whose required shape is "one of several keys", not a fixed list —
/// `check_required_keys` special-cases these instead of forcing
/// [`tool_schema`] to express an OR.
fn check_either_of(name: &str, args: &serde_json::Value) -> Option<Result<(), String>> {
    let has = |k: &str| args.get(k).is_some();
    match name {
        "list_nodes" => Some(if has("path") || has("graph") {
            Ok(())
        } else {
            Err("list_nodes needs `path` or `graph`".to_string())
        }),
        "read_shader" => Some(if has("id") || has("name") {
            Ok(())
        } else {
            Err("read_shader needs `id` or `name`".to_string())
        }),
        "raster" => Some(if has("shader") || has("shader_id") {
            Ok(())
        } else {
            Err("raster needs `shader` or `shader_id`".to_string())
        }),
        _ => None,
    }
}

fn check_required_keys(name: &str, args: &serde_json::Value) -> Result<(), String> {
    if let Some(result) = check_either_of(name, args) {
        return result;
    }
    let required = tool_schema(name).ok_or_else(|| format!("unknown tool: {name}"))?;
    for key in required {
        if args.get(key).is_none() {
            return Err(format!("{name} call is missing required key `{key}`"));
        }
    }
    Ok(())
}

/// The set of tool names a corpus is expected to cover: every tool in
/// [`harness::TOOL_CATALOG`] (implemented or not — H.7's mandatory cases
/// explicitly want the blocked camera/mask tools shown too) plus
/// [`QUESTION_TOOL`].
pub fn known_tool_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = TOOL_CATALOG.iter().map(|(n, _, _)| *n).collect();
    names.push(QUESTION_TOOL);
    names
}

/// Full validation of a parsed [`Corpus`]:
///
/// - every `tool_calls[].name` is a known tool (typo/invented-tool guard);
/// - every call's `arguments` has the required keys for that tool;
/// - every known tool name is exercised by at least `min_per_tool` shots
///   (H.7: "at least two examples per key" — counted per *shot*, not per
///   call, so one shot hammering the same tool five times does not count as
///   five examples of it).
///
/// Returns `Ok(coverage)` mapping tool name -> number of distinct shots that
/// used it, so a caller (or a test) can report exactly which tool is short
/// rather than a single pass/fail bit — the same "a list of facts, not a
/// bool" judgment `metal-graph::lint` already makes, applied here to corpus
/// coverage instead of graph edges.
pub fn validate(
    corpus: &Corpus,
    min_per_tool: usize,
) -> Result<BTreeMap<String, usize>, Vec<String>> {
    let mut errors = Vec::new();
    let mut coverage: BTreeMap<String, usize> = BTreeMap::new();

    for shot in &corpus.shots {
        let mut tools_in_this_shot: Vec<String> = Vec::new();
        for step in &shot.steps {
            for call in &step.tool_calls {
                if let Err(e) = check_required_keys(&call.name, &call.arguments) {
                    errors.push(format!("shot {:?}: {e}", shot.id));
                    continue;
                }
                if !tools_in_this_shot.contains(&call.name) {
                    tools_in_this_shot.push(call.name.clone());
                }
            }
        }
        for name in tools_in_this_shot {
            *coverage.entry(name).or_insert(0) += 1;
        }
    }

    for name in known_tool_names() {
        let count = coverage.get(name).copied().unwrap_or(0);
        if count < min_per_tool {
            errors.push(format!(
                "tool {name:?} has only {count} shot(s), needs at least {min_per_tool}"
            ));
        }
    }

    if errors.is_empty() {
        Ok(coverage)
    } else {
        Err(errors)
    }
}

/// Loads the system prompt file (`data/agent/system_prompt.md`) verbatim —
/// it is Markdown, not data, so there is nothing to parse, only to read and
/// hand to the provider as the `system` message. Exists so callers do not
/// have to know the path convention (`data/agent/`) themselves.
pub fn load_system_prompt(path: impl AsRef<Path>) -> Result<String, String> {
    fs::read_to_string(path.as_ref())
        .map_err(|e| format!("cannot read {}: {e}", path.as_ref().display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolves `data/agent/<name>` from the workspace root regardless of
    /// which directory `cargo test` happens to run from for this crate
    /// (`CARGO_MANIFEST_DIR` is `crates/metal-agent`, so `../../data/agent` is
    /// the workspace's `data/agent`).
    fn data_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/agent")
            .join(name)
    }

    fn load_corpus() -> Corpus {
        load(data_path("shots.json")).expect("shots.json should parse")
    }

    #[test]
    fn shots_json_parses() {
        let corpus = load_corpus();
        assert!(!corpus.shots.is_empty());
    }

    #[test]
    fn every_shot_id_is_unique() {
        let corpus = load_corpus();
        let mut seen = std::collections::HashSet::new();
        for shot in &corpus.shots {
            assert!(
                seen.insert(shot.id.clone()),
                "duplicate shot id: {}",
                shot.id
            );
        }
    }

    #[test]
    fn every_tool_call_validates_against_its_schema_and_every_tool_has_at_least_two_shots() {
        let corpus = load_corpus();
        match validate(&corpus, 2) {
            Ok(coverage) => {
                // Sanity: the mandatory H.7 cases actually landed as real
                // tool usage, not just prose.
                for must in [
                    "capture_camera_frame",
                    "create_mask_grid",
                    "test_mask_grid",
                    "question",
                ] {
                    assert!(
                        coverage.get(must).copied().unwrap_or(0) >= 2,
                        "{must} must have >=2 shots (H.7 mandatory case), got {:?}",
                        coverage.get(must)
                    );
                }
            }
            Err(errors) => panic!("corpus validation failed:\n{}", errors.join("\n")),
        }
    }

    #[test]
    fn an_unknown_tool_name_is_rejected() {
        let bad = Corpus {
            _comment: String::new(),
            shots: vec![Shot {
                id: "bad".to_string(),
                tags: vec![],
                user: "x".to_string(),
                steps: vec![Step {
                    assistant: String::new(),
                    tool_calls: vec![ToolCallShot {
                        name: "invent_a_tool".to_string(),
                        arguments: serde_json::json!({}),
                    }],
                }],
            }],
        };
        let err = validate(&bad, 0).unwrap_err();
        assert!(err.iter().any(|e| e.contains("unknown tool")), "{err:?}");
    }

    #[test]
    fn a_call_missing_a_required_key_is_rejected() {
        let bad = Corpus {
            _comment: String::new(),
            shots: vec![Shot {
                id: "missing-key".to_string(),
                tags: vec![],
                user: "x".to_string(),
                steps: vec![Step {
                    assistant: String::new(),
                    tool_calls: vec![ToolCallShot {
                        // create_node without `kind`.
                        name: "create_node".to_string(),
                        arguments: serde_json::json!({ "graph": "g1" }),
                    }],
                }],
            }],
        };
        let err = validate(&bad, 0).unwrap_err();
        assert!(
            err.iter().any(|e| e.contains("missing required key")),
            "{err:?}"
        );
    }

    /// D5: `solve_head` inverts the aim from a point, so it names the fixture
    /// and the target; a call missing either is rejected, and the corpus's
    /// own shots carry both.
    #[test]
    fn solve_head_needs_a_fixture_and_a_target() {
        assert_eq!(tool_args("solve_head"), Some(&["fixture", "target"][..]));
        assert!(known_tool_names().contains(&"solve_head"));
        assert!(
            check_required_keys("solve_head", &serde_json::json!({ "fixture": "wash-1" })).is_err()
        );
        assert!(check_required_keys(
            "solve_head",
            &serde_json::json!({ "target": [0.0, 1.0, 0.0] })
        )
        .is_err());
        assert!(check_required_keys(
            "solve_head",
            &serde_json::json!({ "fixture": "wash-1", "target": [0.0, 1.0, 0.0] })
        )
        .is_ok());
        // The corpus's two shots both name a fixture and a target.
        let corpus = load_corpus();
        let mut shots = 0;
        for shot in &corpus.shots {
            for step in &shot.steps {
                for call in &step.tool_calls {
                    if call.name == "solve_head" {
                        shots += 1;
                        assert!(call.arguments.get("fixture").is_some(), "{}", shot.id);
                        assert!(call.arguments.get("target").is_some(), "{}", shot.id);
                    }
                }
            }
        }
        assert!(shots >= 2, "solve_head must have >=2 shots, got {shots}");
    }

    #[test]
    fn coverage_below_the_minimum_is_reported_per_tool() {
        let one_shot = Corpus {
            _comment: String::new(),
            shots: vec![Shot {
                id: "only-one".to_string(),
                tags: vec![],
                user: "x".to_string(),
                steps: vec![Step {
                    assistant: String::new(),
                    tool_calls: vec![ToolCallShot {
                        name: "list_tools".to_string(),
                        arguments: serde_json::json!({}),
                    }],
                }],
            }],
        };
        let err = validate(&one_shot, 2).unwrap_err();
        assert!(
            err.iter()
                .any(|e| e.contains("\"list_tools\" has only 1 shot")),
            "{err:?}"
        );
    }

    #[test]
    fn system_prompt_loads_and_is_non_trivial() {
        let prompt = load_system_prompt(data_path("system_prompt.md"))
            .expect("system_prompt.md should read");
        assert!(
            prompt.len() > 500,
            "system prompt looks too short: {} bytes",
            prompt.len()
        );
        // It should actually be the harvested comments, not paraphrased prose
        // written fresh for this file — spot-check a couple of near-verbatim
        // phrases pulled from the real `//` comments cited alongside them.
        assert!(prompt.contains("un cuadro, no un") && prompt.contains("modelo"));
        assert!(prompt.contains("nunca una segunda") && prompt.contains("verdad"));
    }
}
