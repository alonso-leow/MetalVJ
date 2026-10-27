//! G1 — compose on the GPU: the layer stack as shader passes on the same wgpu
//! device the models draw on, so a model's output never leaves the GPU between
//! the renderer and the compositor.
//!
//! # The contract wave-2 passes implement
//!
//! * Every picture is a [`GpuFrame`]: an `Rgba8UnormSrgb` texture (the format
//!   the models draw in, so a readback is the same bytes `solo_chain` always
//!   returned) with an `Rgba8Unorm` **view**. Passes read and write through
//!   that view, so no sRGB conversion happens anywhere in compose and the
//!   blending is on the encoded bytes exactly as [`crate::compose`] does it.
//! * A pass is a [`GpuPass`]: input frames in, one pre-allocated output frame
//!   out, recorded into the caller's `CommandEncoder`. Allocate from
//!   [`GpuCx::alloc`] (pooled; a dropped frame returns its texture), upload CPU
//!   pixels with [`GpuCx::upload`], and read back with [`GpuCx::readback`] only
//!   at a sink that needs bytes. [`GpuCx::run`] is the one-pass convenience.
//! * The CPU code ([`crate::compose`], [`crate::transition`], [`crate::text`])
//!   is the reference every GPU pass is diffed against with a tolerance.
//!
//! Compose itself ([`GpuCompose`]) is one command buffer per frame: a clear,
//! then per layer a copy of the sector rectangle already composed (the
//! `under` the blend mode reads) and a scissored draw. That is the single
//! layer-stack pass; the per-layer copy is a rectangle-sized GPU copy, not a
//! frame-sized round trip.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::gpu_frame::{GpuTile, TileSource};
use crate::layers::{Blend, ClipMode, Composition};

/// The texture format every frame is allocated in.
pub const FRAME_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// The format passes sample and write through.
pub const VIEW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const POOL_KEEP: usize = 12;

struct Pool {
    device: wgpu::Device,
    free: Mutex<HashMap<(u32, u32), Vec<(wgpu::Texture, wgpu::TextureView)>>>,
}

impl Pool {
    fn take(self: &Arc<Self>, w: u32, h: u32) -> GpuFrame {
        let (w, h) = (w.max(1), h.max(1));
        let hit = self.free.lock().ok().and_then(|mut f| f.get_mut(&(w, h)).and_then(Vec::pop));
        let tex = hit.unwrap_or_else(|| {
            let t = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("gpu frame"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FRAME_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[VIEW_FORMAT],
            });
            let v = t.create_view(&wgpu::TextureViewDescriptor { format: Some(VIEW_FORMAT), ..Default::default() });
            (t, v)
        });
        GpuFrame { inner: Arc::new(Inner { tex: Some(tex), w, h, pool: self.clone() }) }
    }
}

struct Inner {
    tex: Option<(wgpu::Texture, wgpu::TextureView)>,
    w: u32,
    h: u32,
    pool: Arc<Pool>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let (Some(t), Ok(mut free)) = (self.tex.take(), self.pool.free.lock()) {
            let slot = free.entry((self.w, self.h)).or_default();
            if slot.len() < POOL_KEEP {
                slot.push(t);
            }
        }
    }
}

/// A picture that lives on the GPU. Cheap to clone (a handle); the texture
/// returns to its pool when the last handle drops.
#[derive(Clone)]
pub struct GpuFrame {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for GpuFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GpuFrame({}x{})", self.inner.w, self.inner.h)
    }
}

impl GpuFrame {
    pub fn width(&self) -> u32 {
        self.inner.w
    }
    pub fn height(&self) -> u32 {
        self.inner.h
    }
    /// The underlying `Rgba8UnormSrgb` texture (copies, readback, and the
    /// model renderer's `solo_chain_into` target).
    pub fn texture(&self) -> &wgpu::Texture {
        &self.inner.tex.as_ref().expect("live frame").0
    }
    /// The `Rgba8Unorm` view passes sample and render through.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.inner.tex.as_ref().expect("live frame").1
    }
}

/// The shared device plus the frame pool: what every pass is handed.
#[derive(Clone)]
pub struct GpuCx {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pool: Arc<Pool>,
}

impl GpuCx {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> GpuCx {
        let pool = Arc::new(Pool { device: device.clone(), free: Mutex::default() });
        GpuCx { device, queue, pool }
    }

    /// A frame of `w x h` (contents undefined).
    pub fn alloc(&self, w: u32, h: u32) -> GpuFrame {
        self.pool.take(w, h)
    }

    /// CPU RGBA8 pixels into a frame.
    pub fn upload(&self, pixels: &[u8], w: u32, h: u32) -> GpuFrame {
        let f = self.alloc(w, h);
        let n = (w as usize) * (h as usize) * 4;
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: f.texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels[..n.min(pixels.len())],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        f
    }

    /// The `cw x ch` rectangle at `(x, y)` of `f` as its own frame: a GPU
    /// rectangle copy, no readback.
    pub fn crop(&self, f: &GpuFrame, x: u32, y: u32, cw: u32, ch: u32) -> GpuFrame {
        let out = self.alloc(cw, ch);
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("crop") });
        enc.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: f.texture(), mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo { texture: out.texture(), mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(enc.finish()));
        out
    }

    /// Waits until every submitted copy has finished (a test or a measure;
    /// the frame loop never waits).
    pub fn wait(&self) {
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
    }

    /// Blocking readback: RGBA8 bytes. Only for a sink that needs CPU pixels
    /// *now*; the frame loop uses [`AsyncReadback`].
    pub fn readback(&self, f: &GpuFrame) -> Vec<u8> {
        metal_render::gpu::read_rgba8(&self.device, &self.queue, f.texture(), f.width(), f.height())
    }

    /// The copy is queued now (call it with the frame in hand, under whatever
    /// lock holds the painter); the returned closure waits for it and returns
    /// the RGBA8 bytes, and may run on any thread, so a sink that needs *this*
    /// frame still never holds the frame loop while the GPU finishes.
    pub fn readback_deferred(&self, f: &GpuFrame) -> Box<dyn FnOnce() -> Vec<u8> + Send> {
        let (w, h) = (f.width(), f.height());
        let bpr = (w * 4).next_multiple_of(256);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("deferred readback"),
            size: (bpr * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("deferred readback") });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: f.texture(), mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(enc.finish()));
        let device = self.device.clone();
        Box::new(move || {
            let (tx, rx) = std::sync::mpsc::channel();
            buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
            if !matches!(rx.recv(), Ok(Ok(()))) {
                return Vec::new();
            }
            let data = match buffer.slice(..).get_mapped_range() {
                Ok(d) => d,
                Err(_) => return Vec::new(),
            };
            let row = (w * 4) as usize;
            let mut out = Vec::with_capacity(row * h as usize);
            for y in 0..h as usize {
                out.extend_from_slice(&data[y * bpr as usize..y * bpr as usize + row]);
            }
            drop(data);
            buffer.unmap();
            out
        })
    }

    /// `src` (a `w x h` texture of this device in [`FRAME_FORMAT`], such as a
    /// renderer's canvas) copied into a frame of the pool: a GPU copy, queued
    /// in order, nothing read back.
    pub fn copy_texture(&self, src: &wgpu::Texture, w: u32, h: u32) -> GpuFrame {
        let out = self.alloc(w, h);
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("copy texture") });
        enc.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: src, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo { texture: out.texture(), mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(enc.finish()));
        out
    }

    /// Runs one pass over `inputs` into a fresh `w x h` frame: allocate,
    /// encode, submit.
    pub fn run(&self, pass: &mut dyn GpuPass, inputs: &[&GpuFrame], w: u32, h: u32) -> GpuFrame {
        let out = self.alloc(w, h);
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(pass.name()) });
        pass.encode(self, &mut enc, inputs, &out);
        self.queue.submit(Some(enc.finish()));
        out
    }
}

/// One GPU pass: input textures to an output texture on the shared device.
///
/// `encode` records commands into `enc`; it does not submit and does not read
/// back. `output` is allocated by the caller (any size; usually `inputs[0]`'s)
/// and its prior contents are undefined, so a pass writes every pixel it owns
/// (clear first when it covers a sub-rectangle). A pass that needs scratch
/// frames takes them from `cx.alloc` and lets them drop at the end of `encode`
/// (the pool keeps the texture alive for the recorded commands' ordering).
/// Per-pass state (pipelines, histories) lives in the implementing struct.
pub trait GpuPass {
    fn name(&self) -> &'static str;
    fn encode(&mut self, cx: &GpuCx, enc: &mut wgpu::CommandEncoder, inputs: &[&GpuFrame], output: &GpuFrame);
}

// ---------------------------------------------------------------------------
// async readback
// ---------------------------------------------------------------------------

struct Slot {
    buf: wgpu::Buffer,
    cap: u64,
    state: Option<(Arc<std::sync::atomic::AtomicBool>, u32, u32, u32, std::time::Instant)>,
}

/// Double-buffered readback for sinks (Art-Net, NDI, v4l2, PNG, peer frames):
/// [`submit`](AsyncReadback::submit) enqueues a copy and returns at once;
/// [`poll`](AsyncReadback::poll) hands back a finished frame without waiting,
/// so the frame loop never stalls on the GPU. A sink therefore sees its pixels
/// one or two frames late, which is invisible on a wire and what keeps the
/// window smooth.
pub struct AsyncReadback {
    slots: [Slot; 2],
    next: usize,
    /// Slot indices in submission order, oldest first.
    queued: std::collections::VecDeque<usize>,
}

impl AsyncReadback {
    pub fn new(cx: &GpuCx) -> AsyncReadback {
        let slot = || Slot {
            buf: cx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("async readback"),
                size: 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            cap: 256,
            state: None,
        };
        AsyncReadback { slots: [slot(), slot()], next: 0, queued: Default::default() }
    }

    /// Enqueues a copy of `f`. Returns `false` (and does nothing) when both
    /// slots are still in flight: the sink skips a frame instead of blocking.
    pub fn submit(&mut self, cx: &GpuCx, f: &GpuFrame) -> bool {
        let i = self.next;
        if self.slots[i].state.is_some() {
            return false;
        }
        let (w, h) = (f.width(), f.height());
        let bpr = (w * 4).next_multiple_of(256);
        let need = (bpr as u64) * h as u64;
        let slot = &mut self.slots[i];
        if slot.cap < need {
            slot.buf = cx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("async readback"),
                size: need,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            slot.cap = need;
        }
        let mut enc = cx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("async readback") });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: f.texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &slot.buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        cx.queue.submit(Some(enc.finish()));
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = done.clone();
        slot.buf.slice(..need).map_async(wgpu::MapMode::Read, move |r| {
            if r.is_ok() {
                flag.store(true, std::sync::atomic::Ordering::Release);
            }
        });
        slot.state = Some((done, w, h, bpr, std::time::Instant::now()));
        self.queued.push_back(i);
        self.next = 1 - i;
        true
    }

    /// The oldest finished frame that is still fresh (under [`MAX_AGE`]), if
    /// any; never blocks. A finished copy older than that is dropped: a sink
    /// polled seldom must not be handed a picture from minutes ago.
    pub fn poll(&mut self, cx: &GpuCx) -> Option<(Vec<u8>, u32, u32)> {
        let _ = cx.device.poll(wgpu::PollType::Poll);
        loop {
            let &i = self.queued.front()?;
            let slot = &mut self.slots[i];
            let (done, w, h, bpr, at) = slot.state.as_ref()?.clone();
            if !done.load(std::sync::atomic::Ordering::Acquire) {
                return None;
            }
            self.queued.pop_front();
            slot.state = None;
            let need = (bpr as u64) * h as u64;
            if at.elapsed() > MAX_AGE {
                slot.buf.unmap();
                continue;
            }
            let out = {
                let data = slot.buf.slice(..need).get_mapped_range().ok()?;
                let row = (w * 4) as usize;
                let mut out = Vec::with_capacity(row * h as usize);
                for y in 0..h as usize {
                    out.extend_from_slice(&data[y * bpr as usize..y * bpr as usize + row]);
                }
                out
            };
            slot.buf.unmap();
            return Some((out, w, h));
        }
    }
}

/// How old a finished async copy may be and still be handed to a sink.
pub const MAX_AGE: std::time::Duration = std::time::Duration::from_millis(250);

/// A `w x h` rectangle of a frame copied out of its texture, read without
/// waiting: [`region_async`] queues the copy and the map, and
/// [`PendingRegion::poll`] hands back the RGBA8 bytes once the GPU is done.
/// The eyedropper reads one pixel this way instead of a whole JPEG.
pub struct PendingRegion {
    buf: wgpu::Buffer,
    done: Arc<std::sync::atomic::AtomicBool>,
    w: u32,
    h: u32,
    bpr: u32,
}

/// Queues a copy of the `w x h` rectangle at `(x, y)` of `f` into a mappable
/// buffer and asks for the map. Nothing is waited for; the rectangle must lie
/// inside `f`.
pub fn region_async(cx: &GpuCx, f: &GpuFrame, x: u32, y: u32, w: u32, h: u32) -> PendingRegion {
    let bpr = (w * 4).next_multiple_of(256);
    let buf = cx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("region readback"),
        size: (bpr * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = cx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("region readback") });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: f.texture(), mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    cx.queue.submit(Some(enc.finish()));
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = done.clone();
    buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        if r.is_ok() {
            flag.store(true, std::sync::atomic::Ordering::Release);
        }
    });
    PendingRegion { buf, done, w, h, bpr }
}

impl PendingRegion {
    /// The region's RGBA8 bytes (rows packed, no padding) once the copy has
    /// finished; `None` while it is still in flight. Never blocks.
    pub fn poll(&self, cx: &GpuCx) -> Option<Vec<u8>> {
        let _ = cx.device.poll(wgpu::PollType::Poll);
        if !self.done.load(std::sync::atomic::Ordering::Acquire) {
            return None;
        }
        let need = (self.bpr * self.h) as u64;
        let out = {
            let data = self.buf.slice(..need).get_mapped_range().ok()?;
            let row = (self.w * 4) as usize;
            let mut out = Vec::with_capacity(row * self.h as usize);
            for y in 0..self.h as usize {
                out.extend_from_slice(&data[y * self.bpr as usize..y * self.bpr as usize + row]);
            }
            out
        };
        self.buf.unmap();
        Some(out)
    }
}

// ---------------------------------------------------------------------------
// the layer-stack compose
// ---------------------------------------------------------------------------

const SHADER: &str = r#"
struct U {
    sector: vec4<i32>,   // x0, y0, sw, sh
    tile: vec4<i32>,     // tw, th, mode (0 overlap, 1 sectors), blend
    f: vec4<f32>,        // opacity, cos, sin, fit
    g: vec4<i32>,        // giro on, fit on, has cover, warp (0 none, 1 bilinear, 2 nearest)
    m0: vec4<f32>,       // warp: inverse map rows (device px -> texel)
    m1: vec4<f32>,
    m2: vec4<f32>,
};
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var under: texture_2d<f32>;
@group(0) @binding(2) var<uniform> u: U;
@group(0) @binding(3) var cover_map: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn mixc(a: vec3<f32>, b: vec3<f32>, op: f32, blend: i32) -> vec3<f32> {
    var r = b;
    if (blend == 1) { r = a + b; }
    else if (blend == 2) { r = a * b; }
    else if (blend == 3) { r = vec3<f32>(1.0) - (vec3<f32>(1.0) - a) * (vec3<f32>(1.0) - b); }
    return clamp(a + (r - a) * op, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn texel(x: i32, y: i32) -> vec4<f32> {
    let tw = u.tile.x; let th = u.tile.y;
    return textureLoad(src, vec2<i32>(clamp(x, 0, tw - 1), clamp(y, 0, th - 1)), 0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let d = vec2<i32>(pos.xy);
    let ud = textureLoad(under, d, 0);
    var clip = 1.0;
    if (u.g.z != 0) {
        clip = textureLoad(cover_map, d, 0).r;
        if (clip <= 0.0) { return ud; }
    }
    let x0 = u.sector.x; let y0 = u.sector.y;
    let sw = u.sector.z; let sh = u.sector.w;
    var col: vec4<f32>;
    if (u.g.w != 0) {
        // Through the warp's inverse map; integer device coordinates, as the CPU.
        let x = f32(d.x); let y = f32(d.y);
        let w = u.m2.x * x + u.m2.y * y + u.m2.z;
        if (abs(w) < 1e-9) { return ud; }
        let tx = (u.m0.x * x + u.m0.y * y + u.m0.z) / w;
        let ty = (u.m1.x * x + u.m1.y * y + u.m1.z) / w;
        if (u.g.w == 2) {
            col = texel(i32(clamp(floor(tx + 0.5), -1.0e6, 1.0e6)), i32(clamp(floor(ty + 0.5), -1.0e6, 1.0e6)));
        } else {
            let fx = clamp(floor(tx), -1.0e6, 1.0e6);
            let fy = clamp(floor(ty), -1.0e6, 1.0e6);
            let ax = tx - fx; let ay = ty - fy;
            let xi = i32(fx); let yi = i32(fy);
            let p00 = texel(xi, yi);
            let p10 = texel(xi + 1, yi);
            let p01 = texel(xi, yi + 1);
            let p11 = texel(xi + 1, yi + 1);
            let top = p00 + (p10 - p00) * ax;
            let bottom = p01 + (p11 - p01) * ax;
            col = top + (bottom - top) * ay;
        }
    } else {
        var s = d;
        if (u.g.x != 0) {
            let cx = f32(x0) + f32(sw) / 2.0;
            let cy = f32(y0) + f32(sh) / 2.0;
            let px = f32(d.x) - cx;
            let py = f32(d.y) - cy;
            var rx = u.f.y * px + u.f.z * py;
            var ry = -u.f.z * px + u.f.y * py;
            if (u.g.y != 0) { rx = rx * u.f.w; ry = ry * u.f.w; }
            let fx = round(rx + cx);
            let fy = round(ry + cy);
            if (fx < f32(x0) || fy < f32(y0) || fx >= f32(x0 + sw) || fy >= f32(y0 + sh)) { return ud; }
            s = vec2<i32>(i32(fx), i32(fy));
        }
        let tw = u.tile.x; let th = u.tile.y;
        var t = s - vec2<i32>(x0, y0);
        if (tw != sw || th != sh) {
            let uu = f32(s.x - x0) / f32(sw);
            let vv = f32(s.y - y0) / f32(sh);
            t = vec2<i32>(min(i32(uu * f32(tw)), tw - 1), min(i32(vv * f32(th)), th - 1));
        }
        col = textureLoad(src, t, 0);
    }
    let cover = clamp(u.f.x * col.a * clip, 0.0, 1.0);
    if (u.tile.z == 1) {
        return vec4<f32>(col.rgb * cover, ud.a);
    }
    return vec4<f32>(mixc(ud.rgb, col.rgb, cover, u.tile.w), ud.a);
}
"#;

const UNIFORM_SIZE: u64 = 112;

/// The layer-stack compositor. One per painter, on the models' device.
pub struct GpuCompose {
    cx: GpuCx,
    /// Bound as the cover map of a tile with no contour.
    no_cover: crate::gpu_cover::CoverRef,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    cap: u64,
    stride: u64,
}

fn blend_id(b: Blend) -> i32 {
    match b {
        Blend::Normal => 0,
        Blend::Add => 1,
        Blend::Multiply => 2,
        Blend::Screen => 3,
    }
}

impl GpuCompose {
    pub fn new(cx: &GpuCx) -> GpuCompose {
        let device = &cx.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gpu_compose"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gpu_compose"),
            entries: &[
                tex(0),
                tex(1),
                tex(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpu_compose"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gpu_compose"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: VIEW_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let stride = (device.limits().min_uniform_buffer_offset_alignment as u64).max(UNIFORM_SIZE);
        let cap = 16;
        let uniforms = Self::uniform_buffer(device, stride * cap);
        GpuCompose { cx: cx.clone(), no_cover: crate::gpu_cover::CoverTex::new(cx, &[255], 1, 1), pipeline, layout, uniforms, cap, stride }
    }

    fn uniform_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_compose uniforms"),
            size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// The device frame: an opaque black `w x h` canvas with `tiles` composed
    /// back to front, bit-for-bit the semantics of
    /// [`crate::compose::compose`] (overlap blends, sectors replace; tile
    /// alpha, opacity and `giro` honoured). CPU tiles are uploaded; texture
    /// tiles are read in place.
    pub fn compose(&mut self, w: u32, h: u32, mode: Composition, tiles: &[GpuTile]) -> GpuFrame {
        let cx = self.cx.clone();
        let out = cx.alloc(w, h);
        let scratch = cx.alloc(w, h);
        if (tiles.len() as u64) > self.cap {
            self.cap = (tiles.len() as u64).next_power_of_two();
            self.uniforms = Self::uniform_buffer(&cx.device, self.stride * self.cap);
        }
        let mut enc = cx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("gpu_compose") });
        drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gpu_compose clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: out.view(),
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }));
        // Uploaded CPU tiles stay alive until the commands are submitted.
        let mut keep: Vec<GpuFrame> = Vec::new();
        for (i, tile) in tiles.iter().enumerate() {
            if mode == Composition::Overlap && tile.opacity <= 0.0 {
                continue;
            }
            let src: GpuFrame = match &tile.source {
                TileSource::Texture(f) => (*f).clone(),
                TileSource::Cpu { pixels, w, h } => {
                    if *w == 0 || *h == 0 || pixels.len() < (*w as usize) * (*h as usize) * 4 {
                        continue;
                    }
                    cx.upload(pixels, *w, *h)
                }
            };
            let (x0, y0, sw, sh) = tile.sector.pixels(w, h);
            let (ex, ey) = ((x0 + sw).min(w), (y0 + sh).min(h));
            if x0 >= ex || y0 >= ey {
                continue;
            }
            let (sin, cos) = tile.turn.sin_cos();
            let fit = tile.turn != 0.0 && tile.giro_mode == ClipMode::Fit;
            let k = if fit { crate::compose::fit_scale(sw, sh, cos.abs(), sin.abs()) } else { 1.0 };
            let mut u = Vec::with_capacity(UNIFORM_SIZE as usize);
            for v in [x0 as i32, y0 as i32, sw as i32, sh as i32, src.width() as i32, src.height() as i32] {
                u.extend(v.to_le_bytes());
            }
            u.extend((if mode == Composition::Sectors { 1i32 } else { 0 }).to_le_bytes());
            u.extend(blend_id(tile.blend).to_le_bytes());
            let opacity = tile.opacity.clamp(0.0, 1.0);
            for v in [opacity, cos, sin, k] {
                u.extend(v.to_le_bytes());
            }
            let warp_mode = tile.warp.map_or(0, |w| if w.nearest { 2 } else { 1 });
            for v in [(tile.turn != 0.0) as i32, fit as i32, tile.cover.is_some() as i32, warp_mode] {
                u.extend(v.to_le_bytes());
            }
            let m = tile.warp.map_or([0.0f32; 9], |w| w.m);
            for row in 0..3 {
                for v in [m[row * 3], m[row * 3 + 1], m[row * 3 + 2], 0.0] {
                    u.extend(v.to_le_bytes());
                }
            }
            let offset = i as u64 * self.stride;
            cx.queue.write_buffer(&self.uniforms, offset, &u);
            let size = wgpu::Extent3d { width: ex - x0, height: ey - y0, depth_or_array_layers: 1 };
            let origin = wgpu::Origin3d { x: x0, y: y0, z: 0 };
            enc.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo { texture: out.texture(), mip_level: 0, origin, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyTextureInfo { texture: scratch.texture(), mip_level: 0, origin, aspect: wgpu::TextureAspect::All },
                size,
            );
            let bind = cx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src.view()) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(scratch.view()) },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(tile.cover.unwrap_or(&self.no_cover).view()),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.uniforms,
                            offset: 0,
                            size: wgpu::BufferSize::new(UNIFORM_SIZE),
                        }),
                    },
                ],
            });
            let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gpu_compose layer"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: out.view(),
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            p.set_pipeline(&self.pipeline);
            p.set_bind_group(0, &bind, &[offset as u32]);
            p.set_scissor_rect(x0, y0, ex - x0, ey - y0);
            p.draw(0..3, 0..1);
            drop(p);
            keep.push(src);
        }
        cx.queue.submit(Some(enc.finish()));
        drop(keep);
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::compose::{compose, Tile};
    use crate::layers::Sector;

    fn cx() -> GpuCx {
        let g = metal_render::gpu::Gpu::new_headless();
        GpuCx::new(g.device, g.queue)
    }

    /// A smooth, asymmetric picture so a misplaced pixel shows.
    pub(crate) fn picture(w: u32, h: u32, seed: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                [
                    ((x * 255 / w + seed * 37) % 256) as u8,
                    ((y * 255 / h + seed * 91) % 256) as u8,
                    ((x + y + seed * 13) % 256) as u8,
                    if (x / 8 + y / 8) % 5 == 0 { 140 } else { 255 },
                ]
            })
            .collect()
    }

    fn stats(a: &[u8], b: &[u8]) -> (f64, u8) {
        let sum: u64 = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u64).sum();
        (sum as f64 / a.len() as f64, a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0))
    }

    struct Spec {
        sector: Sector,
        tw: u32,
        th: u32,
        opacity: f32,
        blend: Blend,
        turn: f32,
        giro_mode: ClipMode,
    }

    fn spec(x: f32, y: f32, w: f32, h: f32, tw: u32, th: u32, opacity: f32, blend: Blend) -> Spec {
        Spec { sector: Sector { x, y, w, h }, tw, th, opacity, blend, turn: 0.0, giro_mode: ClipMode::default() }
    }

    fn both(specs: &[Spec], mode: Composition) -> (Vec<u8>, Vec<u8>) {
        let cx = cx();
        let (w, h) = (160u32, 90u32);
        let pics: Vec<Vec<u8>> = specs.iter().enumerate().map(|(i, s)| picture(s.tw, s.th, i as u32)).collect();
        let cpu_tiles: Vec<Tile> = specs
            .iter()
            .zip(&pics)
            .map(|(s, p)| Tile {
                pixels: p,
                w: s.tw,
                h: s.th,
                sector: s.sector,
                opacity: s.opacity,
                blend: s.blend,
                turn: s.turn,
                giro_mode: s.giro_mode,
                clip: None,
                masks: &[],
            })
            .collect();
        let want = compose(w, h, mode, &cpu_tiles);
        // Alternate texture and CPU sources so both handoffs are covered.
        let frames: Vec<GpuFrame> = specs.iter().zip(&pics).map(|(s, p)| cx.upload(p, s.tw, s.th)).collect();
        let gtiles: Vec<GpuTile> = specs
            .iter()
            .zip(pics.iter().zip(&frames))
            .enumerate()
            .map(|(i, (s, (p, f)))| GpuTile {
                source: if i % 2 == 0 {
                    TileSource::Texture(f)
                } else {
                    TileSource::Cpu { pixels: p, w: s.tw, h: s.th }
                },
                sector: s.sector,
                opacity: s.opacity,
                blend: s.blend,
                turn: s.turn,
                giro_mode: s.giro_mode,
                cover: None,
                warp: None,
            })
            .collect();
        let mut gc = GpuCompose::new(&cx);
        let out = gc.compose(w, h, mode, &gtiles);
        (want, cx.readback(&out))
    }

    #[test]
    fn full_frame_blends_match_the_cpu_compose() {
        let specs = [
            spec(0.0, 0.0, 1.0, 1.0, 160, 90, 1.0, Blend::Normal),
            spec(0.0, 0.0, 1.0, 1.0, 160, 90, 0.7, Blend::Add),
            spec(0.0, 0.0, 1.0, 1.0, 160, 90, 0.5, Blend::Multiply),
            spec(0.0, 0.0, 1.0, 1.0, 160, 90, 0.9, Blend::Screen),
            spec(0.0, 0.0, 1.0, 1.0, 160, 90, 0.0, Blend::Add),
        ];
        let (cpu, gpu) = both(&specs, Composition::Overlap);
        let (mean, max) = stats(&cpu, &gpu);
        assert!(mean < 0.6 && max <= 3, "mean {mean:.3} max {max}");
        assert_ne!(cpu, vec![0u8; cpu.len()]);
    }

    #[test]
    fn sectors_scaled_tiles_and_both_modes_match() {
        let specs = [
            spec(0.0, 0.0, 0.5, 0.5, 80, 45, 1.0, Blend::Normal),
            // a budget-shrunk layer: the tile is smaller than its sector
            spec(0.25, 0.3, 0.6, 0.6, 50, 30, 0.8, Blend::Normal),
            spec(0.6, 0.1, 0.4, 0.8, 64, 72, 0.6, Blend::Screen),
        ];
        for mode in [Composition::Overlap, Composition::Sectors] {
            let (cpu, gpu) = both(&specs, mode);
            let (mean, max) = stats(&cpu, &gpu);
            assert!(mean < 0.6 && max <= 3, "{mode:?}: mean {mean:.3} max {max}");
        }
    }

    #[test]
    fn giro_matches_in_both_clip_modes() {
        for gm in [ClipMode::Crop, ClipMode::Fit] {
            let mut a = spec(0.1, 0.1, 0.7, 0.8, 112, 72, 1.0, Blend::Normal);
            a.turn = 0.6;
            a.giro_mode = gm;
            let mut b = spec(0.0, 0.0, 1.0, 1.0, 90, 90, 0.5, Blend::Add);
            b.turn = std::f32::consts::FRAC_PI_2;
            b.giro_mode = gm;
            let (cpu, gpu) = both(&[a, b], Composition::Overlap);
            let (mean, _) = stats(&cpu, &gpu);
            // rotated edges may differ by a texel where round() ties differ
            assert!(mean < 3.0, "{gm:?}: mean {mean:.3}");
        }
    }

    #[test]
    fn an_empty_stack_is_opaque_black() {
        let cx = cx();
        let mut gc = GpuCompose::new(&cx);
        let out = gc.compose(32, 16, Composition::Overlap, &[]);
        let px = cx.readback(&out);
        assert!(px.chunks_exact(4).all(|p| p == [0, 0, 0, 255]));
    }

    #[test]
    fn async_readback_returns_the_frame_without_blocking_the_caller() {
        let cx = cx();
        let pic = picture(40, 30, 3);
        let f = cx.upload(&pic, 40, 30);
        let mut rb = AsyncReadback::new(&cx);
        assert!(rb.submit(&cx, &f));
        assert!(rb.submit(&cx, &f));
        assert!(!rb.submit(&cx, &f), "both slots in flight: skip, never wait");
        let mut got = None;
        for _ in 0..2000 {
            if let Some(g) = rb.poll(&cx) {
                got = Some(g);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let (px, w, h) = got.expect("a frame completes");
        assert_eq!((w, h), (40, 30));
        assert_eq!(px, pic);
        assert!(rb.submit(&cx, &f), "a slot freed");
    }

    /// `cargo test -p metal-perform --features gpu --release --lib gpu_compose::tests::measure -- --ignored --nocapture --test-threads=1`
    ///
    /// Three full-frame model layers (Normal, Add 0.6, Screen 0.8), model draw
    /// included, per frame: the old hybrid (read each model back, CPU compose)
    /// against the centralized path (textures, one GPU compose), with the
    /// readback a CPU sink would add shown separately.
    #[test]
    #[ignore]
    fn measure() {
        use crate::backend::Backend;
        use crate::layers::{Blend, ClipMode, Sector};
        use crate::paint::{Gpu, Paint};
        use std::time::Instant;
        let mut p = Gpu::with_backend(metal_render::headless::Renderer::new(), Backend::Gpu);
        let rows: Vec<Vec<f32>> = (0..3).map(|i| p.schemas()[i].defaults()).collect();
        let blends = [(Blend::Normal, 1.0f32), (Blend::Add, 0.6), (Blend::Screen, 0.8)];
        let cx = p.gpu_cx().expect("gpu backend");
        for (w, h) in [(1920u32, 1080u32), (3840, 2160)] {
            let n = 20u32;
            let wait = |cx: &GpuCx| {
                let _ = cx.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
            };
            // warm
            for _ in 0..3 {
                for (i, r) in rows.iter().enumerate() {
                    let _ = p.solo_chain(i, r, 1.0, w, h, false, &[]);
                }
            }
            // A. old hybrid: models read back, CPU compose
            let t = Instant::now();
            let (mut models_ms, mut compose_ms) = (0.0, 0.0);
            for _ in 0..n {
                let t0 = Instant::now();
                let px: Vec<_> = rows.iter().enumerate().map(|(i, r)| p.solo_chain(i, r, 1.0, w, h, false, &[]).0).collect();
                let t1 = Instant::now();
                let tiles: Vec<Tile> = px
                    .iter()
                    .zip(blends)
                    .map(|(px, (blend, opacity))| Tile {
                        pixels: px, w, h, sector: Sector::default(), opacity, blend, turn: 0.0,
                        giro_mode: ClipMode::default(), clip: None, masks: &[],
                    })
                    .collect();
                let _ = compose(w, h, Composition::Overlap, &tiles);
                models_ms += (t1 - t0).as_secs_f64();
                compose_ms += t1.elapsed().as_secs_f64();
            }
            let hybrid = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
            // B. centralized: textures, one GPU compose, no readback
            let mut run = |readback: bool| -> f64 {
                let t = Instant::now();
                for _ in 0..n {
                    let frames: Vec<GpuFrame> = rows
                        .iter()
                        .enumerate()
                        .map(|(i, r)| p.solo_texture(i, r, 1.0, w, h, false, &[]).unwrap())
                        .collect();
                    let tiles: Vec<GpuTile> = frames
                        .iter()
                        .zip(blends)
                        .map(|(f, (blend, opacity))| GpuTile {
                            source: TileSource::Texture(f), sector: Sector::default(), opacity, blend, turn: 0.0,
                            giro_mode: ClipMode::default(), cover: None, warp: None,
                        })
                        .collect();
                    let out = p.compose_gpu(w, h, Composition::Overlap, &tiles).unwrap();
                    if readback {
                        let _ = p.readback(&out);
                    } else {
                        wait(&cx);
                    }
                }
                t.elapsed().as_secs_f64() * 1000.0 / n as f64
            };
            let _ = run(false);
            let gpu = run(false);
            let gpu_rb = run(true);
            println!(
                "{w}x{h}: hybrid {hybrid:.1} ms (models+readback {:.1}, CPU compose {:.1}); gpu {gpu:.1} ms (to texture, GPU work waited), gpu + blocking readback {gpu_rb:.1} ms",
                models_ms * 1000.0 / n as f64,
                compose_ms * 1000.0 / n as f64
            );
        }
    }
}
