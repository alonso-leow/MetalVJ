//! The global UI scale (`ui_scale` in `data/settings.json`), a percentage:
//! top-level, like `frame_cap`, so the settings route validates it on every
//! build and the shell applies it as egui's zoom factor.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

pub const MIN: u32 = 75;
pub const MAX: u32 = 200;
pub const STEP: u32 = 5;
pub const DEFAULT: u32 = 100;

static PCT: AtomicU32 = AtomicU32::new(DEFAULT);

pub fn get() -> u32 {
    PCT.load(Relaxed)
}

/// Adopts `n` when it is in `MIN..=MAX`; anything else is ignored.
pub fn set(n: u32) {
    if (MIN..=MAX).contains(&n) {
        PCT.store(n, Relaxed);
    }
}

/// The egui zoom factor for the active percentage.
pub fn factor() -> f32 {
    get() as f32 / 100.0
}

/// `pct` moved by `steps` notches, clamped; `0` steps resets to the default.
pub fn stepped(pct: u32, steps: i32) -> u32 {
    if steps == 0 {
        return DEFAULT;
    }
    (pct as i32 + steps * STEP as i32).clamp(MIN as i32, MAX as i32) as u32
}
