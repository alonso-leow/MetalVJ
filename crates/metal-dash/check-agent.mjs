#!/usr/bin/env node
// H.9's own harness for `metal-agent`: the corpus (`data/agent/shots.json`), the
// system prompt (`data/agent/system_prompt.md`), and the two rejection rules
// H.9's verification list names by hand — "a shader without declared floats
// is rejected" and "an incompatible `connect_nodes` is rejected".
//
// **Overlap with `crates/metal-agent/src/shots.rs`, read before adding to this
// file.** `shots.rs` already implements, in Rust, under `cargo test`:
//   - every tool in `harness::TOOL_CATALOG` (+ `question`) has >=2 shots
//     (`validate`'s per-tool coverage count);
//   - every shot's `tool_calls[].arguments` has the required keys for that
//     tool (`tool_schema`/`check_required_keys`, a hand transcription of
//     `harness.rs`'s own `require_str`/`.get(...)` calls).
// That is H.9 items 1 and 2 in full, already enforced, already exercised
// every `cargo test --workspace`. Re-implementing them here in JS against a
// second, independently-drifting copy of the same schema table is exactly
// the failure mode `shots.rs`'s own doc comment warns about ("a JS-side
// checker nobody remembers to update") — so this file does NOT re-check
// coverage or per-key schema shape. What it checks instead:
//
//   3. a shader with no declared floats is rejected — NOT covered by
//      `shots.rs` (which only checks that a `shader`/`shader_id` *key* is
//      present, never the WGSL text itself). Covered today only by
//      `crates/metal-forge/tests/forge.rs`'s in-process `compile()` calls,
//      which run inside `metal-forge`'s own crate, not from a shot's own
//      arguments. This file adds the corpus-facing half: a syntactic
//      approximation of `metal_forge::uniform::find_float_block` (regex over
//      `var<uniform> ... struct { floatsN: vec4<f32> ... }`, not a real
//      naga parse — see the caveat inline below), run against two literal
//      fixtures so the rule itself is proven here, in this harness, and not
//      just trusted by citation.
//   4. an incompatible `connect_nodes` is rejected — `connect_nodes`
//      (`harness.rs:362`) calls `metal_graph::Graph::add_edge_checked` directly,
//      in-process, with no HTTP route of its own (`metal-agent` cannot depend
//      on `metal-dash`, see `harness.rs`'s module doc). But `metal-dash`'s own
//      `POST /graph/edge` route (`main.rs:2326`) calls that *exact same*
//      function — `check-graph.mjs` section 3 already proves the rejection
//      through that door. This file re-proves it through the same door
//      (`/graph/edge` on a live `metal-dash`) rather than fabricating a second
//      mechanism, and says so rather than pretending it drove `connect_nodes`
//      itself — there is currently no HTTP surface that would let a Node
//      process call the tool by name.
//   5. `data/agent/system_prompt.md` loads and is non-trivial — cheap,
//      re-checked here too because this harness is meant to run standalone
//      against a live `metal-dash` without needing `cargo test` at all.
//
// Run:
//   ./target/debug/metal-dash --port 7900 --no-open &
//   node crates/metal-dash/check-agent.mjs
//
// Exits non-zero if any check fails.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const PORT = process.env.FW_DASH_PORT || 7900;
const BASE = `http://127.0.0.1:${PORT}`;
const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = join(HERE, "..", "..");

let failures = 0;
const ok = (name) => console.log(`  ok   ${name}`);
const fail = (name, detail) => {
  failures++;
  console.log(`  FAIL ${name}\n       ${detail}`);
};
function check(name, condition, detail = "") {
  condition ? ok(name) : fail(name, detail);
}

const get = async (path) => {
  const r = await fetch(BASE + path);
  if (!r.ok) throw new Error(`${path} → ${r.status}`);
  return r;
};
const json = async (path) => (await get(path)).json();
const post = (path, body) =>
  fetch(BASE + path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body || {}),
  });
async function addNode(graph, kind, pos) {
  const r = await post("/graph/node", { graph, op: "add", kind, pos });
  return (await r.json()).node;
}
function addEdge(graph, from, to) {
  return post("/graph/edge", { graph, op: "add", from, to });
}

console.log(`metal-agent harness → ${BASE}`);

// --- 1. shots.json and system_prompt.md exist and parse ---------------------
// `shots.rs` already validates the corpus's *tool-call* shape under
// `cargo test`; this section only proves the files this harness itself needs
// below are readable, not a second pass at that validation.
const shotsPath = join(REPO_ROOT, "data", "agent", "shots.json");
const promptPath = join(REPO_ROOT, "data", "agent", "system_prompt.md");

let corpus;
try {
  corpus = JSON.parse(readFileSync(shotsPath, "utf8"));
  check("data/agent/shots.json parses", Array.isArray(corpus.shots) && corpus.shots.length > 0, `${corpus.shots?.length}`);
} catch (e) {
  fail("data/agent/shots.json parses", String(e));
  corpus = { shots: [] };
}

// H.9 item 5: the system prompt loads and is non-trivial.
try {
  const prompt = readFileSync(promptPath, "utf8");
  check("data/agent/system_prompt.md loads", prompt.length > 500, `${prompt.length} bytes`);
  check(
    "the prompt is the harvested comments, not fresh paraphrase (spot check)",
    prompt.includes("un cuadro, no un") && prompt.includes("modelo") &&
      prompt.includes("nunca una segunda") && prompt.includes("verdad"),
    "expected near-verbatim phrases from the cited `//` comments"
  );
} catch (e) {
  fail("data/agent/system_prompt.md loads", String(e));
}

// --- 2. H.9 item 3: a shader with no declared floats is rejected -----------
// "declared floats, never baked" — a shader with baked constants is a
// picture, not a model (`the design notes`). The real gate is
// `metal_forge::uniform::find_float_block`, a naga-AST scan for a
// `var<uniform>` struct with a `floatsN: vec4<f32>` member
// (`crates/metal-forge/src/uniform.rs`). Node has no naga binding, so this is a
// deliberately lighter textual approximation of the same rule — good enough
// to catch a shot or a hand-written shader that never declares the block,
// not a substitute for `cargo test -p metal-forge`, which is what actually
// compiles WGSL and is the real authority on this rule.
function hasDeclaredFloatBlock(wgsl) {
  // A `var<uniform>` global bound to a struct, and that struct has at least
  // one member named `floats` or `floatsN` (N with no leading zero) typed
  // `vec4<f32>` — mirrors `is_floats_member_name` + `is_vec4_f32` in
  // `uniform.rs`, minus needing to actually resolve the struct type name.
  if (!/var\s*<\s*uniform\s*>/.test(wgsl)) return false;
  const structBodies = [...wgsl.matchAll(/struct\s+\w+\s*\{([^}]*)\}/gs)].map((m) => m[1]);
  const floatMember = /\bfloats([1-9]\d*)?\s*:\s*vec4<\s*f32\s*>/;
  return structBodies.some((body) => floatMember.test(body));
}

const SHADER_WITH_FLOATS = `
struct MeshUniform {
    floats: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: MeshUniform;
@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(u.floats.x, u.floats.y, u.floats.z, 1.0);
}
`;
const SHADER_BAKED = `
@fragment
fn fs_main() -> @location(0) vec4<f32> {
    // constants baked directly into the shader, with no floats block
    return vec4<f32>(0.8, 0.2, 0.1, 1.0);
}
`;

check(
  "a shader that declares a floats block is accepted by the approximation",
  hasDeclaredFloatBlock(SHADER_WITH_FLOATS),
  "expected the fixture with `floats: vec4<f32>` to be recognised"
);
check(
  "a shader with no declared floats (baked constants) is rejected",
  !hasDeclaredFloatBlock(SHADER_BAKED),
  "a shader with only baked constants is a picture, not a model — must be rejected"
);

// Corpus-facing half: any literal WGSL a shot hands to `create_node` should
// not itself be an example of the baked-constant anti-pattern. Shots in
// today's corpus that use `shader` carry a one-line prose comment
// describing the floats block rather than full WGSL (see
// `escena3d-create-a-tree-from-scratch`, `bake-a-wgsl-motion-graphics-clip`)
// — those are skipped here (too short to look like a real `var<uniform>`
// block either way) rather than false-failed for not being a compiler
// fixture.
for (const shot of corpus.shots || []) {
  for (const step of shot.steps || []) {
    for (const call of step.tool_calls || []) {
      if (call.name === "create_node" && typeof call.arguments?.shader === "string") {
        const src = call.arguments.shader;
        if (src.length < 200) continue; // prose placeholder, not real WGSL
        check(
          `shot ${shot.id}: inline shader declares a floats block`,
          hasDeclaredFloatBlock(src),
          "a full WGSL shot fixture must declare floatsN: vec4<f32>, not bake constants"
        );
      }
    }
  }
}

// --- 3. H.9 item 4: an incompatible connect_nodes is rejected ---------------
// `connect_nodes` (`harness.rs:362`) and `metal-dash`'s `POST /graph/edge`
// (`main.rs:2326`) both call `metal_graph::Graph::add_edge_checked` — the same
// function, not two implementations of the same rule. `check-graph.mjs`
// section 3 already proves this rejection through the HTTP door; this
// re-proves it here so `check-agent.mjs` stands on its own for H.9's
// checklist, using the identical mechanism `connect_nodes` wraps (there is
// no HTTP surface for the agent tool itself to drive directly — see the
// file header).
{
  const GRAPH_ID = "check-agent-connect-nodes";
  await post("/graph", { op: "delete", id: GRAPH_ID }); // best-effort, stale run

  const catalog = (await json("/catalog")).catalog;
  const gen = catalog.find((k) => k.family === "generator");
  const cam = catalog.find((k) => k.kind === "cam.track");
  check(
    "the catalog has a generator (Image out) and cam.track (Control in) to build the mismatch",
    !!gen && !!cam,
    JSON.stringify({ gen: gen?.kind, cam: cam?.kind })
  );

  if (gen && cam) {
    await post("/graph", { op: "create", id: GRAPH_ID, name: "check-agent connect_nodes", domain: "live" });
    const genId = await addNode(GRAPH_ID, gen.kind, [0, 0]);
    const camId = await addNode(GRAPH_ID, cam.kind, [200, 0]);

    const before = await json(`/graph?id=${GRAPH_ID}`);
    const bad = await addEdge(GRAPH_ID, { node: genId, port: "image" }, { node: camId, port: "control" });
    check(
      "an incompatible connect_nodes (Image -> Control) is rejected",
      bad.status === 400,
      `${bad.status}`
    );
    const after = await json(`/graph?id=${GRAPH_ID}`);
    check(
      "the rejected cable never lands in the saved graph",
      JSON.stringify(before.graph) === JSON.stringify(after.graph),
      "a rejected connect_nodes must never be what save_graph writes"
    );

    await post("/graph", { op: "delete", id: GRAPH_ID });
  }
}

console.log(failures === 0 ? `\nall checks passed` : `\n${failures} FAILED`);
process.exit(failures === 0 ? 0 : 1);
