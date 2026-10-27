//! P-C: video in and out of other programs.
//!
//! A [`Sink`] takes the device's finished frame — the same pixels `GET
//! /output.jpg` encodes — and hands it to another program: a v4l2loopback
//! virtual camera, an NDI sender, Spout or Syphon. A sink is a *view* of the
//! frame (rule 3): it never picks a model, it is fed whatever the device
//! composed. A [`Source`] is the other direction: an NDI stream as a `capture`
//! node's picture.
//!
//! Every kind answers [`modes`] with `available` and, when not, the reason in
//! the operator's language — a missing kernel module or vendor runtime is a
//! disabled row, never a build failure or a panic.

pub mod convert;
pub mod share;
pub mod v4l2;
#[cfg(not(target_arch = "wasm32"))]
pub mod ndi;
#[cfg(all(target_os = "windows", feature = "spout"))]
pub mod spout;
#[cfg(all(target_os = "macos", feature = "syphon"))]
pub mod syphon;

/// Frames out.
pub trait Sink: Send {
    /// One RGBA frame, `w × h`, at show time `t` seconds.
    fn push(&mut self, rgba: &[u8], w: u32, h: u32, t: f64) -> Result<(), String>;
    /// One line for the status read.
    fn describe(&self) -> String;
}

/// Frames in.
pub trait Source: Send {
    /// The newest frame and its size, `None` until one arrives.
    fn frame(&mut self) -> Option<(Vec<u8>, u32, u32)>;
}

/// Every sink kind, in the order the properties list shows them.
pub const KINDS: [&str; 4] = ["virtualcam", "ndi", "spout", "syphon"];

/// One sink kind as the properties page and the AI read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mode {
    pub kind: &'static str,
    pub available: bool,
    /// Why it is off; `None` when available.
    pub reason: Option<String>,
    /// `(target, label)` choices — loopback devices; empty where the target
    /// is a free name (NDI, Spout, Syphon).
    pub targets: Vec<(String, String)>,
}

fn reason_of(kind: &str) -> Option<String> {
    match kind {
        "virtualcam" => {
            if cfg!(any(target_os = "windows", target_os = "macos")) {
                Some("virtual camera on Windows/macOS: needs a signed driver or extension (scaling)".into())
            } else {
                v4l2::disabled_reason()
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        "ndi" => ndi::disabled_reason(),
        "spout" => spout_reason(),
        "syphon" => syphon_reason(),
        _ => Some(format!("unknown output kind: {kind}")),
    }
}

#[cfg(all(target_os = "windows", feature = "spout"))]
fn spout_reason() -> Option<String> {
    spout::disabled_reason()
}
#[cfg(all(target_os = "windows", not(feature = "spout")))]
fn spout_reason() -> Option<String> {
    Some("this build does not include Spout (feature `spout`)".into())
}
#[cfg(not(target_os = "windows"))]
fn spout_reason() -> Option<String> {
    Some("not available on this platform: Spout only exists on Windows".into())
}

#[cfg(all(target_os = "macos", feature = "syphon"))]
fn syphon_reason() -> Option<String> {
    syphon::disabled_reason()
}
#[cfg(all(target_os = "macos", not(feature = "syphon")))]
fn syphon_reason() -> Option<String> {
    Some("this build does not include Syphon (feature `syphon`)".into())
}
#[cfg(not(target_os = "macos"))]
fn syphon_reason() -> Option<String> {
    Some("not available on this platform: Syphon only exists on macOS".into())
}

/// Every kind with its availability on this machine right now.
pub fn modes() -> Vec<Mode> {
    KINDS
        .iter()
        .map(|&kind| {
            let reason = reason_of(kind);
            let targets = if kind == "virtualcam" && reason.is_none() {
                v4l2::devices().into_iter().map(|d| (d.path, d.label)).collect()
            } else {
                Vec::new()
            };
            Mode { kind, available: reason.is_none(), reason, targets }
        })
        .collect()
}

/// Whether `kind` names a sink at all (validation before availability).
pub fn is_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

/// The sink for `kind`, opened on `target` (`/dev/videoN[:rgb24]` for the
/// virtual camera, the published name for the others). A disabled kind answers
/// its reason.
pub fn open(kind: &str, target: &str) -> Result<Box<dyn Sink>, String> {
    if !is_kind(kind) {
        return Err(format!("unknown output kind: {kind}"));
    }
    if let Some(why) = reason_of(kind) {
        return Err(why);
    }
    match kind {
        "virtualcam" => Ok(Box::new(v4l2::VirtualCam::open(target)?)),
        #[cfg(not(target_arch = "wasm32"))]
        "ndi" => Ok(Box::new(ndi::NdiSend::open(target)?)),
        #[cfg(all(target_os = "windows", feature = "spout"))]
        "spout" => Ok(Box::new(spout::open(target)?)),
        #[cfg(all(target_os = "macos", feature = "syphon"))]
        "syphon" => Ok(Box::new(syphon::open(target)?)),
        _ => Err(format!("{kind} not available in this build")),
    }
}

/// The `capture` node's picture for a source id this crate owns (`ndi:<name>`),
/// or `None` for ids it does not.
pub fn capture_frame(source: &str) -> Option<(Vec<u8>, u32, u32)> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(name) = source.strip_prefix("ndi:") {
        return ndi::receive_frame(name);
    }
    let _ = source;
    None
}

/// The capture sources this crate offers: `(id, label)`.
pub fn capture_sources() -> Vec<(String, String)> {
    #[cfg(not(target_arch = "wasm32"))]
    if ndi::disabled_reason().is_none() {
        return ndi::sources(std::time::Duration::from_millis(300))
            .into_iter()
            .map(|n| (format!("ndi:{n}"), format!("NDI · {n}")))
            .collect();
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_listed_once_and_disabled_ones_say_why() {
        let m = modes();
        assert_eq!(m.iter().map(|m| m.kind).collect::<Vec<_>>(), KINDS.to_vec());
        for mode in &m {
            assert_eq!(mode.available, mode.reason.is_none(), "{mode:?}");
            if let Some(r) = &mode.reason {
                assert!(!r.trim().is_empty());
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn spout_and_syphon_are_disabled_off_their_platform() {
        let m = modes();
        let spout = m.iter().find(|m| m.kind == "spout").unwrap();
        assert!(!spout.available && spout.reason.as_deref().unwrap().contains("platform"));
        let syphon = m.iter().find(|m| m.kind == "syphon").unwrap();
        assert!(!syphon.available && syphon.reason.as_deref().unwrap().contains("platform"));
    }

    #[test]
    fn a_kind_without_its_feature_or_platform_names_the_reason() {
        // Every kind has a sentence, whatever the target: the catalog entry
        // exists everywhere and says why it is off here.
        for kind in ["spout", "syphon"] {
            let m = modes().into_iter().find(|m| m.kind == kind).unwrap();
            assert!(m.reason.as_deref().is_some_and(|r| !r.trim().is_empty()), "{kind}");
        }
    }

    #[test]
    fn opening_selects_by_kind_and_refuses_unknown_or_disabled() {
        assert!(open("hdmi", "x").err().unwrap().contains("unknown"));
        for mode in modes().into_iter().filter(|m| !m.available) {
            assert_eq!(open(mode.kind, "x").err(), mode.reason, "{}", mode.kind);
        }
    }

    #[test]
    fn capture_frame_ignores_ids_it_does_not_own() {
        assert!(capture_frame("v4l2:/dev/video0").is_none());
    }
}
