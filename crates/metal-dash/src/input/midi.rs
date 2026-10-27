#![cfg(feature = "midi")]
//! O.10b: the MIDI reader — one ALSA sequencer client, one thread, ending in
//! `POST /trigger/<id>` over loopback.
//!
//! It is the MIDI twin of [`super::osc`]: it decodes each 3-byte channel
//! message ([`decode_message`], pure so the packet shape is testable without a
//! device), asks the shared [`super::learn`] arm and the `Shared` state whose
//! triggers should fire, and posts. The reader never touches `State` beyond
//! that read, never blocks the app (midir runs the callback on its own thread,
//! and this thread only parks to keep the connections alive), and a bad
//! message or a failed post only logs — one stray byte must not take the
//! reader down (`regla 4`: the routes own the state, the reader only hears).
//!
//! **Feature-gated.** `midir` links the system `libasound` through
//! `alsa-sys`, so the default build must not need ALSA headers — the Pi
//! builds natively with `libasound2-dev` and turns this on with
//! `--features midi` (the measurement in `/tmp/midi-probe/REPORT.md`).

use std::sync::{Arc, Mutex};

use midir::{MidiInput, MidiInputConnection};

use super::{learn, post_trigger};

/// The MIDI half of [`super::osc::decode_addr`]: a pure decoder for a 3-byte
/// channel message.
///
/// Returns `(cc, note)`, exactly one of them `Some`:
/// - Note On (`0x90 | ch`) with velocity > 0 → `(None, Some(data1))`;
/// - Control Change (`0xB0 | ch`) → `(Some(data1), None)`.
///
/// A Note On with velocity 0 is the running-status note off, so it returns
/// `None`, as does every other status (note off, pitch bend, sysex, real-time).
pub fn decode_message(status: u8, data1: u8, data2: u8) -> Option<(Option<u8>, Option<u8>)> {
    match status & 0xF0 {
        0x90 if data2 > 0 => Some((None, Some(data1))),
        0xB0 => Some((Some(data1), None)),
        _ => None,
    }
}

/// Open the ALSA sequencer, connect every matching input port, and park.
///
/// `host`/`http_port` point at this same process's loopback HTTP listener;
/// `port_filter` is a substring of the port name to accept (`None` takes every
/// port). Each port gets the same callback: decode, learn or match, post. The
/// connections are held in `connections` so midir's input threads stay alive;
/// this thread then parks forever.
pub fn run(
    shared: Arc<Mutex<crate::Shared>>,
    host: String,
    http_port: u16,
    port_filter: Option<String>,
) {
    // A probe client only lists/enumerates ports; `connect` consumes its
    // `MidiInput` (`midir` 0.11), so each connection below gets its own.
    let probe = match MidiInput::new("metalvj") {
        Ok(i) => i,
        Err(e) => {
            eprintln!("  midi: cannot open the ALSA sequencer: {e} — MIDI input off");
            return;
        }
    };
    let ports = probe.ports();
    if ports.is_empty() {
        eprintln!("  midi: no input ports — MIDI input off");
        return;
    }
    let wanted: Vec<_> = ports
        .into_iter()
        .filter_map(|p| {
            let name = match probe.port_name(&p) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("  midi: cannot read a port name: {e} — skipped");
                    return None;
                }
            };
            match &port_filter {
                Some(f) if !name.contains(f.as_str()) => None,
                _ => Some((p, name)),
            }
        })
        .collect();
    drop(probe);
    if wanted.is_empty() {
        eprintln!("  midi: no matching input ports (filter {port_filter:?}) — MIDI input off");
        return;
    }

    let mut connections: Vec<MidiInputConnection<()>> = Vec::new();
    for (port, port_name) in wanted {
        let input = match MidiInput::new("metalvj") {
            Ok(i) => i,
            Err(e) => {
                eprintln!("  midi: cannot open the ALSA sequencer: {e} — MIDI input off");
                return;
            }
        };
        let shared = Arc::clone(&shared);
        let cb_host = host.clone();
        let name = port_name.clone();
        let callback = move |_stamp: u64, message: &[u8], _: &mut ()| {
            if message.len() < 3 {
                return;
            }
            let Some((cc, note)) = decode_message(message[0], message[1], message[2]) else {
                return;
            };
            // Learn wins: while the arm is this reader's kind, the first thing
            // heard is written into the armed button and the active matches
            // are skipped.
            if let Some(arm) = learn().lock().ok().and_then(|g| g.clone()) {
                if arm.kind == "midi" {
                    let body = serde_json::json!({
                        "op": "learn",
                        "source": {
                            "source": "midi",
                            "port": &name,
                            "cc": cc,
                            "note": note,
                        },
                    });
                    let _ = post_trigger(&cb_host, http_port, &arm.trigger, &body);
                    return;
                }
            }
            let targets = {
                let guard = match shared.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                let mut ids = Vec::new();
                for dev in guard.state.sources.values() {
                    for t in &dev.triggers {
                        if t.source.matches_midi(&name, cc, note) {
                            ids.push(t.id.clone());
                        }
                    }
                }
                ids
            };
            for id in targets {
                let _ = post_trigger(&cb_host, http_port, &id, &serde_json::json!({}));
            }
        };
        match input.connect(&port, "metalvj-in", callback, ()) {
            Ok(c) => {
                eprintln!(
                    "  midi: listening on {port_name:?} → http://{host}:{http_port}/trigger/"
                );
                connections.push(c);
            }
            Err(e) => eprintln!("  midi: cannot connect {port_name:?}: {e}"),
        }
    }
    if connections.is_empty() {
        eprintln!("  midi: no port connected — MIDI input off");
        return;
    }
    // The callbacks run on midir's own threads; this one only keeps the
    // process from dropping the connections.
    loop {
        std::thread::park();
    }
}

/// Spawns the MIDI reader thread, like [`super::spawn_osc_reader`] does for
/// OSC. An ALSA sequencer that cannot open logs and leaves the rest of the
/// app running — the reader is a convenience, not a precondition.
pub fn spawn(
    shared: Arc<Mutex<crate::Shared>>,
    host: String,
    http_port: u16,
    port_filter: Option<String>,
) {
    let _ = std::thread::Builder::new()
        .name("metal-dash midi".into())
        .spawn(move || run(shared, host, http_port, port_filter));
}

#[cfg(test)]
mod tests {
    use super::decode_message;

    /// Note On with a live velocity is a note; the channel nibble does not
    /// matter and the note number is `data1`.
    #[test]
    fn note_on_with_velocity_maps_to_a_note() {
        assert_eq!(decode_message(0x90, 60, 100), Some((None, Some(60))));
        assert_eq!(decode_message(0x9F, 64, 1), Some((None, Some(64))));
    }

    /// Note On with velocity 0 is the running-status note off (MIDI 1.0), so
    /// it must not fire.
    #[test]
    fn note_on_with_velocity_zero_is_a_note_off() {
        assert_eq!(decode_message(0x90, 60, 0), None);
        assert_eq!(decode_message(0x9F, 60, 0), None);
    }

    /// Control Change is a cc; `data1` is the controller, `data2` is ignored.
    #[test]
    fn control_change_maps_to_a_cc() {
        assert_eq!(decode_message(0xB0, 64, 127), Some((Some(64), None)));
        assert_eq!(decode_message(0xBF, 7, 0), Some((Some(7), None)));
    }

    /// Everything else is ignored: note off, pitch bend and system messages.
    #[test]
    fn everything_else_is_ignored() {
        assert_eq!(decode_message(0x80, 60, 0), None);
        assert_eq!(decode_message(0xE0, 0, 64), None);
        assert_eq!(decode_message(0xF0, 0, 0), None);
    }
}
