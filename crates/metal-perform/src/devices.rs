//! What the machine can listen to and what it can draw on.
//!
//! Enumerated from the running system rather than configured, because the
//! whole point of the INPUT/OUTPUT panel is answering "will this work in this
//! room, on this laptop, tonight" — and a list of devices someone typed into
//! a config file last month cannot answer that.
//!
//! Both halves shell out and both degrade to an empty list rather than an
//! error. A venue laptop with no second screen is a normal state, not a
//! failure, and so is one where the compositor is not Hyprland.
//!
//! **Nothing here opens a device**, and that is still true — but the choice is
//! no longer inert. `metal-dash --listen <id>` passes one of these ids to
//! `crate::live`, which opens it for real. The id is what `pactl` calls the
//! source, which is exactly what the capture side wants.
//!
//! One thing worth knowing before trusting any other listing: a PipeWire
//! *monitor* — the `.monitor` entry below, and the only one that hears the mix
//! rather than the room — **cannot be opened through cpal at all**. See
//! `metal_listen::capture` for the measurement. This list is right; ALSA's is not.

use std::sync::{Mutex, OnceLock};
use web_time::{Duration, Instant};
use metal_schema::platform::Command;

use serde_json::{json, Value};

/// How long one enumeration is reused.
///
/// Every call here is a process spawn (`pactl`, `hyprctl`, `xrandr`) — a few
/// milliseconds each, and the panel reads `/state` once per frame from every
/// client. Without this, a single frame paid four spawns for a list that had
/// not changed since the last one; devices change when somebody plugs
/// something in, not sixty times a second. Two seconds keeps a freshly
/// plugged monitor a blink away while taking the spawns off the frame.
const TTL: Duration = Duration::from_secs(2);

/// How audio would reach the digest. The two the handoff requires, plus the
/// stand-in that is the only one that works today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A microphone in the room.
    Microphone,
    /// The laptop's own output, captured back — what a DJ mixing on this
    /// machine actually wants. PipeWire and PulseAudio both expose it as a
    /// source whose name ends in `.monitor`.
    Internal,
}

#[derive(Clone, Debug)]
pub struct Input {
    /// What the audio stack calls it. Stable across runs; what a saved choice
    /// is stored as.
    pub id: String,
    /// What a human should read.
    pub label: String,
    pub kind: Kind,
}

#[derive(Clone, Debug)]
pub struct Display {
    pub id: String,
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub hz: f32,
    /// The screen this app's own window is on — the preview screen, and so
    /// never the one you want the visuals fullscreened onto.
    pub primary: bool,
}

/// Microphone or internal (monitor) capture, from the id alone.
///
/// Pulled out of [`inputs`] rather than left inline: `the design notes`
/// Conjunto G1 wants exactly this distinction for the `audio.live` graph
/// node's config, and computing it a second way there — instead of asking the
/// one function that already knows `.monitor` is the tell — is how a project
/// ends up with two answers to "is this a microphone".
pub fn kind_of(id: &str) -> Kind {
    if id.ends_with(".monitor") {
        Kind::Internal
    } else {
        Kind::Microphone
    }
}

/// Audio sources, as `pactl` reports them. Empty if PipeWire/PulseAudio is
/// not running, which is a true statement about the machine.
pub fn inputs() -> Vec<Input> {
    cached().0
}

/// One enumeration, held for [`TTL`].
struct Cached {
    at: Instant,
    inputs: Vec<Input>,
    displays: Vec<Display>,
}

/// The shared cache both public readers go through. The lock is held across
/// the spawns on a miss — one caller pays them, everyone else waits for that
/// one list rather than spawning a second set in parallel.
fn cached() -> (Vec<Input>, Vec<Display>) {
    static CACHE: OnceLock<Mutex<Option<Cached>>> = OnceLock::new();
    let mut guard = CACHE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("devices cache");
    let stale = guard
        .as_ref()
        .map(|c| c.at.elapsed() >= TTL)
        .unwrap_or(true);
    if stale {
        *guard = Some(Cached {
            at: Instant::now(),
            inputs: enumerate_inputs(),
            displays: enumerate_displays(),
        });
    }
    let c = guard.as_ref().expect("just filled above");
    (c.inputs.clone(), c.displays.clone())
}

fn enumerate_inputs() -> Vec<Input> {
    let Some(text) = run("pactl", &["list", "short", "sources"]) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let _index = cols.next()?;
            let name = cols.next()?.trim();
            if name.is_empty() {
                return None;
            }
            let kind = kind_of(name);
            Some(Input {
                id: name.to_string(),
                label: pretty(name, kind),
                kind,
            })
        })
        .collect()
}

/// One program's playback stream — what a "capture this app's audio" pick
/// opens (`metal_listen::capture` reads `app:<object.serial>` through
/// `pw-record --target`). Listed live, not cached: a stream exists only while
/// the program plays, and the panel asks when the operator opens the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppStream {
    /// `app:<object.serial>` — the form the listener opens.
    pub id: String,
    pub label: String,
}

/// Every program currently sending audio, from `pactl -f json list
/// sink-inputs`. Empty without PulseAudio/PipeWire tools.
pub fn apps() -> Vec<AppStream> {
    let Some(text) = run("pactl", &["-f", "json", "list", "sink-inputs"]) else {
        return Vec::new();
    };
    parse_apps(&text)
}

fn parse_apps(text: &str) -> Vec<AppStream> {
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|s| {
            let p = &s["properties"];
            let serial = p["object.serial"]
                .as_str()
                .map(str::to_string)
                .or_else(|| p["object.serial"].as_u64().map(|n| n.to_string()))?;
            let app = p["application.name"].as_str().or(p["application.process.binary"].as_str()).unwrap_or("app");
            let media = p["media.name"].as_str().filter(|m| !m.is_empty() && *m != "(null)");
            Some(AppStream {
                id: format!("app:{serial}"),
                label: match media {
                    Some(m) => format!("app · {app} · {m}"),
                    None => format!("app · {app}"),
                },
            })
        })
        .collect()
}

/// Turns `alsa_output.pci-0000_00_1f.3.analog-stereo.monitor` into something
/// readable at a glance in a dark booth.
fn pretty(name: &str, kind: Kind) -> String {
    let core = name
        .trim_end_matches(".monitor")
        .rsplit('.')
        .next()
        .unwrap_or(name)
        .replace(['_', '-'], " ");
    match kind {
        Kind::Internal => format!("internal capture · {core}"),
        Kind::Microphone => format!("microphone · {core}"),
    }
}

/// Screens, from Hyprland. Falls back to `xrandr` so this is not silently
/// empty under a different compositor, and to nothing at all if neither
/// answers.
pub fn displays() -> Vec<Display> {
    cached().1
}

fn enumerate_displays() -> Vec<Display> {
    if let Some(list) = hyprland_displays() {
        if !list.is_empty() {
            return list;
        }
    }
    xrandr_displays()
}

fn hyprland_displays() -> Option<Vec<Display>> {
    let text = run("hyprctl", &["monitors", "-j"])?;
    let parsed: Value = serde_json::from_str(&text).ok()?;
    let monitors = parsed.as_array()?;
    Some(
        monitors
            .iter()
            .map(|m| {
                let id = m["name"].as_str().unwrap_or("?").to_string();
                let width = m["width"].as_u64().unwrap_or(0) as u32;
                let height = m["height"].as_u64().unwrap_or(0) as u32;
                let hz = m["refreshRate"].as_f64().unwrap_or(0.0) as f32;
                let primary = m["focused"].as_bool().unwrap_or(false);
                let description = m["description"].as_str().unwrap_or("").trim();
                let label = if description.is_empty() {
                    id.clone()
                } else {
                    format!("{id} · {description}")
                };
                Display {
                    id,
                    label,
                    width,
                    height,
                    hz,
                    primary,
                }
            })
            .collect(),
    )
}

fn xrandr_displays() -> Vec<Display> {
    let Some(text) = run("xrandr", &["--query"]) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| l.contains(" connected"))
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let id = cols.next()?.to_string();
            let primary = line.contains("primary");
            // The mode is the first token shaped like 1920x1080+0+0.
            let (mut width, mut height) = (0, 0);
            for token in line.split_whitespace() {
                if let Some((w, rest)) = token.split_once('x') {
                    if let (Ok(w), Ok(h)) = (
                        w.parse::<u32>(),
                        rest.split(['+', '_']).next().unwrap_or("").parse::<u32>(),
                    ) {
                        (width, height) = (w, h);
                        break;
                    }
                }
            }
            Some(Display {
                label: id.clone(),
                id,
                width,
                height,
                hz: 0.0,
                primary,
            })
        })
        .collect()
}

/// Runs a command and returns its stdout, or `None` if it is missing or
/// failed. Never panics: every caller here is enumerating optional hardware.
fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The whole panel's state, as the page reads it.
pub fn describe(selected_input: Option<&str>, selected_display: Option<&str>) -> Value {
    let (inputs, displays) = cached();
    json!({
        "inputs": inputs.iter().map(|i| json!({
            "id": i.id,
            "label": i.label,
            "kind": match i.kind { Kind::Internal => "internal", Kind::Microphone => "microphone" },
            // Nothing is captured yet: the digest is the stand-in either way.
            // The page greys these rather than hiding them, so what is coming
            // is visible and what is missing is honest.
            "available": false,
        })).collect::<Vec<_>>(),
        "displays": displays.iter().map(|d| json!({
            "id": d.id,
            "label": d.label,
            "mode": format!("{}x{}{}", d.width, d.height,
                            if d.hz > 0.0 { format!(" @{:.0}", d.hz) } else { String::new() }),
            "primary": d.primary,
        })).collect::<Vec<_>>(),
        "apps": apps().iter().map(|a| json!({ "id": a.id, "label": a.label, "kind": "app" })).collect::<Vec<_>>(),
        "selected_input": selected_input,
        "selected_display": selected_display,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_streams_are_listed_by_object_serial() {
        let text = r#"[{"index":1881,"properties":{"object.serial":"1881","application.name":"Firefox","media.name":"YouTube"}},
                      {"index":3,"properties":{"application.name":"no serial"}}]"#;
        assert_eq!(
            parse_apps(text),
            vec![AppStream { id: "app:1881".into(), label: "app · Firefox · YouTube".into() }]
        );
        assert!(parse_apps("not json").is_empty());
    }

    #[test]
    fn a_monitor_source_is_read_as_internal_capture_not_as_a_microphone() {
        // This is the distinction the handoff's two input types turn on, and
        // it is carried entirely by the `.monitor` suffix. Getting it backwards
        // would offer a DJ a microphone where they wanted their own mix.
        assert_eq!(
            pretty(
                "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor",
                Kind::Internal
            ),
            "internal capture · analog stereo"
        );
        assert_eq!(
            pretty(
                "alsa_input.pci-0000_00_1f.3.analog-stereo",
                Kind::Microphone
            ),
            "microphone · analog stereo"
        );
    }

    #[test]
    fn kind_of_is_the_same_rule_inputs_uses_inline() {
        assert_eq!(
            kind_of("alsa_output.pci-0000_00_1f.3.analog-stereo.monitor"),
            Kind::Internal
        );
        assert_eq!(
            kind_of("alsa_input.pci-0000_00_1f.3.analog-stereo"),
            Kind::Microphone
        );
    }

    #[test]
    fn enumeration_never_panics_and_never_errors_on_this_machine() {
        // Both of these shell out. A venue laptop missing either tool must
        // give an empty list, not a crash on the way to the stage.
        let _ = inputs();
        let _ = displays();
        let payload = describe(None, None);
        assert!(payload["inputs"].is_array());
        assert!(payload["displays"].is_array());
    }

    #[test]
    fn a_missing_program_is_none_rather_than_a_panic() {
        assert!(run("fw-definitely-not-a-real-program", &[]).is_none());
    }
}
