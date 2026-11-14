//! The global frame-rate cap (`fps_cap` in `data/settings.json`): top-level so
//! both the settings route (`call::settings`) and the shell's pacing read the
//! one value without the shell importing the door's submodules.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::time::Duration;

/// The caps the settings dropdown offers.
pub const FPS_CAPS: [u32; 9] = [30, 60, 90, 120, 144, 165, 200, 240, 300];
pub const DEFAULT_FPS_CAP: u32 = 60;

static CAP: AtomicU32 = AtomicU32::new(DEFAULT_FPS_CAP);

/// The active cap.
pub fn get() -> u32 {
    CAP.load(Relaxed)
}

/// Adopts `n` when it is one of [`FPS_CAPS`]; anything else is ignored.
pub fn set(n: u32) {
    if FPS_CAPS.contains(&n) {
        CAP.store(n, Relaxed);
    }
}

/// The minimum time between two frames under the active cap.
pub fn gap() -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(get()))
}
