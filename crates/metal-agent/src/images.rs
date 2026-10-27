//! Turn-scoped image persistence (H.3: "Una imagen adjunta persiste todo el
//! turno... Se sueltan al cerrar el turno.").
//!
//! An attached image — a camera capture, a grid test shot, whatever the loop
//! produces or receives mid-turn — needs to stay addressable for every
//! remaining tool call in that same turn, so a later step can compare
//! "before" and "after". It does not need a database: a turn is one HTTP
//! request/response cycle at most a few seconds long, so an in-memory map
//! that gets dropped when the turn closes is the whole requirement.
//!
//! This is deliberately not a cache with eviction policy — it is a scope.
//! `TurnImages::close` is the explicit, callable end of that scope (rather
//! than relying only on `Drop`) so the loop can log/account for the release
//! at a specific point instead of it happening implicitly whenever the value
//! goes out of scope.

use std::collections::HashMap;

/// One image attached during a turn: raw bytes plus enough metadata for a
/// tool call to reference it back (e.g. `capture_camera_frame`'s result
/// referenced by a later `test_mask_grid`).
#[derive(Clone, Debug)]
pub struct TurnImage {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// All images live for the current turn, keyed by an id the loop hands out
/// when an image is captured or attached (`"cam-1"`, `"grid-test-1"`, ...).
/// Default-empty; a fresh one is created per turn and dropped (or explicitly
/// [`close`](Self::close)d) when the turn ends.
#[derive(Default)]
pub struct TurnImages {
    images: HashMap<String, TurnImage>,
}

impl TurnImages {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores an image under `id`, overwriting whatever was there. Returns
    /// `id` back so call sites can chain it into a tool result.
    pub fn put(&mut self, id: impl Into<String>, mime: impl Into<String>, bytes: Vec<u8>) -> String {
        let id = id.into();
        self.images.insert(id.clone(), TurnImage { mime: mime.into(), bytes });
        id
    }

    pub fn get(&self, id: &str) -> Option<&TurnImage> {
        self.images.get(id)
    }

    pub fn len(&self) -> usize {
        self.images.len()
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// Ends the turn: every image is released. Explicit rather than relying
    /// solely on `Drop` so the loop has a clear point to call when a turn
    /// finishes, independent of when the `TurnImages` value itself happens
    /// to go out of scope.
    pub fn close(&mut self) {
        self.images.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_persist_across_the_turn_and_release_on_close() {
        let mut turn = TurnImages::new();

        turn.put("cam-1", "image/png", vec![1, 2, 3]);
        turn.put("grid-1", "image/png", vec![4, 5, 6]);
        assert_eq!(turn.len(), 2);

        // Still there after a later "call" in the same turn references it.
        assert_eq!(turn.get("cam-1").unwrap().bytes, vec![1, 2, 3]);
        assert_eq!(turn.get("grid-1").unwrap().mime, "image/png");

        turn.close();

        assert!(turn.is_empty());
        assert!(turn.get("cam-1").is_none());
        assert!(turn.get("grid-1").is_none());
    }

    #[test]
    fn putting_the_same_id_twice_overwrites_not_appends() {
        let mut turn = TurnImages::new();
        turn.put("cam-1", "image/png", vec![1]);
        turn.put("cam-1", "image/png", vec![2, 2]);
        assert_eq!(turn.len(), 1);
        assert_eq!(turn.get("cam-1").unwrap().bytes, vec![2, 2]);
    }
}
