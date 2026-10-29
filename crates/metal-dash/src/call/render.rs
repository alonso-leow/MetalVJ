//! The picture routes: the preview, one source's thumbnail, one output's
//! composed frame, the assets rack, and the mask-projection debug frame.
//! Every one of these renders through `render_device`/`render_source`
//! (`main.rs`) and answers [`Bytes`] rather than JSON.

use serde_json::Value;

use crate::call::{Bytes, CallError, Ctx, Params};
use crate::paint::encode_jpeg;

/// The clock `GET /render/<layer>/<frame>` and the remote composition agree
/// on: frame `n` is `n / LAYER_FPS` seconds into the layer's own clock.
pub const LAYER_FPS: f32 = 60.0;

/// `GET /render/<layer>/<frame>`: one layer drawn alone at `frame`, as a JPEG
/// at `w`×`h`. This is the route `metal-sync` answers `fw/frame/1`'s layer asks
/// with.
///
/// Two more forms share the prefix:
/// - `GET /render/<device>/<node>/<frame>`: one graph node drawn alone
///   (the `capture_output` route).
/// - `GET /render/<node>/<frame>`: the same, when the id is not a layer here —
///   the device whose graph holds the node is found and its picture drawn. So
///   `capture_output {node}` reaches any `gen.*`/`fx.*`/mask node with no
///   screen named.
///
/// With `model`, `row` (and `time`) in the query, the picture is drawn from
/// that state and the layer need not exist here: the requester resolved its
/// feed, base floats, audio digest and effects, and the peer only draws. The
/// same query gives the same pixels on any peer. Without them, the layer is
/// looked up here and drawn from this machine's own state, whatever its feed.
pub fn layer_jpg(ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let tail = params.tail.as_deref().unwrap_or_default();
    // `GET /render/<device>/<node>/<frame>`: a graph node's own picture (the
    // `capture_output` route), sharing this prefix with the layer asks.
    if tail.matches('/').count() == 2 {
        return node_render_jpg(ctx, params, tail);
    }
    let Some((layer_id, frame)) = tail.rsplit_once('/') else {
        return Err(CallError::Bad("expected GET /render/<layer>/<frame>".into()));
    };
    let frame: u64 = frame
        .parse()
        .map_err(|_| CallError::Bad("frame must be a whole number".into()))?;
    // A layer spanning several devices is asked at the covering canvas,
    // which can be wider than one output (`span_canvas_size`).
    let w = params.num("w", 1280.0).clamp(32.0, 4096.0) as u32;
    let h = params.num("h", 720.0).clamp(18.0, 2160.0) as u32;
    let time = params
        .param("time")
        .and_then(|t| t.parse::<f32>().ok())
        .unwrap_or(frame as f32 / LAYER_FPS);
    let (model, floats) = match params.param("row") {
        Some(row) => {
            let model: usize = params
                .param("model")
                .and_then(|m| m.parse().ok())
                .ok_or_else(|| CallError::Bad("a row needs its model".into()))?;
            if model >= ctx.paint.model_count() {
                return Err(CallError::Bad("model is not one this machine has".into()));
            }
            let floats = row
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.parse::<f32>().ok())
                .collect::<Option<Vec<f32>>>()
                .ok_or_else(|| CallError::Bad("row must be comma-separated numbers".into()))?;
            (model, floats)
        }
        None => {
            let Some(layer) = ctx.state.layers.iter().find(|l| l.id == layer_id).cloned() else {
                // Not a layer: it may be a graph node of some device, which is
                // what `capture_output {node}` names. Draw that node instead —
                // the same `render_node` path `/node.jpg` and `/frame/export`
                // use, so the two agree.
                let device = device_holding_node(layer_id).ok_or(CallError::NotFound)?;
                return node_bytes(ctx, params, &device, layer_id, frame);
            };
            let air = ctx.state.airing(ctx.sim, time);
            let model = layer.showing(&air, ctx.paint.model_count());
            let floats = layer
                .base
                .clone()
                .unwrap_or_else(|| ctx.state.base[model.min(ctx.state.base.len() - 1)].clone());
            (model, floats)
        }
    };
    let codec = Codec::of(params)?;
    // The chain the requester resolved from the graph (`fx.*` nodes past the
    // generator) travels with the state; a peer draws it exactly as the local
    // path does (`Paint::run`'s `solo_chain`), so the picture matches.
    let chain = match params.param("chain") {
        Some(text) => metal_schema::chain_from_query(text)
            .ok_or_else(|| CallError::Bad("chain is effect:amount:slot triples".into()))?,
        None => Vec::new(),
    };
    // The backdrop mode the source carries (`fondo=1`), so the peer draws the
    // model the way the local generator does.
    let background = params.param("background") == Some("1");
    let (pixels, pw, ph) = if chain.is_empty() {
        ctx.paint.solo(model, &floats, time, w, h, background)
    } else {
        ctx.paint.solo_chain(model, &floats, time, w, h, background, &chain)
    };
    Ok(codec.encode(&pixels, pw, ph))
}

/// `GET /render/<device>/<node>/<frame>`: one graph node drawn alone at
/// `frame`, through the same [`crate::render_node`] path `/node.jpg` and
/// `/frame/export` use. The device names the authored graph the node lives
/// in; the node is any `gen.*`, `fx.*`, text or mask of it.
fn node_render_jpg(ctx: &mut Ctx, params: &Params, tail: &str) -> Result<Bytes, CallError> {
    let mut parts = tail.splitn(3, '/');
    let device = parts.next().unwrap_or_default();
    let node = parts.next().unwrap_or_default();
    let frame = parts.next().unwrap_or_default();
    if device.is_empty() || node.is_empty() {
        return Err(CallError::Bad("expected GET /render/<device>/<node>/<frame>".into()));
    }
    let frame: u64 = frame
        .parse()
        .map_err(|_| CallError::Bad("frame must be a whole number".into()))?;
    node_bytes(ctx, params, device, node, frame)
}

/// A node's picture in the codec the caller negotiated — `w`/`h`/`fmt`/`q`
/// exactly as [`layer_jpg`], and the same `time` fallback from `frame`.
/// `time` is accepted for parity with the layer route; a node draws at the
/// show clock [`crate::render_node`] reads, not a caller-supplied one.
fn node_bytes(ctx: &mut Ctx, params: &Params, device: &str, node: &str, frame: u64) -> Result<Bytes, CallError> {
    let w = params.num("w", 1280.0).clamp(32.0, 4096.0) as u32;
    let h = params.num("h", 720.0).clamp(18.0, 2160.0) as u32;
    let _time = params
        .param("time")
        .and_then(|t| t.parse::<f32>().ok())
        .unwrap_or(frame as f32 / LAYER_FPS);
    let codec = Codec::of(params)?;
    match crate::render_node(ctx.paint, ctx.sim, ctx.state, device, node, w, h) {
        Some((pixels, pw, ph)) => Ok(codec.encode(&pixels, pw, ph)),
        None => Err(CallError::NotFound),
    }
}

/// The device whose authored graph holds `node`, found by reading the graph
/// store `GET /graphs` lists — the one place a bare node id (no device) is
/// turned into the graph it belongs to, so `capture_output {node}` reaches a
/// real node without the operator naming its screen. `None` when no device's
/// graph has a node by that id.
fn device_holding_node(node: &str) -> Option<String> {
    for row in crate::list_graphs() {
        let Some(graph_id) = row.get("id").and_then(Value::as_str) else { continue };
        let Some(device) = graph_id.strip_prefix("device-") else { continue };
        if let Ok(graph) = crate::load_graph(graph_id) {
            if graph.node(node).is_some() {
                return Some(device.to_string());
            }
        }
    }
    None
}

/// The codec a picture travels in: `fmt=jpeg|qoi|rgba` (JPEG at `q=` quality
/// when `jpeg`). Absent, a picture is JPEG at the historical quality, so a
/// caller that does not negotiate sees what it always saw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Codec {
    Jpeg(u8),
    Qoi,
    Rgba,
}

impl Codec {
    pub fn of(params: &Params) -> Result<Codec, CallError> {
        Ok(match params.param("fmt") {
            None | Some("jpeg") => {
                let q = params.num("q", 82.0).clamp(1.0, 100.0) as u8;
                Codec::Jpeg(q)
            }
            Some("qoi") => Codec::Qoi,
            Some("rgba") => Codec::Rgba,
            Some(other) => return Err(CallError::Bad(format!("fmt must be jpeg, qoi or rgba, not {other}"))),
        })
    }

    /// The content type a picture in this codec is served as.
    pub fn kind(self) -> &'static str {
        match self {
            Codec::Jpeg(_) => "image/jpeg",
            Codec::Qoi => "image/qoi",
            Codec::Rgba => "application/octet-stream",
        }
    }

    /// The reply for one picture in this codec.
    pub fn encode(self, rgba: &[u8], w: u32, h: u32) -> Bytes {
        match self {
            Codec::Jpeg(q) => Bytes { kind: "image/jpeg", body: crate::paint::encode_jpeg_q(rgba, w, h, q) },
            Codec::Qoi => Bytes { kind: "image/qoi", body: crate::remote_layers::encode_qoi_frame(rgba, w, h) },
            Codec::Rgba => Bytes { kind: "application/octet-stream", body: rgba.to_vec() },
        }
    }
}


thread_local! {
    /// Set by [`crate::call::dispatch_deferred`] around one handler call: the
    /// image routes then park their pixels in [`PENDING`] instead of encoding,
    /// so the encode runs after the caller has dropped the shared lock.
    static DEFER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PENDING: std::cell::RefCell<Option<(Codec, Vec<u8>, u32, u32)>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn set_deferring(on: bool) {
    DEFER.with(|d| d.set(on));
    if !on {
        PENDING.with(|p| p.borrow_mut().take());
    }
}

/// Takes what a deferred handler parked, if anything.
pub(crate) fn take_pending() -> Option<(Codec, Vec<u8>, u32, u32)> {
    PENDING.with(|p| p.borrow_mut().take())
}

/// One picture as the reply, in the codec `fmt` asks for (JPEG when absent).
/// Under [`crate::call::dispatch_deferred`] the encode is postponed (the body
/// is empty until `finish`), so it never runs inside the shared lock.
fn image(params: &Params, pixels: Vec<u8>, w: u32, h: u32) -> Result<Bytes, CallError> {
    let codec = Codec::of(params)?;
    if DEFER.with(|d| d.get()) {
        PENDING.with(|p| *p.borrow_mut() = Some((codec, pixels, w, h)));
        return Ok(Bytes { kind: codec.kind(), body: Vec::new() });
    }
    Ok(codec.encode(&pixels, w, h))
}

pub fn preview_jpg(ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let w = params.num("w", 640.0).clamp(64.0, 1920.0) as u32;
    let h = params.num("h", 360.0).clamp(36.0, 1080.0) as u32;
    let (pixels, pw, ph) =
        crate::render_device(ctx.paint, ctx.sim, ctx.state, crate::PREVIEW, w, h);
    image(params, pixels, pw, ph)
}

pub fn source_jpg(ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let (Some(device), Some(layer)) = (
        params.param("device").map(str::to_string),
        params.param("layer").map(str::to_string),
    ) else {
        return Err(CallError::Bad("which device and which source".into()));
    };
    let w = params.num("w", 240.0).clamp(32.0, 480.0) as u32;
    let h = params.num("h", 135.0).clamp(18.0, 270.0) as u32;
    let gid = params.param("graph").map(str::to_string);
    match metal_perform::engine::with_preview_graph(gid.as_deref(), || {
        crate::render_source(ctx.paint, ctx.sim, ctx.state, &device, &layer, w, h)
    }) {
        Some((pixels, pw, ph)) => image(params, pixels, pw, ph),
        None => Err(CallError::NotFound),
    }
}

/// `GET /author/render/kind.jpg?kind=<fx.*|mat.*>&variant=0..3&w=&h=`: a kind's
/// own picture at one of its four float variants (`render_kind`), the tile a
/// gallery card shows. A kind with no preview answers 404.
pub fn kind_jpg(ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let Some(kind) = params.param("kind").map(str::to_string) else {
        return Err(CallError::Bad("which kind".into()));
    };
    let variant = (params.num("variant", 0.0).max(0.0) as usize) % metal_perform::engine::VARIANT_T.len();
    let w = params.num("w", 192.0).clamp(32.0, 480.0) as u32;
    let h = params.num("h", 108.0).clamp(18.0, 270.0) as u32;
    match crate::render_kind(ctx.paint, ctx.sim, ctx.state, &kind, variant, w, h) {
        Some((pixels, pw, ph)) => Ok(Bytes {
            kind: "image/jpeg",
            body: encode_jpeg(&pixels, pw, ph),
        }),
        None => Err(CallError::NotFound),
    }
}

/// One node's own picture (`render_node`): a generator with only the
/// effects up to that node, a mask's slice of the output, or the device's
/// composed frame for an output — for isolating one stage while debugging
/// against the final view.
pub fn node_jpg(ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let (Some(device), Some(node)) = (
        params.param("device").map(str::to_string),
        params.param("node").map(str::to_string),
    ) else {
        return Err(CallError::Bad("which device and which node".into()));
    };
    let w = params.num("w", 240.0).clamp(32.0, 1920.0) as u32;
    let h = params.num("h", 135.0).clamp(18.0, 1080.0) as u32;
    let gid = params.param("graph").map(str::to_string);
    match metal_perform::engine::with_preview_graph(gid.as_deref(), || {
        crate::render_node(ctx.paint, ctx.sim, ctx.state, &device, &node, w, h)
    }) {
        Some((pixels, pw, ph)) => image(params, pixels, pw, ph),
        None => Err(CallError::NotFound),
    }
}

pub fn output_jpg(ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    output_jpeg(ctx, params, false)
}

/// `GET /frame`: the same picture for the peers and monitors that poll it.
pub fn frame_jpg(ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    output_jpeg(ctx, params, true)
}

fn output_jpeg(ctx: &mut Ctx, params: &Params, polled: bool) -> Result<Bytes, CallError> {
    let Some(id) = params.param("device").map(str::to_string) else {
        return Err(CallError::Bad("which device".into()));
    };
    let Some(out) = ctx.state.outputs.find(&id).cloned() else {
        return Err(CallError::NotFound);
    };
    if let Some(to) = &out.ceded_to {
        return Err(CallError::Conflict(format!("ceded to {to}")));
    }
    let (w, h) = output_size(&out, params);
    // G3: a peer or monitor polling `/frame` reads through the non-blocking
    // ring (a frame or two old, never older than `MAX_AGE`); a first or rare
    // request has nothing fresh and reads the frame back once. `/output.jpg`
    // is asked for *this* state, so it never takes a late frame.
    let (pixels, pw, ph) = match device_texture(ctx.paint, ctx.sim, ctx.state, &out, w, h) {
        metal_perform::engine::DeviceFrame::Cpu(px, pw, ph) => (px, pw, ph),
        metal_perform::engine::DeviceFrame::Gpu(f) => {
            let late = if polled { ctx.paint.readback_async(&format!("out:{id}:{w}x{h}"), &f) } else { None };
            match late {
                Some(late) => late,
                None => (ctx.paint.readback(&f), f.width(), f.height()),
            }
        }
    };
    image(params, pixels, pw, ph)
}

/// S.A.2: the same frame `/output.jpg` renders, with one mask's outline
/// drawn on top in a chosen colour — for lining a contour up against the
/// camera. Answers JSON on a validation failure and a JPEG on success, same
/// as the arm it came from; `Bytes`/`Params::json` do not distinguish that,
/// so the JSON-shaped errors still travel through [`CallError`].
/// S.A.2: the device's composed frame with mask outlines drawn over it —
/// one mask in `rgb` (white by default) when `mask_id` is given, or every
/// contoured mask in its own [`crate::output::mask_color`] when it is not (the shape
/// `test_mask_grid(outputs)` asks for: the whole grid, each cell told
/// apart by colour, to look at through the camera).
pub fn mask_project(ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };

    let mask_id = body["mask_id"].as_str().map(str::to_string);
    let device_id = match body["device_id"].as_str() {
        Some(id) => id.to_string(),
        None => return Err(CallError::Bad("expected device_id".into())),
    };

    let rgb = if let Some(arr) = body["rgb"].as_array() {
        [
            arr.first().and_then(Value::as_u64).unwrap_or(255) as u8,
            arr.get(1).and_then(Value::as_u64).unwrap_or(255) as u8,
            arr.get(2).and_then(Value::as_u64).unwrap_or(255) as u8,
        ]
    } else {
        [255u8, 255, 255]
    };

    let (w, h, masks) = {
        let Some(out) = ctx.state.outputs.find(&device_id) else {
            return Err(CallError::Bad(format!("no such device: {device_id}")));
        };
        if let Some(mid) = &mask_id {
            if out.mask(mid).is_none() {
                return Err(CallError::Bad(format!("no such mask: {mid}")));
            }
        }
        (out.width, out.height, out.masks.clone())
    };

    let (mut pixels, pw, ph) =
        crate::render_device(ctx.paint, ctx.sim, ctx.state, &device_id, w, h);
    let composition = ctx
        .state
        .sources
        .get(&device_id)
        .map(|d| d.composition)
        .unwrap_or_default();
    crate::output::apply_masks_with(&mut pixels, pw, ph, &masks, &[], composition);
    match &mask_id {
        Some(mid) => {
            if let Some(mask) = masks.iter().find(|m| m.id == *mid) {
                crate::draw_mask_outline(&mut pixels, pw, ph, mask, rgb);
            }
        }
        None => {
            for (i, mask) in masks.iter().enumerate() {
                crate::draw_mask_outline(&mut pixels, pw, ph, mask, crate::output::mask_color(i));
            }
        }
    }
    Ok(Bytes {
        kind: "image/jpeg",
        body: encode_jpeg(&pixels, pw, ph),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// The same fixture shape `call::transmit::tests::fixture` builds: a
    /// painter, a `State` with one output called `"probe"`, and an `Author`
    /// — whose `gpu`-feature construction reads `data/schema.json` off a
    /// bare relative path, hence the cwd dance around it. `output_jpg`
    /// doesn't need the `"probe"` device wired up with triggers, just an
    /// `Output` in `state.outputs`.
    fn fixture() -> (
        Box<dyn crate::paint::Paint>,
        crate::State,
        crate::listeners::Listeners,
        crate::Author,
    ) {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = crate::Author::new();
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let paint = crate::paint::painter();
        let mut state = crate::State::new(paint.as_ref());
        state
            .outputs
            .list
            .push(crate::output::Output::new("probe", "Probe", 64, 64));
        state
            .sources
            .entry("probe".to_string())
            .or_insert_with(crate::layers::Device::default);
        let sim = crate::listeners::Listeners::stand_in();
        (paint, state, sim, author)
    }

    fn params(query: &[(&str, &str)]) -> Params {
        Params {
            query: query
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<_, _>>(),
            body: Vec::new(),
            tail: None,
        }
    }

    /// A device ceded to another program refuses `output_jpg` with 409
    /// rather than quietly serving a black JPEG.
    #[test]
    fn output_jpg_refuses_a_ceded_device_with_conflict() {
        let (mut paint, mut state, mut sim, author) = fixture();
        state.outputs.find_mut("probe").unwrap().ceded_to = Some("resolume".to_string());
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };

        let result = output_jpg(&mut ctx, &params(&[("device", "probe")]));
        assert!(matches!(result, Err(CallError::Conflict(_))));
    }

    /// A device that has not been ceded must not hit the conflict branch —
    /// it may still fail for other reasons in this minimal fixture (no real
    /// paint backend), so this only asserts the cede gate stays out of it.
    #[test]
    fn output_jpg_does_not_conflict_when_not_ceded() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };

        let result = output_jpg(&mut ctx, &params(&[("device", "probe")]));
        assert!(!matches!(result, Err(CallError::Conflict(_))));
    }
}

#[cfg(test)]
mod remote_tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Instant;

    /// Reads one request, returns its path; the caller answers.
    fn read_request(stream: &mut TcpStream) -> String {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap() == 0 || h.trim().is_empty() {
                break;
            }
        }
        line.split_whitespace().nth(1).unwrap_or_default().to_string()
    }

    fn answer(stream: &mut TcpStream, body: &[u8]) {
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
    }

    /// Fake peers on the ports `metal-dash` and `metal-sync` really use: `:7900`
    /// is a metal-dash answering `/render/<layer>/<frame>` with a JPEG; `:7901`
    /// is an metal-sync that relays a layer ask to it, the way a peer's
    /// `fw/frame/1` would. `None` when either port is already taken (a live
    /// dashboard on this machine), so the test skips instead of fighting it.
    fn fake_peers(w: u32, h: u32) -> Option<()> {
        let dash = TcpListener::bind(("127.0.0.1", 7900)).ok()?;
        let sync = TcpListener::bind(("127.0.0.1", 7901)).ok()?;
        let rgba = vec![0x80u8; (w * h * 4) as usize];
        let jpeg = crate::paint::encode_jpeg(&rgba, w, h);
        std::thread::spawn(move || {
            for mut s in dash.incoming().flatten() {
                let path = read_request(&mut s);
                assert!(path.starts_with("/render/"), "fake metal-dash got {path}");
                answer(&mut s, &jpeg);
            }
        });
        std::thread::spawn(move || {
            for mut s in sync.incoming().flatten() {
                let path = read_request(&mut s);
                // `/frame?layer=L&frame=N&on=P&w=W&h=H` → the peer's `/render`.
                let query = path.split_once('?').map(|(_, q)| q).unwrap_or_default();
                let get = |k: &str| {
                    query.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}=")).map(str::to_string))
                        .unwrap_or_default()
                };
                let upstream = format!(
                    "/render/{}/{}?w={}&h={}",
                    get("layer"),
                    get("frame"),
                    get("w"),
                    get("h")
                );
                let mut peer = TcpStream::connect(("127.0.0.1", 7900)).unwrap();
                write!(peer, "GET {upstream} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
                let mut resp = Vec::new();
                peer.read_to_end(&mut resp).unwrap();
                let body_at = resp.windows(4).position(|w| w == b"\r\n\r\n").map_or(0, |i| i + 4);
                answer(&mut s, &resp[body_at..]);
            }
        });
        Some(())
    }

    /// Frames per second a remote layer's frames arrive at over localhost,
    /// one GET per frame through the real `metal-sync` client path. Prints the
    /// number; the floor is loose on purpose (CI boxes vary) — the point is
    /// the measurement, and whether the stream option is needed at all.
    #[test]
    fn remote_layer_frames_per_second_on_localhost() {
        let (w, h) = (1280u32, 720u32);
        if fake_peers(w, h).is_none() {
            eprintln!("skipped: :7900 or :7901 is already taken on this machine");
            return;
        }
        // Let the accept threads come up.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let frames = 120u64;
        let (mut wire, mut decode) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
        let started = Instant::now();
        for f in 0..frames {
            let t = Instant::now();
            let bytes = crate::monitor::fetch_sync_bytes(&format!(
                "/frame?layer=l1&frame={f}&on=stage&w={w}&h={h}"
            ))
            .expect("fake metal-sync answers");
            wire += t.elapsed();
            let t = Instant::now();
            let decoded = crate::shell::frames::decode_jpeg_rgb(&bytes);
            decode += t.elapsed();
            assert!(decoded.is_some(), "frame {f} did not decode");
        }
        let fps = frames as f64 / started.elapsed().as_secs_f64();
        // The peer's side: one JPEG encode per frame, the same encoder the route uses.
        let rgba = vec![0x80u8; (w * h * 4) as usize];
        let t = Instant::now();
        for _ in 0..frames {
            std::hint::black_box(crate::paint::encode_jpeg(&rgba, w, h));
        }
        let encode = t.elapsed();
        eprintln!(
            "remote layer over localhost, one GET per frame, {w}x{h}: {fps:.1} frames/s \
             (GET {:.1} ms, decode {:.1} ms, peer encode {:.1} ms per frame)",
            wire.as_secs_f64() * 1e3 / frames as f64,
            decode.as_secs_f64() * 1e3 / frames as f64,
            encode.as_secs_f64() * 1e3 / frames as f64,
        );
        assert!(fps > 5.0, "{fps:.1} frames/s is below the floor");
    }
}

#[cfg(test)]
/// A device's finished frame at `w × h`: composed (remote layers included),
/// masked, calibration grid on top — the one picture `GET /output.jpg`
/// encodes and the P-C sinks push (rule 3: a view of the frame, never a
/// second render path).
pub(crate) fn device_frame(
    paint: &mut dyn crate::paint::Paint,
    sim: &mut crate::listeners::Listeners,
    state: &mut crate::State,
    out: &crate::output::Output,
    w: u32,
    h: u32,
) -> (Vec<u8>, u32, u32) {
    match device_texture(paint, sim, state, out, w, h) {
        metal_perform::engine::DeviceFrame::Cpu(px, pw, ph) => (px, pw, ph),
        metal_perform::engine::DeviceFrame::Gpu(f) => (paint.readback(&f), f.width(), f.height()),
    }
}

/// The device's finished frame: the composed stack, then the device-wide mask
/// cut and the calibration grid. On the GPU backend all of it is a texture
/// (the cut is a coverage map, the grid a painted layer), so nothing reads
/// back until a sink asks.
pub(crate) fn device_texture(
    paint: &mut dyn crate::paint::Paint,
    sim: &mut crate::listeners::Listeners,
    state: &mut crate::State,
    out: &crate::output::Output,
    w: u32,
    h: u32,
) -> metal_perform::engine::DeviceFrame {
    use metal_perform::engine::DeviceFrame;
    let id = out.id.clone();
    let ctx_state = state;
    // Remote layers are composed at their own place in the stack, from the
    // prefetcher: this never waits for a peer (see `remote_layers`).
    #[cfg(all(feature = "desktop", any(feature = "gpu", feature = "glow")))]
    let mut remote = |ask: &metal_perform::engine::RemoteAsk| crate::remote_layers::global().frame(ask);
    #[cfg(not(all(feature = "desktop", any(feature = "gpu", feature = "glow"))))]
    let mut remote = |_: &metal_perform::engine::RemoteAsk| None;
    let frame = metal_perform::engine::render_device_frame(paint, sim, ctx_state, &id, w, h, &mut remote);
    // R22.6 decision 3: a device whose graph carries a live `mask` node cuts
    // per layer, so the device-wide pass is skipped — the graph is the truth
    // (regla 4). No `mask` node: unchanged.
    let cuts = !crate::graph_has_mask_node(&crate::output::data_root(), &id);
    // A device whose plan carries transient (3D) masks would hand them here;
    // the whole-frame path has none today, so only the persisted set cuts, and
    // the device's composition decides whether a future transient set is
    // skipped (sectors).
    let composition = ctx_state.sources.get(&id).map(|d| d.composition).unwrap_or_default();
    let gpu = match frame {
        DeviceFrame::Gpu(f) => f,
        DeviceFrame::Cpu(mut pixels, pw, ph) => {
            if cuts {
                crate::output::apply_masks_with(&mut pixels, pw, ph, &out.masks, &[], composition);
            }
            if out.calibrating {
                crate::output::draw_calibration(&mut pixels, pw, ph, &out.masks);
            }
            return DeviceFrame::Cpu(pixels, pw, ph);
        }
    };
    let mut cur = gpu.clone();
    let mut ok = true;
    if cuts {
        match crate::output::apply_masks_gpu(paint, &cur, &out.masks, &[], composition, &Default::default(), true) {
            Some(n) => cur = n,
            None => ok = false,
        }
    }
    if ok && out.calibrating {
        // The grid and outlines are drawn by the same CPU routine into a
        // transparent layer (it only ever writes non-black colours), then laid
        // over the frame on the GPU.
        let (pw, ph) = (cur.width(), cur.height());
        let mut layer = vec![0u8; pw as usize * ph as usize * 4];
        crate::output::draw_calibration(&mut layer, pw, ph, &out.masks);
        for p in layer.chunks_exact_mut(4) {
            if p[..3].iter().any(|c| *c != 0) {
                p[3] = 255;
            }
        }
        match paint.shade_gpu(&cur, &metal_perform::gpu_frame::Shade::Over { rgba: &layer }) {
            Some(n) => cur = n,
            None => ok = false,
        }
    }
    if ok {
        return DeviceFrame::Gpu(cur);
    }
    // The painter could not shade: the CPU passes on the read-back frame.
    let mut pixels = paint.readback(&gpu);
    let (pw, ph) = (gpu.width(), gpu.height());
    if cuts {
        crate::output::apply_masks_with(&mut pixels, pw, ph, &out.masks, &[], composition);
    }
    if out.calibrating {
        crate::output::draw_calibration(&mut pixels, pw, ph, &out.masks);
    }
    DeviceFrame::Cpu(pixels, pw, ph)
}


/// G3: the frame behind a `GET` of `/output.jpg` or `/preview.jpg` as a
/// texture on the painter's own device, for a window that shares it. `None`
/// for any other route, for an unknown or ceded device, and when the frame
/// came out as bytes (a CPU backend, a black unplanned device): the caller
/// then asks the JPEG route as before.
pub fn texture_of(ctx: &mut Ctx, path: &str, params: &Params) -> Option<metal_perform::gpu_frame::GpuFrame> {
    use metal_perform::engine::DeviceFrame;
    // G4: a node's or a model's own picture, as a texture when the painter kept
    // it one (a CPU painter or a readback-bound buffer answers `None`).
    match path {
        "/node.jpg" => {
            let device = params.param("device")?.to_string();
            let node = params.param("node")?.to_string();
            let w = params.num("w", 240.0).clamp(32.0, 1920.0) as u32;
            let h = params.num("h", 135.0).clamp(18.0, 1080.0) as u32;
            let gid = params.param("graph").map(str::to_string);
            return metal_perform::engine::with_preview_graph(gid.as_deref(), || {
                crate::render_node_texture(ctx.paint, ctx.sim, ctx.state, &device, &node, w, h)
            });
        }
        "/sensor.jpg" => {
            // G6: the sensor's capture frame, a texture when the painter is GPU.
            return super::sensor::preview_texture(ctx.paint, params);
        }
        "/source.jpg" => {
            // G5: a source's thumbnail: its solo picture, a texture when the
            // painter draws it on the GPU.
            let device = params.param("device")?.to_string();
            let layer = params.param("layer")?.to_string();
            let w = params.num("w", 240.0).clamp(32.0, 480.0) as u32;
            let h = params.num("h", 135.0).clamp(18.0, 270.0) as u32;
            let gid = params.param("graph").map(str::to_string);
            return metal_perform::engine::with_preview_graph(gid.as_deref(), || {
                crate::render_source_texture(ctx.paint, ctx.sim, ctx.state, &device, &layer, w, h)
            });
        }
        _ => {}
    }
    let frame = match path {
        "/preview.jpg" => {
            let w = params.num("w", 640.0).clamp(64.0, 1920.0) as u32;
            let h = params.num("h", 360.0).clamp(36.0, 1080.0) as u32;
            metal_perform::engine::render_device_frame(
                ctx.paint, ctx.sim, ctx.state, crate::PREVIEW, w, h, &mut |_| None,
            )
        }
        "/output.jpg" => {
            let id = params.param("device")?.to_string();
            let out = ctx.state.outputs.find(&id).cloned()?;
            if out.ceded_to.is_some() {
                return None;
            }
            let (w, h) = output_size(&out, params);
            device_texture(ctx.paint, ctx.sim, ctx.state, &out, w, h)
        }
        _ => return None,
    };
    match frame {
        DeviceFrame::Gpu(f) => Some(f),
        DeviceFrame::Cpu(..) => None,
    }
}

/// The size `/output.jpg` draws a device at: its own, clamped, shrunk to fit
/// a `w`/`h` box when the caller names one.
fn output_size(out: &crate::output::Output, params: &Params) -> (u32, u32) {
    let (mut w, mut h) = (out.width.clamp(64, 1920), out.height.clamp(36, 1080));
    if let (Some(bw), Some(bh)) = (params.param("w"), params.param("h")) {
        let bw = bw.parse::<f32>().unwrap_or(0.0).clamp(32.0, 1920.0);
        let bh = bh.parse::<f32>().unwrap_or(0.0).clamp(18.0, 1080.0);
        let scale = (bw / w as f32).min(bh / h as f32).min(1.0);
        w = ((w as f32 * scale) as u32).max(32);
        h = ((h as f32 * scale) as u32).max(18);
    }
    (w, h)
}

#[cfg(all(test, feature = "gpu"))]
mod window_tests {
    use crate::shell::present::{Presenter, Shown};

    /// A frame the worker lands is registered with egui on the window's device
    /// and reads back as a ready texture; the next frame reuses the id.
    #[test]
    fn a_landed_frame_becomes_an_egui_texture_on_the_shared_device() {
        let gpu = metal_render::gpu::Gpu::new_headless();
        let cx = metal_perform::gpu_compose::GpuCx::new(gpu.device.clone(), gpu.queue.clone());
        let mut renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            egui_wgpu::RendererOptions::default(),
        );
        let p = Presenter::default();
        assert!(matches!(p.shown("k"), Shown::Pending));
        assert!(p.begin("k") && !p.begin("k"), "one fetch in flight per key");
        p.land("k", Some(cx.upload(&[255; 16 * 9 * 4], 16, 9)));
        crate::shell::window::present_pending(&p, &mut renderer, &gpu.device, None);
        let Shown::Ready { id, size, landed } = p.shown("k") else { panic!("not ready") };
        assert_eq!((size, landed), ([16, 9], 1));
        assert!(p.begin("k"));
        p.land("k", Some(cx.upload(&[0; 32 * 18 * 4], 32, 18)));
        crate::shell::window::present_pending(&p, &mut renderer, &gpu.device, None);
        let Shown::Ready { id: again, size, .. } = p.shown("k") else { panic!("not ready") };
        assert_eq!((id, size), (again, [32, 18]));
        p.land("none", None);
        assert!(matches!(p.shown("none"), Shown::Unsupported));
    }

    /// G5: the author's solo picture as a texture on the painter's device
    /// matches the `solo.jpg` route's pixels (same row, same size, same draw
    /// path), up to the animation's own clock between two calls.
    #[test]
    fn a_solo_texture_matches_the_solo_pixels_route() {
        let mut paint = crate::paint::painter();
        let Some(gpu) = paint.shared_gpu() else {
            eprintln!("skipped: the CPU painter has no device to share");
            return;
        };
        // The studio reads `data/schema.json` off a relative path: build it
        // from the repo root, as the other fixtures here do.
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir to repo root");
        let author = crate::Author::with_gpu(Some(gpu));
        std::env::set_current_dir(&prior).expect("restore cwd");
        let model = paint.ids()[0].to_string();
        let (w, h) = (96u32, 54u32);
        let (pixels, pw, ph) = author.solo_pixels(&model, None, None, w, h).expect("the route draws");
        let frame = author
            .solo_texture(paint.as_mut(), &[("model", model.clone()), ("w", w.to_string()), ("h", h.to_string())])
            .expect("the author's renderers are on the painter's device");
        assert_eq!((frame.width(), frame.height()), (pw, ph));
        let texture = paint.readback(&frame);
        assert_eq!(texture.len(), pixels.len());
        let mean = texture.iter().zip(&pixels).map(|(a, b)| (i32::from(*a) - i32::from(*b)).unsigned_abs() as f64).sum::<f64>()
            / texture.len() as f64;
        assert!(mean < 8.0, "mean difference {mean:.2} per byte");
    }

    /// G5: the model grid's cost per frame: the `grid.jpg` route (render,
    /// readback, JPEG encode, decode) against the grid texture on the painter's
    /// device. `cargo test -p metal-dash --lib --release
    /// call::render::window_tests::measure_grid -- --ignored --nocapture`
    #[test]
    #[ignore = "prints the G5 grid numbers"]
    fn measure_grid() {
        let mut paint = crate::paint::painter();
        let Some(gpu) = paint.shared_gpu() else {
            eprintln!("skipped: the CPU painter has no device to share");
            return;
        };
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir to repo root");
        let author = crate::Author::with_gpu(Some(gpu));
        std::env::set_current_dir(&prior).expect("restore cwd");
        let device = paint.gpu_cx().expect("gpu painter").device.clone();
        let query = [("w", "320".to_string()), ("h", "180".to_string()), ("cols", "4".to_string()), ("cells", "0".to_string())];
        let n = 10;
        let t = std::time::Instant::now();
        for _ in 0..n {
            let req = metal_web::Request {
                method: "GET".into(),
                path: "/render/grid.jpg".into(),
                query: query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
                headers: Default::default(),
                body: Vec::new(),
            };
            let reply = author.dispatch(&req);
            std::hint::black_box(reply.body.len());
            let decoded = crate::shell::frames::decode_jpeg_rgb(&reply.body);
            std::hint::black_box(decoded.is_some());
        }
        let before = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        let q: Vec<(&str, String)> = query.iter().map(|(k, v)| (*k, v.clone())).collect();
        let cx = paint.gpu_cx().expect("gpu painter");
        let t = std::time::Instant::now();
        for _ in 0..n {
            let f = author.grid_texture(&cx, &q).expect("a texture");
            std::hint::black_box(f.width());
            let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        }
        let after = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("MEASURE grid 320x180 tiles x4 cols: JPEG route {before:.1} ms, texture {after:.1} ms");
    }

    /// Rule 11: the solo preview's cost per frame, before (`solo.jpg`: render,
    /// readback, JPEG encode, JPEG decode) and after (the texture the author's
    /// renderer drew on the painter's device, with nothing read back).
    /// `cargo test -p metal-dash --lib --release call::render::window_tests::measure_solo
    /// -- --ignored --nocapture`
    #[test]
    #[ignore = "prints the G5 solo numbers"]
    fn measure_solo() {
        let mut paint = crate::paint::painter();
        let Some(gpu) = paint.shared_gpu() else {
            eprintln!("skipped: the CPU painter has no device to share");
            return;
        };
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir to repo root");
        let author = crate::Author::with_gpu(Some(gpu));
        std::env::set_current_dir(&prior).expect("restore cwd");
        let model = paint.ids()[0].to_string();
        let device = paint.gpu_cx().expect("gpu painter").device.clone();
        for (w, h) in [(960u32, 540u32), (1920, 1080)] {
            let q = [("model", model.clone()), ("w", w.to_string()), ("h", h.to_string())];
            let n = 10;
            let t = std::time::Instant::now();
            for _ in 0..n {
                let (px, pw, ph) = author.solo_pixels(&model, None, None, w, h).expect("the route draws");
                let jpg = crate::paint::encode_jpeg(&px, pw, ph);
                let decoded = crate::shell::frames::decode_jpeg_rgb(&jpg).expect("decodes");
                std::hint::black_box(decoded);
            }
            let before = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
            let t = std::time::Instant::now();
            for _ in 0..n {
                let f = author.solo_texture(paint.as_mut(), &q).expect("a texture");
                std::hint::black_box(f.width());
                let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
            }
            let after = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
            println!("MEASURE solo {w}x{h}: JPEG route {before:.1} ms, texture {after:.1} ms");
        }
    }

    /// Rule 11: the window's cost per preview frame, before (render -> readback
    /// -> JPEG encode -> JPEG decode -> upload to egui) and after (the compose
    /// texture registered with egui). `cargo test -p metal-dash --lib --release
    /// call::render::window_tests::measure -- --ignored --nocapture`.
    #[test]
    #[ignore = "prints the G3 window numbers"]
    fn measure() {
        use metal_perform::layers::{self, Layer};
        let mut paint = crate::paint::painter();
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = crate::State::new(paint.as_ref());
        state.paused_at = Some(3.0);
        for (w, h) in [(960u32, 540u32), (1920, 1080)] {
            let id = format!("measure-{w}");
            let out = crate::output::Output::new(&id, "m", w, h);
            state.outputs.list.push(out.clone());
            state.sources.insert(id.clone(), layers::Device::default());
            state.layers = (0..3)
                .map(|i| {
                    let mut l = Layer::new(&format!("m{w}-{i}"), "l", i);
                    l.outputs = vec![id.clone()];
                    l
                })
                .collect();
            let cx = paint.gpu_cx().expect("gpu painter");
            let gpu = metal_render::gpu::Gpu::new_headless();
            let mut renderer = egui_wgpu::Renderer::new(&cx.device, wgpu::TextureFormat::Bgra8UnormSrgb, Default::default());
            drop(gpu);
            let n = 20;
            let t = std::time::Instant::now();
            for _ in 0..n {
                let (px, pw, ph) = super::device_frame(paint.as_mut(), &mut sim, &mut state, &out, w, h);
                let jpg = crate::paint::encode_jpeg(&px, pw, ph);
                let (dw, dh, rgb) = crate::shell::frames::decode_jpeg_rgb(&jpg).expect("decodes");
                let img = egui::ColorImage::from_rgb([dw as usize, dh as usize], &rgb);
                std::hint::black_box(img);
            }
            let before = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
            let p = Presenter::default();
            let t = std::time::Instant::now();
            for _ in 0..n {
                let f = match super::device_texture(paint.as_mut(), &mut sim, &mut state, &out, w, h) {
                    metal_perform::engine::DeviceFrame::Gpu(f) => f,
                    metal_perform::engine::DeviceFrame::Cpu(..) => panic!("texture expected"),
                };
                p.begin("m");
                p.land("m", Some(f));
                crate::shell::window::present_pending(&p, &mut renderer, &cx.device, None);
                let _ = cx.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
            }
            let after = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
            println!("MEASURE window preview {w}x{h}: JPEG path {before:.1} ms, texture path {after:.1} ms");
        }
    }
}
