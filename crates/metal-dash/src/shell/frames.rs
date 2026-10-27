//! S.U9b (15-sept-2026): the seam that turns a JPEG route's bytes into an
//! [`egui::TextureHandle`] on a cadence — the shell's first image consumer
//! (`grep -rn TextureHandle crates/metal-dash/src/shell` was empty the day this
//! was written; the HTML dashboard pumped `<img>` tags where this paints).
//!
//! [`Frame`] is deliberately dumb: one texture slot, the instant the last
//! fetch was made, and nothing else. Panels own one [`Frame`] per image they
//! show — [`PanelState`](crate::shell::panels) fields keep them alive across
//! frames / [`Default`] resets them, which is exactly "drop the texture and
//! fetch again" when a panel's image goes stale. The decode half is the same
//! L8-replicating walk `monitor.rs`'s mode does for its window —
//! [`decode_jpeg_rgb`] is the shared function both call now, so the JPEG
//! shapes the two consumers can accept stay one vocabulary.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use web_time::{Duration, Instant};

use egui::{ColorImage, Context, TextureHandle, TextureOptions};

use crate::call::Call;

/// The one pacing cap for every preview picture, GPU texture or JPEG, on any
/// GPU: the global fps cap from settings (60 by default, up to 300).
/// Slower hardware simply runs below it: the worker's one-fetch-in-flight rule
/// means a render slower than a frame lowers the rate instead of queueing. The
/// rendering side is capped at the same rate (see the render loops that use it).
pub fn live_gap() -> Duration {
    crate::frame_cap::gap()
}

/// The picture width the preview asks the worker for under a software GPU
/// (SwiftShader, llvmpipe, lavapipe). A narrower picture is what a software
/// rasteriser can render and upload within a frame; the rate stays the same
/// cap ([`live_gap()`]) as on any other GPU. The route scales it up to the
/// painted size.
pub const SOFT_PREVIEW_W: f32 = 320.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// The preview gap follows the global cap (60 fps unless settings say otherwise).
    #[test]
    fn the_preview_gap_is_one_over_the_cap() {
        let want = 1.0 / f64::from(crate::frame_cap::get());
        assert!((live_gap().as_secs_f64() - want).abs() < 1e-9);
    }
}

/// Set once when the window opens, from the adapter: a software one.
static SOFTWARE_GPU: AtomicBool = AtomicBool::new(false);

/// Records whether the window paints on a software GPU. Only `window::open`
/// calls it; the shell reads it through [`software_gpu`].
pub fn set_software_gpu(on: bool) {
    SOFTWARE_GPU.store(on, Ordering::Relaxed);
}

/// Whether the window is painting on a software GPU, so the shell can reduce
/// its own work (the preview's size, the animating panels' pace).
pub fn software_gpu() -> bool {
    SOFTWARE_GPU.load(Ordering::Relaxed)
}

/// Ask for an animation frame from a panel that is animating. The window's
/// own pace caps it at 60 fps (`window.rs`).
#[track_caller]
pub fn animate(ctx: &Context) {
    repaint(ctx);
}

/// Whether repaint requests are logged with their caller (`FW_REPAINT_TRACE`
/// set in the environment, or a `repaint-trace` file in the working directory,
/// which is the app's files dir on Android). A debug aid for idle repaints.
pub fn tracing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("FW_REPAINT_TRACE").is_some() || Path::new("repaint-trace").exists())
}

/// `ctx.request_repaint()`, logged under [`tracing`] with the caller's site.
#[track_caller]
pub fn repaint(ctx: &Context) {
    if tracing() {
        eprintln!("[repaint] now {}", std::panic::Location::caller());
    }
    ctx.request_repaint();
}

/// `ctx.request_repaint_after(after)`, logged under [`tracing`] with the
/// caller's site.
///
/// Under `cfg(test)` it asks for nothing: kittest steps in wall-clock time, so
/// a timed repaint that re-arms every frame keeps `Harness::run` stepping
/// until `max_steps` on a loaded machine, and the same test passes or fails
/// with the load. Production builds are unchanged.
#[track_caller]
pub fn repaint_after(ctx: &Context, after: Duration) {
    if cfg!(test) {
        return;
    }
    if tracing() {
        eprintln!("[repaint] after {:?} {}", after, std::panic::Location::caller());
    }
    ctx.request_repaint_after(after);
}

/// Every how many pixels the histogram samples — a preview frame is small,
/// and a quarter of it draws the same shape.
const HIST_STRIDE: usize = 4;

/// The off-thread half of the pump: one worker, the door moved into it, and
/// a request in / a decoded frame out.
///
/// `S.U9b` fetched on the frame's own thread. That was affordable while a
/// grid render was a few milliseconds; it is not now — the grid is thirty-six
/// models in a debug build and one fetch is a visible stall of the whole
/// client. The worker is what keeps [`Frame::tick`] a poll instead of a
/// block: the frame asks, keeps painting, and takes the texture when it is
/// there. One worker rather than one thread per fetch on purpose — the
/// renders it drives share the studio's own lock, and serialising them keeps
/// two pumps from queueing behind each other.
#[derive(Clone)]
pub struct Fetcher {
    tx: Sender<Job>,
    /// G3: the mailbox textures land in when the window shares the painter's
    /// device.
    presenter: super::present::Presenter,
}

/// The shell publishes its worker to egui memory each frame, so a panel
/// deep in the tree (an editor's canvas) can pump a picture without the
/// handle being threaded through every signature between it and `Shell::ui`.
pub fn publish_fetcher(ctx: &Context, fetcher: &Fetcher) {
    ctx.data_mut(|d| d.insert_temp(fetcher_id(), fetcher.clone()));
}

fn fetcher_id() -> egui::Id {
    egui::Id::new("shell/frames/fetcher")
}

fn fetcher_of(ctx: &Context) -> Option<Fetcher> {
    ctx.data(|d| d.get_temp::<Fetcher>(fetcher_id()))
}

/// One request: the route, its query already owned, and the channel the
/// decoded frame comes back on.
enum Job {
    Bytes(String, Vec<(String, String)>, Sender<Option<Landed>>),
    /// G3: ask the door for the texture behind the route; it lands in the
    /// presenter under the key.
    Texture(String, Vec<(String, String)>, String),
}

impl Fetcher {
    #[cfg(test)]
    /// Starts the worker around `door` — the same door the shell walks,
    /// moved here so the fetch never runs on the frame's own thread.
    ///
    /// A worker that cannot spawn leaves a `Fetcher` whose sends fail; every
    /// `tick` then just keeps the last texture, which is the same "last good
    /// frame outlives a hiccup" rule the synchronous path had.
    pub fn spawn<D: Call + Send + 'static>(door: D) -> Fetcher {
        Fetcher::spawn_presenting(door, super::present::Presenter::default())
    }

    /// [`Fetcher::spawn`] with the mailbox the window drains.
    pub fn spawn_presenting<D: Call + Send + 'static>(door: D, presenter: super::present::Presenter) -> Fetcher {
        let (tx, rx) = mpsc::channel::<Job>();
        let landing = presenter.clone();
        let _ = thread::Builder::new()
            .name("metal-dash previews".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    match job {
                        Job::Bytes(path, query, back) => {
                            let q: Vec<(&str, String)> =
                                query.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
                            let decoded = crate::perf::timed("GET", &path, || door.bytes("GET", &path, &q))
                                .ok()
                                .and_then(|reply| decode_jpeg_rgb(&reply.body))
                                .map(|(w, h, rgb)| {
                                    let fp = fingerprint(&rgb);
                                    (w, h, rgb, fp)
                                });
                            let _ = back.send(decoded);
                        }
                        Job::Texture(path, query, key) => {
                            let q: Vec<(&str, String)> =
                                query.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
                            landing.land(&key, crate::perf::timed("TEXTURE", &path, || texture_of(&door, &path, &q)));
                        }
                    }
                }
            });
        Fetcher { tx, presenter }
    }

    /// Hands the worker one fetch, or `None` if it is not there to take it.
    fn start(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Option<Receiver<Option<Landed>>> {
        let (back, rx) = mpsc::channel();
        let query = query
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        self.tx.send(Job::Bytes(path.to_string(), query, back)).ok()?;
        Some(rx)
    }

    /// The mailbox the window drains and the previews read.
    pub fn presenter(&self) -> &super::present::Presenter {
        &self.presenter
    }

    /// Hands the worker one texture fetch for `key` (the caller has already
    /// won [`Presenter::begin`](super::present::Presenter::begin)).
    pub fn start_texture(&self, key: &str, path: &str, query: &[(&str, String)]) -> bool {
        let query = query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        self.tx.send(Job::Texture(path.to_string(), query, key.to_string())).is_ok()
    }
}

#[cfg(feature = "gpu")]
fn texture_of<D: Call>(door: &D, path: &str, query: &[(&str, String)]) -> Option<metal_perform::gpu_frame::GpuFrame> {
    door.texture(path, query)
}

#[cfg(not(feature = "gpu"))]
fn texture_of<D: Call>(_: &D, _: &str, _: &[(&str, String)]) -> Option<metal_perform::gpu_frame::GpuFrame> {
    None
}

/// One pumped image: the texture the last fetch decoded into, *when* that
/// fetch was made — [`Self::tick`] asks the door again only after
/// `min_gap` has passed (the live pictures pass [`live_gap()`]) —
/// and, with a [`Fetcher`], the fetch still in flight.
pub struct Frame {
    tex: Option<TextureHandle>,
    last: Option<Instant>,
    in_flight: Option<Receiver<Option<Landed>>>,
    /// The fingerprint of the picture `tex` holds: a landed frame with the
    /// same one is not uploaded again (see [`Self::show`]).
    fp: Option<u64>,
    /// When asked for ([`Self::want_histogram`]), the last frame's RGB +
    /// luma histogram — what the grade page's histogram paints.
    hist: Option<Box<Histogram>>,
    want_hist: bool,
    /// When the eyedropper is armed ([`Self::want_sample`]), the last landed
    /// frame's pixels, so a click can read the colour under it.
    pixels: Option<(u32, u32, Vec<u8>)>,
    want_pick: bool,
    /// How many frames have landed — the preview bar's fps reads its rate.
    landed: u64,
    /// G5: the presented picture (a shared-device texture), when it is one.
    pic: Option<(egui::TextureId, [usize; 2])>,
}

/// A decoded frame as the worker hands it back: size, RGB bytes and their
/// [`fingerprint`], which the worker computes off the UI thread.
type Landed = (u32, u32, Vec<u8>, u64);

/// A 64-bit fingerprint of an RGB buffer (FNV-style over 8-byte words). Two
/// frames with equal fingerprints are treated as the same picture.
pub fn fingerprint(rgb: &[u8]) -> u64 {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = 0xcbf2_9ce4_8422_2325 ^ rgb.len() as u64;
    let mut words = rgb.chunks_exact(8);
    for w in &mut words {
        let mut b = [0u8; 8];
        b.copy_from_slice(w);
        h = (h ^ u64::from_le_bytes(b)).wrapping_mul(PRIME);
    }
    for &b in words.remainder() {
        h = (h ^ u64::from(b)).wrapping_mul(PRIME);
    }
    h
}

/// 256 bins each of red, green, blue and luma.
pub type Histogram = [[u32; 256]; 4];

/// The histogram of an RGB buffer, every `stride`-th pixel.
pub fn histogram_of(rgb: &[u8], stride: usize) -> Histogram {
    let mut h = [[0u32; 256]; 4];
    for px in rgb.chunks_exact(3).step_by(stride.max(1)) {
        let (r, g, b) = (px[0], px[1], px[2]);
        h[0][r as usize] += 1;
        h[1][g as usize] += 1;
        h[2][b as usize] += 1;
        let y = (u32::from(r) * 54 + u32::from(g) * 183 + u32::from(b) * 19) >> 8;
        h[3][y.min(255) as usize] += 1;
    }
    h
}

impl Default for Frame {
    fn default() -> Self {
        Frame {
            tex: None,
            last: None,
            in_flight: None,
            fp: None,
            hist: None,
            want_hist: false,
            pixels: None,
            want_pick: false,
            landed: 0,
            pic: None,
        }
    }
}

impl Frame {
    /// The picture the last landed fetch left, texture or presented: the id and
    /// size egui draws it with, without asking for another.
    pub fn current_picture(&self) -> Option<(egui::TextureId, [usize; 2])> {
        if let Some(tex) = self.tex.as_ref() {
            return Some((tex.id(), tex.size()));
        }
        self.pic
    }

    /// The texture the last completed fetch left, without asking for
    /// another — for a second reader of the same pump (the `+ nodo` menu's
    /// hover tile reads the grid the canvas is already ticking) so one
    /// image is fetched once per cadence however many places paint it.
    pub fn current(&self) -> Option<&TextureHandle> {
        self.tex.as_ref()
    }

    /// How many frames have landed since this pump started.
    pub fn landed(&self) -> u64 {
        self.landed
    }

    /// Ask for (or stop) the per-frame histogram.
    pub fn want_histogram(&mut self, on: bool) {
        self.want_hist = on;
        if !on {
            self.hist = None;
        }
    }

    /// The last frame's histogram, when asked for and one has landed.
    pub fn histogram(&self) -> Option<&Histogram> {
        self.hist.as_deref()
    }

    /// Keep (or drop) the landed frame's pixels, for the eyedropper.
    pub fn want_sample(&mut self, on: bool) {
        self.want_pick = on;
        if !on {
            self.pixels = None;
        }
    }

    /// The RGB under `(u, v)` (each in `0..1` of the picture), when the
    /// pixels are kept and a frame has landed.
    pub fn sample(&self, u: f32, v: f32) -> Option<[u8; 3]> {
        let (w, h, rgb) = self.pixels.as_ref()?;
        let x = ((u.clamp(0.0, 1.0) * *w as f32) as u32).min(w.saturating_sub(1));
        let y = ((v.clamp(0.0, 1.0) * *h as f32) as u32).min(h.saturating_sub(1));
        let i = ((y * w + x) * 3) as usize;
        rgb.get(i..i + 3).map(|p| [p[0], p[1], p[2]])
    }

    /// The eolf of one frame: after `min_gap`, `GET` the image through the
    /// door, decode it, and hand back what to paint this frame — `None` is
    /// a route that has not answered once yet (paint nothing; the panel
    /// says why itself), `Some` is the current texture even when this
    /// frame's own fetch failed (last good frame outlives a hiccup, the
    /// way `pump()` kept the last `<img>` contents on a 503).
    ///
    /// With a `fetcher` the fetch itself is the worker's, not this thread's:
    /// `tick` only starts it and later picks the decoded pixels up — so a
    /// render that takes a fifth of a second costs the frame nothing. The
    /// `door` is still here, and still the only path, for the builds and
    /// tests without a worker (`kittest`'s harness): those fetch inline,
    /// exactly as `S.U9b` shipped it.
    pub fn tick(
        &mut self,
        ctx: &Context,
        door: &dyn Call,
        fetcher: Option<&Fetcher>,
        path: &str,
        query: &[(&str, String)],
        min_gap: Duration,
    ) -> Option<&TextureHandle> {
        if let Some(rx) = &self.in_flight {
            match rx.try_recv() {
                Ok(Some((w, h, rgb, fp))) => {
                    if self.want_hist {
                        self.hist = Some(Box::new(histogram_of(&rgb, HIST_STRIDE)));
                    }
                    self.show(ctx, path, w, h, &rgb, fp);
                    if self.want_pick {
                        self.pixels = Some((w, h, rgb));
                    }
                    self.in_flight = None;
                }
                // A failed fetch keeps the last good texture and frees the
                // slot, so the next due tick tries again.
                Ok(None) | Err(TryRecvError::Disconnected) => self.in_flight = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let due = self.last.map(|l| l.elapsed() >= min_gap).unwrap_or(true);
        if due {
            match fetcher {
                Some(f) => {
                    if self.in_flight.is_none() {
                        self.in_flight = f.start(path, query);
                        self.last = Some(Instant::now());
                        repaint_after(ctx, min_gap);
                    }
                }
                None => {
                    if let Ok(reply) = door.bytes("GET", path, query) {
                        match decode_jpeg_rgb(&reply.body) {
                            Some((w, h, rgb)) => {
                                let fp = fingerprint(&rgb);
                                self.show(ctx, path, w, h, &rgb, fp);
                                if self.want_pick {
                                    self.pixels = Some((w, h, rgb));
                                }
                            }
                            None => {
                                self.tex = None;
                                self.fp = None;
                            }
                        }
                        self.last = Some(Instant::now());
                        // No repaint request on this path (2026-10-09). Only the
                        // kittest harness and the builds without a worker fetch
                        // inline; `window::run` always has a `Fetcher`, which keeps
                        // its own repaint duty. A request here per fetch kept
                        // `Harness::run` stepping until `max_steps` panicked.
                    }
                }
            }
        }
        self.tex.as_ref()
    }

    /// A picture for an editor canvas, asked through the worker the shell
    /// published ([`publish_fetcher`]): the shared-device texture when the
    /// window presents one, else the worker's JPEG, else an inline fetch (no
    /// worker: the kittest harness). `None` until the route has answered once.
    pub fn picture(
        &mut self,
        ctx: &Context,
        door: &dyn Call,
        path: &str,
        query: &[(&str, String)],
        min_gap: Duration,
    ) -> Option<(egui::TextureId, [usize; 2])> {
        use super::present::Shown;
        let _span = crate::perf::Span::new("frame.picture");
        let fetcher = fetcher_of(ctx);
        if let Some(f) = fetcher.as_ref() {
            let key = format!("{path}?{query:?}");
            match self.tick_texture(ctx, f, &key, path, query, min_gap) {
                Shown::Ready { id, size, .. } => {
                    let picture = (id, [size[0] as usize, size[1] as usize]);
                    self.pic = Some(picture);
                    return Some(picture);
                }
                Shown::Pending => return None,
                Shown::Unsupported => {}
            }
        }
        let tex = self.tick(ctx, door, fetcher.as_ref(), path, query, min_gap)?;
        Some((tex.id(), tex.size()))
    }

    /// G3: the device frame as a texture the window shares with the painter,
    /// instead of a JPEG. Asks the worker for a fresh one after `min_gap` and
    /// reports what is on screen; [`Shown::Unsupported`] (retried about once a
    /// second) sends the caller to [`Self::tick`].
    pub fn tick_texture(
        &mut self,
        ctx: &Context,
        fetcher: &Fetcher,
        key: &str,
        path: &str,
        query: &[(&str, String)],
        min_gap: Duration,
    ) -> super::present::Shown {
        use super::present::Shown;
        let presenter = fetcher.presenter();
        if !presenter.enabled() || (self.want_hist && !presenter.histograms()) {
            return Shown::Unsupported;
        }
        // The caller's gap is honoured. Live previews pass `live_gap()` already;
        // the node grid passes its own cadence (8 Hz, or 1 s behind a preview).
        // Clamping it to the display rate here re-rendered the grid every frame.
        presenter.mark_live();
        // G5: a grade's histogram is counted on the GPU for this picture, and
        // the last one counted is kept here for the grade page.
        presenter.want_histogram(key, self.want_hist);
        if self.want_hist {
            if let Some(h) = presenter.histogram(key) {
                self.hist = Some(h);
            }
        }
        let shown = presenter.shown(key);
        let gap = if matches!(shown, Shown::Unsupported) { Duration::from_secs(1) } else { min_gap };
        let due = self.last.map(|l| l.elapsed() >= gap).unwrap_or(true);
        if due && presenter.begin(key) {
            if fetcher.start_texture(key, path, query) {
                self.last = Some(Instant::now());
                repaint_after(ctx, min_gap);
            } else {
                presenter.land(key, None);
            }
        }
        shown
    }

    /// Puts a landed frame on screen: into the texture already held (2026-10-06 —
    /// a fresh `load_texture` per frame allocated and freed a GPU texture at
    /// the pump's full rate), or a new one for the first frame. A frame whose
    /// picture is the one already held is not converted or uploaded again: an
    /// idle route answers the same JPEG at every landing, and each upload
    /// copied the whole image to the GPU in the paint pass for nothing.
    fn show(&mut self, ctx: &Context, path: &str, w: u32, h: u32, rgb: &[u8], fp: u64) {
        self.landed += 1;
        if self.tex.is_some() && self.fp == Some(fp) {
            return;
        }
        let image = ColorImage::from_rgb([w as usize, h as usize], rgb);
        match &mut self.tex {
            Some(tex) => tex.set(image, TextureOptions::LINEAR),
            None => self.tex = Some(ctx.load_texture(path, image, TextureOptions::LINEAR)),
        }
        self.fp = Some(fp);
    }
}

/// A JPEG's pixels as RGB bytes (zune-jpeg converts to RGB on decode; a
/// grayscale or CMYK source comes out as RGB too). `None` for a picture
/// that does not decode.
pub fn decode_jpeg_rgb(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};
    // zune-jpeg, not jpeg-decoder: it decodes a 1280x720 frame several times
    // faster in release, and that cost is paid once per remote layer per frame.
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(bytes), options);
    let rgb = decoder.decode().ok()?;
    let info = decoder.info()?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    (rgb.len() == (width * height * 3) as usize).then_some((width, height, rgb))
}
