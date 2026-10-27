#!/usr/bin/env node
// The node-editor's own harness — set D1/B4/B5, the mutation
// surface over `/graphs`, `/graph`, `/graph/node`, `/graph/edge`,
// `/graph/effects` and `/catalog`. The old page harnesses (`check.mjs`,
// `check-ui.mjs`, deleted 15-sept-2026 with the page itself, S.U18) covered
// the rest; this one is not a page harness — no DOM, no browser. The graph editor
// is a canvas that paints from primitives the server already computed
// (`the design notes` §5), so what needs proving lives entirely on the
// HTTP side: a bad cable is refused, a cycle is refused, deleting a node
// takes its edges with it, a save round-trips byte-identical, and a
// generator's effect chain reads back in the order it was wired — never a
// second list, per `metal_graph::effects`' own top doc.
//
//   ./target/debug/metal-dash --port 7900 --no-window &
//   node crates/metal-dash/check-graph.mjs
//
// Every graph this harness creates is deleted again before it exits (section
// 8 proves it) — `data/graphs/*.json` is real collection state, so it cleans up
// after itself for the same reason the old page harnesses did: `data/`
// is real state.
// `data/graphs/empty-1.json` is not this harness's and is never touched.
//
// Exits non-zero on the first count of failures, with what was expected and
// what came back.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const PORT = process.env.FW_DASH_PORT || 7900;
const BASE = `http://127.0.0.1:${PORT}`;
const HERE = dirname(fileURLToPath(import.meta.url));

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

// Small conveniences over the two POSTs every section below leans on. Neither
// hides a status check — callers still assert `.ok`/`.status` themselves —
// they just save re-typing the op envelope. `addNode` assumes success (the
// call sites that expect failure post to `/graph/node` directly, same as
// `addEdge` never resolves to a bare id because half its callers want the
// rejection, not the edge).
async function addNode(graph, kind, pos) {
  const r = await post("/graph/node", { graph, op: "add", kind, pos });
  return (await r.json()).node;
}
function addEdge(graph, from, to) {
  return post("/graph/edge", { graph, op: "add", from, to });
}

console.log(`metal-dash node-editor harness → ${BASE}`);

// Every graph this run touches, named obviously so a leftover from a crashed
// run is unmistakable in `data/graphs/` rather than looking like real work.
const GRAPH_IDS = {
  basics: "check-graph-basics",
  cable: "check-graph-cable",
  cycle: "check-graph-cycle",
  del: "check-graph-delete",
  rt: "check-graph-roundtrip",
  chain: "check-graph-chain",
  signal: "check-graph-signal",
  signalOffline: "check-graph-signal-offline",
  ops: "check-graph-ops",
  embed: "check-graph-embed",
};

// Best-effort: `POST /graph {op:"create"}` refuses an id that already
// exists, so a stale file from an earlier crashed run of this same harness
// would take every section below down with it rather than just the one case
// that actually broke last time. Failing to delete something that was never
// there is expected and ignored.
for (const id of Object.values(GRAPH_IDS)) {
  await post("/graph", { op: "delete", id });
}

// --- 1. The catalog ----------------------------------------------------------
// Every later section reads its node kinds off here rather than hard-coding
// `gen.turing`/`fx.shake` — `data/schema.json` is exactly the thing this repo
// has already shipped a hand-written-list-vs-manifest drift bug over once
// (see `metal_graph::NodeKind::catalog`'s own doc comment on why the catalog is
// generated rather than literal).
const catalog = (await json("/catalog")).catalog;
check("the catalog answers with something in it", Array.isArray(catalog) && catalog.length > 0);

const generators = catalog.filter((k) => k.family === "generator");
const fxKinds = catalog.filter((k) => k.family === "effect");
check("there is at least one generator kind", generators.length > 0, `${generators.length}`);
check("there are at least three effect kinds", fxKinds.length >= 3, `${fxKinds.length}`);

const GEN = generators[0];
check(
  "a generator exposes image (Image) + row (Control) outputs and a control (Control) input",
  GEN.outputs.some((p) => p.name === "image" && p.ty === "image") &&
    GEN.outputs.some((p) => p.name === "row" && p.ty === "control") &&
    GEN.inputs.some((p) => p.name === "control" && p.ty === "control"),
  JSON.stringify(GEN)
);
// The rename this harness's brief itself warns about: `row` was `router` for
// one commit and nothing on the wire should still be offering that name.
check(
  "no generator still exposes the old `router` output name",
  generators.every((k) => !k.outputs.some((p) => p.name === "router")),
  "the row/router rename must be all the way through the catalog, not just the code that reads it"
);

const CAM = catalog.find((k) => k.kind === "cam.track");
check(
  "cam.track is a fixed catalog kind with a Control-typed control input",
  !!CAM && CAM.inputs.length === 1 && CAM.inputs[0].name === "control" && CAM.inputs[0].ty === "control",
  JSON.stringify(CAM)
);

// --- 2. A graph is created, lists, loads, and validates node kinds ----------
{
  const id = GRAPH_IDS.basics;
  const created = await post("/graph", { op: "create", id, name: "check-graph basics", domain: "live" });
  check("a graph can be created", created.ok, `${created.status}`);
  const createdBody = await created.json();
  check(
    "a fresh graph starts with no nodes or edges",
    createdBody.graph.nodes.length === 0 && createdBody.graph.edges.length === 0,
    JSON.stringify(createdBody.graph)
  );

  const listed = (await json("/graphs")).graphs;
  check(
    "GET /graphs lists the graph just created",
    listed.some((g) => g.id === id && g.name === "check-graph basics" && g.domain === "live"),
    JSON.stringify(listed.find((g) => g.id === id))
  );

  const loaded = await json(`/graph?id=${id}`);
  check("GET /graph reloads it by id", loaded.graph.id === id, loaded.graph.id);
  check(
    "a loaded graph carries primitives and issues alongside it, not just the graph",
    Array.isArray(loaded.primitives) && Array.isArray(loaded.issues)
  );

  // N: every primitive is *tagged* — a painter reads the tag, never a size,
  // to know a node box from a header band from a port dot from a value field.
  // An empty graph has no primitives; a graph with one generator has at least
  // a node box, a header band, a title, a port dot and value fields.
  const genNode = await addNode(id, GEN.kind, [0, 0]);
  const withGen = await json(`/graph?id=${id}`);
  const tags = new Set(withGen.primitives.map((p) => p.tag));
  for (const tag of ["node", "header", "title", "port", "value_field"]) {
    check(`a generator's primitives carry a \`${tag}\` tag`, tags.has(tag),
      [...tags].join(","));
  }
  const portPrim = withGen.primitives.find((p) => p.tag === "port");
  check("a tagged port names its node, its port and its direction",
    !!portPrim && typeof portPrim.node === "string" && typeof portPrim.port === "string" &&
      typeof portPrim.out === "boolean",
    JSON.stringify(portPrim));
  const fieldPrim = withGen.primitives.find((p) => p.tag === "value_field");
  check("a tagged value field names its node and parameter",
    !!fieldPrim && fieldPrim.node === genNode && typeof fieldPrim.name === "string",
    JSON.stringify(fieldPrim));
  // A cable is tagged with its two endpoints and its kind (image/control/param).
  const fxNode = await addNode(id, fxKinds[0].kind, [300, 0]);
  await addEdge(id, { node: genNode, port: "image" }, { node: fxNode, port: "image" });
  const cabled = await json(`/graph?id=${id}`);
  const cable = cabled.primitives.find((p) => p.type === "path");
  check("a cable is tagged with its endpoints and its kind",
    !!cable && cable.kind === "image" && cable.from.node === genNode && cable.to.node === fxNode,
    JSON.stringify(cable));

  // `GET /graph/hit` answers a `param` for a value field and a `port` whose
  // name is `param:<name>` for a parameter dot (N) — the two targets of one
  // row, so a drag edits the field and a cable binds the dot.
  const fx2 = await json(`/graph?id=${id}`);
  const fxField = fx2.primitives.find((p) => p.tag === "value_field" && p.node === fxNode);
  check("the effect has a laid-out value field to hit", !!fxField, JSON.stringify(fxField));
  const fieldHit = await json(
    `/graph/hit?id=${id}&x=${fxField.x + fxField.w / 2}&y=${fxField.y + fxField.h / 2}`
  );
  check("a hit on a value field answers `param`",
    fieldHit.hit === "param" && fieldHit.node === fxNode && fieldHit.name === fxField.name,
    JSON.stringify(fieldHit));
  const dot = fx2.primitives.find((p) => p.tag === "param_dot" && p.node === fxNode);
  check("the effect has a laid-out parameter dot to hit", !!dot, JSON.stringify(dot));
  const dotHit = await json(`/graph/hit?id=${id}&x=${dot.cx}&y=${dot.cy}`);
  check("a hit on a parameter dot answers its `param:<name>` port",
    dotHit.hit === "port" && dotHit.node === fxNode && dotHit.port === `param:${dot.name}` &&
      dotHit.dir === "in",
    JSON.stringify(dotHit));

  // `GET /graph/live` answers the engine's last frame for the graph, or a
  // null `t` when nobody renders it (the canvas polls this at 4–5 Hz).
  const live = await json(`/graph/live?id=${id}`);
  check("GET /graph/live answers the graph and its frame age",
    live.graph === id && "t" in live && "nodes" in live, JSON.stringify(live));

  check(
    "loading a graph that does not exist is a 400, not a 500 or a blank graph",
    (await fetch(`${BASE}/graph?id=check-graph-does-not-exist`)).status === 400
  );

  // `POST /graph/node` checks `kind` against the same catalog `/catalog`
  // serves — see main.rs's own comment on why: an unchecked kind becomes a
  // node `layout`/`lint` cannot make sense of instead of a 400 naming the typo.
  const badKind = await post("/graph/node", { graph: id, op: "add", kind: "gen.not-a-real-model" });
  check("adding a node of an unknown kind is refused", badKind.status === 400, `${badKind.status}`);

  await post("/graph", { op: "delete", id });
}

// --- 3. An invalid cable is rejected -----------------------------------------
// metal_graph::lint's `lint_dangling_and_port_types`, reached through
// `add_edge_checked`'s tentative-add-then-lint-then-rollback: a port-type
// mismatch is an error, and the edge must never make it into the saved
// graph. Section 6 below establishes, independently, why comparing the
// *whole* graph object before/after (not just `edges.length`) is a safe
// thing to assert: a rejected mutation is never the thing `save_graph`
// writes, so nothing about the on-disk graph — down to the id counters —
// can have moved.
{
  const id = GRAPH_IDS.cable;
  await post("/graph", { op: "create", id, name: "cable", domain: "live" });

  const gen = await addNode(id, GEN.kind, [0, 0]);
  const cam = await addNode(id, "cam.track", [200, 0]);

  const before = await json(`/graph?id=${id}`);

  // The generator's `image` output (Image) into cam.track's `control` input
  // (Control) — both ports are real, the types disagree. The same mismatch
  // `add_edge_checked_rejects_mismatched_port_types` proves in-process.
  const bad = await addEdge(id, { node: gen, port: "image" }, { node: cam, port: "control" });
  check("a mismatched cable (Image -> Control) is rejected", bad.status === 400, `${bad.status}`);
  const badMsg = await bad.text();
  check("the rejection names the mismatch, not a generic failure", badMsg.includes("mismatch"), badMsg);

  const after = await json(`/graph?id=${id}`);
  check(
    "the graph is unchanged after a rejected cable",
    JSON.stringify(before.graph) === JSON.stringify(after.graph),
    "a rejected edge must never be the thing that ends up on disk"
  );

  await post("/graph", { op: "delete", id });
}

// --- 4. A cycle is rejected ----------------------------------------------------
{
  const id = GRAPH_IDS.cycle;
  await post("/graph", { op: "create", id, name: "cycle", domain: "live" });

  const fx1 = await addNode(id, fxKinds[0].kind, [0, 0]);
  const fx2 = await addNode(id, fxKinds[1].kind, [200, 0]);

  const wired = await addEdge(id, { node: fx1, port: "image" }, { node: fx2, port: "image" });
  check("fx1 -> fx2 wires cleanly", wired.ok, `${wired.status}`);

  const before = await json(`/graph?id=${id}`);
  // Closing the loop: fx2's image back into fx1's image.
  const cyc = await addEdge(id, { node: fx2, port: "image" }, { node: fx1, port: "image" });
  check("closing the loop (fx2 -> fx1) is rejected", cyc.status === 400, `${cyc.status}`);
  const cycMsg = await cyc.text();
  check("the rejection names the cycle", cycMsg.includes("cycle"), cycMsg);

  const after = await json(`/graph?id=${id}`);
  check(
    "the graph is unchanged after a rejected cycle",
    JSON.stringify(before.graph) === JSON.stringify(after.graph),
    "a rejected edge must never be the thing that ends up on disk"
  );

  await post("/graph", { op: "delete", id });
}

// --- 5. Deleting a node deletes its edges --------------------------------------
// `metal_graph::Graph::remove_node`: "removes a node and every edge touching
// it." Deliberately not the healing `metal_graph::effects::remove_and_heal`
// does when an effect is turned off — plain node deletion just drops every
// cable that named it, so the sharper case is a *middle* node with an edge
// on each side: two edges must go, not one, and neither may survive pointing
// at a node that is no longer there.
{
  const id = GRAPH_IDS.del;
  await post("/graph", { op: "create", id, name: "delete", domain: "live" });

  const gen = await addNode(id, GEN.kind, [0, 0]);
  const fx1 = await addNode(id, fxKinds[0].kind, [200, 0]);
  const fx2 = await addNode(id, fxKinds[1].kind, [400, 0]);
  await addEdge(id, { node: gen, port: "image" }, { node: fx1, port: "image" });
  await addEdge(id, { node: fx1, port: "image" }, { node: fx2, port: "image" });

  const mid = await json(`/graph?id=${id}`);
  check("both edges are wired before the delete", mid.graph.edges.length === 2, `${mid.graph.edges.length}`);

  const removed = await post("/graph/node", { graph: id, op: "remove", node: fx1 });
  check("removing the middle node succeeds", removed.ok, `${removed.status}`);
  const removedBody = await removed.json();
  check("the node itself is gone", !removedBody.graph.nodes.some((n) => n.id === fx1));
  check(
    "both edges that touched it are gone too, not just one",
    removedBody.graph.edges.length === 0,
    JSON.stringify(removedBody.graph.edges)
  );
  check(
    "no surviving edge references the deleted node from either end",
    removedBody.graph.edges.every((e) => e.from.node !== fx1 && e.to.node !== fx1)
  );

  await post("/graph", { op: "delete", id });
}

// --- 6. Save then reload is byte-identical --------------------------------------
// `metal_graph::Graph::to_json` sorts nodes/edges by id before writing, so the
// serialized form is a pure function of content rather than of insertion
// order or which request happened to touch it last — `round_trip_is_byte_
// identical` in metal-graph/src/lib.rs proves this in-process. This proves the
// same thing through the real server and the real file on disk: `move`
// always calls `save_graph`, even when the position given is the one the
// node already has, so moving a node to its own position is a clean way to
// force a second load -> save cycle with content that has not actually
// changed, and check that the bytes on disk agree with themselves.
{
  const id = GRAPH_IDS.rt;
  await post("/graph", { op: "create", id, name: "roundtrip", domain: "live" });
  const gen = await addNode(id, GEN.kind, [12, 34]);
  await addNode(id, fxKinds[0].kind, [212, 34]);

  const path = join(HERE, "..", "..", "data", "graphs", `${id}.json`);
  const bytes1 = readFileSync(path);
  check("the graph file lands where metal-dash's own data_root() puts it", bytes1.length > 0, path);

  const moved = await post("/graph/node", { graph: id, op: "move", node: gen, pos: [12, 34] });
  check("a no-op move still succeeds", moved.ok, `${moved.status}`);
  const bytes2 = readFileSync(path);

  check(
    "save, reload, save again is byte-identical",
    Buffer.compare(bytes1, bytes2) === 0,
    `${bytes1.length} vs ${bytes2.length} bytes`
  );

  await post("/graph", { op: "delete", id });
}

// --- 7. An effect chain reads back in graph order -------------------------------
// B4/B5 (`metal_graph::effects`): a generator's effects are not a second list —
// they are the fx.* nodes chained downstream of its `image` output, and
// `set_effect` always splices a newly-turned-on effect onto the *end* of
// that chain (`§3.2: "in the order they were switched on"`). So three effects
// turned on in order A, B, C must read back in exactly that order, off the
// graph, through `GET /graph/effects` — not just in whatever
// `POST /graph/effects` happened to echo back for its own call.
{
  const id = GRAPH_IDS.chain;
  await post("/graph", { op: "create", id, name: "chain", domain: "live" });
  const gen = await addNode(id, GEN.kind, [0, 0]);

  const [a, b, c] = [fxKinds[0].kind, fxKinds[1].kind, fxKinds[2].kind].map((k) => k.replace(/^fx\./, ""));

  const setA = await post("/graph/effects", { graph: id, node: gen, effect: a, state: "fixed", value: 0.3 });
  check(`turning on ${a} (fijo) succeeds`, setA.ok, `${setA.status}`);

  // `row`: the current name for "inherit this generator's own computed
  // amount" — see section 1's check on the row/router rename.
  const setB = await post("/graph/effects", { graph: id, node: gen, effect: b, state: "reactive", source: "row" });
  check(`turning on ${b} (reactivo, row-sourced) succeeds`, setB.ok, `${setB.status}`);

  // `router` is the retired name for the same source, kept accepted on the
  // wire so a dashboard left open across the rename still works — see
  // main.rs's own comment on `POST /graph/effects`'s `source` field.
  const setC = await post("/graph/effects", { graph: id, node: gen, effect: c, state: "reactive", source: "router" });
  check(`turning on ${c} with the retired "router" alias still succeeds`, setC.ok, `${setC.status}`);

  const view = await json(`/graph/effects?id=${id}&node=${gen}`);
  const present = view.effects.filter((e) => e.state !== "off");
  check("exactly the three effects turned on are present", present.length === 3, JSON.stringify(present));
  check(
    "they read back in the order they were wired, not catalog order",
    present.map((e) => e.effect).join(",") === [a, b, c].join(","),
    present.map((e) => e.effect).join(",")
  );
  check(
    `${a} reads back fijo with its value`,
    present[0]?.state === "fixed" && Math.abs(present[0].value - 0.3) < 1e-6,
    JSON.stringify(present[0])
  );
  check(
    `${b} reads back reactivo, row-sourced`,
    present[1]?.state === "reactive" && present[1].source === "row",
    JSON.stringify(present[1])
  );
  check(
    `${c} reads back reactivo too — the retired "router" alias normalises to "row" on the wire`,
    present[2]?.state === "reactive" && present[2].source === "row",
    JSON.stringify(present[2])
  );

  // Ground truth: the wiring itself, not just what the effects endpoint
  // claims about it. gen --image--> A --image--> B --image--> C.
  const g = (await json(`/graph?id=${id}`)).graph;
  const imageEdge = (fromNode, toNode) =>
    g.edges.some(
      (e) => e.from.node === fromNode && e.from.port === "image" && e.to.node === toNode && e.to.port === "image"
    );
  check(
    "the graph itself chains gen -> A -> B -> C on the image ports",
    present.length === 3 &&
      imageEdge(gen, present[0].node) &&
      imageEdge(present[0].node, present[1].node) &&
      imageEdge(present[1].node, present[2].node),
    JSON.stringify(g.edges)
  );

  await post("/graph", { op: "delete", id });
}

// --- 7b. The destinations that are not screens -----------------------------------
// DDP and JSON for screw-in bulbs, E1.31 (sACN)/OSC/HTTP/MIDI for DMX. They
// were a stub dropdown ("extensiones") before they were nodes, and the whole
// point of making them nodes is that they obey the same rules as everything
// else — so this asserts the rules and not the six labels.
{
  const outs = catalog.filter((k) => k.family === "output_signal");
  check("the catalog publishes the seven non-pixel destinations", outs.length === 7,
    outs.map((k) => k.kind).join(", "));
  check("every one of them ends the graph rather than continuing it",
    outs.every((k) => k.outputs.length === 0),
    "a destination with an output port would invite a cable out of the end of the show");

  const id = GRAPH_IDS.signal;
  await post("/graph", { op: "create", id, name: "signal out", domain: "live" });
  const gen = await addNode(id, GEN.kind, [0, 0]);

  // The wiring the whole family exists for: the eight `sig_*` floats leave the
  // generator on `row` and land on the lamp. Asserted against every protocol,
  // because "it works for sACN" is not the claim — the claim is that the
  // model's row reaches any of them without a conversion in between.
  for (const k of outs) {
    const node = await addNode(id, k.kind, [200, 0]);
    const r = await addEdge(id, { node: gen, port: "row" }, { node, port: "signal" });
    check(`a generator's row reaches ${k.kind}`, r.ok, `${r.status} ${await r.text?.() ?? ""}`);

    // And the mirror, which is the part a comment cannot enforce: the picture
    // is not a light cue. `image -> signal` is Image into Control and must be
    // refused by the same type check that refuses every other mismatch.
    const bad = await addEdge(id, { node: gen, port: "image" }, { node, port: "signal" });
    check(`and its image does not — ${k.kind} refuses pixels on signal`,
      bad.status === 400, `${bad.status}`);
  }

  // Only the two strip protocols take an image at all, and for those two it
  // means something: a bulb string and an LED bar are addressed as a run of
  // RGB. A MIDI note is not a scanline.
  const withImage = outs.filter((k) => k.inputs.some((p) => p.ty === "image")).map((k) => k.kind);
  check("only DDP and sACN accept an image, because only they address a strip",
    withImage.sort().join(",") === "out.ddp,out.sacn", withImage.join(","));

  await post("/graph", { op: "delete", id });
}

// --- 7c. A lamp is lit now or not at all -----------------------------------------
// The domain rule, from the other side. Authoring a light cue offline is
// fine and is not what this forbids — the `sig_*` floats are keyframed in an
// Offline graph like any other float. It is the wire out of the building that
// has to be Live, because an Offline render has no wall clock to be lit at.
{
  const id = GRAPH_IDS.signalOffline;
  await post("/graph", { op: "create", id, name: "offline", domain: "offline" });
  const node = await addNode(id, "out.midi", [0, 0]);
  const g = await json(`/graph?id=${id}`);
  const errs = (g.issues || []).filter((i) => i.severity === "error" && i.node === node);
  check("a signal output in an Offline graph is an error", errs.length > 0,
    JSON.stringify(g.issues));
  check("and the error says where the cue does belong instead",
    errs.some((i) => i.message.includes("sig_*")),
    "an operator who put the wire in the wrong graph is not doing anything wrong");

  await post("/graph", { op: "delete", id });
}

// --- 7d. What can be done to a node ----------------------------------------------
// `bypass` and `solo` have been fields on `metal_graph::Node` since A1 with
// nothing able to set them, and there was no way to copy a node at all. All
// three are properties of the *document*, so the test that matters is that
// they survive a reload — an operator who bypasses a node and comes back
// should find it bypassed.
{
  const id = GRAPH_IDS.ops;
  await post("/graph", { op: "create", id, name: "ops", domain: "live" });
  const gen = await addNode(id, GEN.kind, [0, 0]);

  const nodeOf = async (nid) =>
    (await json(`/graph?id=${id}`)).graph.nodes.find((n) => n.id === nid);

  check("a node starts plain", (await nodeOf(gen)).bypass === false);

  let r = await post("/graph/node", { op: "flag", graph: id, node: gen, field: "bypass", on: true });
  check("bypass can be set", r.ok, `${r.status}`);
  check("and it is on the graph, not in the page", (await nodeOf(gen)).bypass === true,
    "a flag that only lives in the browser is gone on the next reload");

  r = await post("/graph/node", { op: "flag", graph: id, node: gen, field: "solo", on: true });
  check("solo is a separate flag, not the same one renamed", r.ok && (await nodeOf(gen)).solo === true);
  check("and setting it left bypass alone", (await nodeOf(gen)).bypass === true);

  // No `on` at all means flip — the only sensible reading, and what a
  // keyboard shortcut would send.
  await post("/graph/node", { op: "flag", graph: id, node: gen, field: "bypass" });
  check("a flag op with no value toggles", (await nodeOf(gen)).bypass === false);

  const bad = await post("/graph/node", { op: "flag", graph: id, node: gen, field: "invisible" });
  check("an unknown flag is refused rather than silently ignored", bad.status === 400, `${bad.status}`);
  check("and the refusal names the two that exist",
    (await bad.text()).includes("bypass"), "an error that does not say what is allowed costs a second try");

  // Duplicate: same kind, offset, and deliberately unwired.
  const dupR = await post("/graph/node", { op: "duplicate", graph: id, node: gen });
  check("a node can be duplicated", dupR.ok, `${dupR.status}`);
  const copyId = (await dupR.json()).node;
  const copy = await nodeOf(copyId);
  check("the copy is the same kind", copy && copy.kind === GEN.kind, JSON.stringify(copy));
  check("and does not land exactly on the original, which would read as nothing happening",
    copy.pos[0] !== 0 || copy.pos[1] !== 0, JSON.stringify(copy.pos));
  check("a copy arrives plain — inheriting solo would clear the stage the instant it appeared",
    copy.solo === false && copy.bypass === false);
  check("and unwired: a duplicate wired like its original would fan into one input",
    !(await json(`/graph?id=${id}`)).graph.edges.some(
      (e) => e.from.node === copyId || e.to.node === copyId));

  await post("/graph", { op: "delete", id });
}

// --- 7e. The embedded reaction is a chip, not a free node -----------------------
// Plan "AI embedded in the source's properties" (A1/A3.2): `ai.embed` creates
// the owned `ai.*` node with `embedded_in` and the chain `audio → ai →
// source.control`. `GET /graph` then lays it out as a `chip` on the source and
// not as a node box, and the chip's wires are in the primitives (hidden by the
// painter until the source is selected).
{
  const id = GRAPH_IDS.embed;
  await post("/graph", { op: "create", id, name: "embed", domain: "live" });
  const gen = await addNode(id, GEN.kind, [0, 0]);

  const embedded = await post("/graph/node", {
    graph: id, op: "ai.embed", node: gen, ai: "ai.colourist", on: true,
  });
  check("ai.embed succeeds", embedded.ok, `${embedded.status}`);
  const body = await embedded.json();
  const ai = body.node;
  check("the reply names the owned ai node", typeof ai === "string" && ai.length > 0, JSON.stringify(body));

  const loaded = await json(`/graph?id=${id}`);
  const aiNode = loaded.graph.nodes.find((n) => n.id === ai);
  check("the ai node is owned by the source, not free",
    aiNode && aiNode.embedded_in === gen, JSON.stringify(aiNode));
  const chip = loaded.primitives.find((p) => p.tag === "chip");
  check("the embedded ai is laid out as a chip",
    !!chip && chip.node === ai && chip.source === gen, JSON.stringify(chip));
  const boxes = loaded.primitives.filter((p) => p.tag === "node");
  check("and not as a free node box", boxes.length === 1, `${boxes.length} node boxes`);
  const wires = loaded.primitives.filter((p) => p.type === "path" &&
    (p.from.node === ai || p.to.node === ai));
  check("its two wires are in the primitives (so a selected source can show them)",
    wires.length === 2, `${wires.length}`);

  await post("/graph", { op: "delete", id });
}

// --- 8. S.A: Mask grids — invalid grids are rejected without saving state --------
// Per plan-sonnet.md S.A.4: an invalid grid that gets rejected must leave state
// untouched. Precedent: check-agent.mjs's same rule for connect_nodes.
{
  // First, create a device to work with
  const devAddResp = await post("/outputs", { op: "add", id: "check-graph-mask-device", label: "test device", width: 1920, height: 1080 });
  check("a test device is created for mask grid tests", devAddResp.ok, `${devAddResp.status}`);
  const devList1 = (await json("/state")).devices || [];
  const dev1 = devList1.find(d => d.id === "check-graph-mask-device");
  check("the test device appears in the state", !!dev1, "could not find the device after creation");

  // Try to create an invalid grid (0 rows/cols)
  const invalidGridZero = await post("/mask/grid", { device_id: "check-graph-mask-device", rows: 0, cols: 5 });
  check("creating a grid with 0 rows is rejected", invalidGridZero.status === 400, `${invalidGridZero.status}`);

  // Check that state was not mutated by the rejection
  const devList2 = (await json("/state")).devices || [];
  const dev2 = devList2.find(d => d.id === "check-graph-mask-device");
  check(
    "the rejected grid never lands in saved state (0 rows case)",
    (dev1?.masks?.length || 0) === (dev2?.masks?.length || 0),
    `masks before: ${dev1?.masks?.length}, after: ${dev2?.masks?.length}`
  );

  // Try another invalid grid (> 100 cols)
  const invalidGridLarge = await post("/mask/grid", { device_id: "check-graph-mask-device", rows: 5, cols: 101 });
  check("creating a grid with cols > 100 is rejected", invalidGridLarge.status === 400, `${invalidGridLarge.status}`);

  const devList3 = (await json("/state")).devices || [];
  const dev3 = devList3.find(d => d.id === "check-graph-mask-device");
  check(
    "the rejected grid never lands in saved state (cols > 100 case)",
    (dev2?.masks?.length || 0) === (dev3?.masks?.length || 0),
    `masks before: ${dev2?.masks?.length}, after: ${dev3?.masks?.length}`
  );

  // Try a grid on a nonexistent device
  const invalidGridBadDev = await post("/mask/grid", { device_id: "no-such-device", rows: 3, cols: 3 });
  check("creating a grid on a nonexistent device is rejected", invalidGridBadDev.status === 400, `${invalidGridBadDev.status}`);

  // Clean up the test device
  const removeResp = await post("/outputs", { op: "remove", id: "check-graph-mask-device" });
  check("test device is removed", removeResp.ok, `${removeResp.status}`);
}

console.log(failures === 0 ? `\nall checks passed` : `\n${failures} FAILED`);
process.exit(failures === 0 ? 0 : 1);
