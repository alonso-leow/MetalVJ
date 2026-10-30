//! The door-bound authoring tools end to end: a real `metal-dash --no-window`
//! child on a free port with `--data <tempdir>` (a venue, a device graph, an
//! output), and `metal-mcp` over stdio calling them.
//!
//! Two tests run against that fixture:
//! - `door_tools_round_trip_through_dash`, the shape checks for three of them.
//! - `every_hardware_free_tool_answers_a_smoke_call`, one real call per tool the
//!   server lists (see its doc for the skip list).
//!
//! The door (`call::install_agent_door`) and the data root are process-wide
//! `OnceLock`s in metal-dash, so nothing here runs them in-process: each test
//! gets its own child, and the child is killed when the test ends (`Guard`).
//!
//! Needs the default-features metal-dash binary (the door-bound tools exist only
//! on a `gpu` build) next to this test's metal-mcp binary:
//!     cargo build -p metal-dash && cargo test -p metal-mcp --test door_tools

use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

const TOKEN: &str = "door-test-agent-token-0123456789abcdef";
const VENUE_EXAMPLE: &str = "data/agent/venues/bar.toml";

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The metal-dash binary built beside this test's metal-mcp binary.
fn fw_dash_bin() -> PathBuf {
    let dir = Path::new(env!("CARGO_BIN_EXE_metal-mcp")).parent().expect("bin dir");
    let bin = dir.join("metal-dash");
    assert!(
        bin.exists(),
        "{} is missing: run `cargo build -p metal-dash` first (the door tools need its default features)",
        bin.display()
    );
    bin
}

/// A fresh data root: the example venue, two outputs, a live graph with a
/// source, a material and a script node, a device graph of its own for
/// `edit_layer`, a delete-me graph, a saved switch, the shader corpus the
/// tools read, and the operator's `devices` permission granted
/// (`test_mask_grid` drives an output, so an unanswered capability would pause
/// it for approval and be refused).
fn fixture(root: &Path) {
    std::fs::create_dir_all(root.join("venues")).unwrap();
    std::fs::create_dir_all(root.join("graphs")).unwrap();
    std::fs::create_dir_all(root.join("shaders")).unwrap();
    std::fs::copy(repo_root().join(VENUE_EXAMPLE), root.join("venues/bar.toml")).unwrap();
    // The node catalog (`GET /catalog`) reads the schema from the data root.
    std::fs::copy(repo_root().join("data/schema.json"), root.join("schema.json")).unwrap();
    // The shader corpus `read_shader`/`save_shader` read (and the `level`
    // shader `save_shader`'s smoke call re-posts as valid WGSL).
    if let Ok(src) = std::fs::read(repo_root().join("data/shaders/level.wgsl")) {
        std::fs::write(root.join("shaders/level.wgsl"), src).unwrap();
    }
    // Two outputs: `wall` carries the masks, `stage` carries the layer stack
    // `edit_layer` rewrites (its sync writes the device's whole graph, so it
    // must not share one with the graph tools).
    std::fs::write(
        root.join("outputs.json"),
        json!([
            {"id": "wall", "label": "Wall", "width": 640, "height": 360, "masks": []},
            {"id": "stage", "label": "Stage", "width": 640, "height": 360, "masks": []}
        ])
        .to_string(),
    )
    .unwrap();
    // The live graph the graph tools operate on: an output device fed by a
    // generator, plus a second generator, a `mat.*` and a `script.*` node.
    std::fs::write(
        root.join("graphs/authoring.json"),
        json!({"id": "authoring", "name": "authoring", "domain": "live",
            "nodes": [
                {"id": "out-a", "kind": "output.device", "pos": [0, 0], "label": "",
                    "bypass": false, "solo": false, "config": {"mode": {"kind": "own"}}},
                {"id": "gen-a", "kind": "gen.domain", "pos": [-300, 0], "label": "",
                    "bypass": false, "solo": false, "config": {}},
                {"id": "gen-b", "kind": "gen.domain", "pos": [-300, 200], "label": "",
                    "bypass": false, "solo": false, "config": {}},
                {"id": "mat-a", "kind": "mat.wood", "pos": [-150, 0], "label": "",
                    "bypass": false, "solo": false, "config": {}},
                {"id": "script-a", "kind": "script.py", "pos": [-450, 0], "label": "",
                    "bypass": false, "solo": false, "config": {}}
            ],
            "edges": [{"id": "e0", "from": {"node": "gen-a", "port": "image"},
                "to": {"node": "out-a", "port": "image"}}],
            "ids": {"next": {"comp": 1, "gen": 1, "out": 1}}})
        .to_string(),
    )
    .unwrap();
    // The device graph `edit_layer`'s rewrite touches, alone.
    std::fs::write(
        root.join("graphs/device-stage.json"),
        json!({"id": "device-stage", "name": "stage", "domain": "live",
            "nodes": [{"id": "stage-0", "kind": "output.device", "pos": [0, 0], "label": "",
                "bypass": false, "solo": false, "config": {"mode": {"kind": "own"}}}],
            "edges": [], "ids": {"next": {"comp": 1, "gen": 1, "out": 1}}})
        .to_string(),
    )
    .unwrap();
    // A throwaway graph so `delete_node` names a node it may really remove
    // without disturbing the graph the other calls share.
    std::fs::write(
        root.join("graphs/scratch.json"),
        json!({"id": "scratch", "name": "scratch", "domain": "live",
            "nodes": [{"id": "n-0", "kind": "gen.domain", "pos": [0, 0], "label": "",
                "bypass": false, "solo": false, "config": {}}],
            "edges": [], "ids": {"next": {"comp": 1, "gen": 1, "out": 1}}})
        .to_string(),
    )
    .unwrap();
    // A saved switch so `set_switch` (which the catalog orders before
    // `add_switch`) names a button that really exists.
    std::fs::write(
        root.join("switches.json"),
        json!([{"id": "seeded", "name": "Seeded", "kind": "toggle", "graph": "authoring",
            "set": [{"node": "gen-a", "on": true}]}])
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        root.join("ai_tools.json"),
        json!({"permissions": {"dispositivos": "allow"}}).to_string(),
    )
    .unwrap();
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fw-door-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn http_status(addr: &str, path: &str) -> Option<u16> {
    let mut stream = TcpStream::connect(addr).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(20))).ok()?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").ok()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok()?;
    reply.split_whitespace().nth(1)?.parse().ok()
}

/// Kills the child on drop, whatever the assertions did.
struct Guard(Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One metal-mcp over stdio, initialised, talking to the given dash.
struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Mcp {
    fn spawn(dash: &str, token_file: &Path) -> Mcp {
        let mut child = Command::new(env!("CARGO_BIN_EXE_metal-mcp"))
            .args(["--dash", dash, "--allow-ask"])
            .env("FW_AGENT_TOKEN_FILE", token_file)
            .env_remove("FW_DASH_TOKEN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn metal-mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut mcp = Mcp { child, stdin, stdout };
        let init = mcp.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},
            "clientInfo":{"name":"door-test","version":"0"}}}));
        assert_eq!(init["result"]["serverInfo"]["name"], "metal-mcp", "{init}");
        writeln!(mcp.stdin, "{}", json!({"jsonrpc":"2.0","method":"notifications/initialized"})).unwrap();
        mcp
    }

    fn request(&mut self, message: Value) -> Value {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad reply {line:?}: {e}"))
    }

    /// Calls one tool; returns its JSON text, asserting it was not an MCP error.
    fn call(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        let reply = self.request(json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
            "params":{"name":name,"arguments":arguments}}));
        let result = &reply["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or_else(|| panic!("{name}: {reply}"));
        assert!(result["isError"] != json!(true), "{name} failed: {text}");
        serde_json::from_str(text).unwrap_or_else(|e| panic!("{name} text is not JSON ({e}): {text}"))
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A headless metal-dash on `data` with the agent token from `token_file`, up and
/// answering `/state`. The guard kills it when dropped.
fn start_dash(data: &Path, token_file: &Path) -> (Guard, String) {
    // Returns `host:port`.
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    // The child's log stays in the data root, so a failed run leaves it behind.
    let log = std::fs::File::create(data.join("metal-dash.log")).unwrap();
    // The app reads its repo-relative files (`data/schema.json`), so it runs
    // from the repo root, as it does when started by hand.
    let dash = Command::new(fw_dash_bin())
        .current_dir(repo_root().canonicalize().expect("repo root"))
        .args(["--no-window", "--host", "127.0.0.1", "--port", &port.to_string()])
        .args(["--osc-port", &free_port().to_string(), "--data"])
        .arg(data)
        .env("FW_AGENT_TOKEN_FILE", token_file)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .expect("spawn metal-dash");
    let guard = Guard(dash);

    let deadline = Instant::now() + Duration::from_secs(120);
    while http_status(&addr, "/state") != Some(200) {
        assert!(Instant::now() < deadline, "metal-dash never answered /state on {addr}");
        std::thread::sleep(Duration::from_millis(100));
    }
    (guard, addr)
}

/// `GET path` with the agent token, the body parsed as JSON.
fn get_json(addr: &str, path: &str, token: &str) -> Value {
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n").unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    let body = reply.split("\r\n\r\n").nth(1).unwrap_or_default();
    serde_json::from_str(body).unwrap_or_else(|e| panic!("GET {path} is not JSON ({e}): {body}"))
}

/// A value of the schema's JSON type for a required key, chosen so a route
/// that wants a real name (`node`, `tick_level`) is not refused for its value.
fn dummy_arg(key: &str, ty: &str) -> Value {
    match (key, ty) {
        ("tick_level", _) => json!("base"),
        ("venue", _) => json!("bar"),
        (_, "boolean") => json!(false),
        (_, "number" | "integer") => json!(0),
        (_, "array") => json!([]),
        (_, "object") => json!({}),
        _ => json!("x"),
    }
}

#[test]
fn door_tools_round_trip_through_dash() {
    let data = scratch("data");
    fixture(&data);
    let token_file = data.join("agent-token");
    std::fs::write(&token_file, format!("{TOKEN}\n")).unwrap();

    let (_dash, addr) = start_dash(&data, &token_file);

    let mut mcp = Mcp::spawn(&format!("http://{addr}"), &token_file);

    // create_mask_grid: a 2x2 grid on the one output makes four masks.
    let grid = mcp.call(2, "create_mask_grid", json!({"outputs": ["wall"], "rows": 2, "cols": 2}));
    let mask_ids = grid["mask_ids"].as_array().expect("mask_ids");
    assert_eq!(mask_ids.len(), 4, "{grid}");

    // test_mask_grid: one JPEG frame for the output, with its masks outlined.
    let test = mcp.call(3, "test_mask_grid", json!({"outputs": ["wall"]}));
    let frames = test["frames"].as_array().expect("frames");
    assert_eq!(frames.len(), 1, "{test}");
    assert_eq!(frames[0]["device_id"], "wall");
    let jpeg = frames[0]["jpeg_base64"].as_str().unwrap_or_default();
    assert!(jpeg.starts_with("/9j/"), "not a JPEG: {}", &jpeg[..jpeg.len().min(16)]);

    // edit_venue_surface: a new surface is in the reply and in the venue file.
    let edit = mcp.call(4, "edit_venue_surface", json!({
        "venue": "bar", "surface": "probe",
        "points": [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0]]}));
    assert_eq!(edit["ok"], true, "{edit}");
    let surfaces = edit["venue"]["surface"].as_array().expect("surface list");
    assert!(surfaces.iter().any(|s| s["id"] == "probe"), "{edit}");
    let on_disk = std::fs::read_to_string(data.join("venues/bar.toml")).unwrap();
    assert!(on_disk.contains("probe"), "the venue file was not written");

    drop(mcp);
    let _ = std::fs::remove_dir_all(&data);
}

/// One real smoke call per hardware-free tool: the set is exactly what the
/// server's `tools/list` (computed from `TOOL_CATALOG`) offers, so a newly
/// exported tool with no `smoke_args` row and no `SKIP` reason fails the test.
///
/// The fixture creates the subjects the calls need (a venue, an output, a live
/// graph with a generator, a material and a script node, a throwaway graph, a
/// user shader), so the call genuinely succeeds rather than answering a 400.
///
/// The skipped tools, each with the reason it is left out:
/// - camera and projector: `look_venue`, `project_venue`, `aim_fixture`,
///   `solve_head`, `set_head_tracking`. The camera-solve tools themselves
///   (`scan_venue`, `calibrate_projector`, `discover_fixture`,
///   `capture_camera_frame`) are smoked now — the M5 seam gives them a
///   hardware-free synthetic feed — and the rest need a projector, a moving
///   head or a live sensor.
/// - real time or blocking: `raster`, `bake_clip` (GPU compile/render),
///   `sleep` (waits), `export_video` (renders a frame range and encodes),
///   `export_frame` (renders a frame to the operator's Pictures), `artnet_nodes`
///   (an ArtPoll over the network), `capture_output` (renders a node).
/// - the repo's own `data/` (relative paths, not the fixture's `--data` root):
///   `import_segment`, `move_segment`, `remove_segment`, `set_keyframe`,
///   `attach_digest`, `set_layer_mix`, `rasterize_overlay`, `set_palette`,
///   `edit_scene`. Their routes open `data/timelines`, `data/animations`,
///   `data/library/palettes.json` or the scene book relative to the process
///   cwd, so a smoke call would write the real repository, not the fixture.
/// - needing a subject this fixture cannot honestly build: `train_ai` (a
///   trained `ai.*` node from authored takes), `import_gdtf` (a real `.gdtf`),
///   `locate` (a camera/PNG to resolve), `bind_param` and `set_param` (a bound
///   variable / a held parameter, which the bare fixture graph has none of),
///   `add_fixture_node` and `blend_edges` (the venue's fixture/projector state
///   the skipped `project_venue`/`calibrate_projector` would set).
/// - server-side route gaps, not hardware: `read_script` (`GET /script/` takes
///   the node from the path tail, but the adapter forwards it as a query arg),
///   `set_fixture` (the route table sends the args verbatim to `POST /venue`
///   without the `op: "fixture"` the route needs), `dmx_bus` (`tool_route` has
///   no arm, so `/ai-config` lists no route and the adapter refuses it).
const SKIP: &[(&str, &str)] = &[
    ("look_venue", "needs a live camera frame"),
    ("project_venue", "needs a projector / monitor"),
    ("aim_fixture", "needs a moving-head fixture and a real aim"),
    ("solve_head", "needs a moving-head fixture and a real pose"),
    ("set_head_tracking", "needs a live camera / sensor"),
    ("add_fixture_node", "needs the venue projected first (project_venue, skipped)"),
    ("blend_edges", "needs a projector pinned to a surface (calibrate_projector, skipped)"),
    ("read_script", "GET /script/ takes the node from the path tail; the adapter forwards it as a query arg (server-side route gap)"),
    ("set_fixture", "the route table forwards the args verbatim to POST /venue without the op the route needs (server-side route gap)"),
    ("dmx_bus", "metal-dash lists no route for it in /ai-config (tool_route has no arm)"),
    ("raster", "compiles and rasterises a shader on the GPU: real time"),
    ("bake_clip", "renders a clip: real time"),
    ("sleep", "blocks for `ms`; nothing to smoke"),
    ("export_video", "renders a frame range and encodes video: real time"),
    ("export_queue", "an add starts a real-time export and writes data/export-presets.json"),
    ("export_frame", "renders a frame to the operator's Pictures"),
    ("artnet_nodes", "an ArtPoll over the network: real time"),
    ("capture_output", "renders a node: real time"),
    ("import_segment", "writes the repo's data/timelines, not the fixture root"),
    ("move_segment", "writes the repo's data/timelines, not the fixture root"),
    ("remove_segment", "writes the repo's data/timelines, not the fixture root"),
    ("set_keyframe", "writes the repo's data/animations, not the fixture root"),
    ("attach_digest", "writes the repo's data/timelines and digest, not the fixture root"),
    ("set_layer_mix", "writes the repo's data/timelines, not the fixture root"),
    ("rasterize_overlay", "writes the repo's data/timelines, not the fixture root"),
    ("set_palette", "writes the repo's data/library/palettes.json, not the fixture root"),
    ("edit_scene", "writes the repo's scene book, not the fixture root"),
    ("train_ai", "needs a trained ai.* node from authored takes"),
    ("import_gdtf", "needs a real .gdtf file"),
    ("locate", "needs a PNG / camera frame to resolve"),
    ("bind_param", "the fixture graph has no bindable variable to wire"),
    ("set_param", "the fixture graph holds no named parameter to set"),
];

/// The arguments for one smoke call: the schema's required keys as
/// [`dummy_arg`], then a genuinely valid value for each key the tool needs to
/// succeed. `None` means the tool has no smoke row — the test fails so a new
/// tool cannot ship uncovered.
fn smoke_args(name: &str, schema: &Value) -> Option<Value> {
    let mut args = Map::new();
    if let Some(required) = schema["required"].as_array() {
        for key in required {
            let key = key.as_str().unwrap();
            let ty = schema["properties"][key]["type"].as_str().unwrap_or("string");
            args.insert(key.to_string(), dummy_arg(key, ty));
        }
    }
    let overrides = match name {
        "list_tools" => json!({}),
        "get_env" => json!({"graph": "authoring"}),
        "list_nodes" => json!({"graph": "authoring"}),
        "search_nodes" => json!({"q": "gen"}),
        "create_node" => json!({"graph": "authoring", "kind": "gen.domain"}),
        "delete_node" => json!({"graph": "scratch", "id": "n-0"}),
        "connect_nodes" => json!({"graph": "authoring",
            "from": {"node": "gen-a", "port": "row"},
            "to": {"node": "script-a", "port": "control"}}),
        "get_params" => json!({"graph": "authoring", "id": "out-a"}),
        "set_params" => json!({"graph": "authoring", "id": "out-a", "patch": {"label": "smoke"}}),
        "read_shader" => json!({"id": "level"}),
        "take_graph" => json!({"graph": "authoring"}),
        "release_graph" => json!({"graph": "authoring"}),
        "graph_owner" => json!({"graph": "authoring"}),
        "save_shader" => json!({"id": "smoke-level", "wgsl": LEVEL_WGSL}),
        "save_script" => json!({"graph": "authoring", "node": "script-a", "source": "0.0"}),
        "create_mask_grid" => json!({"outputs": ["wall"], "rows": 2, "cols": 2}),
        "test_mask_grid" => json!({"outputs": ["wall"]}),
        "edit_mask" => json!({"device_id": "wall", "name": "smoke",
            "points": [[0, 0], [1, 0], [1, 1], [0, 1]]}),
        "edit_venue_surface" => json!({"venue": "bar", "surface": "smoke",
            "points": [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0]]}),
        // M5 camera tools: the synthetic feed makes them hardware-free. The
        // venue is the fixture's bar example, whose `proj-1` throws on
        // `back-wall`.
        "scan_venue" => json!({"venue": "bar"}),
        "discover_fixture" => json!({"venue": "bar"}),
        "calibrate_projector" => json!({"venue": "bar", "projector": "proj-1", "surface": "back-wall"}),
        "capture_camera_frame" => json!({"w": 64, "h": 36}),
        "edit_layer" => json!({"op": "add", "device": "stage", "model": "domain"}),
        "list_switches" => json!({}),
        "set_switch" => json!({"id": "seeded", "op": "on"}),
        "get_tick_level" => json!({}),
        "set_tick_level" => json!({"tick_level": "base"}),
        "add_switch" => json!({"id": "smoke", "kind": "toggle", "graph": "authoring",
            "set": [{"node": "out-a", "on": true}]}),
        "remove_switch" => json!({"id": "smoke"}),
        "blackout" => json!({"on": false}),
        "solid_color" => json!({"rgb": [0, 0, 0], "duration_seconds": 1}),
        "embed_ai" => json!({"graph": "authoring", "node": "gen-a",
            "ai": "ai.colourist", "on": false}),
        "attach_source" => json!({"graph": "authoring", "node": "out-a",
            "port": "image", "kind": "gen.domain"}),
        "insert_node" => json!({"graph": "authoring", "after": "gen-a", "kind": "fx.level"}),
        "get_io" => json!({"graph": "authoring", "node": "out-a"}),
        "set_material" => json!({"graph": "authoring", "node": "mat-a", "part": "*"}),
        "list_scenes" => json!({}),
        "list_dataset" => json!({}),
        "list_video_io" => json!({}),
        "set_output_sink" => json!({"device_id": "wall", "kind": ""}),
        "sync_status" => json!({}),
        "sync_peers" => json!({}),
        "sync_log" => json!({}),
        "artnet_in_status" => json!({}),
        "timecode_status" => json!({}),
        "list_transitions" => json!({}),
        "listen_status" => json!({}),
        _ => return None,
    };
    for (key, value) in overrides.as_object().expect("overrides is an object") {
        args.insert(key.clone(), value.clone());
    }
    Some(Value::Object(args))
}

/// A user-authored WGSL the `POST /shaders` gate accepts, read from the repo
/// corpus at compile-test time (the fixture copies it into the data root too).
const LEVEL_WGSL: &str = include_str!("../../../data/shaders/level.wgsl");

#[test]
fn every_hardware_free_tool_answers_a_smoke_call() {
    let data = scratch("smoke");
    fixture(&data);
    let token_file = data.join("agent-token");
    std::fs::write(&token_file, format!("{TOKEN}\n")).unwrap();
    let (_dash, addr) = start_dash(&data, &token_file);

    // The route table, so a tool whose route is `null` is reported as such
    // rather than called into a confusing error.
    let config = get_json(&addr, "/ai-config", TOKEN);
    let null_routes: Vec<String> = config["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter(|t| t["route"].is_null())
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();

    let mut mcp = Mcp::spawn(&format!("http://{addr}"), &token_file);
    let listed = mcp.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let tools = listed["result"]["tools"].as_array().expect("tools/list");

    let skip: HashMap<&str, &str> = SKIP.iter().copied().collect();
    let mut smoked = Vec::new();
    let mut skipped = Vec::new();
    let mut refused = Vec::new();
    let mut uncovered = Vec::new();
    let mut id = 100;
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        if let Some(_reason) = skip.get(name) {
            skipped.push(name.to_string());
            continue;
        }
        let Some(args) = smoke_args(name, &tool["inputSchema"]) else {
            uncovered.push(name.to_string());
            continue;
        };
        // Keep the required-argument shape check: every required key is present.
        for key in tool["inputSchema"]["required"].as_array().into_iter().flatten() {
            assert!(
                args.get(key.as_str().unwrap()).is_some(),
                "{name}: smoke args must carry its required `{}`",
                key
            );
        }
        id += 1;
        let reply = mcp.request(json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
            "params":{"name":name,"arguments":args}}));
        let is_error = reply["result"]["isError"] == json!(true);
        let text = reply["result"]["content"][0]["text"].as_str().unwrap_or_default();
        if is_error {
            let route = if null_routes.iter().any(|n| n == name) { "route:null" } else { "routed" };
            refused.push(format!("{name} ({route}): {text}"));
        } else {
            smoked.push(name.to_string());
        }
    }

    // Every listed tool is accounted for: smoked, skipped with a reason, or a
    // failure. A new tool that falls through all three fails below.
    assert_eq!(
        smoked.len() + skipped.len() + refused.len() + uncovered.len(),
        tools.len(),
        "every listed tool must be smoked, skipped or refused"
    );
    assert!(
        uncovered.is_empty(),
        "new exported tools with no smoke row and no SKIP reason:\n{}",
        uncovered.join("\n")
    );
    assert!(
        refused.is_empty(),
        "hardware-free tools failed their smoke call:\n{}",
        refused.join("\n")
    );
    let _ = std::fs::remove_dir_all(&data);
}
