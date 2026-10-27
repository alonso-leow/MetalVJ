//! S.K: the four timeline-edit tools (`import_segment`, `move_segment`,
//! `remove_segment`, `set_keyframe`), as a [`metal_agent::agent_loop::ToolExecutor`]
//! that lives beside [`super::edit_segment`] and [`super::example::save`]
//! instead of inside `metal-agent`.
//!
//! **Why here, and not in `metal_agent::harness::Harness`.** `Harness` (H.4)
//! wraps `metal-graph` — a domain crate with no dependency on `metal-dash` or
//! `metal-agent`, which is exactly what lets `Harness` sit in `metal-agent` and
//! still call the identical mutation `metal-dash`'s graph routes call. Timeline
//! edit has no such domain crate: [`super::timeline::Timeline`] and
//! [`super::example::Example`] are typed against `metal-dash`-only things
//! (`crate::layers::Blend`, `crate::node_layer::NodeLayerConfig`,
//! `crate::output::data_root`), and `Timeline::validate` needs the real node
//! catalog, which needs `metal_render`'s manifest behind the `gpu` feature.
//! `metal-dash` already depends on `metal-agent` (`harness.rs`'s own module doc),
//! so the only way to share the *literal* function the HTTP route calls
//! without a dependency cycle is to put the `ToolExecutor` where that
//! function already lives — here — and have it call `super::edit_segment`/
//! `super::example::save` directly, not a re-implementation of either.
//!
//! Every one of the four is a JSON-shape translation and nothing else:
//! `import_segment`/`move_segment`/`remove_segment` build exactly the
//! `{timeline, track, op, segment|id}` body `POST /timeline/segment`
//! (`super::edit_segment`) already expects, and hand it straight there.
//! `move_segment` has to read the existing segment first — per that
//! function's own comment, `move` **replaces the whole segment by id**,
//! there is no partial patch, so a move that only sent `{from, to}` would
//! blank out every other field the drag in `lanes` would have carried along
//! (`model`, `mix`, …). `set_keyframe` reads the model's `ticks` array
//! (`super::example::load`), replaces-or-inserts the one `Tick` at the given
//! tick, and calls `super::example::save` — the same read-modify-write
//! `POST /example` already does, and the same `ModelSchema::width()` check
//! `Example::validate` already runs inside it.
//!
//! [`AgentHarness`] wraps a [`metal_agent::harness::Harness`] for the other
//! fourteen tools and only intercepts these four — a caller wanting "every
//! tool this agent can run" constructs one `AgentHarness` and never touches
//! `Harness` directly.
//!
//! **Since O.1/O.3's closure (2026-09-12)** it also carries the tools that
//! are the *same* translation onto routes that need the live `State`
//! rather than a file: `create_mask_grid`/`test_mask_grid` (S.B, over
//! `/mask/grid` and `/mask/project`) and `edit_mask` (O.1, over
//! `POST /outputs { op: "mask" }`). Those reach `State` the way every panel
//! does — through the app's door ([`crate::call::Call`], handed over by
//! [`AgentHarness::with_door`]) — and without one they say so instead of
//! answering `ok: true`, which is what the placeholders they replaced did.
//! `set_layer_mix` (S.D) is a fifth file-shaped tool: the same
//! read-modify-write as `move_segment`, patching only a segment's `mix`.

use std::path::Path;

use metal_agent::agent_loop::{ToolCall, ToolExecutor};
use serde_json::{json, Value};

use super::camera_solve::Camera as _;
use super::schema::Manifest;
use super::{example, timeline, EXAMPLES_DIR};
use crate::call::Call;

/// The two tracks whose segments carry a [`timeline::SegmentMix`] — the
/// order `set_layer_mix` searches when the caller leaves `track` out.
const MIX_TRACKS: [&str; 2] = ["model", "media"];

/// The four S.K tools, plus a wrapped [`metal_agent::harness::Harness`] for
/// every other tool it does not intercept.
pub struct AgentHarness {
    inner: metal_agent::harness::Harness,
    manifest: Manifest,
    /// The app's own door (`crate::call::door::Door`, or a test double) —
    /// the only way a tool reaches the live `State`: `masks` live on
    /// `state.outputs`, behind the same lock the shell and `serve` take per
    /// call. `None` until [`Self::with_door`] hands one over, and every
    /// tool that needs it says so instead of pretending
    /// (`the_mask_tools_refuse_honestly_without_a_door`).
    door: Option<Box<dyn Call + Send>>,
}

impl AgentHarness {
    pub fn new(inner: metal_agent::harness::Harness, manifest: Manifest) -> AgentHarness {
        AgentHarness {
            inner,
            manifest,
            door: None,
        }
    }

    /// Hand this harness the door the mask tools (`create_mask_grid`,
    /// `test_mask_grid`, `edit_mask`) call `POST /mask/grid`, `POST
    /// /mask/project` and `POST /outputs` through — rule 6 by
    /// construction: the tool *is* the route the panel calls. The chat's
    /// turn runs on its own worker thread and the door locks per call, so
    /// handing it the real `Door` is exactly what the shell already does.
    pub fn with_door(mut self, door: Box<dyn Call + Send>) -> AgentHarness {
        self.door = Some(door);
        self
    }

    fn door(&self) -> Result<&dyn Call, String> {
        self.door.as_deref().map(|d| d as &dyn Call).ok_or_else(|| {
            "this tool needs the app's door (AgentHarness::with_door) — none was attached"
                .to_string()
        })
    }

    /// `outputs` (S.B's spelling: a list of device ids) or `device_id`
    /// (one) or `device_ids` — the same three the route reads — as the
    /// list the route wants.
    fn device_ids(args: &Value) -> Result<Vec<String>, String> {
        if let Some(list) = args
            .get("outputs")
            .or_else(|| args.get("device_ids"))
            .and_then(Value::as_array)
        {
            let ids: Vec<String> = list
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if ids.is_empty() {
                return Err("`outputs` must name at least one device id".to_string());
            }
            return Ok(ids);
        }
        if let Some(one) = args.get("device_id").and_then(Value::as_str) {
            return Ok(vec![one.to_string()]);
        }
        Err("expected `outputs` (a list of device ids) or `device_id`".to_string())
    }

    fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
        args.get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("expected `{key}`"))
    }

    fn require_u64(args: &Value, key: &str) -> Result<u64, String> {
        args.get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("expected `{key}` (a non-negative integer)"))
    }

    /// `import_segment(timeline, track, segment)` — the `op: "add"` half of
    /// `POST /timeline/segment`, verbatim.
    fn import_segment(&self, args: &Value) -> Result<Value, String> {
        let timeline_id = Self::require_str(args, "timeline")?;
        let track = Self::require_str(args, "track")?;
        let segment = args.get("segment").cloned().ok_or("expected `segment`")?;
        let body =
            json!({ "timeline": timeline_id, "track": track, "op": "add", "segment": segment });
        super::edit_segment(&body).map(|tl| json!(tl))
    }

    /// `move_segment(timeline, track, id, from, to, layer?)`. Fetches the
    /// named segment as it stands today, patches only `from`/`to` (and
    /// `layer`, for a `model`/`media` segment moved to another layer), and
    /// resubmits the *whole* segment through `super::edit_segment` — same
    /// wholesale replace-by-id the page's own drag produces, per that
    /// function's own doc comment.
    fn move_segment(&self, args: &Value) -> Result<Value, String> {
        let timeline_id = Self::require_str(args, "timeline")?;
        let track = Self::require_str(args, "track")?;
        let id = Self::require_str(args, "id")?;
        let from = Self::require_u64(args, "from")?;
        let to = Self::require_u64(args, "to")?;

        let tl = timeline::load(Path::new(crate::author::TIMELINES), timeline_id)
            .ok_or_else(|| format!("no timeline called {timeline_id:?}"))?;
        let mut segment = find_segment_json(&tl, track, id)
            .ok_or_else(|| format!("no {track:?} segment {id:?} on timeline {timeline_id:?}"))?;
        let obj = segment
            .as_object_mut()
            .ok_or("segment is not a JSON object")?;
        obj.insert("from".to_string(), json!(from));
        obj.insert("to".to_string(), json!(to));
        if let Some(layer) = args.get("layer") {
            obj.insert("layer".to_string(), layer.clone());
        }

        let body =
            json!({ "timeline": timeline_id, "track": track, "op": "move", "segment": segment });
        super::edit_segment(&body).map(|tl| json!(tl))
    }

    /// `remove_segment(timeline, track, id)` — the `op: "remove"` half of
    /// `POST /timeline/segment`, verbatim. `edit_segment` reads the id off
    /// either `segment.id` or a bare `id`; this tool uses the latter, since
    /// there is no segment body to remove, only a name.
    fn remove_segment(&self, args: &Value) -> Result<Value, String> {
        let timeline_id = Self::require_str(args, "timeline")?;
        let track = Self::require_str(args, "track")?;
        let id = Self::require_str(args, "id")?;
        let body = json!({ "timeline": timeline_id, "track": track, "op": "remove", "id": id });
        super::edit_segment(&body).map(|tl| json!(tl))
    }

    /// `set_keyframe(model, song, tick, floats)`. Reads the model's own
    /// `ticks` array (`super::example::load`, `target: None` — same corpus
    /// `GET /example` reads with no `device`/`layer`), replaces the `Tick`
    /// at `tick` if one exists or inserts a new one, and rewrites the
    /// document through `super::example::save`, which sorts, validates
    /// against `ModelSchema::width()` (`Example::validate`) and writes —
    /// the exact function `POST /example` calls.
    /// The active tick level, read off `GET /settings` through the door — the
    /// one place the level lives, so a tool speaks it without a second copy.
    /// Base when there is no door or the route is silent (the minimum).
    fn active_rate(&self) -> metal_schema::TickRate {
        self.door()
            .ok()
            .and_then(|d| d.json("GET", "/settings", None).ok())
            .and_then(|v| {
                v["tick_level"]
                    .as_str()
                    .and_then(metal_schema::TickRate::parse)
            })
            .unwrap_or(metal_schema::TickRate::Base)
    }

    fn set_keyframe(&self, args: &Value) -> Result<Value, String> {
        let model = Self::require_str(args, "model")?;
        let song = Self::require_str(args, "song")?;
        // The call's `tick` is in the **active** level; storage is canonical.
        let active = self.active_rate();
        let tick = active.to_canonical(Self::require_u64(args, "tick")? as i64) as usize;
        let floats: Vec<f32> = args
            .get("floats")
            .and_then(Value::as_array)
            .ok_or("expected `floats` (an array of numbers)")?
            .iter()
            .map(|v| {
                v.as_f64()
                    .map(|f| f as f32)
                    .ok_or_else(|| "every float must be a number".to_string())
            })
            .collect::<Result<_, _>>()?;

        let schema = self
            .manifest
            .model(model)
            .ok_or_else(|| format!("no such model: {model}"))?;

        // A new keyframe file is born on the canonical grid, and the tick in
        // the call is a canonical tick (the level the storage and this tool
        // speak). `load` already migrated a legacy file to the same grid.
        let mut ex =
            example::load(Path::new(EXAMPLES_DIR), model, song, None).unwrap_or_else(|| {
                example::Example {
                    model: model.to_string(),
                    song: song.to_string(),
                    rate: metal_schema::TickRate::CANONICAL.hz() as f32,
                    ticks: Vec::new(),
                }
            });
        ex.ticks.retain(|t| t.tick != tick);
        ex.ticks.push(example::Tick { tick, floats });

        let n = example::save(Path::new(EXAMPLES_DIR), ex, schema, None)?;
        // Speak the active level back: the caller's tick and the level it is
        // in, so the notation never silently changes under the agent.
        Ok(json!({
            "model": model,
            "song": song,
            "tick": active.from_canonical(tick as i64),
            "tick_level": active.wire(),
            "ticks": n,
        }))
    }

    /// `attach_digest(timeline, segment_id)` — S.L, decided with the operator
    /// 2026-09-05: "digest de sonido" runs the digester over the *clip's
    /// own* audio, never the timeline's song. No `POST /timeline/*` route
    /// wraps this yet — S.L has no UI half — so this calls the same on-disk
    /// mechanism the song ingest queue already uses
    /// (`crate::author::audio::digest_to`), pointed at one clip's file
    /// instead of the open song, and at its own destination under
    /// `data/digest/` rather than beside the footage in `data/reels/**`.
    ///
    /// Resolves `(library, clip)` to the actual media file the same way
    /// `timeline::resolve`'s media loop plays it: through the indexed
    /// `metal_reel::index::Library`, never a second reader of
    /// `data/reels/<name>/index.json`. `digest_audio.py` opens the file
    /// through librosa/audioread directly, which reads a video's own audio
    /// track exactly like a plain audio file — no separate ffmpeg
    /// extraction step is needed here.
    fn attach_digest(&self, args: &Value) -> Result<Value, String> {
        let timeline_id = Self::require_str(args, "timeline")?;
        let segment_id = Self::require_str(args, "segment_id")?;

        let mut tl = timeline::load(Path::new(crate::author::TIMELINES), timeline_id)
            .ok_or_else(|| format!("no timeline called {timeline_id:?}"))?;
        let (library, clip_id) = {
            let seg = tl
                .media_track
                .iter()
                .find(|m| m.id == segment_id)
                .ok_or_else(|| format!("no such media segment: {segment_id}"))?;
            (seg.library.clone(), seg.clip.clone())
        };

        let lib = metal_reel::index::Library::load(Path::new("data"), &library)
            .map_err(|e| format!("cannot open library {library:?}: {e}"))?;
        let (_, clip) = lib
            .by_id(&clip_id)
            .ok_or_else(|| format!("{library:?} has no clip called {clip_id:?}"))?;
        let media_path = Path::new(&lib.source).join(&clip.file);

        let out = Path::new("data/digest").join(metal_schema::digest_file_name(
            segment_id,
            metal_schema::TickRate::Base,
        ));
        crate::author::audio::digest_to(&media_path, &out)?;

        let seg = tl
            .media_track
            .iter_mut()
            .find(|m| m.id == segment_id)
            .expect("checked above");
        seg.digest = Some(segment_id.to_string());

        let cat = timeline::catalog()?;
        let n = timeline::save(Path::new(crate::author::TIMELINES), tl, &cat)?;
        Ok(json!({
            "ok": true,
            "timeline": timeline_id,
            "segment": segment_id,
            "digest": segment_id,
            "matrix": out.display().to_string(),
            "segments": n,
        }))
    }

    /// S.A.1 / S.B: `create_mask_grid(outputs, rows, cols)` — `POST
    /// /mask/grid` through the door, verbatim: a `rows`×`cols` grid of
    /// rectangular cells on each named output (`call::outputs::mask_grid`).
    /// Returns the route's own reply — the new `mask_ids` and the devices.
    fn create_mask_grid(&self, args: &Value) -> Result<Value, String> {
        let device_ids = Self::device_ids(args)?;
        let rows = Self::require_u64(args, "rows")?;
        let cols = Self::require_u64(args, "cols")?;
        let door = self.door()?;
        door.json(
            "POST",
            "/mask/grid",
            Some(json!({ "device_ids": device_ids, "rows": rows, "cols": cols })),
        )
        .map_err(|(status, msg)| format!("/mask/grid answered {status}: {msg}"))
    }

    /// S.A.2 / S.B: `test_mask_grid(outputs, mask_id?, rgb?)` — `POST
    /// /mask/project` through the door, one frame per output: with no
    /// `mask_id`, every contoured mask on the device outlined in its own
    /// colour (`call::render::mask_project`'s own legend,
    /// `crate::output::mask_color`); with one, that mask alone in `rgb`
    /// (white by default). The frame comes back as `jpeg_base64` — the
    /// route's own bytes, not re-encoded — with the legend beside it so the
    /// model knows which colour is which cell.
    fn test_mask_grid(&self, args: &Value) -> Result<Value, String> {
        let device_ids = Self::device_ids(args)?;
        let door = self.door()?;
        let mut frames = Vec::new();
        for device_id in &device_ids {
            let mut body = json!({ "device_id": device_id });
            if let Some(mid) = args.get("mask_id") {
                body["mask_id"] = mid.clone();
            }
            if let Some(rgb) = args.get("rgb") {
                body["rgb"] = rgb.clone();
            }
            let params = crate::call::Params {
                query: Default::default(),
                body: body.to_string().into_bytes(),
                tail: None,
            };
            let reply = door
                .call("POST", "/mask/project", params)
                .ok_or_else(|| "POST /mask/project matched no route".to_string())?;
            if reply.status != 200 {
                return Err(format!(
                    "/mask/project answered {}: {}",
                    reply.status,
                    String::from_utf8_lossy(&reply.body)
                ));
            }
            // The legend: the same colour-per-index rule the route drew with.
            let state = door
                .json("GET", "/state", None)
                .map_err(|(st, m)| format!("/state answered {st}: {m}"))?;
            let legend: Vec<Value> = state["outputs"]
                .as_array()
                .and_then(|outs| outs.iter().find(|o| o["id"].as_str() == Some(device_id.as_str())))
                .and_then(|o| o["masks"].as_array())
                .map(|masks| {
                    masks
                        .iter()
                        .enumerate()
                        .map(|(i, m)| {
                            json!({ "id": m["id"], "name": m["name"], "rgb": crate::output::mask_color(i) })
                        })
                        .collect()
                })
                .unwrap_or_default();
            frames.push(json!({
                "device_id": device_id,
                "mime": reply.kind,
                "jpeg_base64": metal_agent::harness::base64_encode(&reply.body),
                "legend": legend,
            }));
        }
        Ok(json!({ "ok": true, "frames": frames }))
    }

    /// O.1's authoring half: `edit_mask(device_id, mask_id?, name?, points?,
    /// parent?, alpha?, compose?)` — `POST /outputs { op: "mask" }` through
    /// the door, verbatim (`call::outputs::edit_mask`): the keys present are
    /// written, the rest kept; no `mask_id` creates a mask; `compose` is
    /// `{ op: union|intersect|subtract, a, b }` or `null`, `parent` a mask
    /// id or `null`. H.10's Venn diagram and H.5b's nested contour are this
    /// tool, and the lanes panel's mask editor posts the same body.
    fn edit_mask(&self, args: &Value) -> Result<Value, String> {
        let device_id = Self::require_str(args, "device_id")?;
        let mut body = json!({ "op": "mask", "id": device_id });
        if let Some(mid) = args.get("mask_id").and_then(Value::as_str) {
            body["mask"] = json!(mid);
        }
        let remove = args.get("remove").and_then(Value::as_bool) == Some(true);
        let mut any = false;
        for key in ["name", "points", "parent", "alpha", "compose"] {
            if let Some(v) = args.get(key) {
                body[key] = v.clone();
                any = true;
            }
        }
        // The route's own rule, said out loud rather than fallen into: a
        // body naming no field *deletes*. Only `remove: true` is allowed to
        // mean that here, so a call that forgot its fields errors instead of
        // silently taking a mask off the wall.
        if remove {
            if any {
                return Err("`remove: true` takes no other field".to_string());
            }
            if body.get("mask").is_none() {
                return Err("`remove: true` needs `mask_id`".to_string());
            }
        } else if !any {
            return Err("nothing to edit — give at least one of `name`, `points`, `parent`, `alpha`, `compose`, or `remove: true`".to_string());
        }
        let door = self.door()?;
        let reply = door
            .json("POST", "/outputs", Some(body))
            .map_err(|(status, msg)| format!("/outputs answered {status}: {msg}"))?;
        let masks = reply["devices"]
            .as_array()
            .and_then(|ds| ds.iter().find(|d| d["id"].as_str() == Some(device_id)))
            .map(|d| d["masks"].clone())
            .unwrap_or(Value::Null);
        Ok(json!({ "ok": true, "device_id": device_id, "touched": reply["mask"], "masks": masks }))
    }

    /// G1: `add_effect(graph, node, effect, state, amount?, source?)` —
    /// `POST /graph/effects` verbatim (`call::graph::effects_post`): `state`
    /// is `reactive` or `fixed` (then `amount` is its value, the route's
    /// `value`). The graph-editor's effect row posts the same body.
    fn add_effect(&self, args: &Value) -> Result<Value, String> {
        let state = Self::require_str(args, "state")?;
        let mut body = self.effect_body(args)?;
        match state {
            "reactive" => {}
            "fixed" => {
                let amount = args
                    .get("amount")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| "`fixed` needs `amount`".to_string())?;
                body["value"] = json!(amount);
            }
            other => return Err(format!("`state` is reactive or fixed, not {other:?}")),
        }
        body["state"] = json!(state);
        if let Some(src) = args.get("source") {
            body["source"] = src.clone();
        }
        self.post_effect(body)
    }

    /// G1: `remove_effect(graph, node, effect)` — the same route with
    /// `state: "off"`, which is what the row's off switch posts.
    fn remove_effect(&self, args: &Value) -> Result<Value, String> {
        let mut body = self.effect_body(args)?;
        body["state"] = json!("off");
        self.post_effect(body)
    }

    fn effect_body(&self, args: &Value) -> Result<Value, String> {
        Ok(json!({
            "graph": Self::require_str(args, "graph")?,
            "node": Self::require_str(args, "node")?,
            "effect": Self::require_str(args, "effect")?,
        }))
    }

    fn post_effect(&self, body: Value) -> Result<Value, String> {
        let door = self.door()?;
        let reply = door
            .json("POST", "/graph/effects", Some(body))
            .map_err(|(status, msg)| format!("/graph/effects answered {status}: {msg}"))?;
        Ok(json!({ "ok": true, "effects": reply["effects"].clone() }))
    }

    /// G1: `edit_output(op, id, label?, width?, height?)` — `POST /outputs`
    /// with `op` add (`label`/`width`/`height`), remove (`id` only) or
    /// resolution (`width`/`height`), the Outputs panel's own body.
    fn edit_output(&self, args: &Value) -> Result<Value, String> {
        let op = Self::require_str(args, "op")?;
        let id = Self::require_str(args, "id")?;
        let mut body = json!({ "op": op, "id": id });
        match op {
            "add" => {
                for key in ["label", "width", "height"] {
                    if let Some(v) = args.get(key) {
                        body[key] = v.clone();
                    }
                }
            }
            "remove" => {}
            "resolution" => {
                let (w, h) = (args.get("width"), args.get("height"));
                if w.is_none() && h.is_none() {
                    return Err("`resolution` needs `width` or `height`".to_string());
                }
                for (key, v) in [("width", w), ("height", h)] {
                    if let Some(v) = v {
                        body[key] = v.clone();
                    }
                }
            }
            other => return Err(format!("`op` is add, remove or resolution, not {other:?}")),
        }
        let door = self.door()?;
        let reply = door
            .json("POST", "/outputs", Some(body))
            .map_err(|(status, msg)| format!("/outputs answered {status}: {msg}"))?;
        Ok(json!({ "ok": true, "op": op, "id": id, "devices": reply["devices"].clone() }))
    }

    /// G1: `resolve_draft(venue, decision)` — `POST /venue { op: accept |
    /// reject, id }`, the editor's diff buttons (`call::venue`). Accept
    /// writes the draft over the venue; reject drops it.
    fn resolve_draft(&self, args: &Value) -> Result<Value, String> {
        let venue = Self::require_str(args, "venue")?;
        let decision = Self::require_str(args, "decision")?;
        if decision != "accept" && decision != "reject" {
            return Err(format!("`decision` is accept or reject, not {decision:?}"));
        }
        let door = self.door()?;
        let reply = door
            .json("POST", "/venue", Some(json!({ "op": decision, "id": venue })))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))?;
        Ok(reply)
    }

    /// P3: `edit_venue_surface(venue, surface, name?, points?, origin?, u?, v?,
    /// material?, delete?)` — `POST /venue { op: "surface" }` through the door,
    /// the same body the surface panel posts (rule 6). `points` is four
    /// `[x, y, z]` corners; `delete: true` removes the surface (and every
    /// projector's pins for it).
    /// `embed_ai { graph, node, ai, on }`: the properties panel's reaction
    /// toggle, `POST /graph/node {op: "ai.embed"}` verbatim (rule 6). `on`
    /// mints the owned `ai.*` node wired `audio → ai → source.control`; `off`
    /// frees it into a normal node.
    fn embed_ai(&self, args: &Value) -> Result<Value, String> {
        let graph = Self::require_str(args, "graph")?;
        let node = Self::require_str(args, "node")?;
        let ai = Self::require_str(args, "ai")?;
        let on = args
            .get("on")
            .and_then(Value::as_bool)
            .ok_or("expected `on` (true or false)")?;
        let door = self.door()?;
        door.json(
            "POST",
            "/graph/node",
            Some(json!({ "op": "ai.embed", "graph": graph, "node": node, "ai": ai, "on": on })),
        )
        .map_err(|(status, msg)| format!("/graph/node answered {status}: {msg}"))
    }

    /// `attach_source { graph, node, port, kind?, from?, config? }`: the
    /// properties panel's `attach` body, verbatim (rule 6). Without `kind` or
    /// `from` it detaches the port.
    fn attach_source(&self, args: &Value) -> Result<Value, String> {
        let graph = Self::require_str(args, "graph")?;
        let node = Self::require_str(args, "node")?;
        let port = Self::require_str(args, "port")?;
        let mut body = json!({ "op": "attach", "graph": graph, "node": node, "port": port });
        for key in ["kind", "from", "config"] {
            if let Some(v) = args.get(key).filter(|v| !v.is_null()) {
                body[key] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/graph/node", Some(body))
            .map_err(|(status, msg)| format!("/graph/node answered {status}: {msg}"))
    }

    /// `insert_node { graph, after, kind }`: `POST /graph/node {op: "insert"}`,
    /// the body the panel's `+ effect` dropdown posts.
    fn insert_node(&self, args: &Value) -> Result<Value, String> {
        let graph = Self::require_str(args, "graph")?;
        let after = Self::require_str(args, "after")?;
        let kind = Self::require_str(args, "kind")?;
        let door = self.door()?;
        door.json(
            "POST",
            "/graph/node",
            Some(json!({ "op": "insert", "graph": graph, "after": after, "kind": kind })),
        )
        .map_err(|(status, msg)| format!("/graph/node answered {status}: {msg}"))
    }

    fn edit_venue_surface(&self, args: &Value) -> Result<Value, String> {
        let venue = Self::require_str(args, "venue")?;
        let surface = Self::require_str(args, "surface")?;
        let mut body = json!({ "op": "surface", "id": venue, "surface": surface });
        for key in ["name", "points", "origin", "u", "v", "material", "delete"] {
            if let Some(v) = args.get(key) {
                body[key] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// Venue 3D: `edit_venue_object(object, kind?, name?, at?, size?, yaw?,
    /// crown?, occludes?, emits?, faces?, delete?)` — create, edit or delete
    /// one venue Object through `POST /venue { op: "object" }`, the same body
    /// the 3D viewport posts (rule 6). `kind` is free (stage, truss, dummy…);
    /// `venue` is optional and defaults to the one projected last.
    fn edit_venue_object(&self, args: &Value) -> Result<Value, String> {
        let object = Self::require_str(args, "object")?;
        let mut body = json!({ "op": "object", "object": object });
        if let Some(venue) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(venue);
        }
        for key in [
            "kind", "name", "at", "size", "yaw", "crown", "occludes", "emits", "faces", "delete",
        ] {
            if let Some(v) = args.get(key) {
                body[key] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// Venue 3D: `add_screen(screen, at?, yaw?, surface?, venue?)` — a
    /// display plus its glass plane through `POST /venue { op: "screen" }`,
    /// the body the add-screen form posts (rule 6). `aspect_lock` is
    /// `"pixels"` or `"physical"`; the route corrects the other pair.
    fn add_screen(&self, args: &Value) -> Result<Value, String> {
        let mut screen = args
            .get("screen")
            .cloned()
            .filter(Value::is_object)
            .ok_or("add_screen needs `screen` (an object)")?;
        for key in ["at", "yaw"] {
            if let Some(v) = args.get(key) {
                screen[key] = v.clone();
            }
        }
        let mut body = json!({ "op": "screen", "screen": screen });
        if let Some(venue) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(venue);
        }
        if let Some(surface) = args.get("surface").and_then(Value::as_str) {
            body["surface"] = json!(surface);
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// Venue 3D: `add_projection(projector, on, …)` — add or edit one
    /// projector through `POST /venue { op: "projector" }`, the same body the
    /// add-projection form posts (rule 6).
    fn add_projection(&self, args: &Value) -> Result<Value, String> {
        let projector = Self::require_str(args, "projector")?;
        let mut body = json!({ "op": "projector", "projector": projector });
        if let Some(venue) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(venue);
        }
        for key in [
            "name",
            "output",
            "on",
            "lens",
            "yaw",
            "pitch",
            "throw_ratio",
            "aspect",
            "lumens",
        ] {
            if let Some(v) = args.get(key) {
                body[key] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// Venue 3D: `set_surface_form(surface, form, path?, profile?, caps?,
    /// closed?, kind?)` — the parametric form of a surface through
    /// `POST /venue { op: "surface.form" }` (rule 6).
    fn set_surface_form(&self, args: &Value) -> Result<Value, String> {
        let surface = Self::require_str(args, "surface")?;
        let form = Self::require_str(args, "form")?;
        let mut body = json!({ "op": "surface.form", "surface": surface, "form": form });
        if let Some(venue) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(venue);
        }
        for key in ["path", "profile", "caps", "closed", "kind"] {
            if let Some(v) = args.get(key) {
                body[key] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// Venue 3D: `set_venue_sectors(surface, source?, ratio?, points?,
    /// magnet?)` — how one `venue.surface` node's many sources share the
    /// surface, through `POST /venue { op: "surface.sectors" }` (rule 6).
    fn set_venue_sectors(&self, args: &Value) -> Result<Value, String> {
        let surface = Self::require_str(args, "surface")?;
        let mut body = json!({ "op": "surface.sectors", "surface": surface });
        if let Some(venue) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(venue);
        }
        for key in ["source", "ratio", "points", "magnet"] {
            if let Some(v) = args.get(key) {
                body[key] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// Venue 3D: `move_venue_item(item, at?, yaw?, pitch?, roll?)` — moves or
    /// turns any item through `POST /venue { op: "move" }`, the drag route
    /// (rule 6). A projector refuses `at`: it moves with its object.
    fn move_venue_item(&self, args: &Value) -> Result<Value, String> {
        let item = Self::require_str(args, "item")?;
        let mut body = json!({ "op": "move", "item": item });
        if let Some(venue) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(venue);
        }
        for key in ["at", "yaw", "pitch", "roll"] {
            if let Some(v) = args.get(key) {
                body[key] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// Venue 3D: `projection_area(projector)` — the live projected area and
    /// throw through `POST /venue { op: "area" }` (rule 6). A read, but the
    /// venue route is POST, so it posts like the editor's HUD does.
    fn projection_area(&self, args: &Value) -> Result<Value, String> {
        let projector = Self::require_str(args, "projector")?;
        let mut body = json!({ "op": "area", "projector": projector });
        if let Some(venue) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(venue);
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// Venue 3D: `venue_scene(id?)` — `GET /venue/scene?id=`, the scene graph
    /// the 3D viewport reads (objects, fixtures, projectors, sensors,
    /// displays, surfaces, outputs and masks). With no `id`, the venue
    /// projected last.
    fn venue_scene(&self, args: &Value) -> Result<Value, String> {
        let id = args.get("id").and_then(Value::as_str).unwrap_or("last");
        let door = self.door()?;
        let reply = door
            .bytes("GET", "/venue/scene", &[("id", id.to_string())])
            .map_err(|(status, msg)| format!("/venue/scene answered {status}: {msg}"))?;
        serde_json::from_slice(&reply.body)
            .map_err(|e| format!("/venue/scene answered a body that is not JSON: {e}"))
    }

    /// Venue 3D: `add_stage(venue?, stage, name?, at?, size?, path?, profile?,
    /// yaw?, delete?)` — `POST /venue { op: "stage" }`, the same body as
    /// `object` (rule 6). `stage` is the object id.
    fn add_stage(&self, args: &Value) -> Result<Value, String> {
        self.add_venue_kind(args, "stage", "stage")
    }

    /// Venue 3D: `add_truss(...)` — `POST /venue { op: "truss" }`, the same
    /// body as `object` (rule 6).
    fn add_truss(&self, args: &Value) -> Result<Value, String> {
        self.add_venue_kind(args, "truss", "truss")
    }

    /// Venue 3D: `add_dummy(...)` — `POST /venue { op: "dummy" }`; the route
    /// defaults a dummy to a capsule, the same body as `object` (rule 6).
    fn add_dummy(&self, args: &Value) -> Result<Value, String> {
        self.add_venue_kind(args, "dummy", "dummy")
    }

    fn add_venue_kind(&self, args: &Value, op: &str, key: &str) -> Result<Value, String> {
        let object = Self::require_str(args, key)?;
        let mut body = json!({ "op": op, "object": object });
        if let Some(venue) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(venue);
        }
        for k in [
            "kind", "name", "at", "size", "yaw", "delete", "path", "profile",
        ] {
            if let Some(v) = args.get(k) {
                body[k] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// Venue 3D: `venue_spline(venue?, object, path, profile?, kind?, caps?,
    /// closed?)` — a swept object through `POST /venue { op: "object" }`
    /// with `form: { form: "swept", … }` (rule 6).
    fn venue_spline(&self, args: &Value) -> Result<Value, String> {
        let object = Self::require_str(args, "object")?;
        let path = args
            .get("path")
            .cloned()
            .ok_or("venue_spline needs `path` ([x, y, z] points)")?;
        let mut form = json!({ "form": "swept", "path": path });
        for key in ["profile", "kind", "caps", "closed"] {
            if let Some(v) = args.get(key) {
                form[key] = v.clone();
            }
        }
        let mut body = json!({ "op": "object", "object": object, "form": form });
        if let Some(venue) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(venue);
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// Venue 3D: `place_mask_in_space(venue?, mask, on, points, rgb?, alpha?,
    /// parent?, content?, delete?)` — `POST /venue { op: "mask" }` (rule 6).
    fn place_mask_in_space(&self, args: &Value) -> Result<Value, String> {
        let mask = Self::require_str(args, "mask")?;
        let on = Self::require_str(args, "on")?;
        let mut body = json!({ "op": "mask", "mask": mask, "on": on });
        if let Some(venue) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(venue);
        }
        for key in ["points", "rgb", "alpha", "parent", "content", "delete"] {
            if let Some(v) = args.get(key) {
                body[key] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// Venue 3D: `venue_boolean(venue?, a, b, operation, result?)` — rewrites
    /// `result` (or one input) through `POST /venue { op: "boolean" }`.
    fn venue_boolean(&self, args: &Value) -> Result<Value, String> {
        let a = Self::require_str(args, "a")?;
        let b = Self::require_str(args, "b")?;
        let operation = Self::require_str(args, "operation")?;
        let mut body = json!({ "op": "boolean", "a": a, "b": b, "operation": operation });
        if let Some(venue) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(venue);
        }
        if let Some(v) = args.get("result") {
            body["result"] = v.clone();
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// Venue 3D: `venue_hit(venue?, origin, dir?, target?, max?)` — a ray
    /// query through `POST /venue { op: "hit" }` (rule 6). With `target`, the
    /// direction is `normalize(target - origin)`.
    fn venue_hit(&self, args: &Value) -> Result<Value, String> {
        let origin = args
            .get("origin")
            .cloned()
            .ok_or("venue_hit needs `origin` ([x, y, z])")?;
        let dir = match (args.get("dir"), args.get("target"), origin.as_array()) {
            (Some(dir), _, _) => dir.clone(),
            (None, Some(target), Some(o)) => {
                let t = target
                    .as_array()
                    .filter(|t| t.len() == 3)
                    .ok_or("`target` is [x, y, z]")?;
                let d: Vec<f64> = (0..3)
                    .map(|i| {
                        t[i].as_f64().unwrap_or(0.0)
                            - o.get(i).and_then(Value::as_f64).unwrap_or(0.0)
                    })
                    .collect();
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if len <= 1.0e-9 {
                    return Err("`target` equals `origin` — no direction".to_string());
                }
                json!([d[0] / len, d[1] / len, d[2] / len])
            }
            (None, Some(_), None) => return Err("`origin` is [x, y, z]".to_string()),
            (None, None, _) => return Err("venue_hit needs `dir` or `target`".to_string()),
        };
        let mut body = json!({ "op": "hit", "origin": origin, "dir": dir });
        if let Some(venue) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(venue);
        }
        if let Some(v) = args.get("max") {
            body["max"] = v.clone();
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// Venue 3D: `venue_report(venue?)` — `POST /venue { op: "report" }`
    /// (rule 6), the venue's aspect/geometry check.
    fn venue_report(&self, args: &Value) -> Result<Value, String> {
        let mut body = json!({ "op": "report" });
        if let Some(venue) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(venue);
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// Venue 3D: `venue_view(venue?, shot?, width?, height?)` — `POST /venue
    /// { op: "view" }`; the reply is returned verbatim because its ASCII
    /// `rows` are what the model reads (rule 6).
    fn venue_view(&self, args: &Value) -> Result<Value, String> {
        let mut body = json!({ "op": "view" });
        if let Some(venue) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(venue);
        }
        for key in ["shot", "width", "height"] {
            if let Some(v) = args.get(key) {
                body[key] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// Venue 3D: `venue_assets_search(query?, licence?, limit?)` — searches the
    /// local `data/assets/manifest.json` only. There is no remote fetch in the
    /// tree, so a missing manifest is an empty, honest answer.
    fn venue_assets_search(&self, args: &Value) -> Result<Value, String> {
        let path = crate::output::data_root().join("assets/manifest.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(json!({
                "ok": true,
                "assets": [],
                "note": "no local asset manifest",
            }));
        };
        let manifest: Value = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not JSON: {e}", path.display()))?;
        let assets = manifest
            .get("assets")
            .and_then(Value::as_array)
            .or_else(|| manifest.as_array())
            .cloned()
            .unwrap_or_default();
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let licence = args
            .get("licence")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(12)
            .min(32) as usize;

        let matched: Vec<Value> = assets
            .into_iter()
            .filter(|a| {
                if !licence.is_empty() {
                    let l = a["licence"]
                        .as_str()
                        .or_else(|| a["license"].as_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if l != licence {
                        return false;
                    }
                }
                if query.is_empty() {
                    return true;
                }
                let name = a["name"].as_str().unwrap_or("").to_lowercase();
                let id = a["id"].as_str().unwrap_or("").to_lowercase();
                let tags = a["tags"]
                    .as_array()
                    .map(|ts| {
                        ts.iter()
                            .filter_map(Value::as_str)
                            .any(|t| t.to_lowercase().contains(&query))
                    })
                    .unwrap_or(false);
                name.contains(&query) || id.contains(&query) || tags
            })
            .take(limit)
            .collect();
        Ok(json!({ "ok": true, "assets": matched }))
    }

    /// Venue 3D: `venue_asset_import(venue?, object, path, licence, author?,
    /// url?, scale?, at?, name?)` — imports a local glTF through
    /// `metal_geom::load_gltf`, normalises its largest dimension to `scale`
    /// (default 1 m), and posts it as a mesh object with its attribution
    /// (rule 6). Remote fetching is deliberately absent: `path` is local.
    fn venue_asset_import(&self, args: &Value) -> Result<Value, String> {
        const LICENCES: [&str; 7] = [
            "cc0",
            "public domain",
            "cc-by",
            "cc-by-3.0",
            "cc-by-4.0",
            "cc-by-sa",
            "cc-by-sa-4.0",
        ];
        let object = Self::require_str(args, "object")?;
        let path = Self::require_str(args, "path")?;
        let licence = Self::require_str(args, "licence")?.trim();
        if licence.is_empty() {
            return Err("venue_asset_import needs a licence".to_string());
        }
        if !LICENCES.contains(&licence.to_lowercase().as_str()) {
            return Err(format!(
                "unknown licence {licence:?} — use CC0, Public Domain, CC-BY, CC-BY-3.0, CC-BY-4.0, CC-BY-SA or CC-BY-SA-4.0"
            ));
        }

        let mut mesh = metal_geom::load_gltf(Path::new(path))
            .map_err(|e| format!("cannot import {path:?}: {e}"))?;
        let scale = args.get("scale").and_then(Value::as_f64).unwrap_or(1.0) as f32;
        if let Some((min, max)) = mesh.bbox() {
            let largest = (0..3).map(|i| max[i] - min[i]).fold(0.0f32, f32::max);
            if largest > 1.0e-9 && (scale - largest).abs() > 1.0e-9 {
                let k = scale / largest;
                for p in &mut mesh.positions {
                    for c in p {
                        *c *= k;
                    }
                }
            }
            // A glTF's own origin is rarely its base: put the footprint's
            // centre at x/z = 0 and its lowest point on y = 0, so `at` means
            // where it stands, like every other venue object.
            if let Some((min, max)) = mesh.bbox() {
                let (cx, cz) = ((min[0] + max[0]) * 0.5, (min[2] + max[2]) * 0.5);
                for p in &mut mesh.positions {
                    p[0] -= cx;
                    p[1] -= min[1];
                    p[2] -= cz;
                }
            }
        }

        let author = args.get("author").and_then(Value::as_str).unwrap_or("");
        let url = args.get("url").and_then(Value::as_str).unwrap_or("");
        let attribution = [author, licence, url]
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" — ");

        let mut body = json!({
            "op": "object",
            "object": object,
            "form": {
                "form": "mesh",
                "positions": mesh.positions,
                "indices": mesh.indices,
                "uvs": mesh.uvs,
                "normals": mesh.normals,
            },
            "attribution": attribution,
            "source": if url.is_empty() { path } else { url },
        });
        if let Some(venue) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(venue);
        }
        for key in ["at", "name"] {
            if let Some(v) = args.get(key) {
                body[key] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// D1: `set_fixture(venue, id?, kind?, …, delete?)` — add, edit or remove
    /// one fixture through `POST /venue { op: "fixture" }`, the same body the
    /// Settings "Fixtures" page posts (rule 6). `venue` names the venue, `id`
    /// (absent adds) the fixture; every mapping key is optional and
    /// `delete: true` removes it.
    fn set_fixture(&self, args: &Value) -> Result<Value, String> {
        let venue = Self::require_str(args, "venue")?;
        let mut body = json!({ "op": "fixture", "venue": venue });
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
            if let Some(v) = args.get(key) {
                body[key] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// F9: `add_fixture_node(fixture, colour_from?, model?)` — one
    /// `venue.fixture` node for the active venue's fixture, through
    /// `POST /graph/node {op:"add"}` (rule 6): the same route the Settings
    /// "On the graph" button and the canvas post. The node is a **pointer** at
    /// the venue row (`{venue, fixture}`), never a copy; the route places it
    /// near the venue/group node and autowires its `image` from the device's
    /// compose output. `colour_from` picks the colour source (`frame` default,
    /// `model`, `signal`); `model` names a node to wire into the node's
    /// `colour` input through `POST /graph/edge {op:"add"}`.
    fn add_fixture_node(&self, args: &Value) -> Result<Value, String> {
        let fixture = Self::require_str(args, "fixture")?;
        let door = self.door()?;
        let venue = door
            .json("GET", "/venue", None)
            .ok()
            .and_then(|v| v["last"].as_str().map(str::to_string))
            .ok_or("no active venue — project one first")?;
        let colour_from = args["colour_from"].as_str().unwrap_or("frame");
        let mut config = json!({ "venue": venue, "fixture": fixture, "colour_from": colour_from });
        if let Some(pick) = args.get("pick") {
            config["pick"] = pick.clone();
        }
        if let Some(offset) = args.get("offset") {
            config["offset"] = offset.clone();
        }
        let added = door
            .json(
                "POST",
                "/graph/node",
                Some(json!({ "op": "add", "kind": "venue.fixture", "config": config })),
            )
            .map_err(|(status, msg)| format!("/graph/node answered {status}: {msg}"))?;
        let Some(node) = added["node"].as_str().map(str::to_string) else {
            return Ok(added);
        };
        // The model's colour: wire the named node into the new node's `colour`
        // input, the same edge the properties panel's picker posts.
        if let Some(model) = args["model"].as_str() {
            if let Some(graph) = added["graph"]["id"].as_str() {
                let _ = door.json(
                    "POST",
                    "/graph/edge",
                    Some(json!({ "op": "add", "graph": graph,
                                  "from": { "node": model, "port": "colour" },
                                  "to": { "node": node, "port": "colour" } })),
                );
            }
        }
        Ok(json!({ "node": node, "fixture": fixture }))
    }

    /// P3 + M5: `calibrate_projector(venue, projector, surface, pins?, clear?)`.
    ///
    /// With `pins` (or `clear: true`) it is the P3 write — `POST /venue { op:
    /// "pins" }`, the four corners the shared contour editor posts. Without
    /// `pins` it is the M5 solve: the camera reads the projector's Gray code
    /// on the surface, the solve fits the surface→device homography and
    /// evaluates it at the surface's four corners, and the pins land in a
    /// draft (`POST /venue { op: "draft" }`). Same route either way.
    fn calibrate_projector(&self, args: &Value) -> Result<Value, String> {
        let venue = Self::require_str(args, "venue")?;
        let projector = Self::require_str(args, "projector")?;
        let surface = Self::require_str(args, "surface")?;
        let explicit =
            args.get("pins").is_some() || args.get("clear").and_then(Value::as_bool) == Some(true);
        if explicit {
            let mut body = json!({
                "op": "pins", "id": venue, "projector": projector, "surface": surface,
            });
            if args.get("clear").and_then(Value::as_bool) == Some(true) {
                body["clear"] = json!(true);
            } else {
                let pins = args
                    .get("pins")
                    .cloned()
                    .ok_or("`pins` is four [x, y] corners")?;
                body["pins"] = pins;
            }
            let door = self.door()?;
            return door
                .json("POST", "/venue", Some(body))
                .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"));
        }
        let door = self.door()?;
        let (_, mut v) = self.venue_in(door, &json!({ "venue": venue }))?;
        let output = v
            .projectors
            .iter()
            .find(|p| p.id == projector)
            .map(|p| p.output.clone())
            .ok_or_else(|| format!("venue {} has no projector {projector:?}", v.venue.id))?;
        let pins = crate::author::models::camera_solve::pins(&v, &output, surface)?;
        if let Some(proj) = v.projectors.iter_mut().find(|p| p.id == projector) {
            proj.pins.insert(surface.to_string(), pins);
        }
        v.venue.based_on.push(format!(
            "calibrate_projector: {surface} pinned from the camera"
        ));
        let drafted = self.post_draft(door, &v)?;
        Ok(json!({
            "ok": true,
            "venue": v.venue.id,
            "projector": projector,
            "surface": surface,
            "pins": pins,
            "draft": drafted["draft"],
        }))
    }

    /// P-F: `blend_edges(venue, surface? | projector?, enabled?, gamma?,
    /// curve?, black_lift?)` — `POST /venue { op: "blend" }`, the route the
    /// venue editor's "Blend edges" toggle posts.
    fn blend_edges(&self, args: &Value) -> Result<Value, String> {
        let venue = Self::require_str(args, "venue")?;
        let mut body = json!({ "op": "blend", "id": venue });
        for key in [
            "surface",
            "projector",
            "enabled",
            "gamma",
            "curve",
            "black_lift",
        ] {
            if let Some(v) = args.get(key) {
                body[key] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// 3D venues: `project_venue(venue, device?, projector?, rgb?, seconds?,
    /// monitor?)`, the agentic test projection, in three routes:
    /// `POST /venue { op: "project" }` writes the venue's masks onto the
    /// device, `POST /overlay { op: "solid" }` fills each mask with its own
    /// test colour for `seconds` (the venue's `rgb` per mask, which `/venue`
    /// answers per device as `solid`; `rgb` colours the rest of the frame,
    /// black by default, and a device with no venue mask is `rgb` whole,
    /// white by default), and `POST /monitor { op: "open" }`, the call the
    /// `open monitor` button makes, puts the device on its screen.
    /// `monitor: false` skips the last one (a monitor already open keeps
    /// showing the new frame).
    fn project_venue(&self, args: &Value) -> Result<Value, String> {
        let id = Self::require_str(args, "venue")?;
        let mut body = json!({ "op": "project", "id": id });
        for key in ["device", "projector"] {
            if let Some(v) = args.get(key).and_then(Value::as_str) {
                body[key] = json!(v);
            }
        }
        if let Some(add) = args.get("add_outputs").and_then(Value::as_bool) {
            body["add_outputs"] = json!(add);
        }
        let door = self.door()?;
        let projected = door
            .json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))?;
        let rgb = args.get("rgb").cloned();
        let seconds = args.get("seconds").and_then(Value::as_f64).unwrap_or(30.0);
        let monitor = args.get("monitor").and_then(Value::as_bool) != Some(false);
        let mut monitors = Vec::new();
        for d in projected["devices"].as_array().into_iter().flatten() {
            let Some(device) = d["device"].as_str() else {
                continue;
            };
            let mut solid = json!({ "op": "solid", "device": device, "duration_seconds": seconds });
            match d["solid"].as_object().filter(|m| !m.is_empty()) {
                Some(masks) => {
                    solid["masks"] = json!(masks);
                    if let Some(rgb) = &rgb {
                        solid["rgb"] = rgb.clone();
                    }
                }
                None => solid["rgb"] = rgb.clone().unwrap_or_else(|| json!([255, 255, 255])),
            }
            door.json("POST", "/overlay", Some(solid))
                .map_err(|(status, msg)| format!("/overlay answered {status}: {msg}"))?;
            if monitor {
                let opened = door
                    .json(
                        "POST",
                        "/monitor",
                        Some(json!({ "op": "open", "device": device })),
                    )
                    .map_err(|(status, msg)| format!("/monitor answered {status}: {msg}"))?;
                monitors.push(json!({ "device": device, "reply": opened }));
            }
        }
        Ok(json!({
            "ok": true,
            "venue": id,
            "devices": projected["devices"],
            "report": projected["report"],
            "solid": rgb,
            "seconds": seconds,
            "monitors": monitors,
        }))
    }

    /// M2: `aim_fixture(venue?, fixture, target)` — aims a head at a point
    /// through `POST /venue { op: "aim" }`, the same route the venue editor's
    /// click-to-aim will post. `venue` defaults to the one projected last.
    fn aim_fixture(&self, args: &Value) -> Result<Value, String> {
        let fixture = Self::require_str(args, "fixture")?;
        let target = args
            .get("target")
            .cloned()
            .ok_or("aim_fixture needs a `target`: [x, y, z] in metres")?;
        let mut body = json!({ "op": "aim", "fixture": fixture, "target": target });
        if let Some(v) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(v);
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// D2: `import_gdtf(path?, bytes?, name?, mode?)` — a `.gdtf` (zip of XML
    /// + geometry) becomes a `data/fixtures/<model>.toml` profile, through the
    /// same `POST /fixtures { op: "import-gdtf" }` body the operator's import
    /// posts (rule 6). `path` names a file on this machine's disk; `bytes` is
    /// the whole archive as base64 when the caller has no shared filesystem.
    fn import_gdtf(&self, args: &Value) -> Result<Value, String> {
        let mut body = json!({ "op": "import-gdtf" });
        for key in ["path", "bytes", "name", "mode"] {
            if let Some(v) = args.get(key).and_then(Value::as_str) {
                body[key] = json!(v);
            }
        }
        if body.get("path").is_none() && body.get("bytes").is_none() {
            return Err("import_gdtf needs a `path` or `bytes` (the .gdtf)".into());
        }
        let door = self.door()?;
        door.json("POST", "/fixtures", Some(body))
            .map_err(|(status, msg)| format!("/fixtures answered {status}: {msg}"))
    }

    /// M5: the venue a camera tool reads — the one named (`venue`), else the
    /// last projected (`GET /venue`'s `last`), else the first on disk. Read
    /// through the door, not off disk, so the tool and the editor cannot
    /// disagree about which venue they mean.
    fn venue_in(
        &self,
        door: &dyn Call,
        args: &Value,
    ) -> Result<(String, crate::venue::Venue), String> {
        let id = match args.get("venue").and_then(Value::as_str) {
            Some(id) => id.to_string(),
            None => {
                let list = door
                    .json("GET", "/venue", None)
                    .map_err(|(s, m)| format!("/venue answered {s}: {m}"))?;
                list.get("last")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        list["venues"].as_array().and_then(|vs| {
                            vs.iter().find_map(|v| v["id"].as_str().map(str::to_string))
                        })
                    })
                    .ok_or_else(|| "no venue on disk: name one with `venue`".to_string())?
            }
        };
        let read = door
            .bytes("GET", "/venue", &[("id", id.clone())])
            .map_err(|(s, m)| format!("/venue answered {s}: {m}"))?;
        let read: Value = serde_json::from_slice(&read.body)
            .map_err(|e| format!("/venue answered a body that is not JSON: {e}"))?;
        let v: crate::venue::Venue = serde_json::from_value(read["venue"].clone())
            .map_err(|e| format!("/venue answered no venue: {e}"))?;
        Ok((id, v))
    }

    /// Post `v` as a draft the operator accepts or rejects, and answer the
    /// route's reply (`POST /venue { op: "draft" }`, the documented shape:
    /// the venue TOML itself).
    fn post_draft(&self, door: &dyn Call, v: &crate::venue::Venue) -> Result<Value, String> {
        let text = toml::to_string_pretty(v).map_err(|e| e.to_string())?;
        door.json(
            "POST",
            "/venue",
            Some(json!({ "op": "draft", "id": v.venue.id, "venue": text })),
        )
        .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// M5: `scan_venue(venue?, …)` — the venue as the camera reads it. The
    /// projector paints Gray code on each surface, the synthetic feed renders
    /// the bit planes, and the solve fits the surface→device homography; the
    /// pins that come out are written into a draft. A surface no projector
    /// lights is reported, not invented.
    fn scan_venue(&self, args: &Value) -> Result<Value, String> {
        let door = self.door()?;
        let (id, mut v) = self.venue_in(door, args)?;
        let mut pairs = Vec::new();
        for p in &v.projectors {
            for s in &v.surfaces {
                pairs.push((p.id.clone(), p.output.clone(), s.id.clone()));
            }
        }
        let mut scanned = Vec::new();
        for (projector, output, surface) in pairs {
            match crate::author::models::camera_solve::pins(&v, &output, &surface) {
                Ok(pins) => {
                    if let Some(proj) = v.projectors.iter_mut().find(|p| p.id == projector) {
                        proj.pins.insert(surface.clone(), pins);
                    }
                    scanned.push(
                        json!({ "projector": projector, "surface": surface, "points": pins }),
                    );
                }
                Err(reason) => scanned
                    .push(json!({ "projector": projector, "surface": surface, "error": reason })),
            }
        }
        let solved = scanned.iter().filter(|s| s.get("points").is_some()).count();
        v.venue.based_on.push(format!(
            "scan_venue: {solved} surface(s) pinned from the camera"
        ));
        let drafted = self.post_draft(door, &v)?;
        Ok(json!({ "ok": true, "venue": id, "scanned": scanned, "draft": drafted["draft"] }))
    }

    /// M5: `discover_fixture(venue?, …)` — by flashing. Each fixture is lit
    /// alone in the synthetic feed, the camera frame thresholded, and its
    /// bright blob is the fixture; the discovery is the result, and a draft
    /// the operator can accept keeps the venue itself unchanged.
    fn discover_fixture(&self, args: &Value) -> Result<Value, String> {
        let door = self.door()?;
        let (id, v) = self.venue_in(door, args)?;
        let found = crate::author::models::camera_solve::discover(&v)?;
        let drafted = self.post_draft(door, &v)?;
        Ok(json!({
            "ok": true,
            "venue": id,
            "discovered": found
                .iter()
                .map(|(id, at)| json!({ "fixture": id, "at": at }))
                .collect::<Vec<_>>(),
            "draft": drafted["draft"],
        }))
    }

    /// M5: the positive `capture_camera_frame` — with no `device`, the seam
    /// renders the venue's room (the last projected, else the first on disk)
    /// and hands back a real PNG. With a `device` named, or with no venue to
    /// point the synthetic room at, the inner harness asks the real camera and
    /// reports the room honestly.
    fn capture_camera_frame(&self, args: &Value) -> Result<Value, String> {
        if args.get("device").and_then(Value::as_str).is_some() {
            return self.inner_execute("capture_camera_frame", args.clone());
        }
        let Some(room) = self.venue_for_camera() else {
            return self.inner_execute("capture_camera_frame", args.clone());
        };
        let w = args
            .get("w")
            .and_then(Value::as_u64)
            .unwrap_or(crate::author::models::camera_solve::CAPTURE_W as u64)
            as u32;
        let h = args
            .get("h")
            .and_then(Value::as_u64)
            .unwrap_or(crate::author::models::camera_solve::CAPTURE_H as u64)
            as u32;
        let camera = crate::author::models::camera_solve::SyntheticCamera::for_venue(&room);
        let frame = camera.capture(&room, w, h)?;
        let png = frame.to_png()?;
        Ok(json!({
            "ok": true,
            "device": "synthetic",
            "png_base64": metal_agent::harness::base64_encode(&png),
            "w": w,
            "h": h,
        }))
    }

    /// The venue `capture_camera_frame` renders, through the door: the last
    /// projected, else the first on disk. `None` when no door is attached or
    /// no venue exists — the caller then falls through to the real camera.
    fn venue_for_camera(&self) -> Option<crate::venue::Venue> {
        let door = self.door.as_deref()?;
        let list = door.json("GET", "/venue", None).ok()?;
        let id = list
            .get("last")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                list["venues"]
                    .as_array()?
                    .iter()
                    .find_map(|v| v["id"].as_str().map(str::to_string))
            })?;
        let read = door.bytes("GET", "/venue", &[("id", id)]).ok()?;
        let body: Value = serde_json::from_slice(&read.body).ok()?;
        serde_json::from_value(body["venue"].clone()).ok()
    }

    /// D5: `solve_head(venue?, fixture, target)` — point a head at a real 3D
    /// point from wherever it is hung, through `POST /venue { op:
    /// "solve_head" }`, the same route the venue editor's solve posts. The
    /// route inverts the aim within the profile's limits and slew and answers
    /// the live state plus the beam's landing point; the AI reads it to
    /// judge whether the head reaches what it means to light.
    fn solve_head(&self, args: &Value) -> Result<Value, String> {
        let fixture = Self::require_str(args, "fixture")?;
        let target = args
            .get("target")
            .cloned()
            .ok_or("solve_head needs a `target`: [x, y, z] in metres")?;
        let mut body = json!({ "op": "solve_head", "fixture": fixture, "target": target });
        if let Some(v) = args.get("venue").and_then(Value::as_str) {
            body["id"] = json!(v);
        }
        let door = self.door()?;
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))
    }

    /// 3D venues, the read half: `look_venue(venue?, shot?, device?,
    /// camera?, design?, w?, h?)`. What the AI needs to judge whether what it
    /// meant is what the room shows, in one call:
    ///
    /// - **the scene it expects** — the venue (`GET /venue?id=`, `last` when
    ///   none is named): its surfaces, objects, displays, projectors and
    ///   masks, what each mask should play, and the check's report;
    /// - **what is being rendered** — each output's live frame (`GET
    ///   /frame`, the frame the monitor window shows), with the legend of
    ///   the masks cutting it;
    /// - **what the room shows** — a fresh camera frame
    ///   (`capture_camera_frame`, the same capture the tool of that name
    ///   makes) and the same frame with every mask drawn where the venue's
    ///   geometry says that camera should see it. Lit shapes inside their
    ///   outlines: the intention landed. Light outside them, or outlines on
    ///   dark wall: it did not, and `problems`/`notes` say where to look.
    ///
    /// The camera is the shot's own `device` (or `device`, or the first that
    /// answers); `camera: false` skips it. A photographed shot's source image
    /// comes back too (`design`), the picture the venue was designed from.
    /// `capture_output { device?, node, frame?, w?, h? }`: one graph node drawn
    /// alone at a frame. With `device`, through
    /// `GET /render/<device>/<node>/<frame>`; without, through
    /// `GET /render/<node>/<frame>`, where the route finds the device whose
    /// graph holds the node. The same route a peer's `fw/frame/1` answers
    /// with, and the one `/node.jpg`/"Export frame" read. `frame` defaults
    /// to 0.
    fn capture_output(&self, args: &Value) -> Result<Value, String> {
        let node = args
            .get("node")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("capture_output needs node (a graph node id)")?;
        let door = self.door()?;
        let frame = args.get("frame").and_then(Value::as_u64).unwrap_or(0);
        let w = args.get("w").and_then(Value::as_u64).unwrap_or(1280);
        let h = args.get("h").and_then(Value::as_u64).unwrap_or(720);
        let path = match args
            .get("device")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            Some(device) => format!("/render/{device}/{node}/{frame}"),
            None => format!("/render/{node}/{frame}"),
        };
        let reply = door
            .bytes("GET", &path, &[("w", w.to_string()), ("h", h.to_string())])
            .map_err(|(status, msg)| format!("/render answered {status}: {msg}"))?;
        Ok(json!({
            "node": node,
            "frame": frame,
            "jpeg_base64": metal_agent::harness::base64_encode(&reply.body),
        }))
    }

    fn look_venue(&self, args: &Value) -> Result<Value, String> {
        let door = self.door()?;
        let id = args.get("venue").and_then(Value::as_str).unwrap_or("last");
        let read = door
            .bytes("GET", "/venue", &[("id", id.to_string())])
            .map_err(|(status, msg)| format!("/venue answered {status}: {msg}"))?;
        let read: Value = serde_json::from_slice(&read.body)
            .map_err(|e| format!("/venue answered a body that is not JSON: {e}"))?;
        let v: crate::venue::Venue = serde_json::from_value(read["venue"].clone())
            .map_err(|e| format!("/venue answered no venue: {e}"))?;
        let (w, h) = (
            args.get("w").and_then(Value::as_u64).unwrap_or(640) as u32,
            args.get("h").and_then(Value::as_u64).unwrap_or(360) as u32,
        );

        // Legend: one row per mask, numbered as drawn on the camera overlay.
        let colour =
            |i: usize, m: &crate::venue::VenueMask| m.rgb.unwrap_or(crate::output::mask_color(i));
        let legend: Vec<Value> = v
            .masks
            .iter()
            .enumerate()
            .map(|(i, m)| {
                json!({ "n": i + 1, "id": m.id, "name": m.name, "on": m.on,
                        "rgb": colour(i, m), "content": m.content })
            })
            .collect();

        // What each output renders right now.
        let mut outputs = Vec::new();
        for o in v.outputs() {
            let mut row = json!({
                "device": o,
                "driven_by": v.projectors.iter().find(|p| p.output == o).map(|p| format!("projector {}", p.id))
                    .or_else(|| v.displays.iter().find(|d| d.output == o).map(|d| format!("display {}", d.id))),
                "masks": read["outputs"].as_array().and_then(|a| a.iter().find(|x| x["output"] == json!(o)))
                    .map(|x| x["masks"].as_array().map(|m| m.len()).unwrap_or(0)).unwrap_or(0),
            });
            match door.bytes(
                "GET",
                "/frame",
                &[
                    ("device", o.clone()),
                    ("w", w.to_string()),
                    ("h", h.to_string()),
                ],
            ) {
                Ok(reply) => {
                    row["jpeg_base64"] = json!(metal_agent::harness::base64_encode(&reply.body))
                }
                Err((status, msg)) => {
                    row["error"] = json!(format!("/frame answered {status}: {msg}"))
                }
            }
            outputs.push(row);
        }

        // The shot the camera stands in for.
        let shot = match args.get("shot").and_then(Value::as_str) {
            Some(sid) => Some(
                v.shot(sid)
                    .ok_or_else(|| format!("venue {} has no shot {sid:?}", v.venue.id))?,
            ),
            None => v
                .shots
                .iter()
                .find(|s| s.device.is_some())
                .or(v.shots.first()),
        };
        let mut camera = Value::Null;
        if let (Some(shot), true) = (
            shot,
            args.get("camera").and_then(Value::as_bool) != Some(false),
        ) {
            let mut outlines: Vec<metal_agent::locate::Outline> = v
                .masks_in(shot)
                .into_iter()
                .filter_map(|(mid, points)| {
                    let i = v.masks.iter().position(|m| m.id == mid)?;
                    Some(metal_agent::locate::Outline {
                        points,
                        rgb: colour(i, &v.masks[i]),
                        label: i as u32 + 1,
                    })
                })
                .collect();
            // Each display's glass, in white, numbered after the masks.
            for (k, d) in v.displays.iter().enumerate() {
                if let Some(q) = v.corners_in(shot, &d.surface) {
                    outlines.push(metal_agent::locate::Outline {
                        points: q.to_vec(),
                        rgb: [255, 255, 255],
                        label: (v.masks.len() + k + 1) as u32,
                    });
                }
            }
            let expected: Vec<Value> = outlines
                .iter()
                .map(|o| json!({ "n": o.label, "points": o.points }))
                .collect();
            let mut capture = json!({ "w": w.max(320), "h": h.max(240) });
            if let Some(dev) = args
                .get("device")
                .and_then(Value::as_str)
                .or(shot.device.as_deref())
            {
                capture["device"] = json!(dev);
            }
            let grabbed = self.inner_execute("capture_camera_frame", capture)?;
            camera = json!({ "shot": shot.id, "expected": expected });
            match grabbed["png_base64"]
                .as_str()
                .map(metal_agent::harness::base64_decode)
            {
                Some(Ok(png)) => {
                    camera["device"] = grabbed["device"].clone();
                    camera["frame"] =
                        json!({ "png_base64": metal_agent::harness::base64_encode(&png) });
                    match metal_agent::locate::draw_outlines(&png, &outlines) {
                        Ok(over) => {
                            camera["expected_over_frame"] =
                                json!({ "png_base64": metal_agent::harness::base64_encode(&over) })
                        }
                        Err(e) => camera["overlay_error"] = json!(e),
                    }
                }
                _ => camera["error"] = grabbed["error"].clone(),
            }
        }

        // The picture the venue was designed from, when the shot has one.
        let mut design = Value::Null;
        if let (Some(shot), true) = (
            shot,
            args.get("design").and_then(Value::as_bool) != Some(false),
        ) {
            if !shot.image.is_empty() {
                let root = crate::output::data_root();
                let found = [
                    root.join(&shot.image),
                    root.join("..").join(&shot.image),
                    root.join("../..").join(&shot.image),
                ]
                .into_iter()
                .find(|p| metal_vfs::is_file(&p));
                design = match found.and_then(|p| metal_vfs::read(&p).ok().map(|b| (p, b))) {
                    Some((p, bytes)) => {
                        let key = if p.extension().is_some_and(|e| e == "png") {
                            "png_base64"
                        } else {
                            "jpeg_base64"
                        };
                        json!({ "image": shot.image, key: metal_agent::harness::base64_encode(&bytes) })
                    }
                    None => json!({ "image": shot.image, "missing": true }),
                };
            }
        }

        let problems: Vec<Value> = read["report"]["problems"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .take(8)
            .collect();
        // M6: the preview shows every fixture with its live state — the
        // commanded pan/tilt beside the estimated actual one, the dimmer and
        // the colour — read from `GET /venue/state` (M2).
        let states = door
            .bytes("GET", "/venue/state", &[("id", v.venue.id.clone())])
            .ok()
            .and_then(|r| serde_json::from_slice::<Value>(&r.body).ok());
        let fixtures: Vec<Value> = v
            .fixtures
            .iter()
            .map(|f| {
                let state = states
                    .as_ref()
                    .and_then(|s| s["fixtures"].as_array())
                    .and_then(|a| a.iter().find(|x| x["fixture"] == json!(f.id)))
                    .cloned()
                    .unwrap_or(Value::Null);
                json!({
                    "id": f.id, "name": f.name, "kind": f.kind.word(), "profile": f.profile,
                    "group": f.group, "wire": f.wire, "at": f.at, "yaw": f.yaw, "pitch": f.pitch,
                    "state": state,
                })
            })
            .collect();
        Ok(json!({
            "ok": true,
            "venue": { "id": v.venue.id, "name": v.venue.name, "setting": v.venue.setting, "notes": v.venue.notes },
            "scene": {
                "surfaces": v.surfaces.iter().map(|x| json!({ "id": x.id, "name": x.name, "material": x.material })).collect::<Vec<_>>(),
                "objects": v.objects.iter().map(|o| json!({ "id": o.id, "name": o.name, "kind": o.kind, "faces": o.faces })).collect::<Vec<_>>(),
                "projectors": v.projectors.iter().map(|p| json!({ "id": p.id, "output": p.output, "on": p.on,
                    "lands_on": v.throw(p).ok().flatten().map(|t| t.0), "distance": v.throw(p).ok().flatten().map(|t| t.1) })).collect::<Vec<_>>(),
                "displays": v.displays.iter().map(|d| json!({ "id": d.id, "kind": d.kind, "output": d.output, "link": d.link })).collect::<Vec<_>>(),
                "compositions": read["compositions"],
            },
            "masks": legend,
            "fixtures": fixtures,
            "report": { "errors": read["report"]["problems"].as_array().map(|a| a.iter().filter(|p| p["level"] == "error").count()),
                        "problems": problems },
            "outputs": outputs,
            "camera": camera,
            "design": design,
            "how_to_read": "Each output's frame is what is being rendered now. `camera.expected_over_frame` is a live camera frame with every mask drawn, numbered as in `masks`, where the venue's geometry says this camera should see it (white: a display's glass). Lit shapes inside their outlines: the intention landed. Light outside its outline, or an outline on unlit wall: it did not — move the mask (`edit_mask`) or fix the venue and project again. Each `fixtures[]` row carries its live `state` (commanded vs estimated pan/tilt), so a beam can be checked where it is, not only where it was authored.",
        }))
    }

    /// `listen_status()`: `GET /listen/status`, the route the chrome
    /// indicator reads — every `audio.live`'s listener, `real: false` being
    /// the stand-in (plan "Audio input is a node", Design 8). Each row also
    /// carries a `tone` object — `{loaded, real, latency_ms}`, the Basic Pitch
    /// model's state (the tone design notes §6 step 11): `loaded`/`real` are false with no
    /// model or the `tone` feature off, and the colour then falls back to
    /// chroma (D8) — as it does for an old song with no tone columns.
    fn listen_status(&self, _args: &Value) -> Result<Value, String> {
        self.door()?
            .json("GET", "/listen/status", None)
            .map_err(|(status, msg)| format!("/listen/status answered {status}: {msg}"))
    }

    /// `edit_layer(op, device, layer?, model?, …)`: `POST /layers`, the
    /// route the layer table posts, body for body — `add`, `set`, `route`
    /// (`on: false` takes it off a device), `remove`, `raise`, `lower`.
    /// `model` may be a model id (`"forge"`, `"aurora"`) instead of the
    /// table index the route takes; it is resolved against
    /// `data/schema.json`, whose order is the table's.
    fn edit_layer(&self, args: &Value) -> Result<Value, String> {
        let Some(obj) = args.as_object() else {
            return Err("expected an object".to_string());
        };
        let mut body = Value::Object(obj.clone());
        Self::require_str(args, "op")?;
        Self::require_str(args, "device")?;
        if let Some(id) = args.get("model").and_then(Value::as_str) {
            let index = self
                .manifest
                .models
                .iter()
                .position(|m| m.id == id)
                .ok_or_else(|| format!("no model {id:?} in data/schema.json"))?;
            body["model"] = json!(index);
        }
        let door = self.door()?;
        let reply = door
            .json("POST", "/layers", Some(body))
            .map_err(|(status, msg)| format!("/layers answered {status}: {msg}"))?;
        let device = args["device"].as_str().unwrap_or_default();
        let layers = reply["devices"]
            .as_array()
            .and_then(|ds| ds.iter().find(|d| d["id"].as_str() == Some(device)))
            .map(|d| d["layers"].clone())
            .unwrap_or(Value::Null);
        // Where the layer sits in the device's graph, so the next call can
        // splice an effect after it (`create_node {graph, kind, after}`).
        let lid = reply["layer"]["id"]
            .as_str()
            .or(args["layer"].as_str())
            .map(str::to_string);
        let graph = format!("device-{device}");
        let node = lid.as_deref().and_then(|lid| {
            let g: Value = door
                .bytes("GET", "/graph", &[("id", graph.clone())])
                .ok()
                .and_then(|r| serde_json::from_slice(&r.body).ok())?;
            g["graph"]["nodes"]
                .as_array()?
                .iter()
                .find(|n| {
                    let cfg = match &n["config"] {
                        Value::String(text) => serde_json::from_str(text).unwrap_or(Value::Null),
                        other => other.clone(),
                    };
                    cfg["source_id"].as_str() == Some(lid)
                })
                .and_then(|n| n["id"].as_str().map(str::to_string))
        });
        Ok(json!({
            "ok": true,
            "device": device,
            "layer": reply["layer"],
            "graph": graph,
            "node": node,
            "layers": layers,
        }))
    }

    /// Runs one tool on the wrapped [`metal_agent::harness::Harness`] — for a
    /// tool here that is built out of another (`look_venue` captures the
    /// camera the way `capture_camera_frame` does).
    fn inner_execute(&self, name: &str, arguments: Value) -> Result<Value, String> {
        let call = ToolCall {
            id: format!("inner-{name}"),
            name: name.to_string(),
            arguments,
        };
        // `Harness::execute` takes `&mut self` for the few tools that keep
        // state between calls; `capture_camera_frame` keeps none, so a clone
        // of the wrapped harness answers exactly what it would.
        let mut inner = self.inner.clone();
        inner.execute(&call)
    }

    /// `S.AK`/`O.28`: `list_switches()` — every saved switch and its current
    /// `on`, verbatim off `GET /switch` (the same read the grid's own
    /// buttons make, rule 4: on/off is a read of the graph, not a stored
    /// bit).
    fn list_switches(&self, _args: &Value) -> Result<Value, String> {
        let door = self.door()?;
        door.json("GET", "/switch", None)
            .map_err(|(status, msg)| format!("/switch answered {status}: {msg}"))
    }

    /// `S.AK`/`O.28`: `set_switch(id, op)` — `op` is `"on"` or `"off"` (a
    /// press, for `hold`, or the whole flip, for `toggle`); authoring lives
    /// in the two tools below (`R22.4`'s rule 6), so this one stays a
    /// pure trigger.
    fn set_switch(&self, args: &Value) -> Result<Value, String> {
        let id = Self::require_str(args, "id")?;
        let op = match args.get("op").and_then(Value::as_str) {
            Some(op @ ("on" | "off")) => op,
            Some(other) => return Err(format!("op must be \"on\" or \"off\", got {other:?}")),
            None => return Err("expected `op` (\"on\" or \"off\")".to_string()),
        };
        let door = self.door()?;
        door.json("POST", "/switch", Some(json!({ "op": op, "id": id })))
            .map_err(|(status, msg)| format!("/switch answered {status}: {msg}"))
    }

    /// The sync read tools: one `GET /sync/*` through the door, answered as
    /// the route answers it (`running` plus the list). The same route the
    /// Devices page and the chrome indicator read.
    /// P-D: `GET /artnet/nodes`, `ms`/`target` passed as the query.
    fn artnet_in_status(&self, args: &Value) -> Result<Value, String> {
        let mut q = Vec::new();
        for key in ["net", "subnet", "universe", "first", "count"] {
            if let Some(v) = args[key].as_u64() {
                q.push(format!("{key}={v}"));
            }
        }
        let path = if q.is_empty() {
            "/artnet/in".to_string()
        } else {
            format!("/artnet/in?{}", q.join("&"))
        };
        self.sync_read(&path)
    }

    fn artnet_nodes(&self, args: &Value) -> Result<Value, String> {
        let mut q = Vec::new();
        if let Some(ms) = args["ms"].as_u64() {
            q.push(format!("ms={ms}"));
        }
        if let Some(t) = args["target"].as_str().filter(|t| !t.trim().is_empty()) {
            q.push(format!("target={}", t.trim()));
        }
        let path = if q.is_empty() {
            "/artnet/nodes".to_string()
        } else {
            format!("/artnet/nodes?{}", q.join("&"))
        };
        self.sync_read(&path)
    }

    /// P-E: `GET /transitions` — the presets the properties picker lists.
    fn list_transitions(&self, _args: &Value) -> Result<Value, String> {
        self.sync_read("/transitions")
    }

    fn sync_read(&self, path: &str) -> Result<Value, String> {
        let door = self.door()?;
        door.json("GET", path, None)
            .map_err(|(status, msg)| format!("{path} answered {status}: {msg}"))
    }

    /// V2b: the active tick level, read off `GET /settings` — the same door
    /// the settings overlay reads, so the agent and the panel cannot disagree.
    fn get_tick_level(&self, _args: &Value) -> Result<Value, String> {
        let door = self.door()?;
        let reply = door
            .json("GET", "/settings", None)
            .map_err(|(status, msg)| format!("/settings answered {status}: {msg}"))?;
        Ok(json!({ "tick_level": reply["tick_level"] }))
    }

    /// V2b: set the tick level (`base` | `fine`) through the same
    /// `POST /settings` the control posts. The route validates the
    /// `metal_schema::TickRate` vocabulary, so an unknown level is refused there.
    fn set_tick_level(&self, args: &Value) -> Result<Value, String> {
        let level = Self::require_str(args, "tick_level")?;
        let door = self.door()?;
        let reply = door
            .json("POST", "/settings", Some(json!({ "tick_level": level })))
            .map_err(|(status, msg)| format!("/settings answered {status}: {msg}"))?;
        Ok(json!({ "tick_level": reply["tick_level"] }))
    }

    /// P-C (rule 6): `GET /videoio`, what the "Send to" row lists.
    fn list_video_io(&self) -> Result<Value, String> {
        self.door()?
            .json("GET", "/videoio", None)
            .map_err(|(status, msg)| format!("/videoio answered {status}: {msg}"))
    }

    /// P-C (rule 6): `POST /outputs {op:"sink"}`, the body the "Send to"
    /// dropdown posts.
    fn set_output_sink(&self, args: &Value) -> Result<Value, String> {
        let device_id = Self::require_str(args, "device_id")?;
        let kind = args.get("kind").and_then(Value::as_str).unwrap_or_default();
        let target = args
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let body = json!({ "op": "sink", "id": device_id, "kind": kind, "target": target });
        let reply = self
            .door()?
            .json("POST", "/outputs", Some(body))
            .map_err(|(status, msg)| format!("/outputs answered {status}: {msg}"))?;
        let sink = reply["devices"]
            .as_array()
            .and_then(|ds| ds.iter().find(|d| d["id"].as_str() == Some(device_id)))
            .map(|d| d["sink"].clone())
            .unwrap_or(Value::Null);
        Ok(json!({ "ok": true, "device_id": device_id, "sink": sink }))
    }

    /// D3 (rule 6): `dmx_bus(priority?, mode?, device?, node?)` — the same
    /// `POST /outputs {op:"dmx-bus"}` the bus config uses, so the AI edits a
    /// source's merge priority/mode by exactly the route a panel would.
    fn dmx_bus(&self, args: &Value) -> Result<Value, String> {
        let mut body = json!({ "op": "dmx-bus" });
        for key in ["priority", "mode", "device", "node"] {
            if let Some(v) = args.get(key) {
                if !v.is_null() {
                    body[key] = v.clone();
                }
            }
        }
        self.door()?
            .json("POST", "/outputs", Some(body))
            .map_err(|(status, msg)| format!("/outputs answered {status}: {msg}"))
    }

    /// P-G (rule 6): "Export frame" over `POST /frame/export`, the route
    /// the floating preview's camera button posts.
    fn export_frame(&self, args: &Value) -> Result<Value, String> {
        let device = args
            .get("device")
            .and_then(Value::as_str)
            .ok_or_else(|| "expected `device`".to_string())?;
        let mut body = json!({ "device": device });
        for key in ["node", "dir"] {
            if let Some(v) = args.get(key).and_then(Value::as_str) {
                body[key] = json!(v);
            }
        }
        let door = self.door()?;
        door.json("POST", "/frame/export", Some(body))
            .map_err(|(status, msg)| format!("/frame/export answered {status}: {msg}"))
    }

    /// E1/E3 (rule 6): the offline video export over `POST /export/video`, the
    /// same route the export control posts. `range` is `[start, end)` in
    /// frames; `node`, `fps`, `codec`, `bitrate` and `audio` (`"mix"`,
    /// `"file:<path>"` or null) are the route's own optional fields.
    fn export_video(&self, args: &Value) -> Result<Value, String> {
        let device = args
            .get("device")
            .and_then(Value::as_str)
            .ok_or_else(|| "expected `device`".to_string())?;
        let range = args
            .get("range")
            .cloned()
            .ok_or_else(|| "expected `range` [start,end]".to_string())?;
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "expected `path`".to_string())?;
        let mut body = json!({ "device": device, "range": range, "path": path });
        for key in ["node", "fps", "codec", "bitrate", "audio"] {
            if let Some(v) = args.get(key) {
                body[key] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/export/video", Some(body))
            .map_err(|(status, msg)| format!("/export/video answered {status}: {msg}"))
    }

    /// H5 (rule 6): the head-tracking gesture over `POST /sensor {op:"set"}`,
    /// the same route the `sensor.yolo` panel's controls post. `on` turns the
    /// `cam.track` nodes on or off; `strength` (0..1) is optional, so "just
    /// turn it off" is a legal call.
    fn set_head_tracking(&self, args: &Value) -> Result<Value, String> {
        let on = args
            .get("on")
            .and_then(Value::as_bool)
            .ok_or_else(|| "expected `on` (true|false)".to_string())?;
        let mut body = json!({ "op": "set", "on": on });
        if let Some(strength) = args.get("strength") {
            let s = strength
                .as_f64()
                .ok_or_else(|| "`strength` must be a number".to_string())?;
            body["strength"] = json!(s);
        }
        let door = self.door()?;
        door.json("POST", "/sensor", Some(body))
            .map_err(|(status, msg)| format!("/sensor answered {status}: {msg}"))
    }

    /// `R22.4` (rule 6): `add_switch(id, kind, graph, set, name?, unset?,
    /// fixture?, group?)` — the agent's authoring half over `POST /switch
    /// {op:"add"}`, the exact body the grid's `+ create button` popover writes
    /// (`name` defaults to `id`, `unset` is the inverse batch and may be left
    /// out). G1's `fixture`/`rig` kinds carry their target the same way.
    fn add_switch(&self, args: &Value) -> Result<Value, String> {
        let id = Self::require_str(args, "id")?;
        let kind = match args.get("kind").and_then(Value::as_str) {
            Some(k @ ("toggle" | "hold" | "fixture" | "rig")) => k,
            Some(other) => {
                return Err(format!(
                    "kind must be \"toggle\", \"hold\", \"fixture\" or \"rig\", got {other:?}"
                ))
            }
            None => {
                return Err(
                    "expected `kind` (\"toggle\", \"hold\", \"fixture\" or \"rig\")".to_string(),
                )
            }
        };
        let graph = Self::require_str(args, "graph")?;
        let set = args
            .get("set")
            .and_then(Value::as_array)
            .ok_or_else(|| "expected `set` (array of {node, on})".to_string())?
            .clone();
        for entry in &set {
            if entry["node"].as_str().is_none() {
                return Err("every set entry needs a `node`".to_string());
            }
            if entry["on"].as_bool().is_none() {
                return Err("every set entry needs `on` (true|false)".to_string());
            }
        }
        let name = args.get("name").and_then(Value::as_str).unwrap_or(id);
        let unset = args.get("unset").cloned().unwrap_or(Value::Null);
        let mut body = json!({ "op": "add", "id": id, "name": name, "kind": kind, "graph": graph, "set": set, "unset": unset });
        for field in ["fixture", "group", "mode"] {
            if let Some(v) = args.get(field) {
                body[field] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/switch", Some(body))
            .map_err(|(status, msg)| format!("/switch answered {status}: {msg}"))
    }

    /// `R22.4` (rule 6): `remove_switch(id)` — the same `POST /switch
    /// {op:"remove"}` the grid's right-click fires.
    fn remove_switch(&self, args: &Value) -> Result<Value, String> {
        let id = Self::require_str(args, "id")?;
        let door = self.door()?;
        door.json("POST", "/switch", Some(json!({ "op": "remove", "id": id })))
            .map_err(|(status, msg)| format!("/switch answered {status}: {msg}"))
    }

    /// K (rule 6): `set_palette(id, slot, value, relative?)` — moves one slot
    /// of a stored palette through `POST /author/palettes {op:"set"}`, the
    /// exact body the palette node's folder posts. With `relative` true the
    /// whole palette shifts by the same OklCh offset
    /// (`metal_config::palettes::shift`), so the operator's relational gesture
    /// and the model's call are one path. `slot` is a `0..n` index (the stored
    /// order `base, accent, background, flash, extra…`) and `value` an `[r,g,b]`
    /// triple in `0..1`.
    fn set_palette(&self, args: &Value) -> Result<Value, String> {
        let id = Self::require_str(args, "id")?;
        let slot = args
            .get("slot")
            .and_then(Value::as_u64)
            .ok_or_else(|| "expected `slot` (an index >= 0)".to_string())?;
        let value = args
            .get("value")
            .and_then(Value::as_array)
            .ok_or_else(|| "expected `value` ([r, g, b], 0..1)".to_string())?;
        if value.len() != 3 {
            return Err("`value` is three floats, r g b".to_string());
        }
        let relative = args
            .get("relative")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let door = self.door()?;
        door.json(
            "POST",
            "/author/palettes",
            Some(json!({ "op": "set", "id": id, "slot": slot, "value": value, "relative": relative })),
        )
        .map_err(|(status, msg)| format!("/author/palettes answered {status}: {msg}"))
    }

    /// M4 of the materials batch (rule 6): `set_material(graph, node, part?,
    /// mix?, …)` writes a `mat.*` node's `part` and its fixed overrides through
    /// `POST /graph/node {op:"config"}`, the exact merge the node's properties
    /// page posts. Only the keys the call names are written, so "just set the
    /// part" leaves the knobs alone.
    fn set_material(&self, args: &Value) -> Result<Value, String> {
        let graph = Self::require_str(args, "graph")?;
        let node = Self::require_str(args, "node")?;
        let mut config = serde_json::Map::new();
        if let Some(part) = args.get("part").and_then(Value::as_str) {
            config.insert("part".to_string(), json!(part));
        }
        for (key, _) in metal_graph::materials::CONFIG_DEFAULTS {
            if let Some(v) = args.get(*key).and_then(Value::as_f64) {
                config.insert((*key).to_string(), json!(v));
            }
        }
        if config.is_empty() {
            return Err("set_material needs `part` or one of the material overrides".to_string());
        }
        let body = json!({
            "op": "config", "graph": graph, "node": node,
            "config": Value::Object(config),
        });
        let reply = self
            .door()?
            .json("POST", "/graph/node", Some(body))
            .map_err(|(status, msg)| format!("/graph/node answered {status}: {msg}"))?;
        let _ = reply;
        Ok(json!({ "ok": true, "node": node }))
    }

    /// `list_scenes(clip?)` — a video's scenes (`GET /author/scenes?clip=`),
    /// or the book of kept ones with no `clip`; the gallery's own read.
    fn list_scenes(&self, args: &Value) -> Result<Value, String> {
        let door = self.door()?;
        let clip = args.get("clip").and_then(Value::as_str).unwrap_or_default();
        let query: Vec<(&str, String)> = if clip.is_empty() {
            Vec::new()
        } else {
            vec![("clip", clip.to_string())]
        };
        let reply = door
            .bytes("GET", "/author/scenes", &query)
            .map_err(|(status, msg)| format!("/author/scenes answered {status}: {msg}"))?;
        serde_json::from_slice(&reply.body).map_err(|e| format!("/author/scenes: {e}"))
    }

    /// `list_dataset(part?)` — the Colourist dataset's parts (`GET
    /// /author/dataset`), or one part whole (`GET /author/dataset/example`):
    /// the reads behind the sequence editor's dataset picker.
    fn list_dataset(&self, args: &Value) -> Result<Value, String> {
        let door = self.door()?;
        let part = args.get("part").and_then(Value::as_str).unwrap_or_default();
        let (path, query): (&str, Vec<(&str, String)>) = if part.is_empty() {
            ("/author/dataset", Vec::new())
        } else {
            ("/author/dataset/example", vec![("part", part.to_string())])
        };
        let reply = door
            .bytes("GET", path, &query)
            .map_err(|(status, msg)| format!("{path} answered {status}: {msg}"))?;
        serde_json::from_slice(&reply.body).map_err(|e| format!("{path}: {e}"))
    }

    /// `edit_scene(op, …)` — the body the gallery and the Scenes route post
    /// to `POST /author/scenes`, verbatim.
    fn edit_scene(&self, args: &Value) -> Result<Value, String> {
        Self::require_str(args, "op")?;
        let door = self.door()?;
        door.json("POST", "/author/scenes", Some(args.clone()))
            .map_err(|(status, msg)| format!("/author/scenes answered {status}: {msg}"))
    }

    /// `read_script(graph, node)` — a `script.*` node's `{lang, source}` through
    /// `GET /script/<node>?graph=`, the same read the properties editor makes
    /// (rule 6: one route, human and AI).
    fn read_script(&self, args: &Value) -> Result<Value, String> {
        let graph = Self::require_str(args, "graph")?;
        let node = Self::require_str(args, "node")?;
        let door = self.door()?;
        let reply = door
            .bytes(
                "GET",
                &format!("/script/{node}"),
                &[("graph", graph.to_string())],
            )
            .map_err(|(status, msg)| format!("/script answered {status}: {msg}"))?;
        serde_json::from_slice(&reply.body).map_err(|e| format!("/script: {e}"))
    }

    /// `save_script(graph, node, source)` — the one writer: `POST /script`,
    /// which re-runs the source through the engine's runtime before it writes.
    /// A source that raises, hangs or fails to compile comes back
    /// `{ok: false, error}` to repair from; the node is left as it was.
    fn save_script(&self, args: &Value) -> Result<Value, String> {
        let graph = Self::require_str(args, "graph")?;
        let node = Self::require_str(args, "node")?;
        let source = Self::require_str(args, "source")?;
        let door = self.door()?;
        door.json(
            "POST",
            "/script",
            Some(json!({ "graph": graph, "node": node, "source": source })),
        )
        .map_err(|(status, msg)| format!("/script answered {status}: {msg}"))
    }

    /// `S.O.1`: `blackout(on)` — the room's off switch. Posts straight to
    /// `POST /overlay/blackout { on }`, which is a shortcut for the exact
    /// batch `POST /switch {op: on|off, id: "blackout"}` would run (see
    /// `call/blackout.rs`'s own module doc for why this route has its own
    /// small body rather than the generic switch machinery).
    ///
    /// **Why `on` (bool) and not the placeholder's `duration` (seconds).**
    /// The placeholder took a timed auto-off, which this worktree has no
    /// scheduler for — the only durable state this app keeps is a graph on
    /// disk, and `O.28` point 5 fixes blackout's own state as a plain
    /// `bypass`/`forced` flip with no clock attached. Building a timer just
    /// for this one tool would be new, undocumented infrastructure for a
    /// need nothing else in this batch has; the simplest correct thing is an
    /// on/off toggle that matches the route's actual shape, and an agent
    /// that wants "blackout for N seconds" calls `blackout({on: true})`,
    /// waits with `sleep`, then `blackout({on: false})` — two calls already
    /// in its toolbox, not a third mechanism.
    fn blackout(&self, args: &Value) -> Result<Value, String> {
        let on = args
            .get("on")
            .and_then(Value::as_bool)
            .ok_or("expected `on` (bool)")?;
        let door = self.door()?;
        door.json("POST", "/overlay/blackout", Some(json!({ "on": on })))
            .map_err(|(status, msg)| format!("/overlay/blackout answered {status}: {msg}"))
    }

    /// `S.O.2`: `solid_color(rgb, duration_seconds)` — a solid color overlay
    /// for N seconds. `rgb` is `[R, G, B]`, 0-255. Posts to `POST /overlay
    /// {op: "solid", rgb, duration_seconds}` — the same op the grid's own
    /// solid-color piece uses (`call/overlay.rs`), backed by
    /// `metal_perform::overlay::Overlays::start_solid_color`, whose own
    /// `is_active`/`prune` already expire it — no separate timer here
    /// either, for the same reason `blackout` above has none.
    fn solid_color(&self, args: &Value) -> Result<Value, String> {
        let rgb = args
            .get("rgb")
            .and_then(Value::as_array)
            .ok_or("expected `rgb` ([R, G, B], 0-255)")?;
        if rgb.len() != 3 {
            return Err("rgb must be [R, G, B]".to_string());
        }
        for (i, v) in rgb.iter().enumerate() {
            match v.as_u64() {
                Some(n) if n <= 255 => {}
                Some(_) => return Err(format!("rgb[{i}] must be 0-255")),
                None => return Err(format!("rgb[{i}] must be an integer 0-255")),
            }
        }
        let duration = args
            .get("duration_seconds")
            .and_then(Value::as_f64)
            .ok_or("expected `duration_seconds` (in seconds, as a number)")?;
        if duration <= 0.0 {
            return Err("duration_seconds must be positive".to_string());
        }
        let mut body = json!({ "op": "solid", "rgb": rgb, "duration_seconds": duration });
        // P-F: `pattern: "grid"` — the edge-blend test pattern.
        if let Some(p) = args.get("pattern") {
            body["pattern"] = p.clone();
        }
        if let Some(d) = args.get("device") {
            body["device"] = d.clone();
        }
        let door = self.door()?;
        door.json("POST", "/overlay", Some(body))
            .map_err(|(status, msg)| format!("/overlay answered {status}: {msg}"))
    }

    /// S.D: `set_layer_mix(timeline, segment_id, track?, mask?, blend?,
    /// opacity?)` — how one segment composites against the layers under it,
    /// which is [`timeline::SegmentMix`] (O.3) and nothing else.
    ///
    /// The same read-modify-write [`Self::move_segment`] does, for the same
    /// reason: `POST /timeline/segment`'s `move` replaces the whole segment
    /// by id, so the segment is fetched as it stands, only its `mix` is
    /// patched, and the whole thing goes back through
    /// [`super::edit_segment`]. That is also what makes the lanes control
    /// and this tool one path (S.D punto 3): the control posts the very same
    /// body, and `set_layer_mix_writes_byte_identical_to_the_lanes_control`
    /// holds the two documents equal.
    ///
    /// Each of `mask`/`blend`/`opacity` is optional and only the keys
    /// present are written — an absent key keeps the segment's current
    /// value, a `mask: null` clears the contour. `track` is `model` or
    /// `media` (the two tracks that carry a `mix`); left out, the segment is
    /// looked up on `model` first, then `media`. `blend` is spelled the way
    /// [`crate::layers::Blend::parse`] reads it (`normal`/`add`/`multiply`/
    /// `screen`, or their Spanish names) — the `Blend` the live path already
    /// has, not a second vocabulary.
    fn set_layer_mix(&self, args: &Value) -> Result<Value, String> {
        let timeline_id = Self::require_str(args, "timeline")?;
        let segment_id = Self::require_str(args, "segment_id")?;
        let wants_mask = args.get("mask").is_some();
        let wants_blend = args.get("blend").is_some();
        let wants_opacity = args.get("opacity").is_some();
        if !(wants_mask || wants_blend || wants_opacity) {
            return Err(
                "nothing to set — give at least one of `mask`, `blend`, `opacity`".to_string(),
            );
        }

        let tl = timeline::load(Path::new(crate::author::TIMELINES), timeline_id)
            .ok_or_else(|| format!("no timeline called {timeline_id:?}"))?;
        let track = match args.get("track").and_then(Value::as_str) {
            Some(t @ ("model" | "media")) => t.to_string(),
            Some(other) => {
                return Err(format!(
                    "track {other:?} carries no mix — only `model` and `media` segments composite"
                ))
            }
            None => MIX_TRACKS
                .iter()
                .find(|t| find_segment_json(&tl, t, segment_id).is_some())
                .map(|t| (*t).to_string())
                .ok_or_else(|| {
                    format!("no model or media segment {segment_id:?} on timeline {timeline_id:?}")
                })?,
        };
        let mut segment = find_segment_json(&tl, &track, segment_id).ok_or_else(|| {
            format!("no {track:?} segment {segment_id:?} on timeline {timeline_id:?}")
        })?;

        // Start from the mix the segment already has — `None` reads as the
        // default, exactly as `SegmentMix`'s own `#[serde(default)]`s do.
        let mut mix: timeline::SegmentMix = match segment.get("mix") {
            Some(Value::Null) | None => timeline::SegmentMix::default(),
            Some(v) => serde_json::from_value(v.clone())
                .map_err(|e| format!("segment has an unreadable mix: {e}"))?,
        };
        if wants_mask {
            mix.mask = match &args["mask"] {
                Value::Null => None,
                Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
                Value::String(_) => {
                    return Err("`mask` names an empty contour — use null to clear it".to_string())
                }
                _ => return Err("`mask` must be a mask id or null".to_string()),
            };
        }
        if wants_blend {
            let s = args["blend"].as_str().ok_or("`blend` must be a string")?;
            mix.blend = crate::layers::Blend::parse(s).ok_or_else(|| {
                format!(
                    "no such blend: {s:?} — one of {}",
                    crate::layers::Blend::all()
                        .iter()
                        .map(|b| b.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        }
        if wants_opacity {
            let o = args["opacity"]
                .as_f64()
                .ok_or("`opacity` must be a number between 0 and 1")?;
            if !o.is_finite() || !(0.0..=1.0).contains(&o) {
                return Err(format!("opacity {o} is not a fraction between 0 and 1"));
            }
            mix.opacity = o as f32;
        }

        let obj = segment
            .as_object_mut()
            .ok_or("segment is not a JSON object")?;
        obj.insert(
            "mix".to_string(),
            serde_json::to_value(&mix).map_err(|e| e.to_string())?,
        );
        let body =
            json!({ "timeline": timeline_id, "track": track, "op": "move", "segment": segment });
        super::edit_segment(&body).map(|tl| json!(tl))
    }

    /// `S.O.5`: `rasterize_overlay(timeline, track, segment_id, name?,
    /// device?, layer?, loop?)` — bakes a timeline's model segment into a
    /// library movement the overlays grid can play, through `POST /overlay
    /// {op: "rasterize"}` (`call/overlay.rs`, see its module doc for why a
    /// rasterised lane is a movement and not a picture). Only `track:
    /// "model"` has float rows to bake; the route refuses the rest by name
    /// and this passes `track` through so the refusal is the route's, not a
    /// second copy of the rule here. With `device` + `layer` the route also
    /// arms it, through the same `play` body `POST /overlay {op:"play"}` uses.
    fn rasterize_overlay(&self, args: &Value) -> Result<Value, String> {
        let timeline_id = Self::require_str(args, "timeline")?;
        let track = Self::require_str(args, "track")?;
        let segment_id = Self::require_str(args, "segment_id")?;
        let mut body = json!({
            "op": "rasterize",
            "timeline": timeline_id,
            "track": track,
            "segment": segment_id,
        });
        for key in ["name", "device", "layer"] {
            if let Some(v) = args.get(key).and_then(Value::as_str) {
                body[key] = json!(v);
            }
        }
        if let Some(looping) = args.get("loop").and_then(Value::as_bool) {
            body["loop"] = json!(looping);
        }
        let door = self.door()?;
        door.json("POST", "/overlay", Some(body))
            .map_err(|(status, msg)| format!("/overlay answered {status}: {msg}"))
    }

    /// G2: `play_sequence(op, id?, name?, entries?, device?, collection?)` —
    /// the sequence store and its runs, through `POST /sequences`, the body
    /// the sequences panel posts. `run` and `stop` drive a device's output.
    fn play_sequence(&self, args: &Value) -> Result<Value, String> {
        let op = Self::require_str(args, "op")?;
        let mut body = json!({ "op": op });
        for key in ["id", "name", "device", "collection"] {
            if let Some(v) = args.get(key).and_then(Value::as_str) {
                body[key] = json!(v);
            }
        }
        if let Some(entries) = args.get("entries") {
            body["entries"] = entries.clone();
        }
        let door = self.door()?;
        door.json("POST", "/sequences", Some(body))
            .map_err(|(status, msg)| format!("/sequences answered {status}: {msg}"))
    }

    /// G2: `edit_loops` — the loop library and its choice, through the four
    /// routes the loops panel posts: `save` (`POST /loops/save`), `choose`
    /// (`/loops/choose`), `tap` (`/loops/tap`, the downbeat) and `song`
    /// (`/loops/song`, the musical clock beside the song's audio).
    fn edit_loops(&self, args: &Value) -> Result<Value, String> {
        let (path, body) = match Self::require_str(args, "op")? {
            "save" => {
                let definition = args
                    .get("definition")
                    .filter(|v| v.is_object())
                    .ok_or("expected `definition` (the loop object)")?;
                let as_copy = args.get("as_copy").and_then(Value::as_bool).unwrap_or(false);
                ("/loops/save", json!({ "loop": definition, "as_copy": as_copy }))
            }
            "choose" => (
                "/loops/choose",
                json!({
                    "graph": Self::require_str(args, "graph")?,
                    "node": Self::require_str(args, "node")?,
                    "loop": Self::require_str(args, "choice")?,
                }),
            ),
            "tap" => ("/loops/tap", json!({})),
            "song" => {
                let mut body = json!({
                    "stop": args.get("stop").and_then(Value::as_bool).unwrap_or(false),
                });
                if let Some(song) = args.get("song").and_then(Value::as_str) {
                    body["song"] = json!(song);
                }
                if let Some(seconds) = args.get("seconds").and_then(Value::as_f64) {
                    body["from"] = json!(seconds);
                }
                ("/loops/song", body)
            }
            other => return Err(format!("op is save / choose / tap / song, not {other}")),
        };
        let door = self.door()?;
        door.json("POST", path, Some(body))
            .map_err(|(status, msg)| format!("{path} answered {status}: {msg}"))
    }

    /// G2: `transport(action, song?, seconds?)` — `toggle_pause` is the
    /// transport's own pause/resume (`POST /transport`); `play` and `seek`
    /// start the song's audio at `seconds` (`POST /author/play`); `stop`
    /// ends it.
    fn transport(&self, args: &Value) -> Result<Value, String> {
        let (path, body) = match Self::require_str(args, "action")? {
            "toggle_pause" => ("/transport", None),
            "stop" => ("/author/play", Some(json!({ "stop": true }))),
            "play" | "seek" => {
                let song = Self::require_str(args, "song")?;
                let seconds = args.get("seconds").and_then(Value::as_f64).unwrap_or(0.0);
                ("/author/play", Some(json!({ "song": song, "from": seconds })))
            }
            other => return Err(format!("action is toggle_pause / play / seek / stop, not {other}")),
        };
        let door = self.door()?;
        door.json("POST", path, body)
            .map_err(|(status, msg)| format!("{path} answered {status}: {msg}"))
    }

    /// G2: `fire_trigger(id)` — `POST /trigger/<id>`, the same fire the
    /// trigger pads post. The trigger's own target does the work.
    fn fire_trigger(&self, args: &Value) -> Result<Value, String> {
        let id = Self::require_str(args, "id")?;
        if id.is_empty() || id.contains('/') {
            return Err("id must name one trigger".to_string());
        }
        let door = self.door()?;
        door.json("POST", &format!("/trigger/{id}"), None)
            .map_err(|(status, msg)| format!("/trigger answered {status}: {msg}"))
    }

    /// G2: `overlay_control(op, device?, layer?, movement?, loop?)` — the
    /// overlays grid's start and stop ops on `POST /overlay`. `play` takes
    /// the layer and movement; `solid` stays with `solid_color`.
    fn overlay_control(&self, args: &Value) -> Result<Value, String> {
        let op = Self::require_str(args, "op")?;
        if !matches!(op, "play" | "stop" | "stop-all" | "stop-solid") {
            return Err(format!("op is play / stop / stop-all / stop-solid, not {op}"));
        }
        let mut body = json!({ "op": op });
        for key in ["device", "layer", "movement"] {
            if let Some(v) = args.get(key).and_then(Value::as_str) {
                body[key] = json!(v);
            }
        }
        if let Some(looping) = args.get("loop").and_then(Value::as_bool) {
            body["loop"] = json!(looping);
        }
        let door = self.door()?;
        door.json("POST", "/overlay", Some(body))
            .map_err(|(status, msg)| format!("/overlay answered {status}: {msg}"))
    }

    /// G2: `colour_apply(graph, group | nodes, palette | hsb | rgb)` — the
    /// colourist rule, through `POST /colour/apply`. The route refuses a
    /// body with none or several of the three sources.
    fn colour_apply(&self, args: &Value) -> Result<Value, String> {
        let graph = Self::require_str(args, "graph")?;
        let targets = match (args.get("group").and_then(Value::as_str), args.get("nodes")) {
            (Some(group), _) => json!(group),
            (None, Some(nodes)) if nodes.is_array() => nodes.clone(),
            _ => return Err("expected `group` (all, models, effects or text) or `nodes` (node ids)".to_string()),
        };
        let mut body = json!({ "graph": graph, "targets": targets });
        for key in ["palette", "hsb", "rgb"] {
            if let Some(v) = args.get(key) {
                body[key] = v.clone();
            }
        }
        let door = self.door()?;
        door.json("POST", "/colour/apply", Some(body))
            .map_err(|(status, msg)| format!("/colour/apply answered {status}: {msg}"))
    }
}

impl ToolExecutor for AgentHarness {
    fn execute(&mut self, call: &ToolCall) -> Result<Value, String> {
        match call.name.as_str() {
            "import_segment" => self.import_segment(&call.arguments),
            "move_segment" => self.move_segment(&call.arguments),
            "remove_segment" => self.remove_segment(&call.arguments),
            "set_keyframe" => self.set_keyframe(&call.arguments),
            "attach_digest" => self.attach_digest(&call.arguments),
            "create_mask_grid" => self.create_mask_grid(&call.arguments),
            "test_mask_grid" => self.test_mask_grid(&call.arguments),
            "edit_mask" => self.edit_mask(&call.arguments),
            "add_effect" => self.add_effect(&call.arguments),
            "remove_effect" => self.remove_effect(&call.arguments),
            "edit_output" => self.edit_output(&call.arguments),
            "resolve_draft" => self.resolve_draft(&call.arguments),
            "edit_venue_surface" => self.edit_venue_surface(&call.arguments),
            "edit_venue_object" => self.edit_venue_object(&call.arguments),
            "add_screen" => self.add_screen(&call.arguments),
            "add_projection" => self.add_projection(&call.arguments),
            "set_surface_form" => self.set_surface_form(&call.arguments),
            "set_venue_sectors" => self.set_venue_sectors(&call.arguments),
            "move_venue_item" => self.move_venue_item(&call.arguments),
            "projection_area" => self.projection_area(&call.arguments),
            "venue_scene" => self.venue_scene(&call.arguments),
            "add_stage" => self.add_stage(&call.arguments),
            "add_truss" => self.add_truss(&call.arguments),
            "add_dummy" => self.add_dummy(&call.arguments),
            "venue_spline" => self.venue_spline(&call.arguments),
            "place_mask_in_space" => self.place_mask_in_space(&call.arguments),
            "venue_boolean" => self.venue_boolean(&call.arguments),
            "venue_hit" => self.venue_hit(&call.arguments),
            "venue_report" => self.venue_report(&call.arguments),
            "venue_view" => self.venue_view(&call.arguments),
            "venue_assets_search" => self.venue_assets_search(&call.arguments),
            "venue_asset_import" => self.venue_asset_import(&call.arguments),
            "set_fixture" => self.set_fixture(&call.arguments),
            "add_fixture_node" => self.add_fixture_node(&call.arguments),
            "calibrate_projector" => self.calibrate_projector(&call.arguments),
            "blend_edges" => self.blend_edges(&call.arguments),
            "project_venue" => self.project_venue(&call.arguments),
            "look_venue" => self.look_venue(&call.arguments),
            // M5: with no `device` the seam renders the venue's room; with one
            // (or no venue) the inner harness asks the real camera.
            "capture_camera_frame" => self.capture_camera_frame(&call.arguments),
            "capture_output" => self.capture_output(&call.arguments),
            "aim_fixture" => self.aim_fixture(&call.arguments),
            "import_gdtf" => self.import_gdtf(&call.arguments),
            "scan_venue" => self.scan_venue(&call.arguments),
            "discover_fixture" => self.discover_fixture(&call.arguments),
            "solve_head" => self.solve_head(&call.arguments),
            "edit_layer" => self.edit_layer(&call.arguments),
            "listen_status" => self.listen_status(&call.arguments),
            // S.AK/O.28 — the grid's switches.
            "list_switches" => self.list_switches(&call.arguments),
            "list_transitions" => self.list_transitions(&call.arguments),
            "set_switch" => self.set_switch(&call.arguments),
            // V2b: the tick-level switch, over the shell's own /settings route.
            "get_tick_level" => self.get_tick_level(&call.arguments),
            "set_tick_level" => self.set_tick_level(&call.arguments),
            // H5: the head-tracking gesture.
            "set_head_tracking" => self.set_head_tracking(&call.arguments),
            // P-G: the preview bar's camera, the same route.
            "export_frame" => self.export_frame(&call.arguments),
            // E1: the offline video export job.
            "export_video" => self.export_video(&call.arguments),
            "list_video_io" => self.list_video_io(),
            "set_output_sink" => self.set_output_sink(&call.arguments),
            "dmx_bus" => self.dmx_bus(&call.arguments),
            // The pairing overlay's read half: `metal-sync`'s status, peers and log
            // through the door's `GET /sync/*` (read-only).
            "sync_status" => self.sync_read("/sync/status"),
            "sync_peers" => self.sync_read("/sync/peers"),
            "sync_log" => self.sync_read("/sync/log"),
            // P-D: the out.artnet page's discovery and the timecode reading.
            "artnet_nodes" => self.artnet_nodes(&call.arguments),
            "artnet_in_status" => self.artnet_in_status(&call.arguments),
            "timecode_status" => self.sync_read("/timecode"),
            "add_switch" => self.add_switch(&call.arguments),
            "remove_switch" => self.remove_switch(&call.arguments),
            "embed_ai" => self.embed_ai(&call.arguments),
            "attach_source" => self.attach_source(&call.arguments),
            "insert_node" => self.insert_node(&call.arguments),
            // K — the palette node's write, the same route its folder posts.
            "set_palette" => self.set_palette(&call.arguments),
            // M4 — the `mat.*` node's part and fixed overrides, the same merge
            // its properties page posts.
            "set_material" => self.set_material(&call.arguments),
            // Video scenes — the gallery's read and the book's writes.
            "list_scenes" => self.list_scenes(&call.arguments),
            "list_dataset" => self.list_dataset(&call.arguments),
            "edit_scene" => self.edit_scene(&call.arguments),
            // §9.4 V3 — a script node's own source, read and written through
            // the same `/script` route the properties editor uses.
            "read_script" => self.read_script(&call.arguments),
            "save_script" => self.save_script(&call.arguments),
            // S.O.1, S.O.2, S.D, S.O.5 — overlay and timeline tools
            "blackout" => self.blackout(&call.arguments),
            "solid_color" => self.solid_color(&call.arguments),
            "set_layer_mix" => self.set_layer_mix(&call.arguments),
            "rasterize_overlay" => self.rasterize_overlay(&call.arguments),
            // G2 — the playback and performance gestures the panels post.
            "play_sequence" => self.play_sequence(&call.arguments),
            "edit_loops" => self.edit_loops(&call.arguments),
            "transport" => self.transport(&call.arguments),
            "fire_trigger" => self.fire_trigger(&call.arguments),
            "overlay_control" => self.overlay_control(&call.arguments),
            "colour_apply" => self.colour_apply(&call.arguments),
            _ => self.inner.execute(call),
        }
    }
}

/// The named segment on `track`, as the JSON [`super::edit_segment`] itself
/// would accept back — i.e. `serde_json::to_value` of whichever typed
/// segment struct that track holds. `None` if the track name is not one of
/// the six, or no segment there has that id.
fn find_segment_json(tl: &timeline::Timeline, track: &str, id: &str) -> Option<Value> {
    match track {
        "model" => tl
            .model_track
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| serde_json::to_value(s).ok()),
        "effect" => tl
            .effect_tracks
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| serde_json::to_value(s).ok()),
        "media" => tl
            .media_track
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| serde_json::to_value(s).ok()),
        "still" => tl
            .still_track
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| serde_json::to_value(s).ok()),
        "text" => tl
            .text_track
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| serde_json::to_value(s).ok()),
        "signal" => tl
            .signal_track
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| serde_json::to_value(s).ok()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Timeline and example fixtures write through the *real* relative
    /// paths (`crate::author::TIMELINES` = `"data/timelines"`,
    /// `EXAMPLES_DIR` = `"data/animations"`) — the same hardcoded paths
    /// `edit_segment`/`save_example` use in the real server, and the same
    /// ones every other test in this module already accepts running
    /// against. Each test picks a unique id and cleans up its own file, so
    /// nothing written here is left behind for git to notice.
    fn manifest() -> Manifest {
        serde_json::from_str(
            r#"{"rate":8.0,"models":[
                {"id":"geoterrain","label":"Geoterrain","blocks":[
                    {"type":"floats","fields":[{"name":"radius","min":0.0,"max":5.0,"default":1.0}]}
                ]}
            ]}"#,
        )
        .unwrap()
    }

    fn harness() -> AgentHarness {
        let tmp = std::env::temp_dir().join(format!(
            "metal-dash-agent-tools-inner-{}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&tmp);
        AgentHarness::new(metal_agent::harness::Harness::new(tmp), manifest())
    }

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: "1".to_string(),
            name: name.to_string(),
            arguments: args,
        }
    }

    fn timeline_path(id: &str) -> std::path::PathBuf {
        Path::new(crate::author::TIMELINES).join(format!("{id}.json"))
    }

    fn example_path(model: &str, song: &str) -> std::path::PathBuf {
        Path::new(EXAMPLES_DIR)
            .join(model)
            .join(format!("{song}.json"))
    }

    /// A fresh, empty timeline on disk at `id`, long enough for the fixture
    /// spans below. Text/signal tracks need no node catalog to validate, so
    /// this stays independent of `data/schema.json`'s real model list.
    fn fresh_timeline(id: &str) {
        let tl = timeline::Timeline::new(id, "S.K test", "click-100", 1000);
        let cat = timeline::catalog().expect("data/schema.json is checked in");
        timeline::save(Path::new(crate::author::TIMELINES), tl, &cat)
            .expect("save fixture timeline");
    }

    fn text_segment(id: &str) -> Value {
        json!({ "id": id, "from": 10, "to": 20, "text": "hello", "size": 0.1, "align": "center" })
    }

    #[test]
    fn import_segment_writes_byte_identical_to_the_route_it_wraps() {
        let tl_id = "sk-agent-tools-import";
        let seg_id = "txt-1";

        fresh_timeline(tl_id);
        let mut h = harness();
        h.execute(&call(
            "import_segment",
            json!({ "timeline": tl_id, "track": "text", "segment": text_segment(seg_id) }),
        ))
        .expect("import_segment");
        let via_tool = fs::read_to_string(timeline_path(tl_id)).unwrap();

        fresh_timeline(tl_id); // same starting point, again
        let body = json!({ "timeline": tl_id, "track": "text", "op": "add", "segment": text_segment(seg_id) });
        super::super::edit_segment(&body).expect("edit_segment");
        let via_route = fs::read_to_string(timeline_path(tl_id)).unwrap();

        assert_eq!(via_tool, via_route);
        let _ = fs::remove_file(timeline_path(tl_id));
    }

    #[test]
    fn move_segment_replaces_the_whole_segment_by_id_like_a_drag_would() {
        let tl_id = "sk-agent-tools-move";
        let seg_id = "txt-move";

        fresh_timeline(tl_id);
        // Land the segment first, same as the page already having drawn it.
        let body = json!({ "timeline": tl_id, "track": "text", "op": "add", "segment": text_segment(seg_id) });
        super::super::edit_segment(&body).expect("seed segment");

        let mut h = harness();
        let out = h
            .execute(&call(
                "move_segment",
                json!({ "timeline": tl_id, "track": "text", "id": seg_id, "from": 50, "to": 80 }),
            ))
            .expect("move_segment");
        let moved = out["text_track"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == seg_id)
            .unwrap();
        assert_eq!(moved["from"], 50);
        assert_eq!(moved["to"], 80);
        // Everything else the drag would have carried along survives —
        // this is the "no partial patch" guarantee the module doc promises.
        assert_eq!(moved["text"], "hello");
        assert_eq!(moved["align"], "center");

        let via_tool = fs::read_to_string(timeline_path(tl_id)).unwrap();

        // Rebuild the same before-state and do the identical move through
        // `edit_segment` directly, with the whole segment supplied by hand
        // the way the page's own drag would.
        fresh_timeline(tl_id);
        super::super::edit_segment(&body).expect("reseed");
        let moved_segment = json!({ "id": seg_id, "from": 50, "to": 80, "text": "hello", "size": 0.1, "align": "center" });
        let move_body =
            json!({ "timeline": tl_id, "track": "text", "op": "move", "segment": moved_segment });
        super::super::edit_segment(&move_body).expect("edit_segment move");
        let via_route = fs::read_to_string(timeline_path(tl_id)).unwrap();

        assert_eq!(via_tool, via_route);
        let _ = fs::remove_file(timeline_path(tl_id));
    }

    #[test]
    fn move_segment_errors_clearly_on_an_unknown_id() {
        let tl_id = "sk-agent-tools-move-missing";
        fresh_timeline(tl_id);
        let mut h = harness();
        let err = h
            .execute(&call(
                "move_segment",
                json!({ "timeline": tl_id, "track": "text", "id": "nope", "from": 0, "to": 10 }),
            ))
            .unwrap_err();
        assert!(err.contains("no"), "{err}");
        let _ = fs::remove_file(timeline_path(tl_id));
    }

    #[test]
    fn remove_segment_writes_byte_identical_to_the_route_it_wraps() {
        let tl_id = "sk-agent-tools-remove";
        let seg_id = "txt-remove";

        fresh_timeline(tl_id);
        let add_body = json!({ "timeline": tl_id, "track": "text", "op": "add", "segment": text_segment(seg_id) });
        super::super::edit_segment(&add_body).expect("seed");

        let mut h = harness();
        h.execute(&call(
            "remove_segment",
            json!({ "timeline": tl_id, "track": "text", "id": seg_id }),
        ))
        .expect("remove_segment");
        let via_tool = fs::read_to_string(timeline_path(tl_id)).unwrap();

        fresh_timeline(tl_id);
        super::super::edit_segment(&add_body).expect("reseed");
        let remove_body =
            json!({ "timeline": tl_id, "track": "text", "op": "remove", "id": seg_id });
        super::super::edit_segment(&remove_body).expect("edit_segment remove");
        let via_route = fs::read_to_string(timeline_path(tl_id)).unwrap();

        assert_eq!(via_tool, via_route);
        let _ = fs::remove_file(timeline_path(tl_id));
    }

    /// A generator segment on layer 1, the shape the lanes control lists —
    /// `geoterrain` because it is the first entry of the checked-in
    /// `data/schema.json`, not because the test cares which model it is.
    fn model_segment(id: &str, layer: usize) -> Value {
        json!({ "id": id, "from": 10, "to": 20, "model": "geoterrain", "layer": layer })
    }

    /// S.D's verification, in one test: *"the document saved by the
    /// control and by the tool must be identical, compared in a test"*.
    /// The lanes control (`shell::panels::lanes::mix_row`) posts the whole
    /// segment with its `mix` replaced through `POST /author/models/timeline/
    /// segment { op: "move" }` — the body written by hand below is that
    /// exact body, and `lanes.rs`'s own kittest test
    /// (`picking_a_mix_posts_the_whole_segment_with_its_mix`) pins the
    /// control to it. Here the tool goes first, then the control's body
    /// through `edit_segment` from the same starting point, and the two
    /// files have to match byte for byte.
    #[test]
    fn set_layer_mix_writes_byte_identical_to_the_lanes_control() {
        let tl_id = "sd-agent-tools-mix";
        let seg_id = "gen-top";

        fresh_timeline(tl_id);
        let add_body = json!({ "timeline": tl_id, "track": "model", "op": "add", "segment": model_segment(seg_id, 1) });
        super::super::edit_segment(&add_body).expect("seed segment");

        let mut h = harness();
        let out = h
            .execute(&call(
                "set_layer_mix",
                json!({ "timeline": tl_id, "segment_id": seg_id, "mask": "mask-2", "blend": "screen", "opacity": 0.75 }),
            ))
            .expect("set_layer_mix");
        let mixed = out["model_track"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == seg_id)
            .unwrap();
        assert_eq!(mixed["mix"]["mask"], "mask-2");
        assert_eq!(mixed["mix"]["blend"], "screen");
        assert_eq!(mixed["mix"]["opacity"], 0.75);
        // Everything the drag would have carried along survives — no
        // partial patch, same guarantee `move_segment` makes.
        assert_eq!(mixed["model"], "geoterrain");
        assert_eq!(mixed["layer"], 1);
        let via_tool = fs::read_to_string(timeline_path(tl_id)).unwrap();

        fresh_timeline(tl_id);
        super::super::edit_segment(&add_body).expect("reseed");
        let control_segment = json!({
            "id": seg_id, "from": 10, "to": 20, "model": "geoterrain", "layer": 1,
            "mix": { "mask": "mask-2", "blend": "screen", "opacity": 0.75 }
        });
        let control_body = json!({ "timeline": tl_id, "track": "model", "op": "move", "segment": control_segment });
        super::super::edit_segment(&control_body).expect("edit_segment move");
        let via_control = fs::read_to_string(timeline_path(tl_id)).unwrap();

        assert_eq!(via_tool, via_control);
        let _ = fs::remove_file(timeline_path(tl_id));
    }

    /// Only the keys named are written: a call that sets `blend` alone keeps
    /// the mask and opacity a previous call left, `mask: null` clears the
    /// contour, and the segment is found on `media` without a `track` when
    /// it is not on `model`.
    #[test]
    fn set_layer_mix_patches_only_what_it_is_given_and_finds_the_track_itself() {
        let tl_id = "sd-agent-tools-mix-patch";
        let seg_id = "gen-a";
        fresh_timeline(tl_id);
        super::super::edit_segment(&json!({ "timeline": tl_id, "track": "model", "op": "add", "segment": model_segment(seg_id, 0) }))
            .expect("seed");

        let mut h = harness();
        h.execute(&call(
            "set_layer_mix",
            json!({ "timeline": tl_id, "segment_id": seg_id, "mask": "door", "opacity": 0.5 }),
        ))
        .expect("first");
        let out = h
            .execute(&call(
                "set_layer_mix",
                json!({ "timeline": tl_id, "segment_id": seg_id, "blend": "suma" }),
            ))
            .expect("second");
        let seg = out["model_track"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == seg_id)
            .unwrap();
        assert_eq!(
            seg["mix"]["mask"], "door",
            "blend alone must not touch the mask"
        );
        assert_eq!(
            seg["mix"]["opacity"], 0.5,
            "blend alone must not touch the opacity"
        );
        assert_eq!(
            seg["mix"]["blend"], "add",
            "the Spanish alias `suma` is `Blend::parse`'s own spelling of add"
        );

        let out = h
            .execute(&call(
                "set_layer_mix",
                json!({ "timeline": tl_id, "segment_id": seg_id, "mask": Value::Null }),
            ))
            .expect("clear");
        let seg = out["model_track"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == seg_id)
            .unwrap();
        assert_eq!(seg["mix"]["mask"], Value::Null);

        let err = h
            .execute(&call(
                "set_layer_mix",
                json!({ "timeline": tl_id, "segment_id": seg_id }),
            ))
            .unwrap_err();
        assert!(err.contains("nothing to set"), "{err}");
        let err = h
            .execute(&call(
                "set_layer_mix",
                json!({ "timeline": tl_id, "segment_id": seg_id, "blend": "Multiply" }),
            ))
            .unwrap_err();
        assert!(err.contains("no such blend"), "{err}");
        let err = h
            .execute(&call(
                "set_layer_mix",
                json!({ "timeline": tl_id, "segment_id": "nope", "opacity": 1.0 }),
            ))
            .unwrap_err();
        assert!(err.contains("no model or media segment"), "{err}");
        let _ = fs::remove_file(timeline_path(tl_id));
    }

    /// A door that records every call and answers what it was scripted to
    /// — the same double `shell::FakeDoor` is, written here because that
    /// one is `shell/`'s and this module has tests on builds `shell/` is
    /// configured out of. `Mutex`/`Arc` rather than `RefCell`/`Rc` only
    /// because the harness wants `Box<dyn Call + Send>`.
    struct ScriptedDoor {
        calls: std::sync::Mutex<Vec<(String, String, Vec<u8>)>>,
        replies: std::collections::HashMap<(String, String), (u16, &'static str, Vec<u8>)>,
    }

    impl ScriptedDoor {
        fn new() -> ScriptedDoor {
            ScriptedDoor {
                calls: Default::default(),
                replies: Default::default(),
            }
        }
        fn json(mut self, method: &str, path: &str, body: Value) -> ScriptedDoor {
            self.replies.insert(
                (method.into(), path.into()),
                (200, "application/json", body.to_string().into_bytes()),
            );
            self
        }
        fn bytes(
            mut self,
            method: &str,
            path: &str,
            kind: &'static str,
            body: Vec<u8>,
        ) -> ScriptedDoor {
            self.replies
                .insert((method.into(), path.into()), (200, kind, body));
            self
        }
    }

    impl Call for std::sync::Arc<ScriptedDoor> {
        fn call(
            &self,
            method: &str,
            path: &str,
            params: crate::call::Params,
        ) -> Option<crate::call::Reply> {
            self.calls.lock().unwrap().push((
                method.to_string(),
                path.to_string(),
                params.body.clone(),
            ));
            let (status, kind, body) = self
                .replies
                .get(&(method.to_string(), path.to_string()))
                .cloned()
                .unwrap_or((200, "application/json", b"{}".to_vec()));
            Some(crate::call::Reply { status, kind, body })
        }
    }

    fn with_door(door: &std::sync::Arc<ScriptedDoor>) -> AgentHarness {
        harness().with_door(Box::new(std::sync::Arc::clone(door)))
    }

    fn posted(door: &ScriptedDoor, method: &str, path: &str) -> Option<Value> {
        door.calls
            .lock()
            .unwrap()
            .iter()
            .find(|(m, p, _)| m == method && p == path)
            .map(|(_, _, body)| serde_json::from_slice(body).unwrap_or(Value::Null))
    }

    /// The three sync read tools answer the door's `GET /sync/*` as the
    /// route answers it, and never write: no POST is recorded.
    #[test]
    fn sync_read_tools_read_the_sync_routes_and_never_write() {
        let door = std::sync::Arc::new(
            ScriptedDoor::new()
                .json(
                    "GET",
                    "/sync/status",
                    json!({ "running": true, "name": "room" }),
                )
                .json(
                    "GET",
                    "/sync/peers",
                    json!({ "running": true, "peers": [{ "name": "kitchen" }] }),
                )
                .json("GET", "/sync/log", json!({ "running": false, "log": [] })),
        );
        let mut h = with_door(&door);
        let out = h
            .execute(&call("sync_status", json!({})))
            .expect("sync_status");
        assert_eq!(out["name"], json!("room"));
        let out = h
            .execute(&call("sync_peers", json!({})))
            .expect("sync_peers");
        assert_eq!(out["peers"][0]["name"], json!("kitchen"));
        let out = h.execute(&call("sync_log", json!({}))).expect("sync_log");
        assert_eq!(out["running"], json!(false));

        let calls = door.calls.lock().unwrap();
        let paths: Vec<&str> = calls.iter().map(|(_, p, _)| p.as_str()).collect();
        assert_eq!(paths, ["/sync/status", "/sync/peers", "/sync/log"]);
        assert!(
            calls.iter().all(|(m, _, _)| m == "GET"),
            "a sync read tool wrote"
        );
    }

    /// Surface sources: `attach_source` and `insert_node` post the two graph
    /// ops verbatim, and `attach_source` leaves out the keys it was not given
    /// (no `kind`/`from` is a detach, not a `null` kind).
    #[test]
    fn attach_source_and_insert_node_post_the_graph_ops_the_panel_posts() {
        let door = std::sync::Arc::new(ScriptedDoor::new());
        let mut h = with_door(&door);
        h.execute(&call(
            "attach_source",
            json!({ "graph": "g1", "node": "mask-0", "port": "stroke", "kind": "gen.turing" }),
        ))
        .expect("attach_source");
        let last_body = || -> Value {
            let calls = door.calls.lock().unwrap();
            serde_json::from_slice(&calls.last().expect("a call was made").2).unwrap()
        };
        assert_eq!(
            last_body(),
            json!({ "op": "attach", "graph": "g1", "node": "mask-0", "port": "stroke", "kind": "gen.turing" })
        );
        h.execute(&call(
            "attach_source",
            json!({ "graph": "g1", "node": "mask-0", "port": "stroke", "kind": null }),
        ))
        .expect("attach_source detach");
        assert_eq!(
            last_body(),
            json!({ "op": "attach", "graph": "g1", "node": "mask-0", "port": "stroke" }),
            "a null kind is a detach, not a kind named null"
        );
        h.execute(&call(
            "insert_node",
            json!({ "graph": "g1", "after": "gen-0", "kind": "fx.vignette" }),
        ))
        .expect("insert_node");
        assert_eq!(
            last_body(),
            json!({ "op": "insert", "graph": "g1", "after": "gen-0", "kind": "fx.vignette" })
        );
        assert!(h
            .execute(&call(
                "insert_node",
                json!({ "graph": "g1", "after": "gen-0" })
            ))
            .is_err());
    }

    /// `R22.4` (rule 6): `add_switch` posts exactly the body the grid's
    /// `+ create button` popover writes (`name` defaults to the id, `unset`
    /// `embed_ai` posts the properties panel's `ai.embed` body verbatim, on and off.
    #[test]
    fn embed_ai_posts_the_ai_embed_op_the_panel_posts() {
        let door = std::sync::Arc::new(ScriptedDoor::new());
        let mut h = with_door(&door);
        h.execute(&call(
            "embed_ai",
            json!({ "graph": "g1", "node": "gen-0", "ai": "ai.colourist", "on": true }),
        ))
        .expect("embed_ai on");
        assert_eq!(
            posted(&door, "POST", "/graph/node"),
            Some(json!({
                "op": "ai.embed", "graph": "g1", "node": "gen-0",
                "ai": "ai.colourist", "on": true
            }))
        );
        h.execute(&call(
            "embed_ai",
            json!({ "graph": "g1", "node": "gen-0", "ai": "ai.colourist", "on": false }),
        ))
        .expect("embed_ai off");
        assert_eq!(
            door.calls.lock().unwrap().last().map(|c| c.1.clone()),
            Some("/graph/node".to_string())
        );
        assert!(
            h.execute(&call(
                "embed_ai",
                json!({ "graph": "g1", "node": "gen-0", "ai": "ai.colourist" })
            ))
            .is_err(),
            "`on` is required, not defaulted"
        );
    }

    /// stays out unless given), `remove_switch` posts the same
    /// `{op:"remove"}` the grid's right-click fires — one route, three
    /// callers.
    #[test]
    fn add_and_remove_switch_post_the_bodies_the_grid_writes() {
        let door = std::sync::Arc::new(ScriptedDoor::new());
        let mut h = with_door(&door);
        h.execute(&call(
            "add_switch",
            json!({
                "id": "sw-1", "kind": "hold", "graph": "g1",
                "set": [{ "node": "gen-1", "on": true }]
            }),
        ))
        .expect("add_switch");
        assert_eq!(
            posted(&door, "POST", "/switch"),
            Some(json!({
                "op": "add", "id": "sw-1", "name": "sw-1", "kind": "hold", "graph": "g1",
                "set": [{ "node": "gen-1", "on": true }], "unset": null
            }))
        );
        // G1: a `fixture` button carries its own target too — the same
        // `fixture`/`group` fields the popover writes.
        h.execute(&call(
            "add_switch",
            json!({
                "id": "sw-fx", "kind": "fixture", "graph": "g1",
                "set": [{ "node": "venue-1", "on": true }], "fixture": "wash-1"
            }),
        ))
        .expect("add_switch fixture");
        assert_eq!(
            door.calls.lock().unwrap().last().and_then(|(m, p, b)| {
                (m == "POST" && p == "/switch").then(|| serde_json::from_slice::<Value>(b).unwrap())
            }),
            Some(json!({
                "op": "add", "id": "sw-fx", "name": "sw-fx", "kind": "fixture", "graph": "g1",
                "set": [{ "node": "venue-1", "on": true }], "unset": null, "fixture": "wash-1"
            }))
        );
        h.execute(&call("remove_switch", json!({ "id": "sw-1" })))
            .expect("remove_switch");
        let calls = door.calls.lock().unwrap();
        let last = calls.last().expect("two calls");
        let body: Value = serde_json::from_slice(&last.2).unwrap();
        assert_eq!(last.1, "/switch");
        assert_eq!(body, json!({ "op": "remove", "id": "sw-1" }));
        assert!(
            h.execute(&call(
                "add_switch",
                json!({ "id": "sw-2", "kind": "latch", "graph": "g1", "set": [] })
            ))
            .is_err(),
            "an unknown kind is refused, not defaulted"
        );
    }

    /// §9.4 V3 (rule 6): `read_script`/`save_script` walk the same `/script`
    /// route the properties editor uses — the read is a `GET /script/<node>`,
    /// the write posts `{graph, node, source}` verbatim — and without a door
    /// they refuse rather than touching the graph themselves.
    #[test]
    fn the_script_tools_use_the_script_route() {
        let source = "def process(floats, row):\n    return [1.0]\n";
        let door = std::sync::Arc::new(
            ScriptedDoor::new().bytes(
                "GET",
                "/script/py-0",
                "application/json",
                json!({ "lang": "py", "source": source })
                    .to_string()
                    .into_bytes(),
            ),
        );
        let mut h = with_door(&door);
        let read = h
            .execute(&call(
                "read_script",
                json!({ "graph": "g1", "node": "py-0" }),
            ))
            .expect("read_script");
        assert_eq!(
            (read["lang"].as_str(), read["source"].as_str()),
            (Some("py"), Some(source))
        );
        assert!(
            door.calls
                .lock()
                .unwrap()
                .iter()
                .any(|(m, p, _)| m == "GET" && p == "/script/py-0"),
            "the read goes to GET /script/<node>"
        );
        h.execute(&call(
            "save_script",
            json!({ "graph": "g1", "node": "py-0", "source": "def process(floats, row):\n    return [0.0]\n" }),
        ))
        .expect("save_script");
        assert_eq!(
            posted(&door, "POST", "/script"),
            Some(json!({
                "graph": "g1", "node": "py-0",
                "source": "def process(floats, row):\n    return [0.0]\n"
            }))
        );
        // No door: both refuse instead of being a second writer.
        let mut bare = harness();
        assert!(bare
            .execute(&call(
                "read_script",
                json!({ "graph": "g1", "node": "py-0" })
            ))
            .is_err());
        assert!(bare
            .execute(&call(
                "save_script",
                json!({ "graph": "g1", "node": "py-0", "source": source })
            ))
            .is_err());
    }

    /// K (rule 6): `set_palette` posts exactly the body the palette node's
    /// folder writes (`op:"set"`, the slot, the value, and `relative`), and
    /// without a door it says so rather than posting nowhere.
    #[test]
    fn set_palette_posts_the_body_the_folder_writes() {
        let door = std::sync::Arc::new(ScriptedDoor::new());
        let mut h = with_door(&door);
        h.execute(&call(
            "set_palette",
            json!({ "id": "hotel", "slot": 1, "value": [0.95, 0.55, 0.1], "relative": true }),
        ))
        .expect("set_palette");
        assert_eq!(
            posted(&door, "POST", "/author/palettes"),
            Some(json!({
                "op": "set", "id": "hotel", "slot": 1,
                "value": [0.95, 0.55, 0.1], "relative": true
            }))
        );
        // `relative` is optional and defaults to false — the isolated write.
        h.execute(&call(
            "set_palette",
            json!({ "id": "hotel", "slot": 0, "value": [1.0, 0.0, 0.0] }),
        ))
        .expect("set_palette without relative");
        let calls = door.calls.lock().unwrap();
        let last = calls.last().expect("two calls");
        let body: Value = serde_json::from_slice(&last.2).unwrap();
        assert_eq!(last.1, "/author/palettes");
        assert_eq!(
            body,
            json!({
                "op": "set", "id": "hotel", "slot": 0,
                "value": [1.0, 0.0, 0.0], "relative": false
            })
        );
        // A value that is not three floats is refused by name.
        assert!(
            h.execute(&call(
                "set_palette",
                json!({ "id": "hotel", "slot": 0, "value": [1.0, 0.0] })
            ))
            .is_err(),
            "a two-component value is refused"
        );

        // No door: the honest failure every door-bound tool gives.
        let err = harness()
            .execute(&call(
                "set_palette",
                json!({ "id": "hotel", "slot": 0, "value": [1.0, 0.0, 0.0] }),
            ))
            .unwrap_err();
        assert!(err.contains("with_door"), "{err}");
    }

    /// G2: `play_sequence` posts the `POST /sequences` body the sequences
    /// panel posts for each of its ops, and names no others.
    #[test]
    fn play_sequence_posts_the_body_the_sequences_panel_posts() {
        let door = std::sync::Arc::new(ScriptedDoor::new());
        let mut h = with_door(&door);
        h.execute(&call(
            "play_sequence",
            json!({ "op": "run", "id": "opening", "device": "wall" }),
        ))
        .expect("run");
        assert_eq!(
            posted(&door, "POST", "/sequences"),
            Some(json!({ "op": "run", "id": "opening", "device": "wall" }))
        );
        h.execute(&call("play_sequence", json!({ "op": "stop-all" })))
            .expect("stop-all");
        let calls = door.calls.lock().unwrap();
        let last: Value = serde_json::from_slice(&calls.last().unwrap().2).unwrap();
        assert_eq!(last, json!({ "op": "stop-all" }));
    }

    /// G2: `edit_loops` posts each op to the route the loops panel uses
    /// (`/loops/save`, `/loops/choose`, `/loops/tap`, `/loops/song`), and
    /// refuses an op it does not know.
    #[test]
    fn edit_loops_posts_each_op_to_its_route() {
        let door = std::sync::Arc::new(ScriptedDoor::new());
        let mut h = with_door(&door);
        h.execute(&call(
            "edit_loops",
            json!({ "op": "choose", "graph": "live-1", "node": "n3", "choice": "half-time" }),
        ))
        .expect("choose");
        assert_eq!(
            posted(&door, "POST", "/loops/choose"),
            Some(json!({ "graph": "live-1", "node": "n3", "loop": "half-time" }))
        );
        h.execute(&call("edit_loops", json!({ "op": "tap" }))).expect("tap");
        assert_eq!(posted(&door, "POST", "/loops/tap"), Some(json!({})));
        h.execute(&call(
            "edit_loops",
            json!({ "op": "song", "song": "tema", "seconds": 12.5 }),
        ))
        .expect("song");
        assert_eq!(
            posted(&door, "POST", "/loops/song"),
            Some(json!({ "stop": false, "song": "tema", "from": 12.5 }))
        );
        let err = h
            .execute(&call("edit_loops", json!({ "op": "bend" })))
            .unwrap_err();
        assert!(err.contains("bend"), "{err}");
    }

    /// G2: `transport` posts the toggle to `POST /transport`, and the song
    /// clock's play, seek and stop to `POST /author/play`.
    #[test]
    fn transport_posts_the_toggle_and_the_song_clock() {
        let door = std::sync::Arc::new(ScriptedDoor::new());
        let mut h = with_door(&door);
        h.execute(&call("transport", json!({ "action": "toggle_pause" })))
            .expect("toggle_pause");
        assert!(posted(&door, "POST", "/transport").is_some());
        h.execute(&call(
            "transport",
            json!({ "action": "seek", "song": "tema", "seconds": 30.0 }),
        ))
        .expect("seek");
        assert_eq!(
            posted(&door, "POST", "/author/play"),
            Some(json!({ "song": "tema", "from": 30.0 }))
        );
        h.execute(&call("transport", json!({ "action": "stop" })))
            .expect("stop");
        let calls = door.calls.lock().unwrap();
        let last: Value = serde_json::from_slice(&calls.last().unwrap().2).unwrap();
        assert_eq!(last, json!({ "stop": true }));
    }

    /// G2: `fire_trigger` posts `POST /trigger/<id>` with no body, and
    /// refuses an id that is not one trigger's name.
    #[test]
    fn fire_trigger_posts_the_trigger_route() {
        let door = std::sync::Arc::new(ScriptedDoor::new());
        let mut h = with_door(&door);
        h.execute(&call("fire_trigger", json!({ "id": "pad-1" })))
            .expect("fire_trigger");
        let calls = door.calls.lock().unwrap();
        let (method, path, _) = calls.last().expect("one call");
        assert_eq!((method.as_str(), path.as_str()), ("POST", "/trigger/pad-1"));
        drop(calls);
        let err = h
            .execute(&call("fire_trigger", json!({ "id": "a/b" })))
            .unwrap_err();
        assert!(err.contains("one trigger"), "{err}");
    }

    /// G2: `overlay_control` posts the overlays grid's play and stop ops to
    /// `POST /overlay`, and refuses the solid op (that is `solid_color`'s).
    #[test]
    fn overlay_control_posts_the_overlay_ops() {
        let door = std::sync::Arc::new(ScriptedDoor::new());
        let mut h = with_door(&door);
        h.execute(&call(
            "overlay_control",
            json!({ "op": "play", "device": "wall", "layer": "l1", "movement": "fog", "loop": true }),
        ))
        .expect("play");
        assert_eq!(
            posted(&door, "POST", "/overlay"),
            Some(json!({ "op": "play", "device": "wall", "layer": "l1", "movement": "fog", "loop": true }))
        );
        h.execute(&call("overlay_control", json!({ "op": "stop-all" })))
            .expect("stop-all");
        let calls = door.calls.lock().unwrap();
        let last: Value = serde_json::from_slice(&calls.last().unwrap().2).unwrap();
        assert_eq!(last, json!({ "op": "stop-all" }));
        drop(calls);
        let err = h
            .execute(&call("overlay_control", json!({ "op": "solid" })))
            .unwrap_err();
        assert!(err.contains("solid"), "{err}");
    }

    /// G2: `colour_apply` posts the colourist body `POST /colour/apply` takes:
    /// a group or node ids as `targets`, and the one colour source given.
    #[test]
    fn colour_apply_posts_the_colourist_body() {
        let door = std::sync::Arc::new(ScriptedDoor::new());
        let mut h = with_door(&door);
        h.execute(&call(
            "colour_apply",
            json!({ "graph": "live-1", "group": "models", "hsb": [0.6, 0.8, 1.0] }),
        ))
        .expect("group");
        assert_eq!(
            posted(&door, "POST", "/colour/apply"),
            Some(json!({ "graph": "live-1", "targets": "models", "hsb": [0.6, 0.8, 1.0] }))
        );
        h.execute(&call(
            "colour_apply",
            json!({ "graph": "live-1", "nodes": ["m1", "e2"], "rgb": [1.0, 0.0, 0.0] }),
        ))
        .expect("nodes");
        let calls = door.calls.lock().unwrap();
        let last: Value = serde_json::from_slice(&calls.last().unwrap().2).unwrap();
        assert_eq!(
            last,
            json!({ "graph": "live-1", "targets": ["m1", "e2"], "rgb": [1.0, 0.0, 0.0] })
        );
    }

    /// M4 (rule 6): `set_material` posts `POST /graph/node {op:"config"}` with
    /// only the keys the call names — the exact merge the material page posts —
    /// and refuses a call that names none.
    #[test]
    fn set_material_posts_the_config_merge_the_panel_posts() {
        let door = std::sync::Arc::new(ScriptedDoor::new());
        let mut h = with_door(&door);
        h.execute(&call(
            "set_material",
            json!({ "graph": "g1", "node": "mat-0", "part": "skin", "rough": 0.4, "metal": 0.6 }),
        ))
        .expect("set_material");
        assert_eq!(
            posted(&door, "POST", "/graph/node"),
            Some(json!({
                "op": "config", "graph": "g1", "node": "mat-0",
                "config": { "part": "skin", "rough": 0.4, "metal": 0.6 }
            }))
        );
        // Only `part`: the knobs it does not name are left alone.
        h.execute(&call(
            "set_material",
            json!({ "graph": "g1", "node": "mat-0", "part": "eyes" }),
        ))
        .expect("set_material part only");
        let calls = door.calls.lock().unwrap();
        let last = calls.last().expect("two calls");
        let body: Value = serde_json::from_slice(&last.2).unwrap();
        assert_eq!(last.1, "/graph/node");
        assert_eq!(
            body,
            json!({ "op": "config", "graph": "g1", "node": "mat-0", "config": { "part": "eyes" } })
        );
        drop(calls);
        // Nothing to write is refused by name.
        assert!(h
            .execute(&call(
                "set_material",
                json!({ "graph": "g1", "node": "mat-0" })
            ))
            .is_err());
        // No door: the honest failure every door-bound tool gives.
        let err = harness()
            .execute(&call(
                "set_material",
                json!({ "graph": "g1", "node": "mat-0", "mix": 0.5 }),
            ))
            .unwrap_err();
        assert!(err.contains("with_door"), "{err}");
    }

    /// Without a door the three mask tools say so — the honest failure
    /// that replaced the placeholders which answered `ok: true` and did
    /// nothing (the `S.B` stubs this module carried until O.1's closure).
    #[test]
    fn the_mask_tools_refuse_honestly_without_a_door() {
        let mut h = harness();
        for (name, args) in [
            (
                "create_mask_grid",
                json!({ "outputs": ["screen-1"], "rows": 2, "cols": 2 }),
            ),
            ("test_mask_grid", json!({ "outputs": ["screen-1"] })),
            (
                "edit_mask",
                json!({ "device_id": "screen-1", "mask_id": "mask-1", "alpha": 0.5 }),
            ),
        ] {
            let err = h.execute(&call(name, args)).unwrap_err();
            assert!(err.contains("with_door"), "{name}: {err}");
        }
    }

    /// `create_mask_grid(outputs, rows, cols)` is `POST /mask/grid
    /// { device_ids, rows, cols }`, verbatim, and hands back the route's
    /// reply — S.B's *"thin wrappers over S.A's routes"*.
    #[test]
    fn create_mask_grid_posts_mask_grid_verbatim() {
        let door = std::sync::Arc::new(ScriptedDoor::new().json(
            "POST",
            "/mask/grid",
            json!({ "mask_ids": ["mask-1", "mask-2"], "devices": [] }),
        ));
        let mut h = with_door(&door);
        let out = h
            .execute(&call(
                "create_mask_grid",
                json!({ "outputs": ["a", "b"], "rows": 1, "cols": 2 }),
            ))
            .expect("create_mask_grid");
        assert_eq!(
            posted(&door, "POST", "/mask/grid"),
            Some(json!({ "device_ids": ["a", "b"], "rows": 1, "cols": 2 }))
        );
        assert_eq!(out["mask_ids"], json!(["mask-1", "mask-2"]));
        assert!(
            h.execute(&call("create_mask_grid", json!({ "rows": 1, "cols": 2 })))
                .is_err(),
            "no outputs, no call"
        );
    }

    /// O.1's Venn diagram from the harness: `edit_mask` posts exactly the
    /// body the lanes panel's mask editor posts for the same pick
    /// (`shell::panels::lanes::tests::picking_a_compose_op_posts_the_boolean_through_outputs`
    /// asserts this same JSON) — one route, two callers. A call naming no
    /// field is refused rather than turned into the delete the route would
    /// read it as; `remove: true` is the delete, said out loud.
    #[test]
    fn edit_mask_posts_outputs_mask_with_only_the_given_fields() {
        let door = std::sync::Arc::new(ScriptedDoor::new().json(
            "POST",
            "/outputs",
            json!({ "mask": "mask-3", "devices": [{ "id": "screen-1", "masks": [{ "id": "mask-3" }] }] }),
        ));
        let mut h = with_door(&door);
        let out = h
            .execute(&call(
                "edit_mask",
                json!({ "device_id": "screen-1", "mask_id": "mask-3",
                        "compose": { "op": "intersect", "a": "mask-1", "b": "mask-2" } }),
            ))
            .expect("edit_mask");
        assert_eq!(
            posted(&door, "POST", "/outputs"),
            Some(json!({ "op": "mask", "id": "screen-1", "mask": "mask-3",
                         "compose": { "op": "intersect", "a": "mask-1", "b": "mask-2" } }))
        );
        assert_eq!(out["touched"], "mask-3");
        assert_eq!(out["masks"], json!([{ "id": "mask-3" }]));

        let err = h
            .execute(&call(
                "edit_mask",
                json!({ "device_id": "screen-1", "mask_id": "mask-3" }),
            ))
            .unwrap_err();
        assert!(err.contains("nothing to edit"), "{err}");
        let err = h
            .execute(&call(
                "edit_mask",
                json!({ "device_id": "screen-1", "remove": true }),
            ))
            .unwrap_err();
        assert!(err.contains("needs `mask_id`"), "{err}");

        door.calls.lock().unwrap().clear();
        h.execute(&call(
            "edit_mask",
            json!({ "device_id": "screen-1", "mask_id": "mask-3", "remove": true }),
        ))
        .expect("remove");
        assert_eq!(
            posted(&door, "POST", "/outputs"),
            Some(json!({ "op": "mask", "id": "screen-1", "mask": "mask-3" }))
        );
    }

    /// G1: `add_effect` posts `POST /graph/effects` with the fields the
    /// graph editor's effect row posts; `fixed` carries `amount` as the
    /// route's `value`, and a fixed effect without an amount is refused.
    #[test]
    fn add_effect_posts_graph_effects_with_the_row_body() {
        let door = std::sync::Arc::new(
            ScriptedDoor::new().json("POST", "/graph/effects", json!({ "effects": [{ "name": "blur" }] })),
        );
        let mut h = with_door(&door);
        let out = h
            .execute(&call(
                "add_effect",
                json!({ "graph": "live-1", "node": "gen-1", "effect": "blur", "state": "fixed", "amount": 0.5 }),
            ))
            .expect("add_effect");
        assert_eq!(
            posted(&door, "POST", "/graph/effects"),
            Some(json!({ "graph": "live-1", "node": "gen-1", "effect": "blur", "state": "fixed", "value": 0.5 }))
        );
        assert_eq!(out["effects"], json!([{ "name": "blur" }]));
        let err = h
            .execute(&call(
                "add_effect",
                json!({ "graph": "live-1", "node": "gen-1", "effect": "blur", "state": "fixed" }),
            ))
            .unwrap_err();
        assert!(err.contains("amount"), "{err}");
    }

    /// G1: `remove_effect` is the same route with `state: "off"` — the
    /// removal is the route's own, and it is marked destructive.
    #[test]
    fn remove_effect_posts_state_off_and_is_destructive() {
        let door = std::sync::Arc::new(ScriptedDoor::new().json("POST", "/graph/effects", json!({ "effects": [] })));
        let mut h = with_door(&door);
        h.execute(&call(
            "remove_effect",
            json!({ "graph": "live-1", "node": "gen-1", "effect": "blur" }),
        ))
        .expect("remove_effect");
        assert_eq!(
            posted(&door, "POST", "/graph/effects"),
            Some(json!({ "graph": "live-1", "node": "gen-1", "effect": "blur", "state": "off" }))
        );
        assert!(metal_agent::tool_schemas::meta("remove_effect").is_some_and(|m| m.destructive));
    }

    /// G1: `edit_output` posts `POST /outputs` for add, remove and
    /// resolution; a resolution with no size and an unknown op are refused.
    #[test]
    fn edit_output_posts_add_remove_and_resolution() {
        // `posted` reads the first matching call, so each op gets a fresh door.
        let run = |args: Value| {
            let door = std::sync::Arc::new(ScriptedDoor::new().json("POST", "/outputs", json!({ "devices": [] })));
            let mut h = with_door(&door);
            let result = h.execute(&call("edit_output", args));
            (result, posted(&door, "POST", "/outputs"))
        };
        let (r, body) = run(json!({ "op": "add", "id": "screen-2", "label": "Pared", "width": 1280, "height": 720 }));
        r.expect("add");
        assert_eq!(
            body,
            Some(json!({ "op": "add", "id": "screen-2", "label": "Pared", "width": 1280, "height": 720 }))
        );
        let (r, body) = run(json!({ "op": "remove", "id": "screen-2" }));
        r.expect("remove");
        assert_eq!(body, Some(json!({ "op": "remove", "id": "screen-2" })));
        let (r, body) = run(json!({ "op": "resolution", "id": "screen-2", "width": 800 }));
        r.expect("resolution");
        assert_eq!(body, Some(json!({ "op": "resolution", "id": "screen-2", "width": 800 })));
        assert!(run(json!({ "op": "resolution", "id": "screen-2" })).0.is_err());
        assert!(run(json!({ "op": "rename", "id": "screen-2" })).0.is_err());
    }

    /// G1: `resolve_draft` posts `POST /venue { op: accept | reject, id }`,
    /// the diff buttons' body; any other decision is refused before a call.
    #[test]
    fn resolve_draft_posts_accept_or_reject() {
        let door = std::sync::Arc::new(ScriptedDoor::new().json(
            "POST",
            "/venue",
            json!({ "ok": true, "venue": "sala", "accepted": true }),
        ));
        let mut h = with_door(&door);
        let out = h
            .execute(&call("resolve_draft", json!({ "venue": "sala", "decision": "accept" })))
            .expect("accept");
        assert_eq!(posted(&door, "POST", "/venue"), Some(json!({ "op": "accept", "id": "sala" })));
        assert_eq!(out["accepted"], true);
        assert!(h.execute(&call("resolve_draft", json!({ "venue": "sala", "decision": "maybe" }))).is_err());
    }

    /// 3D venues: `project_venue` is three routes in order: the venue's
    /// masks onto the device, a solid colour the masks cut, and the same
    /// `POST /monitor` body the `open monitor` button posts.
    #[test]
    fn project_venue_writes_masks_then_fills_them_then_opens_the_monitor() {
        let door = std::sync::Arc::new(ScriptedDoor::new().json(
            "POST",
            "/venue",
            json!({ "ok": true, "devices": [{ "device": "screen-1", "masks": [{ "id": "bar.panel-left" }] }],
                    "report": { "problems": [] } }),
        ));
        let mut h = with_door(&door);
        let out = h
            .execute(&call(
                "project_venue",
                json!({ "venue": "bar", "device": "screen-1", "rgb": [255, 120, 40], "seconds": 12.0 }),
            ))
            .expect("project_venue");
        let order: Vec<String> = door
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, p, _)| p.clone())
            .collect();
        assert_eq!(order, ["/venue", "/overlay", "/monitor"]);
        assert_eq!(
            posted(&door, "POST", "/venue"),
            Some(json!({ "op": "project", "id": "bar", "device": "screen-1" }))
        );
        assert_eq!(
            posted(&door, "POST", "/overlay"),
            Some(
                json!({ "op": "solid", "device": "screen-1", "rgb": [255, 120, 40], "duration_seconds": 12.0 })
            )
        );
        assert_eq!(
            posted(&door, "POST", "/monitor"),
            Some(json!({ "op": "open", "device": "screen-1" }))
        );
        assert_eq!(out["devices"][0]["masks"][0]["id"], "bar.panel-left");

        door.calls.lock().unwrap().clear();
        h.execute(&call(
            "project_venue",
            json!({ "venue": "bar", "monitor": false }),
        ))
        .expect("no monitor");
        assert!(posted(&door, "POST", "/monitor").is_none());
        assert!(h.execute(&call("project_venue", json!({}))).is_err());
    }

    /// Per-mask test colours: when `/venue` answers a device's `solid` (each
    /// mask's own colour), the fill posts it as `masks`, with `rgb` only
    /// when the call gave one (then it is the colour outside the masks).
    #[test]
    fn project_venue_fills_each_mask_with_the_venues_own_colour() {
        let door = std::sync::Arc::new(ScriptedDoor::new().json(
            "POST",
            "/venue",
            json!({ "ok": true, "report": { "problems": [] },
                    "devices": [{ "device": "screen-1", "masks": [{ "id": "bar.panel-left" }, { "id": "bar.panel-right" }],
                                  "solid": { "bar.panel-left": [255, 120, 40], "bar.panel-right": [40, 140, 255] } }] }),
        ));
        let mut h = with_door(&door);
        h.execute(&call(
            "project_venue",
            json!({ "venue": "bar", "monitor": false, "seconds": 5.0 }),
        ))
        .expect("project_venue");
        assert_eq!(
            posted(&door, "POST", "/overlay"),
            Some(
                json!({ "op": "solid", "device": "screen-1", "duration_seconds": 5.0,
                         "masks": { "bar.panel-left": [255, 120, 40], "bar.panel-right": [40, 140, 255] } })
            )
        );

        door.calls.lock().unwrap().clear();
        h.execute(&call(
            "project_venue",
            json!({ "venue": "bar", "monitor": false, "rgb": [10, 10, 10] }),
        ))
        .expect("with an outside colour");
        let body = posted(&door, "POST", "/overlay").expect("posted");
        assert_eq!(body["rgb"], json!([10, 10, 10]));
        assert_eq!(body["masks"]["bar.panel-right"], json!([40, 140, 255]));
    }

    /// Venue 3D: `edit_venue_object` posts the route's object body verbatim
    /// (rule 6), naming the venue with `id` when one was given.
    #[test]
    fn edit_venue_object_posts_object() {
        let door = std::sync::Arc::new(ScriptedDoor::new().json(
            "POST",
            "/venue",
            json!({ "ok": true, "id": "bar", "object": { "id": "stage", "kind": "stage" } }),
        ));
        let mut h = with_door(&door);
        let out = h
            .execute(&call(
                "edit_venue_object",
                json!({ "venue": "bar", "object": "stage", "kind": "stage",
                        "at": [0.0, 0.0, 1.5], "size": [6.0, 0.4, 3.0], "yaw": 15.0,
                        "occludes": true, "emits": 0.0, "faces": ["front"] }),
            ))
            .expect("edit_venue_object");
        assert_eq!(
            posted(&door, "POST", "/venue"),
            Some(
                json!({ "op": "object", "id": "bar", "object": "stage", "kind": "stage",
                         "at": [0.0, 0.0, 1.5], "size": [6.0, 0.4, 3.0], "yaw": 15.0,
                         "occludes": true, "emits": 0.0, "faces": ["front"] })
            )
        );
        assert_eq!(out["object"]["id"], "stage");
        assert!(h.execute(&call("edit_venue_object", json!({}))).is_err());
    }

    /// Venue 3D: `add_screen` posts `{op:"screen", screen}` and keeps
    /// `aspect_lock`, the field the add-screen form uses to correct the other
    /// pair; `surface` stays at the body's top level, as the route reads it.
    #[test]
    fn add_screen_posts_screen_with_aspect_lock() {
        let door = std::sync::Arc::new(ScriptedDoor::new().json(
            "POST",
            "/venue",
            json!({ "ok": true, "id": "bar", "display": "led-wall",
                    "surface": "led-wall", "output": "led-wall", "aspect_locked": true }),
        ));
        let mut h = with_door(&door);
        let out = h
            .execute(&call(
                "add_screen",
                json!({ "venue": "bar", "surface": "led-wall",
                        "screen": { "name": "LED wall", "width_m": 8.0, "height_m": 4.5,
                                    "pixels": [1920, 1080], "kind": "led",
                                    "link": "hdmi", "node": "venue-pi",
                                    "aspect_lock": "pixels" } }),
            ))
            .expect("add_screen");
        assert_eq!(
            posted(&door, "POST", "/venue"),
            Some(json!({ "op": "screen", "id": "bar", "surface": "led-wall",
                         "screen": { "name": "LED wall", "width_m": 8.0, "height_m": 4.5,
                                     "pixels": [1920, 1080], "kind": "led",
                                     "link": "hdmi", "node": "venue-pi",
                                     "aspect_lock": "pixels" } }))
        );
        assert_eq!(out["aspect_locked"], true);
        assert!(h.execute(&call("add_screen", json!({}))).is_err());
    }

    /// Venue 3D: `venue_scene` is the read half — `GET /venue/scene?id=`,
    /// no POST — and hands the scene graph straight back.
    #[test]
    fn venue_scene_reads_the_route() {
        let scene = json!({ "venue": "bar", "items": [{ "kind": "object", "id": "stage" }],
                            "surfaces": [{ "id": "back-wall" }], "outputs": [], "masks": [] });
        let door =
            std::sync::Arc::new(ScriptedDoor::new().json("GET", "/venue/scene", scene.clone()));
        let mut h = with_door(&door);
        let out = h
            .execute(&call("venue_scene", json!({ "id": "bar" })))
            .expect("venue_scene");
        assert_eq!(out, scene);
        let calls = door.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "GET");
        assert_eq!(calls[0].1, "/venue/scene");
        assert!(calls.iter().all(|(m, _, _)| m != "POST"));
    }

    /// Venue 3D: the remaining writes post their route bodies verbatim —
    /// `surface.form`, `move` and the `area` read.
    #[test]
    fn surface_form_move_and_area_post_their_bodies() {
        let door = std::sync::Arc::new(ScriptedDoor::new().json(
            "POST",
            "/venue",
            json!({ "ok": true, "id": "bar" }),
        ));
        let mut h = with_door(&door);
        h.execute(&call(
            "set_surface_form",
            json!({ "venue": "bar", "surface": "stage-front", "form": "swept",
                    "path": [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
                    "profile": [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
                    "caps": false, "closed": false, "kind": "catmull_rom" }),
        ))
        .expect("set_surface_form");
        assert_eq!(
            posted(&door, "POST", "/venue").unwrap()["op"],
            "surface.form"
        );
        assert_eq!(posted(&door, "POST", "/venue").unwrap()["form"], "swept");

        door.calls.lock().unwrap().clear();
        h.execute(&call(
            "set_venue_sectors",
            json!({ "venue": "bar", "surface": "stage-front", "source": "points",
                    "ratio": 2.0, "points": [[0.3, 0.0], [0.3, 1.0]], "magnet": true }),
        ))
        .expect("set_venue_sectors");
        assert_eq!(
            posted(&door, "POST", "/venue").unwrap()["op"],
            "surface.sectors"
        );
        assert_eq!(posted(&door, "POST", "/venue").unwrap()["source"], "points");
        assert_eq!(posted(&door, "POST", "/venue").unwrap()["magnet"], true);

        door.calls.lock().unwrap().clear();
        h.execute(&call(
            "move_venue_item",
            json!({ "venue": "bar", "item": "stage", "at": [1.0, 0.0, 0.0], "yaw": 30.0 }),
        ))
        .expect("move_venue_item");
        assert_eq!(
            posted(&door, "POST", "/venue"),
            Some(json!({ "op": "move", "id": "bar", "item": "stage",
                         "at": [1.0, 0.0, 0.0], "yaw": 30.0 }))
        );

        door.calls.lock().unwrap().clear();
        h.execute(&call(
            "projection_area",
            json!({ "venue": "bar", "projector": "proj-1" }),
        ))
        .expect("projection_area");
        assert_eq!(
            posted(&door, "POST", "/venue"),
            Some(json!({ "op": "area", "id": "bar", "projector": "proj-1" }))
        );

        door.calls.lock().unwrap().clear();
        h.execute(&call(
            "add_projection",
            json!({ "venue": "bar", "projector": "proj-1", "on": "table",
                    "yaw": 10.0, "pitch": -20.0, "throw_ratio": 1.2, "aspect": 1.777,
                    "lumens": 6000 }),
        ))
        .expect("add_projection");
        assert_eq!(
            posted(&door, "POST", "/venue"),
            Some(
                json!({ "op": "projector", "id": "bar", "projector": "proj-1",
                         "on": "table", "yaw": 10.0, "pitch": -20.0,
                         "throw_ratio": 1.2, "aspect": 1.777, "lumens": 6000 })
            )
        );
    }

    /// D5 (rule 6): `solve_head` posts exactly the route body the venue
    /// editor's solve posts — `op: "solve_head"`, the fixture, the target
    /// and an optional venue id — and hands the route's answer (the live
    /// state and the beam's landing point) straight back; without a door it
    /// says so rather than posting nowhere.
    #[test]
    fn solve_head_posts_the_venue_solve_route_with_the_target() {
        let door = std::sync::Arc::new(ScriptedDoor::new().json(
            "POST",
            "/venue",
            json!({ "ok": true, "venue": "bar", "fixture": "wash-1",
                    "pan": 12.5, "tilt": -18.0, "hit": { "what": "back-wall", "at": [3.0, 1.5, -5.0] } }),
        ));
        let mut h = with_door(&door);
        let out = h
            .execute(&call(
                "solve_head",
                json!({ "venue": "bar", "fixture": "wash-1", "target": [3.0, 1.5, -5.0] }),
            ))
            .expect("solve_head");
        assert_eq!(
            posted(&door, "POST", "/venue"),
            Some(
                json!({ "op": "solve_head", "id": "bar", "fixture": "wash-1", "target": [3.0, 1.5, -5.0] })
            )
        );
        assert_eq!(out["hit"]["what"], "back-wall");
        assert_eq!(out["pan"], 12.5);

        // The venue is optional: without it the route uses the last one.
        door.calls.lock().unwrap().clear();
        h.execute(&call(
            "solve_head",
            json!({ "fixture": "wash-1", "target": [3.0, 1.5, -5.0] }),
        ))
        .expect("without a venue");
        assert_eq!(
            posted(&door, "POST", "/venue"),
            Some(json!({ "op": "solve_head", "fixture": "wash-1", "target": [3.0, 1.5, -5.0] }))
        );

        // A call naming no target (or no fixture) is refused by name.
        assert!(h
            .execute(&call("solve_head", json!({ "fixture": "wash-1" })))
            .is_err());
        assert!(h
            .execute(&call("solve_head", json!({ "target": [0.0, 0.0, 0.0] })))
            .is_err());

        let err = harness()
            .execute(&call(
                "solve_head",
                json!({ "fixture": "wash-1", "target": [0.0, 0.0, 0.0] }),
            ))
            .unwrap_err();
        assert!(err.contains("with_door"), "{err}");
    }

    /// The camera tools read the venue through the door; the bar example has a
    /// projector (`proj-1`) throwing on `back-wall` and four washes. One
    /// scripted reply serves both `GET /venue` (the list) and `GET /venue?id=`
    /// because the double keys on `(method, path)` alone.
    fn camera_door() -> std::sync::Arc<ScriptedDoor> {
        let v = crate::venue::load(&crate::output::data_root(), "bar").expect("the bar example");
        std::sync::Arc::new(
            ScriptedDoor::new()
                .json(
                    "GET",
                    "/venue",
                    json!({ "venues": [{ "id": "bar" }], "last": "bar", "venue": v }),
                )
                .json(
                    "POST",
                    "/venue",
                    json!({ "ok": true, "venue": "bar", "draft": { "venue": {}, "diff": [] } }),
                ),
        )
    }

    /// M5: the positive `capture_camera_frame` renders the venue's room from
    /// the seam and hands back a real PNG — no camera on the machine.
    #[test]
    fn capture_camera_frame_renders_the_venue_room_from_the_seam() {
        let door = camera_door();
        let mut h = with_door(&door);
        let out = h
            .execute(&call("capture_camera_frame", json!({ "w": 64, "h": 36 })))
            .expect("capture_camera_frame");
        assert_eq!(out["ok"], true, "{out}");
        assert_eq!(out["device"], "synthetic");
        assert_eq!((out["w"].as_u64(), out["h"].as_u64()), (Some(64), Some(36)));
        let png = metal_agent::harness::base64_decode(out["png_base64"].as_str().unwrap())
            .expect("base64");
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    }

    /// M5: `scan_venue` solves the projector's pins against every surface it
    /// lights and posts the venue TOML — with the `pins` table in it — as a
    /// draft the operator accepts or rejects.
    #[test]
    fn scan_venue_solves_the_projectors_pins_and_posts_a_draft() {
        let door = camera_door();
        let mut h = with_door(&door);
        let out = h
            .execute(&call("scan_venue", json!({ "venue": "bar" })))
            .expect("scan_venue");
        assert_eq!(out["ok"], true, "{out}");
        let scanned = out["scanned"].as_array().unwrap();
        assert!(
            scanned.iter().any(|s| s["projector"] == "proj-1"
                && s["surface"] == "back-wall"
                && s["points"].is_array()),
            "the back wall should solve: {scanned:?}"
        );
        let body = posted(&door, "POST", "/venue").expect("a draft");
        assert_eq!(body["op"], "draft");
        assert!(
            body["venue"].as_str().unwrap().contains("pins"),
            "the draft TOML carries the pins"
        );
    }

    /// M5: `discover_fixture` flashes each fixture in the synthetic feed and
    /// reports the ones the camera saw.
    #[test]
    fn discover_fixture_reports_the_flashed_fixtures() {
        let door = camera_door();
        let mut h = with_door(&door);
        let out = h
            .execute(&call("discover_fixture", json!({ "venue": "bar" })))
            .expect("discover_fixture");
        assert_eq!(out["ok"], true, "{out}");
        let ids: Vec<&str> = out["discovered"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|d| d["fixture"].as_str())
            .collect();
        assert!(ids.iter().any(|i| i.starts_with("wash")), "{ids:?}");
    }

    /// M5: without `pins`, `calibrate_projector` solves them from the camera
    /// and posts a draft; with `pins` it still posts the P3 `op: "pins"`
    /// route unchanged.
    #[test]
    fn calibrate_projector_solves_when_pins_are_absent_and_posts_pins_when_given() {
        let door = camera_door();
        let mut h = with_door(&door);
        let out = h
            .execute(&call(
                "calibrate_projector",
                json!({ "venue": "bar", "projector": "proj-1", "surface": "back-wall" }),
            ))
            .expect("calibrate_projector");
        assert_eq!(out["ok"], true, "{out}");
        assert!(out["pins"].as_array().is_some(), "{out}");
        assert_eq!(posted(&door, "POST", "/venue").unwrap()["op"], "draft");

        door.calls.lock().unwrap().clear();
        h.execute(&call(
            "calibrate_projector",
            json!({ "venue": "bar", "projector": "proj-1", "surface": "back-wall",
                    "pins": [[0, 0], [1, 0], [1, 1], [0, 1]] }),
        ))
        .expect("calibrate_projector with pins");
        let body = posted(&door, "POST", "/venue").expect("a call");
        assert_eq!(body["op"], "pins");
        assert_eq!(body["pins"], json!([[0, 0], [1, 0], [1, 1], [0, 1]]));
    }

    /// `look_venue` reads the venue (`GET /venue`), then every output it
    /// drives (`GET /frame`), and hands back the scene, the legend and the
    /// frames — the frames as `jpeg_base64`, which the loop lifts into
    /// images the model sees. `camera: false` leaves the camera out, the
    /// way a machine with none would.
    #[test]
    fn look_venue_reads_the_scene_the_legend_and_every_outputs_frame() {
        let v = crate::venue::load(&crate::output::data_root(), "living-room").expect("example");
        let door = std::sync::Arc::new(
            ScriptedDoor::new()
                .json("GET", "/venue", json!({ "venue": v, "report": { "problems": [] }, "outputs": [], "compositions": [] }))
                .json("GET", "/venue/state", json!({ "venue": "living-room", "fixtures": [] }))
                .bytes("GET", "/frame", "image/jpeg", vec![0xff, 0xd8, 0xff, 0xd9]),
        );
        let mut h = with_door(&door);
        let out = h
            .execute(&call("look_venue", json!({ "camera": false })))
            .expect("look_venue");
        assert_eq!(out["venue"]["id"], "living-room");
        let outputs = out["outputs"].as_array().unwrap();
        let devices: Vec<&str> = outputs
            .iter()
            .map(|o| o["device"].as_str().unwrap())
            .collect();
        assert_eq!(
            devices,
            ["proyector-sala", "tv"],
            "the projector's output, then the TV's"
        );
        assert!(outputs.iter().all(|o| o["jpeg_base64"].is_string()));
        assert_eq!(outputs[1]["driven_by"], "display tv");
        assert_eq!(out["masks"].as_array().unwrap().len(), v.masks.len());
        assert_eq!(
            out["masks"][0]["n"], 1,
            "numbered as drawn on the camera overlay"
        );
        assert!(
            out["fixtures"].is_array(),
            "M6: every fixture with its live state"
        );
        assert!(
            out["camera"].is_null() && out["design"].is_null(),
            "no camera asked; a designed shot has no image"
        );
        let order: Vec<String> = door
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, p, _)| p.clone())
            .collect();
        assert_eq!(order, ["/venue", "/frame", "/frame", "/venue/state"]);
    }

    /// `listen_status` is `GET /listen/status`, answered as the route answers
    /// — the AI reads `real: false` off it and says it is a stand-in, and the
    /// `tone` object's three keys come through with it (the tone design notes §6 step 11).
    #[test]
    fn listen_status_reads_the_listen_route() {
        let door = std::sync::Arc::new(ScriptedDoor::new().json(
            "GET",
            "/listen/status",
            json!({ "nodes": [{ "node": "audio-1", "real": false, "short": "stand-in",
                "tone": { "loaded": false, "real": false, "latency_ms": 4.5 } }] }),
        ));
        let tmp =
            std::env::temp_dir().join(format!("metal-dash-listen-status-{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        let real = Manifest::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json"),
        )
        .expect("data/schema.json");
        let mut h = AgentHarness::new(metal_agent::harness::Harness::new(tmp), real)
            .with_door(Box::new(std::sync::Arc::clone(&door)));
        let out = h
            .execute(&call("listen_status", json!({})))
            .expect("listen_status");
        assert_eq!(out["nodes"][0]["real"], json!(false));
        let tone = &out["nodes"][0]["tone"];
        assert!(
            tone["loaded"].is_boolean()
                && tone["real"].is_boolean()
                && tone["latency_ms"].is_number()
        );
        assert_eq!(tone["latency_ms"], json!(4.5));
    }

    /// `edit_layer` is `POST /layers`, with a model named by id resolved to
    /// the table index the route takes.
    #[test]
    fn edit_layer_names_a_model_by_id_and_posts_the_layer_route() {
        let door = std::sync::Arc::new(ScriptedDoor::new().json(
            "POST",
            "/layers",
            json!({ "devices": [{ "id": "room", "layers": [{ "id": "l0-0" }] }], "layer": { "id": "l0-0" } }),
        ));
        // The real table: `forge` is appended at its end, and the fixture
        // manifest above has one model only.
        let real = Manifest::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json"),
        )
        .expect("data/schema.json");
        let forge = real
            .models
            .iter()
            .position(|m| m.id == "forge")
            .expect("forge in the manifest");
        let tmp =
            std::env::temp_dir().join(format!("metal-dash-edit-layer-{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        let mut h = AgentHarness::new(metal_agent::harness::Harness::new(tmp), real)
            .with_door(Box::new(std::sync::Arc::clone(&door)));
        let out = h
            .execute(&call(
                "edit_layer",
                json!({ "op": "add", "device": "room", "model": "forge", "shader": "turtle", "blend": "screen" }),
            ))
            .expect("edit_layer");
        let body = posted(&door, "POST", "/layers").unwrap();
        assert_eq!(body["model"], json!(forge));
        assert_eq!(
            (body["shader"].as_str(), body["blend"].as_str()),
            (Some("turtle"), Some("screen"))
        );
        assert_eq!(out["graph"], "device-room");
        assert_eq!(out["layer"]["id"], "l0-0");
        assert!(h
            .execute(&call(
                "edit_layer",
                json!({ "op": "add", "device": "room", "model": "nope" })
            ))
            .is_err());
        assert!(
            h.execute(&call("edit_layer", json!({ "device": "room" })))
                .is_err(),
            "an op is required"
        );
    }

    /// `test_mask_grid(outputs)` returns the route's JPEG bytes, base64,
    /// with a legend built from the same colour rule the route drew with.
    #[test]
    fn test_mask_grid_returns_the_frame_and_the_legend() {
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3];
        let door = std::sync::Arc::new(
            ScriptedDoor::new()
                .bytes("POST", "/mask/project", "image/jpeg", jpeg.clone())
                .json(
                    "GET",
                    "/state",
                    json!({ "outputs": [{ "id": "screen-1", "masks": [
                    { "id": "mask-1", "name": "a" }, { "id": "mask-2", "name": "b" }
                ] }] }),
                ),
        );
        let mut h = with_door(&door);
        let out = h
            .execute(&call("test_mask_grid", json!({ "outputs": ["screen-1"] })))
            .expect("test_mask_grid");
        assert_eq!(
            posted(&door, "POST", "/mask/project"),
            Some(json!({ "device_id": "screen-1" }))
        );
        let frame = &out["frames"][0];
        assert_eq!(frame["device_id"], "screen-1");
        assert_eq!(frame["mime"], "image/jpeg");
        assert_eq!(
            frame["jpeg_base64"],
            metal_agent::harness::base64_encode(&jpeg)
        );
        assert_eq!(frame["legend"][0]["id"], "mask-1");
        assert_eq!(
            frame["legend"][1]["rgb"],
            json!(crate::output::mask_color(1))
        );
    }

    #[test]
    fn set_keyframe_writes_byte_identical_to_save_example_and_replaces_by_tick() {
        let model = "geoterrain";
        let song = "sk-agent-tools-song";
        let _ = fs::remove_file(example_path(model, song));

        let mut h = harness();
        h.execute(&call(
            "set_keyframe",
            json!({ "model": model, "song": song, "tick": 5, "floats": [1.0] }),
        ))
        .expect("first keyframe");
        // Replacing the same tick, not appending a second one.
        h.execute(&call(
            "set_keyframe",
            json!({ "model": model, "song": song, "tick": 5, "floats": [2.0] }),
        ))
        .expect("replace keyframe");
        h.execute(&call(
            "set_keyframe",
            json!({ "model": model, "song": song, "tick": 1, "floats": [0.5] }),
        ))
        .expect("second keyframe");
        let via_tool = fs::read_to_string(example_path(model, song)).unwrap();

        let loaded = example::load(Path::new(EXAMPLES_DIR), model, song, None).unwrap();
        assert_eq!(
            loaded.ticks.len(),
            2,
            "tick 5 replaced in place, tick 1 added"
        );
        // Base ticks 5 and 1 land canonical at 20 and 4 (the tool speaks the
        // active base level, storage is canonical).
        assert_eq!(
            loaded.ticks[0].tick, 4,
            "sorted ascending, on the canonical grid"
        );
        assert_eq!(loaded.ticks[1].floats, vec![2.0]);

        let _ = fs::remove_file(example_path(model, song));

        // Same sequence, straight through `example::save` — the function
        // `POST /example` calls.
        let schema = manifest().model(model).unwrap().clone();
        let mut ex = example::Example {
            model: model.to_string(),
            song: song.to_string(),
            rate: metal_schema::TickRate::CANONICAL.hz() as f32,
            ticks: vec![],
        };
        ex.ticks.push(example::Tick {
            tick: 20,
            floats: vec![1.0],
        });
        example::save(Path::new(EXAMPLES_DIR), ex.clone(), &schema, None).unwrap();
        let mut ex = example::load(Path::new(EXAMPLES_DIR), model, song, None).unwrap();
        ex.ticks.retain(|t| t.tick != 20);
        ex.ticks.push(example::Tick {
            tick: 20,
            floats: vec![2.0],
        });
        example::save(Path::new(EXAMPLES_DIR), ex.clone(), &schema, None).unwrap();
        let mut ex = example::load(Path::new(EXAMPLES_DIR), model, song, None).unwrap();
        ex.ticks.push(example::Tick {
            tick: 4,
            floats: vec![0.5],
        });
        example::save(Path::new(EXAMPLES_DIR), ex, &schema, None).unwrap();
        let via_route = fs::read_to_string(example_path(model, song)).unwrap();

        assert_eq!(via_tool, via_route);
        let _ = fs::remove_file(example_path(model, song));
    }

    /// The active tick level is read off the same `/settings` route the shell
    /// control writes, and a tick-addressed tool speaks it: the same canonical
    /// moment is fine tick 128 or base tick 32, and both land on canonical 128.
    #[test]
    fn agent_tools_speak_the_active_level() {
        let model = "geoterrain";
        let (song_fine, song_base) = ("sk-speak-fine", "sk-speak-base");
        let _ = fs::remove_file(example_path(model, song_fine));
        let _ = fs::remove_file(example_path(model, song_base));

        // Fine level: 128 fine ticks is canonical 128 (identity).
        let door = std::sync::Arc::new(ScriptedDoor::new().json(
            "GET",
            "/settings",
            json!({ "tick_level": "fine" }),
        ));
        let mut h = with_door(&door);
        let out = h
            .execute(&call(
                "set_keyframe",
                json!({ "model": model, "song": song_fine, "tick": 128, "floats": [1.0] }),
            ))
            .expect("fine keyframe");
        assert_eq!(out["tick_level"], json!("fine"));
        assert_eq!(
            out["tick"],
            json!(128),
            "the caller's fine tick comes back unchanged"
        );
        let stored = example::load(Path::new(EXAMPLES_DIR), model, song_fine, None).unwrap();
        assert_eq!(stored.ticks[0].tick, 128, "fine 128 is canonical 128");

        // get_tick_level reads the same route.
        let out = h
            .execute(&call("get_tick_level", json!({})))
            .expect("get_tick_level");
        assert_eq!(out["tick_level"], json!("fine"));

        // Base level: the same canonical moment is base tick 32 (×4).
        let door = std::sync::Arc::new(ScriptedDoor::new().json(
            "GET",
            "/settings",
            json!({ "tick_level": "base" }),
        ));
        let mut h = with_door(&door);
        let out = h
            .execute(&call(
                "set_keyframe",
                json!({ "model": model, "song": song_base, "tick": 32, "floats": [1.0] }),
            ))
            .expect("base keyframe");
        assert_eq!(out["tick_level"], json!("base"));
        let stored = example::load(Path::new(EXAMPLES_DIR), model, song_base, None).unwrap();
        assert_eq!(stored.ticks[0].tick, 128, "base 32 is canonical 128");

        let _ = fs::remove_file(example_path(model, song_fine));
        let _ = fs::remove_file(example_path(model, song_base));
    }

    #[test]
    fn set_keyframe_rejects_a_wrong_width_row() {
        let model = "geoterrain";
        let song = "sk-agent-tools-bad-width";
        let mut h = harness();
        let err = h
            .execute(&call(
                "set_keyframe",
                json!({ "model": model, "song": song, "tick": 0, "floats": [1.0, 2.0] }),
            ))
            .unwrap_err();
        assert!(!err.is_empty());
        let _ = fs::remove_file(example_path(model, song));
    }

    #[test]
    fn capture_output_without_a_layer_id_is_refused() {
        let mut h = harness();
        let err = h.execute(&call("capture_output", json!({}))).unwrap_err();
        assert!(err.contains("needs node"), "{err}");
    }

    // -- S.L: attach_digest --------------------------------------------

    fn fake_digester() -> std::path::PathBuf {
        let script = std::env::temp_dir().join("fw-attach-digest-fake.py");
        fs::write(
            &script,
            "import sys, numpy as np\nnp.save(sys.argv[2], np.zeros((12, 16), dtype='float32'))\n",
        )
        .unwrap();
        script
    }

    /// A one-clip library at `data/reels/<name>/index.json`, the shape
    /// `metal_reel::index::build` writes — without laddering a real video,
    /// which `attach_digest` never reads (the digester opens the source
    /// file directly).
    fn one_clip_library(name: &str, source: &Path, file: &str) {
        let dir = Path::new("data/reels").join(name);
        fs::create_dir_all(&dir).unwrap();
        let lib = metal_reel::index::Library {
            name: name.to_string(),
            source: source.display().to_string(),
            rate: 4.0,
            width: 64,
            clips: vec![metal_reel::index::Clip {
                id: "clip-a".into(),
                file: file.into(),
                seconds: 1.5,
                stamp: metal_reel::index::Stamp {
                    bytes: 0,
                    modified: 0,
                },
                rows: vec![[0.0; metal_reel::read::COLUMNS]; 6],
                passages: vec![],
            }],
            root: dir.clone(),
        };
        fs::write(dir.join("index.json"), serde_json::to_string(&lib).unwrap()).unwrap();
    }

    fn media_segment(id: &str, library: &str) -> Value {
        json!({
            "id": id, "from": 0, "to": 50,
            "library": library, "clip": "clip-a",
            "speed": 0.0, "reverse": false, "layer": 0
        })
    }

    #[test]
    fn attach_digest_writes_its_own_matrix_and_stamps_the_segment() {
        let tl_id = "sl-attach-digest-basic";
        let seg_id = "m1";
        let lib_name = "sl-attach-digest-reel";

        let source = std::env::temp_dir().join("fw-attach-digest-footage");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("clip.mp4"),
            b"stand-in bytes; the fake digester never opens this",
        )
        .unwrap();
        one_clip_library(lib_name, &source, "clip.mp4");

        fresh_timeline(tl_id);
        let add = json!({ "timeline": tl_id, "track": "media", "op": "add", "segment": media_segment(seg_id, lib_name) });
        super::super::edit_segment(&add).expect("seed media segment");

        unsafe { std::env::set_var("FW_DIGESTER", fake_digester()) };
        let mut h = harness();
        let out = h
            .execute(&call(
                "attach_digest",
                json!({ "timeline": tl_id, "segment_id": seg_id }),
            ))
            .expect("attach_digest");
        unsafe { std::env::remove_var("FW_DIGESTER") };

        assert_eq!(out["ok"], true);
        assert_eq!(out["digest"], seg_id);
        let matrix = Path::new("data/digest").join(metal_schema::digest_file_name(
            seg_id,
            metal_schema::TickRate::Base,
        ));
        assert!(
            metal_vfs::is_file(&matrix),
            "the clip's own matrix must land in data/digest/"
        );

        let saved = timeline::load(Path::new(crate::author::TIMELINES), tl_id).unwrap();
        assert_eq!(saved.media_track[0].digest, Some(seg_id.to_string()));

        let _ = fs::remove_file(timeline_path(tl_id));
        let _ = fs::remove_file(&matrix);
        let _ = fs::remove_dir_all(Path::new("data/reels").join(lib_name));
    }

    #[test]
    fn attach_digest_refuses_a_segment_that_is_not_on_the_timeline() {
        let tl_id = "sl-attach-digest-missing-segment";
        fresh_timeline(tl_id);
        let mut h = harness();
        let err = h
            .execute(&call(
                "attach_digest",
                json!({ "timeline": tl_id, "segment_id": "ghost" }),
            ))
            .unwrap_err();
        assert!(err.contains("ghost"), "{err}");
        let _ = fs::remove_file(timeline_path(tl_id));
    }
}
