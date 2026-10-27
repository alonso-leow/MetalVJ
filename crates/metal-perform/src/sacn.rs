//! F13 — the sACN **Universe Discovery** listener (ANSI E1.31-2018 §6.2): the
//! half of discovery `out.sacn`'s sender does not need. A source that streams
//! sACN announces which universes it is transmitting on by broadcasting a
//! Universe Discovery packet to the multicast group `239.255.70.1:5568` (the
//! spec's own discovery address) roughly every ten seconds; listening there
//! for a few seconds enumerates the active sources without joining every
//! universe's multicast group.
//!
//! Hand-built like [`crate::artnet::discover`]: no crate, just the byte
//! offsets E1.31 §6.2 defines. The Root layer carries the ACN identifier and
//! the extended root vector `0x00000008`; the Framing layer carries the
//! discovery vector `0x00000002` and the 64-byte source name; the Universe
//! Discovery layer carries the list vector `0x00000001`, a page / last-page
//! pair and the two-byte universes.
//!
//! The socket binds `5568` shared (as Art-Net does on `6454`) and joins the
//! discovery group, falling back to an ephemeral port when another sACN app
//! already holds it; the fallback still hears a source that sends to the
//! sender's port, though a spec-conforming source sends to the group. Never
//! blocks a frame: the read loop has a short timeout and runs only for the
//! caller's own `wait`.

use std::net::{Ipv4Addr, UdpSocket};
use web_time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// sACN's one UDP port, the discovery group's port too.
pub const SACN_PORT: u16 = 5568;

/// The multicast group the spec assigns Universe Discovery packets
/// (ANSI E1.31-2018 §6.2.3).
pub const DISCOVERY_MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 255, 70, 1);

const ACN_PACKET_IDENTIFIER: [u8; 12] = *b"ASC-E1.17\0\0\0";
const VECTOR_ROOT_E131_EXTENDED: u32 = 0x0000_0008;
const VECTOR_E131_EXTENDED_DISCOVERY: u32 = 0x0000_0002;
const VECTOR_UNIVERSE_DISCOVERY_UNIVERSE_LIST: u32 = 0x0000_0001;

/// The fixed bytes before the universe list: Root (38) + Framing (74) + the
/// discovery layer's own flags/vector/page/last-page (8).
const FIXED_LEN: usize = 120;

/// One source that answered a Universe Discovery listen: the device's IP and
/// source name, and every universe it announced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SacnSource {
    pub ip: String,
    pub name: String,
    pub universes: Vec<u16>,
}

fn flags_len(len: usize) -> [u8; 2] {
    ((0x7u16 << 12) | (len as u16 & 0x0fff)).to_be_bytes()
}

fn u32_be(buf: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

/// Hand-builds one Universe Discovery packet (one page, up to 512 universes)
/// — what a source sends, used by the tests' fake source and by nothing else
/// here. Universes are sorted and de-duplicated the way the spec asks.
pub fn universe_discovery_packet(name: &str, universes: &[u16]) -> Vec<u8> {
    let mut unis: Vec<u16> = universes.to_vec();
    unis.sort_unstable();
    unis.dedup();
    let total = FIXED_LEN + unis.len() * 2;
    let mut p = Vec::with_capacity(total);

    // Root layer (38 bytes): fixed preamble/postamble, ACN id, flags, the
    // extended root vector and a CID (a zero CID is fine for a fake source).
    p.extend_from_slice(&0x0010u16.to_be_bytes());
    p.extend_from_slice(&0x0000u16.to_be_bytes());
    p.extend_from_slice(&ACN_PACKET_IDENTIFIER);
    p.extend_from_slice(&flags_len(total - 16));
    p.extend_from_slice(&VECTOR_ROOT_E131_EXTENDED.to_be_bytes());
    p.extend_from_slice(&[0u8; 16]);

    // Framing layer (74 bytes): flags, the discovery vector, the source name
    // padded to 64 bytes, then four reserved zero bytes.
    p.extend_from_slice(&flags_len(total - 38));
    p.extend_from_slice(&VECTOR_E131_EXTENDED_DISCOVERY.to_be_bytes());
    let mut n = [0u8; 64];
    let bytes = name.as_bytes();
    let take = bytes.len().min(63);
    n[..take].copy_from_slice(&bytes[..take]);
    p.extend_from_slice(&n);
    p.extend_from_slice(&[0u8; 4]);

    // Universe Discovery layer: flags, the list vector, page / last page
    // (both 0, one page), then the big-endian universes.
    p.extend_from_slice(&flags_len(total - 112));
    p.extend_from_slice(&VECTOR_UNIVERSE_DISCOVERY_UNIVERSE_LIST.to_be_bytes());
    p.push(0);
    p.push(0);
    for u in &unis {
        p.extend_from_slice(&u.to_be_bytes());
    }
    p
}

/// Reads one Universe Discovery packet: its source name and universes, `None`
/// for a packet that is not one (or is too short). The universe count comes
/// from the discovery layer's own length field, so a padded datagram reads
/// only the universes the source listed.
pub fn parse_universe_discovery(buf: &[u8]) -> Option<(String, Vec<u16>)> {
    if buf.len() < FIXED_LEN {
        return None;
    }
    if &buf[4..16] != ACN_PACKET_IDENTIFIER {
        return None;
    }
    if u32_be(buf, 18) != VECTOR_ROOT_E131_EXTENDED
        || u32_be(buf, 40) != VECTOR_E131_EXTENDED_DISCOVERY
        || u32_be(buf, 114) != VECTOR_UNIVERSE_DISCOVERY_UNIVERSE_LIST
    {
        return None;
    }
    let name = cstr(&buf[44..108]);
    let len = (u16::from_be_bytes([buf[112], buf[113]]) & 0x0fff) as usize;
    if len < 8 {
        return None;
    }
    let count = (len - 8) / 2;
    let mut universes = Vec::with_capacity(count);
    for i in 0..count {
        let at = FIXED_LEN + i * 2;
        if at + 2 > buf.len() {
            break;
        }
        universes.push(u16::from_be_bytes([buf[at], buf[at + 1]]));
    }
    Some((name, universes))
}

/// Files `(ip, name, universes)` into `found`: a source heard again (another
/// page, or a repeat) has its universes unioned rather than listed twice.
fn merge_source(found: &mut Vec<SacnSource>, ip: String, name: String, universes: Vec<u16>) {
    if let Some(existing) = found.iter_mut().find(|s| s.ip == ip && s.name == name) {
        existing.universes.extend(universes);
        existing.universes.sort_unstable();
        existing.universes.dedup();
    } else {
        let mut universes = universes;
        universes.sort_unstable();
        universes.dedup();
        found.push(SacnSource { ip, name, universes });
    }
}

/// Reads every Universe Discovery packet heard on `socket` for `wait` — the
/// testable half of [`discover`], which only adds the bind and the multicast
/// join.
pub fn collect_on(socket: &UdpSocket, wait: Duration) -> std::io::Result<Vec<SacnSource>> {
    socket.set_read_timeout(Some(Duration::from_millis(50)))?;
    let until = Instant::now() + wait;
    let mut found: Vec<SacnSource> = Vec::new();
    let mut buf = [0u8; 2048];
    while Instant::now() < until {
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                if let Some((name, universes)) = parse_universe_discovery(&buf[..n]) {
                    merge_source(&mut found, from.ip().to_string(), name, universes);
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e),
        }
    }
    found.sort_by(|a, b| a.ip.cmp(&b.ip).then_with(|| a.name.cmp(&b.name)));
    Ok(found)
}

/// Discovery on the real network: binds `5568` shared, joins the discovery
/// group, and lists the sources heard for `wait`. Falls back to an ephemeral
/// port when `5568` is taken by another sACN app.
pub fn discover(wait: Duration) -> std::io::Result<Vec<SacnSource>> {
    let socket = match crate::artnet::bind_shared(SACN_PORT) {
        Ok(s) => s,
        Err(_) => UdpSocket::bind("0.0.0.0:0")?,
    };
    let _ = socket.join_multicast_v4(&DISCOVERY_MULTICAST, &Ipv4Addr::UNSPECIFIED);
    collect_on(&socket, wait)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The builder and the parser agree on every field: source name, the
    /// universe list, and a sorted de-duplicated round trip.
    #[test]
    fn a_universe_discovery_packet_round_trips() {
        let p = universe_discovery_packet("sala", &[300, 1, 20, 1]);
        assert_eq!(p.len(), FIXED_LEN + 3 * 2);
        let (name, universes) = parse_universe_discovery(&p).unwrap();
        assert_eq!(name, "sala");
        assert_eq!(universes, vec![1, 20, 300]);
    }

    /// Anything that is not a discovery packet is refused: a short body, a
    /// data packet's root vector, a wrong list vector.
    #[test]
    fn a_foreign_packet_is_refused() {
        assert!(parse_universe_discovery(b"nope").is_none());
        let mut p = universe_discovery_packet("x", &[1]);
        p[18..22].copy_from_slice(&0x0000_0004u32.to_be_bytes());
        assert!(parse_universe_discovery(&p).is_none());
        let mut p = universe_discovery_packet("x", &[1]);
        p[114..118].copy_from_slice(&0x0000_0009u32.to_be_bytes());
        assert!(parse_universe_discovery(&p).is_none());
    }

    /// Two pages from the same source merge into one entry with the union of
    /// their universes, not two rows.
    #[test]
    fn two_pages_from_one_source_merge() {
        let mut found = Vec::new();
        merge_source(&mut found, "2.0.0.5".into(), "sala".into(), vec![1, 2]);
        merge_source(&mut found, "2.0.0.5".into(), "sala".into(), vec![2, 300]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].universes, vec![1, 2, 300]);
        merge_source(&mut found, "2.0.0.6".into(), "otra".into(), vec![9]);
        assert_eq!(found.len(), 2, "a different source is its own row");
    }

    /// A fake source on loopback is heard: the receiving loop parses the
    /// packet and files the source's name and universes.
    #[test]
    fn a_fake_source_is_heard_on_the_socket() {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = rx.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
            tx.send_to(&universe_discovery_packet("fake", &[1, 7, 512]), addr).unwrap();
        });
        let found = collect_on(&rx, Duration::from_millis(300)).unwrap();
        t.join().unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "fake");
        assert_eq!(found[0].universes, vec![1, 7, 512]);
    }
}
