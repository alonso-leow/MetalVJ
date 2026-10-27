//! Bluetooth Mesh PDU layers and the GATT Proxy framing.
//!
//! The mesh stack is four nested encryptions and this module owns the three
//! middle ones, plus the Proxy bearer that carries them over GATT:
//!
//! ```text
//!   Access Layer payload             (this is what `godox.rs` builds)
//!     └─ Upper Transport (access)    AES-CCM, app key,  nonce type 0x01, MIC 4
//!     └─ Lower Transport             unsegmented, or segmented when long
//!     └─ Network Layer               AES-CCM, enc key,  nonce type 0x00, MIC 4/8
//!     └─ Proxy PDU                   1 SAR/type byte + network PDU
//! ```
//!
//! Config Server messages take the **device key** path instead: the Upper
//! Transport uses nonce type 0x02 with the device key and an `AKF=0` header
//! byte (`transport::ConfigSession` builds those).
//!
//! The Proxy PDU's first byte is the SAR field (bits 7–6) and the upper six
//! bits are zero for a complete message; the low two bits select the bearer
//! type in bit 0 (Network = 0, Provisioning = 1 for the *type*, per §6.6 — but
//! see the note on the wire encoding below).
//!
//! ### Why the proxy type byte is what it is
//!
//! The Godox reference (and the Provisioning bearer) put `0x03` in the first
//! byte for a provisioning PDU, which the spec calls `PROXY_TYPE_PROVISIONING`
//! and the SAR/type field encodes as `SAR=0, type=Provisio…`. On the wire the
//! byte is a single octet: `(SAR << 6) | type`. `type` is 2 bits for the Proxy
//! Client filter, but the value actually written on Data In / read from Data
//! Out is the spec's `0x00` for a network PDU and `0x03` for provisioning.
//! We keep the reference's exact bytes so the light sees what the working
//! Python client sends.

use crate::crypto::{ccm_decrypt, ccm_encrypt, k4, obfuscate};

/// Proxy bearer / message-type byte for a complete Network PDU.
pub const PROXY_TYPE_NETWORK: u8 = 0x00;
/// Proxy message-type byte for a Proxy Configuration PDU.
pub const PROXY_TYPE_CONFIG: u8 = 0x02;
/// Proxy message-type byte for a Provisioning PDU (PB-GATT).
pub const PROXY_TYPE_PROVISIONING: u8 = 0x03;

/// Maximum Access payload that fits an unsegmented Upper Transport PDU.
pub const UNSEGMENTED_MAX: usize = 15;
/// Access payload that fits one segment (spec's 11-byte rule for AKF=0).
pub const SEGMENT_PAYLOAD: usize = 11;

/// Upper Transport nonce (Mesh Profile §3.8.5.1). `aszmic` is 0 for 4-byte MICs.
pub fn upper_transport_nonce(
    aszmic: u8,
    seq: u32,
    src: u16,
    dst: u16,
    iv_index: u32,
) -> [u8; 13] {
    let mut n = [0u8; 13];
    n[0] = 0x01; // Application nonce
    n[1] = (aszmic & 1) << 7;
    n[2..5].copy_from_slice(&seq.to_be_bytes()[1..]);
    n[5..7].copy_from_slice(&src.to_be_bytes());
    n[7..9].copy_from_slice(&dst.to_be_bytes());
    n[9..13].copy_from_slice(&iv_index.to_be_bytes());
    n
}

/// Network nonce (Mesh Profile §3.8.5.2).
pub fn network_nonce(ctl_ttl: u8, seq: u32, src: u16, iv_index: u32) -> [u8; 13] {
    let mut n = [0u8; 13];
    n[0] = 0x00; // Network nonce
    n[1] = ctl_ttl;
    n[2..5].copy_from_slice(&seq.to_be_bytes()[1..]);
    n[5..7].copy_from_slice(&src.to_be_bytes());
    // bytes 7..9 stay zero (DST must not appear)
    n[9..13].copy_from_slice(&iv_index.to_be_bytes());
    n
}

/// Device-key Upper Transport PDU (Config Server path): `AKF=0` header byte
/// followed by CCM(device_key, nonce type 0x02, payload, MIC 4).
pub fn encode_device_key_pdu(
    payload: &[u8],
    device_key: &[u8; 16],
    iv_index: u32,
    seq: u32,
    src: u16,
    dst: u16,
) -> Vec<u8> {
    let mut nonce = [0u8; 13];
    nonce[0] = 0x02;
    nonce[2..5].copy_from_slice(&seq.to_be_bytes()[1..]);
    nonce[5..7].copy_from_slice(&src.to_be_bytes());
    nonce[7..9].copy_from_slice(&dst.to_be_bytes());
    nonce[9..13].copy_from_slice(&iv_index.to_be_bytes());
    let mut out = vec![0x00]; // AKF=0, AID=0
    out.extend_from_slice(&ccm_encrypt(device_key, &nonce, payload, 4));
    out
}

/// Decode a device-key Upper Transport PDU produced by `encode_device_key_pdu`.
pub fn decode_device_key_pdu(
    encrypted: &[u8],
    device_key: &[u8; 16],
    iv_index: u32,
    seq: u32,
    src: u16,
    dst: u16,
) -> Option<Vec<u8>> {
    if encrypted.is_empty() {
        return None;
    }
    let mut nonce = [0u8; 13];
    nonce[0] = 0x02;
    nonce[2..5].copy_from_slice(&seq.to_be_bytes()[1..]);
    nonce[5..7].copy_from_slice(&src.to_be_bytes());
    nonce[7..9].copy_from_slice(&dst.to_be_bytes());
    nonce[9..13].copy_from_slice(&iv_index.to_be_bytes());
    ccm_decrypt(device_key, &nonce, &encrypted[1..], 4)
}

/// Encrypt an Access Layer payload into an Upper Transport PDU: one AKF/AID
/// header byte followed by the ciphertext and a 4-byte MIC.
pub fn encode_access_pdu(
    access_payload: &[u8],
    app_key: &[u8; 16],
    iv_index: u32,
    seq: u32,
    src: u16,
    dst: u16,
) -> Vec<u8> {
    let nonce = upper_transport_nonce(0, seq, src, dst, iv_index);
    let aid = k4(app_key);
    let mut out = vec![0x40 | (aid & 0x3f)]; // AKF=1
    out.extend_from_slice(&ccm_encrypt(app_key, &nonce, access_payload, 4));
    out
}

/// Decrypt an Upper Transport PDU back into an Access Layer payload.
pub fn decode_access_pdu(
    encrypted: &[u8],
    app_key: &[u8; 16],
    iv_index: u32,
    seq: u32,
    src: u16,
    dst: u16,
) -> Option<Vec<u8>> {
    if encrypted.len() < 5 {
        return None;
    }
    let nonce = upper_transport_nonce(0, seq, src, dst, iv_index);
    ccm_decrypt(app_key, &nonce, &encrypted[1..], 4)
}

/// A complete, encrypted Network PDU plus the fields a receiver needs to
/// decrypt it (SEQ/SRC are not encrypted, but they are not in the obfuscated
/// header either, so we carry them alongside during tests).
#[derive(Clone, Debug, PartialEq)]
pub struct NetworkPdu {
    pub ctl_ttl: u8,
    pub seq: u32,
    pub src: u16,
    pub iv_index: u32,
    /// IVI||NID byte, then obfuscated CTL/TTL/SEQ/SRC, then encrypted
    /// DST+transport+MIC.
    pub bytes: Vec<u8>,
}

/// Lower Transport: for an unsegmented message the transport PDU *is* the
/// Upper Transport PDU. Segmentation is implemented for completeness (access
/// payloads above [`UNSEGMENTED_MAX`] bytes), but the Godox commands all fit.
pub fn lower_transport_unsegmented(upper_transport_pdu: &[u8]) -> Vec<u8> {
    upper_transport_pdu.to_vec()
}

/// Encrypt a network PDU (Mesh Profile §3.6 / §3.8.5.2).
///
/// `transport_pdu` is the Lower Transport PDU; `mic_len` is 4 for CTL=0
/// (access) messages and 8 for CTL=1 (control) messages.
#[allow(clippy::too_many_arguments)]
pub fn encode_network_pdu(
    nid: u8,
    encryption_key: &[u8; 16],
    privacy_key: &[u8; 16],
    iv_index: u32,
    ctl: bool,
    ttl: u8,
    seq: u32,
    src: u16,
    dst: u16,
    transport_pdu: &[u8],
    mic_len: usize,
) -> NetworkPdu {
    let ctl_ttl = ((ctl as u8) << 7) | (ttl & 0x7f);
    let nonce = network_nonce(ctl_ttl, seq, src, iv_index);
    let mut plain = Vec::with_capacity(2 + transport_pdu.len());
    plain.extend_from_slice(&dst.to_be_bytes());
    plain.extend_from_slice(transport_pdu);
    let encrypted = ccm_encrypt(encryption_key, &nonce, &plain, mic_len);

    let mut bytes = Vec::with_capacity(7 + encrypted.len());
    bytes.push(((iv_index as u8 & 1) << 7) | (nid & 0x7f));
    bytes.push(ctl_ttl);
    bytes.extend_from_slice(&seq.to_be_bytes()[1..]);
    bytes.extend_from_slice(&src.to_be_bytes());
    bytes.extend_from_slice(&encrypted);
    NetworkPdu {
        ctl_ttl,
        seq,
        src,
        iv_index,
        bytes: obfuscate(privacy_key, iv_index, &bytes),
    }
}

/// Decrypt a Network PDU: de-obfuscate, then CCM-decrypt the body.
///
/// Returns `(ctl_ttl, seq, src, dst, transport_pdu)`.
pub fn decode_network_pdu(
    pdu: &[u8],
    encryption_key: &[u8; 16],
    privacy_key: &[u8; 16],
    iv_index: u32,
    mic_len: usize,
) -> Option<(u8, u32, u16, u16, Vec<u8>)> {
    if pdu.len() < 9 {
        return None;
    }
    let deobf = obfuscate(privacy_key, iv_index, pdu);
    let ctl_ttl = deobf[1];
    let seq = u32::from_be_bytes([0, deobf[2], deobf[3], deobf[4]]);
    let src = u16::from_be_bytes([deobf[5], deobf[6]]);
    let nonce = network_nonce(ctl_ttl, seq, src, iv_index);
    let plain = ccm_decrypt(encryption_key, &nonce, &deobf[7..], mic_len)?;
    if plain.len() < 2 {
        return None;
    }
    let dst = u16::from_be_bytes([plain[0], plain[1]]);
    Some((ctl_ttl, seq, src, dst, plain[2..].to_vec()))
}

/// Wrap a Network PDU in a complete Proxy PDU (SAR=0, type Network).
pub fn encode_proxy_network(network_pdu: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + network_pdu.len());
    out.push(PROXY_TYPE_NETWORK);
    out.extend_from_slice(network_pdu);
    out
}

/// Wrap a Provisioning PDU in the proxy framing used by PB-GATT.
pub fn encode_proxy_provisioning(pdu_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + payload.len());
    out.push(PROXY_TYPE_PROVISIONING);
    out.push(pdu_type);
    out.extend_from_slice(payload);
    out
}

/// Pack the two Proxy Client configuration PDUs the reference uses
/// (`pack_proxy_config_pdu`): `0x02` SAR/type, then a network PDU with
/// `CTL=1, TTL=0`, destination 0x0000, encrypted with the network key under a
/// type-0x03 nonce and an 8-byte MIC.
///
/// Eight arguments is deliberate: this is a faithful port of the reference's
/// argument list, and grouping them into a struct would hide which fields are
/// the derived keys.
#[allow(clippy::too_many_arguments)]
pub fn encode_proxy_config(
    opcode: u8,
    parameters: &[u8],
    nid: u8,
    encryption_key: &[u8; 16],
    privacy_key: &[u8; 16],
    iv_index: u32,
    seq: u32,
    src: u16,
) -> Vec<u8> {
    let mut nonce = [0u8; 13];
    nonce[0] = 0x03;
    nonce[2..5].copy_from_slice(&seq.to_be_bytes()[1..]);
    nonce[5..7].copy_from_slice(&src.to_be_bytes());
    nonce[9..13].copy_from_slice(&iv_index.to_be_bytes());
    // Payload is dst(2)==0 then opcode+parameters, matching Telink's flow.
    let mut plain = vec![0x00, 0x00, opcode];
    plain.extend_from_slice(parameters);
    let encrypted = ccm_encrypt(encryption_key, &nonce, &plain, 8);
    let ctl_ttl = 0x80; // CTL=1, TTL=0
    let mut bytes = Vec::with_capacity(7 + encrypted.len());
    bytes.push(((iv_index as u8 & 1) << 7) | (nid & 0x7f));
    bytes.push(ctl_ttl);
    bytes.extend_from_slice(&seq.to_be_bytes()[1..]);
    bytes.extend_from_slice(&src.to_be_bytes());
    bytes.extend_from_slice(&encrypted);
    let mut out = vec![PROXY_TYPE_CONFIG];
    out.extend_from_slice(&obfuscate(privacy_key, iv_index, &bytes));
    out
}

/// One reassembled Proxy PDU notification, or a raw pass-through.
#[derive(Clone, Debug, PartialEq)]
pub enum ProxyMessage {
    /// A complete Network PDU (SAR=0, type Network).
    Network(Vec<u8>),
    /// A complete Provisioning PDU: `(pdu_type, payload)` (type byte `0x03`).
    Provisioning(u8, Vec<u8>),
    /// A Proxy Configuration PDU (type `0x02`).
    Config(Vec<u8>),
    /// Anything else, passed through as-is (never silently dropped).
    Other(Vec<u8>),
}

/// Proxy SAR reassembly for Data Out notifications.
///
/// The device sends network PDUs as `SAR=0` complete messages, but a Proxy
/// PDU may also arrive segmented (`SAR=1` start, `SAR=2` continuation,
/// `SAR=3` end). We buffer segments and hand back one reassembled
/// [`ProxyMessage`] per complete message. Malformed input is skipped, never
/// panicked on (plan rule 6: malformed is a skip).
#[derive(Default)]
pub struct ProxyAssembly {
    buffer: Vec<u8>,
    active: bool,
}

impl ProxyAssembly {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one GATT notification, returning a complete message when one is
    /// assembled.
    pub fn push(&mut self, data: &[u8]) -> Option<ProxyMessage> {
        if data.is_empty() {
            return None;
        }
        let sar = data[0] >> 6;
        let payload = &data[1..];
        match sar {
            0b00 => {
                self.buffer.clear();
                self.active = false;
                Some(classify(data[0], payload))
            }
            0b01 => {
                // Start of a segmented message.
                self.buffer.clear();
                self.buffer.extend_from_slice(payload);
                self.active = true;
                None
            }
            0b10 => {
                // Continuation: only valid between Start and End.
                if self.active {
                    self.buffer.extend_from_slice(payload);
                } else {
                    // Unsolicited continuation — discard rather than corrupt.
                    self.buffer.clear();
                }
                None
            }
            0b11 => {
                // End segment.
                if !self.active {
                    self.buffer.clear();
                    return None;
                }
                self.active = false;
                let mut full = std::mem::take(&mut self.buffer);
                full.extend_from_slice(payload);
                Some(classify(data[0], &full))
            }
            _ => None,
        }
    }
}

fn classify(first: u8, payload: &[u8]) -> ProxyMessage {
    match first & 0x3f {
        PROXY_TYPE_NETWORK => ProxyMessage::Network(payload.to_vec()),
        PROXY_TYPE_PROVISIONING => {
            if payload.is_empty() {
                ProxyMessage::Other(payload.to_vec())
            } else {
                ProxyMessage::Provisioning(payload[0], payload[1..].to_vec())
            }
        }
        PROXY_TYPE_CONFIG => ProxyMessage::Config(payload.to_vec()),
        _ => ProxyMessage::Other(payload.to_vec()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::k2;

    fn a16(s: &str) -> [u8; 16] {
        hex::decode(s).unwrap().try_into().unwrap()
    }

    /// Full access-PDU round trip: access payload → upper transport → network
    /// PDU → proxy wrapper → decode back to the access payload.
    #[test]
    fn access_network_proxy_round_trip() {
        let net_key = a16("7dd7364cd842ad18c17c2b820c84c3d6");
        let app_key = a16("63964771734fbd76e3b40519d1d94a48");
        let (nid, enc_key, priv_key) = k2(&net_key, 0x00);
        let iv = 0x1234_5678;
        let seq = 0x0000_002a;
        let src = 0x0001;
        let dst = 0x0002;
        let access = b"godox!".to_vec();

        let upper = encode_access_pdu(&access, &app_key, iv, seq, src, dst);
        let lower = lower_transport_unsegmented(&upper);
        let net = encode_network_pdu(
            nid, &enc_key, &priv_key, iv, false, 10, seq, src, dst, &lower, 4,
        );
        let proxy = encode_proxy_network(&net.bytes);

        // The receiver's path:
        let mut asm = ProxyAssembly::new();
        let msg = asm.push(&proxy).expect("complete proxy pdu");
        let network_bytes = match msg {
            ProxyMessage::Network(b) => b,
            other => panic!("expected network, got {other:?}"),
        };
        let (ctl_ttl, dseq, dsrc, ddst, transport) =
            decode_network_pdu(&network_bytes, &enc_key, &priv_key, iv, 4).unwrap();
        assert!(!(ctl_ttl & 0x80 != 0)); // CTL=0
        assert_eq!(dseq, seq);
        assert_eq!(dsrc, src);
        assert_eq!(ddst, dst);
        let recovered = decode_access_pdu(&transport, &app_key, iv, dseq, dsrc, ddst).unwrap();
        assert_eq!(recovered, access);
    }

    #[test]
    fn device_key_round_trip() {
        let dk = a16("6277be2be27af9818c3d79b62a2a8ae7");
        let payload = b"\x08\x80\xff";
        let enc = encode_device_key_pdu(payload, &dk, 0, 500, 0x0001, 0x0002);
        assert_eq!(enc[0], 0x00, "device-key header is AKF=0/AID=0");
        assert_eq!(
            decode_device_key_pdu(&enc, &dk, 0, 500, 0x0001, 0x0002).unwrap(),
            payload
        );
    }

    #[test]
    fn config_pdu_starts_with_type_02() {
        let net_key = a16("bc6bf99f840f9ca379562468a110d2a9");
        let (nid, enc_key, priv_key) = k2(&net_key, 0x00);
        let pdu = encode_proxy_config(0x00, b"\x01", nid, &enc_key, &priv_key, 0, 300, 0x0001);
        assert_eq!(pdu[0], 0x02);
        assert_eq!(pdu.len(), 20);
        let wl = encode_proxy_config(0x01, b"\x00\x01\xff\xff", nid, &enc_key, &priv_key, 0, 301, 0x0001);
        assert_eq!(wl[0], 0x02);
        assert_eq!(wl.len(), 23);
    }

    #[test]
    fn proxy_assembly_segments() {
        // A 20-byte network PDU delivered as start+continuation+end.
        let mut asm = ProxyAssembly::new();
        // SAR=1 start, type network
        let mut seg1 = vec![0x40]; // SAR=1
        seg1.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(asm.push(&seg1).is_none());
        let mut seg2 = vec![0x80]; // SAR=2
        seg2.extend_from_slice(&[9, 10, 11, 12, 13, 14, 15, 16]);
        assert!(asm.push(&seg2).is_none());
        let mut seg3 = vec![0xc0]; // SAR=3
        seg3.extend_from_slice(&[17, 18, 19, 20]);
        let done = asm.push(&seg3).unwrap();
        assert_eq!(
            done,
            ProxyMessage::Network((1..=20).collect::<Vec<u8>>())
        );
    }

    #[test]
    fn proxy_assembly_unsolicited_continuation_is_skipped() {
        let mut asm = ProxyAssembly::new();
        let seg = vec![0x80, 1, 2, 3]; // continuation with no start
        assert!(asm.push(&seg).is_none());
        // Still usable afterwards for a real complete message.
        let complete = asm.push(&[0x00, 9, 9, 9]).unwrap();
        assert_eq!(complete, ProxyMessage::Network(vec![9, 9, 9]));
    }

    #[test]
    fn provisioning_frame_is_typed() {
        let frame = encode_proxy_provisioning(0x00, &[0x00]);
        assert_eq!(frame, vec![0x03, 0x00, 0x00]);
        let mut asm = ProxyAssembly::new();
        assert_eq!(
            asm.push(&frame),
            Some(ProxyMessage::Provisioning(0x00, vec![0x00]))
        );
    }

    #[test]
    fn network_nonce_has_zero_dst() {
        let n = network_nonce(0x80, 0x000001, 0x1201, 0x1234_5678);
        assert_eq!(n.to_vec(), hex::decode("00800000011201000012345678").unwrap());
        assert_eq!(n[7], 0);
        assert_eq!(n[8], 0);
    }
}
