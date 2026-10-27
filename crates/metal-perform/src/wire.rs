//! S.AL.1 — the first `out.*` node with a real network backend: `out.sacn`,
//! E1.31 (sACN) DMX over UDP.
//!
//! [`crate::fixture::drive_head`] already turns a device's head and its
//! `sig_*` wire into the DMX slots to send; nothing before this module ever
//! opened a socket and sent them. This is that socket, hand-built the same
//! way `crate::script` hand-rolls its own subprocess wire protocol — no new
//! crate dependency, just the bytes ANSI E1.31-2016 defines.
//!
//! **What this module owns and what it does not.** [`Wire`]/[`Sacn`] know
//! nothing about a graph, a device or a frame — they take a universe and a
//! row of `(slot, byte)` pairs and put them on the wire. [`drive_frame`] is
//! the per-frame walk that owns the graph reading: which `out.sacn` nodes
//! exist, which one's `signal` input is wired to what, whether its config
//! changed since the last frame. `engine::render_device` calls it once per
//! device per frame, after the picture is composed — sACN is a side effect
//! of the frame, not a step that produces one.
//!
//! **Never blocks the frame.** The socket is non-blocking
//! ([`Sacn::new`]); [`Wire::send`] never panics and never propagates an
//! error — it reports success as a `bool` so [`drive_frame`] can count a
//! `false` into `State::wire_errors` instead of stopping the render loop.
//! This is the one deliberate departure from the trait's original single-line
//! sketch (`fn send(&mut self, ...)`, no return): with no way back from
//! `send` a caller cannot tell "sent" from "dropped", and rule 3 ("never
//! block the frame" ) says a dropped packet must be *counted*, not lost
//! silently or turned into a panic — so `send` reports what happened and
//! stays exactly as infallible as before (`bool`, never `Result`, never
//! `?`).
//!
//! **Keep-alive.** §6.7.1 of the spec: a sACN receiver extinguishes its last
//! frame if it stops hearing from a source, so an unchanged signal is still
//! re-sent at least once every [`KEEPALIVE`].
//!
//! # Frame-driven fixtures and the second wire (2026-09-25)
//!
//! [`drive_frame`] drives a device that *is* a fixture (`Device.head`) from a
//! `sig_*` row wired into `signal`. A venue's lights are the other case: a
//! wash, a spot or an LED strip on `drive = "frame"` takes its colour from a
//! projector's picture, where its beam (or each LED) lands. Nothing is cabled
//! for that. The `out.*` node sits in the projector's own device graph, its
//! config says where to look (`heads[].sample` on `out.sacn`, `sample` on
//! `out.serial`, written by `POST /venue {op:"project"}` from
//! `Venue::fixture_points`), and [`drive_device`] — called by
//! `engine::render_device` with the frame it just composed — samples it
//! (`venue::sample_rgb`), turns a light's colour into its `level`/`hue`/`sat`
//! (`venue::light_floats`), packs it through the head's profile
//! (`Fixture::frame`, one packet per universe) and sends it. Config is a
//! reading of the graph (rule 4): what a light samples, which profile it is
//! and where it sits on the universe are node config, edited by the same
//! `POST /graph/node {op:"config"}` as any other node.
//!
//! [`Adalight`] is the second `impl Wire`: USB-serial LED strips
//! (`out.serial`), the `"Ada"` header then RGB triples, over a tty set up
//! raw with termios through `libc` — measured against the `serialport`
//! crate before choosing (see the `Adalight` note).

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::Path;
use web_time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use metal_graph::NodeId;
use metal_schema::Signal;

use crate::fixture::{self, FixtureConfig, Head};
use crate::dmx_bus::{allocate_addresses, bus_source, DmxBus, HeadPlacement};

/// E1.31's own default port (ANSI E1.31-2016 §5).
const SACN_PORT: u16 = 5568;

/// §6.7.1: a source must send a universe at least this often even when
/// nothing has changed, or a receiver may extinguish its last frame.
const KEEPALIVE: Duration = Duration::from_millis(800);

const ACN_PACKET_IDENTIFIER: [u8; 12] = *b"ASC-E1.17\0\0\0";
const VECTOR_ROOT_E131_DATA: u32 = 0x0000_0004;
const VECTOR_E131_DATA_PACKET: u32 = 0x0000_0002;
const VECTOR_DMP_SET_PROPERTY: u8 = 0x02;
const DMP_ADDRESS_AND_DATA_TYPE: u8 = 0xa1;
const SOURCE_NAME: &str = "metalvj";
const DEFAULT_PRIORITY: u8 = 100;

/// One universe's worth of DMX, 512 slots, 1-based addresses.
pub(crate) const DMX_SLOTS: usize = 512;

// ---------------------------------------------------------------------------
// the wire
// ---------------------------------------------------------------------------

/// One backend an `out.*` node can send its frame through. `out.sacn`'s is
/// [`Sacn`], `out.serial`'s is [`Adalight`]; the other `out.*` kinds
/// (`metal_graph`'s `OUTPUT_SIGNAL_KINDS`) have none yet.
pub trait Wire {
    /// Sends one universe's worth of `(1-based absolute address, byte)`
    /// pairs. Never panics, never blocks: `false` means the packet did not
    /// go out (a non-blocking socket that would have blocked, a routing
    /// error, anything else the OS reports) and the caller counts it rather
    /// than treating it as a hard failure — a dropped DMX frame is a glitch
    /// on the fixture, not a reason to stop rendering.
    fn send(&mut self, universe: u16, slots: &[(u16, u8)]) -> bool;

    /// Whether the wire is gone for good (a USB strip unplugged mid-set) and
    /// should be reopened rather than written again. A socket never is.
    fn broken(&self) -> bool {
        false
    }
}

/// E1.31 (sACN) over UDP. One instance per `out.sacn` node, kept alive
/// across frames by [`drive_frame`]'s cache rather than rebuilt every call —
/// opening a socket and picking a CID once per node, not once per frame.
pub struct Sacn {
    socket: UdpSocket,
    sequence: u8,
    cid: [u8; 16],
    /// `Some` pins every send at one address (a node's `unicast` config);
    /// `None` means multicast, derived fresh from whatever universe `send`
    /// is asked to carry (`239.255.<hi>.<lo>`, per the spec).
    unicast: Option<SocketAddr>,
}

impl Sacn {
    /// Binds an ephemeral UDP socket and sets it non-blocking — see the
    /// module doc's "never blocks the frame". `unicast` is the destination
    /// this instance always sends to, when the node names one.
    pub fn new(unicast: Option<SocketAddr>) -> std::io::Result<Sacn> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;
        Ok(Sacn { socket, sequence: 0, cid: random_cid(), unicast })
    }
}

impl Wire for Sacn {
    fn send(&mut self, universe: u16, slots: &[(u16, u8)]) -> bool {
        self.sequence = self.sequence.wrapping_add(1);
        let packet = build_packet(&self.cid, self.sequence, universe, slots);
        let dest = self.unicast.unwrap_or_else(|| multicast_addr(universe));
        matches!(self.socket.send_to(&packet, dest), Ok(n) if n == packet.len())
    }
}

/// The multicast group E1.31 §9.3.1 assigns a universe: `239.255.<hi>.<lo>`,
/// the universe's own two bytes.
fn multicast_addr(universe: u16) -> SocketAddr {
    let [hi, lo] = universe.to_be_bytes();
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(239, 255, hi, lo)), SACN_PORT)
}

/// A CID good enough to tell one `metalvj` source from another on the
/// wire (E1.31 requires one, stable for the life of the source) but not
/// meant to be cryptographically anything — hashed off the clock, the pid
/// and a per-process counter rather than pulling in a `rand` dependency this
/// crate otherwise has no use for.
fn random_cid() -> [u8; 16] {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = (
        web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        crate::atomic::pid_tag(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    );
    let mut cid = [0u8; 16];
    let mut a = DefaultHasher::new();
    seed.hash(&mut a);
    cid[0..8].copy_from_slice(&a.finish().to_be_bytes());
    let mut b = DefaultHasher::new();
    (seed, 0xACE_u32).hash(&mut b);
    cid[8..16].copy_from_slice(&b.finish().to_be_bytes());
    cid
}

/// Hand-builds one E1.31 data packet: root ACN layer, framing layer, DMP
/// layer, then the 513-byte DMX property block (a start code byte the spec
/// requires, then [`DMX_SLOTS`] channel bytes). `slots` outside `1..=512`
/// are dropped — nothing this crate produces should ever ask for one, but a
/// stray address must not corrupt the packet.
///
/// Always the full 638 bytes (126 of fixed header — root + framing + the
/// DMP header and its start code — plus the 512-slot payload), never a
/// truncated universe: most sACN receivers assume a full frame, and sending
/// fewer than 512 slots would leave a channel's last-known value from an
/// earlier, larger packet stuck rather than zeroed.
fn build_packet(cid: &[u8; 16], sequence: u8, universe: u16, slots: &[(u16, u8)]) -> Vec<u8> {
    let mut dmx = [0u8; DMX_SLOTS];
    for &(address, byte) in slots {
        if (1..=DMX_SLOTS as u16).contains(&address) {
            dmx[(address - 1) as usize] = byte;
        }
    }

    // Root layer: 38 bytes before the framing layer starts.
    const ROOT_LEN: usize = 38;
    // Framing layer: 77 bytes before the DMP layer starts.
    const FRAMING_LEN: usize = 77;
    // DMP layer header (before its 513-byte property block): 10 bytes.
    const DMP_HEADER_LEN: usize = 10;
    const PROPERTY_VALUES_LEN: usize = 1 + DMX_SLOTS; // start code + 512 slots
    const TOTAL_LEN: usize = ROOT_LEN + FRAMING_LEN + DMP_HEADER_LEN + PROPERTY_VALUES_LEN;

    let mut pkt = Vec::with_capacity(TOTAL_LEN);

    // --- Root Layer (RLP) ---
    pkt.extend_from_slice(&0x0010u16.to_be_bytes()); // preamble size
    pkt.extend_from_slice(&0x0000u16.to_be_bytes()); // post-amble size
    pkt.extend_from_slice(&ACN_PACKET_IDENTIFIER);
    let root_pdu_len = (TOTAL_LEN - 16) as u16; // from this field to the end
    pkt.extend_from_slice(&(0x7000 | root_pdu_len).to_be_bytes());
    pkt.extend_from_slice(&VECTOR_ROOT_E131_DATA.to_be_bytes());
    pkt.extend_from_slice(cid);

    // --- Framing Layer ---
    let framing_pdu_len = (TOTAL_LEN - ROOT_LEN) as u16;
    pkt.extend_from_slice(&(0x7000 | framing_pdu_len).to_be_bytes());
    pkt.extend_from_slice(&VECTOR_E131_DATA_PACKET.to_be_bytes());
    let mut name = [0u8; 64];
    let name_bytes = SOURCE_NAME.as_bytes();
    name[..name_bytes.len()].copy_from_slice(name_bytes);
    pkt.extend_from_slice(&name);
    pkt.push(DEFAULT_PRIORITY);
    pkt.extend_from_slice(&0u16.to_be_bytes()); // synchronization address: none
    pkt.push(sequence);
    pkt.push(0); // options: no preview, no stream terminate, no force sync
    pkt.extend_from_slice(&universe.to_be_bytes());

    // --- DMP Layer ---
    let dmp_pdu_len = (TOTAL_LEN - ROOT_LEN - FRAMING_LEN) as u16;
    pkt.extend_from_slice(&(0x7000 | dmp_pdu_len).to_be_bytes());
    pkt.push(VECTOR_DMP_SET_PROPERTY);
    pkt.push(DMP_ADDRESS_AND_DATA_TYPE);
    pkt.extend_from_slice(&0u16.to_be_bytes()); // first property address
    pkt.extend_from_slice(&1u16.to_be_bytes()); // address increment
    pkt.extend_from_slice(&(PROPERTY_VALUES_LEN as u16).to_be_bytes());
    pkt.push(0); // DMX start code: 0 = "dimmer data"
    pkt.extend_from_slice(&dmx);

    debug_assert_eq!(pkt.len(), TOTAL_LEN);
    pkt
}

// ---------------------------------------------------------------------------
// config — read live off `Node.config`, the same convention
// `FixtureConfig::from_node` follows
// ---------------------------------------------------------------------------

/// `out.sacn`'s own config: `{"universe": 1, "unicast": "host[:port]"}`.
/// `unicast` omitted (or unparseable) means multicast, `239.255.<hi>.<lo>`
/// derived from `universe`. A `unicast` string with no `:port` gets the
/// spec's own default port ([`SACN_PORT`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SacnConfig {
    #[serde(default = "default_universe")]
    pub universe: u16,
    #[serde(default)]
    pub unicast: Option<String>,
}

fn default_universe() -> u16 {
    1
}

impl Default for SacnConfig {
    fn default() -> SacnConfig {
        SacnConfig { universe: default_universe(), unicast: None }
    }
}

impl SacnConfig {
    /// Reads a node's free-form `config` as a `SacnConfig`, the same
    /// "anything that does not parse is the default" fallback
    /// [`crate::fixture::FixtureConfig::from_node`] takes — a node with no
    /// config yet, or an unrelated kind's own shape, is universe 1,
    /// multicast, rather than a hard error.
    pub fn from_node(node: &metal_graph::Node) -> SacnConfig {
        if node.config.is_empty() {
            return SacnConfig::default();
        }
        serde_json::from_str(&node.config.to_json_str()).unwrap_or_default()
    }

    /// The address `send` should target, or `None` for multicast (resolved
    /// fresh from the universe every send — see [`Sacn::unicast`]).
    fn unicast_addr(&self) -> Option<SocketAddr> {
        let raw = self.unicast.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        if let Ok(addr) = raw.parse::<SocketAddr>() {
            return Some(addr);
        }
        format!("{raw}:{SACN_PORT}").parse().ok()
    }
}

// ---------------------------------------------------------------------------
// the per-frame cache — one live socket per node, never rebuilt unless the
// node's own config changes (regla 4: config is read live, but the socket
// it produced is cheap to keep as long as that reading has not changed)
// ---------------------------------------------------------------------------

/// What [`State`](crate::engine::State) keeps for one `out.sacn` node
/// between frames: the open [`Wire`], the config it was built from (so a
/// changed universe/unicast is detected and the socket rebuilt rather than
/// silently kept stale), and the keep-alive bookkeeping.
pub struct WireSocket {
    wire: Box<dyn Wire + Send>,
    /// What the wire was opened from (`kind` + config): a change reopens it.
    opened: String,
    /// The universe (sACN) or 15-bit Port-Address (Art-Net) it sends on.
    universe: u16,
    /// The config fields the properties panel shows beside "last sent".
    shown: serde_json::Value,
    last_slots: Option<Vec<(u16, u8)>>,
    last_sent_at: Option<Instant>,
}

impl WireSocket {
    fn new(cfg: SacnConfig) -> std::io::Result<WireSocket> {
        let sacn = Sacn::new(cfg.unicast_addr())?;
        Ok(WireSocket {
            wire: Box::new(sacn),
            opened: format!("sacn {cfg:?}"),
            universe: cfg.universe,
            shown: json!({ "universe": cfg.universe, "unicast": cfg.unicast }),
            last_slots: None,
            last_sent_at: None,
        })
    }

    /// P-D: an `out.artnet` node's socket.
    fn artnet(cfg: crate::artnet::ArtNetConfig) -> std::io::Result<WireSocket> {
        let wire = crate::artnet::ArtNet::new(&cfg)?;
        Ok(WireSocket {
            wire: Box::new(wire),
            opened: format!("artnet {cfg:?}"),
            universe: cfg.port_address(),
            shown: json!({
                "universe": cfg.port_address(),
                "unicast": cfg.unicast,
                "destination": cfg.destination().to_string(),
            }),
            last_slots: None,
            last_sent_at: None,
        })
    }

    /// The socket a network DMX node (`out.sacn`/`out.artnet`) asks for,
    /// and the key it is opened from.
    fn opened_for(node: &metal_graph::Node) -> String {
        if node.kind == crate::artnet::ARTNET_KIND {
            format!("artnet {:?}", crate::artnet::ArtNetConfig::from_node(node))
        } else {
            format!("sacn {:?}", SacnConfig::from_node(node))
        }
    }

    fn open_for(node: &metal_graph::Node) -> std::io::Result<WireSocket> {
        if node.kind == crate::artnet::ARTNET_KIND {
            WireSocket::artnet(crate::artnet::ArtNetConfig::from_node(node))
        } else {
            WireSocket::new(SacnConfig::from_node(node))
        }
    }

    /// A JSON snapshot of what this node is doing right now, for the
    /// properties panel's "last sent N ms ago" — built fresh from live
    /// fields on every call, never stored as its own second copy.
    fn status_json(&self, now: Instant) -> serde_json::Value {
        let mut v = self.shown.clone();
        v["last_sent_ms_ago"] = json!(self.last_sent_at.map(|t| now.duration_since(t).as_millis() as u64));
        v
    }
}

/// Whether `kind` is a network DMX node — sACN or Art-Net — driven by
/// [`drive_frame`] (signal row) or [`drive_sampled`] (frame heads).
pub fn is_net_dmx(kind: &str) -> bool {
    kind == "out.sacn" || kind == crate::artnet::ARTNET_KIND
}

/// The live wire status of one node, for the properties panel
/// (`crate::wire`'s one reader outside this module and its own tests):
/// `None` when nothing has ever been cached for that id (never sent, or not
/// an `out.sacn` node) — the caller reads this fresh on every request, per
/// regla 4, rather than keeping any copy of its own.
pub fn status_json(sockets: &HashMap<NodeId, WireSocket>, node_id: &str) -> Option<serde_json::Value> {
    sockets.get(node_id).map(|s| s.status_json(Instant::now()))
}

// ---------------------------------------------------------------------------
// Bluetooth — `out.bluetooth`, one `metal_ble::Link` per node (BLE Phase B)
// ---------------------------------------------------------------------------

/// `out.bluetooth`'s own config: which fixture profile it drives and how
/// `metal-ble` reaches it.
///
/// `{"profile": "godox-sl60iid", "address": "AA:BB:…", "node": 3,
/// "mesh": "mesh.json"}`. `address` is the MAC the link connects to (`""`
/// means the first proxy it finds); `node` is the mesh unicast address when
/// the lamp is behind a proxy; `mesh` is the mesh-state path, **relative to
/// the data root** unless absolute. `profile` names the file under
/// `data/fixtures/` whose [`crate::fixture::Fixture::light_state`] turns the
/// row's `sig_*` floats into the [`metal_ble::LightState`] pushed on the link.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BluetoothConfig {
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub node: Option<u16>,
    #[serde(default)]
    pub mesh: String,
}

impl BluetoothConfig {
    /// The same "anything that does not parse is the default" reading
    /// [`SacnConfig::from_node`] takes.
    pub fn from_node(node: &metal_graph::Node) -> BluetoothConfig {
        if node.config.is_empty() {
            return BluetoothConfig::default();
        }
        serde_json::from_str(&node.config.to_json_str()).unwrap_or_default()
    }

    /// The [`metal_ble::LinkConfig`] this node asks for, with `mesh` resolved
    /// against the data root: a relative path is a path inside `data/`, an
    /// absolute one is left alone.
    fn link_config(&self, root: &Path) -> metal_ble::LinkConfig {
        let mesh = if self.mesh.is_empty() {
            Path::new(&self.mesh).to_path_buf()
        } else {
            let p = Path::new(&self.mesh);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                root.join(p)
            }
        };
        metal_ble::LinkConfig {
            protocol: metal_ble::Protocol::GodoxMesh,
            address: self.address.clone(),
            node: self.node,
            mesh,
        }
    }
}

/// One `out.bluetooth` node's live link, kept across frames by
/// [`drive_frame`]'s cache. The config it was built from is kept too, so a
/// changed profile/address/mesh (an operator edit, rule 4) reopens the link
/// rather than pushing through a stale connection.
pub struct BluetoothLink {
    link: Box<dyn metal_ble::Link>,
    cfg: BluetoothConfig,
}

/// How a [`BluetoothLink`] is opened — [`metal_ble::open`] in the engine, a
/// `metal_ble::Recorder` in tests. A boxed trait object rather than a plain
/// `fn` so a test can close over the log it wants to read back.
pub type LinkFactory = dyn Fn(metal_ble::LinkConfig) -> Box<dyn metal_ble::Link>;

/// Opens one Bluetooth link per `out.bluetooth` node (or reopens it when its
/// config changed), reads the node's `signal` row, turns it into a
/// [`metal_ble::LightState`] through the profile, and pushes it. A `false` from
/// [`metal_ble::Link::push`] is counted, exactly the way a dropped sACN packet is
/// — a link that is still connecting is not a reason to stop the frame.
///
/// A node whose profile or row is missing is skipped, never a crash (the rule
/// at the top of [`crate::fixture`]). Nodes that disappeared from the graph
/// drop their link alongside the sACN sockets.
pub fn drive_bluetooth(
    graph: &metal_graph::Graph,
    root: &Path,
    rows: &HashMap<NodeId, Vec<f32>>,
    script_rows: &HashMap<NodeId, Vec<f32>>,
    raw_digest: &[f32],
    links: &mut HashMap<NodeId, BluetoothLink>,
    factory: &LinkFactory,
) -> u64 {
    let live: HashSet<&str> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == "out.bluetooth")
        .map(|n| n.id.as_str())
        .collect();
    links.retain(|id, _| live.contains(id.as_str()));

    let mut errors = 0u64;
    let needed = metal_schema::EFFECTS_WIDTH + metal_schema::SIGNAL_WIDTH;

    for node in graph.nodes.iter().filter(|n| n.kind == "out.bluetooth" && !n.bypass) {
        let cfg = BluetoothConfig::from_node(node);

        // A node with no profile named — or one that names a profile not on
        // disk — can never be turned into a light state; skip it *before*
        // opening a link it would only ever push garbage through, and drop
        // any link a now-invalid config left behind.
        if cfg.profile.is_empty() || fixture::get(root, &cfg.profile).is_none() {
            links.remove(&node.id);
            continue;
        }

        let needs_new = match links.get(&node.id) {
            Some(existing) => existing.cfg != cfg,
            None => true,
        };
        if needs_new {
            links.insert(
                node.id.clone(),
                BluetoothLink { link: factory(cfg.link_config(root)), cfg: cfg.clone() },
            );
        }
        // unwrap: just inserted above, or already present and matching.
        let link = links.get_mut(&node.id).unwrap();

        let Some(profile) = fixture::get(root, &cfg.profile) else {
            continue;
        };
        let Some(row) = row_for_port(graph, &node.id, "signal", rows, script_rows, raw_digest)
        else {
            continue;
        };
        if row.len() < needed {
            continue;
        }
        let signal = metal_schema::read_signal(&row);
        let state = profile.light_state(&signal);
        if !link.link.push(&state) {
            errors += 1;
        }
    }
    errors
}

/// The row wired into `node_id`'s `port` input, mirroring exactly the
/// source-lookup [`crate::script::evaluate`] uses for a script's `control`
/// in-edge: a `gen.*` node's `row` port is that generator's driven row
/// (`rows`), another script's output is `script_rows`, anything else is the
/// frame's raw digest. `None` when nothing is wired to `port` at all — an
/// `out.sacn` node with no source cabled in has nothing to send, the same
/// way a script with no `control` edge runs on an empty row rather than a
/// borrowed one.
fn row_for_port<'a>(
    graph: &metal_graph::Graph,
    node_id: &str,
    port: &str,
    rows: &'a HashMap<NodeId, Vec<f32>>,
    script_rows: &'a HashMap<NodeId, Vec<f32>>,
    raw_digest: &'a [f32],
) -> Option<Vec<f32>> {
    let edge = graph.edges.iter().find(|e| e.to.node == node_id && e.to.port == port)?;
    let from = graph.node(&edge.from.node);
    let is_gen_row =
        from.map(|n| n.kind.starts_with("gen.")).unwrap_or(false) && edge.from.port == "row";
    Some(if is_gen_row {
        rows.get(&edge.from.node).cloned().unwrap_or_default()
    } else if let Some(row) = script_rows.get(&edge.from.node) {
        row.clone()
    } else {
        raw_digest.to_vec()
    })
}

/// The engine's per-frame signal-wire hook: every `out.sacn` node in `graph`
/// that is not bypassed sends this frame's slots for `head`, every
/// `out.bluetooth` node pushes this frame's [`metal_ble::LightState`], and every
/// node `sockets`/`links` remember that no longer exists in `graph` is
/// dropped. Returns how many sends failed this call, for the caller to fold
/// into `State::wire_errors` — see the module doc's note on why `Wire::send`
/// returns `bool` rather than nothing.
///
/// Called once per device per frame by `engine::render_device`, after the
/// frame is composed — `root`/`cover`/`rows`/`script_rows`/`raw_digest` are
/// exactly what that call already has in hand for this frame. `links` and
/// `factory` are the Bluetooth half: the engine passes [`metal_ble::open`], a
/// test passes a `metal_ble::Recorder` factory (see [`LinkFactory`]).
#[allow(clippy::too_many_arguments)]
pub fn drive_frame(
    graph: &metal_graph::Graph,
    head: &Head,
    root: &Path,
    cover: Option<[u8; 3]>,
    rows: &HashMap<NodeId, Vec<f32>>,
    script_rows: &HashMap<NodeId, Vec<f32>>,
    raw_digest: &[f32],
    sockets: &mut HashMap<NodeId, WireSocket>,
    links: &mut HashMap<NodeId, BluetoothLink>,
    factory: &LinkFactory,
    now: Instant,
) -> u64 {
    // Bluetooth is its own walk over the same frame's rows; fold its failures
    // into the same count the sACN sends report.
    let mut errors = drive_bluetooth(graph, root, rows, script_rows, raw_digest, links, factory);

    // A node that disappeared from the graph (removed in the canvas, or the
    // whole device rewired) drops its socket rather than leaking it forever.
    // A frame-driven node (`heads` sampled from the frame) belongs to
    // [`drive_sampled`], never to this signal-row walk.
    let live: HashSet<&str> = graph
        .nodes
        .iter()
        .filter(|n| is_net_dmx(&n.kind) && !frame_driven(n))
        .map(|n| n.id.as_str())
        .collect();
    sockets.retain(|id, _| live.contains(id.as_str()));

    let needed = metal_schema::EFFECTS_WIDTH + metal_schema::SIGNAL_WIDTH;

    for node in graph
        .nodes
        .iter()
        .filter(|n| is_net_dmx(&n.kind) && !n.bypass && !frame_driven(n))
    {
        let opened = WireSocket::opened_for(node);
        let needs_new = match sockets.get(&node.id) {
            Some(existing) => existing.opened != opened,
            None => true,
        };
        if needs_new {
            match WireSocket::open_for(node) {
                Ok(socket) => {
                    sockets.insert(node.id.clone(), socket);
                }
                Err(_) => {
                    errors += 1;
                    continue;
                }
            }
        }
        // unwrap: just inserted above, or already present and matching.
        let socket = sockets.get_mut(&node.id).unwrap();

        let Some(row) = row_for_port(graph, &node.id, "signal", rows, script_rows, raw_digest)
        else {
            continue;
        };
        if row.len() < needed {
            continue;
        }
        let signal = metal_schema::read_signal(&row);
        let Some(slots) = fixture::drive_head(root, head, Some(graph), cover, &signal) else {
            continue;
        };

        let changed = socket.last_slots.as_deref() != Some(slots.as_slice());
        let stale = socket
            .last_sent_at
            .map(|t| now.duration_since(t) >= KEEPALIVE)
            .unwrap_or(true);
        if changed || stale {
            if !socket.wire.send(socket.universe, &slots) {
                errors += 1;
            }
            socket.last_slots = Some(slots);
            socket.last_sent_at = Some(now);
        }
    }
    errors
}

// ---------------------------------------------------------------------------
// frame-driven fixtures — `out.*` nodes that sample the frame of the device
// whose graph they sit in (see the module doc)
// ---------------------------------------------------------------------------

/// Whether `node` takes its colour from its own device's frame — what
/// [`drive_sampled`] drives and [`drive_frame`] leaves alone: an `out.sacn`
/// carrying `heads`, and every `out.serial`.
pub fn frame_driven(node: &metal_graph::Node) -> bool {
    match node.kind.as_str() {
        "out.sacn" | "out.artnet" => node
            .config
            .get("heads")
            .and_then(|h| h.as_array())
            .is_some_and(|h| !h.is_empty()),
        "out.serial" => true,
        _ => false,
    }
}

/// One DMX head an `out.sacn` node carries on its universe, coloured by its
/// device's frame where its light lands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampledHead {
    /// What the head is to whoever wrote it (`bar.wash-1` for a venue's).
    #[serde(default)]
    pub id: String,
    /// The profile under `data/fixtures/`, as [`Head::fixture`].
    pub fixture: String,
    /// Its 1-based start address; `None` is the profile's own.
    #[serde(default)]
    pub address: Option<u16>,
    /// Where its light lands in the frame, `0..1`. `None` — no projected
    /// light reaches it — is black.
    #[serde(default)]
    pub sample: Option<[f32; 2]>,
}

impl SampledHead {
    /// The [`Head`] its profile packs. No universe of its own: the node's
    /// is the one it goes out on.
    pub fn head(&self) -> Head {
        Head { fixture: self.fixture.clone(), universe: None, address: self.address }
    }
}

/// The frame-driven half of an `out.sacn` node's config, beside its
/// [`SacnConfig`]: `{"universe": 1, "unicast": "…", "heads": [{"id",
/// "fixture", "address", "sample"}]}`. One node is one universe, so several
/// heads on one DMX line go out as one packet a frame — never as packets
/// that zero each other's slots.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SampledSacn {
    #[serde(default)]
    pub heads: Vec<SampledHead>,
}

impl SampledSacn {
    /// The same "anything that does not parse is the default" reading
    /// [`SacnConfig::from_node`] takes.
    pub fn from_node(node: &metal_graph::Node) -> SampledSacn {
        if node.config.is_empty() {
            return SampledSacn::default();
        }
        serde_json::from_str(&node.config.to_json_str()).unwrap_or_default()
    }
}

/// A light's signal from the colour its beam lands on: `level`/`hue`/`sat`
/// ([`crate::venue::light_floats`]) plus the white channels a non-RGB fixture
/// reads — the same [`fixture::colour_signal`] a `model` source uses (F5), so
/// a warm/cool white or a temperature lamp takes the sampled colour too and no
/// beam shaping is authored from a colour. No strobe, and pan/tilt at the
/// middle of their travel — a venue aims a light by where it hangs, not by
/// moving it.
pub fn frame_signal(rgb: [f32; 3]) -> Signal {
    fixture::colour_signal(rgb, None, "free")
}

/// Every head's slots for this frame, by address: sampled
/// ([`crate::venue::sample_rgb`]), turned into a signal, packed through its
/// profile ([`fixture::Fixture::frame`]), with the graph's `forced` colour
/// (blackout) over the frame's. A head whose profile is not under
/// `data/fixtures/` is skipped, never a crash.
fn head_slots(
    heads: &[SampledHead],
    profiles: &[fixture::Fixture],
    forced: Option<[f32; 3]>,
    frame: &[u8],
    w: u32,
    h: u32,
) -> Vec<(u16, u8)> {
    let force = FixtureConfig { forced, patch: PatchMode::Colour };
    let mut slots: Vec<(u16, u8)> = Vec::new();
    for head in heads {
        let Some(profile) = profiles.iter().find(|p| p.model == head.fixture) else {
            continue;
        };
        let signal = frame_signal(crate::venue::sample_rgb(frame, w, h, head.sample));
        // M6: the profile's safety envelope clamps the state before the wire —
        // a dimmer cap, a refused strobe, a pan/tilt no-go zone.
        let mut signal = force.drive(&signal);
        profile.limits.clamp_signal(&mut signal);
        slots.extend(profile.frame(&head.head(), &signal));
    }
    slots.sort_by_key(|(address, _)| *address);
    slots
}

/// Adalight's own default line speed.
const DEFAULT_BAUD: u32 = 115_200;

/// The most LEDs one strip can carry here: three slots each on a 16-bit
/// address.
const MAX_STRIP: usize = u16::MAX as usize / 3;

fn default_baud() -> u32 {
    DEFAULT_BAUD
}

/// The byte order a strip's controller wants its colours in. `rgb` is
/// Adalight's own (the Arduino sketch reorders for its LEDs); the others are
/// for controllers that pass bytes straight to a `grb` WS2812 and the like.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorOrder {
    #[default]
    Rgb,
    Rbg,
    Grb,
    Gbr,
    Brg,
    Bgr,
}

impl ColorOrder {
    /// `[r, g, b]` in the order this strip reads them.
    pub fn arrange(self, [r, g, b]: [u8; 3]) -> [u8; 3] {
        match self {
            ColorOrder::Rgb => [r, g, b],
            ColorOrder::Rbg => [r, b, g],
            ColorOrder::Grb => [g, r, b],
            ColorOrder::Gbr => [g, b, r],
            ColorOrder::Brg => [b, r, g],
            ColorOrder::Bgr => [b, g, r],
        }
    }
}

/// `out.serial`'s config — a USB-serial LED strip speaking Adalight:
/// `{"port": "/dev/ttyUSB0", "baud": 115200, "pixels": 60, "order": "rgb",
/// "sample": [[x, y] | null, …]}`. Written by `POST /graph/node {op:
/// "config"}` like any node's, and by `POST /venue {op:"project"}` for a
/// venue strip on `wire = "usb"` (everything but `port`, which only the
/// person holding the cable knows).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SerialConfig {
    /// The serial device (`/dev/ttyUSB0`, `/dev/ttyACM0`). Empty: the node
    /// is idle — nothing is ever written to a port nobody named.
    #[serde(default)]
    pub port: String,
    /// Line speed, `115200` (Adalight's default) when absent.
    #[serde(default = "default_baud")]
    pub baud: u32,
    /// How many LEDs the strip has; `None` is one per `sample` point.
    #[serde(default)]
    pub pixels: Option<usize>,
    #[serde(default)]
    pub order: ColorOrder,
    /// Where each LED looks in the frame, `0..1`, in strip order; `null` is
    /// an LED no projected light reaches (dark). Empty: `pixels` LEDs spread
    /// across the frame's middle row — the frame sampled across the strip.
    #[serde(default)]
    pub sample: Vec<Option<[f32; 2]>>,
}

impl Default for SerialConfig {
    fn default() -> SerialConfig {
        SerialConfig {
            port: String::new(),
            baud: DEFAULT_BAUD,
            pixels: None,
            order: ColorOrder::default(),
            sample: Vec::new(),
        }
    }
}

impl SerialConfig {
    /// The same "anything that does not parse is the default" reading
    /// [`SacnConfig::from_node`] takes: an idle strip.
    pub fn from_node(node: &metal_graph::Node) -> SerialConfig {
        if node.config.is_empty() {
            return SerialConfig::default();
        }
        serde_json::from_str(&node.config.to_json_str()).unwrap_or_default()
    }

    /// How many LEDs a frame drives.
    pub fn count(&self) -> usize {
        self.pixels.unwrap_or(self.sample.len()).min(MAX_STRIP)
    }

    /// What the open line depends on: a change here reopens it, a new
    /// `sample` does not.
    fn line(&self) -> String {
        format!("serial {} {} {} {:?}", self.port.trim(), self.baud, self.count(), self.order)
    }
}

/// A strip's slots for this frame: LED `i`'s red, green and blue at
/// addresses `3i + 1 ..= 3i + 3`, in RGB — [`Adalight`] puts them in the
/// strip's own `order`. The graph's `forced` colour (blackout) wins over the
/// frame.
fn strip_slots(cfg: &SerialConfig, forced: Option<[f32; 3]>, frame: &[u8], w: u32, h: u32) -> Vec<(u16, u8)> {
    let n = cfg.count();
    let mut slots = Vec::with_capacity(n * 3);
    for i in 0..n {
        let at = if cfg.sample.is_empty() {
            Some([(i as f32 + 0.5) / n as f32, 0.5])
        } else {
            cfg.sample.get(i).copied().flatten()
        };
        let rgb = forced.unwrap_or_else(|| crate::venue::sample_rgb(frame, w, h, at));
        for (c, v) in rgb.iter().enumerate() {
            slots.push(((i * 3 + c + 1) as u16, (v.clamp(0.0, 1.0) * 255.0).round() as u8));
        }
    }
    slots
}

// ---------------------------------------------------------------------------
// the pixel-map node: one `venue.fixtures` node drives a whole venue (M3)
// ---------------------------------------------------------------------------

/// One wire's worth of a venue's frame: the slots to send and where they go.
/// Built by [`venue_packets`] from a `venue.fixtures` node's venue and group —
/// one packet per universe (sACN), per port (serial) or per profile
/// (Bluetooth), so two fixtures on one wire share a packet instead of
/// overwriting each other's slots.
pub struct VenuePacket {
    /// `"sacn"`, `"serial"` or `"bluetooth"`.
    pub wire: String,
    /// The universe (sACN), the port (serial) or the profile (Bluetooth).
    pub key: String,
    pub universe: u16,
    /// The DMX slots for an sACN or serial wire.
    pub slots: Vec<(u16, u8)>,
    /// Bluetooth: the light state to push (no DMX slots).
    pub state: Option<metal_ble::LightState>,
    /// Serial: how many LEDs the packet carries.
    pub pixels: usize,
}

/// The fixtures a selection names: the whole venue, one named group, or
/// exactly the one row a `venue.fixture` node points at (decision 1). Empty
/// when a named group or fixture is not there — an orphan drives nothing,
/// never a crash.
fn venue_selection<'v>(
    v: &'v crate::venue::Venue,
    group: Option<&str>,
    fixture: Option<&str>,
) -> Vec<&'v crate::venue::Fixture> {
    v.fixtures
        .iter()
        .filter(|f| match (group, fixture) {
            (_, Some(id)) => f.id == id,
            (Some(g), None) => f.group.as_deref() == Some(g),
            (None, None) => true,
        })
        .collect()
}

/// The `dmx`/`sacn` heads a venue's fixtures become, addressed by the **one
/// allocator** ([`crate::dmx_bus::allocate_addresses`]) — the same reading
/// `call::venue::write_fixtures` takes for a projector's `out.sacn` node, so
/// the two agree on where each head lands. Returns `(universe, head)` in
/// fixture order; a fixture whose profile is missing is skipped.
pub fn venue_heads(
    v: &crate::venue::Venue,
    group: Option<&str>,
    root: &Path,
) -> Vec<(u16, SampledHead)> {
    heads_of(v, &venue_selection(v, group, None), root)
}

/// [`venue_heads`] restricted to one row — what a `venue.fixture` node drives
/// (F1/F2). Empty when the row is gone.
pub fn fixture_heads(
    v: &crate::venue::Venue,
    fixture_id: &str,
    root: &Path,
) -> Vec<(u16, SampledHead)> {
    heads_of(v, &venue_selection(v, None, Some(fixture_id)), root)
}

fn heads_of(
    v: &crate::venue::Venue,
    fixtures: &[&crate::venue::Fixture],
    root: &Path,
) -> Vec<(u16, SampledHead)> {
    use crate::venue::FixtureKind;

    let profiles = fixture::load(root);
    // (universe, model, sample, id, start, span), in fixture order.
    let mut placed: Vec<(u16, String, Option<[f32; 2]>, String, u16, u16)> = Vec::new();
    for &f in fixtures {
        if matches!(f.kind, FixtureKind::Camera | FixtureKind::Matrix) {
            continue;
        }
        if !matches!(f.wire.as_str(), "dmx" | "sacn") {
            continue;
        }
        let points = match v.fixture_points(f) {
            Ok(points) => points,
            Err(_) => continue,
        };
        let Some(model) = f.profile.as_deref() else { continue };
        let Some(profile) = profiles.iter().find(|p| p.model == model) else { continue };
        // D1: the fixture's own override wins over the profile's defaults.
        let universe = f.universe.unwrap_or(profile.universe);
        let address = f.address.unwrap_or(profile.address);
        let span = profile
            .channels
            .iter()
            .flat_map(|c| c.coarse.into_iter().chain(c.fine))
            .max()
            .map(|last| last + 1)
            .unwrap_or(1);
        placed.push((
            universe,
            model.to_string(),
            points.first().copied().flatten(),
            f.id.clone(),
            address,
            span,
        ));
    }
    let placements: Vec<HeadPlacement> = placed
        .iter()
        .map(|(universe, _, _, id, address, span)| HeadPlacement {
            id: id.clone(),
            universe: *universe,
            address: *address,
            span: *span,
        })
        .collect();
    let addresses = allocate_addresses(&placements, |_| None);
    placed
        .into_iter()
        .zip(addresses)
        .map(|((universe, fixture, sample, id, _, _), address)| {
            (universe, SampledHead { id, fixture, address: Some(address), sample })
        })
        .collect()
}

/// The packets a `venue.fixtures` node sends this frame: the frame is sampled
/// at every fixture footprint of the venue (or the named `group`), each
/// fixture is packed through its profile, and the fixtures are grouped by wire
/// (M3). A fixture whose profile is missing is skipped, never a crash — the
/// rule at the top of [`crate::fixture`].
pub fn venue_packets(
    v: &crate::venue::Venue,
    group: Option<&str>,
    frame: &[u8],
    w: u32,
    h: u32,
    root: &Path,
) -> Vec<VenuePacket> {
    packets_of(v, &venue_selection(v, group, None), None, frame, w, h, root)
}

/// [`venue_packets`] restricted to one fixture (F2): exactly the packet the
/// group node produces when that fixture is the only one it drives. Empty when
/// the row is gone — an orphan sends nothing (decision 1).
pub fn fixture_packets(
    v: &crate::venue::Venue,
    fixture_id: &str,
    frame: &[u8],
    w: u32,
    h: u32,
    root: &Path,
) -> Vec<VenuePacket> {
    packets_of(v, &venue_selection(v, None, Some(fixture_id)), None, frame, w, h, root)
}

/// The shared body of [`venue_packets`]/[`fixture_packets`]: pack a selection
/// of a venue's fixtures. `force` is the graph's blackout colour when the
/// caller has one (a `venue.fixture` node does — F3); a group node passes
/// `None` exactly as today.
fn packets_of(
    v: &crate::venue::Venue,
    fixtures: &[&crate::venue::Fixture],
    force: Option<[f32; 3]>,
    frame: &[u8],
    w: u32,
    h: u32,
    root: &Path,
) -> Vec<VenuePacket> {
    use crate::venue::FixtureKind;
    use std::collections::{BTreeMap, BTreeSet};

    let profiles = fixture::load(root);
    let mut sacn: BTreeMap<u16, Vec<SampledHead>> = BTreeMap::new();
    for (universe, head) in heads_of(v, fixtures, root) {
        sacn.entry(universe).or_default().push(head);
    }
    // universe -> slots a matrix fixture writes, already wrapped at 512.
    let mut matrix_slots: BTreeMap<u16, Vec<(u16, u8)>> = BTreeMap::new();
    // port -> (pixels, sample points), one entry per serial wire.
    let mut serial: BTreeMap<String, (usize, Vec<Option<[f32; 2]>>)> = BTreeMap::new();
    let mut bluetooth: Vec<VenuePacket> = Vec::new();

    for &f in fixtures {
        let points = match v.fixture_points(f) {
            Ok(points) => points,
            Err(_) => continue,
        };
        match (f.kind, f.wire.as_str()) {
            (FixtureKind::Camera, _) => continue,
            (FixtureKind::Matrix, "dmx" | "sacn") => {
                let Some(m) = f.matrix.as_ref() else { continue };
                let channels = m.pixel_channels();
                let (universe, address) = (f.universe.unwrap_or(0), f.address.unwrap_or(1));
                for (cell, offset) in channels.iter().enumerate() {
                    if *offset < 0 {
                        continue;
                    }
                    let rgb = crate::venue::sample_rgb(
                        frame,
                        w,
                        h,
                        points.get(cell).copied().flatten(),
                    );
                    for (c, v) in rgb.iter().enumerate() {
                        let (u, a) = m.channel_at(universe, address, offset + c as i16);
                        matrix_slots.entry(u).or_default().push((
                            a,
                            (v.clamp(0.0, 1.0) * 255.0).round() as u8,
                        ));
                    }
                }
            }
            (_, "bluetooth") => {
                let Some(model) = f.profile.as_deref() else { continue };
                let Some(profile) = profiles.iter().find(|p| p.model == model) else { continue };
                let rgb = crate::venue::sample_rgb(frame, w, h, points.first().copied().flatten());
                let state = profile.light_state(&frame_signal(rgb));
                bluetooth.push(VenuePacket {
                    wire: "bluetooth".into(),
                    key: model.to_string(),
                    universe: 0,
                    slots: Vec::new(),
                    state: Some(state),
                    pixels: 0,
                });
            }
            (FixtureKind::LedStrip, "usb" | "serial") => {
                let entry = serial.entry(f.wire.clone()).or_insert_with(|| (points.len(), Vec::new()));
                entry.0 = entry.0.max(points.len());
                entry.1 = points;
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    let universes: BTreeSet<u16> = sacn
        .keys()
        .copied()
        .chain(matrix_slots.keys().copied())
        .collect();
    for universe in universes {
        let mut slots = matrix_slots.remove(&universe).unwrap_or_default();
        if let Some(heads) = sacn.get(&universe) {
            slots.extend(head_slots(heads, &profiles, force, frame, w, h));
        }
        slots.sort_by_key(|(address, _)| *address);
        out.push(VenuePacket {
            wire: "sacn".into(),
            key: universe.to_string(),
            universe,
            slots,
            state: None,
            pixels: 0,
        });
    }
    for (key, (pixels, sample)) in serial {
        let cfg = SerialConfig {
            port: key.clone(),
            baud: DEFAULT_BAUD,
            pixels: Some(pixels),
            order: ColorOrder::default(),
            sample,
        };
        out.push(VenuePacket {
            wire: "serial".into(),
            key,
            universe: 0,
            slots: strip_slots(&cfg, force, frame, w, h),
            state: None,
            pixels,
        });
    }
    out.extend(bluetooth);
    out
}

/// Sends one [`VenuePacket`] through the wire its `wire` names — the existing
/// [`Sacn`]/[`Adalight`] backends, or a [`metal_ble::Link`] for Bluetooth. A
/// `false` means the packet did not go out, counted rather than treated as a
/// hard failure (the same contract [`Wire::send`] keeps).
pub fn send_venue_packet(
    packet: &VenuePacket,
    sacn: Option<&mut Sacn>,
    serial: Option<&mut Adalight>,
    link: Option<&mut dyn metal_ble::Link>,
) -> bool {
    match packet.wire.as_str() {
        "sacn" => sacn.is_some_and(|s| s.send(packet.universe, &packet.slots)),
        "serial" => serial.is_some_and(|s| s.send(packet.universe, &packet.slots)),
        "bluetooth" => match (link, &packet.state) {
            (Some(l), Some(state)) => l.push(state),
            _ => false,
        },
        _ => false,
    }
}

/// A `venue.fixtures` node's own config: which venue it drives and, when it
/// names one, which group of that venue's fixtures (M3). The same
/// "anything that does not parse is the default" reading
/// [`SacnConfig::from_node`] takes; an empty `venue` drives nothing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VenueNodeConfig {
    #[serde(default)]
    pub venue: String,
    #[serde(default)]
    pub group: Option<String>,
}

impl VenueNodeConfig {
    pub fn from_node(node: &metal_graph::Node) -> VenueNodeConfig {
        if node.config.is_empty() {
            return VenueNodeConfig::default();
        }
        serde_json::from_str(&node.config.to_json_str()).unwrap_or_default()
    }
}

/// A `venue.fixture` node's own config (F1/F2, decision 1): a **pointer** at
/// one row of a venue — `venue` and `fixture` name it — plus how its colour is
/// chosen (`colour_from`/`spread`/`pick`/`offset`/`hue_lock`) and how it writes
/// the bus (`priority`/`mode`, read by [`crate::dmx_bus::bus_source`], not
/// here). Never a copy of the row's address, universe, profile or pose: rename
/// or delete the row and the node follows. The same "anything that does not
/// parse is the default" reading [`VenueNodeConfig`] takes; an empty `venue`
/// or `fixture` is an orphan that drives nothing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VenueFixtureConfig {
    #[serde(default)]
    pub venue: String,
    #[serde(default)]
    pub fixture: String,
    /// `frame` (default), `model` or `signal` — decision 3.
    #[serde(default)]
    pub colour_from: Option<String>,
    /// Which channels a patch-driven node packs: `colour` (the default)
    /// or `patch` (pan/tilt and beam shaping as well).
    #[serde(default)]
    pub drive: Option<String>,
    #[serde(default)]
    pub spread: Option<String>,
    #[serde(default)]
    pub pick: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub hue_lock: Option<String>,
}

/// Which channels a patch-driven `venue.fixture` node packs from its wired row;
/// `Colour` is the historical behaviour, `Patch` the hotel-fiesta rule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PatchMode {
    #[default]
    Colour,
    Patch,
}

impl PatchMode {
    pub fn parse(word: &str) -> PatchMode {
        match word {
            "patch" => PatchMode::Patch,
            _ => PatchMode::Colour,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            PatchMode::Colour => "colour",
            PatchMode::Patch => "patch",
        }
    }
}
impl VenueFixtureConfig {
    pub fn from_node(node: &metal_graph::Node) -> VenueFixtureConfig {
        if node.config.is_empty() {
            return VenueFixtureConfig::default();
        }
        serde_json::from_str(&node.config.to_json_str()).unwrap_or_default()
    }

    /// The colour source the node names, `frame` when it names none (the
    /// default decision 3 fixes so a new node changes nothing visible).
    pub fn colour_from(&self) -> &str {
        self.colour_from.as_deref().unwrap_or("frame")
    }

    /// How a rig splits the model's four slots (decision 5), `slots` when the
    /// node names none — the spreading behaviour the plan defaults to.
    pub fn spread(&self) -> &str {
        self.spread.as_deref().unwrap_or("slots")
    }

    /// The slot a `spread = "same"` node takes, clamped to the four slots a
    /// colour slot has.
    pub fn pick(&self) -> usize {
        self.pick.unwrap_or(0).min(3)
    }

    pub fn offset(&self) -> usize {
        self.offset.unwrap_or(0)
    }

    /// Whether the fixture keeps its own base hue (decision 6), `free` when the
    /// node names none.
    pub fn hue_lock(&self) -> &str {
        self.hue_lock.as_deref().unwrap_or("free")
    }
    /// Which channels a `colour_from = "signal"` node packs.
    pub fn patch_mode(&self) -> PatchMode {
        PatchMode::parse(self.drive.as_deref().unwrap_or("colour"))
    }
}

/// The rows a `venue.fixture` node reads when its colour does not come from the
/// frame (F3 `signal`, F4 `model`): the graph's model rows, their **base** rows
/// (the operator's own colours, for `hue_lock = "hold"`), the script rows and
/// the frame's raw digest — exactly what [`row_for_port`] and the model colour
/// read-back need. A field a caller does not have is `None`, and a source that
/// asks for it then sends nothing rather than a wrong colour. [`Default`] is the
/// frame-only walk every existing caller keeps.
#[derive(Default, Clone, Copy)]
pub struct SourceRows<'a> {
    pub rows: Option<&'a HashMap<NodeId, Vec<f32>>>,
    pub base: Option<&'a HashMap<NodeId, Vec<f32>>>,
    pub script_rows: Option<&'a HashMap<NodeId, Vec<f32>>>,
    pub raw: &'a [f32],
}

impl<'a> SourceRows<'a> {
    /// [`row_for_port`] with this walk's maps — the `signal` source (F3) reads
    /// a `sig_*` row off the producer node (a model's row, a script's output or
    /// the frame's digest), the same lookup `out.*` nodes use.
    fn row_for_port(&self, graph: &metal_graph::Graph, node_id: &str, port: &str) -> Option<Vec<f32>> {
        let empty = HashMap::new();
        row_for_port(
            graph,
            node_id,
            port,
            self.rows.unwrap_or(&empty),
            self.script_rows.unwrap_or(&empty),
            self.raw,
        )
    }
}

/// The slot a fixture takes from a model's four (decision 5): with
/// `spread = "slots"` (anything but `"same"`), fixture `i` takes
/// `(i + offset) mod 4`, so a rig of washes reads as the model's four-colour
/// gradient; with `spread = "same"`, every fixture takes `pick`.
pub fn slot_for(spread: &str, pick: usize, offset: usize, i: usize) -> usize {
    if spread == "same" {
        pick % 4
    } else {
        (i + offset) % 4
    }
}

/// The row offset of `kind`'s colour slot, from the one catalog the engine
/// reads (`metal_graph::params::kind_params`, rule 4) — `None` when the model
/// declares none.
fn model_colour_at(kind: &str) -> Option<usize> {
    let manifest = crate::engine::shared_manifest();
    metal_graph::params::kind_params(kind, manifest)
        .into_iter()
        .find(|p| p.kind == metal_graph::params::ParamKind::Colour)
        .and_then(|p| p.index)
}

/// A model node's four colour slots (`[r,g,b]` x4) as its row holds them —
/// `drive::row_with`'s `Block::ColorSlot`, read back from the row the engine
/// drew with, never recomputed (F4, decision 4). `None` when the model declares
/// no colour slot or its row is not in this frame's map.
fn model_colour_slots(
    node: &metal_graph::Node,
    rows: &HashMap<NodeId, Vec<f32>>,
) -> Option<[[f32; 3]; 4]> {
    let at = model_colour_at(&node.kind)?;
    let row = rows.get(&node.id)?;
    let mut out = [[0.0f32; 3]; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        let o = at + i * 3;
        *slot = [*row.get(o)?, *row.get(o + 1)?, *row.get(o + 2)?];
    }
    Some(out)
}

/// The four colours a `venue.fixture` node's `colour` in-edge offers, and the
/// same four off the source model's **base** row (the operator's own colours,
/// for `hue_lock = "hold"`). A `gen.*` producer is read from its driven row
/// ([`model_colour_slots`]); a `color`/`palette` producer fills all four stops
/// from its own config ([`crate::bind::colour_value`]). `None` when nothing is
/// wired, the producer is neither kind, or its row is missing.
fn colour_slots_for(
    graph: &metal_graph::Graph,
    node_id: &str,
    rows: &SourceRows,
) -> Option<([[f32; 3]; 4], Option<[[f32; 3]; 4]>)> {
    let edge = graph.edges.iter().find(|e| e.to.node == node_id && e.to.port == "colour")?;
    let from = graph.node(&edge.from.node)?;
    if from.kind.starts_with("gen.") {
        let colours = model_colour_slots(from, rows.rows?)?;
        let base = rows.base.and_then(|b| model_colour_slots(from, b));
        Some((colours, base))
    } else {
        crate::bind::colour_value(graph, &edge.from).map(|c| (c, None))
    }
}

/// The `Signal` a `venue.fixture` node sends from a model's driven colour
/// (F4): the slot [`slot_for`] picks, through [`fixture::colour_signal`] with
/// `hue_lock` and the slot's own base colour.
fn model_colour_signal(
    cfg: &VenueFixtureConfig,
    i: usize,
    colours: [[f32; 3]; 4],
    base: Option<[[f32; 3]; 4]>,
) -> Signal {
    let slot = slot_for(cfg.spread(), cfg.pick(), cfg.offset(), i);
    fixture::colour_signal(colours[slot], base.map(|b| b[slot]), cfg.hue_lock())
}

/// The one packet a `venue.fixture` node sends when its colour does not come
/// from the frame (F3 `signal`, F4 `model`): the row's universe and the slots a
/// `Signal` packs through its profile, so the graph's blackout (`forced`) still
/// wins. The row's address is taken from the **whole venue's** heads
/// ([`venue_heads`]), the same D1 allocator the group node packs with, so a
/// fixture driven on its own sits exactly where the group puts it. `None` when
/// the row or its profile is gone — an orphan is a skip, never a send
/// (decision 1).
fn fixture_signal_packet(
    v: &crate::venue::Venue,
    fixture_id: &str,
    root: &Path,
    graph: &metal_graph::Graph,
    wire: &Signal,
    patch: PatchMode,
) -> Option<VenuePacket> {
    let (universe, head) = venue_heads(v, None, root).into_iter().find(|(_, h)| h.id == fixture_id)?;
    let slots = fixture::drive_head_mode(root, &head.head(), Some(graph), None, wire, patch)?;
    Some(VenuePacket { wire: "sacn".into(), key: universe.to_string(), universe, slots, state: None, pixels: 0 })
}

/// The venue half of [`drive_sampled`]: for each `venue.fixtures` (M3) and
/// `venue.fixture` (F1) node in `graph`, load the venue it names and sample
/// the frame at every fixture footprint it drives (or the named group, for the
/// group node). **D3.** Its sACN packets do not send themselves: each writes
/// its slots into `bus` (tagged with its source id, priority and mode) and
/// records its universe as touched, so a venue node and an `out.*` node on one
/// universe merge and flush together. The wires are still cached across frames
/// under a `venue/{graph}/{node}/{key}` key so a venue node and an `out.*` node
/// never share or prune each other's entries. A `venue.fixture` node whose
/// pointer names nothing, whose venue is gone or whose row is gone drives
/// nothing at all (decision 1). A serial or Bluetooth packet is skipped: this
/// walk opens no `metal_ble::Link` (the engine's [`drive_bluetooth`] owns
/// Bluetooth for `out.bluetooth` nodes), so those keep going through the
/// project-time `out.*` nodes.
fn drive_venue_nodes(
    graph: &metal_graph::Graph,
    frame: &[u8],
    w: u32,
    h: u32,
    root: &Path,
    rows: &SourceRows,
    wires: &mut HashMap<String, SampledWire>,
    bus: &mut DmxBus,
    touched: &mut Vec<(u16, String)>,
    now: Instant,
) -> u64 {
    let mut errors = 0u64;
    let mut live: HashSet<String> = HashSet::new();
    // (wire base, the node's own priority/mode, its packets).
    let mut work: Vec<(String, crate::dmx_bus::BusSource, Vec<VenuePacket>)> = Vec::new();
    // F3/F4: the graph's blackout reaches a per-fixture node (the group node
    // keeps today's behaviour and passes `None`).
    let forced = fixture::forced_on(graph);
    for node in graph.nodes.iter().filter(|n| {
        !n.bypass
            && (n.kind == metal_graph::VENUE_FIXTURES_KIND || n.kind == metal_graph::VENUE_FIXTURE_KIND)
    }) {
        let base = format!("venue/{}/{}", graph.id, node.id);
        let packets = if node.kind == metal_graph::VENUE_FIXTURE_KIND {
            let cfg = VenueFixtureConfig::from_node(node);
            // An orphan (no pointer) drives nothing; the same for a venue that
            // is gone. A pointer at a row that is gone yields no packets.
            if cfg.venue.is_empty() || cfg.fixture.is_empty() {
                continue;
            }
            let Ok(v) = crate::venue::load(root, &cfg.venue) else { continue };
            // Three colour sources, one at a time (decision 3): `frame` samples
            // the composed picture; `signal` reads a `sig_*` row off the node
            // wired into `signal`; `model` reads the driven colour slot of the
            // model wired into `colour`. The latter two pack through
            // `drive_venue_fixture_signal`, so blackout and the profile's
            // limits still apply (F3/F4).
            match cfg.colour_from() {
                "signal" => {
                    let Some(row) = rows.row_for_port(graph, &node.id, "signal") else { continue };
                    if row.len() < metal_schema::EFFECTS_WIDTH + metal_schema::SIGNAL_WIDTH {
                        continue;
                    }
                    let signal = metal_schema::read_signal(&row);
                    fixture_signal_packet(&v, &cfg.fixture, root, graph, &signal, cfg.patch_mode())
                        .into_iter()
                        .collect()
                }
                "model" => {
                    let Some((colours, base)) = colour_slots_for(graph, &node.id, rows) else {
                        continue;
                    };
                    let i = v.fixtures.iter().position(|f| f.id == cfg.fixture).unwrap_or(0);
                    let signal = model_colour_signal(&cfg, i, colours, base);
                    fixture_signal_packet(&v, &cfg.fixture, root, graph, &signal, cfg.patch_mode())
                        .into_iter()
                        .collect()
                }
                _ => packets_of(&v, &venue_selection(&v, None, Some(&cfg.fixture)), forced, frame, w, h, root),
            }
        } else {
            let cfg = VenueNodeConfig::from_node(node);
            let Ok(v) = crate::venue::load(root, &cfg.venue) else { continue };
            venue_packets(&v, cfg.group.as_deref(), frame, w, h, root)
        };
        for p in &packets {
            live.insert(format!("{base}/{}", p.key));
        }
        work.push((base, bus_source(node), packets));
    }
    let prefix = format!("venue/{}/", graph.id);
    wires.retain(|key, s| !key.starts_with(&prefix) || (live.contains(key) && !s.wire.broken()));

    for (base, src, packets) in work {
        for p in &packets {
            if p.wire != "sacn" {
                continue;
            }
            let key = format!("{base}/{}", p.key);
            let opened = format!("venue-sacn {} {}", p.universe, p.key);
            if wires.get(&key).is_none_or(|s| s.opened != opened) {
                let wire: std::io::Result<Box<dyn Wire + Send>> =
                    Sacn::new(None).map(|s| Box::new(s) as Box<dyn Wire + Send>);
                match wire {
                    Ok(wire) => {
                        wires.insert(key.clone(), SampledWire { wire, opened, last: None, last_sent_at: None });
                    }
                    Err(_) => {
                        errors += 1;
                        continue;
                    }
                }
            }
            bus.write(p.universe, &key, src.priority, src.mode, &p.slots, now);
            touched.push((p.universe, key));
        }
    }
    errors
}

/// **D3.** Resolves every touched universe once and sends it through its
/// owner's cached wire, keeping the [`KEEPALIVE`]/changed contract: an
/// unchanged, fresh universe is skipped; an unchanged but stale one is
/// re-sent. A universe is sent once however many sources wrote it.
fn flush_bus(
    bus: &DmxBus,
    wires: &mut HashMap<String, SampledWire>,
    touched: &[(u16, String)],
    now: Instant,
) -> u64 {
    let mut errors = 0u64;
    let mut seen: HashSet<u16> = HashSet::new();
    for (universe, key) in touched {
        if !seen.insert(*universe) {
            continue;
        }
        let Some(bytes) = bus.resolve(*universe, now) else { continue };
        let slots = DmxBus::sparse(&bytes);
        let Some(s) = wires.get_mut(key) else { continue };
        let changed = s.last.as_deref() != Some(slots.as_slice());
        let stale = s.last_sent_at.is_none_or(|t| now.duration_since(t) >= KEEPALIVE);
        if !(changed || stale) {
            continue;
        }
        if s.wire.send(*universe, &slots) {
            s.last = Some(slots);
            s.last_sent_at = Some(now);
        } else {
            errors += 1;
        }
    }
    errors
}

/// One Adalight packet for `pixels` LEDs: `"Ada"`, the LED count minus one
/// (high byte, low byte), those two XOR `0x55`, then one colour triple per
/// LED in `order`. `slots` are the strip's RGB bytes by 1-based address
/// ([`strip_slots`]); an LED no slot names is dark, a slot past the strip is
/// dropped.
pub fn adalight_packet(pixels: usize, order: ColorOrder, slots: &[(u16, u8)]) -> Vec<u8> {
    let n = pixels.clamp(1, MAX_STRIP);
    let mut rgb = vec![0u8; n * 3];
    for &(address, byte) in slots {
        if let Some(b) = (address as usize).checked_sub(1).and_then(|i| rgb.get_mut(i)) {
            *b = byte;
        }
    }
    let [hi, lo] = ((n - 1) as u16).to_be_bytes();
    let mut packet = Vec::with_capacity(6 + n * 3);
    packet.extend_from_slice(b"Ada");
    packet.extend_from_slice(&[hi, lo, hi ^ lo ^ 0x55]);
    for px in rgb.chunks_exact(3) {
        packet.extend_from_slice(&order.arrange([px[0], px[1], px[2]]));
    }
    packet
}

/// `out.serial`'s wire: Adalight over a USB-serial line — the protocol of
/// the Adalight/LEDstream Arduino sketch, which Prismatik, Hyperion and
/// WLED's serial input also read ([`adalight_packet`]).
///
/// **Termios through `libc`, not the `serialport` crate** (rule 11, measured
/// 2026-09-25 on this machine: `cargo build` from clean of the same program —
/// open, 115200 8N1 raw, write one packet — both checked byte for byte
/// against a pseudo-terminal): `libc` alone is 2 crates, ~1.0 s wall and
/// ~1.1 s CPU, 371 KB stripped release; `serialport` 4.10 without default
/// features is 15 crates (`nix` 0.26, `unescaper`, `thiserror`, `syn`, …),
/// ~3.6 s wall and 8.5–10 s CPU, 413 KB; with its default features 18 crates
/// and a build-time dependency on the system's libudev. `libc` 0.2.189 was
/// already in this crate's tree (cosmic-text → fontdb → memmap2), so this
/// adds no crate to the workspace. The price: unix only — `serialport` would
/// have covered Windows, where [`Adalight::open`] refuses with a message.
///
/// Never blocks the frame: the line is non-blocking and a packet the tty
/// cannot take whole is finished before the next one starts, so the strip
/// never sees two packets interleaved; a line still busy drops the new frame
/// (`send` answers `false`, counted like a dropped sACN packet).
pub struct Adalight {
    port: metal_vfs::File,
    pixels: usize,
    order: ColorOrder,
    /// The part of the last packet the line has not taken yet.
    pending: Vec<u8>,
    /// The device is gone (unplugged): [`drive_sampled`] reopens it.
    gone: bool,
}

impl Adalight {
    /// Opens `cfg.port` write-only and non-blocking and, when it is a
    /// terminal, sets it raw, 8N1, no flow control, at `cfg.baud`. A regular
    /// file is written as it is — a capture of what the strip would be sent,
    /// which is what the tests read back.
    pub fn open(cfg: &SerialConfig) -> std::io::Result<Adalight> {
        let port = open_line(cfg.port.trim(), cfg.baud)?;
        Ok(Adalight { port, pixels: cfg.count(), order: cfg.order, pending: Vec::new(), gone: false })
    }

    /// Writes what is pending; `true` once nothing is.
    fn flush(&mut self) -> bool {
        while !self.pending.is_empty() {
            match self.port.write(&self.pending) {
                Ok(0) => return false,
                Ok(n) => {
                    self.pending.drain(..n);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return false,
                Err(_) => {
                    self.gone = true;
                    return false;
                }
            }
        }
        true
    }
}

impl Wire for Adalight {
    /// `universe` means nothing on a serial line; `slots` are the strip's
    /// RGB bytes ([`strip_slots`]).
    fn send(&mut self, _universe: u16, slots: &[(u16, u8)]) -> bool {
        if self.gone || !self.flush() {
            return false;
        }
        self.pending = adalight_packet(self.pixels, self.order, slots);
        self.flush();
        !self.gone
    }

    fn broken(&self) -> bool {
        self.gone
    }
}

/// Opens a serial line for [`Adalight`]; see there.
#[cfg(unix)]
fn open_line(path: &str, baud: u32) -> std::io::Result<metal_vfs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;

    if path.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "out.serial: no port"));
    }
    let file = metal_vfs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
        .open(path)?;
    let fd = file.as_raw_fd();
    // SAFETY: `fd` is the descriptor `file` owns for the rest of this call.
    if unsafe { libc::isatty(fd) } != 1 {
        return Ok(file);
    }
    let speed = baud_constant(baud).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("out.serial: {baud} is not a line speed this build can set"),
        )
    })?;
    // SAFETY: `termios` is plain data that `tcgetattr` fills in whole before
    // anything reads it, and every call gets the descriptor `file` owns.
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut t) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        libc::cfmakeraw(&mut t);
        t.c_cflag |= libc::CLOCAL | libc::CREAD;
        t.c_cflag &= !libc::CSTOPB;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            t.c_cflag &= !libc::CRTSCTS;
        }
        if libc::cfsetispeed(&mut t, speed) != 0
            || libc::cfsetospeed(&mut t, speed) != 0
            || libc::tcsetattr(fd, libc::TCSANOW, &t) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_line(path: &str, _baud: u32) -> std::io::Result<metal_vfs::File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!("out.serial: {path}: this build opens serial lines on unix only"),
    ))
}

/// The termios constant for a line speed, or `None` for one this platform's
/// `cfsetospeed` cannot take.
#[cfg(unix)]
fn baud_constant(baud: u32) -> Option<libc::speed_t> {
    Some(match baud {
        9_600 => libc::B9600,
        19_200 => libc::B19200,
        38_400 => libc::B38400,
        57_600 => libc::B57600,
        115_200 => libc::B115200,
        230_400 => libc::B230400,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        460_800 => libc::B460800,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        500_000 => libc::B500000,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        921_600 => libc::B921600,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        1_000_000 => libc::B1000000,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        1_500_000 => libc::B1500000,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        2_000_000 => libc::B2000000,
        _ => return None,
    })
}

/// What [`State`](crate::engine::State) keeps between frames for one
/// frame-driven node: its open wire, what it was opened with (a changed
/// universe, target or port rebuilds it), and what it last sent.
pub struct SampledWire {
    wire: Box<dyn Wire + Send>,
    opened: String,
    last: Option<Vec<(u16, u8)>>,
    last_sent_at: Option<Instant>,
}

/// The frame-driven fixture hook for one graph: every [`frame_driven`] node
/// in it that is not bypassed sends what `frame` shows where its lights
/// land — one `out.sacn` node is one universe packet with all its heads, one
/// `out.serial` node one Adalight packet. Unchanged output goes out again
/// only every [`KEEPALIVE`]; a failed send is retried on the next frame. A
/// node gone from the graph, bypassed, or whose line broke drops its wire.
/// `wires` is keyed `"<graph id>/<node id>"`, so two devices' graphs never
/// share or prune each other's entries. Returns the failed opens and sends,
/// for `State::wire_errors`.
///
/// **D3.** A sACN/Art-Net node no longer sends its own packet here: it writes
/// its slots into `bus` (tagged with its source id, priority and mode) and a
/// serial node still writes its port directly. After the venue walk has also
/// written, every universe a source touched this tick is resolved **once** and
/// sent through the owning node's [`Sacn`]/[`ArtNet`] wire — so two sources on
/// one universe merge instead of overwriting, and one universe is one packet.
pub fn drive_sampled_bus(
    graph: &metal_graph::Graph,
    frame: &[u8],
    w: u32,
    h: u32,
    root: &Path,
    wires: &mut HashMap<String, SampledWire>,
    bus: &mut DmxBus,
    now: Instant,
) -> u64 {
    drive_sampled_bus_with(graph, frame, w, h, root, &SourceRows::default(), wires, bus, now)
}

/// [`drive_sampled_bus`] with the rows a `venue.fixture` node reads when its
/// colour is not the frame (F3 `signal`, F4 `model`). The engine passes the
/// real maps ([`drive_device`]); the plain `drive_sampled_bus` passes none, so
/// a non-frame source sends nothing there.
pub fn drive_sampled_bus_with(
    graph: &metal_graph::Graph,
    frame: &[u8],
    w: u32,
    h: u32,
    root: &Path,
    rows: &SourceRows,
    wires: &mut HashMap<String, SampledWire>,
    bus: &mut DmxBus,
    now: Instant,
) -> u64 {
    let prefix = format!("{}/", graph.id);
    let nodes: Vec<&metal_graph::Node> =
        graph.nodes.iter().filter(|n| !n.bypass && frame_driven(n)).collect();
    let live: HashSet<String> = nodes.iter().map(|n| format!("{prefix}{}", n.id)).collect();
    wires.retain(|key, s| !key.starts_with(&prefix) || (live.contains(key) && !s.wire.broken()));

    bus.begin_tick();
    let forced = fixture::forced_on(graph);
    // Read once per frame, and only when a head needs one.
    let mut profiles: Option<Vec<fixture::Fixture>> = None;
    let mut errors = 0u64;
    // (universe, wire key) in the order sources were seen: the first source on
    // a universe is its owner, who flushes the merged packet through its wire.
    let mut touched: Vec<(u16, String)> = Vec::new();
    for node in nodes {
        let key = format!("{prefix}{}", node.id);
        let is_dmx = is_net_dmx(&node.kind);
        let (opened, universe, slots) = if node.kind == "out.sacn" {
            let cfg = SacnConfig::from_node(node);
            let heads = SampledSacn::from_node(node).heads;
            let profiles = profiles.get_or_insert_with(|| fixture::load(root));
            (format!("sacn {cfg:?}"), cfg.universe, head_slots(&heads, profiles, forced, frame, w, h))
        } else if node.kind == crate::artnet::ARTNET_KIND {
            let cfg = crate::artnet::ArtNetConfig::from_node(node);
            let heads = SampledSacn::from_node(node).heads;
            let profiles = profiles.get_or_insert_with(|| fixture::load(root));
            (format!("artnet {cfg:?}"), cfg.port_address(), head_slots(&heads, profiles, forced, frame, w, h))
        } else {
            let cfg = SerialConfig::from_node(node);
            if cfg.port.trim().is_empty() || cfg.count() == 0 {
                continue;
            }
            (cfg.line(), 0, strip_slots(&cfg, forced, frame, w, h))
        };
        if wires.get(&key).is_none_or(|s| s.opened != opened) {
            let wire: std::io::Result<Box<dyn Wire + Send>> = if node.kind == "out.sacn" {
                Sacn::new(SacnConfig::from_node(node).unicast_addr())
                    .map(|s| Box::new(s) as Box<dyn Wire + Send>)
            } else if node.kind == crate::artnet::ARTNET_KIND {
                crate::artnet::ArtNet::new(&crate::artnet::ArtNetConfig::from_node(node))
                    .map(|s| Box::new(s) as Box<dyn Wire + Send>)
            } else {
                Adalight::open(&SerialConfig::from_node(node)).map(|s| Box::new(s) as Box<dyn Wire + Send>)
            };
            match wire {
                Ok(wire) => {
                    wires.insert(key.clone(), SampledWire { wire, opened, last: None, last_sent_at: None });
                }
                Err(_) => {
                    errors += 1;
                    continue;
                }
            }
        }
        if is_dmx {
            // D3: into the bus; the flush below sends it once per universe.
            let src = bus_source(node);
            bus.write(universe, &key, src.priority, src.mode, &slots, now);
            touched.push((universe, key));
            continue;
        }
        // A serial strip is its own wire, not a universe on the bus.
        let Some(s) = wires.get_mut(&key) else {
            continue;
        };
        let changed = s.last.as_deref() != Some(slots.as_slice());
        let stale = s.last_sent_at.is_none_or(|t| now.duration_since(t) >= KEEPALIVE);
        if !(changed || stale) {
            continue;
        }
        if s.wire.send(universe, &slots) {
            s.last = Some(slots);
            s.last_sent_at = Some(now);
        } else {
            errors += 1;
        }
    }
    errors += drive_venue_nodes(graph, frame, w, h, root, rows, wires, bus, &mut touched, now);
    errors += flush_bus(bus, wires, &touched, now);
    errors
}

/// [`drive_sampled_bus`] with a throwaway bus: the shape a caller that has one
/// source per universe (or a test) wants. The engine keeps the real bus in
/// `State::dmx_bus` and calls [`drive_sampled_bus`] through
/// [`drive_device`]/[`drive_device_with`].
pub fn drive_sampled(
    graph: &metal_graph::Graph,
    frame: &[u8],
    w: u32,
    h: u32,
    root: &Path,
    wires: &mut HashMap<String, SampledWire>,
    now: Instant,
) -> u64 {
    drive_sampled_bus(graph, frame, w, h, root, wires, &mut DmxBus::new(), now)
}

/// `engine::render_device`'s hook: [`drive_sampled`] over `device`'s own
/// authored graph, with the frame just composed. `compiled` is the graph the
/// frame was compiled from, used when it is that authored graph; a
/// synthesized plan (a canvas that wires no generator) carries no `out.*`
/// node, so the file is read instead — the same read `compile_device_plan`
/// makes every frame. A device with no graph drops whatever wires it had.
pub fn drive_device(
    root: &Path,
    device: &str,
    compiled: Option<&metal_graph::Graph>,
    rows: &SourceRows,
    frame: &[u8],
    w: u32,
    h: u32,
    wires: &mut HashMap<String, SampledWire>,
    bus: &mut DmxBus,
    now: Instant,
) -> u64 {
    drive_device_with(
        root,
        device,
        compiled,
        rows,
        || Some((std::borrow::Cow::Borrowed(frame), w, h)),
        wires,
        bus,
        now,
    )
}

/// [`drive_device`] with the frame asked for only when the device's graph has
/// a node that samples it: a texture frame is read through the sink's
/// non-blocking ring, which hands back a picture a frame or two old, or `None`
/// while the first copy is in flight (then nothing is sent this frame).
pub fn drive_device_with<'a>(
    root: &Path,
    device: &str,
    compiled: Option<&metal_graph::Graph>,
    rows: &SourceRows,
    frame: impl FnOnce() -> Option<(std::borrow::Cow<'a, [u8]>, u32, u32)>,
    wires: &mut HashMap<String, SampledWire>,
    bus: &mut DmxBus,
    now: Instant,
) -> u64 {
    let id = crate::engine::device_graph_id(device);
    let read;
    let graph = match compiled.filter(|g| g.id == id) {
        Some(g) => g,
        None => {
            read = crate::engine::device_graph_at(root, device);
            match &read {
                Some(g) => g,
                None => {
                    let prefix = format!("{id}/");
                    wires.retain(|key, _| !key.starts_with(&prefix));
                    return 0;
                }
            }
        }
    };
    if !graph.nodes.iter().any(|n| {
        !n.bypass
            && (frame_driven(n)
                || n.kind == metal_graph::VENUE_FIXTURES_KIND
                || n.kind == metal_graph::VENUE_FIXTURE_KIND)
    }) {
        // Nothing samples the frame; this only retires wires that went away.
        return drive_sampled_bus_with(graph, &[], 0, 0, root, rows, wires, bus, now);
    }
    match frame() {
        Some((px, w, h)) => drive_sampled_bus_with(graph, &px, w, h, root, rows, wires, bus, now),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket as StdUdpSocket;

    const MOVING_HEAD: &str = r#"
label = "Wire Test Head"
universe = 0
address = 1

[[channel]]
signal = "level"
coarse = 4

[[channel]]
signal = "pan"
coarse = 0
fine = 1
"#;

    fn scratch_root(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "fw-wire-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        metal_vfs::create_dir_all(fixture::dir(&root)).unwrap();
        metal_vfs::write(fixture::dir(&root).join("probe.toml"), MOVING_HEAD).unwrap();
        root
    }

    fn signal_row() -> Vec<f32> {
        // `EFFECTS_WIDTH` zeros, then the `SIGNAL_WIDTH` signal floats at
        // their defaults with level at 0.8 (`read_signal` locates them from
        // the end).
        let mut row = vec![0.0f32; metal_schema::EFFECTS_WIDTH];
        let mut sig: Vec<f32> = metal_schema::SIGNAL_FLOATS.iter().map(|s| s.default).collect();
        sig[0] = 0.8;
        row.extend(sig);
        row
    }

    /// A Bluetooth link factory that opens a disconnected `Recorder` — the
    /// sACN tests carry no `out.bluetooth` node, so the link is never even
    /// built; this only satisfies [`drive_frame`]'s argument list.
    pub(super) fn noop_links() -> &'static LinkFactory {
        &(|_cfg| {
            Box::new(metal_ble::Recorder::new().0.with_status(metal_ble::LinkStatus::Failed("test".into())))
                as Box<dyn metal_ble::Link>
        })
    }

    #[test]
    fn a_valid_e131_packet_carries_the_byte_drive_head_would_produce() {
        let root = scratch_root("valid-packet");
        let recv = StdUdpSocket::bind("127.0.0.1:0").unwrap();
        recv.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let port = recv.local_addr().unwrap().port();

        let mut graph = metal_graph::Graph::new("g", "t", metal_graph::Domain::Live);
        let out = graph.add_node("out", "out.sacn", [0.0, 0.0]);
        graph.node_mut(&out).unwrap().config = metal_graph::Config::from_json_str(&format!(
            r#"{{"universe": 7, "unicast": "127.0.0.1:{port}"}}"#
        ))
        .unwrap();
        graph.add_edge(
            metal_graph::Endpoint::new("src", "row"),
            metal_graph::Endpoint::new(&out, "signal"),
        );

        let mut script_rows = HashMap::new();
        script_rows.insert("src".to_string(), signal_row());

        let head = Head { fixture: "probe".into(), universe: None, address: Some(1) };
        let mut sockets = HashMap::new();
        let errors = drive_frame(
            &graph,
            &head,
            &root,
            None,
            &HashMap::new(),
            &script_rows,
            &[],
            &mut sockets,
            &mut HashMap::new(),
            noop_links(),
            Instant::now(),
        );
        assert_eq!(errors, 0, "the send must not fail against a bound local receiver");

        let mut buf = [0u8; 700];
        let (n, _) = recv.recv_from(&mut buf).expect("a packet must arrive");
        assert_eq!(n, 638, "root + framing + dmp header + 513-byte property block");
        assert_eq!(&buf[4..16], b"ASC-E1.17\0\0\0", "root layer's ACN packet identifier");
        let universe = u16::from_be_bytes([buf[113], buf[114]]);
        assert_eq!(universe, 7, "the framing layer's universe field");

        // level is the profile's channel at relative slot 4 -> absolute
        // address 5 (head.address 1 + 4) -> dmx[4] -> byte offset 126 + 4.
        let expected = fixture::drive_head(
            &root,
            &head,
            Some(&graph),
            None,
            &metal_schema::read_signal(&signal_row()),
        )
        .unwrap();
        let (address, byte) = *expected.iter().find(|(a, _)| *a == 5).unwrap();
        assert_eq!(address, 5);
        assert_eq!(buf[126 + (address as usize - 1)], byte);

        metal_vfs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_bypassed_out_sacn_node_sends_nothing() {
        let root = scratch_root("bypassed");
        let recv = StdUdpSocket::bind("127.0.0.1:0").unwrap();
        recv.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        let port = recv.local_addr().unwrap().port();

        let mut graph = metal_graph::Graph::new("g", "t", metal_graph::Domain::Live);
        let out = graph.add_node("out", "out.sacn", [0.0, 0.0]);
        graph.node_mut(&out).unwrap().bypass = true;
        graph.node_mut(&out).unwrap().config = metal_graph::Config::from_json_str(&format!(
            r#"{{"universe": 7, "unicast": "127.0.0.1:{port}"}}"#
        ))
        .unwrap();
        graph.add_edge(
            metal_graph::Endpoint::new("src", "row"),
            metal_graph::Endpoint::new(&out, "signal"),
        );

        let mut script_rows = HashMap::new();
        script_rows.insert("src".to_string(), signal_row());

        let head = Head { fixture: "probe".into(), universe: None, address: Some(1) };
        let mut sockets = HashMap::new();
        let errors = drive_frame(
            &graph,
            &head,
            &root,
            None,
            &HashMap::new(),
            &script_rows,
            &[],
            &mut sockets,
            &mut HashMap::new(),
            noop_links(),
            Instant::now(),
        );
        assert_eq!(errors, 0);
        assert!(sockets.is_empty(), "a bypassed node never even opens a socket");

        let mut buf = [0u8; 700];
        let result = recv.recv_from(&mut buf);
        assert!(result.is_err(), "nothing must arrive for a bypassed node");

        metal_vfs::remove_dir_all(&root).ok();
    }

    /// `crate::wire::status_json` is the properties panel's whole live
    /// read (`metal-dash`'s `call::graph::graph_get`) — `None` for a node
    /// nothing has cached yet, and otherwise the live universe/unicast/
    /// last-sent fields, never a copy kept anywhere between calls.
    #[test]
    fn status_json_reads_live_fields_and_is_none_for_an_unknown_node() {
        let mut sockets: HashMap<NodeId, WireSocket> = HashMap::new();
        assert!(status_json(&sockets, "missing").is_none());

        let socket = WireSocket::new(SacnConfig { universe: 5, unicast: Some("10.0.0.5:5568".into()) })
            .expect("a socket must bind");
        sockets.insert("out-0".to_string(), socket);

        let status = status_json(&sockets, "out-0").expect("now cached");
        assert_eq!(status["universe"], json!(5));
        assert_eq!(status["unicast"], json!("10.0.0.5:5568"));
        assert_eq!(status["last_sent_ms_ago"], serde_json::Value::Null, "never sent yet");
    }

    #[test]
    fn config_defaults_to_universe_one_multicast_when_absent_or_unparseable() {
        let node = metal_graph::Node::new("n", "out.sacn", [0.0, 0.0]);
        let cfg = SacnConfig::from_node(&node);
        assert_eq!(cfg, SacnConfig { universe: 1, unicast: None });
        assert_eq!(cfg.unicast_addr(), None);
    }

    #[test]
    fn unicast_without_a_port_gets_the_spec_default() {
        let cfg = SacnConfig { universe: 1, unicast: Some("10.0.0.9".to_string()) };
        assert_eq!(cfg.unicast_addr(), Some("10.0.0.9:5568".parse().unwrap()));
    }

    #[test]
    fn a_changed_config_rebuilds_the_socket_rather_than_reusing_it() {
        let root = scratch_root("config-change");
        let recv_a = StdUdpSocket::bind("127.0.0.1:0").unwrap();
        recv_a.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
        let port_a = recv_a.local_addr().unwrap().port();
        let recv_b = StdUdpSocket::bind("127.0.0.1:0").unwrap();
        recv_b.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
        let port_b = recv_b.local_addr().unwrap().port();

        let mut graph = metal_graph::Graph::new("g", "t", metal_graph::Domain::Live);
        let out = graph.add_node("out", "out.sacn", [0.0, 0.0]);
        graph.node_mut(&out).unwrap().config =
            metal_graph::Config::from_json_str(&format!(r#"{{"universe": 1, "unicast": "127.0.0.1:{port_a}"}}"#))
                .unwrap();
        graph.add_edge(
            metal_graph::Endpoint::new("src", "row"),
            metal_graph::Endpoint::new(&out, "signal"),
        );
        let mut script_rows = HashMap::new();
        script_rows.insert("src".to_string(), signal_row());
        let head = Head { fixture: "probe".into(), universe: None, address: Some(1) };
        let mut sockets = HashMap::new();

        drive_frame(&graph, &head, &root, None, &HashMap::new(), &script_rows, &[], &mut sockets, &mut HashMap::new(), noop_links(), Instant::now());
        assert!(recv_a.recv_from(&mut [0u8; 700]).is_ok(), "first send goes to the first address");

        // Point the same node at a different unicast address, as `set_params`
        // would after an operator edit.
        graph.node_mut(&out).unwrap().config =
            metal_graph::Config::from_json_str(&format!(r#"{{"universe": 1, "unicast": "127.0.0.1:{port_b}"}}"#))
                .unwrap();
        drive_frame(&graph, &head, &root, None, &HashMap::new(), &script_rows, &[], &mut sockets, &mut HashMap::new(), noop_links(), Instant::now());
        assert!(recv_b.recv_from(&mut [0u8; 700]).is_ok(), "the rebuilt socket sends to the new address");

        metal_vfs::remove_dir_all(&root).ok();
    }
}

/// Frame-driven fixtures ([`drive_sampled`]) and the Adalight wire.
#[cfg(test)]
mod sampled_tests {
    use super::*;
    use std::net::UdpSocket as StdUdpSocket;

    /// An RGB wash, the shape of `data/fixtures/example-wash.toml`: level,
    /// hue, sat and strobe at slots 0..3.
    const WASH: &str = "[[channel]]\nsignal = \"level\"\ncoarse = 0\n\n[[channel]]\nsignal = \"hue\"\ncoarse = 1\n\n\
                        [[channel]]\nsignal = \"sat\"\ncoarse = 2\n\n[[channel]]\nsignal = \"strobe\"\ncoarse = 3\n";

    fn scratch(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "fw-sampled-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = metal_vfs::remove_dir_all(&root);
        metal_vfs::create_dir_all(fixture::dir(&root)).unwrap();
        metal_vfs::write(fixture::dir(&root).join("wash.toml"), WASH).unwrap();
        root
    }

    /// A `w`×`h` RGBA frame: its left half `left`, its right half `right`.
    fn halves(w: u32, h: u32, left: [u8; 3], right: [u8; 3]) -> Vec<u8> {
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..h {
            for x in 0..w {
                let c = if x < w / 2 { left } else { right };
                px.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        px
    }

    fn receiver() -> (StdUdpSocket, u16) {
        let recv = StdUdpSocket::bind("127.0.0.1:0").unwrap();
        recv.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let port = recv.local_addr().unwrap().port();
        (recv, port)
    }

    /// A device graph whose `out` node has `config`.
    fn graph_with(id: &str, kind: &str, config: serde_json::Value) -> (metal_graph::Graph, String) {
        let mut g = metal_graph::Graph::new(id, "t", metal_graph::Domain::Live);
        g.add_node("out", "output.device", [0.0, 0.0]);
        let n = g.add_node("out", kind, [0.0, 100.0]);
        g.node_mut(&n).unwrap().config = metal_graph::Config::from_json_str(&config.to_string()).unwrap();
        (g, n)
    }

    fn sacn(port: u16, heads: serde_json::Value) -> serde_json::Value {
        json!({ "universe": 9, "unicast": format!("127.0.0.1:{port}"), "heads": heads })
    }

    /// Four heads on one universe: each takes the colour of the frame where
    /// its light lands, through its profile, all in **one** E1.31 packet. The
    /// bytes are the wash profile's level, hue, sat, strobe for that colour:
    /// red is `255, 0, 255, 0`; `[40, 140, 255]` is full level, hue
    /// `0.589 · 255 = 150`, sat `215 / 255 · 255 = 215`.
    #[test]
    fn heads_sampled_from_the_frame_go_out_as_one_universe_packet() {
        let root = scratch("heads");
        let (recv, port) = receiver();
        let (g, _) = graph_with(
            "device-wall",
            "out.sacn",
            sacn(
                port,
                json!([
                    { "id": "left", "fixture": "wash", "address": 1, "sample": [0.25, 0.5] },
                    { "id": "right", "fixture": "wash", "address": 5, "sample": [0.75, 0.5] },
                    { "id": "dark", "fixture": "wash", "address": 9, "sample": null },
                    { "id": "nowhere", "fixture": "not-a-profile", "address": 13, "sample": [0.5, 0.5] }
                ]),
            ),
        );
        let frame = halves(8, 4, [255, 0, 0], [40, 140, 255]);
        let mut wires = HashMap::new();
        let t0 = Instant::now();
        assert_eq!(drive_sampled(&g, &frame, 8, 4, &root, &mut wires, t0), 0);

        let mut buf = [0u8; 700];
        let (n, _) = recv.recv_from(&mut buf).expect("the universe packet");
        assert_eq!(n, 638);
        assert_eq!(&buf[4..16], b"ASC-E1.17\0\0\0");
        assert_eq!(u16::from_be_bytes([buf[113], buf[114]]), 9, "the node's universe");
        let dmx = &buf[126..142];
        assert_eq!(&dmx[0..4], &[255, 0, 255, 0], "left: red");
        assert_eq!(&dmx[4..8], &[255, 150, 215, 0], "right: the blue half");
        assert_eq!(&dmx[8..12], &[0, 0, 0, 0], "no projected light reaches it: black");
        assert_eq!(&dmx[12..16], &[0, 0, 0, 0], "a profile that is not there is skipped");
        recv.set_read_timeout(Some(Duration::from_millis(150))).unwrap();
        assert!(recv.recv_from(&mut buf).is_err(), "one node, one universe: one packet, not one per head");

        // Unchanged and not stale: nothing. A new colour: sent. Unchanged
        // but stale: the keep-alive.
        drive_sampled(&g, &frame, 8, 4, &root, &mut wires, t0 + Duration::from_millis(20));
        assert!(recv.recv_from(&mut buf).is_err(), "nothing changed");
        let green = halves(8, 4, [0, 200, 30], [0, 200, 30]);
        let t1 = t0 + Duration::from_millis(40);
        drive_sampled(&g, &green, 8, 4, &root, &mut wires, t1);
        recv.recv_from(&mut buf).expect("a changed frame goes out");
        assert_eq!(&buf[126..130], &[200, 91, 255, 0], "[0, 200, 30]: level 200, hue 0.358, full sat");
        drive_sampled(&g, &green, 8, 4, &root, &mut wires, t1 + KEEPALIVE);
        recv.recv_from(&mut buf).expect("unchanged but stale: the keep-alive");

        metal_vfs::remove_dir_all(&root).ok();
    }

    /// P-D: `out.artnet` with heads is the same frame-driven path, one ArtDmx
    /// to the node's 15-bit Port-Address, keep-alive included.
    #[test]
    fn artnet_heads_go_out_as_one_art_dmx_on_the_port_address() {
        let root = scratch("artnet");
        let (recv, port) = receiver();
        let (g, _) = graph_with(
            "device-art",
            crate::artnet::ARTNET_KIND,
            json!({ "net": 1, "subnet": 2, "universe": 3, "unicast": format!("127.0.0.1:{port}"),
                    "heads": [{ "id": "l", "fixture": "wash", "address": 1, "sample": [0.25, 0.5] }] }),
        );
        let frame = halves(8, 4, [255, 0, 0], [0, 0, 255]);
        let mut wires = HashMap::new();
        let t0 = Instant::now();
        assert_eq!(drive_sampled(&g, &frame, 8, 4, &root, &mut wires, t0), 0);
        let mut buf = [0u8; 700];
        let (n, _) = recv.recv_from(&mut buf).expect("the ArtDmx");
        let (_, pa, data) = crate::artnet::parse_dmx(&buf[..n]).expect("an ArtDmx");
        assert_eq!(pa, 0x0123);
        assert_eq!(&data[0..4], &[255, 0, 255, 0], "red through the wash profile");
        drive_sampled(&g, &frame, 8, 4, &root, &mut wires, t0 + KEEPALIVE);
        recv.recv_from(&mut buf).expect("the keep-alive");
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// D3: two `out.sacn` nodes on one universe merge on the bus — the frame
    /// goes out as **one** packet, and the higher-priority source's bytes win.
    #[test]
    fn a_bus_merges_two_out_nodes_into_one_universe_packet() {
        let root = scratch("bus-merge");
        let (recv, port) = receiver();
        let mut g = metal_graph::Graph::new("device-bus", "t", metal_graph::Domain::Live);
        g.add_node("out", "output.device", [0.0, 0.0]);
        let a = g.add_node("a", "out.sacn", [0.0, 100.0]);
        g.node_mut(&a).unwrap().config = metal_graph::Config::from_json_str(
            &json!({ "universe": 9, "unicast": format!("127.0.0.1:{port}"), "priority": 200,
                     "heads": [{ "id": "h", "fixture": "wash", "address": 1, "sample": [0.25, 0.5] }] })
            .to_string(),
        )
        .unwrap();
        let b = g.add_node("b", "out.sacn", [0.0, 200.0]);
        g.node_mut(&b).unwrap().config = metal_graph::Config::from_json_str(
            &json!({ "universe": 9, "unicast": format!("127.0.0.1:{port}"), "priority": 100,
                     "heads": [{ "id": "h", "fixture": "wash", "address": 1, "sample": [0.75, 0.5] }] })
            .to_string(),
        )
        .unwrap();

        let frame = halves(8, 4, [255, 0, 0], [40, 140, 255]);
        let mut wires = HashMap::new();
        let mut bus = DmxBus::new();
        let errors = drive_sampled_bus(&g, &frame, 8, 4, &root, &mut wires, &mut bus, Instant::now());
        assert_eq!(errors, 0);

        let mut buf = [0u8; 700];
        let (n, _) = recv.recv_from(&mut buf).expect("the merged universe");
        assert_eq!(n, 638);
        assert_eq!(u16::from_be_bytes([buf[113], buf[114]]), 9);
        assert_eq!(&buf[126..130], &[255, 0, 255, 0], "priority 200 owns the universe: red");
        recv.set_read_timeout(Some(Duration::from_millis(150))).unwrap();
        assert!(
            recv.recv_from(&mut buf).is_err(),
            "two sources, one universe: one packet, not one per node"
        );
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// Blackout's fixture half reaches a frame-driven head: the graph's
    /// `forced` black wins over the red the frame shows.
    #[test]
    fn a_forced_black_wins_over_the_frame() {
        let root = scratch("forced");
        let (recv, port) = receiver();
        let (mut g, n) = graph_with(
            "device-wall",
            "out.sacn",
            sacn(port, json!([{ "id": "w", "fixture": "wash", "address": 1, "sample": [0.5, 0.5] }])),
        );
        fixture::set_forced(g.node_mut(&n).unwrap(), Some([0.0, 0.0, 0.0]));
        let frame = halves(4, 2, [255, 0, 0], [255, 0, 0]);
        drive_sampled(&g, &frame, 4, 2, &root, &mut HashMap::new(), Instant::now());
        let mut buf = [0u8; 700];
        recv.recv_from(&mut buf).expect("a packet");
        assert_eq!(&buf[126..130], &[0, 0, 0, 0]);
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// The two paths never both take a node: a frame-driven `out.sacn` is not
    /// opened by the signal-row walk even with a `signal` cable.
    #[test]
    fn drive_frame_leaves_a_frame_driven_node_alone() {
        let root = scratch("split");
        let (mut g, n) = graph_with(
            "device-wall",
            "out.sacn",
            json!({ "heads": [{ "id": "w", "fixture": "wash", "sample": [0.5, 0.5] }] }),
        );
        g.add_edge(metal_graph::Endpoint::new("src", "row"), metal_graph::Endpoint::new(&n, "signal"));
        let mut rows = HashMap::new();
        let mut row = vec![0.0f32; metal_schema::EFFECTS_WIDTH];
        row.extend_from_slice(&[
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            5600.0,
            0.0,
            0.0,
            0.0,
            0.0,
        ]);
        rows.insert("src".to_string(), row);
        let head = Head { fixture: "wash".into(), universe: None, address: Some(1) };
        let mut sockets = HashMap::new();
        drive_frame(&g, &head, &root, None, &HashMap::new(), &rows, &[], &mut sockets, &mut HashMap::new(), super::tests::noop_links(), Instant::now());
        assert!(sockets.is_empty(), "no signal-path socket for a frame-driven node");
        assert!(frame_driven(g.node(&n).unwrap()));
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// Wires are kept per graph: driving one device never drops another's,
    /// and a bypassed (or deleted) node drops only its own.
    #[test]
    fn two_devices_keep_their_own_wires() {
        let root = scratch("two");
        let (_recv, port) = receiver();
        let heads = json!([{ "id": "w", "fixture": "wash", "sample": [0.5, 0.5] }]);
        let (a, _) = graph_with("device-a", "out.sacn", sacn(port, heads.clone()));
        let (mut b, nb) = graph_with("device-b", "out.sacn", sacn(port, heads));
        let frame = halves(4, 2, [9, 9, 9], [9, 9, 9]);
        let mut wires = HashMap::new();
        let now = Instant::now();
        drive_sampled(&a, &frame, 4, 2, &root, &mut wires, now);
        drive_sampled(&b, &frame, 4, 2, &root, &mut wires, now);
        drive_sampled(&a, &frame, 4, 2, &root, &mut wires, now);
        assert_eq!(wires.len(), 2, "{:?}", wires.keys().collect::<Vec<_>>());
        b.node_mut(&nb).unwrap().bypass = true;
        drive_sampled(&b, &frame, 4, 2, &root, &mut wires, now);
        assert_eq!(wires.keys().collect::<Vec<_>>(), [&format!("device-a/{nb}")]);
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// The Adalight packet, byte for byte: `"Ada"`, the LED count minus one
    /// (high, low), their XOR with `0x55`, then one triple per LED in the
    /// strip's order. A slot past the strip is dropped, an LED no slot names
    /// is dark.
    #[test]
    fn an_adalight_packet_is_the_header_the_count_the_checksum_and_the_triples() {
        let slots = [(1, 255), (2, 0), (3, 10), (4, 1), (5, 2), (6, 3), (99, 7)];
        assert_eq!(
            adalight_packet(3, ColorOrder::Rgb, &slots),
            [b'A', b'd', b'a', 0x00, 0x02, 0x57, 255, 0, 10, 1, 2, 3, 0, 0, 0]
        );
        assert_eq!(&adalight_packet(3, ColorOrder::Grb, &slots)[6..12], &[0, 255, 10, 2, 1, 3]);
        assert_eq!(&adalight_packet(3, ColorOrder::Bgr, &slots)[6..9], &[10, 0, 255]);
        let big = adalight_packet(256, ColorOrder::Rgb, &[]);
        assert_eq!(&big[..6], &[b'A', b'd', b'a', 0x00, 0xff, 0xaa], "255 = 0x00ff, checksum 0xaa");
        assert_eq!(big.len(), 6 + 256 * 3);
        assert_eq!(&adalight_packet(300, ColorOrder::Rgb, &[])[3..6], &[0x01, 0x2b, 0x7f], "299 = 0x012b");
    }

    /// An `out.serial` node samples its device's frame at each LED's point
    /// and writes one Adalight packet to its port — a plain file here,
    /// which is written as it is, with no line to configure.
    #[test]
    fn a_strip_writes_the_frame_it_samples_to_its_port() {
        let root = scratch("strip-file");
        let capture = root.join("strip.capture");
        metal_vfs::write(&capture, b"").unwrap();
        let (g, _) = graph_with(
            "device-wall",
            "out.serial",
            json!({ "port": capture, "sample": [[0.25, 0.5], [0.75, 0.5], null], "venue": "bar.strip-1" }),
        );
        let frame = halves(8, 4, [255, 0, 10], [40, 140, 255]);
        let mut wires = HashMap::new();
        assert_eq!(drive_sampled(&g, &frame, 8, 4, &root, &mut wires, Instant::now()), 0);
        assert_eq!(
            metal_vfs::read(&capture).unwrap(),
            [b'A', b'd', b'a', 0, 2, 0x57, 255, 0, 10, 40, 140, 255, 0, 0, 0],
            "three LEDs: the left half, the right half, and one no light reaches"
        );
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// With no sample points the strip reads the frame's middle row, spread
    /// across `pixels` LEDs, in the strip's colour order.
    #[test]
    fn a_strip_without_points_samples_across_the_frame() {
        let root = scratch("strip-row");
        let capture = root.join("strip.capture");
        metal_vfs::write(&capture, b"").unwrap();
        let (g, _) = graph_with("device-wall", "out.serial", json!({ "port": capture, "pixels": 4, "order": "grb" }));
        let frame = halves(8, 4, [255, 0, 0], [0, 0, 255]);
        drive_sampled(&g, &frame, 8, 4, &root, &mut HashMap::new(), Instant::now());
        let got = metal_vfs::read(&capture).unwrap();
        assert_eq!(&got[..6], &[b'A', b'd', b'a', 0, 3, 0x56]);
        assert_eq!(&got[6..], &[0, 255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255], "red red blue blue, as g r b");
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// No port, nothing written and nothing counted; a port that is not
    /// there is an error each frame, never a panic.
    #[test]
    fn a_strip_with_no_port_is_idle_and_a_missing_port_is_counted() {
        let root = scratch("strip-idle");
        let frame = halves(4, 2, [1, 2, 3], [1, 2, 3]);
        let mut wires = HashMap::new();
        let (idle, _) = graph_with("device-wall", "out.serial", json!({ "pixels": 3 }));
        assert_eq!(drive_sampled(&idle, &frame, 4, 2, &root, &mut wires, Instant::now()), 0);
        assert!(wires.is_empty());
        let (gone, _) = graph_with("device-wall", "out.serial", json!({ "pixels": 3, "port": root.join("no-such-tty") }));
        assert_eq!(drive_sampled(&gone, &frame, 4, 2, &root, &mut wires, Instant::now()), 1);
        assert!(wires.is_empty());
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// The real line discipline: a pseudo-terminal pair stands in for the
    /// USB-serial adapter. The strip's bytes arrive on the other side
    /// exactly — `\n` and `\r` included, which a line left in cooked mode
    /// would have rewritten — and the line is raw at the speed asked for.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_strip_reaches_a_pseudo_terminal_raw_at_its_speed() {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::io::AsRawFd;

        // SAFETY: plain libc calls on a descriptor this test owns and closes.
        let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
        assert!(master >= 0, "a pty: {}", std::io::Error::last_os_error());
        let mut name = [0 as libc::c_char; 128];
        // SAFETY: as above; `name` outlives the call and its length is given.
        unsafe {
            assert_eq!(libc::grantpt(master), 0);
            assert_eq!(libc::unlockpt(master), 0);
            assert_eq!(libc::ptsname_r(master, name.as_mut_ptr(), name.len()), 0);
        }
        // SAFETY: `ptsname_r` wrote a NUL-terminated path into `name`.
        let slave = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }.to_str().unwrap().to_string();

        let root = scratch("pty");
        let (g, _) = graph_with(
            "device-wall",
            "out.serial",
            json!({ "port": slave, "baud": 500000, "sample": [[0.25, 0.5], [0.75, 0.5]] }),
        );
        let frame = halves(8, 4, [10, 13, 0], [255, 255, 255]);
        let mut wires = HashMap::new();
        assert_eq!(drive_sampled(&g, &frame, 8, 4, &root, &mut wires, Instant::now()), 0);

        let want = [b'A', b'd', b'a', 0, 1, 0x54, 10, 13, 0, 255, 255, 255];
        let mut got = Vec::new();
        while got.len() < want.len() {
            let mut pfd = libc::pollfd { fd: master, events: libc::POLLIN, revents: 0 };
            // SAFETY: one valid `pollfd`, count 1.
            let ready = unsafe { libc::poll(&mut pfd, 1, 2000) };
            assert!(ready > 0, "the strip's bytes arrive, got {got:?}");
            let mut buf = [0u8; 64];
            // SAFETY: `buf` is valid for `buf.len()` bytes.
            let n = unsafe { libc::read(master, buf.as_mut_ptr().cast(), buf.len()) };
            assert!(n > 0);
            got.extend_from_slice(&buf[..n as usize]);
        }
        assert_eq!(got, want);

        // The line as the wire left it (still open, in `wires`).
        let probe = metal_vfs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
            .open(&slave)
            .unwrap();
        // SAFETY: `termios` is filled in whole by `tcgetattr` before it is read.
        let t = unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(probe.as_raw_fd(), &mut t), 0);
            t
        };
        // SAFETY: reads a field of a filled-in `termios`.
        assert_eq!(unsafe { libc::cfgetospeed(&t) }, libc::B500000);
        assert_eq!(t.c_oflag & libc::OPOST, 0, "no output processing");
        assert_eq!(t.c_lflag & (libc::ICANON | libc::ECHO), 0, "not a cooked line");
        assert_eq!(t.c_cflag & libc::CSIZE, libc::CS8);

        drop(wires);
        // SAFETY: closes the descriptor this test opened.
        unsafe { libc::close(master) };
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// The render loop drives it: a device whose graph carries a
    /// frame-driven head, a solid colour armed on the device, one
    /// `render_device` — and the colour is on the wire, through the real
    /// `data/fixtures/example-wash.toml` profile. An emptied stack renders
    /// black and the head goes dark with it.
    #[test]
    fn the_render_loop_puts_the_frame_colour_on_the_wire() {
        use crate::engine::{render_device, State};

        let device = format!("fixture-probe-{}", std::process::id());
        let root = crate::output::data_root();
        let mut paint = crate::paint::painter();
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.paused_at = Some(3.0);
        let mut a = crate::layers::Layer::new("layer-a", "a", 0);
        a.outputs = vec![device.clone()];
        state.layers = vec![a];
        let dev = crate::layers::Device { mode: crate::layers::DeviceMode::Own, ..Default::default() };
        state.sources.insert(device.clone(), dev);

        let (recv, port) = receiver();
        let (g, _) = graph_with(
            &crate::engine::device_graph_id(&device),
            "out.sacn",
            json!({ "universe": 3, "unicast": format!("127.0.0.1:{port}"),
                    "heads": [{ "id": "wash", "fixture": "example-wash", "address": 1, "sample": [0.5, 0.5] }] }),
        );
        crate::engine::save_graph_at(&root, &g).unwrap();

        let t = state.time();
        state.overlays.start_solid_color(device.clone(), [0, 200, 30], t, 5.0);
        // The wire reads the frame through the sink's non-blocking ring, a frame
        // or two late: render until the first copy lands and the universe goes out.
        let mut buf = [0u8; 700];
        recv.set_nonblocking(true).unwrap();
        let mut sent = false;
        for _ in 0..300 {
            render_device(&mut *paint, &mut sim, &mut state, &device, 64, 36);
            std::thread::sleep(Duration::from_millis(10));
            if recv.recv_from(&mut buf).is_ok() {
                sent = true;
                break;
            }
        }
        assert!(sent, "the render loop sent the universe");
        // What the lag queued behind it.
        while recv.recv_from(&mut buf).is_ok() {}
        recv.set_nonblocking(false).unwrap();
        assert_eq!(u16::from_be_bytes([buf[113], buf[114]]), 3);
        assert_eq!(&buf[126..130], &[200, 91, 255, 0], "the solid's colour, as the wash's level/hue/sat");

        state.layers.clear();
        render_device(&mut *paint, &mut sim, &mut state, &device, 64, 36);
        recv.recv_from(&mut buf).expect("the emptied stack's frame");
        assert_eq!(&buf[126..130], &[0, 0, 0, 0], "a black frame darkens the head");
        assert_eq!(state.wire_errors, 0);

        let _ = metal_vfs::remove_file(crate::engine::graph_path_at(&root, &crate::engine::device_graph_id(&device)));
    }
}

/// BLE Phase B: the four profiles, the DMX beam-head slots, and float
/// authoring end-to-end through an `out.bluetooth` node into a
/// `metal_ble::Recorder` (the `metal_ble::open` stand-in).
#[cfg(test)]
mod ble_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// The four Phase B profiles, copied out of the repo's `data/fixtures/`
    /// into a tempdir — tests never read or write the real `data/` (rule 4).
    fn fixtures_root(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "metal-ble-wire-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = metal_vfs::remove_dir_all(&root);
        metal_vfs::create_dir_all(fixture::dir(&root)).unwrap();
        let real = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/fixtures");
        for name in [
            "moving-head-beam",
            "godox-sl60iid",
            "godox-sl60iibi",
            "godox-rgb",
        ] {
            let text = metal_vfs::read_to_string(real.join(format!("{name}.toml")))
                .unwrap_or_else(|e| panic!("{name}.toml must exist in data/fixtures: {e}"));
            metal_vfs::write(fixture::dir(&root).join(format!("{name}.toml")), text).unwrap();
        }
        root
    }

    /// A full row: the signal block, then the effects block (zeros) — the
    /// declared order (`[model own][signal][effects]`), laid out in
    /// `SIGNAL_FLOATS` order so `read_signal` finds every float where it
    /// expects. Only the named ones are set; the rest keep their defaults.
    fn signal_row(level: f32, hue: f32, sat: f32, cct: f32) -> Vec<f32> {
        let mut sig: Vec<f32> = metal_schema::SIGNAL_FLOATS.iter().map(|s| s.default).collect();
        sig[0] = level;
        sig[1] = hue;
        sig[2] = sat;
        sig[8] = cct;
        let mut row = sig;
        row.extend(std::iter::repeat(0.0f32).take(metal_schema::EFFECTS_WIDTH));
        row
    }

    /// A `metal_ble::open` stand-in that hands back a `Recorder` and keeps its
    /// log, so the test can read what the frame actually pushed.
    fn recording_factory() -> (Box<LinkFactory>, Arc<Mutex<Vec<metal_ble::RecorderLog>>>) {
        let logs: Arc<Mutex<Vec<metal_ble::RecorderLog>>> = Arc::new(Mutex::new(Vec::new()));
        let inner = logs.clone();
        let factory: Box<LinkFactory> = Box::new(move |_cfg| {
            let (rec, log) = metal_ble::Recorder::new();
            inner.lock().unwrap().push(log);
            Box::new(rec) as Box<dyn metal_ble::Link>
        });
        (factory, logs)
    }

    /// A graph with one `out.bluetooth` node, a `signal` cable from a source,
    /// and the given node config.
    fn bluetooth_graph(profile: &str) -> metal_graph::Graph {
        let mut g = metal_graph::Graph::new("device-ble", "t", metal_graph::Domain::Live);
        let out = g.add_node("out", "out.bluetooth", [0.0, 0.0]);
        g.node_mut(&out).unwrap().config = metal_graph::Config::from_json_str(&format!(
            r#"{{"profile":"{profile}","address":"","mesh":"mesh.json"}}"#
        ))
        .unwrap();
        g.add_edge(
            metal_graph::Endpoint::new("src", "row"),
            metal_graph::Endpoint::new(&out, "signal"),
        );
        g
    }

    /// One frame's row for a source, and the `drive_frame` call that pushes it.
    #[allow(clippy::too_many_arguments)]
    fn push_frame(
        graph: &metal_graph::Graph,
        root: &Path,
        row: Vec<f32>,
        links: &mut HashMap<NodeId, BluetoothLink>,
        factory: &LinkFactory,
    ) -> u64 {
        let mut script_rows = HashMap::new();
        script_rows.insert("src".to_string(), row);
        let head = Head { fixture: "no-head".into(), universe: None, address: None };
        let mut sockets = HashMap::new();
        drive_frame(
            graph,
            &head,
            root,
            None,
            &HashMap::new(),
            &script_rows,
            &[],
            &mut sockets,
            links,
            factory,
            Instant::now(),
        )
    }

    #[test]
    fn all_four_phase_b_profiles_parse() {
        let root = fixtures_root("parse");
        for (model, wire, signals) in [
            ("moving-head-beam", fixture::WireKind::Dmx, vec!["pan", "tilt", "level", "hue", "sat", "strobe", "zoom", "focus", "iris", "frost"]),
            ("godox-sl60iid", fixture::WireKind::Bluetooth, vec!["level"]),
            ("godox-sl60iibi", fixture::WireKind::Bluetooth, vec!["level", "cct"]),
            ("godox-rgb", fixture::WireKind::Bluetooth, vec!["level", "hue", "sat", "cct"]),
        ] {
            let f = fixture::get(&root, model).unwrap_or_else(|| panic!("{model} must parse"));
            assert_eq!(f.wire, wire, "{model} wire");
            let names: Vec<&str> = f.channels.iter().map(|c| c.signal.name()).collect();
            assert_eq!(names, signals, "{model} channels");
            for c in &f.channels {
                match wire {
                    fixture::WireKind::Dmx => assert!(c.coarse.is_some(), "{model} DMX channel needs a coarse slot"),
                    fixture::WireKind::Bluetooth => {
                        assert!(c.coarse.is_none(), "{model} Bluetooth channel has no slot");
                        assert!(c.fine.is_none());
                    }
                }
            }
        }
        let bi = fixture::get(&root, "godox-sl60iibi").unwrap();
        assert!(bi.bluetooth.is_some(), "a Bluetooth profile carries its [bluetooth] table");
        let cct = bi.channels.iter().find(|c| c.signal == fixture::Selector::Cct).unwrap();
        assert_eq!(cct.signal_range(), [2800.0, 6500.0]);
        metal_vfs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_dmx_profile_without_coarse_is_refused_and_bluetooth_with_one_is_too() {
        let dmx_no_coarse = "[[channel]]\nsignal = \"level\"\n";
        assert!(fixture::parse("bad", dmx_no_coarse).is_err(), "DMX still requires coarse");

        let bt_with_coarse = "wire = \"bluetooth\"\n[bluetooth]\nprotocol = \"godox-mesh\"\n\n[[channel]]\nsignal = \"level\"\ncoarse = 0\n";
        assert!(fixture::parse("bad", bt_with_coarse).is_err(), "Bluetooth rejects a slot");

        let bt_no_table = "wire = \"bluetooth\"\n\n[[channel]]\nsignal = \"level\"\n";
        assert!(fixture::parse("bad", bt_no_table).is_err(), "Bluetooth needs [bluetooth]");
    }

    #[test]
    fn the_beam_head_packs_zoom_halfway_into_a_dmx_byte() {
        let root = fixtures_root("slots");
        let f = fixture::get(&root, "moving-head-beam").unwrap();
        let head = Head { fixture: "moving-head-beam".into(), universe: None, address: Some(1) };
        let mut sig = metal_schema::Signal {
            level: 1.0,
            hue: 0.0,
            sat: 0.0,
            strobe: 0.0,
            pan: 0.0,
            tilt: 0.0,
            a: 0.0,
            b: 0.0,
            cct: 5600.0,
            zoom: 0.5,
            focus: 0.25,
            iris: 0.0,
            frost: 1.0,
        };
        let frame = f.frame(&head, &sig);
        // zoom is relative slot 8 -> absolute 9; 0.5 over 0..1 -> 127.5 -> 128.
        assert!(frame.contains(&(9, 128)), "zoom 0.5 -> 128: {frame:?}");
        // focus slot 9 -> abs 10; 0.25 -> 63.75 -> 64.
        assert!(frame.contains(&(10, 64)), "focus 0.25 -> 64: {frame:?}");
        // frost slot 11 -> abs 12; 1.0 -> 255.
        assert!(frame.contains(&(12, 255)), "frost 1.0 -> 255: {frame:?}");

        // And the whole frame is the profile's ten channels, pan/tilt 16-bit.
        assert_eq!(frame.len(), 12, "8 single-byte channels + pan/tilt pairs");
        sig.zoom = 0.0;
        assert!(f.frame(&head, &sig).contains(&(9, 0)));
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// Float authoring by time: a lane's `sig_level` keyframed 0 -> 1 over one
    /// second, driven at t = 0, 0.5, 1 through the SL60II D profile, lands in
    /// the `Recorder` as 0, 0.5, 1.
    #[test]
    fn a_keyframed_level_reaches_the_recorder_as_0_half_1() {
        let root = fixtures_root("level");
        let graph = bluetooth_graph("godox-sl60iid");
        let (factory, logs) = recording_factory();
        let mut links = HashMap::new();

        // The lane: two keyframes, linear between them — t in 0..1 is the
        // level, which is what "signed off sig_level by the clock" means.
        let lane = |t: f32| signal_row(t, 0.0, 0.0, 5600.0);
        let mut errors = 0;
        for t in [0.0f32, 0.5, 1.0] {
            errors += push_frame(&graph, &root, lane(t), &mut links, &*factory);
        }
        assert_eq!(errors, 0, "a connected Recorder never fails");

        let states = logs.lock().unwrap()[0].states();
        assert_eq!(states.len(), 3, "one push per frame");
        let levels: Vec<f32> = states.iter().map(|s| s.level).collect();
        for (got, want) in levels.iter().zip([0.0, 0.5, 1.0]) {
            assert!((got - want).abs() < 1e-6, "level {got} != {want}");
        }
        assert!(states.iter().all(|s| s.cct_kelvin.is_none()), "SL60II D has no cct channel");
        assert!(states.iter().all(|s| s.hsi.is_none()), "no hue/sat channels");
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// The same authoring on `sig_cct`: 2800 -> 6500 K over one second, through
    /// the SL60II Bi, becomes the lamp's own kelvin, clamped to its window.
    #[test]
    fn a_keyframed_cct_reaches_the_recorder_in_kelvin() {
        let root = fixtures_root("cct");
        let graph = bluetooth_graph("godox-sl60iibi");
        let (factory, logs) = recording_factory();
        let mut links = HashMap::new();

        let lane = |t: f32| signal_row(1.0, 0.0, 0.0, 2800.0 + t * (6500.0 - 2800.0));
        let mut errors = 0;
        for t in [0.0f32, 0.5, 1.0] {
            errors += push_frame(&graph, &root, lane(t), &mut links, &*factory);
        }
        assert_eq!(errors, 0);

        let states = logs.lock().unwrap()[0].states();
        let ccts: Vec<f32> = states.iter().map(|s| s.cct_kelvin.expect("Bi has a cct channel")).collect();
        for (got, want) in ccts.iter().zip([2800.0, 4650.0, 6500.0]) {
            assert!((got - want).abs() < 1e-3, "cct {got} != {want}");
        }

        // A row asking for a temperature past the lamp's window is clamped to
        // it, not sent.
        let over = push_frame(&graph, &root, signal_row(1.0, 0.0, 0.0, 9000.0), &mut links, &*factory);
        assert_eq!(over, 0);
        assert_eq!(logs.lock().unwrap()[0].last().unwrap().cct_kelvin, Some(6500.0));
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// A hue sweep on the RGB profile: 0 -> 0.5 over one second becomes 0, 90
    /// and 180 degrees, with saturation carried through.
    #[test]
    fn a_keyframed_hue_sweep_reaches_the_recorder_in_degrees() {
        let root = fixtures_root("hue");
        let graph = bluetooth_graph("godox-rgb");
        let (factory, logs) = recording_factory();
        let mut links = HashMap::new();

        let lane = |t: f32| signal_row(1.0, t * 0.5, 1.0, 5600.0);
        let mut errors = 0;
        for t in [0.0f32, 0.5, 1.0] {
            errors += push_frame(&graph, &root, lane(t), &mut links, &*factory);
        }
        assert_eq!(errors, 0);

        let states = logs.lock().unwrap()[0].states();
        let hues: Vec<f32> = states.iter().map(|s| s.hsi.expect("RGB has hsi").hue_deg).collect();
        for (got, want) in hues.iter().zip([0.0, 90.0, 180.0]) {
            assert!((got - want).abs() < 1e-3, "hue {got} != {want}");
        }
        assert!(states.iter().all(|s| (s.hsi.unwrap().sat - 1.0).abs() < 1e-6));
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// A missing profile, a missing row, or a bypassed node is a skip, never a
    /// crash and never a push.
    #[test]
    fn a_bluetooth_node_that_cannot_be_read_pushes_nothing() {
        let root = fixtures_root("skip");
        let (factory, logs) = recording_factory();

        // No such profile.
        let g = bluetooth_graph("not-a-profile");
        let mut links = HashMap::new();
        let e = push_frame(&g, &root, signal_row(1.0, 0.0, 0.0, 5600.0), &mut links, &*factory);
        assert_eq!(e, 0);
        assert!(links.is_empty(), "no link is even opened for an unknown profile");
        assert!(logs.lock().unwrap().is_empty());

        // Bypassed.
        let mut g2 = bluetooth_graph("godox-sl60iid");
        for n in g2.nodes.iter_mut() {
            if n.kind == "out.bluetooth" {
                n.bypass = true;
            }
        }
        let mut links2 = HashMap::new();
        let e = push_frame(&g2, &root, signal_row(1.0, 0.0, 0.0, 5600.0), &mut links2, &*factory);
        assert_eq!(e, 0);
        assert!(links2.is_empty(), "a bypassed node never opens a link");
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// A failed push is counted, exactly like a dropped sACN packet — the
    /// `Recorder` reports `false` when it is not `Connected`.
    #[test]
    fn a_failed_push_is_counted_not_swallowed() {
        let root = fixtures_root("counted");
        let graph = bluetooth_graph("godox-sl60iid");
        let (factory, _logs) = recording_factory();
        let _ = factory;
        // A factory whose link is permanently failed.
        let bad: Box<LinkFactory> = Box::new(|_cfg| {
            Box::new(metal_ble::Recorder::new().0.with_status(metal_ble::LinkStatus::Failed("no radio".into())))
                as Box<dyn metal_ble::Link>
        });
        let mut links = HashMap::new();
        let e = push_frame(&graph, &root, signal_row(1.0, 0.0, 0.0, 5600.0), &mut links, &*bad);
        assert_eq!(e, 1, "one failed push is one wire error");
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// A node that disappears from the graph drops its link, like an sACN socket.
    #[test]
    fn a_removed_node_drops_its_link() {
        let root = fixtures_root("drop");
        let graph = bluetooth_graph("godox-sl60iid");
        let (factory, _logs) = recording_factory();
        let mut links = HashMap::new();
        push_frame(&graph, &root, signal_row(1.0, 0.0, 0.0, 5600.0), &mut links, &*factory);
        assert_eq!(links.len(), 1);

        let empty = metal_graph::Graph::new("device-ble", "t", metal_graph::Domain::Live);
        let head = Head { fixture: "no-head".into(), universe: None, address: None };
        let mut sockets = HashMap::new();
        drive_frame(&empty, &head, &root, None, &HashMap::new(), &HashMap::new(), &[], &mut sockets, &mut links, &*factory, Instant::now());
        assert!(links.is_empty(), "the gone node's link was dropped");
        metal_vfs::remove_dir_all(&root).ok();
    }
}

/// M3: the `venue.fixtures` node — one packet per wire, a group limits what is
/// driven, and the packets go out through the same [`Sacn`]/[`Adalight`]
/// backends a frame-driven `out.*` node uses.
#[cfg(test)]
mod venue_tests {
    use super::*;

    fn data_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    fn bar() -> crate::venue::Venue {
        crate::venue::load(&data_root(), "bar").expect("the bar example")
    }

    /// A 2x1 frame: red on the left, blue on the right.
    fn frame() -> (Vec<u8>, u32, u32) {
        (vec![255, 0, 0, 255, 0, 0, 255, 255], 2, 1)
    }

    /// The fields of two packets that carry a wire's bytes, for equality when
    /// [`VenuePacket`] itself does not derive `PartialEq`.
    fn same_packet(a: &VenuePacket, b: &VenuePacket) -> bool {
        a.wire == b.wire && a.key == b.key && a.universe == b.universe && a.slots == b.slots && a.pixels == b.pixels
    }

    /// F2: a `venue.fixture` node restricted to one row produces exactly the
    /// packet the group node produces when that row is the only one it drives.
    #[test]
    fn a_fixture_node_produces_the_group_packet_for_that_fixture() {
        let root = data_root();
        let mut v = bar();
        let first = v.fixtures.iter().find(|f| f.wire == "dmx").expect("a wash").id.clone();
        for f in v.fixtures.iter_mut() {
            f.group = Some(if f.id == first { "solo" } else { "other" }.to_string());
        }
        let (fr, w, h) = frame();
        let group = venue_packets(&v, Some("solo"), &fr, w, h, &root);
        let one = fixture_packets(&v, &first, &fr, w, h, &root);
        assert_eq!(group.len(), 1, "one wire for the solo wash");
        assert_eq!(one.len(), 1, "one wire for the fixture node");
        assert!(
            same_packet(&group[0], &one[0]),
            "one fixture is the group restricted to it: {:?} vs {:?}",
            group[0].slots,
            one[0].slots
        );
        assert!(
            fixture_packets(&v, "gone", &fr, w, h, &root).is_empty(),
            "a row that is gone drives nothing"
        );
    }

    /// F2: a bypassed `venue.fixture` node writes nothing to the bus; un-bypass
    /// it and its universe appears.
    #[test]
    fn a_bypassed_fixture_node_sends_nothing() {
        let root = data_root();
        let mut g = metal_graph::Graph::new("device-fx", "t", metal_graph::Domain::Live);
        let n = g.add_node("f", metal_graph::VENUE_FIXTURE_KIND, [0.0, 0.0]);
        g.node_mut(&n).unwrap().config = metal_graph::Config::from_json_str(
            &serde_json::json!({ "venue": "bar", "fixture": "wash-1", "priority": 200 }).to_string(),
        )
        .unwrap();
        g.node_mut(&n).unwrap().bypass = true;

        let (fr, w, h) = frame();
        let mut wires = HashMap::new();
        let mut bus = DmxBus::new();
        let now = Instant::now();
        drive_sampled_bus(&g, &fr, w, h, &root, &mut wires, &mut bus, now);
        assert!(bus.resolve(0, now).is_none(), "a bypassed node writes nothing");

        g.node_mut(&n).unwrap().bypass = false;
        drive_sampled_bus(&g, &fr, w, h, &root, &mut wires, &mut bus, now);
        assert!(bus.resolve(0, now).is_some(), "un-bypassed, its universe is live");
    }

    /// F2: a `venue.fixture` node and a group node on one universe resolve by
    /// priority — the higher-priority fixture node owns the universe, so the
    /// fixture only it drives goes dark under it.
    #[test]
    fn priority_resolves_a_fixture_node_against_a_group_node() {
        let root = data_root();
        let mut g = metal_graph::Graph::new("device-fx", "t", metal_graph::Domain::Live);
        // The whole venue at priority 100 drives wash-1 and wash-2 (universe 0).
        let group = g.add_node("group", metal_graph::VENUE_FIXTURES_KIND, [0.0, 0.0]);
        g.node_mut(&group).unwrap().config =
            metal_graph::Config::from_json_str(&serde_json::json!({ "venue": "bar", "priority": 100 }).to_string()).unwrap();
        // One fixture at priority 200 drives only wash-1.
        let one = g.add_node("one", metal_graph::VENUE_FIXTURE_KIND, [1.0, 0.0]);
        g.node_mut(&one).unwrap().config = metal_graph::Config::from_json_str(
            &serde_json::json!({ "venue": "bar", "fixture": "wash-1", "priority": 200 }).to_string(),
        )
        .unwrap();

        // A white frame lights every wash at level 255.
        let (fr, w, h) = (vec![255u8; 2 * 4], 2, 1);
        let mut wires = HashMap::new();
        let mut bus = DmxBus::new();
        let now = Instant::now();
        drive_sampled_bus(&g, &fr, w, h, &root, &mut wires, &mut bus, now);

        let snap = bus.snapshot(now, 10);
        let universe = snap.iter().find(|u| u["universe"] == 0).expect("universe 0 is live");
        assert_eq!(universe["priority"], serde_json::json!(200), "the fixture node owns the universe");
        assert_eq!(universe["sources"].as_array().unwrap().len(), 2, "both nodes wrote it");

        // example-wash: address 20, span 4. wash-1 → 20, wash-2 → 24 (1-based).
        let bytes = bus.resolve(0, now).expect("a live universe");
        assert_ne!(bytes[19], 0, "wash-1's level is the winner's: {}", bytes[19]);
        assert_eq!(bytes[23], 0, "wash-2 is only the group's, so the winner zeroes it");
    }

    /// A digest frame with only a tone block set, so a colour test is about the
    /// tone mapping and not about chroma happening to agree.
    fn tone_frame(x: f32, y: f32) -> crate::digest::Frame {
        let mut f = [0.0f32; crate::digest::FEATURES];
        f[crate::digest::TONE_X] = x;
        f[crate::digest::TONE_Y] = y;
        f[crate::digest::TONE_REGISTER] = 0.5;
        f[crate::digest::TONE_DENSITY] = 0.5;
        f[crate::digest::TONE_MODE] = 0.5;
        f[crate::digest::TONE_CONF] = 1.0;
        crate::digest::Frame::step(f)
    }

    /// The first model with its own colour slot (not only the shared effects
    /// `leak_color`), its id and schema.
    fn a_model_with_a_colour_slot() -> (&'static str, &'static metal_schema::Schema) {
        let painter = crate::paint::painter();
        let (ids, schemas) = (painter.ids(), painter.schemas());
        ids.into_iter()
            .zip(schemas)
            .find(|(id, _)| model_colour_at(&format!("gen.{id}")).is_some())
            .expect("a model declares a colour slot")
    }

    /// A `venue.fixture` node pointing at one row, `config` merged over the
    /// pointer, wired to `model`'s `colour`.
    fn fixture_node(
        g: &mut metal_graph::Graph,
        id: &str,
        fixture: &str,
        config: serde_json::Value,
        model: &str,
    ) -> String {
        let node = g.add_node(id, metal_graph::VENUE_FIXTURE_KIND, [1.0, 0.0]);
        let mut cfg = serde_json::json!({ "venue": "bar", "fixture": fixture });
        for (k, v) in config.as_object().unwrap() {
            cfg[k] = v.clone();
        }
        g.node_mut(&node).unwrap().config =
            metal_graph::Config::from_json_str(&cfg.to_string()).unwrap();
        g.add_edge(
            metal_graph::Endpoint::new(model, "control.colour0"),
            metal_graph::Endpoint::new(&node, "colour"),
        );
        node
    }

    /// F4: a model's driven colour slot — the row `drive::row_with` filled — is
    /// read back on a `venue.fixture` node's `colour` input and reaches the wash
    /// profile's `hue` byte. The tone block sits at a known angle, so the whole
    /// chain from tone to byte is checked against the slot colour's own hue.
    #[test]
    fn a_models_driven_colour_slot_reaches_the_wash_hue_byte() {
        let root = data_root();
        let (id, schema) = a_model_with_a_colour_slot();
        let base = schema.defaults();
        let tone = tone_frame(0.5, 0.5);
        let drive = crate::drive::Drive { palette: crate::drive::Palette::Spectrum, ..Default::default() };
        let row = crate::drive::row(schema, &base, &tone, &drive);
        let at = model_colour_at(&format!("gen.{id}")).expect("the exposed colour slot offset");
        let slot_rgb = [row[at], row[at + 1], row[at + 2]];

        let mut g = metal_graph::Graph::new("device-fx", "t", metal_graph::Domain::Live);
        let model = g.add_node("model", &format!("gen.{id}"), [0.0, 0.0]);
        fixture_node(&mut g, "fx", "wash-1", serde_json::json!({ "colour_from": "model" }), &model);

        let rows = HashMap::from([(model.clone(), row)]);
        let bases = HashMap::from([(model, base)]);
        let source = SourceRows { rows: Some(&rows), base: Some(&bases), script_rows: None, raw: &tone.raw };
        let (px, w, h) = frame();
        let mut wires = HashMap::new();
        let mut bus = DmxBus::new();
        let now = Instant::now();
        drive_sampled_bus_with(&g, &px, w, h, &root, &source, &mut wires, &mut bus, now);

        let bytes = bus.resolve(0, now).expect("wash-1's universe is live");
        let expected = (crate::venue::light_floats(slot_rgb)[0] * 255.0).round() as u8;
        // example-wash: level at 20, hue at 21 (1-based) → 0-based 20.
        assert_eq!(bytes[20], expected, "the wash's hue byte is the model slot's hue");
    }

    /// F4: `hue_lock = "hold"` keeps the source model's own base hue for every
    /// model, taking only its level/chroma — the `hold_keeps_each_models_own_hue`
    /// comparison, on the fixture side. `free` leaves the driven hue, which the
    /// spectrum palette must actually move (or the comparison proves nothing).
    #[test]
    fn hold_keeps_each_models_own_hue_on_a_fixture() {
        use metal_notation::color::from_srgb01;
        let hue_gap = |a: f32, b: f32| ((a - b + 540.0) % 360.0 - 180.0).abs();
        let painter = crate::paint::painter();
        let (ids, schemas) = (painter.ids(), painter.schemas());
        let mut moved = 0;
        for (id, schema) in ids.into_iter().zip(schemas) {
            // Only a model with its own colour slot (the effects `leak_color`
            // is not one the graph exposes).
            let Some(start) = model_colour_at(&format!("gen.{id}")) else { continue };
            let mut base = schema.defaults();
            // Red, green, blue, yellow: a definite hue in every slot.
            base[start..start + 12].copy_from_slice(&[
                0.9, 0.2, 0.2, 0.2, 0.8, 0.3, 0.2, 0.3, 0.9, 0.9, 0.8, 0.2,
            ]);
            let mut tone = tone_frame(0.5, 0.5);
            tone.raw[crate::digest::TONE_DENSITY] = 0.8;
            tone.f[crate::digest::TONE_DENSITY] = 0.8;
            let drive = crate::drive::Drive {
                palette: crate::drive::Palette::Spectrum,
                depth: 0.0,
                ..Default::default()
            };
            let driven = crate::drive::row(schema, &base, &tone, &drive);
            for i in 0..4 {
                let dr = [driven[start + i * 3], driven[start + i * 3 + 1], driven[start + i * 3 + 2]];
                let own = [base[start + i * 3], base[start + i * 3 + 1], base[start + i * 3 + 2]];
                let held = crate::drive::hold_hue(dr, own);
                let want = from_srgb01(own).h;
                let got = from_srgb01(held).h;
                assert!(hue_gap(want, got) < 6.0, "{id}: hold moved slot {i} hue {want} -> {got}");
                // `colour_to_signal` carries exactly that held colour.
                let [hue, _, _] = crate::fixture::colour_to_signal(dr, Some(own), "hold");
                assert!((hue - crate::venue::light_floats(held)[0]).abs() < 1e-5);
                let [free, _, _] = crate::fixture::colour_to_signal(dr, Some(own), "free");
                if hue_gap(free * 360.0, want) > 30.0 {
                    moved += 1;
                }
            }
        }
        assert!(moved > 0, "the spectrum palette never moved a hue, so the comparison proves nothing");
    }

    /// F4: `spread = "slots"` makes four fixtures read four slots — the rig
    /// carries the model's four-colour gradient as four distinct hues (the
    /// fixture-side answer to `n_stops_grows_with_spread`). `spread = "same"`
    /// makes them equal.
    #[test]
    fn spread_slots_gives_four_distinct_hues_across_a_rig() {
        let root = data_root();
        let colours = [[0.9, 0.2, 0.2], [0.2, 0.8, 0.3], [0.2, 0.3, 0.9], [0.9, 0.8, 0.2]];
        let mut base = [0.0f32; 12];
        for (i, c) in colours.iter().enumerate() {
            base[i * 3..i * 3 + 3].copy_from_slice(c);
        }

        let (id, _) = a_model_with_a_colour_slot();
        let mut g = metal_graph::Graph::new("device-fx", "t", metal_graph::Domain::Live);
        let model = g.add_node("model", &format!("gen.{id}"), [0.0, 0.0]);
        for i in 0..4 {
            fixture_node(
                &mut g,
                &format!("f{i}"),
                &format!("wash-{}", i + 1),
                serde_json::json!({ "colour_from": "model", "spread": "slots" }),
                &model,
            );
        }
        // One slot offset: 12 floats, no floats block to find it behind — the
        // model node here is only the graph's colour source.
        let at = model_colour_at(&format!("gen.{id}")).unwrap();
        let mut row_full = vec![0.0f32; at + 12];
        row_full[at..at + 12].copy_from_slice(&base);
        let rows = HashMap::from([(model.clone(), row_full.clone())]);
        let bases = HashMap::from([(model, row_full)]);
        let source = SourceRows { rows: Some(&rows), base: Some(&bases), script_rows: None, raw: &[] };
        let (px, w, h) = frame();
        let mut wires = HashMap::new();
        let mut bus = DmxBus::new();
        let now = Instant::now();
        drive_sampled_bus_with(&g, &px, w, h, &root, &source, &mut wires, &mut bus, now);

        let bytes = bus.resolve(0, now).expect("the washes' universe");
        // example-wash: level 0, hue 1 each 4 slots apart from 20.
        let hues: Vec<u8> = (0..4).map(|i| bytes[20 + i * 4]).collect();
        let distinct: std::collections::HashSet<u8> = hues.iter().copied().collect();
        assert_eq!(distinct.len(), 4, "four slots, four hues: {hues:?}");
        for (i, c) in colours.iter().enumerate() {
            let want = (crate::venue::light_floats(*c)[0] * 255.0).round() as u8;
            assert_eq!(hues[i], want, "fixture {i} carries slot {i}'s hue");
        }

        // `same` collapses the rig to one hue.
        let mut g2 = metal_graph::Graph::new("device-same", "t", metal_graph::Domain::Live);
        let model2 = g2.add_node("model", &format!("gen.{id}"), [0.0, 0.0]);
        for i in 0..4 {
            fixture_node(
                &mut g2,
                &format!("f{i}"),
                &format!("wash-{}", i + 1),
                serde_json::json!({ "colour_from": "model", "spread": "same", "pick": 2 }),
                &model2,
            );
        }
        let rows2 = HashMap::from([(model2.clone(), {
            let mut r = vec![0.0f32; at + 12];
            r[at..at + 12].copy_from_slice(&base);
            r
        })]);
        let source2 = SourceRows { rows: Some(&rows2), base: None, script_rows: None, raw: &[] };
        let mut bus2 = DmxBus::new();
        drive_sampled_bus_with(&g2, &px, w, h, &root, &source2, &mut wires, &mut bus2, now);
        let bytes2 = bus2.resolve(0, now).expect("the same-hue universe");
        let hues2: Vec<u8> = (0..4).map(|i| bytes2[20 + i * 4]).collect();
        assert_eq!(hues2, vec![hues[2]; 4], "spread=same takes one slot for all");
    }

    #[test]
    fn a_fixtures_node_sends_one_packet_per_wire() {
        let root = data_root();
        let v = bar();
        let (f, w, h) = frame();
        let packets = venue_packets(&v, None, &f, w, h, &root);
        // The bar's four washes share one sACN universe; its strip is one
        // serial wire. Two wires, two packets.
        assert_eq!(packets.len(), 2, "one packet per wire: {:?}", packets.iter().map(|p| &p.wire).collect::<Vec<_>>());
        let sacn = packets.iter().find(|p| p.wire == "sacn").expect("the washes' universe");
        let serial = packets.iter().find(|p| p.wire == "serial").expect("the strip's port");

        // Actually send both through the real wire backends to a fake
        // receiver — never real hardware.
        let recv = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        recv.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let mut wire = Sacn::new(Some(recv.local_addr().unwrap())).unwrap();
        assert!(send_venue_packet(sacn, Some(&mut wire), None, None), "the sACN packet goes out");
        let mut buf = [0u8; 700];
        let (n, _) = recv.recv_from(&mut buf).expect("one E1.31 packet");
        assert_eq!(n, 638, "a full sACN frame");
        assert_eq!(u16::from_be_bytes([buf[113], buf[114]]), sacn.universe, "the universe the packet names");

        let path = std::env::temp_dir().join(format!(
            "fw-venue-serial-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        metal_vfs::write(&path, b"").unwrap();
        let cfg = SerialConfig {
            port: path.to_string_lossy().into_owned(),
            pixels: Some(serial.pixels),
            ..Default::default()
        };
        let mut strip = Adalight::open(&cfg).unwrap();
        assert!(send_venue_packet(serial, None, Some(&mut strip), None), "the serial packet goes out");
        let written = metal_vfs::read(&path).unwrap();
        assert!(written.starts_with(b"Ada"), "one Adalight packet: {written:?}");
        let _ = metal_vfs::remove_file(&path);
    }

    #[test]
    fn a_group_limits_the_fixtures_driven() {
        let root = data_root();
        let mut v = bar();
        for f in v.fixtures.iter_mut() {
            f.group = Some(match f.kind {
                crate::venue::FixtureKind::LedStrip => "edge",
                _ => "front-wash",
            }
            .to_string());
        }
        let (f, w, h) = frame();
        assert_eq!(v.group(Some("front-wash")).len(), 4, "four washes");

        let front = venue_packets(&v, Some("front-wash"), &f, w, h, &root);
        assert_eq!(front.len(), 1, "the washes are one sACN wire");
        assert_eq!(front[0].wire, "sacn");
        assert!(!front[0].slots.is_empty(), "the washes are packed, not empty");

        let edge = venue_packets(&v, Some("edge"), &f, w, h, &root);
        assert_eq!(edge.len(), 1, "the strip is one serial wire");
        assert_eq!(edge[0].wire, "serial");
        let strip = v.fixtures.iter().find(|f| f.kind == crate::venue::FixtureKind::LedStrip).unwrap();
        assert_eq!(edge[0].pixels, v.pixel_count(strip).unwrap());

        assert!(venue_packets(&v, Some("nope"), &f, w, h, &root).is_empty(), "an empty group drives nothing");
    }

    /// D4: a matrix fixture is sampled once per cell and writes each cell's
    /// RGB to its own mapped channels — a frame colour at a cell lands on the
    /// channel byte the scan order puts there.
    #[test]
    fn a_matrix_fixture_writes_each_cell_to_its_channels() {
        let root = std::env::temp_dir().join(format!(
            "fw-venue-matrix-wire-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = metal_vfs::remove_dir_all(&root);
        let mut v = bar();
        let projector = v.projectors.first().expect("bar has a projector").id.clone();
        let m = crate::venue::Matrix {
            x_pixels: 2,
            y_pixels: 2,
            first_channels: 1,
            ..Default::default()
        };
        let mut f = crate::venue::Fixture::new(
            "media".into(),
            "media".into(),
            crate::venue::FixtureKind::Matrix,
        );
        f.on = Some("back-wall".into());
        f.wire = "sacn".into();
        f.universe = Some(1);
        f.address = Some(1);
        f.projector = Some(projector);
        f.matrix = Some(m.clone());
        v.fixtures.push(f.clone());

        let (frame, w, h) = frame();
        let packets = venue_packets(&v, None, &frame, w, h, &root);
        let packet = packets
            .iter()
            .find(|p| p.wire == "sacn" && p.universe == 1)
            .expect("the matrix universe");
        let channels = m.pixel_channels();
        let points = v.matrix_points(&f).unwrap();
        let mut checked = 0;
        for (cell, offset) in channels.iter().enumerate() {
            if *offset < 0 {
                continue;
            }
            let rgb = crate::venue::sample_rgb(&frame, w, h, points[cell]);
            for (c, v) in rgb.iter().enumerate() {
                let (u, a) = m.channel_at(1, 1, offset + c as i16);
                assert_eq!(u, 1, "every cell lands in the fixture's first universe");
                let got = packet
                    .slots
                    .iter()
                    .find(|(addr, _)| *addr == a)
                    .map(|(_, b)| *b)
                    .unwrap_or_else(|| panic!("cell {cell} channel {c} at address {a}"));
                assert_eq!(got, (v.clamp(0.0, 1.0) * 255.0).round() as u8);
                checked += 1;
            }
        }
        assert_eq!(checked, m.cells() * crate::venue::MATRIX_CHANNELS_PER_PIXEL);
        let _ = metal_vfs::remove_dir_all(&root);
    }

    /// D4: a matrix whose map runs past slot 512 wraps into the next universe,
    /// as its own packet, and every cell is still driven.
    #[test]
    fn a_matrix_past_slot_512_wraps_into_the_next_universe() {
        let root = std::env::temp_dir().join(format!(
            "fw-venue-matrix-wrap-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = metal_vfs::remove_dir_all(&root);
        let mut v = bar();
        let projector = v.projectors.first().expect("bar has a projector").id.clone();
        let m = crate::venue::Matrix {
            x_pixels: 2,
            y_pixels: 2,
            ..Default::default()
        };
        let mut f = crate::venue::Fixture::new(
            "wrap".into(),
            "wrap".into(),
            crate::venue::FixtureKind::Matrix,
        );
        f.on = Some("back-wall".into());
        f.wire = "sacn".into();
        f.universe = Some(1);
        f.address = Some(510);
        f.projector = Some(projector);
        f.matrix = Some(m.clone());
        v.fixtures.push(f);

        let (frame, w, h) = frame();
        let packets = venue_packets(&v, None, &frame, w, h, &root);
        let one = packets
            .iter()
            .find(|p| p.wire == "sacn" && p.universe == 1)
            .expect("the fixture's first universe");
        let two = packets
            .iter()
            .find(|p| p.wire == "sacn" && p.universe == 2)
            .expect("the wrapped universe");
        let channels = m.pixel_channels();
        for offset in channels.iter().filter(|o| **o >= 0) {
            for c in 0..crate::venue::MATRIX_CHANNELS_PER_PIXEL as i16 {
                let (u, a) = m.channel_at(1, 510, offset + c);
                let packet = if u == 1 { one } else { two };
                assert!(
                    packet.slots.iter().any(|(addr, _)| *addr == a),
                    "universe {u} address {a} is driven"
                );
            }
        }
        let _ = metal_vfs::remove_dir_all(&root);
    }
}
