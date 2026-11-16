//! O.10b: the OSC reader — one UDP socket, one thread, ending in
//! `POST /trigger/<id>` over loopback.
//!
//! It decodes each datagram's address ([`decode_addr`], a pure function so
//! the packet shape is testable without a socket), asks
//! [`super::osc_targets`] whose triggers should fire, and posts. The reader
//! never touches `State` beyond that read, never blocks the app (it is its
//! own thread), and a bad datagram or a failed post only logs — one garbled
//! packet must not take the reader down (`regla 4`: the routes own the
//! state, the reader only hears).

use std::sync::{Arc, Mutex};

use super::{osc_targets, post_trigger};

/// The address of an OSC *message* datagram, or `None` for a bundle or
/// garbage. Bundles are not read: the plan's OSC source is an exact address
/// match, and a bundle's own address is its schedule, not a button.
pub fn decode_addr(bytes: &[u8]) -> Option<String> {
    match rosc::decoder::decode_udp(bytes) {
        Ok((_rest, rosc::OscPacket::Message(m))) => Some(m.addr),
        _ => None,
    }
}

/// Bind the UDP socket and read forever. `host`/`http_port` point at this
/// same process's loopback HTTP listener; `udp_port` is where OSC arrives.
pub fn run(shared: Arc<Mutex<crate::Shared>>, host: String, http_port: u16, udp_port: u16) {
    let socket = match std::net::UdpSocket::bind((host.as_str(), udp_port)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("  osc: cannot bind {host}:{udp_port}: {e} — OSC input off");
            return;
        }
    };
    eprintln!("  osc: listening on {host}:{udp_port} → http://{host}:{http_port}/trigger/");

    let mut buf = [0u8; 8192];
    loop {
        let n = match socket.recv_from(&mut buf) {
            Ok((n, _from)) => n,
            Err(e) => {
                // A read error is transient (a signal, a full buffer); the
                // reader keeps standing rather than dying on one datagram.
                eprintln!("  osc: read error: {e}");
                continue;
            }
        };
        let Some(addr) = decode_addr(&buf[..n]) else {
            continue;
        };
        let (targets, learn_target) = {
            let guard = match shared.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            osc_targets(&guard, &addr)
        };
        if let Some(id) = learn_target {
            let body = serde_json::json!({
                "op": "learn",
                "source": { "source": "osc", "addr": addr },
            });
            if let Err(e) = post_trigger(&host, http_port, &id, &body) {
                eprintln!("  osc: learn post to {id:?} failed: {e}");
            }
            continue;
        }
        for id in targets {
            if let Err(e) = post_trigger(&host, http_port, &id, &serde_json::json!({})) {
                eprintln!("  osc: post to {id:?} failed: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType};

    fn message_bytes(addr: &str) -> Vec<u8> {
        let packet = OscPacket::Message(OscMessage {
            addr: addr.to_string(),
            args: vec![OscType::Int(1)],
        });
        rosc::encoder::encode(&packet).expect("encode a message")
    }

    /// A valid message yields its address; a bundle and garbage yield
    /// nothing (a reader must not mistake a bundle's schedule for a button).
    #[test]
    fn decode_addr_reads_a_message_and_rejects_a_bundle_and_garbage() {
        assert_eq!(decode_addr(&message_bytes("/pad/1")).as_deref(), Some("/pad/1"));

        let bundle = OscPacket::Bundle(OscBundle {
            timetag: OscTime {
                seconds: 0,
                fractional: 1,
            },
            content: vec![],
        });
        let bytes = rosc::encoder::encode(&bundle).expect("encode a bundle");
        assert_eq!(decode_addr(&bytes), None);

        assert_eq!(decode_addr(&[0xff, 0x00, 0x01, 0x02]), None);
    }

    /// A trigger with `Osc{addr}` is matched by its exact address and a
    /// different address is not — the reader's decision, read off the pure
    /// helper, not a live socket.
    #[test]
    fn an_osc_trigger_is_matched_by_its_address_and_nothing_else() {
        *super::super::learn().lock().unwrap() = None;
        let paint = crate::paint::painter();
        let mut state = crate::State::new(paint.as_ref());
        state
            .outputs
            .list
            .push(crate::output::Output::new("probe", "Probe", 64, 64));
        let dev = state
            .sources
            .entry("probe".to_string())
            .or_insert_with(crate::layers::Device::default);
        dev.triggers.push(crate::trigger::Trigger::new_osc(
            "pad".to_string(),
            "overlay-drop".to_string(),
            "/pad/1".to_string(),
        ));
        let shared = crate::Shared {
            state,
            paint,
            sim: crate::listeners::Listeners::stand_in(),
        };

        let (hit, _) = osc_targets(&shared, "/pad/1");
        assert_eq!(hit, vec!["pad".to_string()]);
        let (miss, _) = osc_targets(&shared, "/pad/2");
        assert!(miss.is_empty());
    }
}
