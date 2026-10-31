//! `fw/frame/1`: frames that travel between machines.
//!
//! `metal-dash@X` asks its own `metal-sync` for `GET :7901/frame?device=D`. This
//! module reads `data/outputs.json` (synced, so every machine agrees), takes
//! D's `composed_on` — or, absent that, its `node` — and resolves that
//! machine name through the `name ↔ node_id` table:
//!
//! - no name, or our own name: the device is on this machine, so the frame is
//!   a loopback `GET :7900/frame?device=D` and never touches QUIC;
//! - a name in the table: one bi-stream on ALPN `fw/frame/1` to that peer,
//!   whose `metal-sync` makes the same loopback GET to *its* `metal-dash` and
//!   streams the answer back;
//! - a name nobody in the table carries: 404. There is no fallback — no
//!   "maybe it's local", no rendering here. The caller paints black.
//!
//! The second request kind, `{ layer, frame, w, h }`, is routed the same way
//! (by the `on` machine name). The machine that renders the layer answers it
//! with its own `metal-dash`'s `GET /render/<layer>/<frame>` — that one layer
//! drawn alone at that frame, as a JPEG. The requesting `metal-dash` composes it.
//!
//! Only members of the document are answered. A connection from any other
//! key is closed before a single byte of the request is read.
//!
//! # Wire
//!
//! Request: one JSON object, `finish`. Reply: a 2-byte big-endian status,
//! then the body, `finish`. The status is `metal-dash`'s own, so a ceded device
//! (409) stays a 409 across the trip.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    EndpointId,
};
use serde_json::{json, Value};

use crate::pair::{Club, PairBook};

pub const ALPN: &[u8] = b"fw/frame/1";

/// A composed JPEG at 1920×1080 is well under this; the cap only bounds a
/// misbehaving peer.
pub const MAX_FRAME: usize = 32 * 1024 * 1024;

/// What a caller can ask for.
#[derive(Debug, Clone, PartialEq)]
pub enum FrameAsk {
    Device(String),
    /// `w`/`h`: the size the requester composes at; the renderer draws the
    /// layer at exactly that size so no resampling happens on either side.
    /// `state`: what the requester's frame resolved to (model, float row,
    /// time). When present the peer draws from it and not from its own state,
    /// so the same ask gives the same pixels on any peer.
    /// `fmt`/`q`: the codec the requester can read (`jpeg`, `qoi`, `rgba`) and
    /// the JPEG quality. Absent, the peer answers in its default (JPEG).
    Layer {
        layer: String,
        frame: u64,
        on: Option<String>,
        w: u32,
        h: u32,
        state: Option<LayerState>,
        fmt: Option<String>,
        q: Option<u8>,
    },
}

/// The state a layer frame is drawn from, carried with the ask: which model
/// the requester's layer resolved to, the float row the requester computed
/// for it (audio digest and effects already folded in), and the draw time in
/// seconds. Floats travel as their shortest round-trip decimal, so the peer
/// reads back the exact bits the requester had.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerState {
    pub model: usize,
    pub time: f32,
    pub row: Vec<f32>,
    /// The generator's resolved `fx.*` chain (see `metal_schema::chain_to_query`).
    pub chain: Vec<metal_schema::EffectStep>,
    /// The source's `fondo_alpha`: the backdrop slot renders as alpha.
    pub fondo_alpha: bool,
}

impl LayerState {
    /// `&model=…&time=…&row=a,b,c[&chain=…]` — appended to a `/frame` or `/render` query.
    pub fn query(&self) -> String {
        let row: Vec<String> = self.row.iter().map(|f| finite(*f).to_string()).collect();
        let chain = if self.chain.is_empty() {
            String::new()
        } else {
            format!("&chain={}", metal_schema::chain_to_query(&self.chain))
        };
        let background = if self.fondo_alpha { "&fondo=1" } else { "" };
        format!("&model={}&time={}&row={}{chain}{background}", self.model, finite(self.time), row.join(","))
    }

    /// Reads the three keys back out of a query (`get` looks one key up).
    /// `None` unless all three are present and parse; a request with none of
    /// them is a request for the peer's own state.
    pub fn from_query(get: impl Fn(&str) -> Option<String>) -> Option<LayerState> {
        let model = get("model")?.parse().ok()?;
        let time = get("time")?.parse().ok()?;
        let row = get("row")?
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.parse::<f32>().ok())
            .collect::<Option<Vec<f32>>>()?;
        // A missing chain is an empty one; a malformed one is refused.
        let chain = match get("chain") {
            Some(text) => metal_schema::chain_from_query(&text)?,
            None => Vec::new(),
        };
        let fondo_alpha = get("fondo").as_deref() == Some("1");
        Some(LayerState { model, time, row, chain, fondo_alpha })
    }
}

/// A float the query can carry: NaN and the infinities become 0.
fn finite(f: f32) -> f32 {
    if f.is_finite() { f } else { 0.0 }
}

/// The size a layer ask carries when the requester did not name one.
pub const DEFAULT_LAYER_W: u32 = 1280;
pub const DEFAULT_LAYER_H: u32 = 720;
/// The largest layer ask: a layer spanning a few devices is drawn at the
/// covering canvas, which can be wider than any one output.
pub const MAX_LAYER_W: u32 = 4096;
pub const MAX_LAYER_H: u32 = 2160;

fn layer_state_json(v: &Value) -> Option<LayerState> {
    let row = v["row"]
        .as_array()?
        .iter()
        .map(|f| f.as_f64().map(|f| f as f32))
        .collect::<Option<Vec<f32>>>()?;
    let chain = v["chain"]
        .as_str()
        .map_or(Some(Vec::new()), metal_schema::chain_from_query)?;
    let fondo_alpha = v["fondo_alpha"].as_bool().unwrap_or(false);
    Some(LayerState { model: v["model"].as_u64()? as usize, time: v["time"].as_f64()? as f32, row, chain, fondo_alpha })
}

impl FrameAsk {
    fn to_json(&self) -> Value {
        match self {
            FrameAsk::Device(d) => json!({ "device": d }),
            FrameAsk::Layer { layer, frame, w, h, state, fmt, q, .. } => {
                let mut v = json!({ "layer": layer, "frame": frame, "w": w, "h": h, "fmt": fmt, "q": q });
                if let Some(s) = state {
                    v["state"] = json!({
                        "model": s.model,
                        "time": finite(s.time),
                        "row": s.row.iter().map(|f| finite(*f)).collect::<Vec<f32>>(),
                        "chain": metal_schema::chain_to_query(&s.chain),
                        "fondo_alpha": s.fondo_alpha,
                    });
                }
                v
            }
        }
    }

    fn from_json(v: &Value) -> Option<FrameAsk> {
        if let Some(d) = v["device"].as_str() {
            return Some(FrameAsk::Device(d.to_string()));
        }
        Some(FrameAsk::Layer {
            layer: v["layer"].as_str()?.to_string(),
            frame: v["frame"].as_u64()?,
            on: None,
            w: v["w"].as_u64().map_or(DEFAULT_LAYER_W, |x| x as u32).clamp(32, MAX_LAYER_W),
            h: v["h"].as_u64().map_or(DEFAULT_LAYER_H, |x| x as u32).clamp(18, MAX_LAYER_H),
            state: layer_state_json(&v["state"]),
            fmt: v["fmt"].as_str().map(str::to_string),
            q: v["q"].as_u64().map(|q| q as u8),
        })
    }
}

/// Where a frame comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// This machine's own `metal-dash`.
    Local,
    Peer { name: String, node_id: String },
    /// A machine name that is not ours and not in the table.
    UnknownName(String),
    /// A device `outputs.json` does not list.
    NoDevice,
}

/// The machine named for a device: `composed_on`, else `node`, else none
/// (this machine). `Err` when the device is not in `outputs.json`.
pub fn machine_for(data_root: &Path, device: &str) -> Result<Option<String>, ()> {
    let text = std::fs::read_to_string(data_root.join("outputs.json")).map_err(|_| ())?;
    let list: Value = serde_json::from_str(&text).map_err(|_| ())?;
    let out = list.as_array().into_iter().flatten().find(|o| o["id"] == device).ok_or(())?;
    let pick = |k: &str| out[k].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    Ok(pick("composed_on").or_else(|| pick("node")))
}

/// A machine name to a source.
pub fn source_for_machine(book: &PairBook, machine: Option<&str>) -> Source {
    match machine {
        None => Source::Local,
        Some(m) if m == book.name() => Source::Local,
        Some(m) => match book.node_of(m) {
            Some(id) => Source::Peer { name: m.to_string(), node_id: id.to_string() },
            None => Source::UnknownName(m.to_string()),
        },
    }
}

/// Resolve an ask to where it should be answered.
pub fn resolve(data_root: &Path, book: &PairBook, ask: &FrameAsk) -> Source {
    match ask {
        FrameAsk::Device(d) => match machine_for(data_root, d) {
            Ok(m) => source_for_machine(book, m.as_deref()),
            Err(()) => Source::NoDevice,
        },
        FrameAsk::Layer { on, .. } => source_for_machine(book, on.as_deref()),
    }
}

/// Status and body, as `metal-dash` (or this module) answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Frame {
    fn error(status: u16, why: &str) -> Frame {
        Frame { status, body: json!({ "error": why }).to_string().into_bytes() }
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = self.status.to_be_bytes().to_vec();
        out.extend_from_slice(&self.body);
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Frame> {
        let (head, body) = bytes.split_at_checked(2)?;
        Some(Frame { status: u16::from_be_bytes([head[0], head[1]]), body: body.to_vec() })
    }
}

/// Answer an ask on *this* machine: a loopback GET to `metal-dash`.
pub fn answer_locally(dash_port: u16, ask: &FrameAsk) -> Frame {
    match ask {
        FrameAsk::Device(d) => {
            let path = format!("/frame?device={}", percent_encode(d));
            match loopback_get(dash_port, &path) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(%e, device = %d, "metal-dash did not answer for a frame");
                    Frame::error(502, "metal-dash is not reachable")
                }
            }
        }
        FrameAsk::Layer { layer, frame, w, h, state, fmt, q, .. } => {
            let state = state.as_ref().map(LayerState::query).unwrap_or_default();
            let codec = match fmt {
                Some(f) => format!("&fmt={}", percent_encode(f)),
                None => String::new(),
            } + &q.map(|q| format!("&q={q}")).unwrap_or_default();
            let path = format!("/render/{}/{frame}?w={w}&h={h}{state}{codec}", percent_encode(layer));
            match loopback_get(dash_port, &path) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(%e, %layer, "metal-dash did not answer for a layer frame");
                    Frame::error(502, "metal-dash is not reachable")
                }
            }
        }
    }
}

/// How a peer is reached right now, from its transport addresses: `direct`
/// when an address on a direct IP path is in active use, `relay` when only a
/// relay path is, `None` when nothing is in use (not connected, or unknown).
/// Each item is `(is_relay, is_active)`.
pub fn via_of(paths: impl IntoIterator<Item = (bool, bool)>) -> Option<&'static str> {
    let active: Vec<bool> = paths.into_iter().filter(|(_, on)| *on).map(|(relay, _)| relay).collect();
    if active.iter().any(|relay| !relay) {
        Some("direct")
    } else if !active.is_empty() {
        Some("relay")
    } else {
        None
    }
}

impl crate::pair::Net {
    /// The path a peer's traffic takes now (`direct` or `relay`), from the
    /// endpoint's view of it. `None` when the endpoint knows no active path.
    pub async fn via(&self, id: EndpointId) -> Option<&'static str> {
        let info = self.endpoint.remote_info(id).await?;
        via_of(info.addrs().map(|a| (a.addr().is_relay(), matches!(a.usage(), iroh::endpoint::TransportAddrUsage::Active))))
    }
}

/// Answer an outputs ask on *this* machine: the `outputs` array of the local
/// `metal-dash`'s `GET /state`, and nothing else of it.
pub fn answer_outputs_locally(dash_port: u16) -> Frame {
    match loopback_get(dash_port, "/state") {
        Ok(f) if f.status == 200 => match serde_json::from_slice::<Value>(&f.body) {
            Ok(state) => {
                let outputs = state.get("outputs").cloned().unwrap_or_else(|| json!([]));
                Frame { status: 200, body: json!({ "outputs": outputs }).to_string().into_bytes() }
            }
            Err(_) => Frame::error(502, "metal-dash's state is not JSON"),
        },
        Ok(f) => Frame::error(f.status, "metal-dash did not serve its state"),
        Err(e) => {
            tracing::warn!(%e, "metal-dash did not answer for its state");
            Frame::error(502, "metal-dash is not reachable")
        }
    }
}

/// The peer side: `fw/frame/1` on the router.
#[derive(Debug, Clone)]
pub struct FrameProtocol {
    pub club: Arc<Club>,
}

impl ProtocolHandler for FrameProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let from = conn.remote_id().to_string();
        let member = self.club.book.lock().expect("pair book poisoned").is_member(&from);
        if !member {
            // No answer at all — not even an error body a stranger could
            // learn from.
            tracing::debug!(%from, "frame request from a non-member; closed");
            conn.close(1u32.into(), b"");
            return Ok(());
        }
        self.serve(conn).await.map_err(|e| AcceptError::from_boxed(e.into()))
    }
}

impl FrameProtocol {
    async fn serve(&self, conn: Connection) -> Result<()> {
        let (mut send, mut recv) = conn.accept_bi().await?;
        let req: Value = serde_json::from_slice(&recv.read_to_end(4096).await?)?;
        if let Some(rel) = req["asset"].as_str() {
            // A song's file, streamed (`pull.rs`): the reply is not a Frame.
            let from = req["from"].as_u64().unwrap_or(0);
            crate::pull::stream_asset(&mut send, &self.club.data_root, rel, from).await?;
        } else {
            let frame = if let Some((kind, name)) = crate::pull::Kind::of_ask(&req) {
                let (root, name) = (self.club.data_root.clone(), name.to_string());
                tokio::task::spawn_blocking(move || crate::pull::answer_bundle(&root, kind, &name)).await?
            } else if req["outputs"] == json!(true) {
                let port = self.club.dash_port;
                tokio::task::spawn_blocking(move || answer_outputs_locally(port)).await?
            } else {
                match FrameAsk::from_json(&req) {
                    Some(ask) => {
                        let port = self.club.dash_port;
                        tokio::task::spawn_blocking(move || answer_locally(port, &ask)).await?
                    }
                    None => Frame::error(400, "neither a device nor a layer frame"),
                }
            };
            send.write_all(&frame.encode()).await?;
        }
        send.finish()?;
        conn.closed().await;
        Ok(())
    }
}

impl crate::pair::Net {
    /// `GET /frame` on `:7901`: resolve, then loopback or one QUIC trip.
    pub async fn frame(&self, ask: FrameAsk) -> Frame {
        let source = {
            let book = self.club.book.lock().expect("pair book poisoned");
            resolve(&self.club.data_root, &book, &ask)
        };
        match source {
            Source::Local => {
                let port = self.club.dash_port;
                tokio::task::spawn_blocking(move || answer_locally(port, &ask))
                    .await
                    .unwrap_or_else(|_| Frame::error(500, "frame task failed"))
            }
            Source::NoDevice => Frame::error(404, "no such device in outputs.json"),
            Source::UnknownName(name) => {
                tracing::warn!(%name, "a device names a machine that is not in the club");
                Frame::error(404, &format!("no peer named {name:?}"))
            }
            Source::Peer { name, node_id } => {
                let Ok(id) = node_id.parse::<EndpointId>() else {
                    return Frame::error(500, "the name table holds an unusable node id");
                };
                let addr = crate::pair::addr_for(&self.club, id);
                let req = ask.to_json().to_string();
                match crate::pair::exchange(&self.endpoint, addr, ALPN, req.as_bytes()).await {
                    Ok(bytes) => Frame::decode(&bytes)
                        .unwrap_or_else(|| Frame::error(502, "the peer sent no answer")),
                    Err(e) => {
                        tracing::warn!(%name, error = %format!("{e:#}"), "frame trip failed");
                        Frame::error(502, &format!("{name} did not answer"))
                    }
                }
            }
        }
    }
}

impl crate::pair::Net {
    /// `GET /peer/outputs?name=N` on `:7901`: the screens peer `N` shows,
    /// asked over `fw/frame/1` and answered by its own `metal-dash`. The body is
    /// `{ "outputs": [...] }` on 200, an `{ "error": ... }` otherwise.
    pub async fn peer_outputs(&self, name: &str) -> Frame {
        let node = {
            let book = self.club.book.lock().expect("pair book poisoned");
            book.node_of(name).map(str::to_string)
        };
        let Some(node) = node else {
            return Frame::error(404, &format!("no peer named {name:?}"));
        };
        let Ok(id) = node.parse::<EndpointId>() else {
            return Frame::error(500, "the name table holds an unusable node id");
        };
        let addr = crate::pair::addr_for(&self.club, id);
        match crate::pair::exchange(&self.endpoint, addr, ALPN, br#"{"outputs":true}"#).await {
            Ok(bytes) => Frame::decode(&bytes).unwrap_or_else(|| Frame::error(502, "the peer sent no answer")),
            Err(e) => {
                tracing::warn!(%name, error = %format!("{e:#}"), "outputs trip failed");
                Frame::error(502, &format!("{name} did not answer"))
            }
        }
    }
}

/// One blocking GET to the local `metal-dash`. Short timeouts: a wedged
/// dashboard must not hold a frame request open.
pub fn loopback_get(port: u16, path: &str) -> std::io::Result<Frame> {
    loopback_call(port, "GET", path, None)
}

/// [`loopback_get`] for a JSON `POST` (the farm worker starts its shard's
/// export through the same `/export/video` route the operator's button posts).
pub fn loopback_post(port: u16, path: &str, body: &Value) -> std::io::Result<Frame> {
    loopback_call(port, "POST", path, Some(body.to_string().as_bytes()))
}

fn loopback_call(port: u16, method: &str, path: &str, body: Option<&[u8]>) -> std::io::Result<Frame> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    match body {
        None => write!(stream, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")?,
        Some(b) => {
            write!(
                stream,
                "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                b.len()
            )?;
            stream.write_all(b)?;
        }
    }
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line)?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| std::io::Error::other("not an HTTP response"))?;
    let mut content_length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse::<usize>().ok();
            }
        }
    }
    let mut body = Vec::new();
    match content_length {
        Some(n) => {
            body.resize(n.min(MAX_FRAME), 0);
            reader.read_exact(&mut body)?;
        }
        None => {
            reader.take(MAX_FRAME as u64).read_to_end(&mut body)?;
        }
    }
    Ok(Frame { status, body })
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn data(outputs: Value) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("outputs.json"), outputs.to_string()).unwrap();
        dir
    }

    fn book() -> PairBook {
        let mut peers = BTreeMap::new();
        peers.insert("stage".to_string(), "S".to_string());
        PairBook::new("bar".into(), peers)
    }

    #[test]
    fn an_unknown_name_is_a_404_not_a_guess() {
        let d = data(json!([{ "id": "wall", "node": "booth" }]));
        let ask = FrameAsk::Device("wall".into());
        assert_eq!(resolve(d.path(), &book(), &ask), Source::UnknownName("booth".into()));
        // And a device nobody lists is its own 404, not "probably local".
        assert_eq!(resolve(d.path(), &book(), &FrameAsk::Device("ghost".into())), Source::NoDevice);
    }

    #[test]
    fn composed_on_wins_over_node_and_our_own_name_is_local() {
        let d = data(json!([
            { "id": "a", "node": "bar", "composed_on": "stage" },
            { "id": "b", "node": "bar" },
            { "id": "c" },
        ]));
        let b = book();
        assert_eq!(
            resolve(d.path(), &b, &FrameAsk::Device("a".into())),
            Source::Peer { name: "stage".into(), node_id: "S".into() }
        );
        assert_eq!(resolve(d.path(), &b, &FrameAsk::Device("b".into())), Source::Local);
        assert_eq!(resolve(d.path(), &b, &FrameAsk::Device("c".into())), Source::Local);
    }

    #[test]
    fn a_layer_frame_is_a_404_until_fw_dash_serves_one() {
        let ask = FrameAsk::Layer { layer: "l1".into(), frame: 3, on: None, w: 64, h: 36, state: None, fmt: None, q: None };
        // Port 1 has no listener: a layer ask is forwarded, and an unreachable
        // metal-dash is a 502, never a local guess.
        assert_eq!(answer_locally(1, &ask).status, 502);
    }

    /// The codec the requester negotiated rides to the peer's route, and the
    /// resolved `fx.*` chain rides with the state.
    #[test]
    fn a_layer_ask_carries_its_codec_and_chain_to_the_peer() {
        use metal_schema::EffectStep;
        let (port, seen) = fake_dash(b"qoif-BYTES", 200);
        let chain = vec![EffectStep { effect: 3, amount: 0.6, slot: 0 }];
        let state = LayerState { model: 1, time: 0.5, row: vec![0.5], chain: chain.clone(), fondo_alpha: true };
        let ask = FrameAsk::Layer {
            layer: "l1".into(),
            frame: 7,
            on: None,
            w: 64,
            h: 36,
            state: Some(state),
            fmt: Some("qoi".into()),
            q: None,
        };
        assert_eq!(answer_locally(port, &ask).status, 200);
        let path = seen.recv().unwrap();
        assert!(path.contains("&fmt=qoi"), "codec on the route: {path}");
        let query = path.split_once('?').unwrap().1;
        let get = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}="))).map(str::to_string);
        assert_eq!(LayerState::from_query(get).unwrap().chain, chain, "the chain survives the query");
        // And through the JSON the QUIC hop carries.
        let back = FrameAsk::from_json(&ask.to_json()).unwrap();
        assert_eq!(back, ask);
    }

    /// The layer ask reaches the loopback route with the frame and size in
    /// the path, and the route's JPEG comes back unchanged.
    #[test]
    fn a_layer_ask_is_answered_by_the_loopback_render_route() {
        let (port, seen) = fake_dash(b"JPEG-BYTES", 200);
        let ask = FrameAsk::Layer { layer: "l 1".into(), frame: 42, on: None, w: 640, h: 360, state: None, fmt: None, q: None };
        let f = answer_locally(port, &ask);
        assert_eq!(f, Frame { status: 200, body: b"JPEG-BYTES".to_vec() });
        assert_eq!(seen.recv().unwrap(), "/render/l%201/42?w=640&h=360");
    }

    /// The requester's state rides with the ask: the loopback route gets the
    /// model, time and float row, so the peer draws what the requester saw.
    #[test]
    fn a_layer_ask_carries_its_state_to_the_peer() {
        let (port, seen) = fake_dash(b"JPEG-BYTES", 200);
        let state = LayerState { model: 4, time: 1.5, row: vec![0.25, -0.5, 1.0], chain: vec![], fondo_alpha: false };
        let ask = FrameAsk::Layer { layer: "l1".into(), frame: 90, on: None, w: 64, h: 36, state: Some(state.clone()), fmt: None, q: None };
        assert_eq!(answer_locally(port, &ask).status, 200);
        assert_eq!(
            seen.recv().unwrap(),
            "/render/l1/90?w=64&h=36&model=4&time=1.5&row=0.25,-0.5,1"
        );
        // The wire form round-trips: the peer's `from_json` reads back the same state.
        let back = FrameAsk::from_json(&FrameAsk::to_json(&ask)).expect("parses");
        assert_eq!(back, FrameAsk::Layer { layer: "l1".into(), frame: 90, on: None, w: 64, h: 36, state: Some(state), fmt: None, q: None });
    }

    #[test]
    fn layer_state_query_reads_back_the_same_floats() {
        let s = LayerState { model: 2, time: 0.1, row: vec![0.1, 1e-7, -3.25], chain: vec![], fondo_alpha: true };
        let q = s.query();
        let get = |k: &str| {
            q.trim_start_matches('&').split('&').find_map(|kv| kv.strip_prefix(&format!("{k}=")).map(str::to_string))
        };
        assert_eq!(LayerState::from_query(get), Some(s));
        assert_eq!(LayerState::from_query(|_| None), None, "no state keys, no state");
    }

    /// One-shot HTTP server on an ephemeral port: records the request path,
    /// answers `body` with `status`.
    fn fake_dash(body: &'static [u8], status: u16) -> (u16, std::sync::mpsc::Receiver<String>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
            tx.send(line.split_whitespace().nth(1).unwrap_or_default().to_string()).unwrap();
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        (port, rx)
    }

    #[test]
    fn a_peer_is_direct_when_a_direct_path_is_in_use_and_relay_otherwise() {
        // (is_relay, is_active)
        assert_eq!(via_of([(true, true), (false, true)]), Some("direct"));
        assert_eq!(via_of([(true, true), (false, false)]), Some("relay"), "an idle direct path is not the route");
        assert_eq!(via_of([(true, true)]), Some("relay"));
        assert_eq!(via_of([(false, false)]), None, "no active path: nothing to report");
        assert_eq!(via_of([]), None);
    }

    #[test]
    fn an_outputs_ask_returns_only_the_dash_outputs_array() {
        use std::io::{Read, Write};
        // A fake metal-dash on an ephemeral port: its GET /state, nothing more.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let body = r#"{"node":"pi","secret":1,"outputs":[{"id":"wall","node":"pi"}]}"#;
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let f = answer_outputs_locally(port);
        assert_eq!(f.status, 200);
        let v: Value = serde_json::from_slice(&f.body).unwrap();
        assert_eq!(v, json!({ "outputs": [{ "id": "wall", "node": "pi" }] }));
        // Nothing listening: a 502, not a guess.
        let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(answer_outputs_locally(dead).status, 502);
    }

    #[test]
    fn the_wire_round_trips_status_and_bytes() {
        let f = Frame { status: 409, body: vec![1, 2, 3] };
        assert_eq!(Frame::decode(&f.encode()), Some(f));
        assert_eq!(Frame::decode(&[1]), None);
        let ask = FrameAsk::Device("wall".into());
        assert_eq!(FrameAsk::from_json(&ask.to_json()), Some(ask));
    }
}
