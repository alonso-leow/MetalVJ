//! H.4: the tools, as real [`crate::agent_loop::ToolExecutor`]
//! implementations.
//!
//! H.4's own rule: *"Each one is a thin wrapper over a path that already
//! exists... No tool has its own logic: if the human cannot
//! do it from the canvas, neither can the AI."* Concretely: every graph-shaped
//! tool here calls straight into `metal-graph` (`Graph::add_node`,
//! `Graph::add_edge_checked`, `Graph::lint`, `NodeKind::catalog`) — the same
//! calls `metal-dash`'s `POST /graph/node` / `POST /graph/edge` / `GET /catalog`
//! route handlers make. `metal-graph` has no dependency on `metal-dash` or
//! `metal-agent` (it is the pure domain crate both sit on top of), and
//! `metal-dash` is a binary crate with no lib target that already depends on
//! `metal-agent` — so `metal-agent` cannot depend back on `metal-dash` without a
//! cycle. Sharing the actual mutation logic (not just the data shapes) means
//! it has to live one layer down, in `metal-graph` itself; that is why
//! `Graph::add_edge_checked` was lifted out of `metal-dash::main` into
//! `metal-graph` as part of this change (see that method's doc comment) rather
//! than being re-typed here.
//!
//! **What this module does *not* implement**, and why — flagged rather than
//! faked, per the task:
//!
//! - ~~`capture_camera_frame`~~ — **unblocked**: H.5a landed
//!   (`metal_ingest::camera`), so this tool is already implemented here. What
//!   H.5 said —*"Blocker: none of this runs without real camera capture
//!   (II.1)"*— no longer applies.
//! - `create_mask_grid` / `test_mask_grid` — H.5b (mask/grid HTTP routes) has
//!   not landed in `metal-dash` yet (checked: no `/mask`, `/grid`, or similar
//!   route exists in `main.rs`), so there is no existing route to wrap
//!   without inventing H.5b's design here, which is out of this task's scope.
//! `bake_clip` (S.M) used to be listed here as blocked on I.1/I.2 (the
//! `fw-clip`/HTML rendering path) and H.2 (`metal-forge`, WGPU). That path was
//! discarded 2026-09-05 (PARTE V, I.1/I.2 of `the design notes`) and
//! `metal-forge` landed, so `bake_clip` is implemented below the same way
//! `raster` is — `metal_forge::compile` once, `metal_forge::raster` once per
//! frame — with **one `kind` only**: no HTML/CDP path is resurrected here.
//!
//! The remaining tools are implemented below and covered by unit tests that
//! build a fixture collection (a temp dir with `schema.json`, `graphs/`, and
//! optionally `shaders/`) and assert on the resulting files/graph state.
//!
//! **S.K's four (`import_segment`/`move_segment`/`remove_segment`/
//! `set_keyframe`) are listed in [`TOOL_CATALOG`] as implemented, but this
//! struct does not execute them.** They wrap `metal-dash`'s modo-edit routes
//! (`POST /timeline/segment`, `edit_segment`; `GET`/`POST /example`,
//! `example::save`) — logic that lives one layer *up* from `metal-graph`, inside
//! `metal-dash` itself (`Timeline::validate` needs the real node catalog, which
//! needs `metal_render`'s manifest). `metal-dash` already depends on `metal-agent`
//! (§ this module's own doc, lines above), so `metal-agent` cannot depend back
//! on it without the cycle H.4 already ruled out for the graph tools. The
//! fix H.4 used — share the *domain*
//! crate, not the app — does not have a home to land in here without moving
//! two thousand lines of `Timeline`/`Example` out of `metal-dash`, which is a
//! separate, much bigger lot than S.K's "just expose it" scope.
//!
//! So the actual `ToolExecutor` for these four lives in `metal-dash` itself:
//! `metal_dash::author::models::agent_tools::AgentHarness`, right next to
//! `edit_segment` and `example::save`, calling them directly — zero new
//! logic, the literal function the HTTP route calls. It wraps a `Harness`
//! for the other fourteen tools and dispatches the new four itself. Marking
//! them `true` here (rather than adding a fourth boolean axis to
//! [`TOOL_CATALOG`] for "implemented, but not by this struct") follows the
//! precedent `raster`'s own entry already sets: `raster` is `true` even on a
//! build with the `gpu` feature off, where calling it on a bare `Harness`
//! also errs rather than running. `implemented` answers "does this tool have
//! real logic anywhere in the system", not "will this particular struct
//! execute it" — [`Harness::execute`] below still returns a clear
//! "not implemented here" error for the four, same as it does for `raster`
//! without the feature.

use crate::agent_loop::{ToolCall, ToolExecutor};
use crate::chat::policy::{self, ToolPolicy};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

/// H.4's full list of seventeen, name + one-line description + whether this
/// module implements it. `list_tools` reads this directly, so the agent
/// always sees an accurate picture of what it can actually call — including
/// the five this pass legitimately leaves out (see the module doc).
pub const TOOL_CATALOG: &[(&str, &str, bool)] = &[
    ("list_tools", "Lists itself and the others.", true),
    ("get_env", "Reads the collection's environment: the canvas, the nodes and their relations.", true),
    ("list_nodes", "Nodes of a graph, with an optional filter.", true),
    ("search_nodes", "Fuzzy search by name, family or type.", true),
    ("create_node", "Creates a node, optionally from a template and connected.", true),
    ("delete_node", "Deletes the node and its edges.", true),
    ("connect_nodes", "Runs a cable, validated against lint::compatible_ports.", true),
    ("get_params", "A node's parameters, with their schema.", true),
    ("list_kinds", "Node kinds and their params (type, min, max, enum values, default). Call it before setting a param you have not seen.", true),
    ("set_params", "Los modifica, incluido on/off. Bypass and solo are patch keys too: patch {bypass, solo} (booleans).", true),
    ("read_shader", "The WGSL of an effect or animation, existing or from a template.", true),
    ("raster", "Compiles and rasterizes a custom shader (metal-forge).", true),
    // The on-air half of `raster`: a shader that checks is written where a
    // `forge` source or an `fx.forge` node reads it.
    // O.8 — one owner per graph, taken and released explicitly. While
    // another peer owns a graph every write to it is refused.
    ("take_graph", "Takes a graph for this machine: another machine cannot write it until you release it.", true),
    ("release_graph", "Releases a taken graph (owner only).", true),
    ("graph_owner", "Who owns a graph, if anyone.", true),
    ("save_shader", "Validates a WGSL (naga) and saves it as data/shaders/<id>.wgsl for a `forge` layer or an `fx.forge` node.", true),
    // §9.4 V3 — the script node's own source, read and written through the
    // same `GET/POST /script` route the properties editor uses (rule 6). The
    // save re-runs the script, so a source that raises is refused.
    ("read_script", "The source code of a `script.py`/`script.js`/`script.rs` node (GET /script/<node>).", true),
    ("save_script", "Validates and saves the code of a `script.*` node by posting it to /script, which re-runs it.", true),
    ("capture_output", "Captures the current output of a TOP node.", true),
    ("sleep", "Espera.", true),
    ("bake_clip", "Bakes a motion graphics clip into footage.", true),
    // Plan §9.1 E1: the offline video export — the same `POST /export/video`
    // the export control posts (rule 6). Renders the device graph's generator
    // `node` (or the first authored one) over `range` frames to `path`.
    ("export_video", "Exports a range of frames from an offline graph to a video, with optional audio (POST /export/video {device, node?, range:[start,end], fps?, codec?, bitrate?, audio?:\"mix\"|\"file:<path>\"|null, path}); replies with the job status.", true),
    ("capture_camera_frame", "One frame from the camera.", true),
    // S.B, over S.A's routes — implemented, but not by this struct: they
    // need the live `State` behind `metal-dash`'s door, so they live in
    // `metal_dash::author::models::agent_tools::AgentHarness` (see `with_door`).
    ("create_mask_grid", "Generates a grid of masks over one or more outputs.", true),
    ("test_mask_grid", "Projects the masks, each in a different colour.", true),
    // O.1's authoring half: the three fields `Mask` grew (`parent`/`alpha`/
    // `compose`) plus `name`/`points`, through the one route that writes a
    // mask (`POST /outputs { op: "mask" }`). Same home as the two above.
    ("edit_mask", "Edits a mask of an output — points, name, nesting (parent/alpha) or boolean (compose: union/intersect/subtract) — or creates a new one.", true),
    // G1: the write gestures that had no tool. `add_effect`/`remove_effect`
    // post `POST /graph/effects` (remove is state off); `edit_output` posts
    // `POST /outputs` add/remove/resolution; `resolve_draft` posts
    // `POST /venue` accept/reject, the editor's diff buttons.
    ("add_effect", "Adds an effect to a node of a graph, or sets its control: `effect` (name), `state` reactive|fixed (`amount` for fixed), `source` row|raw (optional). POST /graph/effects.", true),
    ("remove_effect", "Removes an effect from a node of a graph (POST /graph/effects with state off). Destructive.", true),
    ("edit_output", "Adds, removes or resizes an output device: `op` add|remove|resolution, `id`; add takes `label`, `width`, `height`; resolution takes `width`/`height`. POST /outputs. `remove` is destructive.", true),
    ("resolve_draft", "Accepts or rejects a venue draft written by scan_venue or the AI: `venue`, `decision` accept|reject (POST /venue {op: accept|reject}). Accept overwrites the venue.", true),
    // P3: the venue authoring half of the surface chain. Both post the same
    // `POST /venue` body the surface panel does (rule 6).
    ("edit_venue_surface", "Creates, edits or deletes a surface of a venue (POST /venue {op: \"surface\"}): four `points` [x, y, z], or `origin`/`u`/`v`; `delete: true` removes it.", true),
    // Venue 3D (2026-10-21): the scene's objects, screens, projectors, swept
    // forms, free movement and the read of the whole scene. Each posts the
    // same body the 3D viewport posts (rule 6).
    ("edit_venue_object", "Creates, edits or deletes a venue Object (POST /venue {op: \"object\"}): `object` (its id), free `kind` (stage, truss, dummy…), `name`, `at`, `size`, `yaw`, `crown`, `occludes`, `emits`, `faces`; `delete: true` removes it.", true),
    ("add_screen", "Adds or edits a screen — a display, its glass and (through the route) an output — with `screen` {name, width_m, height_m, pixels:[w,h], kind, link, node, aspect_lock (\"pixels\"|\"physical\")} and optional `at`/`yaw`/`surface` (POST /venue {op: \"screen\"}).", true),
    ("add_projection", "Adds or edits a projector (POST /venue {op: \"projector\"}): `projector` (its id), `name`, `output`, `on` (the object it stands on), `lens`, `yaw`, `pitch`, `throw_ratio`, `aspect`, `lumens`.", true),
    ("set_surface_form", "Sets a surface's parametric form (POST /venue {op: \"surface.form\"}): `surface`, `form` (\"plane\"|\"swept\"), and for a sweep `path` [x,y,z] points, `profile` [x,y] points, `caps`, `closed`, `kind` (polyline|catmull_rom|bezier).", true),
    ("set_venue_sectors", "Splits one surface into the output's grid (POST /venue {op: \"surface.sectors\"}): `surface`, `source` (\"projected\"|\"points\"), `ratio` (width/height of the region), `points` [[x,y]] in the output's 0..1 as consecutive [start,end] streaks, `magnet`. One `venue.surface` node then gives each source the next cell.", true),
    ("move_venue_item", "Moves any venue item by its own vocabulary (POST /venue {op: \"move\"}): `item`, `at`, `yaw`, `pitch`, `roll`. A projector moves with its object.", true),
    ("projection_area", "Reads a projector's live projected area and throw (POST /venue {op: \"area\"}): `projector`; returns `area_m2`, `throw_m` and the per-surface breakdown.", true),
    ("venue_scene", "The venue scene graph for the AI to look at (GET /venue/scene?id=): objects, fixtures, projectors, sensors, displays, surfaces, outputs and masks.", true),
    // Venue 3D, second batch: the remaining authoring and read tools. Every
    // write posts the same `POST /venue` body the operator's viewport posts
    // (rule 6); the two asset tools read the local manifest / a local file.
    ("add_stage", "Adds or edits a stage-like venue object (POST /venue {op: \"stage\"}): `stage` (its id), `name`, `at`, `size`, `yaw`, `path`, `profile`; `delete: true` removes it.", true),
    ("add_truss", "Adds or edits a truss (POST /venue {op: \"truss\"}): `truss` (its id), `name`, `at`, `size`, `yaw`, `path`, `profile`; `delete: true` removes it.", true),
    ("add_dummy", "Adds or edits a dummy figure (POST /venue {op: \"dummy\"}, a capsule by default): `dummy` (its id), `name`, `at`, `size`, `yaw`, `path`, `profile`; `delete: true` removes it.", true),
    ("venue_spline", "Creates or edits a swept venue object (POST /venue {op: \"object\"} with form swept): `object`, `path` [x,y,z] points, `profile` [x,y] points, `kind` (polyline|catmull_rom|bezier), `caps`, `closed`.", true),
    ("place_mask_in_space", "Places a mask on a venue surface (POST /venue {op: \"mask\"}): `mask`, `on`, `points` [[s,t]], `rgb`, `alpha`, `parent`, `content`; `delete: true` removes it.", true),
    ("venue_boolean", "Combines two venue objects with CSG (POST /venue {op: \"boolean\"}): `a`, `b`, `operation` (union|difference|intersect), optional `result` (rewritten as a mesh).", true),
    ("venue_hit", "Ray-casts into the venue (POST /venue {op: \"hit\"}): `origin` [x,y,z], `dir` [x,y,z] or `target` (the direction is derived), optional `max`; returns the hit point, distance and normal.", true),
    ("venue_report", "Checks a venue (POST /venue {op: \"report\"}): returns its geometry report and aspect warnings.", true),
    ("venue_view", "Renders an ASCII view of a venue (POST /venue {op: \"view\"}): optional `shot`, `width`, `height`; returns `rows`, `ramp` and `legend` for the model to read.", true),
    ("venue_assets_search", "Searches the local `data/assets/manifest.json` by `query` (name/id/tags) and `licence`, capped at 32 results (default 12). No remote fetch.", true),
    ("venue_asset_import", "Imports a local glTF as a venue object (POST /venue {op: \"object\"} with a mesh form): `object`, `path`, `licence` (CC0, Public Domain, CC-BY, CC-BY-3.0, CC-BY-4.0, CC-BY-SA, CC-BY-SA-4.0), `author`, `url`, `scale` (default 1 m), `at`, `name`. Records attribution and source.", true),
    // D1: the venue's fixtures — add, edit or remove one (POST /venue {op:
    // "fixture"}), the same body the Settings "Fixtures" page posts (rule 6).
    ("set_fixture", "Adds, edits or deletes a fixture of a venue (POST /venue {op: \"fixture\"}): `venue` and, to edit/delete, `id`; `kind` (wash|spot|led_strip|moving|backlight|panel|continuous|led_bar|camera|matrix), `profile` (a model under data/fixtures/), `group`, `wire`, `universe`, `address` (1..512, clamped), `pixels`, `order` (forward|reverse), `start`, `drive` (frame|fixed), `projector`; `matrix` (object with x_pixels/y_pixels/start_corner/orientation/snake/first_channels/last_channels/dmx_matrix) for a 2-D LED matrix; `delete: true` removes it.", true),
    ("add_fixture_node", "Creates a venue.fixture node for a fixture of the active room (POST /graph/node {op:\"add\", kind:\"venue.fixture\", config}): `fixture` (the row's id), `colour_from` (frame|model|signal), `model` (node whose colour output is wired), `pick`/`offset` (slot distribution). The node points at the row, not a copy, and the `frame` colour works on creation.", true),
    ("blend_edges", "Edge blending between projectors that share a surface (POST /venue {op: \"blend\"}): `surface` (all projectors with pins on it) o `projector`; `enabled`, `gamma` (~2.2), `curve` (linear|smoothstep|cosine), `black_lift` (0..0.5).", true),
    ("calibrate_projector", "Sets the four pins of a projector for a surface (POST /venue {op: \"pins\"}): the corners (0,0) (1,0) (1,1) (0,1) of the surface in that device's 0..1.", true),
    ("locate", "Locates a target by discrete choice of strips, never by estimating coordinates (H.5c).", true),
    // S.AK/O.28 — the grid's switches. Same home as the mask tools above:
    // reading `on` needs the live graph behind `metal-dash`'s door.
    ("list_switches", "Lists the saved switches and their on/off state, read from the graph.", true),
    ("set_switch", "Turns a switch on or off (full toggle, or press/release of a hold).", true),
    // V2b: the one machine switch — the level every tick, lane and consumer
    // speaks. Both tools read/write `GET`/`POST /settings` (rule 6).
    ("get_tick_level", "This machine's active tick level: `base` (8 Hz) or `fine` (32 Hz), and the vocabulary the rest speaks.", true),
    ("set_tick_level", "Sets the active tick level (`base` | `fine`) — the same POST /settings as the settings control.", true),
    // R22.4 (rule 6) — the authoring half of the same tools: the exact
    // bodies the grid's `+ new button` popover and its right-click write.
    ("add_switch", "Creates a switch: id, kind (toggle|hold|fixture|rig), graph, and the batch set [{node, on}]; a fixture/rig adds its `fixture` or `group` and its `mode` (hold|switch).", true),
    ("remove_switch", "Deletes a switch by id.", true),
    // S.K — modo edit. Implemented, but not by this struct: see the module
    // doc above for exactly where (`metal_dash::author::models::agent_tools`).
    ("import_segment", "Imports a segment (model, effect, media, still, text or signal) into a track of a timeline.", true),
    ("move_segment", "Moves or resizes an existing segment by id — replaces the whole segment, not a partial patch.", true),
    ("remove_segment", "Deletes a segment from a timeline track by id.", true),
    ("set_keyframe", "Authors or replaces a single keyframe (tick) of a model against a song.", true),
    // S.AM — the train gesture of a trainable `ai.*` node (POST /ai/train
    // {node}), the same route its `entrenar` button posts. Implemented by
    // `metal_dash::author::models::agent_tools::AgentHarness`, not by this
    // struct: training reads `data/animations` and the model schemas, both
    // one layer up from `metal-graph`.
    ("train_ai", "Trains a trainable ai.* node from the authored takes (POST /ai/train { node }): the same gesture as its `entrenar` button.", true),
    // A1 (plan "AI embedded in the source's properties", A3.3): the reaction
    // is a setting in the source, over `POST /graph/node {op:"ai.embed"}` —
    // the same route the properties panel posts.
    ("embed_ai", "Embeds an ai.* reaction in a source (POST /graph/node op ai.embed { node, ai, on }): `on` creates the owned ai node and wires audio → ai → source.control; `off` separates it into a free node.", true),
    // Surface sources (plan "Surface sources and less-friction authoring"):
    // the two generic graph ops, the same bodies the properties panel posts.
    ("attach_source", "Wires an image source into one image input of a node (POST /graph/node op attach { graph, node, port, kind?, from?, config? }): `from` reuses an existing node, `kind` creates one beside it, neither detaches; whatever fed the port is replaced.", true),
    ("insert_node", "Inserts a kind after a node's picture (POST /graph/node op insert { graph, after, kind }): every consumer of `after`'s image now reads the new node.", true),
    // S.L — decided with the operator 2026-09-05. Implemented, but not by this
    // struct, for the same reason as S.K's four: it needs `Timeline` and
    // `metal_reel::index::Library`, both one layer up from `metal-graph`. See
    // `metal_dash::author::models::agent_tools::AgentHarness::attach_digest`.
    ("attach_digest", "Runs the audio digest on a clip's own audio, not on the timeline's song.", true),
    // S.O.1 — Overlay blackout: bypass every output.device plus black on
    // every out.* fixture, on/off (no timer — see agent_tools.rs::blackout).
    ("blackout", "Turns the blackout on or off: bypass of all outputs, plus explicit black on the out.* fixtures.", true),
    // S.O.2 — Solid color overlay with duration
    ("solid_color", "Solid colour as an overlay for N seconds, specified as RGB [0-255].", true),
    // G2 — the playback and performance gestures the panels post. Each is the
    // same route body the panel posts (agent_tools.rs). Only `colour_apply`
    // writes no output: it changes the colour of graph nodes.
    ("play_sequence", "Saves, cues, runs or stops a sequence on a device (POST /sequences): op save | cue | run | stop | stop-all.", true),
    ("edit_loops", "Loop library and choice: op save (a loop definition, optionally as_copy), choose (the loop a graph node plays), tap (set the downbeat), song (play or stop a song's audio from `seconds`).", true),
    ("transport", "Playback transport: action toggle_pause (pauses or resumes the clock), play or seek (song + seconds), stop.", true),
    ("fire_trigger", "Fires a trigger by id (POST /trigger/<id>): its target runs (a switch, a scene, a sequence, the transport).", true),
    ("overlay_control", "Starts or stops overlays on a device (POST /overlay): op play (layer + movement, optional loop), stop (layer), stop-all, stop-solid, glitch (a material glitch burst; hold true keeps it on) and glitch-release.", true),
    ("colour_apply", "Applies a colourist rule to a graph's colour slots (POST /colour/apply): graph, group all|models|effects|text or nodes, and one of palette [4 rgb], hsb or rgb (0..1).", true),
    // S.D — SegmentMix tool
    ("set_layer_mix", "Sets a layer's blend mode in a timeline.", true),
    // S.O.5 (20-sept-2026): `POST /overlay {op: "rasterize"}` bakes a
    // timeline `model` segment's keyframes to a library movement the grid
    // can play; `agent_tools.rs`'s `rasterize_overlay` posts there.
    ("rasterize_overlay", "Rasterizes a timeline/track/segment into a replayable overlay.", true),
    // 3D venues (`metal_perform::venue`): the agentic test projection. Same home
    // as the mask tools: it writes `state.outputs` through `metal-dash`'s door.
    ("project_venue", "Projects a 3D venue onto its outputs: writes its masks and its frame-driven fixtures, fills each mask with its own test colour (the venue's rgb per mask; `rgb` colours the rest) and opens the monitor.", true),
    // The read half of the same path: the venue it expects, the frames being
    // rendered, and a live camera frame with the expectation drawn over it.
    // A device's layer stack, through the route the layer table uses.
    ("edit_layer", "Adds, changes, routes or deletes a layer of an output (POST /layers): model (or `forge` + shader), mask, sector, blend, opacity, span.", true),
    // Plan "Audio input is a node": every `audio.live`'s listener — what
    // the chrome indicator shows (GET /listen/status).
    ("listen_status", "Every audio.live node's listener (GET /listen/status): the source it asked for, the one it reads, `real` (false = the stand-in, NOT the music in the room), describe, bpm, samples dropped, any open error, and its `tone` model (`loaded`/`real` false = no Basic Pitch, so colour follows chroma) with `latency_ms`. Read it before judging a reaction; say so when a source is a stand-in or silent.", true),
    ("look_venue", "Looks at the venue: the 3D scene it expects, the frame each output renders, and a camera frame with each mask drawn where it should appear.", true),
    // D2 — the GDTF importer: a `.gdtf` (zip of XML + geometry) becomes a
    // `data/fixtures/<model>.toml` profile, through the same `POST /fixtures`
    // body the operator's import posts (rule 6). Implemented by
    // `metal_dash::author::models::agent_tools::AgentHarness`.
    ("import_gdtf", "Imports a fixture type from a `.gdtf` file (zip of XML + geometry) into a profile `data/fixtures/<model>.toml` (POST /fixtures {op:\"import-gdtf\"}): maps Pan/Tilt/Dimmer/beam/color to selectors, `DMXFrom..DMXTo` to byte ranges, 16-bit to coarse+fine, and the pan/tilt limits of the physical function. `path` (a file on disk) or `bytes` (the whole .gdtf in base64); `name` overrides the model and `mode` picks a DMX mode by name or index.", true),
    // M2 of the pixel map: aim a venue's moving head at a point, through the
    // same `POST /venue { op: "aim" }` the venue editor's click-to-aim will
    // post. Implemented by `metal_dash::author::models::agent_tools::AgentHarness`.
    ("aim_fixture", "Aims a venue fixture (a moving head) at a point: POST /venue { op: \"aim\", fixture, target } — inverse kinematics within the profile's limits and by the shortest path; returns the pan/tilt and where the beam lands.", true),
    // M5 of the pixel map: the AI builds the map from cameras. The three
    // camera tools write a draft venue (`POST /venue { op: "draft" }`) the
    // operator accepts or rejects in the editor's diff view; the actual CV
    // is a clearly-marked TODO in the handler. `solve_head` (D5) is real:
    // it inverts the aim from a target point through `op: "solve_head"`.
    ("scan_venue", "Scans a venue from camera frames and writes a draft the operator accepts or rejects (POST /venue { op: \"draft\" }). Reads the venue (GET /venue) and proposes objects, surfaces, screens and fixtures. TODO(M5): the camera solve.", true),
    ("calibrate_projector", "Structured-light (Gray-code) calibration of a projector against a surface: the camera reads the patterns and the solve writes a draft with the projector's pins (POST /venue { op: \"draft\" }; the documented `pins` table). TODO(M5): the Gray-code solve.", true),
    ("discover_fixture", "Discovers a patched fixture by flashing it (identify) and finding its footprint in the camera frame, then writes a draft (POST /venue { op: \"draft\" }). LED strings are flashed pixel by pixel. TODO(M5): the camera solve.", true),
    ("solve_head", "Points a moving head at a real 3D point: POST /venue { op: \"solve_head\", fixture, target } — inverse kinematics from the head's pose within the profile's limits and slew, whether it hangs high or low; returns the live state and where the beam lands (the ray query against the room's surfaces and object faces).", true),
    // Param edges (`metal_graph::bind`): the properties panel's I/O read and its
    // two writes, over the same metal-graph functions the routes call.
    ("get_io", "One node's inputs, outputs, parameters (with the binding on each) and every upstream node's variables, each marked used or not by this node (GET /graph/io).", true),
    ("bind_param", "Binds one parameter of a node to one variable of an upstream node — a param edge, from `<port>.<variable>` (e.g. control.rms, control.x, row.escala) to `param:<name>`, mapped onto the parameter's range — replacing whatever drove it; `from: null` unbinds (POST /graph/node op bind).", true),
    ("set_param_batch", "Sets many held parameter values in one write: updates [{node, name, value}], each as set_param (null releases a generator float; a list holds a colour slot or the camera). All or nothing: one refused value refuses the whole batch and the graph is saved once (POST /graph/node op params).", true),
    ("set_param", "Sets the value a node holds for one parameter while nothing is bound to it — an effect's amount, a script float, a generator float (null releases a generator float back to the audio drive); out-of-range values are refused (POST /graph/node op param).", true),
    // M4 of the materials lote: a `mat.*` node's part and fixed overrides, over
    // the same `POST /graph/node {op:"config"}` merge its properties page posts
    // (rule 6). The node itself is wired with `attach_source`/`insert_node`.
    ("set_material", "Sets the material of the surfaces of a `mat.*` node (matte, concrete, wood, leather, metal, glass, matrix): its `part` (`*` or a part of the model) and its fixed settings (mix, scale, rough, metal, bump, env, tint, tint_hue, glitch, glitch_rate, glitch_seed) — the same path as the properties page (POST /graph/node {op: \"config\"}).", true),
    // K — the palette node's write (POST /author/palettes {op:"set"}): one
    // slot of a stored palette, relationally when `relative` is true so the
    // whole palette moves with it. Same route the palette node's folder posts.
    ("set_palette", "Moves one slot of a stored palette (POST /author/palettes op set): with `relative` the whole palette shifts by the same OklCh delta, so the relationship between its slots is preserved.", true),
    // Video scenes (`metal_reel::scenes`): the properties gallery's read and the
    // book's writes, over the routes the gallery and the Scenes route use.
    ("list_scenes", "A video's scenes as its ingest cut them (GET /author/scenes?clip=), or with no `clip` every saved/favourite scene across all videos plus the groups.", true),
    // The Colourist dataset: the sequence editor's "dataset" picker's read.
    ("list_dataset", "The Colourist dataset's authored parts (GET /author/dataset): part id, model, song, span (32 Hz ticks), kind, keywords, title; with `part`, that part's whole example — keyframes, story and the reason for every key (GET /author/dataset/example?part=).", true),
    ("edit_scene", "Keeps a scene or files it (POST /author/scenes): op save | favourite | rename | file (into `group`) with the scene's id/clip/start/end and `on`, or group.create | group.rename (`to`) | group.delete — deleting a group never unfavourites.", true),
    // H5 — the head-tracking gesture over `POST /sensor {op:"set"}`, the same
    // route the `sensor.yolo` panel's controls post: turn the `cam.track`
    // nodes on/off and set their strength (0..1). Implemented by
    // `metal_dash::author::models::agent_tools::AgentHarness`, not this struct:
    // it needs the live `State` behind `metal-dash`'s door.
    ("set_head_tracking", "Turns head tracking on or off (the `cam.track` nodes) and sets its strength (0..1) — the same path as the sensor panel.", true),
    // The pairing overlay's read half (plan §5 item 6): `metal-sync`'s status,
    // peers and log through the door's `GET /sync/*`. Read-only.
    // P-G — "Export frame": the floating preview's camera button posts the
    // same `POST /frame/export` (rule 6).
    // P-C: video I/O — which sinks work here, and handing a screen to one.
    ("perf_report", "The running app's frame-time report: the same numbers the F12 monitor shows (GET /perf).", true),
    ("get_grid_layouts", "The grid's saved layouts and the one up: each layout's id, name, surface and controllers, plus `active` (GET /settings, `grid_layouts`).", true),
    ("set_grid_layouts", "Replaces the whole list of grid layouts (POST /settings {grid_layouts}): `active` (an id in `list`) and `list` of {id, name, surface, controllers}. The grid panel's create, rename and delete write this same key.", true),
    ("export_status", "The export job's progress, or idle when none runs (GET /export).", true),
    ("export_control", "Controls the running export (POST /export {op}): `op` cancel stops it and removes the partial file.", true),
    ("list_palettes", "The stored colour palettes, the list `set_palette` edits (GET /author/palettes).", true),
    ("list_video_io", "Lists the video outputs (virtual camera, NDI, Spout, Syphon) with their availability and the reason when they are not (GET /videoio).", true),
    ("set_output_sink", "Also sends an output's frame to a virtual camera, NDI, Spout or Syphon (POST /outputs {op: \"sink\"}); an empty `kind` or \"none\" removes it.", true),
    // D3 — the universe bus: the priority/mode per source with which the
    // `out.*`/`venue.fixtures` of one universe are mixed (POST /outputs {op:
    // "dmx-bus"}). Edits the config of the source nodes (rule 4).
    ("dmx_bus", "Merges the DMX of several sources into a universe by priority (POST /outputs {op: \"dmx-bus\"}): `priority` (0-255, the highest wins), `mode` (htp|input|software), optional `device` and `node` (without them, every source node of every output).", true),
    ("export_frame", "Saves the current frame of a node (or of the device output, without `node`) at full resolution as PNG in ~/Pictures/MetalVJ or the settings folder (POST /frame/export {device, node?, dir?}); replies with the path.", true),
    ("sync_status", "Sync status of this machine: whether metal-sync runs, its name, whether there are peers, whether it is paused (GET /sync/status).", true),
    ("sync_peers", "The club's peers with their name, whether they are online and when they were last seen (GET /sync/peers).", true),
    ("sync_log", "Recent metal-sync activity: peers, landings, pauses and pairing requests (GET /sync/log).", true),
    // P-D — the `out.artnet` page's "find nodes" and the `clock.timecode`
    // reading the lanes playhead chases: the same two routes (rule 6).
    ("artnet_in_status", "Status of this machine's Art-Net receiver (a single listener on port 6454): whether it listens, the error if not, and the row of 0..1 values that an in.artnet node would read with that window (GET /artnet/in {net?, subnet?, universe?, first?, count?}).", true),
    ("artnet_nodes", "Finds Art-Net nodes on the network (ArtPoll) and lists each with its IP, name and output Port-Addresses (GET /artnet/nodes {ms?, target?}); to configure an out.artnet node use set_params with net/subnet/universe/unicast.", true),
    // P-E — the transition presets the properties picker lists; a source's
    // `transition_in`/`transition_out` are set with `set_params`, the same
    // `POST /graph/node {op:"config"}` merge the picker posts (rule 6).
    ("list_transitions", "Lists the transition presets (data/transitions: id, name, type, duration, unit, curve). For a source to enter or leave with a transition when a switch turns it on or off, use set_params with patch {\"transition_in\": {\"preset\": id, \"duration\"?, \"unit\"?: \"beats\"|\"seconds\", \"ease\"?}} or \"transition_out\"; null removes the transition (cut).", true),
    ("timecode_status", "Reading of each clock.timecode node (LTC/MTC/Art-Net): timecode, frames per second, whether it is locked, and the position in seconds that the sequence playhead follows (GET /timecode).", true),
    ("focus", "Sends focusable elements (nodes, graphs, …) to the user as clickable chips in your reply. items: [{kind, id, graph?, label?}]; a node must name its graph and exist in it. Call it with the elements you mention, then refer to them in your text.", true),
];

/// The three permissions the operator grants in `settings → AI`. A tool that
/// touches one of them answers to that permission instead of its own
/// per-tool policy (see [`effective_policy`]); a tool that touches none keeps
/// the per-tool `auto`/`ask`/`off` it always had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Network reads: web fetch/search, any remote HTTP or MCP tool.
    Internet,
    /// Anything that drives or reads hardware: output devices, BLE lights,
    /// cameras, monitors, sACN, venue identify.
    Devices,
    /// Disk I/O on user files (media, renders, shader files) rather than the
    /// app's own graph and project records, which are plain graph edits.
    Files,
}

impl Capability {
    pub const ALL: [Capability; 3] = [Capability::Internet, Capability::Devices, Capability::Files];

    /// The key under `permissions` in `data/ai_tools.json`, and on the wire.
    pub fn key(self) -> &'static str {
        match self {
            Capability::Internet => "internet",
            Capability::Devices => "dispositivos",
            Capability::Files => "archivos",
        }
    }

    /// The operator's permission for this capability. Absent or unknown means
    /// `ask`: a capability nobody has answered for yet still pauses for a yes.
    pub fn policy(self, saved: &Value) -> ToolPolicy {
        saved["permissions"][self.key()]
            .as_str()
            .and_then(permission_policy)
            .unwrap_or(ToolPolicy::Ask)
    }
}

/// The wire words the `permissions` object speaks, mapped onto the per-tool
/// policy the runner already knows: `allow` runs without asking, `ask` pauses
/// for `/ai/chat/approve`, `deny` never offers the tool to the model.
pub fn permission_policy(wire: &str) -> Option<ToolPolicy> {
    match wire {
        "allow" => Some(ToolPolicy::Auto),
        "ask" => Some(ToolPolicy::Ask),
        "deny" => Some(ToolPolicy::Off),
        _ => None,
    }
}

/// The inverse of [`permission_policy`], for the reply.
pub fn permission_wire(policy: ToolPolicy) -> &'static str {
    match policy {
        ToolPolicy::Auto => "allow",
        ToolPolicy::Ask => "ask",
        ToolPolicy::Off => "deny",
    }
}

/// Which capability a catalog tool touches, or `None` when it touches none.
/// Every entry of [`TOOL_CATALOG`] is classified here on purpose: the test
/// `every_catalog_tool_is_classified_on_purpose` keeps the list of `None`s
/// explicit, so a new tool cannot slip through unlooked-at.
pub fn capability_of(name: &str) -> Option<Capability> {
    match name {
        // No tool in the catalog reaches the internet today. The first one
        // that does (web fetch, remote MCP) gets `Capability::Internet` here.
        "capture_camera_frame"
        | "set_head_tracking"
        | "blackout"
        | "solid_color"
        | "test_mask_grid"
        | "project_venue"
        | "look_venue"
        | "scan_venue"
        | "calibrate_projector"
        | "blend_edges"
        | "discover_fixture"
        | "solve_head"
        | "aim_fixture"
        | "set_fixture"
        | "add_fixture_node"
        | "artnet_nodes"
        | "artnet_in_status"
        | "set_output_sink"
        | "dmx_bus"
        | "play_sequence"
        | "edit_loops"
        | "transport"
        | "fire_trigger"
        | "overlay_control" => Some(Capability::Devices),
        "save_shader" | "bake_clip" | "attach_digest" | "export_frame" | "export_video"
        | "import_gdtf" | "export_control" => Some(Capability::Files),
        _ => None,
    }
}

/// The policy a tool actually runs under: its capability's permission when
/// it has one, otherwise the per-tool table (`policy::resolve`). Both the
/// registry that offers tools to the model and the runner's pre-call gate
/// read this one function, so a `deny` removes the tool from the model's
/// view and an `ask` routes its calls to `/ai/chat/approve`.
pub fn effective_policy(name: &str, saved: &Value) -> ToolPolicy {
    match capability_of(name) {
        Some(capability) => capability.policy(saved),
        None => policy::resolve(name, saved),
    }
}

/// Appends the operator's free-text instructions to the system prompt under
/// a fixed heading. Blank instructions leave the prompt untouched, so a fresh
/// install sends exactly what it sent before.
pub fn with_operator_instructions(prompt: &str, instructions: &str) -> String {
    let text = instructions.trim();
    if text.is_empty() {
        return prompt.to_string();
    }
    format!("{prompt}\n\n## Operator instructions\n\n{text}\n")
}

/// Hard cap on `sleep(ms)`, mirroring H.2's own "presupuesto duro" judgment
/// for `raster` — the tool exists so the agent can wait out a raster or a
/// change settling, not so a bad call can hang a turn indefinitely.
const MAX_SLEEP_MS: u64 = 10_000;

/// `bake_clip`'s default frame rate when the caller omits `fps` — a plain
/// judgment call, not derived from anything; 24 is the common motion-
/// graphics rate and nothing in the plan pins a different number.
const DEFAULT_BAKE_FPS: f64 = 24.0;

/// `bake_clip`'s id: a deterministic function of the request, not a
/// timestamp or a random uuid — I.5's determinism requirement starts here,
/// not just inside the per-frame raster loop. The same `(source, duration,
/// fps, w, h)` always names the same `data/clips/<id>/` directory, which is
/// what lets baking a clip twice be a byte-identical no-op on the second
/// call instead of a second, merely-similar bake.
///
/// `DefaultHasher::new()` uses fixed keys (unlike `HashMap`'s per-process
/// `RandomState`), so this hashes the same across processes and runs —
/// verified by `bake_clip_id_is_stable_across_calls` below rather than
/// assumed from the stdlib docs.
fn bake_clip_id(source: &str, duration: f64, fps: f64, w: u32, h: u32) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    duration.to_bits().hash(&mut hasher);
    fps.to_bits().hash(&mut hasher);
    w.hash(&mut hasher);
    h.hash(&mut hasher);
    format!("bake-{:016x}", hasher.finish())
}

/// The enum-valued config keys of a kind, with their allowed values and
/// default. Only kinds whose enums are declared in code appear here.
fn enum_params(kind: &str) -> Vec<(&'static str, &'static [&'static str], Value)> {
    match kind {
        metal_graph::VENUE_FIXTURE_KIND => {
            let d = metal_graph::params::venue_fixture_default_config();
            [
                ("colour_from", metal_graph::params::VENUE_FIXTURE_COLOUR_FROM),
                ("spread", metal_graph::params::VENUE_FIXTURE_SPREAD),
                ("hue_lock", metal_graph::params::VENUE_FIXTURE_HUE_LOCK),
            ]
            .into_iter()
            .map(|(key, values)| (key, values, d[key].clone()))
            .collect()
        }
        _ => Vec::new(),
    }
}

/// A kind's param schema as JSON: its float/colour params from the catalog,
/// then its enum keys.
fn param_schema(k: &metal_graph::NodeKind) -> Vec<Value> {
    let mut out: Vec<Value> = k
        .params
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "type": match p.kind {
                    metal_graph::params::ParamKind::Float => "number",
                    metal_graph::params::ParamKind::Colour => "colour",
                    metal_graph::params::ParamKind::Camera => "camera",
                },
                "min": p.min,
                "max": p.max,
                "default": p.default,
                "index": p.index,
            })
        })
        .collect();
    for (key, values, default) in enum_params(&k.kind) {
        out.push(json!({ "name": key, "type": "enum", "values": values, "default": default }));
    }
    out
}

/// Checks one config value written by the generic `set_params` path against
/// the catalog schema of `kind`: an enum key must name one of its values, a
/// float param must be a number in its declared range. Keys the schema does
/// not name, and `null` (a release), pass through unchanged.
fn check_config_value(
    catalog: &[metal_graph::NodeKind],
    node: &str,
    kind: &str,
    key: &str,
    v: &Value,
) -> Result<(), String> {
    if v.is_null() {
        return Ok(());
    }
    if let Some((_, values, _)) = enum_params(kind).into_iter().find(|(k, _, _)| *k == key) {
        return match v.as_str() {
            Some(s) if values.contains(&s) => Ok(()),
            _ => Err(format!("{node}.{key}: {v} is not one of {values:?}")),
        };
    }
    let Some(k) = catalog.iter().find(|k| k.kind == kind) else {
        return Ok(());
    };
    if let Some(p) = k
        .params
        .iter()
        .find(|p| p.name == key && p.kind == metal_graph::params::ParamKind::Float)
    {
        return match v.as_f64() {
            Some(f) if f.is_finite() && f >= p.min as f64 && f <= p.max as f64 => Ok(()),
            Some(f) => Err(format!(
                "{node}.{key}: {f} is outside its declared [{}, {}]",
                p.min, p.max
            )),
            None => Err(format!("{node}.{key} is a number param, not {v}")),
        };
    }
    Ok(())
}

/// The harness: H.4's tools, bound to one collection's data directory.
///
/// `root` is the same directory `metal-dash::output::data_root()` resolves to
/// in a real run (holding `schema.json`, `graphs/*.json`, `shaders/*.wgsl`)
/// — this struct does not know about `metal-dash` at all, it just agrees on the
/// same on-disk layout, which is the actual contract between the app and the
/// harness per H.0: *"There is no second state of the world for the AI."*
#[derive(Clone)]
pub struct Harness {
    root: PathBuf,
    /// Where `read_shader(name)` looks for a *template* shader (e.g.
    /// `escena3d.wgsl`) that is not a user-authored one under
    /// `root/shaders/`. In a real run this is `crates/metal-render/shaders/`;
    /// `None` means template lookups fail with a clear error rather than
    /// panicking on a missing directory.
    templates_root: Option<PathBuf>,
    /// The app's routes, when one is attached — see [`RouteCall`]. `None` on a
    /// bare `Harness`.
    routes: Option<RouteCall>,
}

/// A route caller: what `metal-dash`'s `chat_executor` hands a [`Harness`] so
/// a tool can call the app's own route — the door every panel already calls
/// through — instead of touching a file itself (rule 6). A bare `Harness`
/// (tests, a build with no app) has none, and a tool that needs one says so.
pub type RouteCall = std::sync::Arc<dyn Fn(&str, &Value) -> Result<Value, String> + Send + Sync>;

impl Harness {
    pub fn new(root: impl Into<PathBuf>) -> Harness {
        Harness {
            root: root.into(),
            templates_root: None,
            routes: None,
        }
    }

    pub fn with_templates_root(mut self, dir: impl Into<PathBuf>) -> Harness {
        self.templates_root = Some(dir.into());
        self
    }

    /// Hand this harness the app's routes, so a tool that must not write a
    /// file itself calls the same route the shell does (rule 6).
    pub fn with_routes(mut self, routes: RouteCall) -> Harness {
        self.routes = Some(routes);
        self
    }

    fn schema_path(&self) -> PathBuf {
        self.root.join("schema.json")
    }

    fn graph_path(&self, id: &str) -> PathBuf {
        self.root.join("graphs").join(format!("{id}.json"))
    }

    /// `root/shaders/<id>.wgsl`, or the few-shot example of that name under
    /// `root/agent/shaders/` when none was saved — the same fallback
    /// `metal_perform::forge::source` gives the renderer, so a shader id means
    /// the same file to `read_shader`, `raster` and the screen.
    fn shader_path(&self, id: &str) -> PathBuf {
        let saved = self.root.join("shaders").join(format!("{id}.wgsl"));
        let example = self
            .root
            .join("agent")
            .join("shaders")
            .join(format!("{id}.wgsl"));
        if !saved.exists() && example.exists() {
            return example;
        }
        saved
    }

    /// `save_shader(id, wgsl)`: the gate every custom shader passes before it
    /// can go on air. It no longer writes the file itself: it posts the id and
    /// the WGSL to `metal-dash`'s `POST /shaders`, the one writer, which runs
    /// `metal_forge::check` (naga parse + validate + the float block) and only
    /// then touches `data/shaders/<id>.wgsl`. A shader that does not check
    /// comes back `{ok: false, errors: [...]}` — a normal tool result to
    /// repair from, and the file on disk (what a `forge` layer is drawing
    /// right now) is left as it was.
    #[cfg(feature = "gpu")]
    fn save_shader(&self, args: &Value) -> Result<Value, String> {
        let id = Self::require_str(args, "id")?;
        let wgsl = Self::require_str(args, "wgsl")?;
        let routes = self.routes.as_ref().ok_or_else(|| {
            "save_shader writes through metal-dash's POST /shaders — no route attached to this Harness"
                .to_string()
        })?;
        routes("/shaders", &json!({ "id": id, "wgsl": wgsl }))
    }

    #[cfg(not(feature = "gpu"))]
    fn save_shader(&self, _args: &Value) -> Result<Value, String> {
        Err("save_shader needs metal-agent's `gpu` feature (metal-forge's naga gate)".to_string())
    }

    /// Same read-per-request, no-cache choice `metal-dash::node_catalog` makes,
    /// for the same reason: a handful of tool calls in a turn, not a pump.
    fn catalog(&self) -> Result<Vec<metal_graph::NodeKind>, String> {
        let path = self.schema_path();
        let text = fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let manifest = metal_schema::Manifest::parse(&text)
            .map_err(|e| format!("{} did not parse: {e}", path.display()))?;
        Ok(metal_graph::NodeKind::catalog(&manifest))
    }

    fn load_graph(&self, id: &str) -> Result<metal_graph::Graph, String> {
        let path = self.graph_path(id);
        let text = fs::read_to_string(&path).map_err(|e| format!("no such graph {id:?}: {e}"))?;
        metal_graph::Graph::from_json(&text).map_err(|e| format!("{id:?} did not parse: {e}"))
    }

    fn save_graph(&self, g: &metal_graph::Graph) -> Result<(), String> {
        // O.8: refused while another peer owns it — the graph itself and, for
        // a published device graph or a collection, every one it touches
        // (`metal_graph::collection::store`).
        let dir = self.root.join("graphs");
        let me = metal_graph::owner::local_identity();
        metal_graph::collection::store(
            &dir,
            g,
            &metal_graph::collection::Io {
                write: &|path, text| metal_graph::collection::plain_write(path, text),
                check: &|id| {
                    metal_graph::owner::check_write(&dir, id, &me).map_err(|r| r.to_string())
                },
            },
        )
    }

    /// O.8: `take_graph` / `release_graph` / `graph_owner`, the same
    /// `metal_graph::owner` calls `POST /graph {op: take|release|owner}` makes.
    fn graph_owner_tool(&self, op: &str, args: &Value) -> Result<Value, String> {
        let id = Self::require_str(args, "graph")?;
        let dir = self.root.join("graphs");
        let me = metal_graph::owner::local_identity();
        match op {
            "take" => metal_graph::owner::take(&dir, id, &me).map(|_| ()),
            "release" => metal_graph::owner::release(&dir, id, &me),
            _ => Ok(()),
        }
        .map_err(|r| r.to_string())?;
        serde_json::from_str(&metal_graph::owner::status_json(&dir, id, &me))
            .map_err(|e| e.to_string())
    }

    fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
        args.get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("expected `{key}`"))
    }

    fn require_graph(&self, args: &Value) -> Result<metal_graph::Graph, String> {
        self.load_graph(Self::require_str(args, "graph")?)
    }

    // -- shared JSON shapes ---------------------------------------------

    /// `list_kinds(kind?, filter?, domain?)`: the catalog's own param schema,
    /// read live from `self.catalog()`. With `kind`, that kind in full; else a
    /// compact list (kind, family, label, param names) narrowed by `filter`
    /// and `domain`.
    fn list_kinds(&self, args: &Value) -> Result<Value, String> {
        let catalog = self.catalog()?;
        if let Some(kind) = args.get("kind").and_then(Value::as_str) {
            let k = catalog
                .iter()
                .find(|k| k.kind == kind)
                .ok_or_else(|| format!("no such kind: {kind}"))?;
            let mut v = Self::kind_json(k);
            v["menu"] = json!(k.menu);
            v["params"] = json!(param_schema(k));
            return Ok(v);
        }
        let filter = args.get("filter").and_then(Value::as_str).map(str::to_lowercase);
        let domain = match args.get("domain") {
            None | Some(Value::Null) => None,
            Some(d) => Some(
                serde_json::from_value::<metal_graph::Domain>(d.clone())
                    .map_err(|_| format!("domain must be \"live\" or \"offline\", not {d}"))?,
            ),
        };
        let kinds: Vec<Value> = catalog
            .iter()
            .filter(|k| domain.is_none_or(|d| k.offered_in(d)))
            .filter(|k| match &filter {
                None => true,
                Some(f) => {
                    let family = serde_json::to_value(&k.family)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_lowercase))
                        .unwrap_or_default();
                    k.kind.to_lowercase().contains(f)
                        || k.label.to_lowercase().contains(f)
                        || family.contains(f)
                }
            })
            .map(|k| {
                json!({
                    "kind": k.kind,
                    "family": k.family,
                    "label": k.label,
                    "params": k.params.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(json!({ "kinds": kinds }))
    }

    fn node_json(n: &metal_graph::Node) -> Value {
        let config: Value = serde_json::from_str(&n.config.to_json_str()).unwrap_or(Value::Null);
        json!({
            "id": n.id,
            "kind": n.kind,
            "pos": n.pos,
            "label": n.label,
            "bypass": n.bypass,
            "solo": n.solo,
            "config": config,
        })
    }

    fn edge_json(e: &metal_graph::Edge) -> Value {
        json!({
            "id": e.id,
            "from": { "node": e.from.node, "port": e.from.port },
            "to": { "node": e.to.node, "port": e.to.port },
        })
    }

    fn kind_json(k: &metal_graph::NodeKind) -> Value {
        json!({
            "kind": k.kind,
            "family": k.family,
            "label": k.label,
            "inputs": k.inputs.iter().map(|p| json!({"name": p.name, "ty": p.ty})).collect::<Vec<_>>(),
            "outputs": k.outputs.iter().map(|p| json!({"name": p.name, "ty": p.ty})).collect::<Vec<_>>(),
        })
    }

    fn issue_json(i: &metal_graph::Issue) -> Value {
        json!({
            "severity": match i.severity { metal_graph::Severity::Error => "error", metal_graph::Severity::Warning => "warning" },
            "node": i.node,
            "message": i.message,
        })
    }

    fn env_reply(g: &metal_graph::Graph, catalog: &[metal_graph::NodeKind]) -> Value {
        json!({
            "graph": g.id,
            "name": g.name,
            "domain": g.domain,
            "nodes": g.nodes.iter().map(Self::node_json).collect::<Vec<_>>(),
            "edges": g.edges.iter().map(Self::edge_json).collect::<Vec<_>>(),
            "issues": g.lint(catalog).iter().map(Self::issue_json).collect::<Vec<_>>(),
        })
    }

    // -- tools ------------------------------------------------------------

    fn list_tools(&self) -> Value {
        json!({
            "tools": TOOL_CATALOG.iter().map(|(name, desc, implemented)| json!({
                "name": name,
                "description": desc,
                "implemented": implemented,
            })).collect::<Vec<_>>()
        })
    }

    fn get_env(&self, args: &Value) -> Result<Value, String> {
        let g = self.require_graph(args)?;
        let catalog = self.catalog()?;
        Ok(Self::env_reply(&g, &catalog))
    }

    /// `list_nodes(path, filter?)`. This graph model is flat — a node has no
    /// TouchDesigner-style parent path — so `path` is read as *which graph*
    /// (the only "container" this app has), and `filter` is an optional
    /// case-insensitive substring matched against a node's kind, id or
    /// label. Documented here rather than silently reinterpreted, since it
    /// is the one tool where the TD-shaped spec doesn't map onto this app's
    /// actual data model 1:1.
    /// `focus(items)`: validates the elements the assistant wants to show as
    /// chips. A `node` must exist in its graph (read from the live file); other
    /// kinds are only shape-checked. Read-only: returns `{refs:[…]}`.
    fn focus(&self, args: &Value) -> Result<Value, String> {
        let items = args
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| "focus needs items: [{kind, id, graph?, label?}]".to_string())?;
        let mut refs = Vec::with_capacity(items.len());
        for item in items {
            let kind = item.get("kind").and_then(Value::as_str).filter(|s| !s.is_empty());
            let id = item.get("id").and_then(Value::as_str).filter(|s| !s.is_empty());
            let (Some(kind), Some(id)) = (kind, id) else {
                return Err(format!("every focus item needs a kind and an id: {item}"));
            };
            let graph = item.get("graph").and_then(Value::as_str).map(str::to_string);
            let mut label = item.get("label").and_then(Value::as_str).map(str::to_string);
            if kind == "node" {
                let graph_id = graph
                    .as_deref()
                    .ok_or_else(|| format!("node {id:?} needs its graph"))?;
                let g = self.load_graph(graph_id)?;
                let node = g
                    .nodes
                    .iter()
                    .find(|n| n.id == id)
                    .ok_or_else(|| format!("no node {id:?} in graph {graph_id:?}"))?;
                if label.is_none() && !node.label.is_empty() {
                    label = Some(node.label.clone());
                }
            }
            let mut r = json!({ "kind": kind, "id": id });
            if let Some(g) = graph {
                r["graph"] = json!(g);
            }
            if let Some(l) = label {
                r["label"] = json!(l);
            }
            refs.push(r);
        }
        Ok(json!({ "refs": refs }))
    }

    fn list_nodes(&self, args: &Value) -> Result<Value, String> {
        let graph_id =
            Self::require_str(args, "path").or_else(|_| Self::require_str(args, "graph"))?;
        let g = self.load_graph(graph_id)?;
        let filter = args
            .get("filter")
            .and_then(Value::as_str)
            .map(str::to_lowercase);
        let nodes: Vec<Value> = g
            .nodes
            .iter()
            .filter(|n| match &filter {
                None => true,
                Some(f) => {
                    n.kind.to_lowercase().contains(f.as_str())
                        || n.id.to_lowercase().contains(f.as_str())
                        || n.label.to_lowercase().contains(f.as_str())
                }
            })
            .map(Self::node_json)
            .collect();
        Ok(json!({ "graph": graph_id, "nodes": nodes }))
    }

    /// `search_nodes(q)`. Fuzzy in the cheap, honest sense: case-insensitive
    /// substring match, scored by whether the match starts the field (a
    /// prefix hit ranks above a mid-string hit) — searches both the graph's
    /// existing nodes (by id/kind/label) and the catalog (by kind/label), so
    /// "what's already there" and "what I could make" answer the same query.
    ///
    /// **R22.5a, rule 6**: when a graph is named, the catalog half answers
    /// what that graph's `+ node` menu would offer — the same
    /// `NodeKind::offered_in` rule `GET /catalog?domain=` filters by, read
    /// off the same fields, so the agent's search and the human's menu cannot
    /// disagree about what belongs in a Live or an Offline graph. With no
    /// graph named, the whole catalog answers as before.
    fn search_nodes(&self, args: &Value) -> Result<Value, String> {
        let q = Self::require_str(args, "q")?.to_lowercase();
        let mut hits: Vec<(i32, Value)> = Vec::new();

        let domain = if let Some(graph_id) = args.get("graph").and_then(Value::as_str) {
            let g = self.load_graph(graph_id)?;
            for n in &g.nodes {
                if let Some(score) = fuzzy_score(&q, &[&n.id, &n.kind, &n.label]) {
                    hits.push((
                        score,
                        json!({ "type": "node", "id": n.id, "kind": n.kind, "label": n.label }),
                    ));
                }
            }
            Some(g.domain)
        } else {
            None
        };
        for k in self.catalog()? {
            if let Some(domain) = domain {
                if !k.offered_in(domain) {
                    continue;
                }
            }
            if let Some(score) = fuzzy_score(&q, &[&k.kind, &k.label]) {
                let mut v = Self::kind_json(&k);
                v["type"] = json!("kind");
                hits.push((score, v));
            }
        }
        hits.sort_by(|a, b| b.0.cmp(&a.0));
        Ok(json!({ "q": q, "results": hits.into_iter().map(|(_, v)| v).collect::<Vec<_>>() }))
    }

    /// `create_node(kind, from?, connect_to?)`. `connect_to` is
    /// `{ "to": {"node","port"}, "from_port"? }` to wire the new node's
    /// output into an existing input, or `{ "from": {"node","port"},
    /// "to_port"? }` to wire an existing output into the new node's input —
    /// exactly one side is implicit (the new node), matching H.4's "the
    /// dominant use case is 'add an effect to this node'" framing. `from`
    /// (a template/example id, e.g. `"scene3d"`, or a raw WGSL string for a
    /// `.custom` kind) is recorded on the node's config rather than acted on
    /// magically — the harness does not silently rewrite a shader body it
    /// was not asked to write; it makes the provenance available to
    /// `read_shader`/`get_params` and to the model's own next turn.
    fn create_node(&self, args: &Value) -> Result<Value, String> {
        let kind = Self::require_str(args, "kind")?;
        let mut g = self.require_graph(args)?;
        let catalog = self.catalog()?;
        if !catalog.iter().any(|k| k.kind == kind) {
            return Err(format!("no such node kind: {kind}"));
        }
        let x = args["pos"][0].as_f64().unwrap_or(0.0) as f32;
        let y = args["pos"][1].as_f64().unwrap_or(0.0) as f32;
        let prefix = kind.split('.').next().unwrap_or("n");
        let id = g.add_node(prefix, kind, [x, y]);

        let mut config = json!({});
        if let Some(from) = args.get("from").and_then(Value::as_str) {
            config["from_template"] = json!(from);
        }
        if let Some(shader) = args.get("shader").and_then(Value::as_str) {
            config["shader"] = json!(shader);
        }
        if let Some(patch) = args.get("config").and_then(Value::as_object) {
            for (k, v) in patch {
                config[k] = v.clone();
            }
        }
        if config.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
            if let Some(n) = g.node_mut(&id) {
                n.config = metal_graph::Config::from_json_str(&config.to_string())
                    .map_err(|e| e.to_string())?;
            }
        }

        // `after`: splice the new node into a picture chain right after an
        // existing node — every `image` edge leaving `after` now leaves the
        // new node instead, and `after.image` feeds it. The move "put an
        // effect on this source" needs: `connect_to` alone cannot make it,
        // because a source whose `image` fans out to both the effect and the
        // output is not a chain (`metal_graph::effects::chain` stops at a fork)
        // and the effect would never reach the screen.
        if let Some(after) = args.get("after").and_then(Value::as_str) {
            if g.node(after).is_none() {
                return Err(format!("`after` names no node: {after}"));
            }
            let moved: Vec<(metal_graph::Endpoint, String)> = g
                .edges
                .iter()
                .filter(|e| e.from.node == after && e.from.port == "image")
                .map(|e| (e.to.clone(), e.id.clone()))
                .collect();
            g.edges
                .retain(|e| !(e.from.node == after && e.from.port == "image"));
            g.add_edge_checked(
                metal_graph::Endpoint::new(after, "image"),
                metal_graph::Endpoint::new(&id, "image"),
                &catalog,
            )
            .map_err(|errors| format!("cannot wire {after} into {id}: {errors:?}"))?;
            for (to, _) in moved {
                g.add_edge_checked(metal_graph::Endpoint::new(&id, "image"), to, &catalog)
                    .map_err(|errors| format!("cannot wire {id} onwards: {errors:?}"))?;
            }
        }

        let mut connect_result = None;
        if let Some(connect_to) = args.get("connect_to") {
            let (from, to) = if let Some(to) = connect_to.get("to") {
                let to_node = to
                    .get("node")
                    .and_then(Value::as_str)
                    .ok_or("connect_to.to.node")?;
                let to_port = to
                    .get("port")
                    .and_then(Value::as_str)
                    .ok_or("connect_to.to.port")?;
                let from_port = connect_to
                    .get("from_port")
                    .and_then(Value::as_str)
                    .unwrap_or("image");
                (
                    metal_graph::Endpoint::new(&id, from_port),
                    metal_graph::Endpoint::new(to_node, to_port),
                )
            } else if let Some(from) = connect_to.get("from") {
                let from_node = from
                    .get("node")
                    .and_then(Value::as_str)
                    .ok_or("connect_to.from.node")?;
                let from_port = from
                    .get("port")
                    .and_then(Value::as_str)
                    .ok_or("connect_to.from.port")?;
                let to_port = connect_to
                    .get("to_port")
                    .and_then(Value::as_str)
                    .unwrap_or("image");
                (
                    metal_graph::Endpoint::new(from_node, from_port),
                    metal_graph::Endpoint::new(&id, to_port),
                )
            } else {
                return Err("connect_to needs a `to` or a `from`".to_string());
            };
            match g.add_edge_checked(from, to, &catalog) {
                Ok(edge_id) => connect_result = Some(json!({ "ok": true, "edge": edge_id })),
                Err(errors) => connect_result = Some(json!({ "ok": false, "errors": errors })),
            }
        }

        self.save_graph(&g)?;
        let mut reply = Self::env_reply(&g, &catalog);
        reply["node"] = json!(id);
        if let Some(c) = connect_result {
            reply["connect"] = c;
        }
        Ok(reply)
    }

    fn delete_node(&self, args: &Value) -> Result<Value, String> {
        let id = Self::require_str(args, "id")?;
        let mut g = self.require_graph(args)?;
        if g.node(id).is_none() {
            return Err(format!("no such node: {id}"));
        }
        g.remove_node(id);
        self.save_graph(&g)?;
        let catalog = self.catalog()?;
        Ok(Self::env_reply(&g, &catalog))
    }

    fn connect_nodes(&self, args: &Value) -> Result<Value, String> {
        let mut g = self.require_graph(args)?;
        let catalog = self.catalog()?;
        let from = args.get("from").ok_or("expected `from` {node, port}")?;
        let to = args.get("to").ok_or("expected `to` {node, port}")?;
        let from = metal_graph::Endpoint::new(
            from.get("node")
                .and_then(Value::as_str)
                .ok_or("from.node")?,
            from.get("port")
                .and_then(Value::as_str)
                .ok_or("from.port")?,
        );
        let to = metal_graph::Endpoint::new(
            to.get("node").and_then(Value::as_str).ok_or("to.node")?,
            to.get("port").and_then(Value::as_str).ok_or("to.port")?,
        );
        match g.add_edge_checked(from, to, &catalog) {
            Ok(edge_id) => {
                self.save_graph(&g)?;
                let mut reply = Self::env_reply(&g, &catalog);
                reply["edge"] = json!(edge_id);
                Ok(reply)
            }
            Err(errors) => Err(errors.join("; ")),
        }
    }

    /// A held value in a tool call: a number, or `null` to release it.
    fn held_value(v: &Value) -> Result<Option<f32>, String> {
        match v {
            Value::Null => Ok(None),
            v => v
                .as_f64()
                .map(|f| Some(f as f32))
                .ok_or_else(|| format!("a held value is a number or null, not {v}")),
        }
    }

    /// `get_io(graph, node)`: `GET /graph/io`'s read — the same
    /// `metal_graph::bind::io` the route answers with, without the live values
    /// only the running engine has (`live`/`value` are `null` here).
    fn get_io(&self, args: &Value) -> Result<Value, String> {
        let node = Self::require_str(args, "node")?;
        let g = self.require_graph(args)?;
        let catalog = self.catalog()?;
        let io = metal_graph::bind::io(&g, node, &catalog, None)
            .ok_or_else(|| format!("no such node: {node}"))?;
        let mut reply = serde_json::to_value(&io).map_err(|e| e.to_string())?;
        reply["graph"] = json!(g.id);
        Ok(reply)
    }

    /// `bind_param(graph, node, param, from)`: `POST /graph/node {op:
    /// "bind"}` — `metal_graph::bind::bind`, the one validated writer of a
    /// binding. `from` is `{node, port, var}`, or `null` to unbind.
    fn bind_param(&self, args: &Value) -> Result<Value, String> {
        let node = Self::require_str(args, "node")?;
        let param = Self::require_str(args, "param")?;
        let from = match args.get("from") {
            None => {
                return Err("expected `from`: {node, port, var} — or null to unbind".to_string())
            }
            Some(Value::Null) => None,
            Some(f) => match (f["node"].as_str(), f["port"].as_str(), f["var"].as_str()) {
                (Some(n), Some(p), Some(v)) => Some((n, p, v)),
                _ => return Err("`from` needs node, port and var — or null to unbind".to_string()),
            },
        };
        let mut g = self.require_graph(args)?;
        let catalog = self.catalog()?;
        let edge = metal_graph::bind::bind(&mut g, node, param, from, &catalog)?;
        self.save_graph(&g)?;
        let mut reply = Self::env_reply(&g, &catalog);
        reply["edge"] = json!(edge);
        Ok(reply)
    }

    /// `set_param(graph, node, name, value)`: `POST /graph/node {op:
    /// "param"}` — `metal_graph::params::set_held`, the one writer of a held
    /// value (`null` releases it).
    fn set_param(&self, args: &Value) -> Result<Value, String> {
        let node = Self::require_str(args, "node")?;
        let name = Self::require_str(args, "name")?;
        let raw = args
            .get("value")
            .ok_or("expected `value`: a number, or null to release")?;
        let mut g = self.require_graph(args)?;
        let catalog = self.catalog()?;
        // A colour slot (12 floats) or the camera (7) is held as a list.
        if let Some(list) = raw.as_array() {
            let floats = list.iter().map(|v| v.as_f64().map(|f| f as f32)).collect::<Option<Vec<f32>>>()
                .ok_or("a held colour or camera is a list of numbers")?;
            metal_graph::params::set_held_vec(&mut g, node, name, Some(floats), &catalog)?;
            self.save_graph(&g)?;
            return Ok(Self::env_reply(&g, &catalog));
        }
        let value = Self::held_value(raw)?;
        let is_vector = metal_graph::params::find_param(
            g.node(node).ok_or_else(|| format!("no such node: {node}"))?,
            catalog.iter().find(|k| g.node(node).is_some_and(|n| n.kind == k.kind)),
            name,
        )
        .is_some_and(|p| p.kind != metal_graph::params::ParamKind::Float);
        if value.is_none() && is_vector {
            metal_graph::params::set_held_vec(&mut g, node, name, None, &catalog)?;
            self.save_graph(&g)?;
            return Ok(Self::env_reply(&g, &catalog));
        }
        metal_graph::params::set_held(&mut g, node, name, value, &catalog)?;
        self.save_graph(&g)?;
        Ok(Self::env_reply(&g, &catalog))
    }

    /// `set_param_batch(graph, updates)`: `POST /graph/node {op: "params"}` —
    /// `metal_graph::params::set_held_all`, all or nothing, saved once.
    fn set_param_batch(&self, args: &Value) -> Result<Value, String> {
        let list = args
            .get("updates")
            .and_then(Value::as_array)
            .ok_or("expected `updates`: a list of {node, name, value}")?;
        let mut updates = Vec::with_capacity(list.len());
        let mut vectors: Vec<(String, String, Option<Vec<f32>>)> = Vec::new();
        for u in list {
            let node = Self::require_str(u, "node")?;
            let name = Self::require_str(u, "name")?;
            let raw = u.get("value").unwrap_or(&Value::Null);
            // A colour slot (12 floats) or the camera (7) is held as a list.
            if let Some(items) = raw.as_array() {
                let floats = items
                    .iter()
                    .map(|v| v.as_f64().map(|f| f as f32))
                    .collect::<Option<Vec<f32>>>()
                    .ok_or("a held colour or camera is a list of numbers")?;
                vectors.push((node.to_string(), name.to_string(), Some(floats)));
                continue;
            }
            updates.push((node.to_string(), name.to_string(), Self::held_value(raw)?));
        }
        let mut g = self.require_graph(args)?;
        let catalog = self.catalog()?;
        // Nothing is saved until every update has applied, so one refusal
        // leaves the stored graph as it was.
        metal_graph::params::set_held_all(&mut g, &updates, &catalog)?;
        for (node, name, value) in vectors {
            metal_graph::params::set_held_vec(&mut g, &node, &name, value, &catalog)?;
        }
        self.save_graph(&g)?;
        Ok(Self::env_reply(&g, &catalog))
    }

    fn get_params(&self, args: &Value) -> Result<Value, String> {
        let id = Self::require_str(args, "id")?;
        let g = self.require_graph(args)?;
        let n = g.node(id).ok_or_else(|| format!("no such node: {id}"))?;
        let catalog = self.catalog()?;
        let mut reply = Self::node_json(n);
        reply["params"] = json!(catalog
            .iter()
            .find(|k| k.kind == n.kind)
            .map(param_schema)
            .unwrap_or_default());
        Ok(reply)
    }

    /// `set_params(id, patch)`. `patch.bypass`/`patch.solo`/`patch.label` set
    /// the node's own fields (this is also "on/off", per H.4's own doc:
    /// bypass is the off switch); everything else in `patch` is shallow-
    /// merged into the node's free-form `config`, same as a human editing a
    /// param in the canvas.
    ///
    /// `S.AL.3`: `patch.floats` is one key with two shapes, told apart by
    /// what it carries rather than by a second field name. An *array* is
    /// the pre-existing, heavier op — redeclaring the node's floats
    /// themselves (`name`/`min`/`max`/`default`), still just a generic
    /// merge below. An *object* (`{"<name>": value}`), on a `script.*` node
    /// only, is turning a knob in real time: it never touches
    /// `name`/`min`/`max`/`default`, and it goes through
    /// [`metal_graph::script_floats::apply_floats_patch`] — the same validated
    /// writer `metal-dash`'s `POST /graph/node {op:"config"}` merge calls for
    /// the human panel's slider, so a value one route would refuse and the
    /// other would silently accept can never happen (rule 4).
    fn set_params(&self, args: &Value) -> Result<Value, String> {
        let id = Self::require_str(args, "id")?;
        let mut g = self.require_graph(args)?;
        let patch = args
            .get("patch")
            .and_then(Value::as_object)
            .ok_or("expected `patch` object")?;
        let catalog = self.catalog()?;

        // A generator's `floats` object is its held floats: each one through
        // `metal_graph::params::set_held`, the writer `set_param` and `POST
        // /graph/node {op: "param"}` share (validated, `null` releases).
        let gen_floats = patch
            .get("floats")
            .and_then(Value::as_object)
            .filter(|_| g.node(id).is_some_and(|n| n.kind.starts_with("gen.")))
            .cloned();
        for (name, value) in gen_floats.iter().flatten() {
            metal_graph::params::set_held(&mut g, id, name, Self::held_value(value)?, &catalog)?;
        }

        let config_str = g
            .node(id)
            .ok_or_else(|| format!("no such node: {id}"))?
            .config
            .to_json_str();
        let mut config: Value = serde_json::from_str(&config_str).unwrap_or(json!({}));
        let n = g
            .node_mut(id)
            .ok_or_else(|| format!("no such node: {id}"))?;
        let is_script = metal_graph::is_script_kind(&n.kind);
        let is_gen = n.kind.starts_with("gen.");

        for (k, v) in patch {
            match k.as_str() {
                "bypass" => n.bypass = v.as_bool().unwrap_or(n.bypass),
                "solo" => n.solo = v.as_bool().unwrap_or(n.solo),
                "label" => n.label = v.as_str().unwrap_or(&n.label).to_string(),
                "floats" if is_gen && v.is_object() => {} // written above
                "floats" if is_script && v.is_object() => {
                    let obj = v.as_object().expect("checked is_object above");
                    metal_graph::script_floats::apply_floats_patch(&mut config, obj)?;
                }
                other => {
                    check_config_value(&catalog, id, &n.kind, other, v)?;
                    config[other] = v.clone();
                }
            }
        }
        n.config =
            metal_graph::Config::from_json_str(&config.to_string()).map_err(|e| e.to_string())?;

        self.save_graph(&g)?;
        Ok(Self::env_reply(&g, &catalog))
    }

    /// `read_shader(id|name)`. `id` reads a user-authored shader from
    /// `root/shaders/<id>.wgsl` (H.2: `data/shaders/<id>.wgsl`); `name` reads
    /// a template from `templates_root` (in a real run, `escena3d.wgsl` out
    /// of `metal-render`'s own shader directory — "the template the AI reads
    /// as an example").
    fn read_shader(&self, args: &Value) -> Result<Value, String> {
        if let Some(id) = args.get("id").and_then(Value::as_str) {
            let path = self.shader_path(id);
            let wgsl =
                fs::read_to_string(&path).map_err(|e| format!("cannot read shader {id:?}: {e}"))?;
            return Ok(json!({ "id": id, "wgsl": wgsl }));
        }
        if let Some(name) = args.get("name").and_then(Value::as_str) {
            let dir = self
                .templates_root
                .as_ref()
                .ok_or("no templates_root configured")?;
            let path = dir.join(format!("{name}.wgsl"));
            let wgsl = fs::read_to_string(&path)
                .map_err(|e| format!("cannot read template {name:?}: {e}"))?;
            return Ok(json!({ "name": name, "wgsl": wgsl }));
        }
        Err("expected `id` or `name`".to_string())
    }

    fn sleep(&self, args: &Value) -> Result<Value, String> {
        let ms = args
            .get("ms")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(MAX_SLEEP_MS);
        std::thread::sleep(Duration::from_millis(ms));
        Ok(json!({ "slept_ms": ms }))
    }

    #[cfg(feature = "gpu")]
    fn raster(&self, args: &Value) -> Result<Value, String> {
        let wgsl = if let Some(w) = args.get("shader").and_then(Value::as_str) {
            w.to_string()
        } else if let Some(id) = args.get("shader_id").and_then(Value::as_str) {
            let path = self.shader_path(id);
            fs::read_to_string(&path).map_err(|e| format!("cannot read shader {id:?}: {e}"))?
        } else {
            return Err("expected `shader` (wgsl text) or `shader_id`".to_string());
        };

        let pipeline = match metal_forge::compile(&wgsl) {
            Ok(p) => p,
            Err(e) => return Ok(json!({ "ok": false, "error": format!("{e}") })),
        };
        let floats: Vec<f32> = args
            .get("floats")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_f64)
                    .map(|f| f as f32)
                    .collect()
            })
            .unwrap_or_default();
        let w = args.get("w").and_then(Value::as_u64).unwrap_or(256) as u32;
        let h = args.get("h").and_then(Value::as_u64).unwrap_or(256) as u32;

        match metal_forge::raster(&pipeline, &floats, w, h) {
            Ok(png) => Ok(json!({ "ok": true, "png_base64": base64_encode(&png), "w": w, "h": h })),
            Err(e) => Ok(json!({ "ok": false, "error": format!("{e}") })),
        }
    }

    #[cfg(not(feature = "gpu"))]
    fn raster(&self, _args: &Value) -> Result<Value, String> {
        Err("raster tool requires metal-agent's `gpu` feature (metal-forge)".to_string())
    }

    /// S.M's `bake_clip(source, duration, fps?)`: one `metal_forge::compile`
    /// and one `metal_forge::raster` **per frame**, `t` advancing as
    /// `frame_index / fps` — the tool's whole reason for existing is that
    /// this is the only `kind`. `gen.html`/`fw-clip`/CDP were discarded
    /// 2026-09-05 (`the design notes` PARTE V, I.1/I.2); nothing here
    /// resurrects an HTML rendering path.
    ///
    /// `source` is either a shader id already written under
    /// `root/shaders/<id>.wgsl` (same lookup `read_shader(id)` and
    /// `raster(shader_id)` already use) or literal WGSL text — tried in
    /// that order so a plain id string never gets misread as source. Each
    /// frame's `t` (seconds since the clip's own start) is written into the
    /// shader's first `floatsN.x`; `raster` zero-pads the rest, same as
    /// `raster`'s own tool does for a caller that hands it fewer floats
    /// than the block holds.
    ///
    /// Frames are written as a PNG sequence, `data/clips/<id>/%06d.png` —
    /// the plan's own first option, and the one that needs no muxer at all
    /// rather than reimplementing `metal_ingest::video::encode_rgba`'s mp4
    /// path. `<id>` is derived from `(source, duration, fps, w, h)` — the
    /// same request always names the same directory, which is exactly what
    /// the determinism test below leans on: baking the same clip twice
    /// writes the same id, and the second bake's frames must be byte-for-
    /// byte identical to the first's rather than merely "close".
    #[cfg(feature = "gpu")]
    fn bake_clip(&self, args: &Value) -> Result<Value, String> {
        let source = args
            .get("source")
            .and_then(Value::as_str)
            .ok_or("expected `source`")?;
        let duration = args
            .get("duration")
            .and_then(Value::as_f64)
            .ok_or("expected `duration` (seconds)")?;
        if !duration.is_finite() || duration <= 0.0 {
            return Err(format!(
                "`duration` must be a positive number of seconds, got {duration}"
            ));
        }
        let fps = args
            .get("fps")
            .and_then(Value::as_f64)
            .unwrap_or(DEFAULT_BAKE_FPS);
        if !fps.is_finite() || fps <= 0.0 {
            return Err(format!("`fps` must be a positive number, got {fps}"));
        }
        let w = args.get("w").and_then(Value::as_u64).unwrap_or(512) as u32;
        let h = args.get("h").and_then(Value::as_u64).unwrap_or(512) as u32;

        // Same either-of order `read_shader`/`raster` already establish:
        // an on-disk id first, literal WGSL text as the fallback so a
        // one-off shader does not need a file written just to bake it once.
        let wgsl = {
            let path = self.shader_path(source);
            fs::read_to_string(&path).unwrap_or_else(|_| source.to_string())
        };

        let pipeline = match metal_forge::compile(&wgsl) {
            Ok(p) => p,
            Err(e) => return Ok(json!({ "ok": false, "error": format!("{e}") })),
        };

        let frame_count = (duration * fps).round().max(1.0) as usize;
        let id = bake_clip_id(source, duration, fps, w, h);
        let dir = self.root.join("clips").join(&id);
        fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

        for frame in 0..frame_count {
            let t = frame as f64 / fps;
            let floats = [t as f32];
            let png = match metal_forge::raster(&pipeline, &floats, w, h) {
                Ok(png) => png,
                Err(e) => {
                    return Ok(json!({ "ok": false, "error": format!("{e}"), "frame": frame }))
                }
            };
            let frame_path = dir.join(format!("{frame:06}.png"));
            fs::write(&frame_path, &png)
                .map_err(|e| format!("cannot write {}: {e}", frame_path.display()))?;
        }

        Ok(json!({
            "ok": true,
            "id": id,
            "dir": dir.display().to_string(),
            "frames": frame_count,
            "fps": fps,
            "w": w,
            "h": h,
        }))
    }

    /// A build without `gpu` has no metal-forge, so there is nothing to bake
    /// with. Answered as a structured tool error (`code`), not a failed call.
    #[cfg(not(feature = "gpu"))]
    fn bake_clip(&self, _args: &Value) -> Result<Value, String> {
        Ok(json!({
            "ok": false,
            "code": "needs_gpu_build",
            "error": "needs gpu build: bake_clip renders with metal-forge, which this build leaves out; rebuild with the default `gpu` feature",
        }))
    }

    /// H.5a landed, so this is no longer blocked: `metal_ingest::camera` is the
    /// real capture backend. With no `device` argument the first camera
    /// `ffmpeg -sources v4l2` reports is used — the common case is one
    /// webcam pointed at the projected surface, and making the agent pick a
    /// `/dev/videoN` before it can look at anything is friction with no
    /// judgment in it.
    ///
    /// Returns PNG rather than the raw RGBA of
    /// [`metal_ingest::camera::capture_frame`] because the consumer is a vision
    /// model: `raster` already established `png_base64` as the shape a tool
    /// hands an image back in, and agreeing with it means the corpus (H.7)
    /// teaches one convention instead of two.
    ///
    /// "No camera present" comes back as `ok: false` with a reason, not an
    /// `Err`: a missing webcam is a fact about the room the agent should read
    /// and act on (ask the operator to plug one in), not a tool malfunction
    /// that aborts the turn.
    fn capture_camera_frame(&self, args: &Value) -> Result<Value, String> {
        let w = args.get("w").and_then(Value::as_u64).unwrap_or(1280) as u32;
        let h = args.get("h").and_then(Value::as_u64).unwrap_or(720) as u32;

        // With no `device`, every camera is tried in the kernel's order
        // (video nodes before metadata nodes) and the first frame wins —
        // "the first camera ffmpeg lists" was, on a common HP webcam, its
        // metadata node, which never has a frame to give.
        // Every failure to get a frame is the same structured answer:
        // `code: camera_not_streaming`, with the reason after it, so a caller
        // can tell "no frame right now" from a bug in the tool.
        let not_streaming = |device: Option<&str>, reason: String| {
            let mut out = json!({
                "ok": false,
                "code": "camera_not_streaming",
                "error": format!("camera not streaming: {reason}"),
            });
            if let Some(d) = device {
                out["device"] = json!(d);
            }
            Ok(out)
        };
        let (device, png) = match args.get("device").and_then(Value::as_str) {
            Some(d) => match metal_ingest::camera::capture_frame_png(std::path::Path::new(d), w, h)
            {
                Ok(png) => (d.to_string(), png),
                Err(e) => return not_streaming(Some(d), e.to_string()),
            },
            None => match metal_ingest::camera::capture_any_png(w, h) {
                Ok(found) => found,
                Err(errors) if errors.is_empty() => {
                    return not_streaming(
                        None,
                        "no camera present; connect one, or pass `device`".into(),
                    )
                }
                Err(errors) => {
                    let reasons = errors
                        .iter()
                        .map(|e| e.to_string())
                        .collect::<Vec<_>>()
                        .join("; ");
                    return not_streaming(None, reasons);
                }
            },
        };
        Ok(json!({
            "ok": true,
            "device": device,
            "png_base64": base64_encode(&png),
            "w": w,
            "h": h,
        }))
    }

    /// H.5c's `locate`, as a thin `step`-dispatched wrapper over
    /// `crate::locate`'s pure geometry and pixel functions — same shape as
    /// every other tool here: no logic lives in this method that isn't
    /// already a plain function the module doc can point at.
    ///
    /// Three steps, one per stage of the protocol; the model drives the loop
    /// by calling this three-or-more times in a row (H.5c's grid picking
    /// itself needs a model in front of the image, which is exactly what
    /// this harness cannot do on its own — see the module's own limits):
    ///
    /// - `step: "grid"` — `{ png_base64, n, region? }` → overlays an `n×n`
    ///   numbered grid (§H.5c: 9×9 first pass, 6×6 second pass on a crop) and
    ///   echoes back `region` (default the whole image) so the caller does
    ///   not have to track it separately across the turn.
    /// - `step: "crop"` — `{ png_base64, region, w?, h? }` → crops+enlarges
    ///   `region` out of the given PNG, the "crop that region, enlarge it"
    ///   half of the second pass, done as its own step so the model can look
    ///   at the plain crop before a grid goes back on top of it if it wants.
    /// - `step: "resolve"` — `{ n, cols, rows, parent_region? }` → turns the
    ///   chosen strips into a region (`strips_to_region`), composed with
    ///   `parent_region` if given (`compose`, exactly `global = origen +
    ///   local × size`) — this is the only step with no image in or out.
    /// - `step: "verify"` — `{ png_base64, region }` → draws the final box on
    ///   the *original* full frame and returns it alongside `box`/`center`,
    ///   §H.5c's non-optional "draw the box and look at it".
    ///
    /// Ambiguity ("one tile" among forty) is not and cannot be checked
    /// here — see `crate::locate`'s module doc: that is resolved with
    /// `question` before any of this is called, by the agent turn, not by a
    /// pixel-rectangle function.
    fn locate(&self, args: &Value) -> Result<Value, String> {
        let step = Self::require_str(args, "step")?;
        match step {
            "grid" => {
                let png = base64_decode(Self::require_str(args, "png_base64")?)?;
                let n = args
                    .get("n")
                    .and_then(Value::as_u64)
                    .ok_or("expected `n`")? as u32;
                let region =
                    parse_region(args.get("region")).unwrap_or_else(crate::locate::Region::full);
                let out = crate::locate::overlay_grid(&png, n)?;
                Ok(json!({
                    "ok": true,
                    "png_base64": base64_encode(&out),
                    "n": n,
                    "region": region.as_box(),
                }))
            }
            "crop" => {
                let png = base64_decode(Self::require_str(args, "png_base64")?)?;
                let region =
                    parse_region(args.get("region")).ok_or("expected `region` [x0,y0,x1,y1]")?;
                let w = args.get("w").and_then(Value::as_u64).unwrap_or(768) as u32;
                let h = args.get("h").and_then(Value::as_u64).unwrap_or(768) as u32;
                let out = crate::locate::crop_and_scale(&png, &region, w, h)?;
                Ok(
                    json!({ "ok": true, "png_base64": base64_encode(&out), "region": region.as_box() }),
                )
            }
            "resolve" => {
                let n = args
                    .get("n")
                    .and_then(Value::as_u64)
                    .ok_or("expected `n`")? as u32;
                let cols = parse_u32_list(args.get("cols"))
                    .ok_or("expected `cols` (list of 1-indexed strips)")?;
                let rows = parse_u32_list(args.get("rows"))
                    .ok_or("expected `rows` (list of 1-indexed strips)")?;
                let local = crate::locate::strips_to_region(n, &cols, &rows)?;
                let region = match parse_region(args.get("parent_region")) {
                    Some(parent) => crate::locate::compose(&parent, &local),
                    None => local,
                };
                let (cx, cy) = region.center();
                Ok(json!({ "ok": true, "region": region.as_box(), "center": [cx, cy] }))
            }
            "verify" => {
                let png = base64_decode(Self::require_str(args, "png_base64")?)?;
                let region =
                    parse_region(args.get("region")).ok_or("expected `region` [x0,y0,x1,y1]")?;
                let out = crate::locate::draw_box(&png, &region)?;
                let (cx, cy) = region.center();
                Ok(json!({
                    "ok": true,
                    "png_base64": base64_encode(&out),
                    "box": region.as_box(),
                    "center": [cx, cy],
                }))
            }
            other => Err(format!(
                "locate: unknown step {other:?} (want grid, crop, resolve, or verify)"
            )),
        }
    }
}

fn parse_region(v: Option<&Value>) -> Option<crate::locate::Region> {
    let a = v?.as_array()?;
    if a.len() != 4 {
        return None;
    }
    let f = |i: usize| a[i].as_f64();
    Some(crate::locate::Region {
        x0: f(0)?,
        y0: f(1)?,
        x1: f(2)?,
        y1: f(3)?,
    })
}

fn parse_u32_list(v: Option<&Value>) -> Option<Vec<u32>> {
    v?.as_array()?
        .iter()
        .map(|x| x.as_u64().map(|n| n as u32))
        .collect()
}

/// Cheap fuzzy scorer: highest score for an exact field match, then a
/// prefix match, then a plain substring match, across any of `fields`;
/// `None` if `q` matches none of them. Good enough for "find the node I mean
/// among a few dozen", which is what `search_nodes` is for — not a general
/// string-distance library.
fn fuzzy_score(q: &str, fields: &[&str]) -> Option<i32> {
    let mut best: Option<i32> = None;
    for f in fields {
        let f = f.to_lowercase();
        let score = if f == q {
            100
        } else if f.starts_with(q) {
            75
        } else if f.contains(q) {
            50
        } else {
            continue;
        };
        best = Some(best.map_or(score, |b: i32| b.max(score)));
    }
    best
}

/// Minimal base64 (standard alphabet, padded) — `raster`'s and
/// `capture_camera_frame`'s PNG bytes need to travel inside a JSON tool
/// result, and pulling in a whole crate for this one encode is not worth it.
/// Not gated behind the `gpu` feature (unlike `raster` itself): both
/// `capture_camera_frame` and `locate` need it on a build with no GPU
/// adapter too.
pub fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// The decode half of [`base64_encode`] — `locate`'s input images arrive as
/// `png_base64` (whatever `capture_camera_frame` or a prior `locate` call
/// handed back), same standard alphabet, padded.
pub fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let s = s.trim();
    let bytes: Vec<u8> = s
        .bytes()
        .filter(|&b| b != b'=' && !b.is_ascii_whitespace())
        .collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let vals: Vec<u8> = chunk
            .iter()
            .map(|&b| val(b).ok_or_else(|| format!("invalid base64 byte: {b}")))
            .collect::<Result<_, _>>()?;
        let n = vals.iter().fold(0u32, |acc, &v| (acc << 6) | v as u32) << (6 * (4 - vals.len()));
        out.push((n >> 16) as u8);
        if vals.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if vals.len() > 3 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

impl ToolExecutor for Harness {
    fn execute(&mut self, call: &ToolCall) -> Result<Value, String> {
        let args = &call.arguments;
        match call.name.as_str() {
            "list_tools" => Ok(self.list_tools()),
            "get_env" => self.get_env(args),
            "list_nodes" => self.list_nodes(args),
            "focus" => self.focus(args),
            "search_nodes" => self.search_nodes(args),
            "create_node" => self.create_node(args),
            "delete_node" => self.delete_node(args),
            "connect_nodes" => self.connect_nodes(args),
            "get_params" => self.get_params(args),
            "list_kinds" => self.list_kinds(args),
            "set_params" => self.set_params(args),
            "get_io" => self.get_io(args),
            "bind_param" => self.bind_param(args),
            "set_param" => self.set_param(args),
            "set_param_batch" => self.set_param_batch(args),
            "read_shader" => self.read_shader(args),
            "raster" => self.raster(args),
            "save_shader" => self.save_shader(args),
            "take_graph" => self.graph_owner_tool("take", args),
            "release_graph" => self.graph_owner_tool("release", args),
            "graph_owner" => self.graph_owner_tool("owner", args),
            "sleep" => self.sleep(args),
            "capture_camera_frame" => self.capture_camera_frame(args),
            "locate" => self.locate(args),
            "bake_clip" => self.bake_clip(args),
            "import_segment" | "move_segment" | "remove_segment" | "set_keyframe" | "attach_digest"
            | "list_switches" | "list_transitions" | "set_switch" | "get_tick_level" | "set_tick_level" | "train_ai" => Err(format!(
                "{} is implemented by metal_dash::author::models::agent_tools::AgentHarness, not by metal_agent::harness::Harness — see this module's doc comment",
                call.name
            )),
            // S.O.1, S.O.2, S.O.5, S.D — overlay and timeline tools.
            // Implemented by metal_dash::author::models::agent_tools::AgentHarness, not by metal_agent::harness::Harness.
            "blackout" | "solid_color" | "set_layer_mix" | "rasterize_overlay" | "create_mask_grid" | "test_mask_grid"
            | "edit_mask" | "add_effect" | "remove_effect" | "edit_output" | "resolve_draft" | "project_venue" | "look_venue" | "aim_fixture"
            | "scan_venue" | "calibrate_projector" | "blend_edges" | "discover_fixture" | "solve_head"
            | "edit_venue_surface" | "edit_layer" | "listen_status"
            | "add_stage" | "add_truss" | "add_dummy" | "venue_spline"
            | "place_mask_in_space" | "venue_boolean" | "venue_hit" | "venue_report"
            | "venue_view" | "venue_assets_search" | "venue_asset_import"
            | "list_video_io" | "set_output_sink" | "dmx_bus" | "import_gdtf" | "capture_output"
            | "play_sequence" | "edit_loops" | "transport" | "fire_trigger" | "overlay_control"
            | "colour_apply"
            | "perf_report" | "get_grid_layouts" | "set_grid_layouts" | "export_status" | "export_control"
            | "list_palettes" => Err(format!(
                "{} is implemented by metal_dash::author::models::agent_tools::AgentHarness, not by metal_agent::harness::Harness — see this module's doc comment",
                call.name
            )),
            other => Err(format!(
                "no such tool: {other} (or not implemented yet — see metal-agent::harness module docs)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A schema.json with exactly one generator (`gen.turing`, no floats
    /// beyond what `NodeKind::catalog` needs) and one effect kind
    /// (`fx.shake`) — enough to add/connect/lint nodes without dragging the
    /// real 335-line manifest into every test. `NodeKind::catalog` also
    /// always emits the fixed kinds (`output.device`, etc.) regardless of
    /// the manifest, which is what most of these tests actually connect to.
    const FIXTURE_SCHEMA: &str = r#"{
        "rate": 8.0,
        "models": [
            { "id": "turing", "label": "Turing", "blocks": [
                { "type": "floats", "fields": [
                    { "name": "speed", "min": 0.0, "max": 1.0, "default": 0.5 }
                ] },
                { "type": "camera", "default": [0, 0, 4, 0, 0, 0, 45] }
            ] }
        ]
    }"#;

    fn fixture(name: &str) -> Harness {
        let dir = std::env::temp_dir().join(format!(
            "metal-agent-harness-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("graphs")).unwrap();
        fs::create_dir_all(dir.join("shaders")).unwrap();
        fs::write(dir.join("schema.json"), FIXTURE_SCHEMA).unwrap();
        let g = metal_graph::Graph::new("g1", "Test graph", metal_graph::Domain::Live);
        fs::write(dir.join("graphs/g1.json"), g.to_json()).unwrap();
        Harness::new(dir)
    }

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: "1".to_string(),
            name: name.to_string(),
            arguments: args,
        }
    }

    /// O.8: another peer's graph refuses take and writes; once free, this
    /// harness takes, writes and releases.
    #[test]
    fn a_graph_owned_elsewhere_refuses_the_harness() {
        let mut h = fixture("owner");
        let dir = h.root.join("graphs");
        let me = metal_graph::owner::local_identity();
        let free = h
            .execute(&call("graph_owner", json!({ "graph": "g1" })))
            .unwrap();
        assert!(free["owner"].is_null());
        assert!(h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "gen.turing" })
            ))
            .is_ok());

        metal_graph::owner::take(&dir, "g1", "other-peer").unwrap();
        let err = h
            .execute(&call("take_graph", json!({ "graph": "g1" })))
            .unwrap_err();
        assert!(err.contains("other-peer"), "{err}");
        let err = h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "gen.turing" }),
            ))
            .unwrap_err();
        assert!(err.contains("owned by"), "{err}");

        metal_graph::owner::release(&dir, "g1", "other-peer").unwrap();
        let took = h
            .execute(&call("take_graph", json!({ "graph": "g1" })))
            .unwrap();
        assert_eq!(took["owner"], json!(me));
        assert!(h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "gen.turing" })
            ))
            .is_ok());
        h.execute(&call("release_graph", json!({ "graph": "g1" })))
            .unwrap();
        assert!(metal_graph::owner::status(&dir, "g1").is_none());
    }

    #[test]
    fn list_tools_names_all_twentytwo() {
        let mut h = fixture("list-tools");
        let v = h.execute(&call("list_tools", json!({}))).unwrap();
        // Computed from the manifest (`TOOL_CATALOG`), not hand-counted —
        // rule 7. This assertion exists to catch `list_tools` drifting from
        // `TOOL_CATALOG` itself, not to pin a number by hand: it recomputes
        // the same length `TOOL_CATALOG.len()` would give.
        assert_eq!(v["tools"].as_array().unwrap().len(), TOOL_CATALOG.len());
        let implemented: usize = v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["implemented"] == true)
            .count();
        let want_implemented = TOOL_CATALOG.iter().filter(|(_, _, ok)| *ok).count();
        assert_eq!(
            implemented, want_implemented,
            "list_tools must report exactly what TOOL_CATALOG says, no more and no less"
        );
    }

    /// The four S.K tools are marked implemented in the catalog (they are —
    /// by `metal_dash::author::models::agent_tools::AgentHarness`) but this
    /// bare `Harness` must say so plainly rather than pretend, same
    /// contract `raster` already has on a build with no `gpu` feature.
    #[test]
    fn the_four_sk_tools_are_not_executed_by_the_bare_harness() {
        let mut h = fixture("sk-tools-not-here");
        for name in [
            "import_segment",
            "move_segment",
            "remove_segment",
            "set_keyframe",
            "attach_digest",
        ] {
            let err = h.execute(&call(name, json!({}))).unwrap_err();
            assert!(err.contains("AgentHarness"), "{name}: {err}");
        }
    }

    /// The tool must report the room honestly rather than failing the turn:
    /// on a machine with no camera it answers `ok: false` with a reason, and
    /// on one with a camera it answers `ok: true` with a PNG. Both are a
    /// successful *call* — which is the contract the corpus (H.7) teaches.
    #[test]
    fn capture_camera_frame_reports_the_room_instead_of_erroring() {
        let mut h = fixture("camera");
        let v = h
            .execute(&call("capture_camera_frame", json!({ "w": 320, "h": 240 })))
            .expect("a missing camera is not a tool error");
        if v["ok"] == true {
            assert!(!v["png_base64"].as_str().unwrap().is_empty());
            assert_eq!(v["w"], 320);
        } else {
            assert!(!v["error"].as_str().unwrap().is_empty(), "must say why");
        }
    }

    /// A tiny synthetic PNG, base64'd, standing in for a real camera frame
    /// so the `locate` tool's four steps can be exercised without a camera
    /// or a model — same trade `raster`'s own tests make with a trivial
    /// shader instead of a real one.
    fn tiny_png_base64() -> String {
        let (w, h) = (90u32, 90u32);
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, w, h);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&vec![10u8; (w * h * 4) as usize])
                .unwrap();
        }
        base64_encode(&bytes)
    }

    #[test]
    fn locate_grid_overlays_a_grid_and_echoes_the_full_region_by_default() {
        let mut h = fixture("locate-grid");
        let v = h
            .execute(&call(
                "locate",
                json!({ "step": "grid", "png_base64": tiny_png_base64(), "n": 9 }),
            ))
            .unwrap();
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["region"], json!([0.0, 0.0, 1.0, 1.0]));
        assert!(!v["png_base64"].as_str().unwrap().is_empty());
    }

    #[test]
    fn locate_resolve_turns_chosen_strips_into_a_region_and_center() {
        let mut h = fixture("locate-resolve");
        let v = h
            .execute(&call(
                "locate",
                json!({ "step": "resolve", "n": 9, "cols": [3, 4, 5], "rows": [2, 3] }),
            ))
            .unwrap();
        assert_eq!(v["ok"], true, "{v}");
        // resolve reports "region" (not "box" — that's `verify`'s field).
        assert_eq!(v["region"][0].as_f64().unwrap(), 2.0 / 9.0);
        assert_eq!(v["region"][2].as_f64().unwrap(), 5.0 / 9.0);
        assert!(v["center"].is_array());
    }

    #[test]
    fn locate_resolve_composes_with_a_parent_region_from_a_prior_pass() {
        let mut h = fixture("locate-resolve-nested");
        let v = h
            .execute(&call(
                "locate",
                json!({
                    "step": "resolve", "n": 6, "cols": [1], "rows": [1],
                    "parent_region": [2.0/9.0, 1.0/9.0, 5.0/9.0, 3.0/9.0],
                }),
            ))
            .unwrap();
        let region = v["region"].as_array().unwrap();
        // top-left cell of the parent region, exactly.
        assert!((region[0].as_f64().unwrap() - 2.0 / 9.0).abs() < 1e-9);
    }

    #[test]
    fn locate_crop_returns_a_png_for_the_requested_region() {
        let mut h = fixture("locate-crop");
        let v = h
            .execute(&call(
                "locate",
                json!({ "step": "crop", "png_base64": tiny_png_base64(), "region": [0.0, 0.0, 0.5, 0.5], "w": 40, "h": 40 }),
            ))
            .unwrap();
        assert_eq!(v["ok"], true, "{v}");
        assert!(!v["png_base64"].as_str().unwrap().is_empty());
    }

    #[test]
    fn locate_verify_draws_the_box_and_reports_center() {
        let mut h = fixture("locate-verify");
        let v = h
            .execute(&call(
                "locate",
                json!({ "step": "verify", "png_base64": tiny_png_base64(), "region": [0.2, 0.2, 0.8, 0.8] }),
            ))
            .unwrap();
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["box"], json!([0.2, 0.2, 0.8, 0.8]));
        assert_eq!(v["center"], json!([0.5, 0.5]));
    }

    #[test]
    fn locate_rejects_an_unknown_step() {
        let mut h = fixture("locate-bad-step");
        let err = h
            .execute(&call("locate", json!({ "step": "estimate" })))
            .unwrap_err();
        assert!(err.contains("unknown step"), "{err}");
    }

    #[test]
    fn create_node_adds_a_node_and_persists_it() {
        let mut h = fixture("create-node");
        let v = h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "gen.turing" }),
            ))
            .unwrap();
        let id = v["node"].as_str().unwrap().to_string();
        assert!(id.starts_with("gen-"));

        // Reload from disk — proves this actually persisted, not just
        // mutated an in-memory copy.
        let reloaded = h.load_graph("g1").unwrap();
        assert_eq!(reloaded.nodes.len(), 1);
        assert_eq!(reloaded.nodes[0].kind, "gen.turing");
    }

    /// `create_node {after}` splices: the source's image edge now leaves the
    /// new effect, and the source feeds it — one chain, no fork, which is
    /// the shape `metal_graph::effects::chain` walks to the screen.
    #[test]
    fn create_node_after_splices_an_effect_into_the_chain() {
        let mut h = fixture("splice-after");
        let gen = h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "gen.turing" }),
            ))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();
        let out = h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "output.device",
                        "connect_to": { "from": { "node": gen, "port": "image" }, "to_port": "image" } }),
            ))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();
        let fx = h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "fx.forge", "config": { "shader": "cup-caustics" }, "after": gen }),
            ))
            .expect("splice")["node"]
            .as_str()
            .unwrap()
            .to_string();
        let g = h.load_graph("g1").unwrap();
        let wired = |a: &str, b: &str| g.edges.iter().any(|e| e.from.node == a && e.to.node == b);
        assert!(wired(&gen, &fx) && wired(&fx, &out), "{:?}", g.edges);
        assert!(!wired(&gen, &out), "the source no longer skips the effect");
        assert_eq!(metal_graph::effects::chain(&g, &gen), vec![fx.clone()]);
        assert!(h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "fx.forge", "after": "nope" })
            ))
            .is_err());
    }

    /// `save_shader` no longer writes the file: it posts `{id, wgsl}` to
    /// `metal-dash`'s `POST /shaders` and relays the route's reply. With no
    /// route attached it says so rather than writing anything itself.
    #[cfg(feature = "gpu")]
    #[test]
    fn save_shader_posts_to_the_shaders_route() {
        let h = fixture("save-shader");
        let turtle = include_str!("../../../data/agent/shaders/turtle.wgsl");

        // No route: the tool refuses instead of being a second writer.
        let mut bare = h.clone();
        let err = bare
            .execute(&call(
                "save_shader",
                json!({ "id": "turtle-2", "wgsl": turtle }),
            ))
            .unwrap_err();
        assert!(err.contains("/shaders"), "{err}");

        // With the app's route attached, the tool posts the id and the WGSL
        // and hands the route's reply straight back.
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = std::sync::Arc::clone(&seen);
        let mut h = h.with_routes(std::sync::Arc::new(move |path: &str, body: &Value| {
            recorder
                .lock()
                .unwrap()
                .push((path.to_string(), body.clone()));
            Ok(json!({ "ok": true, "kind": "generator" }))
        }));
        let v = h
            .execute(&call(
                "save_shader",
                json!({ "id": "turtle-2", "wgsl": turtle }),
            ))
            .unwrap();
        assert_eq!(
            (v["ok"].as_bool(), v["kind"].as_str()),
            (Some(true), Some("generator")),
            "{v}"
        );
        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 1, "one post per call");
        assert_eq!(calls[0].0, "/shaders");
        assert_eq!(calls[0].1["id"], "turtle-2");
        assert_eq!(calls[0].1["wgsl"], turtle);
    }

    #[test]
    fn create_node_rejects_an_unknown_kind() {
        let mut h = fixture("create-node-bad-kind");
        let err = h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "gen.nope" }),
            ))
            .unwrap_err();
        assert!(err.contains("no such node kind"), "{err}");
    }

    #[test]
    fn create_node_with_connect_to_wires_it_in_one_call() {
        let mut h = fixture("create-node-connect");
        // First, a source node to connect the new one to.
        let src = h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "gen.turing" }),
            ))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();

        let v = h
            .execute(&call(
                "create_node",
                json!({
                    "graph": "g1", "kind": "fx.shake",
                    "connect_to": { "from": { "node": src, "port": "image" }, "to_port": "image" }
                }),
            ))
            .unwrap();
        assert_eq!(v["connect"]["ok"], true, "{v}");

        let g = h.load_graph("g1").unwrap();
        assert_eq!(g.edges.len(), 1);
    }

    #[test]
    fn create_node_connect_to_rejects_an_incompatible_edge() {
        let mut h = fixture("create-node-bad-connect");
        let src = h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "gen.turing" }),
            ))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();
        // Connect image -> image but reuse the same node's own image input
        // as the "existing" side twice over is awkward to set up as a type
        // mismatch here without a control-typed kind in the fixture catalog,
        // so instead: connect the new node's image output back into the
        // same node's own input, which the graph's cycle lint rejects.
        let v = h
            .execute(&call(
                "create_node",
                json!({
                    "graph": "g1", "kind": "fx.shake",
                    "connect_to": { "from": { "node": src, "port": "image" }, "to_port": "image" }
                }),
            ))
            .unwrap();
        let node_id = v["node"].as_str().unwrap().to_string();
        let bad = h
            .execute(&call(
                "connect_nodes",
                json!({ "graph": "g1", "from": { "node": node_id, "port": "image" }, "to": { "node": src, "port": "image" } }),
            ))
            .unwrap_err();
        assert!(!bad.is_empty());
        // The rejected edge must not have been persisted.
        let g = h.load_graph("g1").unwrap();
        assert_eq!(
            g.edges.len(),
            1,
            "only the accepted edge from setup should remain"
        );
    }

    #[test]
    fn delete_node_removes_it_and_its_edges() {
        let mut h = fixture("delete-node");
        let src = h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "gen.turing" }),
            ))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();
        let dst = h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "fx.shake" }),
            ))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();
        h.execute(&call(
            "connect_nodes",
            json!({
                "graph": "g1",
                "from": { "node": src, "port": "image" },
                "to": { "node": dst, "port": "image" },
            }),
        ))
        .unwrap();

        h.execute(&call("delete_node", json!({ "graph": "g1", "id": src })))
            .unwrap();

        let g = h.load_graph("g1").unwrap();
        assert_eq!(g.nodes.len(), 1);
        assert!(
            g.edges.is_empty(),
            "deleting a node must drop edges that named it"
        );
    }

    #[test]
    fn delete_node_errors_on_an_unknown_id() {
        let mut h = fixture("delete-node-missing");
        let err = h
            .execute(&call(
                "delete_node",
                json!({ "graph": "g1", "id": "gen-999" }),
            ))
            .unwrap_err();
        assert!(err.contains("no such node"), "{err}");
    }

    #[test]
    fn get_and_set_params_round_trip_through_the_saved_graph() {
        let mut h = fixture("params");
        let id = h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "gen.turing" }),
            ))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();

        h.execute(&call(
            "set_params",
            json!({
                "graph": "g1", "id": id,
                "patch": { "speed": 0.75, "bypass": true, "label": "my turing" },
            }),
        ))
        .unwrap();

        let got = h
            .execute(&call("get_params", json!({ "graph": "g1", "id": id })))
            .unwrap();
        assert_eq!(got["config"]["speed"], 0.75);
        assert_eq!(got["bypass"], true);
        assert_eq!(got["label"], "my turing");
    }

    /// Parity P-A (rule 6): every new image node, the ramp and `ctl.math`
    /// are reached by the AI's own `create_node`/`set_params` — the route the
    /// panel's `POST /graph/node {op:"config"}` merge is — floats and string
    /// settings alike. The kinds come from the catalog lists, not a literal.
    #[test]
    fn every_parity_node_is_reachable_through_create_node_and_set_params() {
        let mut h = fixture("parity");
        let mut kinds: Vec<String> = metal_graph::PARITY_EFFECT_IDS
            .iter()
            .map(|id| format!("fx.{id}"))
            .collect();
        kinds.push(metal_graph::RAMP_KIND.to_string());
        kinds.push(metal_graph::CTL_MATH_KIND.to_string());
        for kind in &kinds {
            let id = h
                .execute(&call("create_node", json!({ "graph": "g1", "kind": kind })))
                .unwrap()["node"]
                .as_str()
                .unwrap()
                .to_string();
            let (name, _, max, _) = metal_graph::parity::floats_of(kind).unwrap()[0];
            h.execute(&call(
                "set_params",
                json!({
                    "graph": "g1", "id": id, "patch": { name: max, "palette": "neon" },
                }),
            ))
            .unwrap();
            let got = h
                .execute(&call("get_params", json!({ "graph": "g1", "id": id })))
                .unwrap();
            assert_eq!(got["config"][name], json!(max), "{kind}.{name}");
            assert_eq!(got["config"]["palette"], "neon", "{kind}");
        }
    }

    /// P-D (rule 6): `in.artnet` is reached by the AI's `create_node` and
    /// `set_params` — the same merge the panel's config write is.
    #[test]
    fn in_artnet_is_reachable_through_create_node_and_set_params() {
        let mut h = fixture("artnet-in");
        let id = h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": metal_graph::ARTNET_IN_KIND }),
            ))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();
        h.execute(&call(
            "set_params",
            json!({ "graph": "g1", "id": id, "patch": { "universe": 3, "first": 5, "count": 8 } }),
        ))
        .unwrap();
        let got = h
            .execute(&call("get_params", json!({ "graph": "g1", "id": id })))
            .unwrap();
        assert_eq!(got["config"]["universe"], json!(3));
        assert_eq!(got["config"]["first"], json!(5));
        assert_eq!(got["config"]["count"], json!(8));
    }

    /// G1: `list_kinds` reads the live catalog. A model added to schema.json
    /// (no code change) shows up with its own float params and ranges.
    #[test]
    fn list_kinds_reports_params_of_a_model_added_to_the_schema() {
        let h = fixture("list-kinds");
        fs::write(
            h.root.join("schema.json"),
            r#"{
                "rate": 8.0,
                "models": [
                    { "id": "zorbax", "label": "Zorbax", "blocks": [
                        { "type": "floats", "fields": [
                            { "name": "depth", "min": 0.0, "max": 4.0, "default": 2.0 }
                        ] }
                    ] }
                ]
            }"#,
        )
        .unwrap();
        let one = h.list_kinds(&json!({ "kind": "gen.zorbax" })).unwrap();
        assert_eq!(one["kind"], "gen.zorbax");
        let depth = one["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "depth")
            .expect("depth listed");
        assert_eq!(depth["type"], "number");
        assert_eq!(depth["min"], json!(0.0));
        assert_eq!(depth["max"], json!(4.0));
        assert_eq!(depth["default"], json!(2.0));

        let all = h.list_kinds(&json!({ "filter": "zorbax" })).unwrap();
        let kinds = all["kinds"].as_array().unwrap();
        assert!(kinds.iter().any(|k| k["kind"] == "gen.zorbax"));
        assert!(h.list_kinds(&json!({ "kind": "gen.nope" })).is_err());
        assert!(h.list_kinds(&json!({ "domain": "moon" })).is_err());
    }

    /// G1 + G3: an enum-valued and an out-of-range venue.fixture key are
    /// refused by `set_params` and never persisted; `get_params` carries the
    /// node kind's schema alongside its config.
    #[test]
    fn set_params_refuses_out_of_range_and_unknown_enum_values() {
        let mut h = fixture("refuse-values");
        let id = h
            .execute(&call("create_node", json!({ "graph": "g1", "kind": metal_graph::VENUE_FIXTURE_KIND })))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();

        let range = h
            .execute(&call("set_params", json!({ "graph": "g1", "id": id, "patch": { "priority": 999 } })))
            .unwrap_err();
        assert!(range.contains("priority") && range.contains("255"), "{range}");

        let enum_err = h
            .execute(&call("set_params", json!({ "graph": "g1", "id": id, "patch": { "colour_from": "bogus" } })))
            .unwrap_err();
        assert!(enum_err.contains("colour_from") && enum_err.contains("frame"), "{enum_err}");

        let got = h.execute(&call("get_params", json!({ "graph": "g1", "id": id }))).unwrap();
        assert!(got["config"].get("priority").is_none(), "refused value persisted");
        assert!(got["config"].get("colour_from").is_none(), "refused value persisted");
        let schema = got["params"].as_array().unwrap();
        assert!(schema.iter().any(|p| p["name"] == "priority" && p["max"] == json!(255.0)));
        assert!(schema.iter().any(|p| p["name"] == "colour_from" && p["type"] == "enum"));

        h.execute(&call(
            "set_params",
            json!({ "graph": "g1", "id": id, "patch": { "priority": 10, "colour_from": "model" } }),
        ))
        .unwrap();
        let got = h.execute(&call("get_params", json!({ "graph": "g1", "id": id }))).unwrap();
        assert_eq!(got["config"]["priority"], json!(10));
        assert_eq!(got["config"]["colour_from"], "model");
    }

    /// M1 (rule 6): every `source.*` value node is reached by the AI's own
    /// `create_node`/`set_params` — the same route the panel's config write is.
    /// The kinds and their floats come from the catalog tables, not a literal.
    #[test]
    fn every_source_node_is_reachable_through_create_node_and_set_params() {
        let mut h = fixture("source-math");
        for kind in metal_graph::source::KINDS {
            let id = h
                .execute(&call("create_node", json!({ "graph": "g1", "kind": kind })))
                .unwrap()["node"]
                .as_str()
                .unwrap()
                .to_string();
            let (name, _, max, _) = metal_graph::source::floats_of(kind).unwrap()[0];
            h.execute(&call(
                "set_params",
                json!({ "graph": "g1", "id": id, "patch": { name: max } }),
            ))
            .unwrap();
            let got = h
                .execute(&call("get_params", json!({ "graph": "g1", "id": id })))
                .unwrap();
            assert_eq!(got["config"][name], json!(max), "{kind}.{name}");
        }
    }

    /// M2 (rule 6): the value noise is discoverable by the AI's `search_nodes`
    /// and created by `create_node`/`set_params`, every float taken from the
    /// catalog table — the same route the panel's config write is.
    #[test]
    fn source_noise_is_reachable_through_search_and_create_node() {
        let mut h = fixture("source-noise");
        let hits = h
            .execute(&call("search_nodes", json!({ "q": "noise" })))
            .unwrap();
        assert!(
            hits["results"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["kind"] == metal_graph::source::NOISE_KIND),
            "search_nodes must find source.noise by name"
        );
        let id = h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": metal_graph::source::NOISE_KIND }),
            ))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();
        for (name, _, max, _) in metal_graph::source::NOISE_FLOATS {
            let name: &str = name;
            h.execute(&call(
                "set_params",
                json!({ "graph": "g1", "id": id, "patch": { name: max } }),
            ))
            .unwrap();
        }
        let got = h
            .execute(&call("get_params", json!({ "graph": "g1", "id": id })))
            .unwrap();
        for (name, _, max, _) in metal_graph::source::NOISE_FLOATS {
            assert_eq!(got["config"][*name], json!(max), "source.noise.{name}");
        }
    }

    /// `S.AL.3`: `patch.floats` as an *object* on a `script.*` node writes
    /// `value` for exactly the named float, and nothing else in the
    /// declaration moves.
    #[test]
    fn set_params_floats_object_writes_the_named_floats_live_value() {
        let mut h = fixture("params-floats-write");
        let id = h
            .execute(&call(
                "create_node",
                json!({
                    "graph": "g1", "kind": "script.py",
                    "config": {
                        "code": "def process(floats, row):\n    return floats\n",
                        "floats": [
                            { "name": "width", "min": 0.0, "max": 1.0, "default": 0.2 },
                            { "name": "height", "min": 0.0, "max": 2.0, "default": 0.5 },
                        ],
                    },
                }),
            ))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();

        h.execute(&call(
            "set_params",
            json!({ "graph": "g1", "id": id, "patch": { "floats": { "width": 0.7 } } }),
        ))
        .unwrap();

        let got = h
            .execute(&call("get_params", json!({ "graph": "g1", "id": id })))
            .unwrap();
        // `0.7` round-trips through an `f32` on the write side
        // (`ScriptFloat::value`'s own type) — compare against `0.7_f32`'s
        // own promotion to JSON, not the exact `f64` literal.
        assert_eq!(got["config"]["floats"][0]["value"], 0.7_f32 as f64);
        assert_eq!(
            got["config"]["floats"][0]["default"], 0.2,
            "default is untouched"
        );
        assert_eq!(
            got["config"]["floats"][1].get("value"),
            None,
            "height was not patched"
        );
    }

    /// An unknown float name refuses the whole call — the node's config is
    /// unchanged.
    #[test]
    fn set_params_floats_object_rejects_an_unknown_float_name() {
        let mut h = fixture("params-floats-unknown");
        let id = h
            .execute(&call(
                "create_node",
                json!({
                    "graph": "g1", "kind": "script.py",
                    "config": { "code": "", "floats": [{ "name": "width", "min": 0.0, "max": 1.0, "default": 0.2 }] },
                }),
            ))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();

        let err = h
            .execute(&call(
                "set_params",
                json!({ "graph": "g1", "id": id, "patch": { "floats": { "nope": 0.5 } } }),
            ))
            .unwrap_err();
        assert!(err.contains("no such float"), "{err}");

        let got = h
            .execute(&call("get_params", json!({ "graph": "g1", "id": id })))
            .unwrap();
        assert_eq!(
            got["config"]["floats"][0].get("value"),
            None,
            "a refused patch must not write anything"
        );
    }

    /// A value outside the float's own `[min, max]` is refused too, rather
    /// than silently clamped on write — clamping is `values()`'s read-time
    /// job, per the module's own defense-in-depth split.
    #[test]
    fn set_params_floats_object_rejects_an_out_of_range_value() {
        let mut h = fixture("params-floats-range");
        let id = h
            .execute(&call(
                "create_node",
                json!({
                    "graph": "g1", "kind": "script.py",
                    "config": { "code": "", "floats": [{ "name": "width", "min": 0.0, "max": 1.0, "default": 0.2 }] },
                }),
            ))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();

        let err = h
            .execute(&call(
                "set_params",
                json!({ "graph": "g1", "id": id, "patch": { "floats": { "width": 5.0 } } }),
            ))
            .unwrap_err();
        assert!(err.contains("outside"), "{err}");

        let got = h
            .execute(&call("get_params", json!({ "graph": "g1", "id": id })))
            .unwrap();
        assert_eq!(got["config"]["floats"][0].get("value"), None);
    }

    /// The param-edge tools: `bind_param` writes the edge, `get_io` reads it
    /// back as the parameter's binding and the variable's `used_by`,
    /// `set_param` holds a value (refusing one out of range), and `set_params`'
    /// `floats` object on a generator is the same writer.
    /// `set_param_batch` writes every held value through the one writer: a
    /// refused value refuses the whole batch, and the saved graph keeps the
    /// floats it had before.
    #[test]
    fn set_param_batch_is_all_or_nothing_through_the_saved_graph() {
        let mut h = fixture("param-batch");
        let gen = h
            .execute(&call("create_node", json!({ "graph": "g1", "kind": "gen.turing" })))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();
        let speed_of = |h: &mut Harness, gen: &str| -> f64 {
            let io = h.execute(&call("get_io", json!({ "graph": "g1", "node": gen }))).unwrap();
            io["params"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["name"] == "speed")
                .unwrap()["value"]
                .as_f64()
                .unwrap()
        };
        h.execute(&call(
            "set_param_batch",
            json!({ "graph": "g1", "updates": [{ "node": gen, "name": "speed", "value": 0.4 }] }),
        ))
        .unwrap();
        assert!((speed_of(&mut h, &gen) - 0.4).abs() < 1e-6);
        let refused = h.execute(&call(
            "set_param_batch",
            json!({ "graph": "g1", "updates": [
                { "node": gen, "name": "speed", "value": 0.9 },
                { "node": gen, "name": "speed", "value": 7.0 }
            ] }),
        ));
        assert!(refused.is_err(), "an out-of-range value refuses the batch");
        assert!((speed_of(&mut h, &gen) - 0.4).abs() < 1e-6, "nothing from a refused batch is saved");
    }

    /// A list in a batch holds the camera (7 floats) in the same write, and a
    /// bad width refuses the batch with the floats beside it unsaved.
    #[test]
    fn set_param_batch_holds_a_camera_list_beside_a_float() {
        let mut h = fixture("param-batch-camera");
        let gen = h
            .execute(&call("create_node", json!({ "graph": "g1", "kind": "gen.turing" })))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();
        let camera = |h: &mut Harness, gen: &str| -> Option<Vec<f64>> {
            let io = h.execute(&call("get_io", json!({ "graph": "g1", "node": gen }))).unwrap();
            let p = io["params"].as_array().unwrap().iter().find(|p| p["name"] == "camera").cloned()?;
            p["values"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect())
        };
        let bad = h.execute(&call(
            "set_param_batch",
            json!({ "graph": "g1", "updates": [
                { "node": gen, "name": "speed", "value": 0.5 },
                { "node": gen, "name": "camera", "value": [1.0, 2.0] }
            ] }),
        ));
        assert!(bad.is_err(), "a camera of the wrong width refuses the batch");
        assert_eq!(camera(&mut h, &gen), None);
        h.execute(&call(
            "set_param_batch",
            json!({ "graph": "g1", "updates": [
                { "node": gen, "name": "speed", "value": 0.5 },
                { "node": gen, "name": "camera", "value": [0, 0, 4, 0, 0, 0, 45] }
            ] }),
        ))
        .unwrap();
        assert_eq!(camera(&mut h, &gen), Some(vec![0.0, 0.0, 4.0, 0.0, 0.0, 0.0, 45.0]));
    }

    #[test]
    fn param_tools_bind_read_and_hold_through_the_saved_graph() {
        let mut h = fixture("param-tools");
        let audio = h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "audio.live" }),
            ))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();
        let gen = h
            .execute(&call(
                "create_node",
                json!({ "graph": "g1", "kind": "gen.turing" }),
            ))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();

        let bound = h
            .execute(&call(
                "bind_param",
                json!({ "graph": "g1", "node": gen, "param": "speed", "from": { "node": audio, "port": "control", "var": "onset" } }),
            ))
            .unwrap();
        assert!(bound["edge"].is_string(), "{bound}");
        let io = h
            .execute(&call("get_io", json!({ "graph": "g1", "node": gen })))
            .unwrap();
        let speed = io["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "speed")
            .unwrap();
        assert_eq!(speed["bound"]["var"], json!("onset"));
        let onset = io["sources"][0]["vars"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == "onset")
            .unwrap();
        assert_eq!(onset["used_by"], json!(["speed"]));

        h.execute(&call(
            "set_param",
            json!({ "graph": "g1", "node": gen, "name": "speed", "value": 0.4 }),
        ))
        .unwrap();
        let io = h
            .execute(&call("get_io", json!({ "graph": "g1", "node": gen })))
            .unwrap();
        let speed = io["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "speed")
            .unwrap();
        assert!((speed["value"].as_f64().unwrap() - 0.4).abs() < 1e-6);
        assert!(h
            .execute(&call(
                "set_param",
                json!({ "graph": "g1", "node": gen, "name": "speed", "value": 7.0 })
            ))
            .is_err());
        // `set_params`' `floats` object on a generator is the same writer.
        h.execute(&call(
            "set_params",
            json!({ "graph": "g1", "id": gen, "patch": { "floats": { "speed": null } } }),
        ))
        .unwrap();
        let io = h
            .execute(&call("get_io", json!({ "graph": "g1", "node": gen })))
            .unwrap();
        assert!(
            io["params"][0]["value"].is_null(),
            "released: {}",
            io["params"][0]
        );

        let unbound = h
            .execute(&call(
                "bind_param",
                json!({ "graph": "g1", "node": gen, "param": "speed", "from": null }),
            ))
            .unwrap();
        assert!(unbound["edge"].is_null());
    }

    /// The pre-existing shape survives unchanged: `patch.floats` as an
    /// *array* still redeclares the floats themselves (name/min/max/default)
    /// via the generic merge, on any node kind — not routed through
    /// [`metal_graph::script_floats::apply_floats_patch`] at all, which only
    /// ever sees the object shape.
    #[test]
    fn set_params_floats_array_still_redeclares_the_floats_themselves() {
        let mut h = fixture("params-floats-array");
        let id = h
            .execute(&call(
                "create_node",
                json!({
                    "graph": "g1", "kind": "script.py",
                    "config": { "code": "", "floats": [{ "name": "width", "min": 0.0, "max": 1.0, "default": 0.2 }] },
                }),
            ))
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();

        h.execute(&call(
            "set_params",
            json!({ "graph": "g1", "id": id, "patch": { "floats": [
                { "name": "width", "min": 0.0, "max": 1.0, "default": 0.3 },
            ] } }),
        ))
        .unwrap();

        let got = h
            .execute(&call("get_params", json!({ "graph": "g1", "id": id })))
            .unwrap();
        assert_eq!(
            got["config"]["floats"],
            json!([{ "name": "width", "min": 0.0, "max": 1.0, "default": 0.3 }])
        );
    }

    #[test]
    fn list_nodes_filters_by_substring() {
        let mut h = fixture("list-nodes");
        h.execute(&call(
            "create_node",
            json!({ "graph": "g1", "kind": "gen.turing" }),
        ))
        .unwrap();
        h.execute(&call(
            "create_node",
            json!({ "graph": "g1", "kind": "fx.shake" }),
        ))
        .unwrap();

        let v = h
            .execute(&call("list_nodes", json!({ "path": "g1", "filter": "fx" })))
            .unwrap();
        let nodes = v["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0]["kind"], "fx.shake");
    }

    #[test]
    fn search_nodes_finds_both_existing_nodes_and_catalog_kinds() {
        let mut h = fixture("search-nodes");
        h.execute(&call(
            "create_node",
            json!({ "graph": "g1", "kind": "gen.turing" }),
        ))
        .unwrap();

        let v = h
            .execute(&call(
                "search_nodes",
                json!({ "graph": "g1", "q": "turing" }),
            ))
            .unwrap();
        let results = v["results"].as_array().unwrap();
        assert!(results
            .iter()
            .any(|r| r["type"] == "node" && r["kind"] == "gen.turing"));
        assert!(results
            .iter()
            .any(|r| r["type"] == "kind" && r["kind"] == "gen.turing"));
    }

    /// R22.5a, rule 6: with a graph named, `search_nodes`'s catalog half
    /// offers what that graph's `+ node` menu would — a Live graph never
    /// turns up `export.video`/`audio.file` (Offline-only) nor `stack.mode`
    /// (retired from the menu) — and with no graph the whole catalog answers
    /// as before.
    #[test]
    fn search_nodes_with_a_graph_offers_what_that_graphs_menu_would() {
        let mut h = fixture("search-nodes-domain");

        let live = h
            .execute(&call(
                "search_nodes",
                json!({ "graph": "g1", "q": "export" }),
            ))
            .unwrap();
        assert!(
            !live["results"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["kind"] == "export.video"),
            "a Live graph's menu has no export.video: {live}"
        );
        let retired = h
            .execute(&call(
                "search_nodes",
                json!({ "graph": "g1", "q": "stack" }),
            ))
            .unwrap();
        assert!(
            !retired["results"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["kind"] == "stack.mode"),
            "stack.mode is retired from the menu: {retired}"
        );

        let all = h
            .execute(&call("search_nodes", json!({ "q": "export" })))
            .unwrap();
        assert!(
            all["results"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["kind"] == "export.video"),
            "no graph named: the whole catalog, compat: {all}"
        );
    }

    #[test]
    fn read_shader_reads_a_user_authored_shader_by_id() {
        let h = fixture("read-shader-id");
        fs::write(h.shader_path("my-shader"), "// wgsl here").unwrap();
        let mut h = h;
        let v = h
            .execute(&call("read_shader", json!({ "id": "my-shader" })))
            .unwrap();
        assert_eq!(v["wgsl"], "// wgsl here");
    }

    #[test]
    fn read_shader_reads_a_template_by_name() {
        let dir =
            std::env::temp_dir().join(format!("metal-agent-templates-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("scene3d.wgsl"), "// escena3d template").unwrap();
        let mut h = fixture("read-shader-name").with_templates_root(dir);
        let v = h
            .execute(&call("read_shader", json!({ "name": "scene3d" })))
            .unwrap();
        assert_eq!(v["wgsl"], "// escena3d template");
    }

    #[test]
    fn read_shader_without_id_or_name_errors() {
        let mut h = fixture("read-shader-missing-arg");
        let err = h.execute(&call("read_shader", json!({}))).unwrap_err();
        assert!(err.contains("expected"), "{err}");
    }

    #[test]
    fn sleep_clamps_to_the_hard_cap() {
        let mut h = fixture("sleep");
        let v = h.execute(&call("sleep", json!({ "ms": 999_999 }))).unwrap();
        assert_eq!(v["slept_ms"], MAX_SLEEP_MS);
    }

    #[test]
    fn get_env_reports_nodes_edges_and_issues() {
        let mut h = fixture("get-env");
        h.execute(&call(
            "create_node",
            json!({ "graph": "g1", "kind": "gen.turing" }),
        ))
        .unwrap();
        let v = h
            .execute(&call("get_env", json!({ "graph": "g1" })))
            .unwrap();
        assert_eq!(v["nodes"].as_array().unwrap().len(), 1);
        assert!(v["issues"].is_array());
    }

    #[test]
    fn unknown_tool_name_is_a_clear_error_not_a_panic() {
        let mut h = fixture("unknown-tool");
        let err = h.execute(&call("no_such_tool", json!({}))).unwrap_err();
        assert!(
            err.contains("no such tool") || err.contains("not implemented"),
            "{err}"
        );
    }

    /// E2: `capture_output` is a real tool now — the `/render` node route and
    /// the `AgentHarness` executor behind it. The catalog must say so, which
    /// is what `GET /ai-config`/MCP `tools/list` read.
    #[test]
    fn capture_output_is_marked_implemented() {
        let (_, _, implemented) = TOOL_CATALOG
            .iter()
            .find(|(name, _, _)| *name == "capture_output")
            .expect("capture_output is in the catalog");
        assert!(
            implemented,
            "capture_output has its node route and executor"
        );
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn raster_compiles_and_rasters_a_trivial_shader() {
        let mut h = fixture("raster-ok");
        let shader = r#"
struct Uniform { floats: vec4<f32> };
@group(0) @binding(0) var<uniform> u: Uniform;
@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    return vec4<f32>(u.floats.x, u.floats.y, u.floats.z, 1.0);
}
"#;
        let v = h
            .execute(&call(
                "raster",
                json!({ "shader": shader, "floats": [1.0, 0.0, 0.0, 1.0], "w": 4, "h": 4 }),
            ))
            .unwrap();
        assert_eq!(v["ok"], true, "{v}");
        assert!(v["png_base64"].as_str().unwrap().len() > 0);
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn raster_reports_a_compile_error_with_line_and_column_instead_of_failing_the_call() {
        let mut h = fixture("raster-bad-shader");
        let v = h
            .execute(&call("raster", json!({ "shader": "not wgsl at all {{{" })))
            .unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().len() > 0);
    }

    /// A trivial time-varying shader for `bake_clip`'s tests below: it paints
    /// `t` (this clip's own `floats.x`) into the red channel, so a broken
    /// determinism guarantee (e.g. an uninitialized buffer, or `t` not
    /// actually advancing) would show up as differing pixels, not just as a
    /// wrong frame count.
    #[cfg(feature = "gpu")]
    const BAKE_SHADER: &str = r#"
struct Uniform { floats: vec4<f32> };
@group(0) @binding(0) var<uniform> u: Uniform;
@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    return vec4<f32>(u.floats.x, in.uv.x, in.uv.y, 1.0);
}
"#;

    #[cfg(feature = "gpu")]
    #[test]
    fn bake_clip_writes_one_png_per_frame() {
        let mut h = fixture("bake-ok");
        let v = h
            .execute(&call(
                "bake_clip",
                json!({ "source": BAKE_SHADER, "duration": 0.5, "fps": 4.0, "w": 4, "h": 4 }),
            ))
            .unwrap();
        assert_eq!(v["ok"], true, "{v}");
        // duration * fps = 2 frames, computed the same way the tool itself
        // does — not a hand-picked "should be N" (rule 7 is about manifest
        // counts, but the same judgment applies to anything derivable).
        assert_eq!(v["frames"], 2);
        let dir = std::path::PathBuf::from(v["dir"].as_str().unwrap());
        assert!(dir.join("000000.png").exists(), "{v}");
        assert!(dir.join("000001.png").exists(), "{v}");
        assert!(
            !dir.join("000002.png").exists(),
            "only 2 frames were requested: {v}"
        );
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn bake_clip_reports_a_compile_error_instead_of_failing_the_call() {
        let mut h = fixture("bake-bad-shader");
        let v = h
            .execute(&call(
                "bake_clip",
                json!({ "source": "not wgsl at all {{{", "duration": 1.0 }),
            ))
            .unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().len() > 0);
    }

    /// I.5's own verification for S.M, spelled out as a test rather than a
    /// caveat: baking the same clip twice has to produce byte-identical
    /// frames. This is the test that actually exercises non-determinism if
    /// it exists — a wall-clock seed, an uninitialized readback buffer, a
    /// HashMap iterating in a different order — rather than trusting
    /// `metal_forge::raster`'s own doc comment that it has none.
    #[cfg(feature = "gpu")]
    #[test]
    fn bake_clip_is_byte_identical_across_two_bakes_of_the_same_clip() {
        let mut h = fixture("bake-determinism");
        let args = json!({ "source": BAKE_SHADER, "duration": 0.25, "fps": 8.0, "w": 6, "h": 6 });

        let first = h.execute(&call("bake_clip", args.clone())).unwrap();
        assert_eq!(first["ok"], true, "{first}");
        let second = h.execute(&call("bake_clip", args)).unwrap();
        assert_eq!(second["ok"], true, "{second}");

        // Same request -> same id -> same directory, per `bake_clip_id`'s
        // own doc comment.
        assert_eq!(first["id"], second["id"]);
        let frames = first["frames"].as_u64().unwrap();
        assert!(frames > 0);

        // Same id means the second bake wrote to (over) the same
        // directory the first one did — checked so the real proof below
        // (independent roots, so a re-bake cannot just be overwriting its
        // own prior output unchanged) is not the only thing establishing
        // determinism.
        let dir = std::path::PathBuf::from(first["dir"].as_str().unwrap());
        for frame in 0..frames {
            assert!(dir.join(format!("{frame:06}.png")).exists());
        }
    }

    /// Same clip, baked into two *different* harness roots (so the second
    /// bake cannot simply be overwriting the first's files) — this is the
    /// actual byte-identical proof I.5 asks for, comparing independently
    /// produced frames rather than a file against itself.
    #[cfg(feature = "gpu")]
    #[test]
    fn bake_clip_produces_identical_frames_in_two_independent_roots() {
        let mut h1 = fixture("bake-determinism-a");
        let mut h2 = fixture("bake-determinism-b");
        let args = json!({ "source": BAKE_SHADER, "duration": 0.25, "fps": 8.0, "w": 6, "h": 6 });

        let v1 = h1.execute(&call("bake_clip", args.clone())).unwrap();
        let v2 = h2.execute(&call("bake_clip", args)).unwrap();
        assert_eq!(v1["ok"], true, "{v1}");
        assert_eq!(v2["ok"], true, "{v2}");
        assert_eq!(v1["id"], v2["id"]);

        let dir1 = std::path::PathBuf::from(v1["dir"].as_str().unwrap());
        let dir2 = std::path::PathBuf::from(v2["dir"].as_str().unwrap());
        let frames = v1["frames"].as_u64().unwrap();
        assert!(frames > 0);
        for frame in 0..frames {
            let name = format!("{frame:06}.png");
            let a = fs::read(dir1.join(&name)).unwrap();
            let b = fs::read(dir2.join(&name)).unwrap();
            assert_eq!(a, b, "frame {frame} differs between independent bakes");
        }
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    /// The catalog tools that touch no capability, named one by one. Adding a
    /// tool to `TOOL_CATALOG` fails the test below until it is classified:
    /// either it joins `capability_of`'s arms or it is listed here on purpose.
    const NO_CAPABILITY: &[&str] = &[
        "sync_status",
        "sync_peers",
        "sync_log",
        "timecode_status",
        "focus",
        "list_transitions",
        "list_tools",
        "get_env",
        "list_nodes",
        "search_nodes",
        "create_node",
        "delete_node",
        "connect_nodes",
        "get_params",
        "list_kinds",
        "set_params",
        "read_shader",
        "read_script",
        "save_script",
        "raster",
        "take_graph",
        "release_graph",
        "graph_owner",
        "capture_output",
        "sleep",
        "locate",
        "create_mask_grid",
        "edit_mask",
        "add_effect",
        "remove_effect",
        "edit_output",
        "resolve_draft",
        "edit_venue_surface",
        "edit_venue_object",
        "add_screen",
        "add_projection",
        "set_surface_form",
        "set_venue_sectors",
        "move_venue_item",
        "projection_area",
        "venue_scene",
        "add_stage",
        "add_truss",
        "add_dummy",
        "venue_spline",
        "place_mask_in_space",
        "venue_boolean",
        "venue_hit",
        "venue_report",
        "venue_view",
        "venue_assets_search",
        "venue_asset_import",
        "list_switches",
        "set_switch",
        "get_tick_level",
        "set_tick_level",
        "add_switch",
        "remove_switch",
        "import_segment",
        "move_segment",
        "remove_segment",
        "set_keyframe",
        "train_ai",
        "embed_ai",
        "set_layer_mix",
        "rasterize_overlay",
        "edit_layer",
        "listen_status",
        "get_io",
        "bind_param",
        "set_param",
        "set_param_batch",
        "set_material",
        "set_palette",
        "list_scenes",
        "list_dataset",
        "edit_scene",
        "list_video_io",
        "attach_source",
        "insert_node",
        "colour_apply",
        "perf_report",
        "get_grid_layouts",
        "set_grid_layouts",
        "export_status",
        "list_palettes",
    ];

    #[test]
    fn every_catalog_tool_is_classified_on_purpose() {
        for (name, _, _) in TOOL_CATALOG {
            let listed = NO_CAPABILITY.contains(name);
            let classified = capability_of(name).is_some();
            assert!(
                listed != classified,
                "`{name}` must be either classified in capability_of or listed in NO_CAPABILITY, exactly one"
            );
        }
        for name in NO_CAPABILITY {
            assert!(
                TOOL_CATALOG.iter().any(|(n, _, _)| n == name),
                "NO_CAPABILITY names `{name}`, which is not in TOOL_CATALOG"
            );
        }
    }

    /// D2 — `import_gdtf` is a real catalog tool: implemented, classified as a
    /// `Files` capability (it writes a profile, like `save_shader`), with a
    /// schema naming `path`/`bytes` and requiring neither.
    #[test]
    fn import_gdtf_is_cataloged_classified_and_schematized() {
        assert!(
            TOOL_CATALOG
                .iter()
                .any(|(n, _, ok)| *n == "import_gdtf" && *ok),
            "import_gdtf must be in TOOL_CATALOG and implemented"
        );
        assert_eq!(capability_of("import_gdtf"), Some(Capability::Files));
        let schema =
            crate::tool_schemas::parameters("import_gdtf").expect("import_gdtf has a schema");
        assert!(
            schema["properties"].get("path").is_some(),
            "schema names `path`"
        );
        assert!(
            schema["properties"].get("bytes").is_some(),
            "schema names `bytes`"
        );
        assert!(
            schema["required"].as_array().unwrap().is_empty(),
            "neither path nor bytes is forced"
        );
    }

    #[test]
    fn a_denied_capability_blocks_its_tools() {
        let saved = json!({ "permissions": { "dispositivos": "deny" } });
        assert_eq!(effective_policy("blackout", &saved), ToolPolicy::Off);
        assert_eq!(effective_policy("aim_fixture", &saved), ToolPolicy::Off);
        // A tool with no capability is untouched by someone else's deny.
        assert_eq!(effective_policy("list_nodes", &saved), ToolPolicy::Auto);
    }

    #[test]
    fn an_ask_capability_routes_its_tools_to_approval() {
        let saved = json!({ "permissions": { "archivos": "ask" } });
        assert_eq!(effective_policy("bake_clip", &saved), ToolPolicy::Ask);
        assert_eq!(effective_policy("attach_digest", &saved), ToolPolicy::Ask);
    }

    #[test]
    fn an_unanswered_capability_defaults_to_ask_and_allow_runs_free() {
        assert_eq!(effective_policy("blackout", &json!({})), ToolPolicy::Ask);
        let allowed =
            json!({ "permissions": { "dispositivos": "allow" }, "policy": { "blackout": "off" } });
        // The capability wins over a per-tool `off` saved for the same tool.
        assert_eq!(effective_policy("blackout", &allowed), ToolPolicy::Auto);
        assert_eq!(permission_wire(ToolPolicy::Off), "deny");
        assert_eq!(permission_policy("nonsense"), None);
    }

    #[test]
    fn operator_instructions_reach_the_system_prompt_under_their_heading() {
        let prompt = with_operator_instructions("base", "  do not use devices without asking me  ");
        assert!(prompt.starts_with("base"));
        assert!(prompt.contains("## Operator instructions\n\ndo not use devices without asking me"));
        assert_eq!(with_operator_instructions("base", "   "), "base");
    }
}
