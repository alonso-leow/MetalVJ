//! Everything that mutates `state.outputs`/`state.sources`/`state.layers`/
//! `state.collections`: the outputs table, a device's layer stack, the mask
//! grid helper (`S.A.1`) and the collections library. Grouped together
//! because every one of them ends the same way — write the outputs (and
//! sometimes the collections) file through and hand back `devices()`.

use serde_json::{json, Value};

use crate::call::{CallError, Ctx, Params};
use crate::layers::{Collection, Layer};
use crate::output::{BoolOp, Compose, Mask, MaskStroke, Output};
use crate::PREVIEW;

pub fn outputs(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let state = &mut *ctx.state;
    let id = body["id"].as_str().unwrap_or_default().to_string();
    match body["op"].as_str().unwrap_or_default() {
        "add" => {
            if id.is_empty() {
                return Err(CallError::Bad("a device needs an id".into()));
            }
            if id == PREVIEW {
                return Err(CallError::Bad("that id is reserved for the preview".into()));
            }
            if state.outputs.find(&id).is_none() {
                let label = body["label"].as_str().unwrap_or(&id).to_string();
                let w = body["width"].as_u64().unwrap_or(1920) as u32;
                let h = body["height"].as_u64().unwrap_or(1080) as u32;
                state.outputs.list.push(Output::new(&id, &label, w, h));
                state
                    .sources
                    .entry(id.clone())
                    .or_insert_with(crate::layers::Device::default);
                crate::ensure_device_graph_at(&crate::output::data_root(), &id, &label);
            }
        }
        "remove" => {
            state.outputs.list.retain(|o| o.id != id);
            state.sources.remove(&id);
            crate::layers::unroute_device(&mut state.layers, &id);
        }
        "resolution" => {
            if let Some(out) = state.outputs.find_mut(&id) {
                if let Some(w) = body["width"].as_u64() {
                    out.width = (w as u32).max(1);
                }
                if let Some(h) = body["height"].as_u64() {
                    out.height = (h as u32).max(1);
                }
            }
        }
        "calibrate" => {
            if let Some(out) = state.outputs.find_mut(&id) {
                out.calibrating = body["on"].as_bool().unwrap_or(!out.calibrating);
            }
        }
        "node" => {
            let Some(out) = state.outputs.find_mut(&id) else {
                return Err(CallError::Bad("no such device".into()));
            };
            out.node = body["node"].as_str().map(str::to_string);
        }
        "composed_on" => {
            let Some(out) = state.outputs.find_mut(&id) else {
                return Err(CallError::Bad("no such device".into()));
            };
            out.composed_on = body["composed_on"].as_str().map(str::to_string);
        }
        // O.20/S.AC: make a device a DMX head (or clear it). `fixture` names a
        // profile in `data/fixtures/`; unknown names are refused so an agent
        // cannot put a head on a model that does not exist. `universe` and
        // `address` are per-head overrides of the profile's defaults.
        "head" => {
            let device = body["device"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(&id)
                .to_string();
            match body["fixture"].as_str().filter(|s| !s.trim().is_empty()) {
                Some(fixture) => {
                    if crate::fixture::get(&crate::output::data_root(), fixture).is_none() {
                        return Err(CallError::Bad(format!("no such fixture: {fixture}")));
                    }
                    let Some(dev) = state.sources.get_mut(&device) else {
                        return Err(CallError::Bad("no such device".into()));
                    };
                    dev.head = Some(crate::fixture::Head {
                        fixture: fixture.to_string(),
                        universe: body["universe"].as_u64().map(|v| v as u16),
                        address: body["address"].as_u64().map(|v| v as u16),
                    });
                }
                None => {
                    if let Some(dev) = state.sources.get_mut(&device) {
                        dev.head = None;
                    }
                }
            }
        }
        "cede" => {
            let Some(out) = state.outputs.find_mut(&id) else {
                return Err(CallError::Bad("no such device".into()));
            };
            out.ceded_to = body["to"].as_str().map(str::to_string);
        }
        // P-C: hand this screen's frame to a virtual camera / NDI / Spout /
        // Syphon too. `kind: null` (or absent) clears it; a kind that is off
        // on this machine is refused with the reason `GET /videoio` shows.
        "sink" => {
            let Some(out) = state.outputs.find_mut(&id) else {
                return Err(CallError::Bad("no such device".into()));
            };
            match body["kind"].as_str().filter(|k| !k.is_empty() && *k != "none") {
                None => out.sink = None,
                Some(kind) => {
                    crate::call::videoio::check_sink(kind, &crate::call::videoio::modes_json())
                        .map_err(CallError::Bad)?;
                    out.sink = Some(crate::output::OutputSink {
                        kind: kind.to_string(),
                        target: body["target"].as_str().unwrap_or_default().to_string(),
                    });
                }
            }
        }
        "mask" => {
            let Some(out) = state.outputs.find_mut(&id) else {
                return Err(CallError::Bad("no such device".into()));
            };
            let at = match body["mask"].as_str() {
                Some(mid) => out.masks.iter().position(|m| m.id == mid),
                None => body["index"]
                    .as_u64()
                    .map(|v| v as usize)
                    .filter(|v| *v < out.masks.len()),
            };
            let before = out.masks.len();
            if let Some(gone) = edit_mask(&mut out.masks, at, &body).map_err(CallError::Bad)? {
                state.unbind(&gone);
            } else if at.is_none() && out.masks.len() > before {
                // A new contour addressed by an id that is not here yet takes
                // that id: the same object contoured on a second projector
                // keeps its name, so a source routed to both is cut to it on
                // both (`Outputs::name_masks` has the whole argument).
                if let Some(mid) = body["mask"].as_str().filter(|m| !m.trim().is_empty()) {
                    if let Some(new) = out.masks.last_mut() {
                        new.id = mid.to_string();
                    }
                }
            }
        }
        // P5: the overlay trigger's destination is stored as the device's
        // own `Trigger` (the plan's "the config is a reading of the graph"):
        // `params` carries `device`/`layer`/`loop`, and `target` is
        // `overlay-<movement>`, so `/state` can report it and the firing
        // route can act on it. One writer, `edit_mask`'s own save-and-reply
        // shape.
        "trigger" => {
            let device = body["device"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| CallError::Bad("a trigger needs a device".into()))?;
            let Some(dev) = state.sources.get_mut(device) else {
                return Err(CallError::Bad("no such device".into()));
            };
            set_trigger(dev, &body["trigger"]).map_err(CallError::Bad)?;
        }
        "trigger-remove" => {
            let device = body["device"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| CallError::Bad("a trigger needs a device".into()))?;
            let tid = body["id"].as_str().unwrap_or_default();
            let Some(dev) = state.sources.get_mut(device) else {
                return Err(CallError::Bad("no such device".into()));
            };
            dev.triggers.retain(|t| t.id != tid);
        }
        // D3: the universe bus. Config is a reading of the graph (rule 4), so
        // this writes `priority`/`mode` into the source nodes' own config
        // (never a second state) — one node, one device's every source, or
        // (no device) every device's every source. The bus reads it back at
        // drive time (`metal_perform::dmx_bus::bus_source`).
        "dmx-bus" => {
            let priority = body["priority"].as_u64().map(|v| v.min(u8::MAX as u64) as u8);
            let mode = match body["mode"].as_str().map(str::to_ascii_lowercase) {
                Some(m) if matches!(m.as_str(), "htp" | "input" | "software") => Some(m),
                Some(other) => {
                    return Err(CallError::Bad(format!(
                        "dmx-bus mode is \"htp\", \"input\" or \"software\", not {other:?}"
                    )))
                }
                None => None,
            };
            if priority.is_none() && mode.is_none() {
                return Err(CallError::Bad("dmx-bus needs `priority` and/or `mode`".into()));
            }
            let only_node = body["node"].as_str().map(str::to_string);
            let devices: Vec<String> = match body["device"].as_str() {
                Some(d) => vec![d.to_string()],
                None => state.outputs.list.iter().map(|o| o.id.clone()).collect(),
            };
            let root = crate::output::data_root();
            let mut edited = 0u32;
            for device in &devices {
                let Some(mut g) = metal_perform::engine::device_graph_at(&root, device) else {
                    continue;
                };
                let mut changed = false;
                for n in g.nodes.iter_mut() {
                    if n.bypass {
                        continue;
                    }
                    if let Some(only) = &only_node {
                        if &n.id != only {
                            continue;
                        }
                    } else if !matches!(
                        n.kind.as_str(),
                        "out.sacn" | "out.artnet" | metal_graph::VENUE_FIXTURES_KIND
                    ) {
                        continue;
                    }
                    let mut cfg: serde_json::Map<String, Value> =
                        serde_json::from_str::<Value>(&n.config.to_json_str())
                            .ok()
                            .and_then(|v| v.as_object().cloned())
                            .unwrap_or_default();
                    if let Some(p) = priority {
                        cfg.insert("priority".into(), json!(p));
                    }
                    if let Some(m) = &mode {
                        cfg.insert("mode".into(), json!(m));
                    }
                    n.config = metal_graph::Config::from_json_str(&Value::Object(cfg).to_string())
                        .map_err(|e| CallError::Bad(format!("dmx-bus: {e}")))?;
                    changed = true;
                    edited += 1;
                }
                if changed {
                    metal_perform::engine::save_graph_at(&root, &g)
                        .map_err(|e| CallError::Bad(format!("dmx-bus: {e}")))?;
                }
            }
            if edited == 0 {
                return Err(CallError::Bad("dmx-bus: no matching source node".into()));
            }
        }
        other => return Err(CallError::Bad(format!("no such op: {other}"))),
    }
    if let Err(e) = state.outputs.save(&state.sources, &state.layers) {
        eprintln!("  outputs not saved: {e}");
    }
    let touched = crate::touched_mask(state, &id, &body);
    let time = state.time();
    let air = state.airing(ctx.sim, time);
    let models = ctx.paint.model_count();
    Ok(json!({
        "devices": ctx.state.devices(&air, models),
        "mask": touched,
    }))
}

/// P5: insert or replace one device's trigger by id — the one writer of
/// `Device.triggers` (`POST /outputs {op:"trigger"}`). `params` carries the
/// overlay's destination (`device`/`layer`/`loop`); `target` is
/// `overlay-<movement>`, the value `POST /trigger/<id>` resolves.
pub(crate) fn set_trigger(dev: &mut crate::layers::Device, t: &Value) -> Result<(), String> {
    let tid = t["id"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "a trigger needs an id".to_string())?;
    let target = t["target"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "a trigger needs a target".to_string())?;
    let params = t.get("params").cloned().unwrap_or_else(|| json!({}));
    let next = crate::trigger::Trigger {
        id: tid.to_string(),
        source: crate::trigger::TriggerSource::HttpPost {
            route: tid.to_string(),
        },
        target: target.to_string(),
        params,
    };
    match dev.triggers.iter_mut().find(|x| x.id == tid) {
        Some(existing) => *existing = next,
        None => dev.triggers.push(next),
    }
    Ok(())
}

pub fn layers(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let models = ctx.paint.model_count();
    let limits = ctx.state.limits;
    // Every device the addressed layer is routed to, before the op (a
    // `remove` or a `route {on:false}` takes it off devices whose graphs
    // still list it) — the post-op half is added below.
    let lid = body["layer"].as_str().map(str::to_string);
    let mut touched: Vec<String> = lid
        .as_deref()
        .and_then(|l| ctx.state.layers.iter().find(|x| x.id == l))
        .map(|l| l.outputs.clone())
        .unwrap_or_default();
    let created = match crate::apply_layers_op(ctx.state, models, limits, &body) {
        Ok(created) => created,
        Err(e) => return Err(CallError::Bad(e)),
    };

    if let Err(e) = ctx
        .state
        .outputs
        .save(&ctx.state.sources, &ctx.state.layers)
    {
        eprintln!("  outputs not saved: {e}");
    }
    // LAY.8c: the graph is the list. Every op above mutates the pool — the
    // table's own copy — and this is the one place that writes what it now
    // holds into the device's graph: each routed layer a `gen.*` node, the z
    // order the edge order, and the mode the op just resolved as the
    // composition node (`add`/`route`/`duplicate`/`apply` flip it to `Own`;
    // the dropdown writes its own). A device with no graph is a no-op — the
    // canvas has exactly two creators, and the boot pass fills existing
    // installs — so the test suite pointed at the repo's `data/` stays clean.
    //
    // **Every device the layer is on, not only the one the body names.** A
    // layer routed to two devices is one source, and each device's graph
    // carries a copy of its pins (sector, mask, opacity) that overrides the
    // pool when the device renders (`NodeLayerConfig::apply_to`). Syncing
    // only `device` left the other copy stale: a source moved through device
    // A kept drawing at its old place on device B.
    let device = body["device"].as_str().unwrap_or(PREVIEW).to_string();
    let after = lid
        .as_deref()
        .and_then(|l| ctx.state.layers.iter().find(|x| x.id == l))
        .map(|l| l.outputs.clone())
        .unwrap_or_default();
    let created_on = created.as_ref().map(|l| l.outputs.clone()).unwrap_or_default();
    touched.extend(after);
    touched.extend(created_on);
    touched.retain(|d| *d != device);
    touched.insert(0, device);
    sync_graphs(ctx, touched)?;
    let time = ctx.state.time();
    let air = ctx.state.airing(ctx.sim, time);
    let models = ctx.paint.model_count();
    let mut resp = json!({ "devices": ctx.state.devices(&air, models) });
    if let Some(layer) = created {
        resp["layer"] = serde_json::to_value(&layer).unwrap_or(Value::Null);
    }
    Ok(resp)
}

/// Rewrites each named device's graph from the pool — the one writer of
/// "the graph is the list" (LAY.8c) — in order, each device once. A device
/// with no stack is skipped, the same no-op a device with no graph is.
pub(crate) fn sync_graphs(ctx: &mut Ctx, mut devices: Vec<String>) -> Result<(), CallError> {
    let mut seen = std::collections::HashSet::new();
    devices.retain(|d| seen.insert(d.clone()));
    for device in devices {
        let Some(stack) = ctx.state.stack(&device) else {
            continue;
        };
        let mode = stack.mode.clone();
        let layers = ctx.state.layers_on(&device);
        let ids = ctx.paint.ids();
        if let Err(e) = crate::sync_device_graph_at(
            &crate::output::data_root(),
            &device,
            Some(&mode),
            &layers,
            &ids,
        ) {
            // Same posture as the mode op: the device already holds the edit,
            // and a graph left behind would render the old list — the silent,
            // in-range, wrong failure class §3.3 names.
            return Err(CallError::Bad(format!(
                "layers saved to {device} but not to its graph: {e}"
            )));
        }
    }
    Ok(())
}

pub fn collections(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let state = &mut *ctx.state;
    match body["op"].as_str().unwrap_or_default() {
        "save" => {
            let device = body["device"].as_str().unwrap_or(PREVIEW).to_string();
            let name = body["name"].as_str().unwrap_or("collection").to_string();
            let Some(stack) = state.stack(&device) else {
                return Err(CallError::Bad("no such device".into()));
            };
            let composition = stack.composition;
            let routed = state.routed_to(&device);
            if routed.is_empty() {
                return Err(CallError::Bad("this device has no layers to save".into()));
            }
            let layers: Vec<Layer> = routed
                .into_iter()
                .cloned()
                .map(|mut l| {
                    l.outputs = Vec::new();
                    l
                })
                .collect();
            let id = body["id"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| crate::slug(&name, state.collections.list.len()));
            let entry = Collection {
                id: id.clone(),
                name,
                layers,
                composition,
            };
            match state.collections.list.iter_mut().find(|c| c.id == id) {
                Some(slot) => *slot = entry,
                None => state.collections.list.push(entry),
            }
        }
        "remove" => {
            let id = body["id"].as_str().unwrap_or_default().to_string();
            state.collections.list.retain(|c| c.id != id);
            state.orphan(&id);
        }
        "rename" => {
            let id = body["id"].as_str().unwrap_or_default().to_string();
            let Some(c) = state.collections.list.iter_mut().find(|c| c.id == id) else {
                return Err(CallError::Bad("no such collection".into()));
            };
            if let Some(name) = body["name"].as_str() {
                c.name = name.to_string();
            }
        }
        other => return Err(CallError::Bad(format!("no such op: {other}"))),
    }
    if let Err(e) = state.collections.save() {
        eprintln!("  collections not saved: {e}");
    }
    if let Err(e) = state.outputs.save(&state.sources, &state.layers) {
        eprintln!("  outputs not saved: {e}");
    }
    let time = state.time();
    let air = state.airing(ctx.sim, time);
    let models = ctx.paint.model_count();
    Ok(json!({
        "collections": ctx.state.collections.describe(models),
        "devices": ctx.state.devices(&air, models),
    }))
}

/// S.A.1: a `rows`×`cols` grid of rectangular masks over one or more outputs
/// — each cell born as the rectangle it names (see [`grid_cell`]).
pub fn mask_grid(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let state = &mut *ctx.state;

    let device_ids: Vec<String> = if let Some(singular) = body["device_id"].as_str() {
        vec![singular.to_string()]
    } else if let Some(ids) = body["device_ids"].as_array() {
        ids.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    } else {
        return Err(CallError::Bad("expected device_id or device_ids".into()));
    };

    if device_ids.is_empty() {
        return Err(CallError::Bad("no devices specified".into()));
    }

    let rows = body["rows"].as_u64().unwrap_or(0) as u32;
    let cols = body["cols"].as_u64().unwrap_or(0) as u32;

    if rows == 0 || cols == 0 {
        return Err(CallError::Bad("rows and cols must be > 0".into()));
    }
    if rows > 100 || cols > 100 {
        return Err(CallError::Bad("rows and cols must be <= 100".into()));
    }

    for device_id in &device_ids {
        if state.outputs.find(device_id).is_none() {
            return Err(CallError::Bad(format!("no such device: {device_id}")));
        }
    }

    let mut mask_ids = Vec::new();
    // P3: a second call replaces the grid instead of stacking another one.
    for device_id in &device_ids {
        if let Some(out) = state.outputs.find_mut(device_id) {
            out.masks.retain(|m| !m.name.starts_with("grid_row_"));
        }
    }
    // P3: one grid spanning the named devices — the `cols` columns are split
    // between them in order, so a laptop and the monitor beside it show one
    // `rows`×`cols` grid between them rather than each showing a whole copy.
    // A single device gets every column, so its grid is exactly what it
    // always was. More devices than columns leaves the later ones empty
    // rather than doubling a column.
    let n = device_ids.len().max(1);
    for (i, device_id) in device_ids.iter().enumerate() {
        let c0 = (i * cols as usize / n) as u32;
        let c1 = ((i + 1) * cols as usize / n) as u32;
        if let Some(out) = state.outputs.find_mut(device_id) {
            for row in 0..rows {
                for col in c0..c1 {
                    let mask_name = format!("grid_row_{}_col_{}", row, col);
                    // Each cell is the rectangle it names — see `grid_cell`
                    // on why an empty cell would be no cell at all.
                    let mask = Mask {
                        id: String::new(),
                        name: mask_name,
                        points: grid_cell_window(rows, cols, row, col, c0, c1),
                        parent: None,
                        alpha: 1.0,
                        compose: None,
                        stroke: None,
                    };
                    out.masks.push(mask);
                }
            }
        }
    }

    if let Err(e) = state.outputs.save(&state.sources, &state.layers) {
        eprintln!("  outputs not saved: {e}");
        return Err(CallError::Internal("failed to save outputs".into()));
    }

    for device_id in &device_ids {
        if let Some(out) = state.outputs.find(device_id) {
            for mask in &out.masks {
                if mask.name.starts_with("grid_row_") {
                    mask_ids.push(mask.id.clone());
                }
            }
        }
    }

    let time = state.time();
    let air = state.airing(ctx.sim, time);
    let models = ctx.paint.model_count();
    Ok(json!({
        "mask_ids": mask_ids,
        "devices": ctx.state.devices(&air, models),
    }))
}

// ---------------------------------------------------------------------------
// the one writer of a `Mask` — O.1's authoring half
// ---------------------------------------------------------------------------

/// The keys `op: "mask"` reads off its body, besides the address
/// (`mask`/`index`). A body naming none of them is the delete it always was;
/// a body naming any of them is an upsert that writes **only the keys
/// present** — an existing mask keeps every field the body leaves out.
const MASK_FIELDS: [&str; 8] = [
    "points",
    "name",
    "parent",
    "alpha",
    "compose",
    // 2026-09-22 (afternoon): the stroke, written additively key by key so a
    // `stroke_width` edit never disturbs a `stroke_color` already set (and
    // vice versa) — the same "only the keys present" rule as the five above.
    "stroke_width",
    "stroke_color",
    "stroke_source",
];

/// `POST /outputs { op: "mask" }` on one device's `masks`, factored out of
/// [`outputs`] so it can be tested without a `Ctx`.
///
/// # What this is for
///
/// O.1 gave [`Mask`] its `parent`/`alpha`/`compose` and wired them into the
/// render path, and until this function nothing but a hand-edited
/// `data/outputs.json` could set any of the three — the route wrote `name`
/// and `points` and nothing else, so H.10's Venn diagram and H.5b's nested
/// contour existed in the compositor and nowhere an operator (or the
/// harness, regla 6) could reach. This is that path: the same op, the same
/// address, three more keys.
///
/// # Semantics, key by key
///
/// - `at == None` and any field present: a new mask is pushed (its id is
///   assigned by `Outputs::name_masks` on save, as always); `name` defaults
///   to `"mask"` for a new mask only.
/// - `at == None` and no field present: nothing happens (there is nothing
///   to delete) — same as before.
/// - `at == Some` and no field present: the mask is removed and its id
///   returned so the caller can `State::unbind` every source cut to it —
///   `check.mjs`'s `{op: "mask", id, index: 0}` and the page's own remove.
/// - `points`: clamped to 0..1 as always. `name`: replaced when given,
///   kept otherwise (the old arm reset it to `"mask"` on every points
///   write that forgot to resend it — every caller resent it).
/// - `parent`: a mask id on the same device, or `null` to unnest. Refused
///   when it names this mask, a mask that does not exist here, or a mask
///   whose own `parent` chain leads back to this one — `Mask::contains_at`
///   would cut such a cycle at `MAX_MASK_DEPTH` rather than hang, but an
///   editor should refuse to write bad data, not rely on the reader
///   surviving it.
/// - `alpha`: a number, clamped to 0..1.
/// - `compose`: `{ op, a, b }` with `op` one of `union`/`intersect`/
///   `subtract` and `a`/`b` mask ids on the same device, or `null` to go
///   back to this mask's own `points`. Refused for the same self-reference
///   and cycle cases as `parent`, walked through both `parent` and
///   `compose` because the reader walks both.
///
/// Returns the id of a removed mask, or `None` for an upsert.
pub(crate) fn edit_mask(
    masks: &mut Vec<Mask>,
    at: Option<usize>,
    body: &Value,
) -> Result<Option<String>, String> {
    let has_fields = MASK_FIELDS.iter().any(|k| body.get(*k).is_some());
    if !has_fields {
        return Ok(at.map(|at| masks.remove(at).id));
    }

    // The id this edit is addressed to — a new mask has none yet, and no
    // reference can name it, so every self-reference check is vacuous.
    let own_id: Option<String> = at.map(|at| masks[at].id.clone());
    let exists = |id: &str| masks.iter().any(|m| m.id == id);
    let check_ref = |field: &str, id: &str| -> Result<(), String> {
        if id.trim().is_empty() {
            return Err(format!("`{field}` names an empty mask id"));
        }
        if own_id.as_deref() == Some(id) {
            return Err(format!("`{field}` cannot name the mask itself ({id})"));
        }
        if !exists(id) {
            return Err(format!("`{field}` names no mask on this device: {id}"));
        }
        if let Some(own) = own_id.as_deref() {
            if reaches(masks, id, own) {
                return Err(format!("`{field}` = {id} would make a cycle back to {own}"));
            }
        }
        Ok(())
    };

    let parent = match body.get("parent") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(pid)) => {
            check_ref("parent", pid)?;
            Some(Some(pid.clone()))
        }
        Some(_) => return Err("`parent` must be a mask id or null".into()),
    };
    let compose = match body.get("compose") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::Object(c)) => {
            let op = match c.get("op").and_then(Value::as_str) {
                Some("union") => BoolOp::Union,
                Some("intersect") => BoolOp::Intersect,
                Some("subtract") => BoolOp::Subtract,
                Some(other) => {
                    return Err(format!(
                        "no such boolean op: {other:?} — union, intersect or subtract"
                    ))
                }
                None => return Err("`compose` needs an `op`".into()),
            };
            let a = c
                .get("a")
                .and_then(Value::as_str)
                .ok_or("`compose` needs `a`, a mask id")?;
            let b = c
                .get("b")
                .and_then(Value::as_str)
                .ok_or("`compose` needs `b`, a mask id")?;
            check_ref("compose.a", a)?;
            check_ref("compose.b", b)?;
            Some(Some(Compose {
                op,
                a: a.to_string(),
                b: b.to_string(),
            }))
        }
        Some(_) => return Err("`compose` must be { op, a, b } or null".into()),
    };
    let alpha = match body.get("alpha") {
        None => None,
        Some(v) => Some(
            v.as_f64()
                .ok_or("`alpha` must be a number between 0 and 1")?
                .clamp(0.0, 1.0) as f32,
        ),
    };
    let points: Option<Vec<[f32; 2]>> = body["points"].as_array().map(|raw| {
        raw.iter()
            .filter_map(|p| {
                let a = p.as_array()?;
                Some([
                    (a.first()?.as_f64()? as f32).clamp(0.0, 1.0),
                    (a.get(1)?.as_f64()? as f32).clamp(0.0, 1.0),
                ])
            })
            .collect()
    });
    let name = body["name"].as_str().map(str::to_string);

    // 2026-09-22 (afternoon): the stroke keys. `stroke_width` and
    // `stroke_color` are independent writes into the same `MaskStroke`; a
    // mask that had no `stroke` gets one on the first of either. `null`
    // clears the sub-field (color/source) rather than the whole stroke —
    // the same additive rule as the rest of this function.
    let stroke_width: Option<Option<f32>> = match body.get("stroke_width") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => Some(Some(
            v.as_f64()
                .ok_or("`stroke_width` must be a number of pixels")?
                .clamp(0.0, f64::from(metal_perform::output::MASK_STROKE_MAX)) as f32,
        )),
    };
    let stroke_color: Option<Option<[f32; 4]>> = match body.get("stroke_color") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::Array(c)) => Some(Some([
            c.first().and_then(Value::as_f64).unwrap_or(1.0).clamp(0.0, 1.0) as f32,
            c.get(1).and_then(Value::as_f64).unwrap_or(1.0).clamp(0.0, 1.0) as f32,
            c.get(2).and_then(Value::as_f64).unwrap_or(1.0).clamp(0.0, 1.0) as f32,
            c.get(3).and_then(Value::as_f64).unwrap_or(1.0).clamp(0.0, 1.0) as f32,
        ])),
        Some(_) => return Err("`stroke_color` must be [r, g, b, a] or null".into()),
    };
    let stroke_source: Option<Option<String>> = match body.get("stroke_source") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(Some(s.clone())),
        Some(_) => return Err("`stroke_source` must be a layer name or null".into()),
    };

    let mask = match at {
        Some(at) => &mut masks[at],
        None => {
            masks.push(Mask {
                id: String::new(),
                name: "mask".to_string(),
                ..Default::default()
            });
            masks.last_mut().expect("just pushed")
        }
    };
    if let Some(name) = name {
        mask.name = name;
    }
    if let Some(points) = points {
        mask.points = points;
    }
    if let Some(parent) = parent {
        mask.parent = parent;
    }
    if let Some(alpha) = alpha {
        mask.alpha = alpha;
    }
    if let Some(compose) = compose {
        mask.compose = compose;
    }
    // The three stroke keys share one `MaskStroke`: the first of any to
    // arrive materialises it (a mask with no stroke starts at width 0 / no
    // colour, which paints nothing until a width above 0 lands). Clearing
    // every sub-field is not the same as removing the stroke, but the
    // renderer treats `width: 0` as no stroke, so both read as absent.
    if stroke_width.is_some() || stroke_color.is_some() || stroke_source.is_some() {
        let s = mask.stroke.get_or_insert_with(MaskStroke::default);
        if let Some(w) = stroke_width {
            s.width = w.unwrap_or(0.0);
        }
        if let Some(c) = stroke_color {
            s.color = c;
        }
        if let Some(src) = stroke_source {
            s.source = src;
        }
    }
    Ok(None)
}

/// Whether following `parent` and `compose` references from the mask `from`
/// ever arrives at `target` — the cycle check [`edit_mask`] runs before
/// writing a reference. Bounded by the same [`crate::output::MAX_MASK_DEPTH`]
/// the readers cut at, so a file that already carries a cycle (hand-edited)
/// terminates here too instead of recursing forever.
fn reaches(masks: &[Mask], from: &str, target: &str) -> bool {
    fn walk(masks: &[Mask], at: &str, target: &str, depth: u8) -> bool {
        if at == target {
            return true;
        }
        if depth > crate::output::MAX_MASK_DEPTH {
            return false;
        }
        let Some(m) = masks.iter().find(|m| m.id == at) else {
            return false;
        };
        let via_parent = m
            .parent
            .as_deref()
            .is_some_and(|p| walk(masks, p, target, depth + 1));
        let via_compose = m.compose.as_ref().is_some_and(|c| {
            walk(masks, &c.a, target, depth + 1) || walk(masks, &c.b, target, depth + 1)
        });
        via_parent || via_compose
    }
    walk(masks, from, target, 0)
}

#[cfg(test)]
/// The corners of cell `(row, col)` of a `rows`×`cols` grid over the whole
/// device, clockwise from the top left, in the same 0..1 space
/// `Mask::points` lives in — what `POST /mask/grid` writes into each cell.
///
/// A grid cell with no points is not a cell: `Mask::closed` needs three, so
/// an empty one is never lit, never projected by `/mask/project`, and never
/// drawn by `test_mask_grid` — which is the whole loop H.5 builds the grid
/// for (*"propose the grid → project with colours → look"*). The grid
/// exists so the operator or the harness can then move a cell's corners
/// onto the real object; it has to start as the rectangle it names.
pub(crate) fn grid_cell(rows: u32, cols: u32, row: u32, col: u32) -> Vec<[f32; 2]> {
    let (rows, cols) = (rows.max(1) as f32, cols.max(1) as f32);
    let (x0, x1) = (col as f32 / cols, (col + 1) as f32 / cols);
    let (y0, y1) = (row as f32 / rows, (row + 1) as f32 / rows);
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// P3: cell `(row, col)` of a grid whose columns `c0..c1` are the share one
/// device of several shows. The cell's x-range is remapped from the whole
/// grid into that device's own 0..1, so a device showing columns 2–3 of 4
/// draws them edge to edge rather than bunched in the middle. A single device
/// (`c0 == 0`, `c1 == cols`) is exactly [`grid_cell`].
pub(crate) fn grid_cell_window(
    rows: u32,
    _cols: u32,
    row: u32,
    col: u32,
    c0: u32,
    c1: u32,
) -> Vec<[f32; 2]> {
    let rows = rows.max(1) as f32;
    let span = (c1.saturating_sub(c0)).max(1) as f32;
    let (x0, x1) = ((col - c0) as f32 / span, (col + 1 - c0) as f32 / span);
    let (y0, y1) = (row as f32 / rows, (row + 1) as f32 / rows);
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polygon(id: &str) -> Mask {
        Mask {
            id: id.to_string(),
            name: id.to_string(),
            points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            ..Default::default()
        }
    }

    fn three() -> Vec<Mask> {
        vec![polygon("a"), polygon("b"), polygon("c")]
    }

    /// The identity the route always had: a body with only an address
    /// removes, a body with `name`+`points` upserts exactly those two, and
    /// a mask nobody nests or composes stays a plain polygon.
    #[test]
    fn name_and_points_write_what_they_always_wrote_and_no_fields_removes() {
        let mut masks = three();
        edit_mask(
            &mut masks,
            Some(1),
            &json!({ "name": "puerta", "points": [[0.1, 0.1], [0.9, 0.1], [0.5, 0.9]] }),
        )
        .unwrap();
        assert_eq!(masks[1].name, "puerta");
        assert_eq!(masks[1].points, vec![[0.1, 0.1], [0.9, 0.1], [0.5, 0.9]]);
        assert_eq!(
            (
                masks[1].parent.clone(),
                masks[1].alpha,
                masks[1].compose.clone()
            ),
            (None, 1.0, None)
        );

        edit_mask(
            &mut masks,
            None,
            &json!({ "points": [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0]] }),
        )
        .unwrap();
        assert_eq!(masks.len(), 4);
        assert_eq!(
            masks[3].name, "mask",
            "a new mask with no name gets the old default"
        );

        assert_eq!(
            edit_mask(&mut masks, Some(0), &json!({})).unwrap(),
            Some("a".to_string())
        );
        assert_eq!(masks.len(), 3);
        assert_eq!(
            edit_mask(&mut masks, None, &json!({})).unwrap(),
            None,
            "no address, nothing to remove"
        );
        assert_eq!(masks.len(), 3);
    }

    /// Writing a field alone leaves the others alone — a `parent` edit does
    /// not blank the points, an `alpha` edit does not unnest.
    #[test]
    fn each_field_writes_alone_and_keeps_the_rest() {
        let mut masks = three();
        edit_mask(&mut masks, Some(2), &json!({ "parent": "a" })).unwrap();
        assert_eq!(masks[2].parent.as_deref(), Some("a"));
        assert_eq!(
            masks[2].points.len(),
            3,
            "points survive a parent-only edit"
        );
        assert_eq!(masks[2].name, "c", "name survives a parent-only edit");

        edit_mask(&mut masks, Some(2), &json!({ "alpha": 0.25 })).unwrap();
        assert_eq!(masks[2].alpha, 0.25);
        assert_eq!(
            masks[2].parent.as_deref(),
            Some("a"),
            "parent survives an alpha-only edit"
        );

        edit_mask(&mut masks, Some(2), &json!({ "alpha": 7.0 })).unwrap();
        assert_eq!(masks[2].alpha, 1.0, "alpha clamps like points do");

        edit_mask(&mut masks, Some(2), &json!({ "parent": Value::Null })).unwrap();
        assert_eq!(masks[2].parent, None);
        assert_eq!(masks[2].alpha, 1.0);
    }

    /// H.10's Venn diagram: `c = a ∩ b`, written through the route, is the
    /// same `Compose` the compositor tests build by hand — and `null` puts
    /// the mask back on its own points.
    #[test]
    fn compose_writes_a_boolean_and_null_returns_to_points() {
        let mut masks = three();
        edit_mask(
            &mut masks,
            Some(2),
            &json!({ "compose": { "op": "intersect", "a": "a", "b": "b" } }),
        )
        .unwrap();
        assert_eq!(
            masks[2].compose,
            Some(Compose {
                op: BoolOp::Intersect,
                a: "a".into(),
                b: "b".into()
            })
        );
        assert_eq!(
            crate::output::lit_masks(&masks).len(),
            1,
            "operands are building blocks, only c is lit"
        );

        edit_mask(&mut masks, Some(2), &json!({ "compose": Value::Null })).unwrap();
        assert_eq!(masks[2].compose, None);
        assert_eq!(crate::output::lit_masks(&masks).len(), 3);
    }

    /// A new mask may be born composed — no points of its own, its shape
    /// entirely `a <op> b`.
    #[test]
    fn a_new_mask_can_be_composed_from_birth() {
        let mut masks = three();
        edit_mask(
            &mut masks,
            None,
            &json!({ "name": "venn", "compose": { "op": "union", "a": "a", "b": "b" } }),
        )
        .unwrap();
        assert_eq!(masks.len(), 4);
        assert_eq!(masks[3].name, "venn");
        assert!(masks[3].points.is_empty());
        assert!(
            masks[3].closed(),
            "a composed mask is closed without points"
        );
    }

    /// The refusals: a reference to itself, to a mask that is not on the
    /// device, an unknown op, and a cycle through either `parent` or
    /// `compose` — none of which should ever reach `data/outputs.json`.
    #[test]
    fn bad_references_are_refused_before_anything_is_written() {
        let mut masks = three();
        let before = masks.clone();
        let refused = [
            json!({ "parent": "c" }),
            json!({ "parent": "zzz" }),
            json!({ "parent": 3 }),
            json!({ "compose": { "op": "xor", "a": "a", "b": "b" } }),
            json!({ "compose": { "op": "union", "a": "c", "b": "b" } }),
            json!({ "compose": { "op": "union", "a": "a" } }),
            json!({ "compose": "a and b" }),
            json!({ "alpha": "opaque" }),
        ];
        for body in refused {
            assert!(
                edit_mask(&mut masks, Some(2), &body).is_err(),
                "{body} should be refused"
            );
        }
        assert_eq!(masks, before, "a refused edit writes nothing");

        // a → parent b, then b → parent a would close the loop.
        edit_mask(&mut masks, Some(0), &json!({ "parent": "b" })).unwrap();
        let err = edit_mask(&mut masks, Some(1), &json!({ "parent": "a" })).unwrap_err();
        assert!(err.contains("cycle"), "{err}");
        // and through compose: c = a ∪ b, then a = c ∪ b would too.
        edit_mask(
            &mut masks,
            Some(2),
            &json!({ "compose": { "op": "union", "a": "a", "b": "b" } }),
        )
        .unwrap();
        let err = edit_mask(
            &mut masks,
            Some(0),
            &json!({ "compose": { "op": "union", "a": "c", "b": "b" } }),
        )
        .unwrap_err();
        assert!(err.contains("cycle"), "{err}");
    }

    /// The four corners of every cell tile the device exactly, every cell
    /// is closed, and the top-left cell starts at the origin — so a fresh
    /// grid is lit, projectable and editable from its first frame.
    #[test]
    fn grid_cells_are_closed_rectangles_that_tile_the_device() {
        assert_eq!(
            grid_cell(2, 2, 0, 0),
            vec![[0.0, 0.0], [0.5, 0.0], [0.5, 0.5], [0.0, 0.5]]
        );
        assert_eq!(
            grid_cell(2, 2, 1, 1),
            vec![[0.5, 0.5], [1.0, 0.5], [1.0, 1.0], [0.5, 1.0]]
        );
        let (rows, cols) = (3, 4);
        let mut area = 0.0f32;
        for r in 0..rows {
            for c in 0..cols {
                let cell = Mask {
                    id: format!("{r}-{c}"),
                    points: grid_cell(rows, cols, r, c),
                    ..Default::default()
                };
                assert!(cell.closed());
                let p = &cell.points;
                area += (p[1][0] - p[0][0]) * (p[2][1] - p[1][1]);
            }
        }
        assert!(
            (area - 1.0).abs() < 1e-5,
            "the cells tile the whole device, got {area}"
        );
    }

    /// S.R's `op: "node"`/`op: "composed_on"` write exactly this expression
    /// into the device — checked directly, against a bare `Output`, since
    /// reaching `outputs()` itself needs a `Ctx` this module does not build.
    /// A string sets the field; `null` and an absent key both clear it.
    #[test]
    fn node_and_composed_on_write_a_string_and_clear_on_null_or_absent() {
        let mut out = Output::new("A", "a", 1920, 1080);

        out.node = json!({ "node": "pi" })["node"].as_str().map(str::to_string);
        assert_eq!(out.node.as_deref(), Some("pi"));
        out.node = json!({ "node": Value::Null })["node"]
            .as_str()
            .map(str::to_string);
        assert_eq!(out.node, None, "an explicit null clears it");
        out.node = Some("pi".to_string());
        out.node = json!({})["node"].as_str().map(str::to_string);
        assert_eq!(out.node, None, "an absent key clears it just like null");

        out.composed_on = json!({ "composed_on": "laptop" })["composed_on"]
            .as_str()
            .map(str::to_string);
        assert_eq!(out.composed_on.as_deref(), Some("laptop"));
        out.composed_on = json!({})["composed_on"].as_str().map(str::to_string);
        assert_eq!(
            out.composed_on, None,
            "an absent key clears it just like null"
        );
    }

    /// S.R's `op: "cede"` writes exactly this expression into the device —
    /// checked directly, against a bare `Output`, since reaching `outputs()`
    /// itself needs a `Ctx` this module does not build. A string sets the
    /// field; `null` and an absent key both clear it.
    #[test]
    fn cede_writes_a_string_and_clears_on_null_or_absent() {
        let mut out = Output::new("A", "a", 1920, 1080);

        out.ceded_to = json!({ "to": "resolume" })["to"]
            .as_str()
            .map(str::to_string);
        assert_eq!(out.ceded_to.as_deref(), Some("resolume"));
        out.ceded_to = json!({ "to": Value::Null })["to"]
            .as_str()
            .map(str::to_string);
        assert_eq!(out.ceded_to, None, "an explicit null clears it");
        out.ceded_to = Some("resolume".to_string());
        out.ceded_to = json!({})["to"].as_str().map(str::to_string);
        assert_eq!(out.ceded_to, None, "an absent key clears it just like null");
    }

    // -- 2026-09-22 (afternoon): the mask stroke -----------------------------

    /// The stroke keys write additively: the first one to arrive materialises
    /// the stroke, a second one lands beside it, and an edit that names none
    /// of them leaves whatever stroke the mask already had untouched — the
    /// editor's own posts must never drop what the page wrote first.
    #[test]
    fn the_stroke_keys_write_additively_and_never_disturb_each_other() {
        let mut masks = three();

        edit_mask(&mut masks, Some(0), &json!({ "stroke_width": 8.0 })).unwrap();
        let stroke = masks[0].stroke.as_ref().expect("materialised by the first key");
        assert_eq!(stroke.width, 8.0);
        assert_eq!(stroke.color, None, "a width-only post invents no colour");
        assert!(stroke.source.is_none());

        edit_mask(&mut masks, Some(0), &json!({ "stroke_color": [1.0, 0.0, 0.0, 1.0] })).unwrap();
        let stroke = masks[0].stroke.as_ref().unwrap();
        assert_eq!(stroke.width, 8.0, "a colour post keeps the width beside it");
        assert_eq!(stroke.color, Some([1.0, 0.0, 0.0, 1.0]));

        edit_mask(&mut masks, Some(0), &json!({ "alpha": 0.5 })).unwrap();
        let stroke = masks[0].stroke.as_ref().unwrap();
        assert_eq!(
            stroke.width,
            8.0,
            "an alpha post with no stroke key must not disturb the stroke"
        );
        assert_eq!(stroke.color, Some([1.0, 0.0, 0.0, 1.0]));

        // null clears the sub-field, not the whole stroke.
        edit_mask(&mut masks, Some(0), &json!({ "stroke_color": Value::Null })).unwrap();
        let stroke = masks[0].stroke.as_ref().unwrap();
        assert_eq!(stroke.color, None);
        assert_eq!(stroke.width, 8.0, "a colour clear keeps the width");

        // Clamped like everything else.
        edit_mask(&mut masks, Some(1), &json!({ "stroke_width": 9999.0 })).unwrap();
        assert_eq!(masks[1].stroke.as_ref().unwrap().width, metal_perform::output::MASK_STROKE_MAX);
        edit_mask(&mut masks, Some(1), &json!({ "stroke_width": -3.0 })).unwrap();
        assert_eq!(masks[1].stroke.as_ref().unwrap().width, 0.0);

        // Bad shapes refused outright.
        assert!(edit_mask(&mut masks, Some(0), &json!({ "stroke_color": 7 })).is_err());
        assert!(edit_mask(&mut masks, Some(0), &json!({ "stroke_source": 7 })).is_err());
        assert!(edit_mask(&mut masks, Some(0), &json!({ "stroke_width": "wide" })).is_err());
    }

    /// The stroke keys count as *edit* keys, not as "nothing named": a body
    /// with only a stroke key must not be read as the old remove-sentinel.
    #[test]
    fn a_body_naming_only_a_stroke_key_is_an_upsert_not_a_remove() {
        let mut masks = three();
        assert_eq!(edit_mask(&mut masks, Some(0), &json!({ "stroke_width": 4.0 })).unwrap(), None);
        assert_eq!(masks.len(), 3, "nothing removed");
        assert!(masks[0].stroke.is_some());
    }
}
