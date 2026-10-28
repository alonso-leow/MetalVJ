//! The port side (feature `ports`): list the system's MIDI inputs and keep a
//! connection open on every one whose name matches, each decoded message
//! handed to one callback with its port name.

use midir::{MidiInput, MidiInputConnection};

use crate::{decode, Msg};

/// The input ports the system has right now, by name.
pub fn list() -> Result<Vec<String>, String> {
    let probe = MidiInput::new("metalvj-probe").map_err(|e| e.to_string())?;
    Ok(probe.ports().iter().filter_map(|p| probe.port_name(p).ok()).collect())
}

/// Open connections; dropping it closes every port.
pub struct Inputs {
    pub names: Vec<String>,
    _connections: Vec<MidiInputConnection<()>>,
}

/// Connects every input port whose name contains `filter` (all of them when
/// `None`; ALSA's own `Midi Through` and this app's own ports — the MTC
/// output — are skipped unless the filter names them).
/// `on_msg(port, msg)` runs on midir's thread for each decoded message.
pub fn open<F>(filter: Option<&str>, on_msg: F) -> Result<Inputs, String>
where
    F: Fn(&str, Msg) + Send + Sync + Clone + 'static,
{
    let probe = MidiInput::new("metalvj").map_err(|e| e.to_string())?;
    let wanted: Vec<_> = probe
        .ports()
        .into_iter()
        .filter_map(|p| {
            let name = probe.port_name(&p).ok()?;
            let keep = match filter {
                Some(f) => name.contains(f),
                None => !name.contains("Midi Through") && !name.contains("metalvj"),
            };
            keep.then_some((p, name))
        })
        .collect();
    drop(probe);
    let mut names = Vec::new();
    let mut connections = Vec::new();
    for (port, name) in wanted {
        // `connect` consumes its `MidiInput`, so each port gets its own.
        let input = MidiInput::new("metalvj").map_err(|e| e.to_string())?;
        let cb = on_msg.clone();
        let port_name = name.clone();
        match input.connect(
            &port,
            "metalvj-in",
            move |_stamp, bytes, _| {
                if let Some(msg) = decode(bytes) {
                    cb(&port_name, msg);
                }
            },
            (),
        ) {
            Ok(c) => {
                names.push(name);
                connections.push(c);
            }
            Err(e) => eprintln!("  midi: cannot connect {name:?}: {e}"),
        }
    }
    Ok(Inputs { names, _connections: connections })
}
