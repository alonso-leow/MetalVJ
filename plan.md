# MetalVJ — Plan

The single plan document. **It holds only work still to do and the standing
decisions that work needs.** The code is the source of truth for what is
built; built material with lasting value lives in `docs/DOCS.md`.

**How this file is edited (binding for every agent).**

- **Edit the item, never append a report.** When work lands, delete the
  finished step or rewrite the line that is now wrong, in the same commit.
  No dated "Status (…)", "what landed", per-session or per-model sections, no
  lists of new floats/models/shaders: those belong in the code, the commit
  message, or `docs/DOCS.md`.
- A new model, look or effect is not a plan item unless it blocks one below.
- Side status files (`docs/WORK-STATUS.md`, `*-plan.md` at the root) are
  temporary; fold what is still pending into this file and delete them.

Contents: rules · collections · **next steps** · the shell · the room
(masks, venue, pixel map) · audio and trained reaction · the performance
model · other pending work · legacy IDs.

## How to use this document

- **A batch is one dispatchable unit**; its steps run in order. Parallel
  batches only on disjoint files, each in its own worktree in `~/Projects`,
  own branch, own `CARGO_TARGET_DIR`. An error in a file you did not touch is
  another batch mid-edit: wait.
- **Blocked batches are not started**; don't invent structure a pending
  decision will fix.
- **Measure before dispatching.** A batch that turns out to be a whole canvas
  gets its own session.
- Items end with **Escalate if**: stop and ask instead of solving.

## Rules no batch may break

1. **`registry::all_models()` is append-only.** Retired models stay as
   tombstones; `Layer::model` stores the positional index.
2. **Floats are appended at the end of their block**, never in the middle.
3. **One origin.** An output is a view of the frame; `output.rs` never picks
   a model.
4. **Config is a reading of the graph**, never a second state.
5. **Graph layout lives in Rust.** The panel paints resolved primitives;
   hit-testing goes through `GET /graph/hit`.
6. **The AI uses the same routes as the human.** A tool with no control, or
   a control with no route, is two paths to one thing.
7. **No hand-written count** of models/generators/effects in a test: compute
   it from the manifest.
8. **`data/` is not fully versioned** (`collections.json`, `empty-1` live
   only in the main checkout; a fresh worktree fails `compile_device_plan`).
   Tests never write into the real `data/`: stores take their path, tests use
   a `tempdir`.
9. **One client: the native egui shell.** No panel calls an HTTP handler
   directly; it calls `call::door::Door`. Exception: the offline gate app
   `tools/song-gate/`.
10. **A harness that skips is not a harness that passes.**
    `check-graph.mjs`/`check-agent.mjs` need a live server on `:7900`.
11. **Measure, don't read.** Runtimes, dependencies and protocols are chosen
    with a number in hand.
12. **Every operator control ends in a `Trigger`** aimed at a button of the
    overlays grid (finger, HTTP, OSC, MIDI, keyboard fire the same button).
13. **One graph per collection.** The canvas edits a collection graph; the
    engine runs the `device-<screen>` graphs it publishes. Never write a
    `device-*` file by hand: every writer goes through
    `metal_graph::collection::store` (see "Collections").

## Closing verification, for every batch

- Per crate, `cargo test -p <crate> -- --test-threads=1` (`--workspace` runs
  out of memory); sum the totals; `metal-dash` with `--features glow`. The count
  never goes down.
- Known reds: a batch closes if its red set is this set or smaller, never a
  new one. Current: `metal-perform::stand::…a_frame_is_a_picture_rather_than_a_flat_colour`
  (a `parallax-*` model draws flat). Environment-only: two tests need `numpy`
  in system `python3` (`author::audio::…digest_that_succeeds…`,
  `agent_tools::…attach_digest_writes…`). Re-count the older `metal-dash`/
  `metal-perform` golden-frame reds before trusting this list.
- Lint debt (not a gate): clippy error `metal-perform::digest.rs:198` (`6.28`
  literals); `cargo fmt --check` drift in `metal-sync`/`metal-web`/`metal-agent`.
- Both JS harnesses green and idempotent; kittests that click below the fold
  need a tall viewport.
- **Try a mode on real hardware before marking it.**

---

# Collections: one graph per collection

**Standing decisions** (`metal-graph/src/collection.rs`, `shell/collection.rs`)

- A **collection graph** is any graph whose id is not `device-*`. Saving it
  **publishes** `device-<screen>` per screen it feeds (node ids kept,
  `Graph::source` = collection id). Device files are generated.
- A write to a published device graph is an edit of its collection:
  `collection::store` merges it back and republishes every screen it feeds.
  All writers call `store`.
- A screen has one collection at a time. **Binding claims it**; plain edits only
  republish owned screens. Unbinding releases the screen and keeps its last graph.
- **One collection holds several screens.** Adding a screen becomes an
  `output.device` node of the central collection `col-main` (`collection::`
  `ensure_device`), never a collection of its own. Boot folds the per-screen
  collections an earlier install minted (`col-<screen>`) into `col-main`
  (`merge_device_collections`) and adopts unowned device graphs there too
  (`adopt_device_graphs`). `POST /outputs {op:"add"}` calls `ensure_device`.

**Open**

1. The header collections menu (`apply to`, `save current from`) still moves
   *layer sets* between devices. Decide whether it becomes "open this collection
   on a screen" and retire the layer copy.
2. `device_of` (`shell/collection.rs`): a node feeding two screens shows only the
   first. Show both where it matters.

---

# Next steps (in order of value; each is step by step)

## 0. Parity lote (2026-10-11): what TouchDesigner/Resolume/OBS have that we lack

One worktree per batch (`~/Projects/mvj-<batch>`, branch `feat/parity-<batch>`,
`CARGO_TARGET_DIR=~/.cache/mvj-targets/<batch>`, `CARGO_BUILD_JOBS=3`); the
machine has 15 GB RAM, so **at most two batches build at once**. Batches touching
`metal-graph/src/lib.rs` (catalog) only append; the merge resolves order.
Every new node: catalog entry + params + properties page + Spanish locale +
AI tool reachability through the same route (rule 6) + tests computed from
the catalog (rule 7). Measure before choosing (rule 11).

### P-C. Video I/O: what is left
1. **Escalated**: virtual camera on Windows/macOS (signed driver / system
   extension).
2. Spout: test the sender on Windows with `SpoutLibrary.dll` present. The
   `SLOTS` vtable in `metal-videoio/src/spout.rs` is filled (fetched from
   `SpoutLibrary.h` 2026-10-12, DLL pinned by date); recheck the slots when the
   DLL moves.
3. NDI send/receive and the v4l2loopback write are unverified on hardware: run
   the loopback test (`modprobe v4l2loopback` first,
   `cargo test -p metal-videoio -- --ignored`; only the loopback write has an
   ignored test) and NDI against a running NDI runtime.

### P-D. Art-Net + SMPTE timecode: what is left
1. **Try on real hardware**: Art-Net node (ArtDmx, ArtSync, ArtPoll on a LAN), LTC
   on a real input, MTC via `--features midi`, `in.artnet` fed by a real console,
   LTC/MTC out to a sink and read back by another device. Nothing run on hardware.

### P-F. Edge blending: what is left
1. **Try on real hardware**: two projectors on one wall, tune gamma/lift by eye.
2. The blend applies only where a device graph binds a `venue.surface` layer, and
   only from `pins` (not from a `view_from_fits` pose).

### G. GPU-centric compose: standing decisions

**Backend rule (binding).** `backend::Backend` (`Gpu` | `Cpu`) is chosen once
at startup, never per frame (`paint::Pick` is gone). `Gpu` on any adapter,
software, downlevel, GL ES and WebGL2 included; `Cpu` only when no adapter
exists or `METALVJ_BACKEND=cpu` forces it. A missing feature never falls
back to the CPU: degrade inside the GPU (no `Rgba16Float` target: stay on
`Rgba8Unorm`, which is what compose uses today; texture larger than the
adapter's limit: compose lower and upscale; low memory: lower the compose
resolution; downlevel limits where needed). The CPU code stays the `Cpu`
backend, the peers/mobile/headless/wasm fallback and the test reference.
`GET /perf` reports `backend`.

**API contract for wave 2.** Frames are `GpuFrame` (`Rgba8UnormSrgb`
texture, `Rgba8Unorm` view `frame.view()`; sample and render through the
view so no sRGB conversion happens and blending is on encoded bytes like the
CPU). A pass implements `gpu_compose::GpuPass { name, encode(cx, enc,
inputs, output) }`: record into the caller's encoder, write every pixel of
the pre-allocated `output`, never read back. `GpuCx::{alloc, upload, run,
readback}` (`Paint::gpu_cx()`); per-pass state lives in the struct. Every
pass is diffed against its CPU reference with a tolerance.

**Rule (binding).** The painter uses the most capable adapter; on an APU
(one integrated adapter, or no discrete one) it uses the iGPU. Rank adapters
explicitly, not by `power_preference`: `Discrete` > `Integrated` > `Virtual` >
`Cpu`, ties broken by the larger `max_buffer_size`/VRAM. `METALVJ_ADAPTER`
still overrides. `pick_adapter` (`crates/metal-render/src/gpu.rs`) ranks the
enumerated adapters and the window reads its present case from the real
surface, so a hybrid laptop keeps the dGPU for painting and presents the
preview through a surface-capable adapter.

Steps:

1. **Human check (this laptop)**: run with `METALVJ_ADAPTER` unset and confirm
   `GET /perf` reports the present case (`preview_path`) and the preview is not
   JPEG. Try a hybrid laptop for case (b) if one is at hand.

**Escalate if** a driver stack cannot do a cross-adapter texture copy and the
raw readback is too slow at the compose size (then lower the preview size).

### M. Materials: what is left (`feat/line-materials`)

The `mat.*` family, `material.wgsl` + `PartSet`, `room`/`rio` filled faces,
`mat.matrix` and the glitch beat lock are on master. The brief
(`docs/briefs/materials.md`, committed) holds the rest:

1. **Tier C models**: `geoterrain` has no dedicated/material shader (it renders
   through the shared `line2d` pipeline); `paisaje` and the others need a
   per-model set/clear test. Volumetric variants stay opt-in (tint the density,
   not a surface).
2. **Dataset and authoring**: material is seasoning like the post effects (glitch
   on strobe/accent hits, slow stays slow, kinds by genre) per
   `docs/briefs/colourist-dataset.md`; re-budget `parts.json`
   (`scripts/dataset_parts.py`); the router gate takes the `room` id.
3. An overlays-grid `Trigger` for the material glitch (rule 12).
4. Measure inline `material_apply` against baked albedo at 720p/1080p on a
   raymarched model before widening the contract (rule 11); heavy kinds (wood,
   concrete) need a per-model budget and a pipeline cache keyed by kind.

**Escalate if** the row-tail change breaks `camera_track_offset` or a saved row
cannot be migrated.

## 1. Finish the dataset, train, and fold the `ai.*` nodes into the source

**Goal.** The AI is a property of the source (`modo`), not a canvas node. Training
is a button on the source and in config. `ai.router` and `ai.curator` are never
free nodes; their choice lives in the `router`/`reel.*` source properties.

**Binding.** Song list is `data/audio/` (74 songs; canonical list
`data/songs/catalogue.json`, from `metal-dash --bin catalogue`, keyed by `stem`).
The router gate (`data/router/gate.json`) is a hard mask; an unknown song uses the
union of its predicted genre's rows. Colour slots come only from
`data/palettes.json`. The Colourist learns from **parts** (intro, build→drop,
drop, break, bridge, harmonic change, outro, with margins); design in
`data/dataset/colourist/README.md`, process in `docs/briefs/colourist-dataset.md`.

**Steps.**

1. **Retire `ai.colourist` as a node** (`fx-layout-colour-plan.md` §4.3): move
   `weights` onto the source's `modo`; delete the catalog entry (tombstone, rule 1),
   `offered_ai_for`, `ai.embed` and helpers, `Node.embedded_in`, the chip primitive
   and the reaction toggle; `metal-graph/src/migrate.rs` turns an embedded/free
   `ai.colourist` into `modo: "ai"` + weights. `embed_ai` tool goes; `train_ai`
   stays and gets its `AgentHarness` door arm. Test:
   `an_embedded_colourist_migrates_to_source_modo`.
2. **`modo` on the canvas**: in-canvas node-body dropdown and compact float fold
   (`fx-layout-colour-plan.md` §4.1–2, `RectTag::Dropdown`, wire `Node.expanded`).
   Gates 2 and 3 apply.
3. **Author the remaining parts** in waves of two subagents (one part each,
   disjoint files), covering every model and keyword before repeating; Alonso
   approves via `status.json`. Bulk is delegated to another AI; the 171 mother
   examples are the reference.
4. **Train** each model once it has ≥ 3 songs of approved parts
   (`POST /author/grade/train`, from the source's `entrenar` and from config);
   newest weights are the default.
5. **Router choice as a source property**: `modo: "ai"` on `router` runs
   `Brain::decide` with the gate as hard mask; genre classifier over the 12 router
   features (logistic regression like `learn.rs`, leave-one-out accuracy reported);
   seed per song from `segment`. `Mode::Model` becomes a tombstone. Tests:
   `a_disallowed_model_is_never_picked`,
   `an_unknown_song_uses_its_predicted_genre_union`,
   `a_section_boundary_is_the_only_place_the_pick_changes`.
6. **Curator choice as a source property** on `reel.clip`/`reel.batch` and
   `fx.image-pop` (`select` stays internal); unwired behaviour stays round-robin.
   The crop becomes image-pop's `crop_zoom` origin.
7. **Close**: `data/graphs/demo-ai.json` with `audio.live` and three sources in
   `modo: ai`; play five songs of different genres on the projector.

**Open decisions for Alonso.** Cameras are not learnable (`drive::row` never moves
one): author cameras in parts or not. The editor preview runs on wall time,
`example-proof` on song time: pass the playhead as `t` or not.

**Model bugs found while authoring** (fix when a part needs it): aurora's
`noise2` seams (`shaders/shared.wgsl:137`); `aurora.velocidad` and `puzzle.step`
multiply song time by speed (late changes surge or reverse); puzzle structural
floats re-deal the board; dark colours render lifted. Repo bug:
`scripts/gen_loop_templates.py` deletes all templates then fails on `jaw_open`.

**Escalate if** the gate leaves a model with < 3 songs (round-robin stays, list back
to Alonso), genre leave-one-out accuracy < 60 %, or a model cannot express its part
inside its learnable band.

## 2. Label voice

**Goal.** `data/voice-labels/` covers the songs `monstruo` is gated for (wherever
`oracle` is allowed, ~20 songs), so its parts can be authored and the voice block
of the 29-col digest is validated on real songs. Eight labels exist today.

1. Freeze the label schema from the 8 TOMLs (fields, units, 32 Hz time base); write
   it in `voice-plan.md`'s decisions, fold that file's remaining items here, delete it.
2. List the gated songs missing a label; label them (sung/spoken spans,
   vowel/phoneme classes the face solver uses, lead vs backing).
3. Score `metal_listen::voice` against the labels per song (onset F1, voiced
   accuracy); record the table in `docs/DOCS.md`.
4. Author `monstruo` parts from step 1.3 of "Next steps".
5. Voice wiring left: live listener reads `tick_level` into the engine;
   `overlay::Play.ticks` decay; `base-only` badge pixels; runtime look-ahead row for
   the face solver; `monstruo` eye tuning and ≥ 60 fps check; live-mic and Pi checks.

**Escalate if** the analyser scores under the labels badly enough that a better
model (rule 11) is needed.

## 3. Shake the tree

1. **Dead-code pass**: find and remove tombstone-only modules in crates still
   reached by `metal-dash`, in small commits with tests green after each (rule 7
   counts come from the manifest). Tombstones required by rule 1 stay.
2. **README with real material**: embed screenshots captured headless from
   kittest (nodes, properties, lanes, venue editor, phone shell) and frames from
   `metal-render --bin model-demo` / the palette proof sheet
   (`docs/palettes/proof.png`); a short loop GIF of a song reacting. No mockups.

**Escalate if** a crate looks dead but a test or the agent catalog reaches it.

## 4. MCP: close the gap (step by step)

Open: `look_venue`/`project_venue`/`calibrate_projector` never run on a real
projector/camera. Three of the four camera-solve bodies (`scan_venue`,
`discover_fixture`, `calibrate_projector`) run on a synthetic feed; `solve_head`
posts analytic inverse kinematics with no feed. The smoke set
(`crates/metal-mcp/tests/door_tools.rs`,
`every_hardware_free_tool_answers_a_smoke_call`) skips the three route-gap tools
below; the executor split is guarded by
`every_exported_schema_is_valid_and_matches_tool_schemas`
(`crates/metal-mcp/src/catalog.rs`).

1. **Hardware run (last action, needs a person)**: real projector + camera;
   run `scan_venue` → `calibrate_projector` → `look_venue`/`project_venue`,
   record the result in `docs/MCP.md`.
2. **Route gaps** the smoke test skips: `read_script` (the adapter forwards
   args as query; the route reads the node from the path tail), `set_fixture`
   (no `op:"fixture"`), `dmx_bus` (no `tool_route` arm in the real table; the
   test-only copy in `app.rs` has one, which masks the gap). Give each its
   route; they leave the skip list.

**Escalate if** the seam cannot be tested without hardware.

## 5. Frames between machines (step by step)

**Decided**: `Output.node` (who shows), `Output.composed_on` (who composes),
`Layer.render_on` (who renders). A remote layer's frame is one GET per frame
over `fw/frame/1`; `remote_layers::choose_fmt` sends raw RGBA on a direct LAN and
QOI on relay (an unknown path defaults to QOI). The compose lags at most one
frame behind a peer — it waits `PRIME_WAIT` (50 ms) only for a peer's first frame.

1. **QOI encode budget**: strips encode in independent bands (each starts with
   an `OP_RGBA` literal, own index/prev) on the serving peer's request thread.
   The 720p idle bench has only run on a loaded box; re-measure on a quiet one
   and confirm < 5 ms (release, `qoi::tests::bench_720p`).
2. **Hardware runs (last actions, need machines)**: the Pi LAN run, logging
   with `scripts/frames-perf-log.sh` (release build on the Pi, one remote
   layer at 720p; a persistent stream only if it cannot hold 30 fps); then
   `via: relay` end to end on two real machines (phone on mobile data +
   desktop), checking `remote_layers::choose_fmt` (raw vs QOI) and the frame
   rate. Land the numbers in `docs/DOCS.md` and `docs/MULTIPLATFORM.md`.

**Escalate if** the route isn't plain plumbing or the tick must be synced
between machines.

## 6. `wasm32` build

The browser runs the desktop shell over the engine, on WebGL2 or WebGPU, with
bundled demo data (`scripts/build-wasm.sh`; `docs/PRODUCT.md`,
`docs/MULTIPLATFORM.md`). Decided: the web demo uses a fixed scene and no AI;
no API keys reach the browser. Open:

1. Previews run inline on the page's one thread. Move the painter to a Web
   Worker (`wasm-bindgen` + `OffscreenCanvas`); inline fallback when workers
   are unavailable. The desktop worker (`shell/frames.rs`) cannot run there.
2. Compute models on WebGL2 (dla, solvent, turing, terrario, particulas) are stand-ins.
   Port each to fragment-shader ping-pong, one per commit, parity-checked
   against the desktop frame; drop each from `needs_compute` as it lands.
3. Author side (`/author/*`, rig, Curator, Router authoring) is a no-GPU stub.
   Browser gets manual routes only (lanes, properties, scenes) on `metal-vfs`;
   AI routes answer "desktop only".
4. Publish the demo on alonsoleon.com, only with Alonso's go, after 1 and 3.

## 7. Android / iOS packages

`scripts/build-apk.sh` produces the arm64-v8a build. Open:

1. Relay fallback (`via: relay`) not exercised: iroh relay unreachable from
   the sandbox. Pairing works over the direct path.
2. Install and run on a real phone; no device tested yet, no release keystore
   (keep it out of the repo), then `build-apk.sh --release` signing.
3. Android has no ffmpeg: video HAP encode and desktop-only process spawns are
   absent. Decode H.264 natively (AMediaCodec); pack or pull `songs/`,
   `models/` and device graphs on demand (song pull exists; extend to graphs
   and models). Every `platform::Command` without an Android path answers
   "unavailable on this platform (Android has no child processes)" and is listed
   in docs.
4. iOS (`aarch64-apple-ios`, `cargo-xcode`) needs a Mac.
5. Other targets: `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`,
   `aarch64-apple-darwin`; `--low-power` GLES backend; local `memfd`.

## 8. Closing the gaps of this lote (step by step)

One worktree + branch + target dir per item (rules above); each closes with
the closing verification.

1. **Android on a real phone** (§7.2). Install the arm64-v8a APK over adb;
   repeat the emulator checklist (render, video clip, audio + playhead drift,
   in-process `metal-sync`, pair + song pull) on device GPU; record frame time
   vs. SwiftShader. Then release keystore and signing.
2. **Android data and spawns** (§7.3). As §7.3 above.
3. **Frames on a Pi** (§5.1–2). Release build on the Pi, one remote layer at
   720p; log fetch+decode ms and dropped frames from `GET /perf`. Persistent
   stream only if it can't hold 30 fps. If QOI encode idle is >5 ms, encode on
   the worker in parallel strips.
4. **Relay path** (§5.2, §7.1). Two machines on different networks, force
   `via: relay`; check raw-vs-QOI choice and frame rate; document the numbers.
5. **wasm previews off the main thread**: see §6.1.
6. **Compute models on WebGL2**: see §6.2.
7. **Browser author side**: see §6.3.
8. **Publish the demo**: see §6.4.
9. **MCP camera solves**: see §4 (rewritten by the lead).
10. **Flaky tests.** metal-perform JS script tests time out under load: longer
    budget or serialize. metal-sync two-node tests get the same treatment.

## 9. Show control: standing decisions

Evidence: `~/Projects/analysis/REPORT.md`. A later change to these areas must
honour them.

### 9.1 Export

Adopt filmcraft's shape: an `Encoder` abstraction (packets out) plus one
muxer; a stepped, resumable export job (`Step::{Progress,Pending,Done}`)
overlapping render and encode; deterministic output (fixed write groups,
pinned encoder flags as in `metal-ingest::video::encode_rgba`); presets and
queue in `data/export-presets.json`. Backend stays ffmpeg through
`metal-ingest`. Do not reimplement a codec now.

**Escalate if** export needs a second timeline/project state, or a codec
cannot sit behind the `Encoder` trait.

### 9.2 DMX and fixtures

Copy grandMA3's per-universe slot map with priority + 3 s sACN timeout,
ShowXpress' per-input HTP/Input/Software, and ShowXpress' pixel matrix (grid
of channel offsets, `-1` unused, base = min offset − first-channels count,
with start corner/orientation/snake). Extend `Fixture`/`FixtureState` and
`fixture.rs` profiles; never replace them.

**Escalate if** a fixture needs a new `Mask` field, or D3 needs a second state
beside the graph.

### 9.5 Math nodes (`source.*`)

`source.*` is a value family at `Stage::Source` driving any `fx.*` control or
`param:<name>`. Pure CPU, no manifest model, no GPU pass (precedent `gen.ramp`,
`ctl.math`). Floats appended at the end (rule 2). Same AI route (rule 6);
tests computed from the catalog (rule 7).

## 10. Text block and UI scale (`leap/*`)

Each is its own worktree in `~/Projects/leap-<name>`, branch `leap/<name>`, own `CARGO_TARGET_DIR`.

3. **`leap/text-block-core`** — the text block's finish. `background`/`border`/
   `stroke` already render (`text::plan`/`render` and `text_gpu::{draw_into,
   SHADER}`, byte parity under `gpu_text_matches_the_cpu_reference`; `width 0`
   byte-identical to none). Add `shadow` to `TextConfig` (offset, blur, colour —
   one per node) and render it behind the glyphs on both paths. Files:
   `crates/metal-perform/src/text.rs`, `crates/metal-perform/src/text_gpu.rs`.

6. **`leap/ui-scale-core`** — the new setting. `ui_scale` (a multiplier,
   default 1.0, clamped) in `call::settings`' `KEYS`/`defaults`/`post_at`,
   validated and round-trip tested; read by `settings::PanelState::refresh` and
   applied every frame as `ctx.set_zoom_factor(scale)` beside `window.rs`'s
   `theme().apply`, so every UI component scales (fonts and spacing follow
   through egui's pixels-per-point) and survives in `data/settings.json`.
   Files: `crates/metal-dash/src/call/settings.rs`,
   `crates/metal-dash/src/shell/panels/settings.rs` (state + `refresh`),
   `crates/metal-dash/src/shell/window.rs`, `crates/metal-dash/src/shell/mod.rs`.

8. **`leap/text-block-ui`** — the panel half of the text block: shadow, stroke
   and background controls in `properties/text.rs` through the same
   `POST /graph/node {op:"config"}` merge, Spanish in
   `locales/es/properties.toml`, and the catalog/lint/tests that name the new
   config facts. Files:
   `crates/metal-dash/src/shell/panels/properties/text.rs`,
   `crates/metal-dash/locales/es/properties.toml`, `crates/metal-graph/src/lib.rs`.
9. **`leap/ui-scale-ui`** — the settings control: a slider/dropdown in
   `settings.rs`'s `UI` section, replacing the `UI_PLACEHOLDERS` "interface
   scale" line, posting `POST /settings {ui_scale}` and adopting the reply,
   Spanish in `locales/es/settings.toml`; kittest posts it and reads it back.
   Files: `crates/metal-dash/src/shell/panels/settings.rs`,
   `crates/metal-dash/locales/es/settings.toml`.

**Escalate if** the text block needs a second state beside the graph (rule 4).

## 11. Wave 2: models and fx (`feat/models-fx-wave2`)

Spec: `docs/briefs/models-fx-wave2.md`; effect recipe `docs/briefs/fx-recipe.md`;
scaffold `shared/paper.rs`. The `papel-*` upstream licence is unverified.
`data/router/gate.json` has no `papel-*` ids yet (`scripts/gate_add_models.py`).

- Tune `objeto` and every `papel-*` default by eye; nobody has looked at a frame.
- Perf at 1280x720 (readback included): `papel-simplexnoise` ~20 ms,
  `papel-dotorbit` ~18 ms, `papel-perlinnoise` ~13 ms, `papel-metaballs`
  ~13 ms, `fineline` dense ~70 ms. Profile and trim.
- Router `model.json` is untrained: retrain, re-measure leave-one-out after
  the gate grows.
- Colourist dataset: re-budget `parts.json` with `scripts/dataset_parts.py`
  (needs local `data/audio`), +52 parts (171 -> 223), then author.
- hardstutter runs on `time` at a `tempo` float; wire the song's beat clock.
  The window path (`app.rs`) ignores brush and hardstutter.
- Wave 3 fx (effectcraft): ripple, twirl/spherize, polar, mosaic, tile, echo,
  radial blur, turbulent displace, venetian, scatter, halftone, find edges.

## Shape

- **egui + wgpu**, `egui_glow` fallback; one client (rule 9). Gates: dependency
  direction, no visual literals outside `classes.rs` (gate 2,
  `literal-allowlist.txt`), restyle drill (gate 3), backend confinement.
- **One object per panel**: Nodes · Properties · Lanes · Library · Chat (right
  sidebar). The phone shell is the same shell with fewer panels.
- All strings through `crate::shell::i18n::tr`, Spanish in `locales/es/`.

## AI chat

Remaining: a manual pass with a real key per preset (Claude, OpenAI,
DeepInfra): streaming text, one auto tool call, one destructive `ask`→allow,
one deny, and a forced 429 showing retry. Known risk: SSE over the hand-rolled
client (`http::post_stream`) is the sharpest edge; it falls back to a single
JSON request.

## Editors (borrowing from ArtCraft/EffectCraft)

Port, don't vendor: adapt to `classes.rs` tokens and `Door`; keep upstream
attribution (`storytold/artcraft`, `storytold/effectcraft`, egui 0.36,
`MIT OR Apache-2.0`).

1. **Widget base**: `components.rs` ← vectorcraft `widgets.rs` (property rows,
   number fields), designcraft `NumKind`, effectcraft `panel_kit.rs`. One kit.
2. **Viewer**: effectcraft `viewer.rs` (pan/zoom, `to_screen`/`to_comp`) +
   `viewer_tools.rs` (snapping, rulers, guides) as the shared frame view for the
   venue editor, mask editor and preview. Cadcraft `snap.rs` for the top view;
   vectorcraft `perspective.rs` for 3D; vectorcraft `bbox.rs` for grips in
   `properties/surface.rs`; gridcraft `grid.rs` for `properties/patch.rs`.
   `ContourEditor` stays the only polygon editor.
3. **Layer editor**: stack per page/output (order, blend, opacity, mask) on
   effectcraft's `timeline.rs` row layout and `keyframe` `Interp`/`Ease`;
   posts `edit_layer`'s route.
4. **Curve editor** for float keys: effectcraft `graph.rs` (bezier handles,
   fit) plus vertical drag-to-edit.
5. **Script code editor** (py/js/rs) in script-node properties: read-only
   first, then editable through the script route; shows presets
   (`script_preset.rs`).
6. **3D camera/lights**: effectcraft `dialogs_3d.rs` for the venue camera and
   head-tracking capture camera.
7. Kittests per editor; restyle drill through both themes.

**Escalate if** a borrowed widget needs a second state beside the graph, or
two polygon editors appear.

## Finish the translation (code to English; UI stays Spanish via locales)

1. One batch per crate, disjoint files, in order: metal-dash (shell, call),
   metal-agent (incl. shots), metal-perform, metal-graph (catalog labels),
   metal-schema, metal-listen, metal-ingest. Identifiers, comments and log
   strings only; user text goes through `tr` + `locales/es/`.
2. Persisted Spanish names (≈368 float/slot names, metal-router/metal-reel wire
   names, `PaletteDef` `acento/fondo/destello`): rename with serde aliases and
   a versioned migration; model ids stay. Proposals: `docs/briefs/rename-proposals.py`.
   Rules 1–2 hold (positions unchanged).
3. Presets keyed by model label → keyed by id.
4. `locales/es/dynamic.toml` entries for new English model labels.
5. Test that fails on a non-ASCII string literal outside `locales/`.
6. Do step 2 together with the MetalVJ rename (§ "Next steps" step 3) so
   names change once.

## Review `data/agent/system_prompt.md`

Still Spanish with stale material (`escena`/`pared`).

1. Translate to English; replies stay in the user's language.
2. Remove stale material: `escena`/`pared`, references to old `plan.md`
   sections, the bar shot that passes `rgb` (now the colour outside the masks).
3. Add what is missing: per-mask `masks`, `venue.surface` (mask → surface →
   device), pixel map and `venue.fixtures`, head tracking, source `modo`,
   script presets.
4. Each tool named must exist in `TOOL_CATALOG`; a test parses the prompt and
   fails on unknown names (rule 6).
5. Re-run `check-agent.mjs` and the shots.

## Shell leftovers

- Scene rows (library `scenes`, scenes gallery) still show a text label while
  dragged; use `components::drag_ghost` with the row's own painter, as the
  library's file rows do.
- A gallery card cannot be dropped on the sequence timeline; `importar` adds
  at the playhead.
- The studio grid (`/author/render/*`) shows defaults, not the node's state:
  request the tile with the node's row once the per-model form exists.
- Per-model form: floats/colour/camera panel per model (dynamic per-schema
  form); make/assets (`/author/palettes`, trim).
- Hover posters in the models dropdown: show `demos/<id>.jpg` (from
  `metal-render --bin model-demo`) with name and look line; fall back to the
  live tile; one cached JPEG on hover. Kittest.
- Settings: borders per surface type and the persisted theme dropdown.
- Two styles (`Theme::phosphor`/`daylight`): restyle drill through both with
  screenshots for Alonso.
- `@` in the harness prompt: Library files, nodes, overlays, `node@property`
  on an independent ranking (fuzzy + recency + focus); Tab completes.
- Screen handover: verify on hardware (Virtual DJ selector / ShowXpress
  fallback, back without restart) when Alonso names the window.

---

# The room: masks, venue and pixel map

## Standing decisions

- **Three jobs, three places.** `mask` decides **what** (source pixels, flat);
  `venue.surface` decides **where** (warps onto one surface); the **venue**
  (`metal-perform::venue::Venue`) is the room: surfaces, objects, projectors
  (per-surface `pins`), displays, fixtures, sensors. Chain:
  `gen → fx… → mask → venue.surface → output.device`. A mask after a surface,
  or two surfaces in series, is a lint error. The device is last **and offers
  the wire after it**: `output.device`'s `venue` output carries this hardware
  output's finished frame to a `venue.surface`'s `image` (never to a
  fixture), so the room's grid is a *view* of the frame, not a second
  composition — the frame is pixel-identical with and without the cable, and
  the reverse over the ordinary chain is a cycle the lint names.
- **The output's grid is the surface node's `sectors`, and one surface node
  fans in many sources.** A `venue.surface` node is one output's whole grid,
  not one generator: its `image` input fans in (the `compose.*`/`router`
  convention), and each upstream generator is dealt the next cell of the
  surface's `SectorGrid` — *projected* (a fan centred on the surface and
  mapped through the projector, `ratio × 3` wedges) or *drawn* (consecutive
  `[start, end]` streaks in the output's 0..1, the mask-stroke vocabulary,
  with the same stroke width/colour/alpha and an eighth-metre magnet).
  The cell sets a layer's clip, place and render size; the warp stays the
  ordinary homography/mesh. A surface with no `sectors` is one cell, the whole
  plane, so every older venue renders and saves unchanged. Written by
  `POST /venue {op:"surface.sectors"}` and `set_venue_sectors`. **Escalate if**
  a batch needs a per-cell field beyond the polygon.
- **`Mask` is the only mask representation**: nesting (`parent`/`alpha`) and
  booleans (`union`/`intersect`/`subtract`) in `Output.masks`, CPU-evaluated;
  deepest wins, ties to max. Written by `POST /outputs { op: "mask" }` (and
  `/mask/grid`) and the `edit_mask` tool. A device whose graph has a `mask`
  node skips the whole-frame mask pass; same for `venue.surface`. **Escalate
  if** a batch needs a new `Mask` field.
- **Stroke**: `stroke { width, color, source }`, band = cover(poly) −
  cover(eroded); `width: 0` is byte-identical to none. **Material ports**: one
  row in `MATERIAL_PORTS` (`metal-graph`) + `Port::material` + a sampler + an
  editor route. A batch editing `compile` for another material special case
  stops.
- **Composition happens in `resolve`** (`SegmentMix { mask, blend, opacity }`
  via `POST /author/models/timeline/segment`). Device mode is config on
  `output.device` (`automix | collection | own | disabled`); a legacy
  `stack.mode`/`compose.*` node is still read as fallback. `GET
  /catalog?domain=live|offline` filters the menu.
- **The venue is the pixel map**: every emitter/sensor with a pose and a
  footprint (projector → surfaces/object faces; display → glass; wash/
  continuous/panel → sample point/area; LED bar → one colour; LED string → N
  pixels on a polyline; moving head → sample + aim; camera → sensor).
  `venue.fixtures` (`Stage::Output`) samples the frame at each footprint and
  sends per wire. Built by hand in the venue editor or by the AI from cameras
  through the same routes; the AI drafts, the operator accepts.
- **Projection mapping by the AI**: `locate` picks grid cells, never estimates
  coordinates (~3–4 % vs ~16–24 % error).
- **Out of scope**: non-planar surfaces. **Edge blending** is config on the
  projector (`blend`), never a `Mask` field.
- **One room per node, never a dropdown.** A `venue.fixtures` node owns its
  room in `config.venue`; every node driving that room reads it through
  `venue_map::venue_for` (the room its cables already reach wins over a fresh
  one, so pre-existing scenes stay attached). The room is edited **on the
  nodes canvas itself**: the gear on a `venue.surface` node (or the Map card,
  or the `output.device` page's *abrir la escena*) flips the canvas into a 3D
  scene of the whole room (`panels::venue_canvas`), its bar names the room and
  carries `+` and `nodos`. The popup (`panels::venue_popup`) is only the
  gallery the `+` opens — screens, projections and fixtures, one fixed size
  per frame — never the room. No canvas palette, no global venue picker. A
  screen and a projection are one `Display`; the projection's lens stays apart
  from its glass in the 3D view. The Map folder carries the input → output
  table (`entradas`/`salidas`): which generator reaches which surface/fixture
  and which hardware output carries it, sectors included.

## Pending

1. **M5 computer vision** (tools and draft flow exist; CV is `TODO(M5)` in
   `metal-agent`): Gray-code structured light → projector pins per surface;
   fixture discovery by flashing (LED strings pixel-by-pixel or binary-coded);
   moving-head pose by pan/tilt sweep. **Escalate if** the camera can't see a
   beam in the room's light or a second camera is needed.
2. `TODO(P2.4)` multi-view refinement in `Venue::view_from_fits`
   (`metal-perform/src/venue.rs`); `TODO(P3.3)` one shared canvas render across
   devices (`metal-perform/src/engine.rs`).
3. Head tracking H0: ONNX/MediaPipe and Pi rows of the vision bench
   (`scripts/setup-vision`; target ≥ 30 fps, ≤ 50 ms). Nothing built yet.
4. **Silhouette as mask and as exclusion** (RobustVideoMatting ONNX, after the
   detector is measured); **the body feeding the mask** (head/body position →
   `Venue::spotlight` → contour, behaviour in a script node).
5. Venue generation by a local model: run 5×flyer-1 and 5×flyer-2 with
   `cargo run -p metal-dash --example venue_gen -- --backend local`
   (`~/.local/share/fw-llm/start.sh qwen25vl-3b`, `--no-mmproj-offload`); fix the
   facade example's aspect first. The local
   model takes over when it parses ≥ 95 %, zero errors ≥ 80 %, median
   reprojection ≤ 0.04, acceptable time. Self-consistency is not truth:
   tape-measure one real room.
6. **Hardware check, one session**: box + grid on a projector (one surface per
   face); two projectors on one wall; webcam head-tracking lag with a 240 fps
   phone; the room rig (projector, TV, Godox 60 W, wash, moving head, LED bar,
   LED string) mapped by hand and by camera, head aimed at three points;
   frame-driven fixtures to `out.sacn`/`out.serial`; DMX pan/tilt curves (~10
   heads at the hotel); MIDI/OSC latency (`midir` on the Pi).

---

# Audio and trained reaction

## Tone block

Still deferred: (ii) Basic Pitch golden test from exported posteriors;
(iii) live end-to-end latency measurement (the offline number is not the live
number; live estimate is ~0.65 CPU-s per audio-second, extrapolated); (iv)
library re-digest and the reggaeton/EDM-drop check; (v) the offline path
re-runs the model on every 32 Hz row; reusing the live hop's 8-frame cadence
would cut that cost ~3x.

## Audio input

- Per-node consumers still read the graph's *first* audio node
  (`Digests::graph_raw`, `metal-perform/src/listeners.rs`); making them follow
  their cable changes `Paint::run`'s signature (escalated). The clock (router,
  `Looper`, music clock, `reel.batch` beats) follows the seed listener.
- **Stems** (not started): `audio.live` gains `control`, `drums`, `bass`,
  `other`, `vocals`; config `stems: bool` (off → silent ports + lint).
  Separation in `metal-listen` before the `Digester` (five digesters per
  stemmed source); `trait Separator` with `ort` CPU and a `Passthrough`;
  deadline thread, a missed tick uses the mix. Model: stemgen-rt's causal
  HS-TasNet ONNX (MIT, 5.8 ms, 4.5 dB SDR) at `data/models/stems/hs-tasnet.onnx`,
  outside git. **Measure CPU on laptop and Pi first**; if the Pi can't, stems
  become a capability of the composing machine. **Escalate if** `ort` doesn't
  build for `aarch64-unknown-linux-gnu`.

## Loops

Two kinds, never mixed. **Manual loops** are authored by the human, triggered by
the grid and attached to the gen node's own information (the loop library and
`/loops/choose`); they are set beforehand. **The loop AI** is implicit
inference, not a node: it is required for every inference when a node's mode is
`ai` or `ai_fx`, and re-uses the gen AI's float offsets so the animation is
stronger and less stochastic. Loop length is in beats, `{1/8 … 8}`; `0` means
the music changes. `LoopFinder` (`metal-reel/src/loopfind.rs`, trained as
`ai.loops` from the approved loops dataset, one song per genre, per-tick
`loop_beats`) finds it. `Supervisor` idles the gen AI 50% of the time while
looping and authors when the length is `0`; `Conductor` has the router switch
every 4th authoring entry. Open: threshold and hold values are unmeasured;
replay is per (node, model); dither is unverified on GPU; rules 9 and 12 for the
`fill_range` button.

## Value sources

- **Palettes as graph state**: the `palette` kind (`POST /palettes`, slot→param
  binding) exists. Left: the palette dropdown (node properties + personalization
  overlay); shaders read slots, palettes hold values; colours exposed as appended
  floats `c_0…c_n`. Split before writing.

---

# The performance model

- **A sequence is the show for one song**: one Library file, one grid button.
  A **movement** is a reusable keyframe run; only Alonso sets `favourite`.
  **Every final output has its own page** (device, mask of it, and next:
  fixture). **Nodes first, then sequences**: the graph is the always-on
  fallback; sequences sit on top, per page; never the reverse.
- **The overlays grid is the only control surface**: sequences, node switches
  (`toggle`/`hold` on `Node.bypass`), blackout, identify, stop-all. HTTP/OSC/
  MIDI/keyboard/learn are `TriggerSource`s.
- **Pending decision (Alonso)**: the sequence editor saves one project per song
  (`data/projects/<song>.json`); `Movement` (`data/library/movements.json`) and
  `metal_perform::sequence::Sequence` (`data/sequences.json`) still exist beside
  it. After the answer: the grid fires a song's project; the Library lists
  projects as sequences; fold or retire the loser; `page_tab` persisted per
  device; pages importable from a node collection. **Escalate if** a page
  needs to write the graph.
- Unwired keybind rows in `shell/keybinds.rs`.
- Authoring leftovers: the curator lane (own session; read-only → write →
  editing); the whole chain of a track's models and effects in one properties
  panel; timeline keyboard shortcuts.
- Unverified: a global strobe button over all outputs; "two moods and a button
  to switch".

---

# Other pending work

- **Remaining `out.*`**: `out.ddp`, `out.json`, `out.osc`, `out.http` are
  catalog entries only; each needs one `impl Wire` (`metal-perform/src/wire.rs`,
  modelled on `Sacn`) with its packet test, never a stub. A solid-colour
  overlay → the timeline (inverse of rasterizing). **Escalate if** an `out.*`
  needs more than one row per frame.
- **Text**: stroke/shadow/background per block, per-pair kerning, bidi/wrap,
  a keyframe for every parameter; measure `glyphon`.
- **Video reel**: stills must be JPEG today; a plain video loops its whole
  length instead of the interval; reel source ignores a dropped scene's
  `scene`/`start`/`end`; scene posters not synced, no favourite toggle in the
  Scenes route; no render test for `reel.batch` drawing.
- **Video ingest, next** — the pipeline itself is in (ycgco `zscale` prefix on
  every decode path; HAP Alpha on every path, the node, layer and library all
  encoding straight from the source so a DXV alpha cut-out keeps its
  transparency; `reel.clip`, `reel.batch` and `POST /author/ingest` each
  trigger a build; ffmpeg's own frame progress drives the job; `ingest_proxy`
  in `settings.json` switches the automatic build off):
  **one indexer**. `index::build` still cuts the library at 12 fps/640 px with
  `Passage`s in `index.json`, beside the shared node/editor/renderer proxy at
  8 fps/480 px with `metal_reel::scenes` cuts — two proxies, two cut lists for
  the same clip. Collapse them onto one spec (one rate and width, one slug,
  one cut list). One spec must win and existing libraries re-proxy, so it is
  its own batch.
- **Land the ingest batch**: the launcher builds only `origin/master`, so
  `feat/ingest-normalize` reaches a booth once it is merged and pushed.
- **Example DMX collection** (not a batch yet): script nodes that take a
  model's floats as origin and transform pan/tilt (invert, reduce range,
  duplicate, split into 2/4 lanes), plus a collection with 4 output subgroups
  driven by one float group. Needs the pan/tilt columns.
- **Peer monitor**: `cpu_pct`/`gpu_pct`/`temp_c`/`uptime_s`/`paint_backend`/
  `fps_paint` on a 5 s tick in `metal-sync`, stale after 20 s (never zeros); a
  read-only panel.
- **Render farm** (after pairing): `fw/farm/1` control, conductor in `[farm]`,
  synced `data/render/queue.json`, shards with 2 s heartbeat, delivery to
  `data/reels/<reel>/verified/`.
- **Harness**: harness-on-monitor (follower for pixels, peer for state; pass
  the sync author to `land.rs` for the owner guard); shared agent session
  `data/agent/session.jsonl` with roles (super/producer), stale-seq refusal,
  run-once. Goes last.
- **Song detection from DJ software** (outside the MVP): VirtualDJ `History/`
  watcher first, Pro DJ Link later; `TriggerSource::Song`, a normalized
  `DeckSignal`, an external router `Mode` driven by the deck.
- **MetalVJCloud (paid, optional)**: private repo `alonso-leow/MetalVJCloud`
  (Bun + Postgres, Tilopay; API in its `docs/`). The app stays free and
  provider-agnostic: every cloud feature has a bring-your-own path (own AI
  keys, own Google Drive). In order:
  1. **Chat**: `MetalVJCloud` provider preset on the `OpenAiCompat` adapter at
     `/v1/chat/completions` with an `mvj_` key; model list shows the cost
     multiplier (x1 = DeepSeek V4.1 Flash).
  2. **Funnel popup** when no provider is configured: "Configurar manualmente"
     (settings `IA` page) vs "Ayuda" (landing → price calculator → checkout).
  3. **Usage**: settings page reads `GET /v1/account/usage` (per service: used,
     remaining %, reset date); chrome hint at 30 % remaining.
  4. **Drive**: Library mounts a remote bucket over the GCS JSON API
     (`/storage/v1`), same path for MetalVJCloud and Google Cloud Storage;
     Google Drive as the free alternative.
  5. **Frame**: `out.cloud` publishing `fw/frame/1` frames (u16 status + image)
     or scene-state JSON to `wss://…/v1/frame/<slug>/publish`; shares step 5's
     frame route.
  6. **Forms**: `script.cloud_form` preset node (worker thread, backfill then
     SSE, floats `connected`/`count`/`pulse` + one per exposed field); key in
     the user config store, never in the graph file. Proposal in the cloud
     repo's `docs/preset-script.md`.

---

Pre-rewrite plans for history: `git show 35a9f6f8:plan.md`,
`git show 0bc59052^:plan-opus.md`.
