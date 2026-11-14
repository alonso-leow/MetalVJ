#![cfg(feature = "midi")]
//! O.10b: the MIDI reader — one thread that keeps every matching input port
//! open (re-scanning, so a controller plugged in after start is picked up)
//! and hands each message two ways:
//!
//! - **the control surface**: every decoded message (note on *and* off, CC
//!   with its value, pitch bend) goes on [`super::push_surface`]'s queue,
//!   which the overlays grid drains each frame — bindings, dials, `rec`,
//!   `fn` and the piano all live there;
//! - **triggers**: a Note On or a CC still asks the shared [`super::learn`]
//!   arm and the `Shared` state whose `Trigger`s should fire, and posts
//!   `POST /trigger/<id>` over loopback, as before (`regla 6`).
//!
//! The reader never touches `State` beyond that read and never blocks the app
//! (midir runs the callback on its own thread); a bad message or a failed post
//! only logs — one stray byte must not take the reader down.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use metal_midi::Msg;

use super::{learn, post_trigger};

/// How often the port list is re-read for a controller plugged in (or pulled)
/// since the last look.
const RESCAN: Duration = Duration::from_secs(2);

/// The trigger half of a message: `(cc, note)`, exactly one `Some`, for a
/// Note On or a CC — the two a `TriggerSource::Midi` can name.
pub fn trigger_key(msg: &Msg) -> Option<(Option<u8>, Option<u8>)> {
    match *msg {
        Msg::NoteOn { note, .. } => Some((None, Some(note))),
        Msg::Cc { cc, .. } => Some((Some(cc), None)),
        _ => None,
    }
}

fn on_msg(shared: &Arc<Mutex<crate::Shared>>, host: &str, http_port: u16, port: &str, msg: Msg) {
    super::push_surface(super::SurfaceEvent { port: port.to_string(), input: msg.input_on_channel(), edge: msg.edge() });
    let Some((cc, note)) = trigger_key(&msg) else {
        return;
    };
    // Learn wins: while the arm is this reader's kind, the first thing heard is
    // written into the armed trigger and the active matches are skipped.
    if let Some(arm) = learn().lock().ok().and_then(|g| g.clone()) {
        if arm.kind == "midi" {
            let body = serde_json::json!({
                "op": "learn",
                "source": { "source": "midi", "port": port, "cc": cc, "note": note },
            });
            let _ = post_trigger(host, http_port, &arm.trigger, &body);
            return;
        }
    }
    let targets: Vec<String> = {
        let guard = match shared.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard
            .state
            .sources
            .values()
            .flat_map(|dev| dev.triggers.iter())
            .filter(|t| t.source.matches_midi(port, cc, note))
            .map(|t| t.id.clone())
            .collect()
    };
    for id in targets {
        let _ = post_trigger(host, http_port, &id, &serde_json::json!({}));
    }
}

/// Keeps the matching ports open forever, re-opening whenever the set of
/// ports changes. `port_filter` is a substring of the port name to accept
/// (`None` takes every port but ALSA's `Midi Through`).
pub fn run(shared: Arc<Mutex<crate::Shared>>, host: String, http_port: u16, port_filter: Option<String>) {
    let mut seen: Option<Vec<String>> = None;
    let mut open: Option<metal_midi::ports::Inputs> = None;
    loop {
        match metal_midi::ports::list() {
            Ok(ports) if seen.as_ref() != Some(&ports) => {
                // Close first: ALSA will not hand the same port out twice.
                drop(open.take());
                let (shared, host) = (Arc::clone(&shared), host.clone());
                let cb = move |port: &str, msg: Msg| on_msg(&shared, &host, http_port, port, msg);
                match metal_midi::ports::open(port_filter.as_deref(), cb) {
                    Ok(inputs) => {
                        for name in &inputs.names {
                            let known = metal_midi::profile(name).map(|p| format!(" ({p})")).unwrap_or_default();
                            eprintln!("  midi: listening on {name:?}{known}");
                        }
                        if inputs.names.is_empty() {
                            eprintln!("  midi: no input ports (filter {port_filter:?}) — waiting for one");
                        }
                        super::set_midi_ports(inputs.names.clone());
                        open = Some(inputs);
                    }
                    Err(e) => eprintln!("  midi: cannot open the sequencer: {e}"),
                }
                seen = Some(ports);
            }
            Ok(_) => {}
            Err(e) => {
                if seen.is_some() || open.is_none() {
                    eprintln!("  midi: cannot list ports: {e}");
                }
                seen = None;
            }
        }
        std::thread::sleep(RESCAN);
    }
}

/// Spawns the MIDI reader thread. A sequencer that cannot open logs and leaves
/// the rest of the app running — the reader is a convenience, not a
/// precondition.
pub fn spawn(shared: Arc<Mutex<crate::Shared>>, host: String, http_port: u16, port_filter: Option<String>) {
    let _ = std::thread::Builder::new()
        .name("metal-dash midi".into())
        .spawn(move || run(shared, host, http_port, port_filter));
}

#[cfg(test)]
mod tests {
    use super::trigger_key;
    use metal_midi::decode;

    /// A Note On with a live velocity is a note trigger; the channel nibble
    /// does not matter.
    #[test]
    fn note_on_with_velocity_maps_to_a_note() {
        assert_eq!(trigger_key(&decode(&[0x90, 60, 100]).unwrap()), Some((None, Some(60))));
        assert_eq!(trigger_key(&decode(&[0x9F, 64, 1]).unwrap()), Some((None, Some(64))));
    }

    /// Note On with velocity 0 is the running-status note off (MIDI 1.0), so
    /// it fires no trigger (the surface still hears the release).
    #[test]
    fn note_off_fires_no_trigger() {
        assert_eq!(trigger_key(&decode(&[0x90, 60, 0]).unwrap()), None);
        assert_eq!(trigger_key(&decode(&[0x80, 60, 0]).unwrap()), None);
    }

    /// Control Change is a cc trigger whatever its value.
    #[test]
    fn control_change_maps_to_a_cc() {
        assert_eq!(trigger_key(&decode(&[0xB0, 64, 127]).unwrap()), Some((Some(64), None)));
        assert_eq!(trigger_key(&decode(&[0xBF, 7, 0]).unwrap()), Some((Some(7), None)));
    }

    /// Pitch bend reaches the surface only.
    #[test]
    fn bend_fires_no_trigger() {
        assert_eq!(trigger_key(&decode(&[0xE0, 0, 64]).unwrap()), None);
    }
}
