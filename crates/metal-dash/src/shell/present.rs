//! G3 - the window presents the compose texture itself.
//!
//! When the native window and the painter share one wgpu device
//! (`window::open_wgpu`), a device preview does not travel as a JPEG: the
//! worker renders the frame ([`Call::texture`](crate::call::Call::texture)) and
//! lands the `GpuFrame` here, and the window registers that very texture with
//! egui (`register_native_texture`) before it paints. No encode, no readback,
//! no upload. [`Presenter`] is the mailbox between the two threads.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use web_time::{Duration, Instant};

use metal_perform::gpu_compose::{region_async, AsyncReadback, GpuCx, PendingRegion};
use metal_perform::gpu_frame::GpuFrame;

use crate::seq_cache::INSERT_GAP_MS;

/// The least time between two recorded sequence frames, wall clock: the
/// sequence cache's own insertion gap, on the device side too.
const RECORD_GAP: Duration = Duration::from_millis(INSERT_GAP_MS as u64);

/// A recording readback still unfinished after this long is abandoned with its
/// whole [`AsyncReadback`], so one stuck copy cannot hold the recorder shut.
const FLIGHT_TIMEOUT: Duration = Duration::from_secs(1);

/// Finished records kept for the shell to drain; the oldest go first.
const MAX_READY: usize = 8;

/// What a recorded frame is filed under in the sequence cache: the sequence
/// it belongs to, its sequence time, and the overlay floats it was drawn from.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordTag {
    pub seq: String,
    pub t_ms: u32,
    pub floats: Vec<f32>,
}

/// One recorded frame: its tag and its pixels, RGBA8 at `w` x `h`.
pub struct Recorded {
    pub tag: RecordTag,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// A route's landing count and the frame time its latest landing measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Landing {
    /// How many frames have landed for the key, ever.
    pub count: u64,
    /// Milliseconds between the last two landings; `None` before the second.
    pub gap_ms: Option<f32>,
}

/// The sequence taps (`shell/panels/lanes.rs` drives them each frame).
#[derive(Default)]
struct Taps {
    /// The key whose landed frames are recorded now, and their tag. `None`
    /// records nothing: the pacer says the live render is behind.
    record: Option<(String, RecordTag)>,
    /// When the last record was taken, from either route (throttle).
    last_record: Option<Instant>,
    /// The copy of one landed frame in flight, and when it was queued.
    flight: Option<(RecordTag, Instant)>,
    rb: Option<AsyncReadback>,
    ready: Vec<Recorded>,
    /// Pictures the shell shows in place of the late live frame, by key.
    fallback: HashMap<String, egui::TextureHandle>,
}

/// What a [`Presenter`] knows about one picture.
pub enum Shown {
    /// A texture egui can draw now.
    Ready { id: egui::TextureId, size: [u32; 2], landed: u64 },
    /// Asked for, not landed yet.
    Pending,
    /// The door has no texture form for it (a CPU painter, another route):
    /// use the JPEG path.
    Unsupported,
}

/// How long after the last GPU picture request the window still counts a
/// live preview as on screen (its own pacing requests keep it fresh).
pub const LIVE_WINDOW: Duration = Duration::from_millis(500);

/// 256 bins each of red, green, blue and luma (`metal_perform::gpu_hist`).
pub type Histogram = [[u32; 256]; 4];

/// Every how many pixels a grade's histogram counts, as the CPU reduction does.
const HIST_STRIDE: u32 = 4;

#[derive(Default)]
struct Slot {
    /// Landed, not yet registered with egui.
    pending: Option<GpuFrame>,
    /// Registered and on screen; kept so the pool does not recycle it.
    shown: Option<GpuFrame>,
    id: Option<egui::TextureId>,
    size: [u32; 2],
    landed: u64,
    /// When the last frame landed, and the gap to the one before it.
    last_landing: Option<Instant>,
    gap_ms: Option<f32>,
    in_flight: bool,
    unsupported: bool,
    /// G5: the caller wants this picture's histogram (a grade page is open).
    want_hist: bool,
    /// The `landed` count the last histogram was asked of.
    hist_asked: u64,
    /// The latest histogram of this picture, once its readback finished.
    hist: Option<Box<Histogram>>,
}

/// The eyedropper's one pick: the click asked for, and the copy in flight.
#[derive(Default)]
struct Pick {
    /// `(key, u, v)`: the clicked picture and the point in it, each `0..1`.
    asked: Option<(String, f32, f32)>,
    in_flight: Option<PendingRegion>,
}

#[derive(Default)]
struct Inner {
    /// When a GPU picture was last asked for (`Frame::tick_texture`): the
    /// window keeps its 60 Hz pace while one is on screen, even when egui
    /// itself has nothing to animate.
    live: Mutex<Option<Instant>>,
    on: AtomicBool,
    /// G5: the window's device can count histograms on the GPU.
    hist_on: AtomicBool,
    ctx: Mutex<Option<egui::Context>>,
    slots: Mutex<HashMap<String, Slot>>,
    pick: Mutex<Pick>,
    taps: Mutex<Taps>,
}

/// The gap between two landings of one key, in milliseconds (`None` for the
/// first). A landing time is the frame time the sequence pacer reads.
pub fn frame_gap_ms(prev: Option<Instant>, now: Instant) -> Option<f32> {
    prev.map(|p| now.saturating_duration_since(p).as_secs_f32() * 1000.0)
}

/// An RGB frame as RGBA with opaque alpha, the form the sequence cache keeps.
pub fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for px in rgb.chunks_exact(3) {
        out.extend_from_slice(px);
        out.push(255);
    }
    out
}

/// Counts one landing of a slot and measures its gap to the one before.
fn note(slot: &mut Slot, now: Instant) {
    slot.landed += 1;
    slot.gap_ms = frame_gap_ms(slot.last_landing, now);
    slot.last_landing = Some(now);
}

/// Whether a record may be taken now: none was taken within [`RECORD_GAP`].
fn record_due(last: Option<Instant>) -> bool {
    last.is_none_or(|at| at.elapsed() >= RECORD_GAP)
}

/// Files a finished record, dropping the oldest once [`MAX_READY`] wait.
fn push_ready(t: &mut Taps, r: Recorded) {
    if t.ready.len() >= MAX_READY {
        t.ready.remove(0);
    }
    t.ready.push(r);
}

/// The shared mailbox; clones are handles to the same one.
#[derive(Clone, Default)]
pub struct Presenter {
    inner: Arc<Inner>,
}

impl Presenter {
    /// The window shares the painter's device: textures may be presented.
    pub fn enable(&self, ctx: egui::Context) {
        if let Ok(mut c) = crate::perf::locked(&self.inner.ctx, "present.ctx") {
            *c = Some(ctx);
        }
        self.inner.on.store(true, Ordering::Release);
    }

    pub fn enabled(&self) -> bool {
        self.inner.on.load(Ordering::Acquire)
    }

    /// A GPU picture was asked for just now: a live preview is on screen.
    pub fn mark_live(&self) {
        if let Ok(mut live) = crate::perf::locked(&self.inner.live, "present.live") {
            *live = Some(Instant::now());
        }
    }

    /// Whether a GPU picture was asked for within [`LIVE_WINDOW`].
    pub fn live(&self) -> bool {
        self.inner
            .live
            .lock()
            .ok()
            .and_then(|l| *l)
            .is_some_and(|at| at.elapsed() < LIVE_WINDOW)
    }

    /// G5: the window's device can run the histogram pass (`set` once, at
    /// the window's start).
    pub fn enable_histograms(&self) {
        self.inner.hist_on.store(true, Ordering::Release);
    }

    /// Whether histograms of presented pictures are counted on the GPU.
    pub fn histograms(&self) -> bool {
        self.inner.hist_on.load(Ordering::Acquire)
    }

    /// G5: asks (or stops asking) for `key`'s histogram on each frame it lands.
    pub fn want_histogram(&self, key: &str, on: bool) {
        let Ok(mut slots) = crate::perf::locked(&self.inner.slots, "present.slots") else { return };
        let slot = slots.entry(key.to_string()).or_default();
        slot.want_hist = on;
        if !on {
            slot.hist = None;
        }
    }

    /// G5: the latest histogram of `key`'s picture, once one has been counted.
    pub fn histogram(&self, key: &str) -> Option<Box<Histogram>> {
        let slots = crate::perf::locked(&self.inner.slots, "present.slots").ok()?;
        slots.get(key)?.hist.clone()
    }

    /// Window thread, before the UI: hands finished histograms to their keys,
    /// and queues a count of each newly landed frame whose key asked for one.
    /// Never waits on the GPU; a frame the pass cannot take yet is asked again
    /// on a later call.
    pub fn histograms_pending(&self, hist: &mut metal_perform::gpu_hist::Histogrammer<String>) {
        let Ok(mut slots) = crate::perf::locked(&self.inner.slots, "present.slots") else { return };
        while let Some((key, bins)) = hist.poll() {
            if let Some(slot) = slots.get_mut(&key) {
                slot.hist = Some(Box::new(bins));
            }
        }
        for (key, slot) in slots.iter_mut() {
            if !slot.want_hist || slot.landed == slot.hist_asked {
                continue;
            }
            let Some(frame) = slot.pending.as_ref().or(slot.shown.as_ref()) else { continue };
            if hist.submit(frame, HIST_STRIDE, key.clone()) {
                slot.hist_asked = slot.landed;
            }
        }
    }

    /// Asks for another window frame (a copy in flight is polled on it).
    fn repaint(&self) {
        if let Some(ctx) = self.inner.ctx.lock().ok().and_then(|c| c.clone()) {
            ctx.request_repaint();
        }
    }

    /// Marks `key` as being fetched; `false` when a fetch is already out.
    pub fn begin(&self, key: &str) -> bool {
        let Ok(mut slots) = crate::perf::locked(&self.inner.slots, "present.slots") else { return false };
        let slot = slots.entry(key.to_string()).or_default();
        if slot.in_flight {
            return false;
        }
        slot.in_flight = true;
        true
    }

    /// Counts one landed frame of `key` for the pacer's timing, without a
    /// picture: the CPU (JPEG) route lands through here.
    pub fn note_landed(&self, key: &str) {
        if let Ok(mut slots) = crate::perf::locked(&self.inner.slots, "present.slots") {
            note(slots.entry(key.to_string()).or_default(), Instant::now());
        }
    }

    /// The landing count of `key` and its latest frame gap (see [`Landing`]);
    /// `None` for a key that never landed.
    pub fn landing(&self, key: &str) -> Option<Landing> {
        let slots = crate::perf::locked(&self.inner.slots, "present.slots").ok()?;
        let s = slots.get(key)?;
        (s.landed > 0).then_some(Landing { count: s.landed, gap_ms: s.gap_ms })
    }

    /// The worker's answer: a frame, or `None` when the door has no texture.
    pub fn land(&self, key: &str, frame: Option<GpuFrame>) {
        if let Ok(mut slots) = crate::perf::locked(&self.inner.slots, "present.slots") {
            let slot = slots.entry(key.to_string()).or_default();
            slot.in_flight = false;
            match frame {
                Some(f) => {
                    slot.size = [f.width(), f.height()];
                    note(slot, Instant::now());
                    slot.pending = Some(f);
                    slot.unsupported = false;
                }
                None => slot.unsupported = true,
            }
        }
        if let Some(ctx) = crate::perf::locked(&self.inner.ctx, "present.ctx").ok().and_then(|c| c.clone()) {
            ctx.request_repaint();
        }
    }

    /// The eyedropper's click on the picture `key` shows, at `(u, v)` of it
    /// (each `0..1`). The pixel is read on the next window frame.
    pub fn request_pick(&self, key: &str, u: f32, v: f32) {
        if let Ok(mut p) = self.inner.pick.lock() {
            p.asked = Some((key.to_string(), u, v));
        }
    }

    /// Sequence taps: which key's landed frames are recorded now, and under
    /// what tag. `None` records nothing (the pacer says the render is behind).
    pub fn set_record(&self, want: Option<(String, RecordTag)>) {
        if let Ok(mut t) = crate::perf::locked(&self.inner.taps, "present.taps") {
            t.record = want;
        }
    }

    /// Sequence taps: the records finished since the last call, oldest first.
    pub fn take_records(&self) -> Vec<Recorded> {
        match crate::perf::locked(&self.inner.taps, "present.taps") {
            Ok(mut t) => std::mem::take(&mut t.ready),
            Err(_) => Vec::new(),
        }
    }

    /// Sequence taps: the picture `key` shows in place of its live frame, or
    /// `None` to show the live frame again.
    pub fn set_fallback(&self, key: &str, pic: Option<egui::TextureHandle>) {
        if let Ok(mut t) = crate::perf::locked(&self.inner.taps, "present.taps") {
            match pic {
                Some(h) => t.fallback.insert(key.to_string(), h),
                None => t.fallback.remove(key),
            };
        }
    }

    /// Sequence taps: the fallback picture of `key`, if one is set.
    pub fn fallback(&self, key: &str) -> Option<egui::TextureHandle> {
        crate::perf::locked(&self.inner.taps, "present.taps").ok()?.fallback.get(key).cloned()
    }

    /// Sequence taps: nothing recorded, no fallback picture, no records
    /// waiting. Called when no sequence is playing.
    pub fn clear_sequence(&self) {
        if let Ok(mut t) = crate::perf::locked(&self.inner.taps, "present.taps") {
            t.record = None;
            t.fallback.clear();
            t.ready.clear();
        }
    }

    /// The CPU route's landed frame (`Frame::tick`): recorded when `key` is
    /// the one being recorded and the record gap has passed. Copies only then.
    pub fn offer_frame(&self, key: &str, w: u32, h: u32, rgb: &[u8]) {
        let Ok(mut t) = crate::perf::locked(&self.inner.taps, "present.taps") else { return };
        let Some((want, tag)) = t.record.clone() else { return };
        let len = (w as usize) * (h as usize) * 3;
        if want != key || rgb.len() != len || !record_due(t.last_record) {
            return;
        }
        t.last_record = Some(Instant::now());
        push_ready(&mut t, Recorded { tag, w, h, rgba: rgb_to_rgba(rgb) });
    }

    /// Window thread: finishes the recording readback in flight, and starts
    /// one for the frame `record` names when the gap has passed. The copy is
    /// queued now and read on a later call; nothing waits on the GPU. Skipped
    /// while a copy is out, so the recorder never holds two.
    fn records_pending(&self, cx: &GpuCx) {
        let Ok(mut guard) = crate::perf::locked(&self.inner.taps, "present.taps") else { return };
        let t = &mut *guard;
        if let Some((tag, at)) = t.flight.clone() {
            let polled = t.rb.as_mut().and_then(|rb| rb.poll(cx));
            match polled {
                Some((rgba, w, h)) => {
                    t.flight = None;
                    push_ready(t, Recorded { tag, w, h, rgba });
                }
                None if at.elapsed() > FLIGHT_TIMEOUT => {
                    t.flight = None;
                    t.rb = None;
                }
                None => {}
            }
        }
        let Some((key, tag)) = t.record.clone() else { return };
        if t.flight.is_some() || !record_due(t.last_record) {
            return;
        }
        let Ok(slots) = crate::perf::locked(&self.inner.slots, "present.slots") else { return };
        let Some(frame) = slots.get(&key).and_then(|s| s.shown.as_ref().or(s.pending.as_ref())) else { return };
        let rb = t.rb.get_or_insert_with(|| AsyncReadback::new(cx));
        if rb.submit(cx, frame) {
            let now = Instant::now();
            t.flight = Some((tag, now));
            t.last_record = Some(now);
        }
    }

    /// Window thread, before the UI: starts the copy of a pixel asked for
    /// since the last call, and returns the colour of one that has finished.
    /// Never waits on the GPU; an unfinished copy asks for another frame.
    pub fn picks_pending(&self, cx: &GpuCx) -> Option<[u8; 3]> {
        self.records_pending(cx);
        let mut pick = self.inner.pick.lock().ok()?;
        if let Some(polled) = pick.in_flight.as_ref().map(|r| r.poll(cx)) {
            match polled {
                Some(px) => {
                    pick.in_flight = None;
                    return px.get(..3).map(|p| [p[0], p[1], p[2]]);
                }
                None => {
                    self.repaint();
                    return None;
                }
            }
        }
        let (key, u, v) = pick.asked.take()?;
        let slots = self.inner.slots.lock().ok()?;
        let frame = slots.get(&key).and_then(|s| s.shown.as_ref().or(s.pending.as_ref()))?;
        let x = ((u.clamp(0.0, 1.0) * frame.width() as f32) as u32).min(frame.width().saturating_sub(1));
        let y = ((v.clamp(0.0, 1.0) * frame.height() as f32) as u32).min(frame.height().saturating_sub(1));
        pick.in_flight = Some(region_async(cx, frame, x, y, 1, 1));
        self.repaint();
        None
    }

    pub fn shown(&self, key: &str) -> Shown {
        let Ok(slots) = crate::perf::locked(&self.inner.slots, "present.slots") else { return Shown::Unsupported };
        match slots.get(key) {
            Some(s) if s.id.is_some() => Shown::Ready { id: s.id.expect("checked"), size: s.size, landed: s.landed },
            Some(s) if s.unsupported => Shown::Unsupported,
            _ => Shown::Pending,
        }
    }

    /// Window thread, before the UI runs: hands every frame that landed since
    /// the last paint to `register` (with the egui id it already has, if any),
    /// which registers its texture with egui on the window's device and answers
    /// the id. Only `window.rs` names the backend types that takes.
    pub fn register_pending(&self, mut register: impl FnMut(&GpuFrame, Option<egui::TextureId>) -> egui::TextureId) {
        let Ok(mut slots) = crate::perf::locked(&self.inner.slots, "present.slots") else { return };
        for slot in slots.values_mut() {
            let Some(frame) = slot.pending.take() else { continue };
            slot.id = Some(register(&frame, slot.id));
            slot.shown = Some(frame);
        }
    }
}

#[cfg(all(test, feature = "gpu"))]
mod tests {
    use super::*;
    use metal_perform::gpu_compose::GpuCx;
    use metal_perform::gpu_hist::{histogram_cpu, Histogrammer};

    /// G5: a landed picture whose key asks for a histogram is counted on the
    /// window's device, and the counts match the CPU reduction of its pixels.
    #[test]
    fn a_grade_key_gets_the_gpu_histogram_of_its_landed_frame() {
        let gpu = metal_render::gpu::Gpu::new_headless();
        let cx = GpuCx::new(gpu.device.clone(), gpu.queue.clone());
        let Some(mut hist) = Histogrammer::<String>::new(&gpu.device, &gpu.queue, gpu.compute) else {
            eprintln!("skipped: this adapter has no compute shaders");
            return;
        };
        let p = Presenter::default();
        p.enable(egui::Context::default());
        p.enable_histograms();
        p.want_histogram("k", true);
        let (w, h) = (33u32, 17u32);
        let px: Vec<u8> = (0..w * h * 4).map(|i| (i.wrapping_mul(37) % 251) as u8).collect();
        assert!(p.begin("k"));
        p.land("k", Some(cx.upload(&px, w, h)));
        // `poll` pumps the device each call; the window does the same per frame.
        for _ in 0..2000 {
            p.histograms_pending(&mut hist);
            if p.histogram("k").is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let got = p.histogram("k").expect("the frame is counted");
        assert_eq!(*got, histogram_cpu(&px, HIST_STRIDE));
        assert!(p.histogram("other").is_none(), "only keys that asked are counted");
    }

    /// The eyedropper (2026-10-12): the pixel a click reads off the landed
    /// texture is the texel the frame holds, byte for byte (the copy is raw,
    /// so the sRGB texture gives the encoded values the JPEG route encodes),
    /// and the JPEG route's pixel at the same point agrees within the codec's
    /// error.
    #[test]
    fn a_pick_reads_the_texel_the_click_points_at() {
        let gpu = metal_render::gpu::Gpu::new_headless();
        let cx = GpuCx::new(gpu.device.clone(), gpu.queue.clone());
        let p = Presenter::default();
        p.enable(egui::Context::default());
        let (w, h) = (96u32, 54u32);
        let px = gradient(w, h);
        assert!(p.begin("k"));
        p.land("k", Some(cx.upload(&px, w, h)));
        let (dw, _, rgb) = crate::shell::frames::decode_jpeg_rgb(&crate::paint::encode_jpeg(&px, w, h)).expect("decodes");
        for (u, v) in [(0.0f32, 0.0f32), (0.5, 0.25), (0.9, 0.95)] {
            p.request_pick("k", u, v);
            let got = wait_pick(&p, &cx);
            let x = ((u * w as f32) as u32).min(w - 1);
            let y = ((v * h as f32) as u32).min(h - 1);
            let i = ((y * w + x) * 4) as usize;
            assert_eq!(got, [px[i], px[i + 1], px[i + 2]], "the texel the click points at");
            let j = ((y * dw + x) * 3) as usize;
            for c in 0..3 {
                assert!(got[c].abs_diff(rgb[j + c]) <= 12, "the JPEG route's pixel within codec error: {got:?} vs {:?}", &rgb[j..j + 3]);
            }
        }
    }

    /// A smooth RGBA picture whose every pixel differs, so a misplaced read shows.
    fn gradient(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                [(x * 255 / (w - 1)) as u8, (y * 255 / (h - 1)) as u8, 128, 255]
            })
            .collect()
    }

    /// Asks for the pick and polls the window's pass until it lands.
    fn wait_pick(p: &Presenter, cx: &GpuCx) -> [u8; 3] {
        for _ in 0..2000 {
            if let Some(rgb) = p.picks_pending(cx) {
                return rgb;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("the pick never landed");
    }

    /// `cargo test -p metal-dash --lib --release shell::present::tests::measure_previews -- --ignored --nocapture`
    ///
    /// Rule 11, at the window's own size (960x540): the sensor picture before
    /// (JPEG encode + decode, the bytes the panel uploaded) against the capture
    /// frame uploaded once to the device; the eyedropper before (readback of the
    /// frame, JPEG encode + decode) against the one-pixel region copy.
    #[test]
    #[ignore = "prints the G6 numbers"]
    fn measure_previews() {
        use std::time::Instant;
        let gpu = metal_render::gpu::Gpu::new_headless();
        let cx = GpuCx::new(gpu.device.clone(), gpu.queue.clone());
        let (w, h) = (960u32, 540u32);
        let px = gradient(w, h);
        let f = cx.upload(&px, w, h);
        let n = 20u32;
        let wait = || cx.wait();
        let t = Instant::now();
        for _ in 0..n {
            std::hint::black_box(crate::shell::frames::decode_jpeg_rgb(&crate::paint::encode_jpeg(&px, w, h)));
        }
        let sensor_before = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        let t = Instant::now();
        for _ in 0..n {
            std::hint::black_box(cx.upload(&px, w, h).width());
            wait();
        }
        let sensor_after = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        let t = Instant::now();
        for _ in 0..n {
            let rgba = cx.readback(&f);
            std::hint::black_box(crate::shell::frames::decode_jpeg_rgb(&crate::paint::encode_jpeg(&rgba, w, h)));
        }
        let pick_before = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        let t = Instant::now();
        for _ in 0..n {
            let region = metal_perform::gpu_compose::region_async(&cx, &f, w / 2, h / 2, 1, 1);
            while region.poll(&cx).is_none() {}
        }
        let pick_after = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("MEASURE sensor {w}x{h}: JPEG encode+decode {sensor_before:.2} ms, texture upload {sensor_after:.2} ms");
        println!("MEASURE pick {w}x{h}: readback+JPEG encode+decode {pick_before:.2} ms, 1x1 region {pick_after:.3} ms");
    }
}

#[cfg(test)]
mod sequence_tap_tests {
    use super::*;

    fn tag(t_ms: u32) -> RecordTag {
        RecordTag { seq: "show".into(), t_ms, floats: vec![0.5] }
    }

    #[test]
    fn the_gap_between_landings_is_the_frame_time() {
        let t0 = Instant::now();
        assert_eq!(frame_gap_ms(None, t0), None, "the first landing has no gap");
        let g = frame_gap_ms(Some(t0), t0 + Duration::from_millis(40)).expect("gap");
        assert!((g - 40.0).abs() < 0.01, "{g}");
    }

    #[test]
    fn landings_count_and_measure_per_key() {
        let p = Presenter::default();
        assert!(p.landing("/output.jpg?[]").is_none(), "never landed");
        p.note_landed("/output.jpg?[]");
        let first = p.landing("/output.jpg?[]").expect("one landing");
        assert_eq!((first.count, first.gap_ms), (1, None));
        std::thread::sleep(Duration::from_millis(5));
        p.note_landed("/output.jpg?[]");
        let second = p.landing("/output.jpg?[]").expect("two landings");
        assert_eq!(second.count, 2);
        assert!(second.gap_ms.is_some_and(|g| g >= 4.0), "{second:?}");
        assert!(p.landing("/node.jpg?[]").is_none(), "other keys are not counted");
    }

    #[test]
    fn rgb_becomes_opaque_rgba() {
        assert_eq!(rgb_to_rgba(&[1, 2, 3, 4, 5, 6]), vec![1, 2, 3, 255, 4, 5, 6, 255]);
        assert!(rgb_to_rgba(&[]).is_empty());
    }

    #[test]
    fn a_frame_is_recorded_only_for_the_wanted_key_and_throttled() {
        let p = Presenter::default();
        let rgb = vec![9u8; 2 * 2 * 3];
        p.offer_frame("k", 2, 2, &rgb);
        assert!(p.take_records().is_empty(), "nothing wanted: nothing copied");
        p.set_record(Some(("k".into(), tag(100))));
        p.offer_frame("other", 2, 2, &rgb);
        assert!(p.take_records().is_empty(), "another key is not recorded");
        p.offer_frame("k", 2, 2, &rgb[..5]);
        assert!(p.take_records().is_empty(), "a short buffer is refused");
        p.offer_frame("k", 2, 2, &rgb);
        p.offer_frame("k", 2, 2, &rgb);
        let got = p.take_records();
        assert_eq!(got.len(), 1, "the second offer falls inside the record gap");
        assert_eq!(got[0].tag, tag(100));
        assert_eq!(got[0].rgba.len(), 2 * 2 * 4);
        assert!(got[0].rgba.chunks_exact(4).all(|px| px == [9, 9, 9, 255]));
    }

    #[test]
    fn the_ready_queue_keeps_the_newest_records() {
        let mut t = Taps::default();
        for i in 0..(MAX_READY as u32 + 3) {
            push_ready(&mut t, Recorded { tag: tag(i), w: 1, h: 1, rgba: vec![0; 4] });
        }
        assert_eq!(t.ready.len(), MAX_READY);
        assert_eq!(t.ready[0].tag.t_ms, 3, "the three oldest went first");
    }

    #[test]
    fn a_fallback_picture_is_set_per_key_and_cleared_with_the_sequence() {
        let p = Presenter::default();
        assert!(p.fallback("k").is_none());
        // A texture handle needs a context; the taps only store it, so a
        // default context is enough to make one.
        let ctx = egui::Context::default();
        let tex = ctx.load_texture("t", egui::ColorImage::example(), egui::TextureOptions::LINEAR);
        p.set_fallback("k", Some(tex.clone()));
        assert_eq!(p.fallback("k").map(|h| h.id()), Some(tex.id()));
        assert!(p.fallback("other").is_none());
        p.set_fallback("k", None);
        assert!(p.fallback("k").is_none());
        p.set_fallback("k", Some(tex));
        p.clear_sequence();
        assert!(p.fallback("k").is_none(), "clearing drops every fallback");
    }
}
