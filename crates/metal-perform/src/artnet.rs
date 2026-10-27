//! P-D — Art-Net 4, hand-built like [`crate::wire::Sacn`]: no crate, just the
//! bytes the Art-Net 4 specification (Artistic Licence, rev. 1.4) defines.
//!
//! What lives here is the wire format and one socket: [`ArtNet`] is the
//! second network [`Wire`] (`out.artnet`), sending ArtDmx to a 15-bit
//! Port-Address (Net 7 bits · Sub-Net 4 · Universe 4), unicast or broadcast,
//! with an optional ArtSync after each frame. [`discover`] is ArtPoll →
//! ArtPollReply (the properties page's "buscar nodos"), and
//! [`parse_timecode`]/[`timecode_packet`] are ArtTimeCode, which
//! `clock.timecode` reads (`crate::timecode`).
//!
//! **No new crate, measured** (rule 11): an ArtDmx is an 18-byte header and
//! the slots; ArtPoll is 14 bytes; the parse of a reply is a dozen fixed
//! offsets. `artnet_protocol` 0.4 would add `bitflags`, `byteorder` and its
//! own error types for the same three packets.
//!
//! Never blocks the frame: the socket is non-blocking and [`Wire::send`]
//! answers `bool`, exactly like sACN's.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use web_time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::wire::Wire;

/// Art-Net's one UDP port (0x1936).
pub const ARTNET_PORT: u16 = 6454;
const ID: &[u8; 8] = b"Art-Net\0";
const PROT_VER: u16 = 14;

pub const OP_POLL: u16 = 0x2000;
pub const OP_POLL_REPLY: u16 = 0x2100;
pub const OP_DMX: u16 = 0x5000;
pub const OP_SYNC: u16 = 0x5200;
pub const OP_TIMECODE: u16 = 0x9700;

/// The node kind this module backs.
pub const ARTNET_KIND: &str = "out.artnet";

/// The receiving node kind ([`ArtNetInConfig`]).
pub const ARTNET_IN_KIND: &str = metal_graph::ARTNET_IN_KIND;

/// The largest Port-Address: 15 bits.
pub const MAX_PORT_ADDRESS: u16 = 0x7fff;

fn header(op: u16) -> Vec<u8> {
    let mut p = Vec::with_capacity(530);
    p.extend_from_slice(ID);
    p.extend_from_slice(&op.to_le_bytes());
    p.extend_from_slice(&PROT_VER.to_be_bytes());
    p
}

/// The OpCode of an Art-Net packet, or `None` when `buf` is not one.
pub fn opcode(buf: &[u8]) -> Option<u16> {
    if buf.len() < 10 || &buf[..8] != ID {
        return None;
    }
    Some(u16::from_le_bytes([buf[8], buf[9]]))
}

/// The 15-bit Port-Address from its three parts; parts out of range are
/// masked to their width.
pub fn port_address(net: u8, subnet: u8, universe: u8) -> u16 {
    (u16::from(net & 0x7f) << 8) | (u16::from(subnet & 0x0f) << 4) | u16::from(universe & 0x0f)
}

/// `(net, subnet, universe)` of a Port-Address.
pub fn split_port_address(pa: u16) -> (u8, u8, u8) {
    (((pa >> 8) & 0x7f) as u8, ((pa >> 4) & 0x0f) as u8, (pa & 0x0f) as u8)
}

/// One ArtDmx: header, sequence, physical 0, SubUni, Net, length, data.
/// The length is the highest addressed slot rounded up to even (the spec
/// wants 2..=512, even); slots outside `1..=512` are dropped.
pub fn dmx_packet(sequence: u8, port_address: u16, slots: &[(u16, u8)]) -> Vec<u8> {
    let mut dmx = [0u8; 512];
    let mut top = 0usize;
    for &(address, byte) in slots {
        if (1..=512).contains(&address) {
            dmx[(address - 1) as usize] = byte;
            top = top.max(address as usize);
        }
    }
    let len = (top.max(2) + 1) & !1;
    let len = len.min(512);
    let pa = port_address & MAX_PORT_ADDRESS;
    let mut p = header(OP_DMX);
    p.push(sequence);
    p.push(0); // physical
    p.push((pa & 0xff) as u8); // SubUni
    p.push((pa >> 8) as u8); // Net
    p.extend_from_slice(&(len as u16).to_be_bytes());
    p.extend_from_slice(&dmx[..len]);
    p
}

/// `(sequence, port_address, data)` of an ArtDmx, `None` for anything else.
pub fn parse_dmx(buf: &[u8]) -> Option<(u8, u16, &[u8])> {
    if opcode(buf)? != OP_DMX || buf.len() < 18 {
        return None;
    }
    let pa = u16::from(buf[14]) | (u16::from(buf[15] & 0x7f) << 8);
    let len = u16::from_be_bytes([buf[16], buf[17]]) as usize;
    let data = buf.get(18..18 + len.min(512))?;
    Some((buf[12], pa, data))
}

/// ArtSync: tells every node to latch the ArtDmx it holds.
pub fn sync_packet() -> Vec<u8> {
    let mut p = header(OP_SYNC);
    p.extend_from_slice(&[0, 0]); // Aux1, Aux2
    p
}

/// ArtPoll: flags `0x02` (send a reply on change too), diag priority low.
pub fn poll_packet() -> Vec<u8> {
    let mut p = header(OP_POLL);
    p.push(0x02);
    p.push(0x10);
    p
}

/// One node that answered an ArtPoll.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtNode {
    pub ip: String,
    pub short_name: String,
    pub long_name: String,
    /// The Port-Addresses of its output ports (`net·subnet·SwOut`).
    pub outputs: Vec<u16>,
    /// The Port-Addresses of its input ports.
    pub inputs: Vec<u16>,
    pub oem: u16,
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

/// Reads an ArtPollReply (offsets of Art-Net 4 §ArtPollReply). `None` for a
/// packet that is not one or is shorter than the fields read.
pub fn parse_poll_reply(buf: &[u8]) -> Option<ArtNode> {
    if opcode(buf)? != OP_POLL_REPLY || buf.len() < 194 {
        return None;
    }
    let ip = Ipv4Addr::new(buf[10], buf[11], buf[12], buf[13]).to_string();
    let net = buf[18];
    let sub = buf[19];
    let oem = u16::from_be_bytes([buf[20], buf[21]]);
    let short_name = cstr(&buf[26..44]);
    let long_name = cstr(&buf[44..108]);
    let ports = (u16::from_be_bytes([buf[172], buf[173]]) as usize).min(4);
    let mut outputs = Vec::new();
    let mut inputs = Vec::new();
    for i in 0..ports {
        let kind = buf[174 + i];
        if kind & 0x80 != 0 {
            outputs.push(port_address(net, sub, buf[190 + i]));
        }
        if kind & 0x40 != 0 {
            inputs.push(port_address(net, sub, buf[186 + i]));
        }
    }
    Some(ArtNode { ip, short_name, long_name, outputs, inputs, oem })
}

/// Builds an ArtPollReply (239 bytes) — what a node sends; used by the tests'
/// fake node and by nothing else here.
pub fn poll_reply_packet(ip: Ipv4Addr, short: &str, long: &str, outputs: &[u16]) -> Vec<u8> {
    let mut p = vec![0u8; 239];
    p[..8].copy_from_slice(ID);
    p[8..10].copy_from_slice(&OP_POLL_REPLY.to_le_bytes());
    p[10..14].copy_from_slice(&ip.octets());
    p[14..16].copy_from_slice(&ARTNET_PORT.to_le_bytes());
    let first = outputs.first().copied().unwrap_or(0);
    let (net, sub, _) = split_port_address(first);
    p[18] = net;
    p[19] = sub;
    let s = short.as_bytes();
    p[26..26 + s.len().min(17)].copy_from_slice(&s[..s.len().min(17)]);
    let l = long.as_bytes();
    p[44..44 + l.len().min(63)].copy_from_slice(&l[..l.len().min(63)]);
    let n = outputs.len().min(4);
    p[172..174].copy_from_slice(&(n as u16).to_be_bytes());
    for (i, pa) in outputs.iter().take(4).enumerate() {
        p[174 + i] = 0x80;
        p[190 + i] = split_port_address(*pa).2;
    }
    p
}

/// ArtTimeCode type byte ↔ frame rate.
pub fn timecode_packet(tc: &crate::timecode::Timecode) -> Vec<u8> {
    let mut p = header(OP_TIMECODE);
    p.extend_from_slice(&[0, 0]); // filler, stream id
    p.extend_from_slice(&[tc.frames, tc.seconds, tc.minutes, tc.hours, tc.fps.artnet_type()]);
    p
}

/// Reads an ArtTimeCode.
pub fn parse_timecode(buf: &[u8]) -> Option<crate::timecode::Timecode> {
    if opcode(buf)? != OP_TIMECODE || buf.len() < 19 {
        return None;
    }
    let fps = crate::timecode::Fps::from_artnet_type(buf[18])?;
    Some(crate::timecode::Timecode { hours: buf[17], minutes: buf[16], seconds: buf[15], frames: buf[14], fps })
}

// ---------------------------------------------------------------------------
// config + the wire
// ---------------------------------------------------------------------------

/// `out.artnet`'s config: `{"net":0,"subnet":0,"universe":0,"unicast":
/// "host[:port]","broadcast":"255.255.255.255","sync":false}` (and, for a
/// frame-driven node, `heads` as on `out.sacn`). No `unicast` means
/// broadcast to `broadcast` (default the limited broadcast).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ArtNetConfig {
    #[serde(default)]
    pub net: u8,
    #[serde(default)]
    pub subnet: u8,
    #[serde(default)]
    pub universe: u8,
    #[serde(default)]
    pub unicast: Option<String>,
    #[serde(default)]
    pub broadcast: Option<String>,
    #[serde(default)]
    pub sync: bool,
}

impl ArtNetConfig {
    /// "Anything that does not parse is the default", as `SacnConfig`.
    pub fn from_node(node: &metal_graph::Node) -> ArtNetConfig {
        if node.config.is_empty() {
            return ArtNetConfig::default();
        }
        serde_json::from_str(&node.config.to_json_str()).unwrap_or_default()
    }

    pub fn port_address(&self) -> u16 {
        port_address(self.net, self.subnet, self.universe)
    }

    /// Where ArtDmx goes: the unicast target, else the broadcast address.
    pub fn destination(&self) -> SocketAddr {
        let parse = |raw: &str| -> Option<SocketAddr> {
            let raw = raw.trim();
            if raw.is_empty() {
                return None;
            }
            raw.parse::<SocketAddr>().ok().or_else(|| format!("{raw}:{ARTNET_PORT}").parse().ok())
        };
        self.unicast
            .as_deref()
            .and_then(parse)
            .or_else(|| self.broadcast.as_deref().and_then(parse))
            .unwrap_or(SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), ARTNET_PORT))
    }

    pub fn is_unicast(&self) -> bool {
        self.unicast.as_deref().is_some_and(|u| !u.trim().is_empty())
    }
}

/// Art-Net over UDP: one instance per `out.artnet` node.
pub struct ArtNet {
    socket: UdpSocket,
    sequence: u8,
    dest: SocketAddr,
    sync: bool,
}

impl ArtNet {
    pub fn new(cfg: &ArtNetConfig) -> std::io::Result<ArtNet> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;
        // Broadcast needs the flag; harmless for unicast.
        let _ = socket.set_broadcast(true);
        Ok(ArtNet { socket, sequence: 0, dest: cfg.destination(), sync: cfg.sync })
    }
}

impl Wire for ArtNet {
    /// `universe` is the 15-bit Port-Address.
    fn send(&mut self, universe: u16, slots: &[(u16, u8)]) -> bool {
        // Sequence 0 means "off" to a receiver; wrap 255 → 1.
        self.sequence = if self.sequence == 255 { 1 } else { self.sequence + 1 };
        let packet = dmx_packet(self.sequence, universe, slots);
        let ok = matches!(self.socket.send_to(&packet, self.dest), Ok(n) if n == packet.len());
        if ok && self.sync {
            let s = sync_packet();
            return matches!(self.socket.send_to(&s, self.dest), Ok(n) if n == s.len());
        }
        ok
    }
}

/// Binds UDP `port` with `SO_REUSEADDR` (unix) so discovery, `in`-side
/// timecode and another Art-Net app on this machine can share 6454.
pub fn bind_shared(port: u16) -> std::io::Result<UdpSocket> {
    #[cfg(unix)]
    {
        use std::os::unix::io::FromRawFd;
        // SAFETY: plain BSD socket calls on a descriptor this function owns
        // until it is handed to `UdpSocket`.
        unsafe {
            let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let one: libc::c_int = 1;
            let len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, &one as *const _ as *const libc::c_void, len);
            libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_BROADCAST, &one as *const _ as *const libc::c_void, len);
            let mut addr: libc::sockaddr_in = std::mem::zeroed();
            addr.sin_family = libc::AF_INET as libc::sa_family_t;
            addr.sin_port = port.to_be();
            addr.sin_addr = libc::in_addr { s_addr: 0 };
            if libc::bind(fd, &addr as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t) != 0 {
                let e = std::io::Error::last_os_error();
                libc::close(fd);
                return Err(e);
            }
            Ok(UdpSocket::from_raw_fd(fd))
        }
    }
    #[cfg(not(unix))]
    {
        let s = UdpSocket::bind(("0.0.0.0", port))?;
        let _ = s.set_broadcast(true);
        Ok(s)
    }
}

/// Sends one ArtPoll from `socket` to `dest` and collects every
/// ArtPollReply heard for `wait`, one entry per IP (the last wins).
pub fn discover_on(socket: &UdpSocket, dest: SocketAddr, wait: Duration) -> std::io::Result<Vec<ArtNode>> {
    socket.set_read_timeout(Some(Duration::from_millis(50)))?;
    socket.send_to(&poll_packet(), dest)?;
    let until = Instant::now() + wait;
    let mut found: Vec<ArtNode> = Vec::new();
    let mut buf = [0u8; 1024];
    while Instant::now() < until {
        match socket.recv_from(&mut buf) {
            Ok((n, _)) => {
                if let Some(node) = parse_poll_reply(&buf[..n]) {
                    found.retain(|f| f.ip != node.ip);
                    found.push(node);
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(e),
        }
    }
    found.sort_by(|a, b| a.ip.cmp(&b.ip));
    Ok(found)
}

/// Discovery on the real network: binds 6454 shared (nodes answer to that
/// port), falls back to an ephemeral port (nodes that answer the sender's
/// port still show), polls `dest` (the limited broadcast when `None`).
pub fn discover(dest: Option<SocketAddr>, wait: Duration) -> std::io::Result<Vec<ArtNode>> {
    let socket = match bind_shared(ARTNET_PORT) {
        Ok(s) => s,
        Err(_) => {
            let s = UdpSocket::bind("0.0.0.0:0")?;
            s.set_broadcast(true)?;
            s
        }
    };
    discover_on(&socket, dest.unwrap_or(SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), ARTNET_PORT)), wait)
}

// ---------------------------------------------------------------------------
// in.artnet — the receiver
// ---------------------------------------------------------------------------

/// `in.artnet`'s config: `{"net":0,"subnet":0,"universe":0,"first":1,
/// "count":16}`. `first` is the 1-based DMX channel the row starts at, `count`
/// how many values it holds (clamped to the 512 slots a universe has).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtNetInConfig {
    #[serde(default)]
    pub net: u8,
    #[serde(default)]
    pub subnet: u8,
    #[serde(default)]
    pub universe: u8,
    #[serde(default = "first_channel")]
    pub first: u16,
    #[serde(default = "channel_count")]
    pub count: u16,
}

fn first_channel() -> u16 {
    1
}

fn channel_count() -> u16 {
    16
}

impl Default for ArtNetInConfig {
    fn default() -> ArtNetInConfig {
        serde_json::from_str("{}").expect("all fields default")
    }
}

impl ArtNetInConfig {
    /// "Anything that does not parse is the default", as [`ArtNetConfig`].
    pub fn from_node(node: &metal_graph::Node) -> ArtNetInConfig {
        if node.config.is_empty() {
            return ArtNetInConfig::default();
        }
        serde_json::from_str(&node.config.to_json_str()).unwrap_or_default()
    }

    pub fn port_address(&self) -> u16 {
        port_address(self.net, self.subnet, self.universe)
    }

    /// `(first, count)` with `first` in `1..=512` and the row never running
    /// past slot 512.
    pub fn window(&self) -> (usize, usize) {
        let first = usize::from(self.first.clamp(1, 512));
        let count = usize::from(self.count).clamp(1, 513 - first);
        (first, count)
    }
}

/// The latest ArtDmx data per 15-bit Port-Address, 512 slots each (missing
/// slots read 0). Written by the listener thread, read by the frame.
type Frames = HashMap<u16, [u8; 512]>;

fn inbox() -> &'static Mutex<Frames> {
    static INBOX: OnceLock<Mutex<Frames>> = OnceLock::new();
    INBOX.get_or_init(Mutex::default)
}

fn listen_error_slot() -> &'static Mutex<Option<String>> {
    static ERR: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    ERR.get_or_init(Mutex::default)
}

/// What [`dispatch`] did with one datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dispatched {
    /// An ArtDmx, filed in its universe's inbox (`in.artnet` reads it).
    Dmx,
    /// An ArtTimeCode, sent to every [`subscribe_timecode`] receiver.
    TimeCode,
    /// Anything else (ArtPoll, ArtSync, a reply…) — ignored here.
    Other,
}

type TimecodeSubscribers = Mutex<Vec<Sender<crate::timecode::Timecode>>>;

fn timecode_subscribers() -> &'static TimecodeSubscribers {
    static SUBS: OnceLock<TimecodeSubscribers> = OnceLock::new();
    SUBS.get_or_init(Mutex::default)
}

/// A receiver of every ArtTimeCode the one listener hears. Dropping it
/// unsubscribes (the next send finds it gone).
pub fn subscribe_timecode() -> Receiver<crate::timecode::Timecode> {
    let (tx, rx) = channel();
    if let Ok(mut g) = timecode_subscribers().lock() {
        g.push(tx);
    }
    rx
}

/// The one place an Art-Net datagram is read: ArtDmx into the inboxes,
/// ArtTimeCode to the timecode subscribers. Every consumer of Art-Net input
/// goes through here, so the process binds UDP 6454 once.
pub fn dispatch(buf: &[u8]) -> Dispatched {
    if let Some((_, pa, data)) = parse_dmx(buf) {
        let mut frame = [0u8; 512];
        frame[..data.len()].copy_from_slice(data);
        if let Ok(mut g) = inbox().lock() {
            g.insert(pa, frame);
        }
        return Dispatched::Dmx;
    }
    if let Some(tc) = parse_timecode(buf) {
        if let Ok(mut g) = timecode_subscribers().lock() {
            g.retain(|tx| tx.send(tc).is_ok());
        }
        return Dispatched::TimeCode;
    }
    Dispatched::Other
}

/// The `in.artnet` row for this frame: `count` values from `first` of the
/// configured universe, each a byte over 255. Zeros until a packet arrives.
pub fn in_row(cfg: &ArtNetInConfig) -> Vec<f32> {
    let (first, count) = cfg.window();
    let frame = inbox().lock().ok().and_then(|g| g.get(&cfg.port_address()).copied());
    (0..count)
        .map(|i| frame.map_or(0.0, |f| f32::from(f[first - 1 + i]) / 255.0))
        .collect()
}

/// Starts the one Art-Net listener on UDP 6454 once per process (shared bind,
/// so it can sit beside discovery and another Art-Net app); every datagram
/// goes through [`dispatch`]. A bind that fails is kept for [`listen_error`];
/// the rows then stay zero.
pub fn listen() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let spawned = std::thread::Builder::new().name("metal-perform artnet in".into()).spawn(|| {
            let socket = match bind_shared(ARTNET_PORT) {
                Ok(s) => s,
                Err(e) => {
                    if let Ok(mut g) = listen_error_slot().lock() {
                        *g = Some(format!("Art-Net :{ARTNET_PORT}: {e}"));
                    }
                    return;
                }
            };
            let _ = socket.set_read_timeout(Some(Duration::from_millis(200)));
            let mut buf = [0u8; 1024];
            loop {
                if let Ok((n, _)) = socket.recv_from(&mut buf) {
                    dispatch(&buf[..n]);
                }
            }
        });
        if let Err(e) = spawned {
            if let Ok(mut g) = listen_error_slot().lock() {
                *g = Some(e.to_string());
            }
        }
    });
}

/// Why the receiver is not listening, when it is not.
pub fn listen_error() -> Option<String> {
    listen_error_slot().lock().ok().and_then(|g| g.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_address_is_fifteen_bits_and_round_trips() {
        assert_eq!(port_address(0, 0, 0), 0);
        assert_eq!(port_address(1, 2, 3), 0x0123);
        assert_eq!(port_address(0x7f, 0xf, 0xf), MAX_PORT_ADDRESS);
        assert_eq!(port_address(0xff, 0xff, 0xff), MAX_PORT_ADDRESS, "masked to width");
        assert_eq!(split_port_address(0x0123), (1, 2, 3));
    }

    #[test]
    fn art_dmx_header_and_even_length() {
        let p = dmx_packet(7, port_address(1, 2, 3), &[(1, 10), (3, 30), (600, 1)]);
        assert_eq!(&p[..8], b"Art-Net\0");
        assert_eq!(u16::from_le_bytes([p[8], p[9]]), OP_DMX);
        assert_eq!((p[10], p[11]), (0, 14));
        assert_eq!(p[12], 7);
        assert_eq!(p[14], 0x23, "SubUni");
        assert_eq!(p[15], 0x01, "Net");
        assert_eq!(u16::from_be_bytes([p[16], p[17]]), 4, "3 slots rounded up to even");
        assert_eq!(&p[18..], &[10, 0, 30, 0]);
        let (seq, pa, data) = parse_dmx(&p).unwrap();
        assert_eq!((seq, pa, data), (7, 0x0123, &[10u8, 0, 30, 0][..]));
        let full = dmx_packet(1, 0, &[(512, 9)]);
        assert_eq!(full.len(), 18 + 512);
    }

    #[test]
    fn sync_and_poll_are_fourteen_bytes() {
        assert_eq!(sync_packet().len(), 14);
        assert_eq!(opcode(&sync_packet()), Some(OP_SYNC));
        assert_eq!(poll_packet().len(), 14);
        assert_eq!(opcode(&poll_packet()), Some(OP_POLL));
        assert_eq!(opcode(b"nope"), None);
    }

    #[test]
    fn poll_reply_parses_name_ip_and_ports() {
        let p = poll_reply_packet(Ipv4Addr::new(2, 0, 0, 9), "nodo", "Nodo de prueba", &[0x0120, 0x0121]);
        let n = parse_poll_reply(&p).unwrap();
        assert_eq!(n.ip, "2.0.0.9");
        assert_eq!(n.short_name, "nodo");
        assert_eq!(n.long_name, "Nodo de prueba");
        assert_eq!(n.outputs, vec![0x0120, 0x0121]);
        assert!(n.inputs.is_empty());
    }

    #[test]
    fn config_defaults_to_broadcast_and_reads_unicast() {
        let node = metal_graph::Node::new("a", ARTNET_KIND, [0.0, 0.0]);
        let cfg = ArtNetConfig::from_node(&node);
        assert_eq!(cfg.port_address(), 0);
        assert_eq!(cfg.destination(), "255.255.255.255:6454".parse().unwrap());
        let cfg = ArtNetConfig { unicast: Some("10.0.0.4".into()), ..Default::default() };
        assert_eq!(cfg.destination(), "10.0.0.4:6454".parse().unwrap());
        let cfg = ArtNetConfig { broadcast: Some("2.255.255.255".into()), ..Default::default() };
        assert_eq!(cfg.destination(), "2.255.255.255:6454".parse().unwrap());
    }

    /// A unicast ArtNet sends an ArtDmx then, with `sync`, an ArtSync.
    #[test]
    fn wire_sends_dmx_then_sync_to_a_loopback_node() {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let port = rx.local_addr().unwrap().port();
        let cfg = ArtNetConfig { universe: 5, unicast: Some(format!("127.0.0.1:{port}")), sync: true, ..Default::default() };
        let mut w = ArtNet::new(&cfg).unwrap();
        assert!(w.send(cfg.port_address(), &[(1, 255), (2, 128)]));
        let mut buf = [0u8; 600];
        let n = rx.recv(&mut buf).unwrap();
        let (seq, pa, data) = parse_dmx(&buf[..n]).unwrap();
        assert_eq!((seq, pa), (1, 5));
        assert_eq!(&data[..2], &[255, 128]);
        let n = rx.recv(&mut buf).unwrap();
        assert_eq!(opcode(&buf[..n]), Some(OP_SYNC));
    }

    /// Discovery against a fake node on loopback that answers the poller.
    #[test]
    fn discover_lists_the_nodes_that_reply() {
        let node = UdpSocket::bind("127.0.0.1:0").unwrap();
        let node_addr = node.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            node.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut buf = [0u8; 64];
            let (n, from) = node.recv_from(&mut buf).unwrap();
            assert_eq!(opcode(&buf[..n]), Some(OP_POLL));
            node.send_to(&poll_reply_packet(Ipv4Addr::new(127, 0, 0, 1), "fake", "Fake node", &[3]), from).unwrap();
        });
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let found = discover_on(&s, node_addr, Duration::from_millis(400)).unwrap();
        t.join().unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].short_name, "fake");
        assert_eq!(found[0].outputs, vec![3]);
    }

    #[test]
    fn timecode_packet_round_trips() {
        use crate::timecode::{Fps, Timecode};
        let tc = Timecode { hours: 1, minutes: 2, seconds: 3, frames: 4, fps: Fps::F25 };
        let p = timecode_packet(&tc);
        assert_eq!(p.len(), 19);
        assert_eq!(parse_timecode(&p), Some(tc));
    }

    /// The `in.artnet` row is the configured channels of the configured
    /// universe, read from an ArtDmx that arrived on the socket's path.
    #[test]
    fn in_row_reads_the_channel_window_of_its_universe() {
        let cfg = ArtNetInConfig { universe: 0x0d, first: 3, count: 4, ..Default::default() };
        assert_eq!(dispatch(&dmx_packet(1, cfg.port_address(), &[(3, 255), (4, 128), (6, 1)])), Dispatched::Dmx);
        let row = in_row(&cfg);
        assert_eq!(row.len(), 4);
        assert_eq!(row[0], 1.0);
        assert!((row[1] - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(row[2], 0.0, "slot 5 was never sent");
        assert!((row[3] - 1.0 / 255.0).abs() < 1e-6);
        let other = ArtNetInConfig { universe: 0x0e, first: 3, count: 4, ..Default::default() };
        assert_eq!(in_row(&other), vec![0.0; 4], "another universe stays dark");
    }

    /// Non-ArtDmx datagrams are ignored, and a window past slot 512 is cut.
    #[test]
    fn in_row_ignores_other_packets_and_clamps_the_window() {
        assert_eq!(dispatch(&sync_packet()), Dispatched::Other);
        assert_eq!(dispatch(&poll_packet()), Dispatched::Other);
        let cfg = ArtNetInConfig { universe: 0x0f, first: 510, count: 99, ..Default::default() };
        assert_eq!(cfg.window(), (510, 3));
        assert_eq!(dispatch(&dmx_packet(2, cfg.port_address(), &[(512, 64)])), Dispatched::Dmx);
        assert_eq!(in_row(&cfg), vec![0.0, 0.0, 64.0 / 255.0]);
        let zero = ArtNetInConfig { first: 0, count: 0, ..Default::default() };
        assert_eq!(zero.window(), (1, 1), "parts out of range are clamped, not refused");
    }

    /// The config is the default when empty, and reads the node's own keys.
    #[test]
    fn in_config_defaults_to_sixteen_channels_from_one() {
        let node = metal_graph::Node::new("i", ARTNET_IN_KIND, [0.0, 0.0]);
        let cfg = ArtNetInConfig::from_node(&node);
        assert_eq!((cfg.first, cfg.count, cfg.port_address()), (1, 16, 0));
        let cfg: ArtNetInConfig = serde_json::from_str(r#"{"subnet":2,"universe":5,"first":7}"#).unwrap();
        assert_eq!(cfg.port_address(), 0x0025);
        assert_eq!(cfg.window(), (7, 16));
    }

    /// One listener, two kinds: an ArtDmx lands in its universe's inbox and an
    /// ArtTimeCode reaches the subscriber, both through [`dispatch`], in the
    /// order they arrive, with the other packets passing untouched.
    #[test]
    fn dispatch_routes_art_dmx_and_art_timecode_from_one_stream() {
        use crate::timecode::{Fps, Timecode};
        let rx = subscribe_timecode();
        let cfg = ArtNetInConfig { universe: 0x11, first: 1, count: 2, ..Default::default() };
        let tc = Timecode { hours: 2, minutes: 3, seconds: 4, frames: 5, fps: Fps::F25 };
        let stream = [
            dmx_packet(1, cfg.port_address(), &[(1, 51), (2, 102)]),
            timecode_packet(&tc),
            sync_packet(),
            poll_packet(),
        ];
        let kinds: Vec<Dispatched> = stream.iter().map(|p| dispatch(p)).collect();
        assert_eq!(kinds, vec![Dispatched::Dmx, Dispatched::TimeCode, Dispatched::Other, Dispatched::Other]);
        let row = in_row(&cfg);
        assert!((row[0] - 0.2).abs() < 1e-6 && (row[1] - 0.4).abs() < 1e-6, "the DMX reached its universe: {row:?}");
        assert_eq!(rx.try_recv().ok(), Some(tc), "the timecode reached the subscriber");
        assert!(rx.try_recv().is_err(), "and nothing else did");
    }
}
