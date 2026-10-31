//! I.3's typed tool surface: the per-tool [`ToolMeta`] flags and the JSON
//! Schema each tool's arguments answer to. The authoritative name list is
//! [`crate::harness::TOOL_CATALOG`] (rule 7 forbids keeping a second,
//! hand-counted copy); the required keys come from [`crate::shots::tool_args`].
//! This module reifies both — it does not re-list the tools, it reads them.

use serde_json::{json, Map, Value};

use crate::harness::TOOL_CATALOG;

/// What policy needs to know about a tool: whether it only reads (safe to run
/// unprompted) and whether it changes the world (defaults to `ask`). See
/// `chat::policy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolMeta {
    pub readonly: bool,
    pub destructive: bool,
}

/// Tools that only read — never write a graph, file or route. Kept as a small
/// explicit set, not derived: "read-only" is a judgment about the side
/// effects, not something the catalog records.
const READONLY: &[&str] = &[
    "list_tools",
    "get_env",
    "list_nodes",
    "search_nodes",
    "get_params",
    "list_kinds",
    "read_shader",
    "read_script",
    "graph_owner",
    "list_switches",
    "list_transitions",
    "focus",
    "get_tick_level",
    "listen_status",
    "look_venue",
    "get_io",
    "list_scenes",
    "list_dataset",
    "sync_status",
    "sync_peers",
    "sync_log",
    "artnet_nodes",
    "artnet_in_status",
    "timecode_status",
    "list_video_io",
    "perf_report",
    "get_grid_layouts",
    "export_status",
    "list_palettes",
    "venue_report",
    "venue_view",
    "venue_hit",
    "venue_assets_search",
];

/// Tools that change what is on air or destroy authored work: they default to
/// `ask` in `chat::policy`.
const DESTRUCTIVE: &[&str] = &[
    "delete_node",
    "remove_segment",
    "remove_switch",
    "import_segment",
    "set_keyframe",
    "train_ai",
    "edit_mask",
    "blackout",
    "remove_effect",
    "edit_output",
    "resolve_draft",
    "play_sequence",
    "transport",
    "fire_trigger",
    "overlay_control",
];

/// Whether `name` is one `TOOL_CATALOG` knows — the one gate every public fn
/// here uses so `meta`/`parameters` agree exactly with the catalog rather than
/// with a parallel list that could drift.
fn in_catalog(name: &str) -> bool {
    TOOL_CATALOG.iter().any(|(n, _, _)| *n == name)
}

/// The manifest for one tool: `readonly`/`destructive` from the sets above.
/// `None` for any name the catalog does not carry, so a caller can tell
/// "unknown tool" apart from "known, not read-only".
pub fn meta(name: &str) -> Option<ToolMeta> {
    if !in_catalog(name) {
        return None;
    }
    Some(ToolMeta {
        readonly: READONLY.contains(&name),
        destructive: DESTRUCTIVE.contains(&name),
    })
}

/// The JSON type a parameter name implies. There is no formal schema upstream
/// (see `shots.rs`'s module doc), so this is the plausible reading of each
/// key: counts and measures are numbers, the flag-shaped keys are booleans,
/// the list-shaped keys are arrays, and everything else is the safe string.
fn type_for_key(key: &str) -> &'static str {
    match key {
        "ms" | "rows" | "cols" | "n" | "tick" | "slot" | "start" | "end" | "duration"
        | "duration_seconds" | "seconds" | "fps" | "w" | "h" | "frame" | "time"
        | "strength" | "alpha" | "gamma" | "black_lift" | "universe" | "address"
        | "pixels" | "priority" | "amount" | "width" | "height" => "number",
        // Venue 3D: physical sizes, angles and projector measures.
        "width_m" | "height_m" | "throw_ratio" | "aspect" | "lumens" | "yaw" | "pitch" | "roll"
        | "emits" => "number",
        // M4: a `mat.*` node's fixed overrides are all numbers.
        "mix" | "scale" | "rough" | "metal" | "bump" | "env" | "tint" | "tint_hue"
        | "glitch" | "glitch_rate" | "glitch_seed" => "number",
        "on" | "relative" | "delete" | "monitor" | "enabled" | "stop" | "as_copy" | "loop" | "hold" => "boolean",
        "outputs" | "points" | "floats" | "rgb" | "options" | "set" | "items" | "entries" | "palette"
        | "hsb" | "nodes" => "array",
        "patch" | "segment" | "config" | "matrix" | "definition" | "grid_layouts" => "object",
        _ => "string",
    }
}

/// Every key a tool accepts, required and optional alike — the property list
/// of its schema. Tools whose optional keys matter name them here; the rest
/// fall back to [`crate::shots::tool_args`], which is exactly their required
/// set. The either-of tools (`list_nodes`, `read_shader`, `raster`) name both
/// alternatives so the schema advertises them even though neither is required.
fn tool_keys(name: &str) -> Option<Vec<&'static str>> {
    Some(match name {
        "list_nodes" => vec!["path", "graph"],
        "list_kinds" => vec!["kind", "filter", "domain"],
        "read_shader" => vec!["id", "name"],
        "raster" => vec!["shader", "shader_id"],
        "locate" => vec!["step", "png_base64", "n", "region", "cols", "rows"],
        // G1: graph effects, outputs and venue drafts (see TOOL_CATALOG).
        "add_effect" => vec!["graph", "node", "effect", "state", "amount", "source"],
        "remove_effect" => vec!["graph", "node", "effect"],
        "edit_output" => vec!["op", "id", "label", "width", "height"],
        "resolve_draft" => vec!["venue", "decision"],
        "edit_mask" => vec![
            "device_id",
            "mask_id",
            "name",
            "points",
            "parent",
            "alpha",
            "compose",
        ],
        "edit_venue_surface" => vec![
            "venue", "surface", "name", "points", "origin", "u", "v", "material", "delete",
        ],
        // Venue 3D: objects, screens, projectors, swept forms and movement.
        "edit_venue_object" => vec![
            "venue", "object", "kind", "name", "at", "size", "yaw", "crown", "occludes", "emits",
            "faces", "delete",
        ],
        "add_screen" => vec!["venue", "screen", "at", "yaw", "surface"],
        "add_projection" => vec![
            "venue",
            "projector",
            "name",
            "output",
            "on",
            "lens",
            "yaw",
            "pitch",
            "throw_ratio",
            "aspect",
            "lumens",
        ],
        "set_surface_form" => vec![
            "venue", "surface", "form", "path", "profile", "caps", "closed", "kind",
        ],
        "set_venue_sectors" => vec![
            "venue", "surface", "source", "ratio", "points", "magnet",
        ],
        "move_venue_item" => vec!["venue", "item", "at", "yaw", "pitch", "roll"],
        "projection_area" => vec!["venue", "projector"],
        "venue_scene" => vec!["id"],
        "add_stage" => vec![
            "venue", "stage", "name", "at", "size", "path", "profile", "yaw", "delete",
        ],
        "add_truss" => vec![
            "venue", "truss", "name", "at", "size", "path", "profile", "yaw", "delete",
        ],
        "add_dummy" => vec![
            "venue", "dummy", "name", "at", "size", "path", "profile", "yaw", "delete",
        ],
        "venue_spline" => vec![
            "venue", "object", "path", "profile", "kind", "caps", "closed",
        ],
        "place_mask_in_space" => vec![
            "venue", "mask", "on", "points", "rgb", "alpha", "parent", "content", "delete",
        ],
        "venue_boolean" => vec!["venue", "a", "b", "operation", "result"],
        "venue_hit" => vec!["venue", "origin", "dir", "target", "max"],
        "venue_report" => vec!["venue"],
        "venue_view" => vec!["venue", "shot", "width", "height"],
        "venue_assets_search" => vec!["query", "licence", "limit"],
        "venue_asset_import" => vec![
            "venue", "object", "path", "licence", "author", "url", "scale", "at", "name",
        ],
        // D1: the venue's fixtures — `venue` names the venue, `id` (absent
        // adds) the fixture, and every mapping field is optional.
        "set_fixture" => vec![
            "venue",
            "id",
            "kind",
            "profile",
            "group",
            "wire",
            "universe",
            "address",
            "pixels",
            "order",
            "start",
            "drive",
            "projector",
            "delete",
            "matrix",
        ],
        // F9: the per-fixture node — `fixture` is required (the active
        // venue's row), the colour source and slot split optional.
        "add_fixture_node" => vec!["fixture", "colour_from", "model", "pick", "offset"],
        "add_switch" => vec![
            "id", "kind", "graph", "set", "name", "unset", "fixture", "group", "mode",
        ],
        "bake_clip" => vec!["source", "duration", "fps", "w", "h"],
        "train_ai" => vec!["node", "graph"],
        "set_layer_mix" => vec!["timeline", "segment_id", "mask", "blend", "opacity"],
        "project_venue" => vec!["venue", "device", "projector", "rgb", "seconds", "monitor"],
        "aim_fixture" => vec!["fixture", "target", "venue"],
        "solve_head" => vec!["fixture", "target", "venue"],
        "import_gdtf" => vec!["path", "bytes", "name", "mode"],

        "set_palette" => vec!["id", "slot", "value", "relative"],
        // M4 of the materials lote: the `mat.*` node's part plus its fixed
        // overrides, read off the same table the node reads so the schema
        // cannot drift from `metal_graph::materials::CONFIG_DEFAULTS`.
        "set_material" => {
            let mut keys = vec!["graph", "node", "part"];
            keys.extend(
                metal_graph::materials::CONFIG_DEFAULTS
                    .iter()
                    .map(|(k, _)| *k),
            );
            keys
        }
        "list_dataset" => vec!["part"],
        "focus" => vec!["items"],
        "edit_scene" => vec!["op", "id", "clip", "start", "end", "on", "group", "to"],
        "set_head_tracking" => vec!["on", "strength"],
        "attach_source" => vec!["graph", "node", "port", "kind", "from", "config"],
        "export_frame" => vec!["device", "node", "dir"],
        "export_video" => vec![
            "device", "node", "range", "fps", "codec", "bitrate", "audio", "path",
        ],
        // E2: `capture_output` reads a graph node drawn alone. `node` is
        // required (the catalog/shot shape); `device` names its screen when
        // known, otherwise the route finds the graph that holds the node.
        "capture_output" => vec!["device", "node", "frame", "w", "h", "time"],
        "artnet_nodes" => vec!["ms", "target"],
        "artnet_in_status" => vec!["net", "subnet", "universe", "first", "count"],
        "set_output_sink" => vec!["device_id", "kind", "target"],
        // D3: the universe bus's per-source priority/mode — every key optional
        // (no device/node means every source node of every device).
        "dmx_bus" => vec!["priority", "mode", "device", "node"],
        "blend_edges" => vec![
            "venue",
            "surface",
            "projector",
            "enabled",
            "gamma",
            "curve",
            "black_lift",
        ],
        "solid_color" => vec!["rgb", "duration_seconds", "device", "pattern"],
        // G2: the playback and performance gestures. Each key is one the
        // route body reads; `seconds` (not `from`, which `attach_source`
        // already types as a string) is the song offset.
        "play_sequence" => vec!["op", "id", "name", "entries", "device", "collection"],
        "edit_loops" => vec!["op", "definition", "as_copy", "graph", "node", "choice", "song", "seconds", "stop"],
        "transport" => vec!["action", "song", "seconds"],
        "fire_trigger" => vec!["id"],
        "overlay_control" => vec!["op", "device", "layer", "movement", "loop", "hold"],
        "colour_apply" => vec!["graph", "group", "nodes", "palette", "hsb", "rgb"],
        _ => return None,
    })
}

/// The JSON Schema object for `name`'s arguments:
/// `{"type":"object","properties":{<key>:{"type":<type>}},"required":[...]}`.
/// `None` for any name the catalog does not carry; this function never panics,
/// whatever `name` is handed to it.
pub fn parameters(name: &str) -> Option<Value> {
    if !in_catalog(name) {
        return None;
    }
    let keys: Vec<&'static str> = tool_keys(name).unwrap_or_else(|| {
        crate::shots::tool_args(name)
            .map(|s| s.to_vec())
            .unwrap_or_default()
    });
    // The either-of tools declare no required key: naming one alternative is
    // enough (the tool itself checks that at least one arrived), so the
    // schema must not force `path` and `graph` together.
    let required: Vec<&'static str> = match name {
        "list_nodes" | "read_shader" | "raster" | "venue_scene" => Vec::new(),
        // Venue 3D: the required keys are named here because these tools are
        // newer than `shots`' own table (which still answers only its tools).
        "edit_venue_object" => vec!["object"],
        "add_screen" => vec!["screen"],
        "add_projection" => vec!["projector"],
        "set_surface_form" => vec!["surface", "form"],
        "set_venue_sectors" => vec!["surface"],
        "move_venue_item" => vec!["item"],
        "projection_area" => vec!["projector"],
        "add_stage" => vec!["stage"],
        "add_truss" => vec!["truss"],
        "add_dummy" => vec!["dummy"],
        "venue_spline" => vec!["object", "path"],
        "place_mask_in_space" => vec!["mask", "on"],
        "venue_boolean" => vec!["a", "b", "operation"],
        // `dir` and `target` are either-or; only `origin` is forced.
        "venue_hit" => vec!["origin"],
        "venue_report" | "venue_view" | "venue_assets_search" => Vec::new(),
        "venue_asset_import" => vec!["object", "path", "licence"],
        _ => crate::shots::tool_args(name)
            .map(|s| s.to_vec())
            .unwrap_or_default(),
    };
    let mut properties = Map::new();
    for key in keys {
        let ty = match (name, key) {
            ("add_screen", "pixels") => "array",
            ("set_surface_form", "path") | ("set_surface_form", "profile") => "array",
            ("set_venue_sectors", "points") => "array",
            ("set_venue_sectors", "ratio") => "number",
            ("set_venue_sectors", "magnet") => "boolean",
            (_, "at") | (_, "size") | (_, "lens") | (_, "faces") => "array",
            ("edit_venue_object", "crown") => "object",
            ("venue_spline", "path")
            | ("venue_spline", "profile")
            | ("add_stage", "path")
            | ("add_stage", "profile")
            | ("add_truss", "path")
            | ("add_truss", "profile")
            | ("add_dummy", "path")
            | ("add_dummy", "profile")
            | ("place_mask_in_space", "points")
            | ("place_mask_in_space", "rgb")
            | ("venue_hit", "origin")
            | ("venue_hit", "dir")
            | ("venue_hit", "target") => "array",
            ("venue_hit", "max")
            | ("venue_view", "width")
            | ("venue_view", "height")
            | ("venue_assets_search", "limit")
            | ("venue_asset_import", "scale") => "number",
            ("venue_spline", "caps") | ("venue_spline", "closed") => "boolean",
            _ => type_for_key(key),
        };
        properties.insert(key.to_string(), json!({ "type": ty }));
    }
    Some(json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": required,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// 1. Exactly the catalog: every name has both, a made-up name neither.
    #[test]
    fn catalog_names_have_meta_and_parameters_and_others_do_not() {
        for (name, _, _) in TOOL_CATALOG {
            assert!(meta(name).is_some(), "{name} has no meta");
            assert!(parameters(name).is_some(), "{name} has no parameters");
        }
        assert!(meta("no_such_tool").is_none());
        assert!(parameters("no_such_tool").is_none());
    }

    /// 2. Shape: object/object/array.
    #[test]
    fn parameters_are_a_well_formed_object_schema() {
        for (name, _, _) in TOOL_CATALOG {
            let schema = parameters(name).expect("catalog name");
            assert_eq!(schema["type"], "object", "{name}");
            assert!(
                schema["properties"].is_object(),
                "{name} properties not an object"
            );
            assert!(
                schema["required"].is_array(),
                "{name} required not an array"
            );
            for req in schema["required"].as_array().unwrap() {
                let key = req.as_str().expect("required keys are strings");
                assert!(
                    schema["properties"].get(key).is_some(),
                    "{name}: required `{key}` missing from properties"
                );
            }
        }
    }

    /// 3. Flags: policy's two cases and the destructive set exactly.
    #[test]
    fn flags_match_policy_expectations() {
        assert_eq!(
            meta("delete_node"),
            Some(ToolMeta {
                readonly: false,
                destructive: true
            })
        );
        assert_eq!(
            meta("list_nodes"),
            Some(ToolMeta {
                readonly: true,
                destructive: false
            })
        );

        let destructive: BTreeSet<&str> = TOOL_CATALOG
            .iter()
            .map(|(n, _, _)| *n)
            .filter(|n| meta(n).unwrap().destructive)
            .collect();
        let expected: BTreeSet<&str> = [
            "delete_node",
            "remove_segment",
            "remove_switch",
            "import_segment",
            "set_keyframe",
            "train_ai",
            "edit_mask",
            "blackout",
            "remove_effect",
            "edit_output",
            "resolve_draft",
            "play_sequence",
            "transport",
            "fire_trigger",
            "overlay_control",
        ]
        .into_iter()
        .collect();
        assert_eq!(destructive, expected);
    }

    /// E1: the video export tool advertises every field of its route
    /// (`POST /export/video {device,node,range,fps,codec,bitrate,path}`), and
    /// requires the three a call cannot work without.
    #[test]
    fn export_video_schema_lists_its_route_fields() {
        let p = parameters("export_video").expect("export_video is in the catalog");
        for key in [
            "device", "node", "range", "fps", "codec", "bitrate", "audio", "path",
        ] {
            assert!(p["properties"].get(key).is_some(), "missing property {key}");
        }
        let required: Vec<&str> = p["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(required.contains(&"device"), "{required:?}");
        assert!(required.contains(&"range"), "{required:?}");
        assert!(required.contains(&"path"), "{required:?}");
    }

    /// 4. The either-of tools require nothing.
    #[test]
    fn either_of_tools_have_no_required_keys() {
        for name in ["list_nodes", "read_shader", "raster"] {
            let required = parameters(name).unwrap()["required"]
                .as_array()
                .unwrap()
                .len();
            assert_eq!(required, 0, "{name} must have no required keys");
        }
    }

    /// D1: `set_fixture` names its venue and advertises every mapping key the
    /// route reads, so the AI's schema and
    /// `POST /venue {op: "fixture"}` (the Settings page's own body) agree
    /// (regla 6).
    #[test]
    fn set_fixture_advertises_its_venue_and_mapping_keys() {
        let params = parameters("set_fixture").expect("set_fixture is in the catalog");
        assert_eq!(params["required"], json!(["venue"]));
        for key in [
            "id",
            "kind",
            "profile",
            "group",
            "wire",
            "universe",
            "address",
            "pixels",
            "order",
            "start",
            "drive",
            "projector",
            "delete",
            "matrix",
        ] {
            assert!(
                params["properties"].get(key).is_some(),
                "set_fixture is missing `{key}` from its schema"
            );
        }
        assert_eq!(crate::shots::tool_args("set_fixture"), Some(&["venue"][..]));
        assert!(meta("set_fixture").is_some());
    }
}
