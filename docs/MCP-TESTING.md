# MCP testing protocol

Every feature that the AI can reach through MCP is checked by MCP itself: a
step calls the tools, then asserts on what comes back. The runner is
`scripts/mcp-test` (Python 3, stdlib only). It drives `metal-mcp --http`, which
forwards to a running `metal-dash`.

## Layout

Steps live in a local store, outside the repo:

```
~/.cache/metalvj/mcp-tests/<domain>/<step>.json
```

`MVJ_MCP_TESTS` moves the store. Domains are plain directories. The runner
discovers them by name, so a new kind of development adds steps by creating a
directory and files; the runner does not change. Current domains: `models`,
`scenes`, `perform`, `dmx`, `venue`, `dataset`.

## Step schema

```json
{
  "id": "models-all",
  "domain": "models",
  "description": "What this step proves, in one sentence.",
  "setup_calls": [ { "tool": "take_graph", "args": { "graph": "g1" } } ],
  "calls":       [ { "tool": "set_params", "args": { "graph": "g1", "id": "n1", "patch": {} } } ],
  "expect":      [ { "path": "result.length", "op": "gte", "value": 1, "call": 0 } ],
  "last_run":    { "date": "2026-11-11T10:00:00", "ok": true, "detail": "3 expect ok" }
}
```

- `setup_calls` run first, in order. Their results are not checked.
- `calls` are the subject of the step. `expect` is evaluated against them.
- `call` in an expect picks a call by index (default: the last one).
- `path` addresses the parsed JSON of the tool's text reply. `a.b[0].c`; a
  trailing `.length` takes the length of a list, string or object. The
  specials `isError` and `text` read the raw MCP result.
- `op`: `eq`, `ne`, `gt`, `gte`, `lt`, `lte`, `contains`, `matches` (regex),
  `exists`.
- `last_run` is written by the runner. Do not edit it by hand.

## Running

```sh
scripts/mcp-test --list                       # every step and its last run
scripts/mcp-test --domain models              # one domain
scripts/mcp-test --step models-all            # one step
scripts/mcp-test                              # all domains
```

Options: `--dash URL` (default `http://127.0.0.1:7960`), `--mcp URL` (default
`http://127.0.0.1:7961/mcp`), `--no-start` (use the running services only).

The runner starts `metal-dash --no-window` and `metal-mcp --http` from
`$CARGO_TARGET_DIR` (default `~/.cache/mvj-targets/mcp-tests`) when they do
not answer, and stops only the PIDs it started. The bearer token is read from
`~/.config/metalvj/agent-token` (`FW_MCP_TOKEN` overrides). Services started by
the runner use a scratch data root under the target dir, never the real `data/`.

A step is green only if every call succeeded, every expect held and every tool
it names exists in `tools/list`. A missing tool fails the step with
`missing tool <name>`; it is never silently skipped (rule 10 of `plan.md`).

## Rules

- A step asserts observable state, not implementation. Read it back through a
  tool (`get_params`, `list_nodes`), not through the file.
- Steps never write into the real `data/`. The runner's scratch data root is
  the only data they touch.
- Add a step with the feature, in the same commit. Delete a step when its
  feature goes.

## Domain `models`

Step `models-all` builds the graph `test-all-models` and sweeps the model
controls from low to high, including a frenetic setting with fast transitions.
It wires every model that has trained weights: the colourist reels
`colourist-<model>.json` (fractals, kaleido, room, turing) and any weights the
catalog reports.

## Missing tools

Tools the protocol needs that `metal-agent`'s `TOOL_CATALOG` does not have:

- `create_graph` (or `new_graph`): no catalog tool creates a graph. The step
  needs an existing graph id to add nodes to. Until it exists the step uses
  the graph named `test-all-models` if it is already present and fails with
  `missing tool create_graph` otherwise.
- `tolerance`: no catalog param named `tolerance`. The sweep uses the router's
  `sensitivity` (0..1) where a node exposes it, and skips the tolerance sweep
  with a note otherwise.
