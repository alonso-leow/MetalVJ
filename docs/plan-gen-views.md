# Gen views: one generator, many perspectives (2026-11-14)

Base: pub/main a47fa24. Integration branch `feat/gen-views` (~/Projects/mvj-views).

## Goal

A single `gen.*` node renders **K views per frame**, one per projection. Every view
starts from the node's single **base row**: the model defaults, then the AI weights
(Auto mode offsets, `ai_offsets`), then the held floats that controller pads, knobs
and scenes write (`params::set_held`), then the `param:` bindings and the scripts.
Each view then changes that row in two ways:

1. **Camera (perspective).** The view's 7-float `Camera` block (`[eye.xyz, target.xyz, fov]`)
   comes from a venue projector or surface. Its `cam_x/y/z` track floats can add off-axis
   parallax. The 3D models already read both from the row (`tracked_view_proj`,
   `metal-render/src/model.rs:129`), so **no model code changes**.
2. **Variations.** Per-view edits to one or several params.
   - `vary`: a linear spread across the views.
   - `add` and `set`: explicit per-view edits.

   One controllable `view_amount` (0..1) scales every variation, so one knob opens or
   closes them all.

Because controllers and AI already land in the base row, moving a pad moves every view
together. The views differ only by camera and by variation.

## Research (file:line on pub/main)

- **Row assembly.** `metal-perform/src/engine.rs` `row_for_node` (~4140) builds defaults →
  drive / `ai_offsets` (4463) → loop. Then `bind::before_scripts` (bind.rs:426) applies held
  floats and then bindings. Then `script::evaluate_clocked`, then `bind::after_scripts`
  (bind.rs:434). Rows are a `HashMap<NodeId, Vec<f32>>` (engine.rs:2967).
- **One node, one render.** `Paint::run` (paint.rs:910) makes one buffer per plan op. No
  per-node multi-render exists.
- **Camera.** `metal_schema::Block::Camera([f32;7])`. A held camera lives in config `camera`
  (`params::GEN_CAMERA_KEY`). Track floats `cam_x/y/z` sit just before the signal and effects
  tail (`camera_track_offset`). The off-axis frustum is `off_axis_with`.
- **Venue.** `metal-perform/src/venue.rs`:
  - `Projector` (691) has `lens`, `yaw`, `pitch`, `throw_ratio`, `aspect` and `on`.
  - World position comes from `on` (see `viewport.rs:335 projector_at`).
  - `Surface` (489) has `origin`, `u` and `v`.
  - `viewer_from` (2846) gives the viewer point.
- **Fan-out.** `venue.surface` already takes N images, one per sector, in edge order
  (engine.rs:1263).
- **Controllers.** Grid controllers write `{node, param, value}` with
  `POST /graph/node op:param`, which calls `set_held` → config. `find_param` (params.rs:766)
  decides what is a param.

## Config (gen node)

```json
"views": {
  "amount": 1.0,
  "anchor": [0, 1.6, 4],  "scale": 1.0,
  "list": [
    {"name": "left",  "camera": {"projector": "p1"}},
    {"name": "front", "camera": {"surface": "wall-front", "viewer": [0,1.6,4]}},
    {"name": "right", "camera": {"eye": [1,0.6,3], "target": [0,0,0], "fov": 50},
     "add": {"hue": 0.1}, "set": {"seed": 3}}
  ],
  "vary": [{"param": "hue", "spread": 0.3}, {"param": "speed", "spread": 0.2}]
}
```

- `vary` applies a linear spread across the views. View `k` of `K` gets
  `base + amount * spread * (2k/(K-1) - 1)`, so it is centred on the base value. With K = 1
  the offset is 0.
- `anchor` and `scale` map venue metres into model space: `model = (world - anchor) * scale`.
- Every value is clamped to the param's own min..max.

## Ports

- Each view adds an **output port `view.<name>`** (`Image`) on a gen node. The ports are
  dynamic by prefix, the same way `tex.<part>` inputs are.
- `image` stays the base, unvaried render.
- Only views with a downstream edge are rendered.
- Typical wiring: `gen.chamber:view.left → venue.surface(left wall)`, and so on, one per
  projector output.

## Tasks (Haiku subagents, one worktree + target each)

| id | crate | task | depends |
|----|-------|------|---------|
| T1 | metal-graph | `views` module: `GenView`/`Views` parse, `apply_view(row, names, mins, maxs, view, k, K, amount)`, `view.<name>` ports (`is_view_port`), lint accepts them only when the view exists, `find_param` exposes `view_amount` and `set_held` writes `views.amount` | – |
| T2 | metal-perform venue | `Venue::projector_camera(id, anchor, scale) -> Option<[f32;7]>`, `Venue::surface_camera(surface, viewer, anchor, scale) -> Option<([f32;7],[f32;3])>` (camera plus off-axis track), throw-ratio → fov | – |
| T3 | metal-perform engine/paint | per-view rows after `after_scripts`; resolve the camera via T2; render each wired view into its own buffer; edges from `view.<name>` read that buffer | T1, T2 |
| T4 | metal-dash | properties "Views" section (list, camera source dropdown, vary rows, amount) and a "one view per projector" action that creates the views and wires each one to its projector's surface or output; the canvas draws `view.*` ports | T1 |
| T5 | integration | merge, full tests, fix reds, gaps follow-up | all |

## Out of scope (v1)

- Per-view AI weights. AI drives the shared base row only.
- Per-view audio.
- Automatic projector calibration.

---

# Round 2 (2026-11-14): tracked off-axis, parallel params, rooms

## Use cases

1. **One world, many windows (perspective).** Each wall or surface shows the same 3D scene
   through the viewer's eye, so the walls read as windows into one space.
   - The viewer's position comes from a webcam: `capture` → `sensor.yolo`. It reaches the
     gen node through a graph edge `sensor.yolo:control → gen:viewer`.
   - The view frustum is off-axis, through the surface rectangle.
2. **Same music, different rooms (fixed palettes).** One gen node feeds several rooms or
   outputs. Every view keeps the same music reaction (digest drive, AI weights, controllers,
   bindings), but each room has its own **fixed palette**.
   - The colour slots are marked *parallel*, and each view holds its own colours.
   - No camera is needed. Any param can be parallel, so the same mechanism covers "same
     reaction, different speed" or "different density".

## Data (gen node `views`)

- `parallel: [name]`: params that are discrete per view. A name is a float param, a colour
  slot name (`colours` key), or `camera`. These names disappear from the regular params
  section and appear once per view, in per-view sections.
- Per view:
  - `set: {float: value}`: the existing per-view floats.
  - `colours: {slot: <held colour value, same format as config.colours>}`: new.
  - `palette: <palette id>`: optional. The UI uses it to fill `colours` from a stored palette.
- A parallel param with no per-view value uses the base row.
- Params that are not parallel are shared, so every view gets the same reaction.

## Viewer and off-axis

- Gen nodes with views get a dynamic `viewer` Control input. The lint accepts it only when
  views exist, and the layout draws it.
- The engine reads the wired `sensor.yolo` reading (`state.head`), maps it to the room with
  `Venue::viewer_from(<venue sensor>, eye)`, and uses that viewer for every surface view.
- `Venue::surface_camera` returns:
  - rest eye = the surface centre plus the normal times d, where d is the viewer's distance
    to the surface plane;
  - target = the surface centre;
  - fov = 2·atan(half_h / d);
  - **track = viewer − rest eye** (model space).
- The engine writes the track into `cam_x/y/z`. The 3D models' `tracked_view_proj` then builds
  `off_axis_with` through exactly the surface rectangle.

## Built-in vision

- The `vision` feature is on in the desktop build.
- A YOLO pose ONNX detector (`Mode::Eyes` from the nose and eye keypoints) sits in metal-sense
  beside MediaPipe.
- The model downloads once into the models dir, the first time a `sensor.yolo` runs.
- The webcam is a `capture` node with a `/dev/video*` source.

## Tasks

| id | scope | task |
|----|-------|------|
| R1 | metal-graph | `parallel`, per-view `colours` and `palette`; `viewer` port (lint, layout); helpers |
| R2 | metal-sense + build | YOLO pose detector, model URL and auto-download, vision on by default |
| R3 | metal-perform | off-axis surface camera with track, viewer from the sensor edge, per-view colours into the row |
| R4 | metal-dash | parallel picker, regular params filtered, per-view sections (floats, colours, palette), viewer status, "Rooms" preset |
| R5 | app run | webcam → yolo → gen with views on living-room surfaces; screenshots; fixes |
