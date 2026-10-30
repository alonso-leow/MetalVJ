# MetalVJ as an MCP server (`metal-mcp`)

`metal-mcp` exposes the agent's tool catalog (`metal_agent::harness::TOOL_CATALOG`)
to any MCP client. It is a thin adapter: `tools/list` is computed from the
catalog and schemas, and each `tools/call` is forwarded to the route metal-dash
already serves for that tool. The app must be running.

## Run

```sh
metal-dash --port 7900                          # the app (or ./scripts/app start)
metal-mcp --dash http://127.0.0.1:7900          # stdio, for an MCP client
metal-mcp --dash http://127.0.0.1:7900 --http :7910 --http-token $T   # Streamable HTTP at POST /mcp
metal-mcp --dash http://127.0.0.1:7900 --allow-ask  # let MCP callers run `ask` tools
```

`--dash` defaults to `$FW_DASH_URL`, then `http://127.0.0.1:7900`. The HTTP
transport binds `127.0.0.1` for `:PORT`, and refuses requests whose `Origin`
is not local.

## HTTP bearer token

`--http` needs a token of its own, separate from the one metal-mcp uses toward
metal-dash. It is `--http-token VALUE` when given, else the agent-token file (the
same file metal-dash writes; see "Agent token" below). Without either, metal-mcp
refuses to start with `--http`, on loopback as well as on other addresses,
because a local process could otherwise drive the operator's tools through
it.

- Every `POST /mcp` must carry `Authorization: Bearer <token>`. Without it,
  or with a wrong one, the reply is `401 Unauthorized` with
  `WWW-Authenticate: Bearer`, and nothing reaches metal-dash. The comparison is
  constant-time.
- An address that is not loopback (`0.0.0.0:7910`, `192.168.1.5:7910`) is
  allowed, and the token is then the only guard. Put it behind TLS if the
  network is not trusted.

How a client passes it:

```sh
TOKEN=$(cat ~/.config/metalvj/agent-token)
curl -s -X POST http://127.0.0.1:7910/mcp \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

In an MCP client config, the header goes in the server entry, for example
`"headers": {"Authorization": "Bearer <token>"}` for a Streamable HTTP server
at `http://127.0.0.1:7910/mcp`.

`--allow-ask` is the operator's choice. Without it a tool whose policy is
`ask` answers `denied by operator`; `off` tools never run. The policy itself
is the sidebar's (`GET /ai-config`, `policy` per tool).

## Install

```sh
cargo install --path crates/metal-mcp            # or --root <dir> for a private prefix
```

## Register with Claude Code

```sh
claude mcp add MetalVJ -- metal-mcp --dash http://127.0.0.1:7900
```

## Register with other clients

Any client that reads an `mcpServers` map:

```json
{
  "mcpServers": {
    "metalvj": {
      "command": "metal-mcp",
      "args": ["--dash", "http://127.0.0.1:7900"]
    }
  }
}
```

For a client that speaks Streamable HTTP, start `metal-mcp --http :7910` and
point it at `http://127.0.0.1:7910/mcp` with the bearer header above.

## What is exported

- Every tool in `TOOL_CATALOG` marked implemented. `implemented: false`
  entries (for example `capture_output`) are not offered.
- Never the retired `ai.*` names.
- `readOnlyHint` and `destructiveHint` annotations come from the catalog's
  read-only and destructive sets.

## How a call is forwarded

The adapter reads `GET /ai-config` once and takes each tool's `route`
(`{method, path}`) from it. Arguments go verbatim: a `GET` route receives them
as its query string, a `POST` route as its JSON body. The adapter keeps no
mapping of its own.

Two kinds of tool take another path:

- **In-process tools** (`sleep`, `locate`, `capture_camera_frame`,
  `bake_clip`, `save_shader`, `take_graph`, `release_graph`, `graph_owner`)
  have no route of their own. Their route is `POST /agent/tool`, and the
  adapter sends `{"name", "args", "allow_ask"}`. metal-dash runs the tool through
  the same executor a chat turn uses (`crate::chat::agent_tool_at`), with the
  same policy: `off` is refused, `ask` is refused unless `allow_ask`. A refusal
  or tool error comes back as `{"error": ...}` and the MCP result has
  `isError: true`. The route runs before metal-dash takes its state lock, so a
  `sleep` never stalls the app.
- **Executor-routed tools** (`get_env`, `list_nodes`, `get_params`,
  `read_shader`, `get_io`, `create_node`, `delete_node`, `connect_nodes`,
  `set_params`, `raster`, `embed_ai`, `attach_source`, `insert_node`, `bind_param`,
  `set_param`, `edit_mask`,
  `set_switch`, `solid_color`, `rasterize_overlay`, `edit_layer`,
  `import_segment`, `move_segment`, `remove_segment`, `set_layer_mix`,
  `set_keyframe`, `attach_digest`, `set_palette`, `edit_scene`, `add_switch`,
  `remove_switch`, `create_mask_grid`, `test_mask_grid`,
  `edit_venue_surface`, `project_venue`, `calibrate_projector`, `blend_edges`, `aim_fixture`,
  `look_venue`, `scan_venue`, `discover_fixture`, `solve_head`). Their argument contract is the tool's schema, which does
  not match the HTTP route body (`which graph`, `no such op`, `expected
  {timeline}`, a missing `model`/`name`), so they run through the executor as
  well. The route table in metal-dash (`tool_route`) names `/agent/tool` for them.
  `set_fixture` and `dmx_bus` are in this group too: their routes
  (`POST /venue`, `POST /outputs`) need an `op` the verbatim body does not carry,
  and the executor adds it. `read_script` reads the node from `?node=` as well
  as the path tail, so the verbatim `GET` works.

## Tools

Each tool names its route in `GET /ai-config`. The grid, export and palette
tools were added to close coverage gaps; each reads or writes through the route
the matching panel uses (rule 6).

| Tool | Route | What it does |
|---|---|---|
| `perf_report` | `GET /perf` | The frame-time report the F12 monitor shows. |
| `get_grid_layouts` | `GET /settings` | The grid's saved layouts (`grid_layouts`): id, name, surface, controllers, and `active`. |
| `set_grid_layouts` | `POST /settings` | Replaces the whole layout list. The panel's create, create-from-hardware, rename and delete write this key. |
| `export_status` | `GET /export` | Progress of the export job, or idle. |
| `export_control` | `POST /export` | `op: cancel` stops the running export and removes the partial file. |
| `list_palettes` | `GET /author/palettes` | The stored palettes that `set_palette` edits. |

Not given a tool, on purpose: the MIDI input and controller faces (shell and
hardware only, no door route; a face's controllers are stored in
`grid_layouts`, which the tools above read and write); export presets
(`data/export-presets.json`, chosen through `export_video`); and palette
deletion (destructive, left to the operator).

## Hardware run (needs a person)

Not run by the automated suite. Do this once, with a real projector and camera
on the venue, and record the result below.

1. Start metal-dash with the venue (`bar` or the real one) and the camera
   streaming.
2. `scan_venue`: the camera finds the room. Check the reported surfaces.
3. `calibrate_projector` for each projector and surface. Check the reported
   error and the projected pattern.
4. `look_venue` and `project_venue`: the masks land on the real surfaces.

Notes for whoever runs it: `scan_venue`, `discover_fixture` and
`calibrate_projector` are exercised in the suite only against a synthetic feed
(`M5` seam). `solve_head` posts analytic inverse kinematics with no camera feed,
so its pose is not checked against a real head. `look_venue` and
`project_venue` have no hardware-free smoke call.

Result: _not yet run._ Record the date, the venue, the projector and camera
models, and the reported error for each step here.

Image results (`png_base64`, or an `image/*` body) come back as MCP image
content next to the JSON text. `locate` with `step: "grid"` on a PNG is the
verified case.

Structured refusals keep their shape. `capture_camera_frame` answers
`{"ok": false, "code": "camera_not_streaming", "error": "camera not streaming: ..."}`
when no frame can be had, and `bake_clip` answers `code: "needs_gpu_build"` on
a build without the `gpu` feature. When a reply carries a `code`, the MCP error
text is that whole JSON object, so the client can branch on the code.

Door-bound tools reach the live state through the door the shell uses, and
run through the executor like the rest of the executor-routed list above. The
executor adapts the schema's arguments (`outputs` becomes `device_ids`,
`venue` and `surface` become `POST /venue {op: "surface"}`); a verbatim forward
to the route would not match the schema. metal-dash installs the door at startup,
so a headless `--no-window` build serves them too. The door-bound tools exist
only on a default-features (`gpu`) metal-dash build.

## Agent token (`/agent/tool`)

`POST /agent/tool` runs tools as the operator, so it is not open to every local
process. metal-dash creates a random per-install token on first start, 64 hex
characters, written 0600 to `~/.config/metalvj/agent-token` (or
`$XDG_CONFIG_HOME/metalvj/agent-token`). `FW_AGENT_TOKEN_FILE` moves it.
The file is reused on later starts, so the token changes only when the file is
deleted.

- Every `POST /agent/tool` must carry `Authorization: Bearer <token>`. Without it
  or with a wrong token the reply is `401 Unauthorized` and no tool runs. The
  comparison is constant-time.
- `metal-mcp` reads the same file at startup. `FW_DASH_TOKEN` overrides it. It
  sends the header on every request to metal-dash, and warns on stderr when no
  token is found.
- The other routes do not check the token. Keep metal-dash on `127.0.0.1` unless
  the network is trusted. The token stops other local processes, not a remote
  caller who can reach the port and read the file.

```sh
TOKEN=$(cat ~/.config/metalvj/agent-token)
curl -s -X POST http://127.0.0.1:7900/agent/tool \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"name":"sleep","args":{"ms":1}}'
```

## Test

```sh
cargo test -p metal-mcp -- --test-threads=1
```

`tests/door_tools.rs` spawns a headless `metal-dash` (built beside it: run
`cargo build -p metal-dash` first) on a free port with `--data <tempdir>`, then
calls `create_mask_grid`, `test_mask_grid` and `edit_venue_surface` through
metal-mcp stdio and checks the results and the venue file. It kills the child on
the way out.

`every_hardware_free_tool_answers_a_smoke_call` calls every listed tool that
needs no hardware once, against the same fixture. Each tool is either smoked,
skipped with a reason in `SKIP`, or fails the test; a new tool with neither
fails it.

The schema round-trip test is computed from the catalog; the stdio test
spawns the built binary and runs `initialize` and `tools/list`; `tests/http.rs`
checks the bearer gate (401 without or with a wrong token, token-file default,
refusal to start without a token, remote bind with a token). The forwarding
tests run against a fake metal-dash on a local port: they check the `/agent/tool`
body, `allow_ask`, and that a refusal becomes a tool error.

Checked by hand against a live metal-dash (`--no-window`, worktree `data/`): the
installed binary answers `initialize`, `tools/list` returns 58 tools with no
`capture_output`, `list_nodes`, `get_tick_level`, `set_tick_level`, `sleep`
and `locate` (PNG in, PNG back as image content) return results, and
`delete_node` is denied without `--allow-ask`.
