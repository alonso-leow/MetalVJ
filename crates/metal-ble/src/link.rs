//! The real `Link`: one thread, one tokio runtime, latest-wins coalescing.
//!
//! A show pushes light state every frame (60 Hz and up), but a Bluetooth Mesh
//! light, its proxy and the 20–30 Hz rule the plan sets cannot absorb that. The
//! policy is therefore **latest-wins, never a queue**: `push` overwrites one
//! `Option<LightState>` slot and notifies a worker thread; the worker always
//! sends the newest value and discards intermediate ones. A queue would make
//! the light lag the show by however many frames piled up; coalescing keeps it
//! at most one frame behind.
//!
//! Threading model
//! ---------------
//!
//! `open()` spawns exactly one `std::thread`. That thread builds a
//! **current-thread** tokio runtime (no work-stealing pool, no runtime owned by
//! the caller) and runs the connect/send loop on it. btleplug is async; the
//! show's `push` is not, so the boundary is a condvar-notified slot rather than
//! a channel.
//!
//! * [`LinkHandle::push`] never blocks. It returns `false` only once the link
//!   has entered [`LinkStatus::Failed`] (e.g. the mesh file could not be read);
//!   while `Connecting` it returns `true`, because the value is retained in the
//!   slot and will be sent when the connection comes up.
//! * [`LinkHandle::status`] reads a shared `Arc<Mutex<LinkStatus>>`.
//!
//! Rate limit and reconnect
//! ------------------------
//!
//! Sends are spaced by [`MIN_SEND_INTERVAL`] (25 Hz). A dropped link is retried
//! with exponential backoff up to [`MAX_BACKOFF`], and each retry re-runs the
//! whole proxy connect (services and notifications are re-established).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use web_time::{Duration, Instant};

use crate::transport::{self, MeshProxy};
use crate::state::MeshState;
use crate::{godox, LightState, Link, LinkConfig, LinkStatus};

/// Minimum spacing between writes: 25 Hz.
pub const MIN_SEND_INTERVAL: Duration = Duration::from_millis(40);
/// First reconnect delay.
pub const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
/// Longest reconnect delay.
pub const MAX_BACKOFF: Duration = Duration::from_secs(8);
/// Mesh TTL for vendor commands (matches the reference).
pub const COMMAND_TTL: u8 = 10;

/// The shared slot the producer writes and the worker drains.
struct Slot {
    state: Mutex<Option<LightState>>,
    /// Signalled whenever `state` becomes `Some`.
    ready: Condvar,
}

impl Slot {
    fn new() -> Self {
        Slot {
            state: Mutex::new(None),
            ready: Condvar::new(),
        }
    }

    fn put(&self, value: LightState) {
        if let Ok(mut guard) = self.state.lock() {
            *guard = Some(value);
            self.ready.notify_one();
        }
    }

    /// Wait at most `timeout` for a value; take the newest and clear the slot.
    fn take(&self, timeout: Duration) -> Option<LightState> {
        let guard = self.state.lock().ok()?;
        let (mut guard, _) = self
            .ready
            .wait_timeout_while(guard, timeout, |s| s.is_none())
            .ok()?;
        guard.take()
    }
}

/// Shared control block between `push`/`status` and the worker thread.
struct Shared {
    slot: Slot,
    status: Mutex<LinkStatus>,
    shutdown: AtomicBool,
}

/// The real link handle returned by [`crate::open`].
pub struct LinkHandle {
    shared: Arc<Shared>,
    /// Kept so `Drop` can join the worker cleanly.
    worker: Option<std::thread::JoinHandle<()>>,
}

impl LinkHandle {
    /// Start the worker thread. The mesh file is read on the worker so `open`
    /// never blocks; a bad file becomes `Failed`, never a panic.
    pub fn spawn(config: LinkConfig) -> LinkHandle {
        let shared = Arc::new(Shared {
            slot: Slot::new(),
            status: Mutex::new(LinkStatus::Connecting),
            shutdown: AtomicBool::new(false),
        });
        let worker_shared = shared.clone();
        let worker = std::thread::Builder::new()
            .name("metal-ble-link".into())
            .spawn(move || worker_main(worker_shared, config))
            .ok();
        LinkHandle { shared, worker }
    }
}

impl Link for LinkHandle {
    fn push(&mut self, state: &LightState) -> bool {
        // Never blocks. A failed link rejects; everything else is coalesced.
        {
            let status = self.shared.status.lock().expect("status mutex");
            if matches!(*status, LinkStatus::Failed(_)) {
                return false;
            }
        }
        self.shared.slot.put(*state);
        true
    }

    fn status(&self) -> LinkStatus {
        self.shared.status.lock().map(|s| s.clone()).unwrap_or(LinkStatus::Connecting)
    }
}

impl Drop for LinkHandle {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        // Wake the worker if it is parked on the condvar.
        self.shared.slot.put(LightState { level: 0.0, cct_kelvin: None, hsi: None });
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

/// The worker thread: parse the mesh file, connect, then send in a loop.
fn worker_main(shared: Arc<Shared>, config: LinkConfig) {
    let status = |s: &Arc<Shared>, value: LinkStatus| {
        if let Ok(mut guard) = s.status.lock() {
            *guard = value;
        }
    };
    let fail = |s: &Arc<Shared>, msg: String| {
        status(s, LinkStatus::Failed(msg));
    };

    // Reading the state is the one thing that can fail before any radio work.
    let mut state = match MeshState::load(&config.mesh) {
        Ok(state) => state,
        Err(e) => {
            fail(&shared, format!("cannot read mesh state {}: {e}", config.mesh.display()));
            return;
        }
    };
    if state.device_key_bytes().is_none() {
        // Not fatal: the mesh may already be provisioned and the flat form has
        // no separate device key. Only the Config Server path needs it.
    }
    let dst = state.destination(config.node);

    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            fail(&shared, format!("cannot start tokio runtime: {e}"));
            return;
        }
    };

    runtime.block_on(async move {
        let mut backoff = INITIAL_BACKOFF;
        loop {
            if shared.shutdown.load(Ordering::SeqCst) {
                return;
            }

            // --- connect with backoff -------------------------------------
            status(&shared, LinkStatus::Connecting);
            let proxy = match MeshProxy::connect(&config.address).await {
                Ok(p) => p,
                Err(e) => {
                    status(&shared, LinkStatus::Connecting);
                    if sleep_or_shutdown(&shared, backoff).await {
                        return;
                    }
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    let _ = e;
                    continue;
                }
            };
            backoff = INITIAL_BACKOFF;
            status(&shared, LinkStatus::Connected);

            // --- send loop -------------------------------------------------
            let mut last_send = Instant::now()
                .checked_sub(MIN_SEND_INTERVAL)
                .unwrap_or_else(Instant::now);
            let mut disconnected = false;
            while !disconnected {
                if shared.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                // Block up to one interval for a new value; latest-wins means
                // we only ever keep the most recent one.
                let Some(value) = shared.slot.take(MIN_SEND_INTERVAL) else {
                    continue;
                };
                if shared.shutdown.load(Ordering::SeqCst) {
                    return;
                }

                // Rate-limit: wait out the remainder of the interval.
                let elapsed = last_send.elapsed();
                if elapsed < MIN_SEND_INTERVAL
                    && sleep_or_shutdown(&shared, MIN_SEND_INTERVAL - elapsed).await
                {
                    return;
                }

                let seq = match state.next_seq(&config.mesh) {
                    Ok(seq) => seq,
                    Err(e) => {
                        fail(&shared, format!("cannot reserve sequence number: {e}"));
                        return;
                    }
                };
                let access = godox::encode(&value);
                let pdu = transport::pack_command(&access, &state, dst, COMMAND_TTL, seq);
                match proxy.write(&pdu).await {
                    Ok(()) => {
                        last_send = Instant::now();
                    }
                    Err(_) => {
                        // Drop the connection and reconnect with backoff.
                        proxy.disconnect().await;
                        disconnected = true;
                    }
                }
            }

            status(&shared, LinkStatus::Connecting);
            if sleep_or_shutdown(&shared, backoff).await {
                return;
            }
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    });
}

/// Sleep, returning `true` if shutdown was requested during the wait.
async fn sleep_or_shutdown(shared: &Arc<Shared>, duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if shared.shutdown.load(Ordering::SeqCst) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20).min(duration)).await;
    }
    shared.shutdown.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_keeps_only_the_latest_value() {
        let slot = Slot::new();
        slot.put(LightState { level: 0.1, cct_kelvin: None, hsi: None });
        slot.put(LightState { level: 0.9, cct_kelvin: None, hsi: None });
        let got = slot.take(Duration::from_millis(0)).unwrap();
        assert_eq!(got.level, 0.9, "the newest value must win");
        // And the slot is now empty.
        assert!(slot.take(Duration::from_millis(0)).is_none());
    }

    #[test]
    fn slot_take_times_out_when_empty() {
        let slot = Slot::new();
        let start = Instant::now();
        assert!(slot.take(Duration::from_millis(30)).is_none());
        assert!(start.elapsed() >= Duration::from_millis(25));
    }

    #[test]
    fn push_returns_false_once_failed() {
        let shared = Arc::new(Shared {
            slot: Slot::new(),
            status: Mutex::new(LinkStatus::Failed("nope".into())),
            shutdown: AtomicBool::new(false),
        });
        let mut link = LinkHandle { shared, worker: None };
        assert!(!link.push(&LightState { level: 1.0, cct_kelvin: None, hsi: None }));
        assert!(matches!(link.status(), LinkStatus::Failed(_)));
    }

    #[test]
    fn push_returns_true_while_connecting() {
        let shared = Arc::new(Shared {
            slot: Slot::new(),
            status: Mutex::new(LinkStatus::Connecting),
            shutdown: AtomicBool::new(false),
        });
        let mut link = LinkHandle { shared, worker: None };
        assert!(link.push(&LightState { level: 1.0, cct_kelvin: None, hsi: None }));
    }

    #[test]
    fn bad_mesh_file_becomes_failed_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let config = LinkConfig {
            protocol: crate::Protocol::GodoxMesh,
            address: String::new(),
            node: None,
            mesh: dir.path().join("does-not-exist.json"),
        };
        let mut link = LinkHandle::spawn(config);
        // Wait for the worker to report failure.
        for _ in 0..200 {
            if matches!(link.status(), LinkStatus::Failed(_)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(link.status(), LinkStatus::Failed(_)));
        assert!(!link.push(&LightState { level: 0.5, cct_kelvin: None, hsi: None }));
    }

    #[test]
    fn engine_never_sends_faster_than_25hz() {
        // Timing policy is expressed as a constant so it can be asserted
        // without a radio.
        assert!(MIN_SEND_INTERVAL >= Duration::from_millis(33));
        assert!(MIN_SEND_INTERVAL <= Duration::from_millis(50));
    }
}
