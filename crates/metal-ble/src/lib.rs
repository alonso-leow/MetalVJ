//! metal-ble: Bluetooth lights (Godox Bluetooth Mesh) driven from a show.
//!
//! This is the Phase-A Bluetooth Mesh link. The modules below build the whole
//! stack so that only [`link`] ever touches a radio:
//!
//! * [`crypto`] — s1/k1/k2/k3/k4, AES-CCM, header obfuscation.
//! * [`pdu`] — access / lower / network PDUs and the GATT Proxy framing.
//! * [`godox`] — the vendor command bytes (level, CCT, HSI).
//! * [`state`] — the mesh keys/addresses and crash-safe sequence reservation.
//! * [`provision`] — PB-GATT provisioning (ECDH, confirmations, session keys).
//! * [`transport`] — btleplug scan/connect/GATT I/O.
//! * [`link`] — the [`Link`] implementation: coalescing, 25 Hz, reconnect.
//!
//! [`open`] wires them together and returns a [`Link`]. [`Recorder`] is a
//! non-radio [`Link`] for callers' own tests.

pub mod crypto;
pub mod godox;
#[cfg(not(target_arch = "wasm32"))]
pub mod link;
pub mod pdu;
pub mod provision;
pub mod state;
#[cfg(not(target_arch = "wasm32"))]
pub mod transport;

use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightState {
    pub level: f32,
    pub cct_kelvin: Option<f32>,
    pub hsi: Option<Hsi>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsi {
    pub hue_deg: f32,
    pub sat: f32,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LinkConfig {
    pub protocol: Protocol,
    pub address: String,
    pub node: Option<u16>,
    pub mesh: std::path::PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    GodoxMesh,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkStatus {
    Connecting,
    Connected,
    Failed(String),
}

pub trait Link: Send {
    fn push(&mut self, state: &LightState) -> bool;
    fn status(&self) -> LinkStatus;
}

/// Open the real Bluetooth Mesh link for `config`.
///
/// Spawns one worker thread that owns a current-thread tokio runtime, connects
/// to the Godox proxy and sends coalesced, rate-limited commands. See
/// [`link`] for the policy (latest-wins, 25 Hz, reconnect with backoff).
///
/// `push` after `open` never blocks and returns `false` only if the link has
/// failed (for example the mesh file could not be read).
#[cfg(not(target_arch = "wasm32"))]
pub fn open(config: LinkConfig) -> Box<dyn Link> {
    Box::new(link::LinkHandle::spawn(config))
}

/// On wasm32 there is no radio: the link exists so the engine's wiring is the
/// same, and it reports itself failed from the first frame. The page shows
/// that as a dead Bluetooth node, not as a crash.
#[cfg(target_arch = "wasm32")]
pub fn open(_config: LinkConfig) -> Box<dyn Link> {
    let (recorder, _log) = Recorder::new();
    Box::new(recorder.with_status(LinkStatus::Failed(
        "no Bluetooth radio in the browser".into(),
    )))
}

#[derive(Clone, Default)]
pub struct RecorderLog(Arc<Mutex<Vec<LightState>>>);

impl RecorderLog {
    pub fn states(&self) -> Vec<LightState> {
        self.0.lock().map(|v| v.clone()).unwrap_or_default()
    }
    pub fn last(&self) -> Option<LightState> {
        self.0.lock().ok().and_then(|v| v.last().copied())
    }
    pub fn len(&self) -> usize {
        self.0.lock().map(|v| v.len()).unwrap_or(0)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn clear(&self) {
        if let Ok(mut v) = self.0.lock() {
            v.clear();
        }
    }
}

pub struct Recorder {
    log: RecorderLog,
    status: LinkStatus,
}

impl Recorder {
    pub fn new() -> (Recorder, RecorderLog) {
        let log = RecorderLog::default();
        (Recorder { log: log.clone(), status: LinkStatus::Connected }, log)
    }
    pub fn with_status(mut self, status: LinkStatus) -> Self {
        self.status = status;
        self
    }
}

impl Link for Recorder {
    fn push(&mut self, state: &LightState) -> bool {
        if self.status != LinkStatus::Connected {
            return false;
        }
        match self.log.0.lock() {
            Ok(mut v) => {
                v.push(*state);
                true
            }
            Err(_) => false,
        }
    }
    fn status(&self) -> LinkStatus {
        self.status.clone()
    }
}
