//! The lanes playhead as the shell last drew it, read by `GET /author/play`
//! (`playhead_s`) beside the audio clock, for measuring the two against each
//! other. Written by the lanes panel each frame; read-only from outside.
//! Top-level so the shell and the call surface both reach it without either
//! importing the other.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

static TICK: AtomicUsize = AtomicUsize::new(0);
static ON: AtomicBool = AtomicBool::new(false);
/// Paired with [`STARTED`] so a timecode thread idling while the transport is
/// stopped can sleep until it starts, rather than polling the atomics.
static WAKE: Mutex<()> = Mutex::new(());
static STARTED: Condvar = Condvar::new();

/// Publishes the lanes playhead (`tick` at the editor's rate) and whether the
/// transport runs.
pub fn publish(tick: usize, playing: bool) {
    TICK.store(tick, Ordering::Relaxed);
    let was = ON.swap(playing, Ordering::Relaxed);
    if playing && !was {
        // Taking the lock first closes the window where a waiter has checked
        // `ON` but not yet started waiting.
        let _guard = WAKE.lock();
        STARTED.notify_all();
    }
}

/// Blocks while the transport is stopped, for at most `timeout` (so the caller
/// can still check its stop flag). Returns at once when it is running.
pub fn wait_while_stopped(timeout: Duration) {
    let Ok(guard) = WAKE.lock() else { return };
    if !ON.load(Ordering::Relaxed) {
        let _ = STARTED.wait_timeout_while(guard, timeout, |_| !ON.load(Ordering::Relaxed));
    }
}

/// The lanes playhead in seconds at `rate` ticks a second, and whether the
/// transport runs. Before the panel has drawn once it is the start, not
/// playing.
pub fn published(rate: f32) -> (f64, bool) {
    let tick = TICK.load(Ordering::Relaxed);
    (tick as f64 / f64::from(rate), ON.load(Ordering::Relaxed))
}
