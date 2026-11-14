//! `sensor.yolo`'s reads and its one write: the live eyes reading, a picture
//! of the capture with the head drawn on it, and the `set_head_tracking`
//! gesture.
//!
//! The properties panel's "lost track" indicator and its preview pump the two
//! reads; the `set_head_tracking` agent tool posts the write. Every one is a
//! reading of state the engine already holds — the reading is what
//! `poll_head_sensors` stored this frame (`State::head`), the picture is the
//! same capture frame the engine fed the detector — never a second copy
//! (rule 4).

use serde_json::{json, Value};

use crate::call::{Bytes, CallError, Ctx, Params};

/// The `sensor.yolo` node `params` names, its graph, and the `capture` node
/// feeding it (if any).
fn wired(
    gid: &str,
    node_id: &str,
) -> Result<(metal_graph::Graph, String, Option<metal_graph::NodeId>), CallError> {
    let root = crate::output::data_root();
    let graph = crate::load_graph_at(&root, gid)
        .map_err(|e| CallError::Bad(format!("no graph {gid:?}: {e}")))?;
    if graph.node(node_id).map(|n| n.kind.as_str()) != Some("sensor.yolo") {
        return Err(CallError::Bad(format!("{node_id:?} is not a sensor.yolo node")));
    }
    let capture = graph
        .edges
        .iter()
        .find(|e| e.to.node == node_id && e.to.port == "image")
        .map(|e| e.from.node.clone());
    Ok((graph, node_id.to_string(), capture))
}

/// The capture node's `source` string, or `None` for a node that is not a
/// `capture` or has no source written yet.
fn capture_source(graph: &metal_graph::Graph, capture: &metal_graph::NodeId) -> Option<String> {
    let node = graph.node(capture)?;
    if node.kind != "capture" {
        return None;
    }
    let v: Value = serde_json::from_str(&node.config.to_json_str()).ok()?;
    v["source"].as_str().map(str::to_string)
}

/// `GET /sensor?graph=&node=`: the reading the engine holds for this
/// `sensor.yolo` node, its upstream capture source, and whether the reading
/// is stale (the "lost track" indicator's one fact).
pub fn reading(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let (Some(gid), Some(node_id)) = (params.param("graph"), params.param("node")) else {
        return Err(CallError::Bad("which graph and which node".into()));
    };
    let (graph, node_id, capture) = wired(gid, node_id)?;
    let source = capture.as_ref().and_then(|c| capture_source(&graph, c));
    let reading = ctx.state.head.get(&node_id);
    Ok(json!({
        "node": node_id,
        "source": source,
        "reading": reading.is_some(),
        "stale": reading.map(|c| c.stale).unwrap_or(true),
        "eye_x": reading.and_then(|c| c.get("eye_x")),
        "eye_y": reading.and_then(|c| c.get("eye_y")),
        "eye_z_est": reading.and_then(|c| c.get("eye_z_est")),
    }))
}

/// `GET /sensor.jpg?graph=&node=&w=&h=`: the upstream capture's newest frame,
/// black until the first one lands. The head dot and face box are drawn by
/// the properties panel over the picture, not baked in here.
pub fn preview_jpg(_ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let (Some(gid), Some(node_id)) = (params.param("graph"), params.param("node")) else {
        return Err(CallError::Bad("which graph and which node".into()));
    };
    let w = params.num("w", 320.0).clamp(64.0, 960.0) as u32;
    let h = params.num("h", 180.0).clamp(36.0, 540.0) as u32;
    let (graph, _, capture) = wired(gid, node_id)?;
    let rgba = capture
        .as_ref()
        .and_then(|c| capture_source(&graph, c))
        .and_then(|s| crate::capture::frame(&s, w, h))
        .unwrap_or_else(|| vec![0u8; (w * h * 4) as usize]);
    Ok(Bytes {
        kind: "image/jpeg",
        body: crate::paint::encode_jpeg(&rgba, w, h),
    })
}

/// G6: the same picture as a texture on the painter's device, for a window
/// that shares it. The capture's native frame is uploaded once per new frame
/// (`Paint::cached_texture`, keyed by its sequence number) and the window
/// scales it when it paints; no JPEG, no scaling on the CPU. `None` on a CPU
/// painter, where the JPEG route answers.
pub fn preview_texture(paint: &mut dyn metal_perform::paint::Paint, params: &Params) -> Option<metal_perform::gpu_frame::GpuFrame> {
    let (gid, node_id) = (params.param("graph")?, params.param("node")?);
    let (graph, node_id, capture) = wired(gid, node_id).ok()?;
    let source = capture_source(&graph, &capture?)?;
    let (w, h) = (crate::capture::W, crate::capture::H);
    let (seq, raw) = match crate::capture::latest_raw(&source) {
        Some((seq, raw)) => (seq, Some(raw)),
        None => (0, None),
    };
    paint.cached_texture(&format!("sensor:{node_id}"), seq, &mut || {
        let px = match &raw {
            Some(raw) => raw.as_ref().clone(),
            None => vec![0u8; (w * h * 4) as usize],
        };
        Some((px, w, h))
    })
}

/// `POST /sensor { op: "set", on?, strength?, graph? }`: the
/// `set_head_tracking` gesture. Turns the `cam.track` nodes on or off (their
/// `bypass`, the same flag the node's own switch writes) and sets their
/// strength, in one graph or — with no `graph` — every graph on disk. A
/// graph with no `cam.track` node is left exactly as it was.
pub fn set(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let on = body["on"].as_bool();
    let strength = body["strength"].as_f64().map(|v| v as f32);
    if on.is_none() && strength.is_none() {
        return Err(CallError::Bad("set_head_tracking needs `on` or `strength`".into()));
    }
    if let Some(s) = strength {
        if !(0.0..=1.0).contains(&s) {
            return Err(CallError::Bad(format!("strength {s} is outside 0..1")));
        }
    }
    let ids: Vec<String> = match body["graph"].as_str() {
        Some(g) => vec![g.to_string()],
        None => {
            let dir = crate::graphs_dir(&crate::output::data_root());
            let mut ids: Vec<String> = metal_vfs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    (p.extension()? == "json").then(|| p.file_stem()?.to_str().map(str::to_string))?
                })
                .collect();
            ids.sort();
            ids
        }
    };
    let mut touched = Vec::new();
    for gid in ids {
        let Ok(mut g) = crate::load_graph(&gid) else { continue };
        let cams: Vec<String> = g
            .nodes
            .iter()
            .filter(|n| n.kind == "cam.track")
            .map(|n| n.id.clone())
            .collect();
        if cams.is_empty() {
            continue;
        }
        for id in &cams {
            let Some(node) = g.node_mut(id) else { continue };
            if let Some(on) = on {
                node.bypass = !on;
            }
            if let Some(strength) = strength {
                let mut cfg: metal_graph::camera_track::CamTrackConfig =
                    serde_json::from_str(&node.config.to_json_str()).unwrap_or_default();
                cfg.strength = strength;
                node.config = cfg.to_config();
            }
        }
        if let Err(e) = crate::save_graph(&g) {
            return Err(CallError::Internal(e));
        }
        touched.push(json!({ "graph": gid, "cam_track": cams }));
    }
    Ok(json!({ "ok": true, "on": on, "strength": strength, "touched": touched }))
}
