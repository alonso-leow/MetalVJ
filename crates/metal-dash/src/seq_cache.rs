//! Sequence playback fallback: when live renders fall behind the frame
//! target, show a cached picture of the same sequence time instead of stalling.
//!
//! The byte-bounded frame store is [`metal_perform::frame_cache::FrameCache`];
//! this module adds the shell's policy on top of it:
//!
//! - one `FrameCache` per sequence id, fed with downscaled copies (at most
//!   [`MAX_W`] x [`MAX_H`]) of live frames, at most one insertion per
//!   [`INSERT_GAP_MS`] of sequence time;
//! - [`Pacer`], a hysteresis on the live frame time: fallback starts after
//!   [`SLOW_RUN`] consecutive frames over `1.5 x target` and stops after
//!   [`GOOD_RUN`] consecutive frames within target;
//! - autosave of every dirty sequence to `<previews>/sequence/<seq>.frames`
//!   every [`AUTOSAVE_SECS`] seconds, so a restart keeps what was rendered.
//!
//! The floats are never cached as the answer: a caller always passes the
//! current overlay floats, and a fallback only returns a frame rendered from
//! the same floats (after quantising, see [`FLOAT_QUANT`]). A frame whose
//! live render keeps up is therefore always the live one.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use metal_perform::frame_cache::{hash_floats, AutoSave, Entry, FrameCache};

/// Largest picture kept per frame, in pixels. Larger frames are box-averaged
/// down to fit, keeping their aspect ratio.
pub const MAX_W: u32 = 160;
pub const MAX_H: u32 = 90;

/// Sequence time between two insertions for one sequence.
pub const INSERT_GAP_MS: u32 = 100;

/// A cached frame this close in sequence time answers a fallback request.
pub const TOLERANCE_MS: u32 = 50;

/// Pixel budget per sequence. One 160x90 frame is 57.6 KB, so this holds
/// roughly 580 frames, a little over a minute of sequence time at the
/// insertion rate.
pub const BUDGET_BYTES: usize = 32 << 20;

/// Float quantisation step for the cache key: overlay jitter below half of it
/// still matches a cached frame.
pub const FLOAT_QUANT: f32 = 0.001;

/// Live frames above `SLOW_RATIO * target` count as slow.
pub const SLOW_RATIO: f32 = 1.5;
/// Consecutive slow frames that start the fallback.
pub const SLOW_RUN: u32 = 3;
/// Consecutive frames within target that end it.
pub const GOOD_RUN: u32 = 10;

/// Seconds between autosaves of a dirty sequence.
pub const AUTOSAVE_SECS: f64 = 30.0;

/// Environment variable that replaces `~/.cache/metalvj/previews` as the root.
pub const DIR_ENV: &str = "METALVJ_PREVIEW_DIR";

/// Fallback state machine over live frame times.
///
/// A frame over `SLOW_RATIO * target` adds to a slow run; a frame at or under
/// `target` adds to a good run; anything in between resets both. Fallback is
/// on once the slow run reaches [`SLOW_RUN`] and off once the good run reaches
/// [`GOOD_RUN`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pacer {
    slow: u32,
    good: u32,
    falling_back: bool,
}

impl Pacer {
    /// Record one live frame time and return whether fallback is on.
    pub fn observe(&mut self, frame_ms: f32, target_ms: f32) -> bool {
        if frame_ms > target_ms * SLOW_RATIO {
            self.slow += 1;
            self.good = 0;
            if self.slow >= SLOW_RUN {
                self.falling_back = true;
            }
        } else if frame_ms <= target_ms {
            self.slow = 0;
            self.good += 1;
            if self.good >= GOOD_RUN {
                self.falling_back = false;
            }
        } else {
            self.slow = 0;
            self.good = 0;
        }
        self.falling_back
    }

    pub fn falling_back(&self) -> bool {
        self.falling_back
    }
}

/// One sequence's frames, plus its insertion throttle and autosave clock.
struct SeqState {
    cache: FrameCache,
    /// Sequence time of the last insertion, for the throttle.
    last_insert_ms: Option<u32>,
    /// Clock value of the last autosave attempt. `None` until the first
    /// autosave call, which starts the clock instead of saving at once.
    last_save_secs: Option<f64>,
}

/// The sequence fallback cache: frames by sequence id, the pacer, and where
/// the frames are saved.
pub struct SeqRenderCache {
    dir: Option<PathBuf>,
    seqs: HashMap<String, SeqState>,
    pacer: Pacer,
}

impl SeqRenderCache {
    /// A cache saving under [`default_dir`].
    pub fn new() -> Self {
        Self::with_dir(default_dir())
    }

    /// A cache saving under `dir`, or in memory only when `None`.
    pub fn with_dir(dir: Option<PathBuf>) -> Self {
        Self { dir, seqs: HashMap::new(), pacer: Pacer::default() }
    }

    /// Begin playing `seq_id`: load its saved frames if any. Idempotent, so a
    /// call per playback start is enough. `record` and `fallback` also call it,
    /// but a load there would land on a frame that is already late.
    pub fn start(&mut self, seq_id: &str) {
        if self.seqs.contains_key(seq_id) {
            return;
        }
        let cache = match &self.dir {
            Some(dir) => FrameCache::load(dir, seq_id, BUDGET_BYTES),
            None => FrameCache::new(BUDGET_BYTES),
        };
        self.seqs.insert(
            seq_id.to_string(),
            SeqState { cache, last_insert_ms: None, last_save_secs: None },
        );
    }

    /// Store a live frame rendered at sequence time `t_ms` from `floats`.
    ///
    /// `rgba` is `w * h * 4` bytes. The frame is box-averaged down to at most
    /// [`MAX_W`] x [`MAX_H`] first. Returns `false` when the frame is skipped:
    /// the sequence already has an insertion within [`INSERT_GAP_MS`] of
    /// `t_ms`, or the buffer does not match the size.
    pub fn record(
        &mut self,
        seq_id: &str,
        t_ms: u32,
        floats: &[f32],
        rgba: &[u8],
        w: u32,
        h: u32,
    ) -> bool {
        let Some(len) = frame_len(w, h) else {
            return false;
        };
        if rgba.len() != len {
            return false;
        }
        self.start(seq_id);
        let Some(state) = self.seqs.get_mut(seq_id) else {
            return false;
        };
        if let Some(last) = state.last_insert_ms {
            if t_ms.abs_diff(last) < INSERT_GAP_MS {
                return false;
            }
        }
        let (dw, dh) = fit_size(w, h);
        let pixels = if (dw, dh) == (w, h) {
            rgba.to_vec()
        } else {
            downscale(rgba, w, h, dw, dh)
        };
        state.cache.insert(
            seq_id,
            Entry {
                t_ms,
                floats_hash: hash_floats(floats, FLOAT_QUANT),
                w: dw,
                h: dh,
                rgba: pixels,
            },
        );
        state.last_insert_ms = Some(t_ms);
        true
    }

    /// The cached frame of `seq_id` nearest `t_ms` (within [`TOLERANCE_MS`])
    /// rendered from exactly these `floats`, or `None`. A hit counts as recently
    /// used.
    pub fn fallback(&mut self, seq_id: &str, t_ms: u32, floats: &[f32]) -> Option<&Entry> {
        self.start(seq_id);
        let key = hash_floats(floats, FLOAT_QUANT);
        self.seqs
            .get_mut(seq_id)?
            .cache
            .lookup(seq_id, t_ms, key, TOLERANCE_MS)
    }

    /// Feed one live frame time (milliseconds) against the target frame time
    /// and return whether the caller should draw cached frames now.
    pub fn should_fallback(&mut self, frame_ms: f32, target_ms: f32) -> bool {
        self.pacer.observe(frame_ms, target_ms)
    }

    /// Whether the pacer has fallback on, without feeding it a frame.
    pub fn falling_back(&self) -> bool {
        self.pacer.falling_back()
    }

    /// Save every sequence whose cache is dirty and whose autosave is due at
    /// `now_secs` (seconds on any clock that only moves forward). Returns how
    /// many were written. Every due sequence is attempted; the first error is
    /// returned after the rest, and a failed sequence waits a full interval
    /// before its next attempt. Does nothing without a directory.
    pub fn autosave(&mut self, now_secs: f64) -> io::Result<usize> {
        let Some(dir) = self.dir.clone() else {
            return Ok(0);
        };
        let policy = AutoSave { interval_secs: AUTOSAVE_SECS };
        let mut saved = 0;
        let mut first_err = None;
        for (seq_id, state) in self.seqs.iter_mut() {
            let last = *state.last_save_secs.get_or_insert(now_secs);
            if !policy.due(now_secs, last, state.cache.dirty()) {
                continue;
            }
            state.last_save_secs = Some(now_secs);
            match state.cache.save(&dir, seq_id) {
                Ok(()) => saved += 1,
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(saved),
        }
    }

    /// Frames held for `seq_id`.
    pub fn len(&self, seq_id: &str) -> usize {
        self.seqs.get(seq_id).map_or(0, |s| s.cache.len())
    }
}

impl Default for SeqRenderCache {
    fn default() -> Self {
        Self::new()
    }
}

/// `<previews>/sequence`, where `<previews>` is `$METALVJ_PREVIEW_DIR` or
/// `~/.cache/metalvj/previews`. `None` without a home directory.
pub fn default_dir() -> Option<PathBuf> {
    let home = metal_config::home_dir();
    dir_for(std::env::var_os(DIR_ENV).as_deref(), &home)
}

/// [`default_dir`] with the environment and home given explicitly.
pub fn dir_for(env: Option<&OsStr>, home: &Path) -> Option<PathBuf> {
    let root = match env {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            if home.as_os_str().is_empty() || home == Path::new(".") {
                return None;
            }
            home.join(".cache/metalvj/previews")
        }
    };
    Some(root.join("sequence"))
}

/// The size a `w` x `h` frame is kept at: unchanged when it already fits
/// [`MAX_W`] x [`MAX_H`], else scaled by the larger factor that fits and
/// rounded, never below one pixel.
pub fn fit_size(w: u32, h: u32) -> (u32, u32) {
    if w <= MAX_W && h <= MAX_H {
        return (w, h);
    }
    let scale = (f64::from(MAX_W) / f64::from(w)).min(f64::from(MAX_H) / f64::from(h));
    let dw = ((f64::from(w) * scale).round() as u32).clamp(1, MAX_W);
    let dh = ((f64::from(h) * scale).round() as u32).clamp(1, MAX_H);
    (dw, dh)
}

/// An RGBA `w` x `h` frame reduced the way the cache keeps it (see
/// [`fit_size`]): `(pixels, width, height)`. `None` when the buffer does not
/// match the size. Used for the library thumbnail of a sequence.
pub fn shrink(rgba: &[u8], w: u32, h: u32) -> Option<(Vec<u8>, u32, u32)> {
    if rgba.len() != frame_len(w, h)? {
        return None;
    }
    let (dw, dh) = fit_size(w, h);
    let pixels = if (dw, dh) == (w, h) {
        rgba.to_vec()
    } else {
        downscale(rgba, w, h, dw, dh)
    };
    Some((pixels, dw, dh))
}

/// Bytes of an RGBA `w` x `h` frame, or `None` on overflow or an empty size.
fn frame_len(w: u32, h: u32) -> Option<usize> {
    if w == 0 || h == 0 {
        return None;
    }
    (w as usize).checked_mul(h as usize)?.checked_mul(4)
}

/// Box-average an RGBA `w` x `h` frame down to `dw` x `dh`. Each output pixel
/// is the mean of the source block it covers; every source pixel is in exactly
/// one block when the target is no larger than the source.
fn downscale(rgba: &[u8], w: u32, h: u32, dw: u32, dh: u32) -> Vec<u8> {
    let (w, h, dw, dh) = (w as usize, h as usize, dw as usize, dh as usize);
    let mut out = vec![0u8; dw * dh * 4];
    for dy in 0..dh {
        let y0 = dy * h / dh;
        let y1 = ((dy + 1) * h / dh).max(y0 + 1);
        for dx in 0..dw {
            let x0 = dx * w / dw;
            let x1 = ((dx + 1) * w / dw).max(x0 + 1);
            let mut sum = [0u64; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = (sy * w + sx) * 4;
                    for c in 0..4 {
                        sum[c] += u64::from(rgba[i + c]);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            let o = (dy * dw + dx) * 4;
            for c in 0..4 {
                out[o + c] = ((sum[c] + n / 2) / n) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fw-seq-cache-{}-{}-{tag}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn solid(w: u32, h: u32, px: [u8; 4]) -> Vec<u8> {
        px.iter().copied().cycle().take((w * h * 4) as usize).collect()
    }

    #[test]
    fn fit_size_keeps_small_frames_and_caps_large_ones() {
        assert_eq!(fit_size(64, 36), (64, 36));
        assert_eq!(fit_size(160, 90), (160, 90));
        assert_eq!(fit_size(320, 180), (160, 90));
        assert_eq!(fit_size(1920, 1080), (160, 90));
        // Tall frames are capped by height and keep their aspect ratio.
        assert_eq!(fit_size(100, 1000), (9, 90));
        // A wide frame of nearly zero height never goes below one pixel.
        assert_eq!(fit_size(10_000, 1), (160, 1));
    }

    #[test]
    fn shrink_keeps_the_cache_size_and_rejects_bad_buffers() {
        let big = solid(640, 360, [30, 60, 90, 255]);
        let (px, w, h) = shrink(&big, 640, 360).expect("fits");
        assert_eq!((w, h), (160, 90));
        assert_eq!(px.len(), 160 * 90 * 4);
        let small = solid(8, 4, [1, 2, 3, 4]);
        assert_eq!(shrink(&small, 8, 4), Some((small.clone(), 8, 4)));
        assert_eq!(shrink(&small[..10], 8, 4), None);
        assert_eq!(shrink(&[], 0, 0), None);
    }

    #[test]
    fn downscale_averages_blocks() {
        // 2x2 checker of black and white, one output pixel: mean grey.
        let src = [
            0, 0, 0, 255, 255, 255, 255, 255, //
            0, 0, 0, 255, 255, 255, 255, 255,
        ];
        assert_eq!(downscale(&src, 2, 2, 1, 1), vec![128, 128, 128, 255]);
        // A uniform frame stays uniform at any size.
        let flat = solid(320, 180, [9, 99, 199, 255]);
        let out = downscale(&flat, 320, 180, 160, 90);
        assert_eq!(out.len(), 160 * 90 * 4);
        assert!(out.chunks_exact(4).all(|p| p == [9, 99, 199, 255]));
    }

    #[test]
    fn record_downscales_large_frames() {
        let mut c = SeqRenderCache::with_dir(None);
        let big = solid(640, 360, [200, 0, 0, 255]);
        assert!(c.record("s", 0, &[0.5], &big, 640, 360));
        let e = c.fallback("s", 0, &[0.5]).expect("hit");
        assert_eq!((e.w, e.h), (160, 90));
        assert_eq!(e.rgba.len(), 160 * 90 * 4);
    }

    #[test]
    fn record_rejects_bad_buffers_and_empty_sizes() {
        let mut c = SeqRenderCache::with_dir(None);
        assert!(!c.record("s", 0, &[], &[0; 3], 1, 1));
        assert!(!c.record("s", 0, &[], &[], 0, 0));
        assert!(!c.record("s", 0, &[], &[], 0, 5));
        assert_eq!(c.len("s"), 0);
    }

    #[test]
    fn record_throttles_to_one_per_gap() {
        let mut c = SeqRenderCache::with_dir(None);
        let px = solid(2, 2, [1, 2, 3, 4]);
        assert!(c.record("s", 0, &[], &px, 2, 2));
        assert!(!c.record("s", 50, &[], &px, 2, 2), "inside the gap");
        assert!(!c.record("s", 99, &[], &px, 2, 2), "still inside the gap");
        assert!(c.record("s", 100, &[], &px, 2, 2), "exactly the gap");
        // Looping back in time is measured the same way.
        assert!(!c.record("s", 40, &[], &px, 2, 2));
        assert!(c.record("s", 0, &[], &px, 2, 2));
        assert_eq!(c.len("s"), 2);
    }

    #[test]
    fn fallback_matches_time_within_tolerance_and_exact_floats() {
        let mut c = SeqRenderCache::with_dir(None);
        let px = solid(2, 2, [7, 7, 7, 255]);
        assert!(c.record("s", 1000, &[0.25, 0.75], &px, 2, 2));

        assert!(c.fallback("s", 1000, &[0.25, 0.75]).is_some());
        assert!(c.fallback("s", 1000 + TOLERANCE_MS, &[0.25, 0.75]).is_some());
        assert!(c.fallback("s", 1000 + TOLERANCE_MS + 1, &[0.25, 0.75]).is_none());
        // Floats jitter below half a quantum still hit; a real change misses.
        assert!(c.fallback("s", 1000, &[0.25 + FLOAT_QUANT * 0.2, 0.75]).is_some());
        assert!(c.fallback("s", 1000, &[0.26, 0.75]).is_none());
        assert!(c.fallback("s", 1000, &[0.25]).is_none(), "length is part of the key");
        // Another sequence never answers.
        assert!(c.fallback("other", 1000, &[0.25, 0.75]).is_none());
    }

    #[test]
    fn pacer_starts_after_three_slow_frames() {
        let mut p = Pacer::default();
        let target = 16.7;
        assert!(!p.observe(40.0, target));
        assert!(!p.observe(40.0, target));
        assert!(p.observe(40.0, target), "third slow frame");
        assert!(p.falling_back());
    }

    #[test]
    fn pacer_slow_run_must_be_consecutive() {
        let mut p = Pacer::default();
        let target = 16.7;
        p.observe(40.0, target);
        p.observe(40.0, target);
        // A frame between target and 1.5x target breaks the run.
        assert!(!p.observe(20.0, target));
        assert!(!p.observe(40.0, target));
        assert!(!p.observe(40.0, target));
        assert!(p.observe(40.0, target));
    }

    #[test]
    fn pacer_stays_on_until_ten_good_frames() {
        let mut p = Pacer::default();
        let target = 16.7;
        for _ in 0..SLOW_RUN {
            p.observe(40.0, target);
        }
        for _ in 0..GOOD_RUN - 1 {
            assert!(p.observe(10.0, target));
        }
        // A frame in the band does not count as good.
        assert!(p.observe(20.0, target));
        for _ in 0..GOOD_RUN - 1 {
            assert!(p.observe(10.0, target));
        }
        assert!(!p.observe(10.0, target), "the tenth consecutive good frame");
        assert!(!p.falling_back());
    }

    #[test]
    fn pacer_treats_nan_as_neither_slow_nor_good() {
        let mut p = Pacer::default();
        for _ in 0..SLOW_RUN {
            p.observe(40.0, 16.7);
        }
        assert!(p.observe(f32::NAN, 16.7), "NaN does not clear fallback");
        assert!(p.observe(f32::NAN, f32::NAN));
    }

    #[test]
    fn cache_should_fallback_drives_the_pacer() {
        let mut c = SeqRenderCache::with_dir(None);
        assert!(!c.should_fallback(10.0, 16.7));
        assert!(!c.should_fallback(50.0, 16.7));
        assert!(!c.should_fallback(50.0, 16.7));
        assert!(c.should_fallback(50.0, 16.7));
    }

    #[test]
    fn autosave_writes_dirty_sequences_once_per_interval() {
        let dir = scratch_dir("autosave");
        let mut c = SeqRenderCache::with_dir(Some(dir.clone()));
        let px = solid(2, 2, [5, 6, 7, 8]);
        c.start("show A");
        assert!(c.record("show A", 0, &[0.1], &px, 2, 2));

        // The first call starts the clock; nothing is due yet.
        assert_eq!(c.autosave(100.0).expect("ok"), 0);
        assert_eq!(c.autosave(100.0 + AUTOSAVE_SECS - 1.0).expect("ok"), 0);
        assert_eq!(c.autosave(100.0 + AUTOSAVE_SECS).expect("ok"), 1);
        assert!(frame_cache_file(&dir, "show A").is_file());
        // Clean now: no write until something changes.
        assert_eq!(c.autosave(100.0 + 3.0 * AUTOSAVE_SECS).expect("ok"), 0);

        assert!(c.record("show A", 500, &[0.1], &px, 2, 2));
        assert_eq!(c.autosave(100.0 + 4.0 * AUTOSAVE_SECS).expect("ok"), 1);

        // A new cache reads the saved frames back when the sequence starts.
        let mut back = SeqRenderCache::with_dir(Some(dir.clone()));
        back.start("show A");
        assert_eq!(back.len("show A"), 2);
        assert!(back.fallback("show A", 500, &[0.1]).is_some());

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn autosave_without_a_directory_keeps_frames_in_memory() {
        let mut c = SeqRenderCache::with_dir(None);
        let px = solid(2, 2, [1, 1, 1, 1]);
        assert!(c.record("s", 0, &[], &px, 2, 2));
        assert_eq!(c.autosave(0.0).expect("ok"), 0);
        assert_eq!(c.autosave(1000.0).expect("ok"), 0);
        assert_eq!(c.len("s"), 1);
    }

    #[test]
    fn dir_for_uses_the_env_root_then_home() {
        let home = Path::new("/home/u");
        assert_eq!(
            dir_for(Some(OsStr::new("/x/previews")), home),
            Some(PathBuf::from("/x/previews/sequence"))
        );
        assert_eq!(
            dir_for(None, home),
            Some(PathBuf::from("/home/u/.cache/metalvj/previews/sequence"))
        );
        assert_eq!(
            dir_for(Some(OsStr::new("")), home),
            Some(PathBuf::from("/home/u/.cache/metalvj/previews/sequence"))
        );
        assert_eq!(dir_for(None, Path::new(".")), None);
        assert_eq!(dir_for(None, Path::new("")), None);
    }

    fn frame_cache_file(dir: &Path, seq_id: &str) -> PathBuf {
        metal_perform::frame_cache::file_path(dir, seq_id)
    }
}
