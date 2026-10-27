# wire-new-model — how a model is added to the whole program

A checklist so that the next model is not a research project. The living
example of every step is `dla` (the latest one to go in); the precedent for
state is `turing`.

## The map before touching anything

A model is NOT wired by hand everywhere. There are exactly **two sources of
truth**, and the rest of the program derives from them:

1. `crates/metal-render/src/registry.rs` — `all_models()`, **append-only**.
   `metal-dash`'s `/select` takes an index into this list; inserting in the
   middle moves an operator's on-air choice. A model ALWAYS goes in at the
   end.
2. `data/schema.json` — generated from the renderers
   (`cargo run -p metal-render --bin emit-schema`). `metal-graph` (node
   catalog), `metal-dash` (UI, `/select`, examples), `metal-agent` (harness) and
   `metal-schema` read it; nobody keeps a list of their own. The test
   `metal-render/tests/schema.rs` goes red if the file on disk is stale.

Everything else you touch is **counts written into tests** (generators in
`metal-graph` and `metal-dash`) and a short list in `data/router/model.json`.

## The checklist

### 1. The model — `crates/metal-render/src/models/<id>.rs`

- `Model` trait (`crates/metal-render/src/model.rs`): `id()` (ASCII,
  stable — renaming it orphans every example), `name()`,
  `schema()`, `render()`. Trait defaults: `footage` false,
  `backdrop` false, `camera_track` false — only override them if they apply.
- `Schema`: blocks in this order — `Block::Floats(FLOATS)`, colour slots,
  `fondo()` if the model covers the whole frame and composes its own
  background, and ALWAYS at the end `SIGNAL_FLOATS_BLOCK`,
  `EFFECTS_FLOATS_BLOCK`, `EFFECTS_COLOR_BLOCK`.
- **Floats and variability** (the whole point of the program):
  - `spec(name, min, max, default)` — 100% drivable (`drive: 1.0`): the
    audio digest can move it across its whole range.
  - `spec_held(name, min, max, default, drive)` — structural: it selects a
    regime, not an intensity; the drive is a weight that damps how much the
    signal can sweep it.
  - `spec_timed(...)` — when the smoothing should disagree with the column
    that drives it (see `FloatSpec::smooth`).
  - The rule of thumb: everything expressive (speed, size, brightness,
    colour) is `spec`; whatever changes *which* model it is (`semilla`,
    regime parameters, stability limits) is `spec_held`.
- **If it keeps state** (simulation, accumulation): do NOT re-derive from
  t=0. Follow the `turing`/`dla` precedent: the state lives in the model's
  struct (not inside `Built`, which is rebuilt per target format — see
  gotcha 4 below), it advances only when time advances, `schedule()` reseeds
  on a new seed / rewind / seek > `SEEK_GAP`, with `BURN_IN` when seeding and
  `MAX_STEPS_PER_FRAME` so that a dropped frame arrives as a slow-down. The
  promise to keep: *the same instant twice advances nothing and returns the
  same image* — the render gate demands it byte for byte.
- Unit tests in the file: the schema's width against the slices that
  `render()` reads by hand, and any constraint of its own (dla: the kill ring
  never leaves the grid).

### 2. The shader — `crates/metal-render/shaders/<id>.wgsl`

- `shared::build_shader` **already prepends the prelude** (`shaders/shared.wgsl`:
  `ColorSlot`, `grad4`, `backdrop_from_uv`, `lattice`, noise) — pass it
  only `format!("{VERTEX}\n{your_wgsl}")` if the model is 2D fullscreen
  (`VERTEX` from `shared::fullscreen`), or the wgsl alone if it brings its own
  vertex stage (3D with a mesh, `turing`-style).
- A binding is declared ONCE per (group, binding) in the whole module — the
  same buffer with a different access = another binding number (dla:
  `occ_rw` at 1 for compute, `occ` at 7 read-only for render).
- See the gotchas below (reserved words, etc.).

### 3. Wire it into metal-render

- `crates/metal-render/src/models/mod.rs` — `pub mod <id>;` at the end.
- `crates/metal-render/src/registry.rs` — `use crate::models::<id>::<Id>;`
  with the others, and `Box::new(<Id>::new())` **at the end of `all_models()`**
  with a dated comment that says what it is and why.
- If it keeps state: a test of its own in
  `crates/metal-render/tests/render.rs` (in the style of
  `turing_advances_with_time_and_holds_still_without_it`).

### 4. Regenerate the manifest

```
cargo run -p metal-render --bin emit-schema
```

It rewrites `data/schema.json` (with `drive` per field). Without this, the test
`the_manifest_on_disk_matches_the_renderers` fails — and that is all that
`metal-graph`, `metal-dash`, `metal-agent` and `metal-schema` need: the new `gen.<id>`
node shows up in the catalog by itself.

### 5. The two counts and the router list

- `data/router/model.json` — append the id to `classes` (the only list of
  models maintained by hand apart from the registry).
- `crates/metal-graph/src/lib.rs` — test `catalog_has_N_generators...`:
  N+1, and the message that says where the number comes from.
- `crates/metal-dash/src/main.rs` — the same N+1 in the test
  `the_catalog_route_serves_every_family` and in the `node_catalog` comment
  that states the count.
- Search for the old count with `rg "33|treinta" crates/` (or whatever it is)
  so as not to leave a third copy behind.

### 6. Run the gates

```
cargo test -p metal-render          # gate de render + schema + unit
cargo test -p metal-graph -p metal-schema -p metal-router -p metal-agent
cargo test -p metal-dash            # ojo: puede haber WIP ajeno en rojo
```

The render gate draws a frame at the defaults (not black, no NaN,
byte-identical when the instant is repeated, and also on a wide target).
`metal-dash`'s `check*.mjs` need a live server on :7900 — they are not part of
CI.

### 7. Document it

- `the design notes` — a section with the entry number, the floats in
  order and the design argument (see the one for `dla`, or `escena3d`'s
  H.1).

## Gotchas

1. **WGSL reserves words that look harmless**: `meta` and `active` are
   reserved and naga rejects them as variable/field names. Rename them
   (`bead`, `alive`).
2. **`build_shader` prepends the prelude by itself**. Prepending it again
   = `ColorSlot` redefined and a parse error far from the cause.
3. **One dispatch per compute pass**, `turing`-style: a new pass is the
   simple way of saying "the previous pass's writes are visible now".
4. **`Built<T>` is rebuilt when the target format changes**
   (window `Rgba16Float` vs preview `Rgba8UnormSrgb`). Everything that is
   model state (buffers, sim textures, counters) goes in the model's struct;
   `Built` holds only pipelines. `turing` pays for reseeds because of this;
   `dla` does not.
5. **Nothing transient in the image.** The gate compares the same instant
   twice: any "this frame" effect (accumulated particles, spark counters)
   breaks the equality. `dla`'s output with sparks works because they are a
   *readout* of the walkers' state baked every frame — a frame that did not
   advance draws the same sparks.
6. **`data/schema.json` is the only thing the consumers see** — if a float
   is missing there (because `emit-schema` was not run), the dash drives it
   with its default and drive 1.0, and a model whose structural floats are
   driven freely drifts into a white regime without anything saying so (the
   full story is in `metal-schema`'s `FloatSpec::drive`).
