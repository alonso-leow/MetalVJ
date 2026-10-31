//! The browser's preview painter in a Web Worker: what crosses `postMessage`,
//! and the one decision of whether to use a worker at all.
//!
//! Pure data and no wasm types, so it is tested natively; the worker, the
//! `OffscreenCanvas` and the proxy `Paint` that use it are in `metal-dash`'s
//! `web_worker.rs` (wasm32 only).
//!
//! # The wire
//!
//! - main -> worker: [`DrawRequest`], a JSON header ([`DrawRequest::header`])
//!   and the float row as a `Float32Array`.
//! - worker -> main: [`FrameReply`], one byte buffer (`id`, `w`, `h` as
//!   little-endian `u32`, then RGBA8). `w == 0` is "no frame": the worker's
//!   device cannot draw that model as a texture, and the main thread keeps the
//!   frame its CPU stand-in drew.
//!
//! # Frames are mailboxed, never awaited
//!
//! `Paint::solo` is synchronous. The proxy answers it at once with the newest
//! frame the worker has landed for the same `(model, size, alpha)` (one
//! request in flight per key), and the draw's own answer is the next call's.
//! That is the same "last good frame outlives a hiccup" rule the desktop's
//! preview worker has.

use std::collections::HashMap;

use metal_schema::EffectStep;
use serde::{Deserialize, Serialize};

/// One draw asked of the worker.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawRequest {
    pub id: u32,
    pub index: usize,
    pub time: f32,
    pub w: u32,
    pub h: u32,
    pub background_alpha: bool,
    pub chain: Vec<EffectStep>,
    pub floats: Vec<f32>,
}

#[derive(Serialize, Deserialize)]
struct Header {
    id: u32,
    index: usize,
    time: f32,
    w: u32,
    h: u32,
    bg: bool,
    chain: Vec<EffectStep>,
}

/// What a frame is filed under in the mailbox.
pub type Key = (usize, u32, u32, bool);

impl DrawRequest {
    pub fn key(&self) -> Key {
        (self.index, self.w, self.h, self.background_alpha)
    }

    /// The JSON header; the floats travel beside it as a typed array.
    pub fn header(&self) -> String {
        serde_json::to_string(&Header {
            id: self.id,
            index: self.index,
            time: self.time,
            w: self.w,
            h: self.h,
            bg: self.background_alpha,
            chain: self.chain.clone(),
        })
        .expect("a header is plain data")
    }

    pub fn parse(header: &str, floats: Vec<f32>) -> Result<DrawRequest, String> {
        let h: Header = serde_json::from_str(header).map_err(|e| e.to_string())?;
        Ok(DrawRequest {
            id: h.id,
            index: h.index,
            time: h.time,
            w: h.w,
            h: h.h,
            background_alpha: h.bg,
            chain: h.chain,
            floats,
        })
    }
}

/// A frame (or its absence) coming back.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameReply {
    pub id: u32,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl FrameReply {
    /// "The worker has no texture for this draw."
    pub fn none(id: u32) -> FrameReply {
        FrameReply { id, w: 0, h: 0, rgba: Vec::new() }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + self.rgba.len());
        for v in [self.id, self.w, self.h] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.rgba);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<FrameReply, String> {
        if bytes.len() < 12 {
            return Err(format!("a frame reply is at least 12 bytes, got {}", bytes.len()));
        }
        let word = |i: usize| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().expect("4 bytes"));
        let (id, w, h) = (word(0), word(1), word(2));
        let rgba = bytes[12..].to_vec();
        if rgba.len() != (w as usize) * (h as usize) * 4 {
            return Err(format!("{w}x{h} needs {} bytes, got {}", (w as usize) * (h as usize) * 4, rgba.len()));
        }
        Ok(FrameReply { id, w, h, rgba })
    }
}

/// The newest landed frame per key, and which keys have a request out.
#[derive(Default)]
pub struct Mailbox {
    frames: HashMap<Key, (u32, u32, Vec<u8>)>,
    out: HashMap<u32, Key>,
    next: u32,
}

impl Mailbox {
    /// A request for `key` when none is in flight, else `None` (the caller
    /// keeps what it has). `make` builds the request around the fresh id.
    pub fn begin(&mut self, key: Key, make: impl FnOnce(u32) -> DrawRequest) -> Option<DrawRequest> {
        if self.out.values().any(|k| *k == key) {
            return None;
        }
        self.next = self.next.wrapping_add(1);
        let req = make(self.next);
        self.out.insert(req.id, key);
        Some(req)
    }

    /// A reply lands: kept when it carries pixels, and its request is no
    /// longer in flight either way.
    pub fn land(&mut self, reply: FrameReply) {
        if let Some(key) = self.out.remove(&reply.id) {
            if reply.w > 0 {
                self.frames.insert(key, (reply.w, reply.h, reply.rgba));
            }
        }
    }

    pub fn frame(&self, key: &Key) -> Option<&(u32, u32, Vec<u8>)> {
        self.frames.get(key)
    }

    pub fn in_flight(&self) -> usize {
        self.out.len()
    }
}

/// Which painter the page gets, decided once at startup (the backend rule).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The preview painter runs in a Web Worker on an `OffscreenCanvas`.
    Worker,
    /// Today's path: the painter on the page's own thread.
    Inline,
}

/// `has_worker`/`has_offscreen`: the globals exist. `worker_ready`: what the
/// booted worker said about its device (`None` when it never answered), so a
/// browser with `Worker` but no WebGPU/GL in workers still lands on `Inline`.
pub fn choose_mode(has_worker: bool, has_offscreen: bool, worker_ready: Option<bool>) -> Mode {
    if has_worker && has_offscreen && worker_ready == Some(true) {
        Mode::Worker
    } else {
        Mode::Inline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: u32) -> DrawRequest {
        DrawRequest {
            id,
            index: 3,
            time: 1.5,
            w: 64,
            h: 32,
            background_alpha: true,
            chain: Vec::new(),
            floats: vec![0.25, 0.5, 1.0],
        }
    }

    #[test]
    fn a_draw_request_round_trips_through_its_header_and_floats() {
        let r = req(7);
        let back = DrawRequest::parse(&r.header(), r.floats.clone()).unwrap();
        assert_eq!(back, r);
        assert!(DrawRequest::parse("not json", vec![]).is_err());
    }

    #[test]
    fn a_frame_reply_round_trips_and_a_short_or_mis_sized_one_is_refused() {
        let f = FrameReply { id: 9, w: 2, h: 1, rgba: vec![1, 2, 3, 4, 5, 6, 7, 8] };
        assert_eq!(FrameReply::decode(&f.encode()).unwrap(), f);
        assert!(FrameReply::decode(&[0; 5]).is_err());
        let mut bad = f.encode();
        bad.pop();
        assert!(FrameReply::decode(&bad).is_err(), "the pixel count must match w x h");
        assert_eq!(FrameReply::decode(&FrameReply::none(4).encode()).unwrap().w, 0);
    }

    #[test]
    fn the_mailbox_keeps_one_request_per_key_and_the_newest_frame() {
        let mut m = Mailbox::default();
        let a = m.begin(req(0).key(), |id| req(id)).expect("the first ask goes out");
        assert!(m.begin(req(0).key(), |id| req(id)).is_none(), "one in flight per key");
        let other = DrawRequest { index: 4, ..req(0) };
        assert!(m.begin(other.key(), |id| DrawRequest { id, ..other.clone() }).is_some(), "another key is independent");
        assert!(m.frame(&a.key()).is_none());
        m.land(FrameReply { id: a.id, w: 1, h: 1, rgba: vec![9, 9, 9, 9] });
        assert_eq!(m.frame(&a.key()).unwrap().2, vec![9, 9, 9, 9]);
        assert!(m.begin(a.key(), |id| req(id)).is_some(), "the key is free again");
    }

    #[test]
    fn a_none_reply_frees_the_key_and_keeps_the_last_frame() {
        let mut m = Mailbox::default();
        let a = m.begin(req(0).key(), |id| req(id)).unwrap();
        m.land(FrameReply { id: a.id, w: 1, h: 1, rgba: vec![1, 1, 1, 1] });
        let b = m.begin(a.key(), |id| req(id)).unwrap();
        m.land(FrameReply::none(b.id));
        assert_eq!(m.in_flight(), 0);
        assert_eq!(m.frame(&a.key()).unwrap().2, vec![1, 1, 1, 1], "the last good frame outlives a miss");
    }

    #[test]
    fn the_worker_is_used_only_when_every_piece_is_there() {
        assert_eq!(choose_mode(true, true, Some(true)), Mode::Worker);
        assert_eq!(choose_mode(false, true, Some(true)), Mode::Inline, "no Worker");
        assert_eq!(choose_mode(true, false, Some(true)), Mode::Inline, "no OffscreenCanvas");
        assert_eq!(choose_mode(true, true, Some(false)), Mode::Inline, "no device in the worker");
        assert_eq!(choose_mode(true, true, None), Mode::Inline, "the worker never answered");
    }
}
