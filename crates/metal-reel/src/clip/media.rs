//! 2026-10-09: clips on Android without ffmpeg. `AMediaExtractor` demuxes the
//! first video track (MP4/MOV, H.264 or HEVC, whatever the device decodes),
//! `AMediaCodec` decodes it, and [`super::Yuv420::to_rgba`] turns each picture
//! into RGBA at the requested width.
//!
//! The decoder renders into an `AImageReader` surface (`YUV_420_888`), and each
//! picture is read back as an image with its own plane strides and crop. That
//! is the route every decoder supports: the emulator's `c2.goldfish.h264`
//! decoder emits graphic buffers that buffer-mode output never delivers, and
//! flexible (`0x7F420888`) output needs no layout guess. (`AMediaCodec_getOutputImage`
//! is not in the NDK.)
//!
//! Pictures are paired with their presentation times in order: each rendered
//! output queues its time, and each image read back takes the oldest. Images are
//! drained after every output, so the reader never holds more than a few and
//! the codec's surface buffers never run out. An image that does not arrive in
//! time drops its frame rather than stalling the decode.
//!
//! Seeking is `AMediaExtractor_seekTo` to the sync sample before the target,
//! a codec flush, then decoding forward until the frame at the target. Looping
//! is the caller's (`super::looped`). [`MediaClip::sample_all`] decodes the
//! whole clip once, front to back, for a length the container may not give.

use std::collections::VecDeque;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::ptr::NonNull;
use std::time::Duration;

use ndk::hardware_buffer::HardwareBufferUsage;
use ndk::media::image_reader::{AcquireResult, Image, ImageFormat, ImageReader};
use ndk::media::media_codec::{DequeuedInputBufferResult, DequeuedOutputBufferInfoResult, MediaCodec, MediaCodecDirection};
use ndk::media::media_format::MediaFormat;
use ndk_sys as ffi;

use super::{Chroma, ClipDecoder, Matrix, Rgba, Yuv420};

/// The software AVC decoder every Android device carries.
const SOFTWARE_AVC: &str = "c2.android.avc.decoder";
/// Polls (`TIMEOUT` each) with no picture out before the decoder counts as
/// stalled: about five seconds.
const STALL_POLLS: u32 = 500;
/// The error text of a stalled decoder: the caller retries on the software one.
pub const STALLED: &str = "decoder stalled";

/// `AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM`.
const FLAG_EOS: u32 = 4;
const SEEK_PREVIOUS_SYNC: ffi::SeekMode = ffi::SeekMode(0);
/// A forward gap longer than this is a seek, not playback.
const SEEK_JUMP_US: i64 = 2_000_000;
const TIMEOUT: Duration = Duration::from_millis(10);
/// Images the reader keeps for the decoder: a few, drained after every output.
const MAX_IMAGES: i32 = 16;
/// The consumer usage the decoder asks of its surface: CPU read, GPU sampled,
/// composer overlay (`0x903`, from the codec's own log). A reader without it
/// gets no pictures from the emulator's decoder.
const CODEC_USAGE: u64 = 0x903;
/// How long to wait for a rendered picture to reach the reader before its
/// frame is dropped.
const IMAGE_WAIT: Duration = Duration::from_millis(2);
const IMAGE_TRIES: u32 = 500;

/// Plane `i` of an image: its bytes, row stride and pixel stride.
fn image_plane(img: &Image, i: i32) -> Result<(&[u8], usize, usize), String> {
    let data = img.plane_data(i).map_err(|e| format!("plane {i}: {e:?}"))?;
    let rs = img.plane_row_stride(i).map_err(|e| format!("row stride {i}: {e:?}"))?;
    let ps = img.plane_pixel_stride(i).map_err(|e| format!("pixel stride {i}: {e:?}"))?;
    Ok((data, rs.max(0) as usize, ps.max(1) as usize))
}

/// One clip, open on its first video track.
pub struct MediaClip {
    extractor: NonNull<ffi::AMediaExtractor>,
    /// The clip's file, open for the extractor's fd (the media server cannot
    /// open an app's private files by path).
    _file: std::fs::File,
    // Declared before `reader`: the codec renders into the reader's window,
    // so it must stop first.
    codec: MediaCodec,
    reader: ImageReader,
    /// Visible size of the picture, from the track until the first image.
    size: (u32, u32),
    duration_us: i64,
    out_w: usize,
    /// Presentation times of pictures rendered into the reader, oldest first,
    /// not yet read back.
    pending: VecDeque<i64>,
    /// Pictures read back, oldest first.
    ready: VecDeque<(i64, Rgba)>,
    /// The frame the last `frame_at` returned, with its time.
    shown: Option<(i64, Rgba)>,
    /// A frame decoded past the target, kept for the next call.
    carry: Option<(i64, Rgba)>,
    eos_in: bool,
    eos_out: bool,
    /// Counts for the decode log: samples queued, outputs seen, idle polls.
    stats: [u32; 3],
}

// The extractor and codec are used from one thread at a time: the playout
// keeps each clip behind a `Mutex`.
unsafe impl Send for MediaClip {}

impl Drop for MediaClip {
    fn drop(&mut self) {
        let _ = self.codec.stop();
        unsafe { ffi::AMediaExtractor_delete(self.extractor.as_ptr()) };
    }
}

impl MediaClip {
    /// Opens `path`'s first video track and starts a decoder for it. Frames
    /// come out `out_w` pixels wide (height by aspect).
    pub fn open(path: &Path, out_w: u32) -> Result<MediaClip, String> {
        Self::open_with(path, out_w, false)
    }

    /// As [`open`](Self::open); `software` picks the platform's software AVC
    /// decoder instead of the default (hardware) one.
    pub fn open_with(path: &Path, out_w: u32, software: bool) -> Result<MediaClip, String> {
        let extractor = NonNull::new(unsafe { ffi::AMediaExtractor_new() }).ok_or("AMediaExtractor_new failed")?;
        match Self::open_on(extractor, path, out_w, software) {
            Ok(clip) => Ok(clip),
            Err(e) => {
                unsafe { ffi::AMediaExtractor_delete(extractor.as_ptr()) };
                Err(format!("{}: {e}", path.display()))
            }
        }
    }

    fn open_on(extractor: NonNull<ffi::AMediaExtractor>, path: &Path, out_w: u32, software: bool) -> Result<MediaClip, String> {
        let ex = extractor.as_ptr();
        // The file is opened here and handed to the extractor as an fd.
        let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
        let len = file.metadata().map_err(|e| format!("stat: {e}"))?.len();
        let st = unsafe { ffi::AMediaExtractor_setDataSourceFd(ex, file.as_raw_fd(), 0, len as i64) };
        if st.0 != 0 {
            return Err(format!("setDataSourceFd: status {}", st.0));
        }
        let tracks = unsafe { ffi::AMediaExtractor_getTrackCount(ex) };
        let mut chosen = None;
        for i in 0..tracks {
            let Some(raw) = NonNull::new(unsafe { ffi::AMediaExtractor_getTrackFormat(ex, i) }) else { continue };
            let mut format = unsafe { MediaFormat::from_ptr(raw) };
            let mime = format.str("mime").unwrap_or("").to_string();
            if mime.starts_with("video/") {
                let duration_us = format.i64("durationUs").unwrap_or(0);
                let w = format.i32("width").unwrap_or(0).max(0) as u32;
                let h = format.i32("height").unwrap_or(0).max(0) as u32;
                chosen = Some((i, mime, duration_us, (w, h), format));
                break;
            }
        }
        let Some((index, mime, duration_us, size, format)) = chosen else {
            return Err("no video track".into());
        };
        let st = unsafe { ffi::AMediaExtractor_selectTrack(ex, index) };
        if st.0 != 0 {
            return Err(format!("selectTrack: status {}", st.0));
        }
        let (w, h) = size;
        if w == 0 || h == 0 {
            return Err("the video track has no size".into());
        }
        let codec = if software {
            MediaCodec::from_codec_name(SOFTWARE_AVC).ok_or_else(|| format!("no {SOFTWARE_AVC}"))?
        } else {
            MediaCodec::from_decoder_type(&mime).ok_or_else(|| format!("no decoder for {mime}"))?
        };
        let usage = HardwareBufferUsage::from_bits_retain(CODEC_USAGE);
        let reader = ImageReader::new_with_usage(w as i32, h as i32, ImageFormat::YUV_420_888, usage, MAX_IMAGES)
            .map_err(|e| format!("image reader: {e:?}"))?;
        let window = reader.window().map_err(|e| format!("image reader window: {e:?}"))?;
        codec.configure(&format, Some(&window), MediaCodecDirection::Decoder).map_err(|e| format!("configure: {e:?}"))?;
        codec.start().map_err(|e| format!("start: {e:?}"))?;
        drop(window);
        drop(format);
        // The extractor now belongs to the clip; `Drop` deletes it.
        Ok(MediaClip {
            extractor,
            _file: file,
            codec,
            reader,
            size,
            duration_us,
            out_w: out_w.max(1) as usize,
            pending: VecDeque::new(),
            ready: VecDeque::new(),
            shown: None,
            carry: None,
            eos_in: false,
            eos_out: false,
            stats: [0; 3],
        })
    }

    /// Makes the next `frame_at` cheap around `t_us`: flush the codec and seek
    /// the extractor to the sync sample before it.
    fn seek_us(&mut self, t_us: i64) -> Result<(), String> {
        self.codec.flush().map_err(|e| format!("flush: {e:?}"))?;
        let st = unsafe { ffi::AMediaExtractor_seekTo(self.extractor.as_ptr(), t_us, SEEK_PREVIOUS_SYNC) };
        if st.0 != 0 {
            return Err(format!("seekTo {t_us}us: status {}", st.0));
        }
        self.pending.clear();
        self.ready.clear();
        self.shown = None;
        self.carry = None;
        self.eos_in = false;
        self.eos_out = false;
        Ok(())
    }

    /// Queues one compressed sample, if the codec has a free input buffer.
    fn feed(&mut self) -> Result<(), String> {
        if self.eos_in {
            return Ok(());
        }
        let DequeuedInputBufferResult::Buffer(mut buf) =
            self.codec.dequeue_input_buffer(TIMEOUT).map_err(|e| format!("dequeue input: {e:?}"))?
        else {
            return Ok(());
        };
        let dst = buf.buffer_mut();
        let n = unsafe { ffi::AMediaExtractor_readSampleData(self.extractor.as_ptr(), dst.as_mut_ptr().cast(), dst.len()) };
        if n < 0 {
            self.eos_in = true;
            self.codec.queue_input_buffer(buf, 0, 0, 0, FLAG_EOS).map_err(|e| format!("queue EOS: {e:?}"))?;
            return Ok(());
        }
        let pts = unsafe { ffi::AMediaExtractor_getSampleTime(self.extractor.as_ptr()) }.max(0) as u64;
        self.codec.queue_input_buffer(buf, 0, n as usize, pts, 0).map_err(|e| format!("queue input: {e:?}"))?;
        self.stats[0] += 1;
        unsafe { ffi::AMediaExtractor_advance(self.extractor.as_ptr()) };
        Ok(())
    }

    /// The next decoded picture in presentation order, or `None` at the end.
    /// Each output the codec makes that carries a picture is rendered into the
    /// reader; the pictures are read back as they arrive (see the module doc).
    fn next_decoded(&mut self) -> Result<Option<(i64, Rgba)>, String> {
        if let Some(c) = self.carry.take() {
            return Ok(Some(c));
        }
        loop {
            if let Some(frame) = self.ready.pop_front() {
                return Ok(Some(frame));
            }
            if self.eos_out {
                if self.pending.is_empty() {
                    return Ok(None);
                }
                self.await_picture()?;
                continue;
            }
            self.feed()?;
            let result = self.codec.dequeue_output_buffer(TIMEOUT).map_err(|e| format!("dequeue output: {e:?}"))?;
            if matches!(result, DequeuedOutputBufferInfoResult::Buffer(_)) {
                self.stats[1] += 1;
            } else {
                self.stats[2] += 1;
                // Some hardware decoders (the emulator's goldfish one) take a
                // few samples and never give a picture back. Several seconds of
                // polling with no picture is that, not a slow start.
                if self.stats[1] == 0 && self.stats[2] >= STALL_POLLS {
                    return Err(format!("{STALLED}: {} samples in, no picture after {} polls", self.stats[0], self.stats[2]));
                }
            }
            match result {
                DequeuedOutputBufferInfoResult::Buffer(out) => {
                    let pts = out.info().presentation_time_us();
                    let eos = out.info().flags() & FLAG_EOS != 0;
                    // Surface output reports size 0 for every frame it renders:
                    // the picture is in the reader, not in the buffer. So every
                    // output but the end-of-stream marker is rendered.
                    let has_picture = !eos || out.info().size() > 0;
                    self.codec.release_output_buffer(out, has_picture).map_err(|e| format!("release: {e:?}"))?;
                    if has_picture {
                        self.pending.push_back(pts);
                    }
                    if eos {
                        self.eos_out = true;
                    }
                    self.drain()?;
                }
                DequeuedOutputBufferInfoResult::OutputFormatChanged
                | DequeuedOutputBufferInfoResult::OutputBuffersChanged
                | DequeuedOutputBufferInfoResult::TryAgainLater => {
                    // Nothing new from the codec: the rendered pictures may
                    // still be on their way to the reader.
                    if !self.pending.is_empty() {
                        self.await_picture()?;
                    }
                }
            }
        }
    }

    /// Reads back every image the reader holds now, without waiting.
    fn drain(&mut self) -> Result<(), String> {
        while let AcquireResult::Image(img) = self.acquire()? {
            self.take(&img)?;
        }
        Ok(())
    }

    /// Waits briefly for the oldest rendered picture. One that does not come
    /// is dropped with its frame, so the decode cannot stall on it.
    fn await_picture(&mut self) -> Result<(), String> {
        for _ in 0..IMAGE_TRIES {
            if let AcquireResult::Image(img) = self.acquire()? {
                self.take(&img)?;
                return self.drain();
            }
            std::thread::sleep(IMAGE_WAIT);
        }
        self.pending.pop_front();
        Ok(())
    }

    fn acquire(&self) -> Result<AcquireResult<Image>, String> {
        self.reader.acquire_next_image().map_err(|e| format!("acquire image: {e:?}"))
    }

    /// Converts one picture and queues it under the oldest pending time.
    fn take(&mut self, img: &Image) -> Result<(), String> {
        let pts = self.pending.pop_front().unwrap_or(0);
        let rgba = self.convert(img)?;
        self.ready.push_back((pts, rgba));
        Ok(())
    }

    /// RGBA of one image at the clip's preview width. The image's crop is the
    /// visible picture; its planes carry their own strides.
    fn convert(&mut self, img: &Image) -> Result<Rgba, String> {
        let crop = img.crop_rect().map_err(|e| format!("crop: {e:?}"))?;
        let (iw, ih) = (img.width().unwrap_or(0), img.height().unwrap_or(0));
        let (cl, ct) = (crop.left.max(0) as usize, crop.top.max(0) as usize);
        let (cw, chh): (usize, usize) = ((crop.right - crop.left).max(0) as usize, (crop.bottom - crop.top).max(0) as usize);
        let (w, h): (usize, usize) = match (cw, chh) {
            (0, _) | (_, 0) => (iw.max(1) as usize, ih.max(1) as usize),
            (w, h) => (w, h),
        };
        self.size = (w as u32, h as u32);
        let (ew, eh) = ((w + 1) & !1, (h + 1) & !1);
        let c_stride = ew / 2;
        let (u0, v0) = (ew * eh, ew * eh + c_stride * (eh / 2));
        let mut out = vec![0u8; v0 + c_stride * (eh / 2)];
        let (y, ys, yp) = image_plane(img, 0)?;
        let (u, us, up) = image_plane(img, 1)?;
        let (v, vs, vp) = image_plane(img, 2)?;
        let at = |buf: &[u8], i: usize| buf.get(i).copied().ok_or_else(|| format!("image plane short at {i}"));
        for r in 0..h {
            for c in 0..w {
                out[r * ew + c] = at(y, (ct + r) * ys + (cl + c) * yp)?;
            }
        }
        for cr in 0..h.div_ceil(2) {
            let sr = (ct + 2 * cr) / 2;
            for cc in 0..w.div_ceil(2) {
                let sc = (cl + 2 * cc) / 2;
                out[u0 + cr * c_stride + cc] = at(u, sr * us + sc * up)?;
                out[v0 + cr * c_stride + cc] = at(v, sr * vs + sc * vp)?;
            }
        }
        let layout = Yuv420 {
            chroma: Chroma::Planar,
            width: w,
            height: h,
            stride: ew,
            slice_height: eh,
            crop_x: 0,
            crop_y: 0,
            matrix: Matrix::for_height(h),
        };
        layout.to_rgba(&out, self.out_w)
    }

    /// Decodes the whole clip once, front to back, and hands `keep(k, frame)`
    /// the picture of each sample slot `k`: the last frame at or before
    /// `k / fps` seconds (the first one when a slot comes before it). With
    /// `slots` (the length from the container) that is exactly that many
    /// slots, the last frame repeated past the end of the decode; with `None`
    /// the slots run to the last decoded frame. Returns the slots kept.
    pub fn sample_all(&mut self, fps: f64, slots: Option<usize>, mut keep: impl FnMut(usize, &Rgba) -> Result<(), String>) -> Result<usize, String> {
        self.seek_us(0)?;
        let limit = slots.unwrap_or(usize::MAX);
        let slot_us = |k: usize| (k as f64 / fps * 1e6) as i64;
        let mut k = 0usize;
        let mut cur: Option<(i64, Rgba)> = None;
        while let Some((pts, rgba)) = self.next_decoded()? {
            if let Some((_, prev)) = &cur {
                while k < limit && slot_us(k) < pts {
                    keep(k, prev)?;
                    k += 1;
                }
            }
            cur = Some((pts, rgba));
        }
        if let Some((last, prev)) = &cur {
            while k < limit && (slots.is_some() || slot_us(k) <= *last) {
                keep(k, prev)?;
                k += 1;
            }
        }
        Ok(k)
    }
}

impl ClipDecoder for MediaClip {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn duration(&self) -> f64 {
        self.duration_us.max(0) as f64 / 1e6
    }

    fn seek(&mut self, t: f64) -> Result<(), String> {
        self.seek_us((t.max(0.0) * 1e6) as i64)
    }

    fn frame_at(&mut self, t: f64) -> Result<Rgba, String> {
        let target = (t.max(0.0) * 1e6) as i64;
        let behind = match &self.shown {
            None => true,
            Some((pts, _)) => target < *pts || target - *pts > SEEK_JUMP_US,
        };
        if behind {
            self.seek_us(target)?;
        }
        loop {
            let Some((pts, rgba)) = self.next_decoded()? else {
                return self.shown.as_ref().map(|(_, r)| r.clone()).ok_or_else(|| "no frames decoded".into());
            };
            if pts > target {
                if let Some((_, prev)) = &self.shown {
                    let out = prev.clone();
                    self.carry = Some((pts, rgba));
                    return Ok(out);
                }
            }
            self.shown = Some((pts, rgba));
        }
    }
}
