//! Pre-rendered frame cache for sequence playback.
//!
//! While a sequence plays, each finished frame is stored together with the
//! overlay floats that produced it. If live rendering falls below the target
//! fps, the player can show the nearest cached pixels (by time, and by floats
//! when an exact match exists) instead of stalling. When fps is fine, overlay
//! edits keep driving real-time rendering as before.
//!
//! Entries are bounded by a byte budget and evicted least-recently-used. The
//! cache can be autosaved per sequence to one `<seq>.frames` file in a
//! directory; saves go through [`crate::atomic::write`] so a crash never leaves
//! a torn file behind.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::atomic;

/// Entries are keyed by time bucket (`t_ms / BUCKET_MS`) and floats hash, so
/// re-inserting the same frame replaces it instead of duplicating it.
const BUCKET_MS: u32 = 100;

/// File header: 8-byte magic, `u32` version, `u32` record count (little endian).
const MAGIC: &[u8; 8] = b"MVJFRAME";
const VERSION: u32 = 1;
const HEADER_LEN: usize = 16;

/// Per-record fixed fields: `t_ms` (u32), `floats_hash` (u64), `w` (u32),
/// `h` (u32), `rgba_len` (u64). The RGBA bytes follow.
const RECORD_LEN: usize = 4 + 8 + 4 + 4 + 8;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// One rendered frame: the pixels produced by the floats whose hash is
/// `floats_hash`, at sequence time `t_ms`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub t_ms: u32,
    pub floats_hash: u64,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Hash a float slice after quantising each value to multiples of `quant`.
///
/// Two slices that differ only by less than half a step hash the same, so
/// float jitter from the UI does not invalidate cached frames. The length is
/// part of the hash. NaN, infinities and out-of-range values each get their
/// own tagged key, so they never collide with finite values. A non-positive or
/// non-finite `quant` disables quantisation and hashes the exact bits.
pub fn hash_floats(floats: &[f32], quant: f32) -> u64 {
    let step = (quant.is_finite() && quant > 0.0).then_some(quant);
    let mut h = fnv1a(FNV_OFFSET, &(floats.len() as u64).to_le_bytes());
    for &f in floats {
        let (tag, key) = quant_key(f, step);
        h = fnv1a(h, &[tag]);
        h = fnv1a(h, &key.to_le_bytes());
    }
    h
}

/// The `(tag, key)` pair one float contributes to [`hash_floats`].
fn quant_key(f: f32, step: Option<f32>) -> (u8, i64) {
    if f.is_nan() {
        return (1, 0);
    }
    if !f.is_finite() {
        return (2, if f > 0.0 { 1 } else { -1 });
    }
    match step {
        Some(q) => {
            let scaled = (f as f64 / q as f64).round();
            if scaled.abs() < 9.0e18 {
                (0, scaled as i64)
            } else {
                (3, f.to_bits() as i64)
            }
        }
        None => (4, f.to_bits() as i64),
    }
}

fn fnv1a(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// Bytes of RGBA pixels for a `w` x `h` frame, or `None` on overflow.
fn frame_bytes(w: u32, h: u32) -> Option<usize> {
    (w as usize).checked_mul(h as usize)?.checked_mul(4)
}

#[derive(Clone, Debug)]
struct Slot {
    entry: Entry,
    /// Logical clock value of the last insert or lookup hit (LRU order).
    used: u64,
}

/// Byte-bounded LRU cache of rendered frames, grouped by sequence id.
#[derive(Debug)]
pub struct FrameCache {
    max_bytes: usize,
    used_bytes: usize,
    tick: u64,
    dirty: bool,
    seqs: HashMap<String, HashMap<(u32, u64), Slot>>,
}

impl FrameCache {
    /// An empty cache that holds at most `max_bytes` of RGBA pixel data.
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            used_bytes: 0,
            tick: 0,
            dirty: false,
            seqs: HashMap::new(),
        }
    }

    /// Store a frame for `seq_id`, replacing any entry with the same time
    /// bucket and floats hash, then evict least-recently-used entries until the
    /// byte budget holds again.
    ///
    /// Frames whose pixel buffer does not match `w * h * 4`, or that alone
    /// exceed the budget, are ignored.
    pub fn insert(&mut self, seq_id: &str, e: Entry) {
        let Some(len) = frame_bytes(e.w, e.h) else {
            return;
        };
        if e.rgba.len() != len || len > self.max_bytes {
            return;
        }
        self.tick += 1;
        let tick = self.tick;
        let key = (e.t_ms / BUCKET_MS, e.floats_hash);
        let map = self.seqs.entry(seq_id.to_string()).or_default();
        if let Some(old) = map.insert(key, Slot { entry: e, used: tick }) {
            self.used_bytes -= old.entry.rgba.len();
        }
        self.used_bytes += len;
        self.dirty = true;
        self.evict();
    }

    /// The frame for `seq_id` with exactly `floats_hash` whose time is nearest
    /// `t_ms`, within `tolerance_ms`. Marks the hit as recently used.
    pub fn lookup(
        &mut self,
        seq_id: &str,
        t_ms: u32,
        floats_hash: u64,
        tolerance_ms: u32,
    ) -> Option<&Entry> {
        let key = self.find_key(seq_id, t_ms, Some(floats_hash), tolerance_ms)?;
        self.touch(seq_id, key)
    }

    /// Like [`FrameCache::lookup`], but when no exact-hash frame is within
    /// tolerance, falls back to the nearest frame in time with any floats hash.
    pub fn lookup_any_floats(
        &mut self,
        seq_id: &str,
        t_ms: u32,
        floats_hash: u64,
        tolerance_ms: u32,
    ) -> Option<&Entry> {
        let key = self
            .find_key(seq_id, t_ms, Some(floats_hash), tolerance_ms)
            .or_else(|| self.find_key(seq_id, t_ms, None, tolerance_ms))?;
        self.touch(seq_id, key)
    }

    /// True when entries were inserted since the last successful [`FrameCache::save`]
    /// (or since [`FrameCache::load`]).
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// Total number of cached frames across all sequences.
    pub fn len(&self) -> usize {
        self.seqs.values().map(HashMap::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes of pixel data currently held.
    pub fn bytes(&self) -> usize {
        self.used_bytes
    }

    /// Write the frames of `seq_id` to `dir/<sanitised seq_id>.frames`,
    /// creating `dir` if needed, and clear the dirty flag.
    ///
    /// Frames are written oldest-first so a later [`FrameCache::load`] restores
    /// the same LRU order. The write is atomic (temp file plus rename).
    pub fn save(&mut self, dir: &Path, seq_id: &str) -> io::Result<()> {
        metal_vfs::create_dir_all(dir)?;
        let mut entries: Vec<(u64, &Entry)> = self
            .seqs
            .get(seq_id)
            .into_iter()
            .flat_map(|m| m.values().map(|s| (s.used, &s.entry)))
            .collect();
        entries.sort_by_key(|(used, _)| *used);

        let mut out = Vec::with_capacity(HEADER_LEN);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (_, e) in &entries {
            out.extend_from_slice(&e.t_ms.to_le_bytes());
            out.extend_from_slice(&e.floats_hash.to_le_bytes());
            out.extend_from_slice(&e.w.to_le_bytes());
            out.extend_from_slice(&e.h.to_le_bytes());
            out.extend_from_slice(&(e.rgba.len() as u64).to_le_bytes());
            out.extend_from_slice(&e.rgba);
        }
        atomic::write(file_path(dir, seq_id), out)?;
        self.dirty = false;
        Ok(())
    }

    /// Read the frames saved for `seq_id` under `dir` into a new cache with
    /// the given budget. A missing, unreadable or corrupt file yields an empty
    /// cache. A truncated file keeps the complete records before the damage.
    pub fn load(dir: &Path, seq_id: &str, max_bytes: usize) -> FrameCache {
        let mut cache = FrameCache::new(max_bytes);
        if let Ok(bytes) = metal_vfs::read(file_path(dir, seq_id)) {
            for e in decode(&bytes) {
                cache.insert(seq_id, e);
            }
        }
        cache.dirty = false;
        cache
    }

    /// Nearest key for `seq_id` within `tolerance_ms` of `t_ms`, optionally
    /// restricted to one floats hash. Ties go to the earlier time, then the
    /// smaller hash, so the result does not depend on map iteration order.
    fn find_key(
        &self,
        seq_id: &str,
        t_ms: u32,
        floats_hash: Option<u64>,
        tolerance_ms: u32,
    ) -> Option<(u32, u64)> {
        let map = self.seqs.get(seq_id)?;
        map.values()
            .filter(|s| floats_hash.is_none_or(|h| s.entry.floats_hash == h))
            .filter_map(|s| {
                let diff = (s.entry.t_ms as i64 - t_ms as i64).unsigned_abs();
                (diff <= tolerance_ms as u64).then_some((
                    diff,
                    s.entry.t_ms,
                    s.entry.floats_hash,
                ))
            })
            .min()
            .map(|(_, t, h)| (t / BUCKET_MS, h))
    }

    /// Mark the slot at `key` as most recently used and return its entry.
    fn touch(&mut self, seq_id: &str, key: (u32, u64)) -> Option<&Entry> {
        self.tick += 1;
        let tick = self.tick;
        let slot = self.seqs.get_mut(seq_id)?.get_mut(&key)?;
        slot.used = tick;
        Some(&slot.entry)
    }

    /// Drop least-recently-used entries until the budget holds.
    fn evict(&mut self) {
        while self.used_bytes > self.max_bytes {
            let victim = self
                .seqs
                .iter()
                .flat_map(|(seq, map)| map.iter().map(move |(key, slot)| (slot.used, seq, *key)))
                .min_by_key(|(used, _, _)| *used)
                .map(|(_, seq, key)| (seq.clone(), key));
            let Some((seq, key)) = victim else {
                break;
            };
            if let Some(map) = self.seqs.get_mut(&seq) {
                if let Some(slot) = map.remove(&key) {
                    self.used_bytes -= slot.entry.rgba.len();
                }
                if map.is_empty() {
                    self.seqs.remove(&seq);
                }
            }
        }
    }
}

/// Where the frames of `seq_id` live under `dir`.
pub fn file_path(dir: &Path, seq_id: &str) -> PathBuf {
    dir.join(format!("{}.frames", file_stem(seq_id)))
}

/// A filename-safe form of `seq_id`. Characters outside `[A-Za-z0-9_-]` become
/// `_`. When anything changed, the FNV hash of the original id is appended so
/// that different ids cannot map to the same file.
fn file_stem(seq_id: &str) -> String {
    let mut stem: String = seq_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if stem.is_empty() || stem != seq_id {
        let tag = fnv1a(FNV_OFFSET, seq_id.as_bytes());
        stem.push_str(&format!("-{tag:016x}"));
    }
    stem
}

fn le_u32(b: &[u8]) -> Option<u32> {
    <[u8; 4]>::try_from(b).ok().map(u32::from_le_bytes)
}

fn le_u64(b: &[u8]) -> Option<u64> {
    <[u8; 8]>::try_from(b).ok().map(u64::from_le_bytes)
}

/// Parse a frame file. Anything malformed stops the read at that point, so
/// the result holds the valid records seen before the damage.
fn decode(bytes: &[u8]) -> Vec<Entry> {
    let mut out = Vec::new();
    let Some(header) = bytes.get(..HEADER_LEN) else {
        return out;
    };
    if header[..MAGIC.len()] != MAGIC[..] || le_u32(&header[8..12]) != Some(VERSION) {
        return out;
    }
    let count = le_u32(&header[12..16]).unwrap_or(0) as usize;
    let mut at = HEADER_LEN;
    for _ in 0..count {
        let Some(rec) = bytes.get(at..at + RECORD_LEN) else {
            break;
        };
        let (Some(t_ms), Some(floats_hash), Some(w), Some(h), Some(len)) = (
            le_u32(&rec[0..4]),
            le_u64(&rec[4..12]),
            le_u32(&rec[12..16]),
            le_u32(&rec[16..20]),
            le_u64(&rec[20..28]),
        ) else {
            break;
        };
        let Some(end) = usize::try_from(len)
            .ok()
            .and_then(|n| (at + RECORD_LEN).checked_add(n))
        else {
            break;
        };
        let Some(rgba) = bytes.get(at + RECORD_LEN..end) else {
            break;
        };
        at = end;
        if frame_bytes(w, h) != Some(rgba.len()) {
            continue;
        }
        out.push(Entry {
            t_ms,
            floats_hash,
            w,
            h,
            rgba: rgba.to_vec(),
        });
    }
    out
}

/// Autosave policy: write when the cache is dirty and at least
/// `interval_secs` have passed since the last save.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoSave {
    pub interval_secs: f64,
}

impl AutoSave {
    /// Whether a save is due at `now_secs`, given the time of the last save.
    pub fn due(&self, now_secs: f64, last: f64, dirty: bool) -> bool {
        dirty && now_secs - last >= self.interval_secs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fw-frame-cache-{}-{}-{tag}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn frame(t_ms: u32, floats_hash: u64, w: u32, h: u32, fill: u8) -> Entry {
        Entry {
            t_ms,
            floats_hash,
            w,
            h,
            rgba: vec![fill; (w * h * 4) as usize],
        }
    }

    #[test]
    fn hash_floats_quantises_and_is_nan_safe() {
        let q = 0.01;
        assert_eq!(hash_floats(&[0.1001, 0.5], q), hash_floats(&[0.1, 0.5], q));
        assert_ne!(hash_floats(&[0.2, 0.5], q), hash_floats(&[0.1, 0.5], q));
        assert_eq!(hash_floats(&[-0.0], q), hash_floats(&[0.0], q));
        assert_ne!(hash_floats(&[0.0], q), hash_floats(&[0.0, 0.0], q));
        assert_eq!(hash_floats(&[f32::NAN], q), hash_floats(&[f32::NAN], q));
        assert_ne!(hash_floats(&[f32::NAN], q), hash_floats(&[0.0], q));
        assert_ne!(hash_floats(&[f32::NAN], q), hash_floats(&[f32::INFINITY], q));
        assert_ne!(
            hash_floats(&[f32::INFINITY], q),
            hash_floats(&[f32::NEG_INFINITY], q)
        );
        // An invalid step falls back to exact bits.
        assert_ne!(hash_floats(&[0.1001], 0.0), hash_floats(&[0.1], 0.0));
    }

    #[test]
    fn lookup_prefers_exact_hash_within_tolerance() {
        let mut c = FrameCache::new(1 << 20);
        c.insert("seq", frame(0, 1, 1, 1, 10));
        c.insert("seq", frame(200, 1, 1, 1, 20));
        c.insert("seq", frame(300, 2, 1, 1, 30));

        assert_eq!(c.lookup("seq", 210, 1, 50).map(|e| e.t_ms), Some(200));
        assert!(c.lookup("seq", 210, 1, 5).is_none());
        assert!(c.lookup("seq", 290, 2, 20).is_some());
        assert!(c.lookup("seq", 290, 2, 5).is_none());
        // Exact hash wins over a closer frame with a different hash.
        assert_eq!(c.lookup("seq", 290, 1, 200).map(|e| e.t_ms), Some(200));
        assert!(c.lookup("other", 210, 1, 50).is_none());
    }

    #[test]
    fn lookup_any_floats_falls_back_only_when_asked() {
        let mut c = FrameCache::new(1 << 20);
        c.insert("seq", frame(0, 1, 1, 1, 10));
        c.insert("seq", frame(200, 1, 1, 1, 20));
        c.insert("seq", frame(300, 2, 1, 1, 30));

        let hit = c.lookup_any_floats("seq", 290, 1, 20).map(|e| (e.t_ms, e.floats_hash));
        assert_eq!(hit, Some((300, 2)));
        let exact = c.lookup_any_floats("seq", 210, 1, 50).map(|e| (e.t_ms, e.floats_hash));
        assert_eq!(exact, Some((200, 1)));
        assert!(c.lookup("seq", 290, 1, 20).is_none());
    }

    #[test]
    fn lru_evicts_least_recently_used() {
        // Budget for exactly three 1x1 frames (4 bytes each).
        let mut c = FrameCache::new(12);
        c.insert("s", frame(0, 1, 1, 1, 1));
        c.insert("s", frame(100, 1, 1, 1, 2));
        c.insert("s", frame(200, 1, 1, 1, 3));
        assert_eq!(c.bytes(), 12);

        // Touch the oldest, so the middle one becomes the victim.
        assert!(c.lookup("s", 0, 1, 0).is_some());
        c.insert("s", frame(300, 1, 1, 1, 4));

        assert!(c.lookup("s", 100, 1, 0).is_none());
        assert!(c.lookup("s", 0, 1, 0).is_some());
        assert!(c.lookup("s", 200, 1, 0).is_some());
        assert!(c.lookup("s", 300, 1, 0).is_some());
        assert_eq!(c.len(), 3);
        assert_eq!(c.bytes(), 12);
    }

    #[test]
    fn insert_replaces_same_bucket_and_hash() {
        let mut c = FrameCache::new(1 << 20);
        c.insert("s", frame(10, 7, 2, 2, 1));
        c.insert("s", frame(90, 7, 2, 2, 2));
        assert_eq!(c.len(), 1);
        assert_eq!(c.bytes(), 16);
        assert_eq!(c.lookup("s", 90, 7, 0).map(|e| e.rgba[0]), Some(2));

        // A different bucket is a new entry.
        c.insert("s", frame(110, 7, 2, 2, 3));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn oversize_or_malformed_frames_are_rejected() {
        let mut c = FrameCache::new(12);
        c.insert("s", frame(0, 1, 2, 2, 0)); // 16 bytes > budget
        let mut bad = frame(0, 1, 1, 1, 0);
        bad.rgba.push(0); // buffer does not match 1x1x4
        c.insert("s", bad);
        assert!(c.is_empty());
        assert!(!c.dirty());
    }

    #[test]
    fn save_load_round_trip() {
        let dir = scratch_dir("roundtrip");
        let mut c = FrameCache::new(1 << 20);
        c.insert("show/A b", frame(0, 11, 2, 1, 7));
        c.insert("show/A b", frame(500, 12, 2, 1, 9));
        c.insert("other", frame(0, 1, 1, 1, 3));
        assert!(c.dirty());

        c.save(&dir, "show/A b").expect("save");
        assert!(!c.dirty());
        let path = file_path(&dir, "show/A b");
        assert!(path.is_file());
        assert_eq!(path.parent(), Some(dir.as_path()));
        let files = std::fs::read_dir(&dir).expect("read dir").count();
        assert_eq!(files, 1, "no scratch files left behind");

        let mut back = FrameCache::load(&dir, "show/A b", 1 << 20);
        assert_eq!(back.len(), 2);
        assert!(!back.dirty());
        assert_eq!(back.lookup("show/A b", 0, 11, 0).cloned(), Some(frame(0, 11, 2, 1, 7)));
        assert_eq!(back.lookup("show/A b", 500, 12, 0).cloned(), Some(frame(500, 12, 2, 1, 9)));
        assert!(back.lookup("other", 0, 1, 0).is_none());

        let missing = FrameCache::load(&dir, "never saved", 1 << 20);
        assert!(missing.is_empty());

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_tolerated() {
        let dir = scratch_dir("corrupt");
        let path = file_path(&dir, "s");

        // Garbage bytes: empty cache, no panic.
        metal_vfs::write(&path, b"definitely not a frame file").expect("write");
        assert!(FrameCache::load(&dir, "s", 1 << 20).is_empty());

        // Wrong version: empty cache.
        let mut c = FrameCache::new(1 << 20);
        c.insert("s", frame(0, 1, 1, 1, 1));
        c.save(&dir, "s").expect("save");
        let mut bytes = metal_vfs::read(&path).expect("read");
        bytes[8] = 99;
        metal_vfs::write(&path, &bytes).expect("write");
        assert!(FrameCache::load(&dir, "s", 1 << 20).is_empty());

        // Truncated: the complete first record survives, the cut one is dropped.
        c.insert("s", frame(100, 2, 1, 1, 2));
        c.save(&dir, "s").expect("save");
        let mut bytes = metal_vfs::read(&path).expect("read");
        bytes.truncate(bytes.len() - 2);
        metal_vfs::write(&path, &bytes).expect("write");
        assert_eq!(FrameCache::load(&dir, "s", 1 << 20).len(), 1);

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn file_stem_is_filename_safe_and_collision_free() {
        // Already-safe ids are used as they are.
        assert_eq!(file_stem("intro_1"), "intro_1");
        // Unsafe characters are replaced, and a hash keeps distinct ids apart.
        assert!(file_stem("show/A b").starts_with("show_A_b-"));
        assert_ne!(file_stem("a/b"), file_stem("a_b"));
        assert!(!file_stem("../x").contains('/'));
        assert!(!file_stem("").is_empty());
    }

    #[test]
    fn autosave_due_logic() {
        let a = AutoSave { interval_secs: 5.0 };
        assert!(a.due(10.0, 4.0, true));
        assert!(a.due(9.0, 4.0, true), "exactly the interval is due");
        assert!(!a.due(8.0, 4.0, true));
        assert!(!a.due(100.0, 99.0, false), "clean cache never saves");
    }
}
