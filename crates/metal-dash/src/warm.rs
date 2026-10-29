//! Background model warming (docs/PERF-2026-10-29.md §3). A model's first draw
//! builds its GPU pipelines and meshes on the calling thread, which freezes the
//! painter for seconds (monster 4.2 s, bulb 1.8 s). The routes that pick a
//! model ask for it here instead, and one low-priority worker warms the queue:
//! it takes the shared lock for one model at a time and releases it between
//! them, so the UI and the on-air frames are never held for more than one warm.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::app::Shared;
use crate::paint::Paint;

/// Models waiting to be warmed, in the order they were asked for.
static QUEUE: Mutex<VecDeque<usize>> = Mutex::new(VecDeque::new());

/// How long the worker sleeps when there is nothing to warm.
const IDLE: Duration = Duration::from_millis(100);
/// The gap it leaves between two warms, so the lock is free for a frame.
const BETWEEN: Duration = Duration::from_millis(10);

/// Asks for model `index` to be warmed. Repeated asks are kept once.
pub fn request(index: usize) {
    let mut queue = QUEUE.lock().unwrap_or_else(PoisonError::into_inner);
    if !queue.contains(&index) {
        queue.push_back(index);
    }
}

/// Starts the worker. It warms one queued model per lock, skipping any the
/// renderer already has warm.
pub fn spawn(shared: Arc<Mutex<Shared>>) {
    std::thread::Builder::new()
        .name("warm".into())
        .spawn(move || loop {
            let next = QUEUE.lock().unwrap_or_else(PoisonError::into_inner).pop_front();
            let Some(index) = next else {
                std::thread::sleep(IDLE);
                continue;
            };
            {
                let mut guard = shared.lock().unwrap_or_else(PoisonError::into_inner);
                if !guard.paint.is_warm(index) {
                    guard.paint.warm(index);
                }
            }
            std::thread::sleep(BETWEEN);
        })
        .expect("the warm worker thread");
}
