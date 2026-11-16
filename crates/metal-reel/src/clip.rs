//! 2026-10-09: one interface for a clip's pictures, whatever decodes them.
//!
//! `metal-dash`'s sequence editor plays video clips by asking for the frame at a
//! time. Three things can answer that:
//!
//! ```text
//!   HAP proxy (pure Rust, every platform)  ──► HapClip
//!   ffmpeg (desktop, MJPEG pipe)           ──► playout's own pipe (unchanged)
//!   AMediaExtractor + AMediaCodec (Android) ──► media::MediaClip   (cfg android)
//! ```
//!
//! The first and third implement [`ClipDecoder`]. Callers hold a
//! `Box<dyn ClipDecoder>` and never learn which one answered. The pure parts
//! live here so they run on a host: the YUV→RGBA conversion a hardware decoder
//! needs (planar I420 and semi-planar NV12, with the row padding and crop an
//! Android output buffer carries), the loop arithmetic, and the HAP adapter.

use crate::hap::Hap;

/// A decoded picture, RGBA8, rows top to bottom, no padding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// A clip, read by time.
pub trait ClipDecoder {
    /// Visible size in pixels.
    fn size(&self) -> (u32, u32);

    /// Length in seconds; `0.0` when the container does not say.
    fn duration(&self) -> f64;

    /// Make the next [`frame_at`](Self::frame_at) cheap around `t` (a hint: a
    /// decoder that reads by index does nothing here).
    fn seek(&mut self, t: f64) -> Result<(), String>;

    /// The picture at `t` seconds: the last frame whose time is `<= t`, or the
    /// first one after it when the clip has nothing earlier. `t` is not
    /// wrapped; use [`looped`] for a loop.
    fn frame_at(&mut self, t: f64) -> Result<Rgba, String>;
}

/// `t` folded into `[0, duration)` for a clip that loops. A clip of unknown
/// length is not wrapped, only clamped to zero.
pub fn looped(t: f64, duration: f64) -> f64 {
    if duration > 0.0 && t.is_finite() {
        t.rem_euclid(duration)
    } else {
        t.max(0.0)
    }
}

/// Which colour matrix the YUV was made with. Phone and camera HD is BT.709;
/// SD is BT.601. Limited range in both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matrix {
    Bt601,
    Bt709,
}

impl Matrix {
    /// The matrix an untagged clip of this height is almost always made with.
    pub fn for_height(height: usize) -> Matrix {
        if height >= 720 {
            Matrix::Bt709
        } else {
            Matrix::Bt601
        }
    }
}

/// How the chroma samples sit in the buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chroma {
    /// I420 (`COLOR_FormatYUV420Planar`): all U rows, then all V rows, each at
    /// half the luma stride.
    Planar,
    /// NV12 (`COLOR_FormatYUV420SemiPlanar`): one plane of interleaved UV at the
    /// luma stride.
    SemiPlanar,
}

/// A 4:2:0 buffer as a codec hands it over: the strides and the padding that
/// come with them, and the visible rectangle inside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Yuv420 {
    pub chroma: Chroma,
    /// Visible size.
    pub width: usize,
    pub height: usize,
    /// Bytes per luma row (may exceed `width`).
    pub stride: usize,
    /// Luma rows in the buffer, padding included (`slice-height`).
    pub slice_height: usize,
    /// Top-left of the visible rectangle inside the buffer (crop).
    pub crop_x: usize,
    pub crop_y: usize,
    pub matrix: Matrix,
}

impl Yuv420 {
    /// Converts to RGBA at `out_w` pixels wide (nearest sample, height scaled
    /// to keep the aspect). A preview does not need the decoder's full size,
    /// and converting 1080p 8 times a second on a phone does not fit a frame.
    pub fn to_rgba(&self, buf: &[u8], out_w: usize) -> Result<Rgba, String> {
        let (w, h) = (self.width, self.height);
        if w == 0 || h == 0 {
            return Err("yuv: empty frame".into());
        }
        if self.stride < self.crop_x + w || self.slice_height < self.crop_y + h {
            return Err(format!("yuv: crop {}x{}+{}+{} outside a {}x{} buffer", w, h, self.crop_x, self.crop_y, self.stride, self.slice_height));
        }
        let cw_rows = (self.crop_y + h).div_ceil(2);
        let c_stride = match self.chroma {
            Chroma::Planar => self.stride / 2,
            Chroma::SemiPlanar => self.stride,
        };
        if c_stride == 0 {
            return Err("yuv: zero stride".into());
        }
        // Where each chroma plane starts. Planar: U then V, each (stride/2) wide.
        let u0 = self.stride * self.slice_height;
        let v0 = match self.chroma {
            Chroma::Planar => u0 + c_stride * self.slice_height.div_ceil(2),
            Chroma::SemiPlanar => u0,
        };
        let (u_step, v_off) = match self.chroma {
            Chroma::Planar => (1, 0),
            Chroma::SemiPlanar => (2, 1),
        };
        let luma_end = (self.crop_y + h - 1) * self.stride + self.crop_x + w;
        let chroma_end = match self.chroma {
            Chroma::Planar => v0 + c_stride * cw_rows,
            Chroma::SemiPlanar => u0 + c_stride * cw_rows,
        };
        let need = luma_end.max(chroma_end);
        if buf.len() < need {
            return Err(format!("yuv: buffer holds {} bytes, the picture needs {need}", buf.len()));
        }

        let out_w = out_w.clamp(1, w);
        let out_h = (h * out_w / w).max(1);
        let mut pixels = vec![255u8; out_w * out_h * 4];
        for oy in 0..out_h {
            let by = self.crop_y + oy * h / out_h;
            for ox in 0..out_w {
                let bx = self.crop_x + ox * w / out_w;
                let y = f32::from(buf[by * self.stride + bx]);
                let cy = by / 2;
                let cx = bx / 2;
                let (u, v) = match self.chroma {
                    Chroma::Planar => (buf[u0 + cy * c_stride + cx], buf[v0 + cy * c_stride + cx]),
                    Chroma::SemiPlanar => {
                        let i = u0 + cy * c_stride + cx * u_step;
                        (buf[i], buf[i + v_off])
                    }
                };
                let rgb = yuv_to_rgb(y, f32::from(u), f32::from(v), self.matrix);
                pixels[(oy * out_w + ox) * 4..][..4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        Ok(Rgba { width: out_w as u32, height: out_h as u32, pixels })
    }
}

/// One limited-range YCbCr sample to RGB.
pub fn yuv_to_rgb(y: f32, u: f32, v: f32, matrix: Matrix) -> [u8; 3] {
    let c = (y - 16.0) * 1.164;
    let d = u - 128.0;
    let e = v - 128.0;
    let (r, g, b) = match matrix {
        Matrix::Bt601 => (c + 1.596 * e, c - 0.392 * d - 0.813 * e, c + 2.017 * d),
        Matrix::Bt709 => (c + 1.793 * e, c - 0.213 * d - 0.533 * e, c + 2.112 * d),
    };
    let q = |x: f32| x.round().clamp(0.0, 255.0) as u8;
    [q(r), q(g), q(b)]
}

/// A HAP proxy as a [`ClipDecoder`]. Frame `k` is at `k / fps` seconds; the
/// proxy is read by index, so there is nothing to seek.
pub struct HapClip {
    hap: Hap,
    fps: f64,
}

impl HapClip {
    pub fn new(hap: Hap, fps: f64) -> HapClip {
        HapClip { hap, fps }
    }
}

impl ClipDecoder for HapClip {
    fn size(&self) -> (u32, u32) {
        (self.hap.index.width, self.hap.index.height)
    }

    fn duration(&self) -> f64 {
        self.hap.frames() as f64 / self.fps
    }

    fn seek(&mut self, _t: f64) -> Result<(), String> {
        Ok(())
    }

    fn frame_at(&mut self, t: f64) -> Result<Rgba, String> {
        let k = (t.max(0.0) * self.fps).floor() as usize;
        let frame = self.hap.frame(k)?;
        Ok(Rgba { width: frame.width, height: frame.height, pixels: frame.rgba() })
    }
}

#[cfg(target_os = "android")]
pub mod media;

#[cfg(test)]
mod tests {
    use super::*;

    fn planar(w: usize, h: usize, stride: usize, y: u8, u: u8, v: u8) -> Vec<u8> {
        let rows = h.div_ceil(2) * 2;
        let mut b = vec![0xEE; stride * rows + (stride / 2) * rows];
        for r in 0..h {
            b[r * stride..r * stride + w].fill(y);
        }
        let u0 = stride * rows;
        let v0 = u0 + (stride / 2) * rows.div_ceil(2) * 1;
        for r in 0..h.div_ceil(2) {
            b[u0 + r * (stride / 2)..][..w.div_ceil(2)].fill(u);
            b[v0 + r * (stride / 2)..][..w.div_ceil(2)].fill(v);
        }
        b
    }

    fn layout(chroma: Chroma, w: usize, h: usize, stride: usize) -> Yuv420 {
        Yuv420 { chroma, width: w, height: h, stride, slice_height: h.div_ceil(2) * 2, crop_x: 0, crop_y: 0, matrix: Matrix::Bt601 }
    }

    #[test]
    fn limited_range_white_and_black_are_white_and_black() {
        assert_eq!(yuv_to_rgb(235.0, 128.0, 128.0, Matrix::Bt601), [255, 255, 255]);
        assert_eq!(yuv_to_rgb(16.0, 128.0, 128.0, Matrix::Bt709), [0, 0, 0]);
    }

    #[test]
    fn pure_red_and_green_come_out_of_both_matrices() {
        // Red in BT.601 limited range: Y 82, Cb 90, Cr 240.
        let r = yuv_to_rgb(82.0, 90.0, 240.0, Matrix::Bt601);
        assert!(r[0] >= 252 && r[1] <= 3 && r[2] <= 3, "{r:?}");
        // Green in BT.709: Y 173, Cb 42, Cr 26.
        let g = yuv_to_rgb(173.0, 42.0, 26.0, Matrix::Bt709);
        assert!(g[1] >= 252 && g[0] <= 3 && g[2] <= 3, "{g:?}");
    }

    #[test]
    fn planar_and_semi_planar_of_the_same_picture_convert_the_same() {
        let (w, h, stride) = (4, 4, 4);
        let p = planar(w, h, stride, 120, 60, 200);
        // The same chroma as NV12: U and V interleaved in one plane.
        let mut n = vec![0u8; stride * 4 + stride * 2];
        for r in 0..h {
            n[r * stride..r * stride + w].fill(120);
        }
        for i in 0..(w / 2) * (h / 2) {
            n[stride * 4 + i * 2] = 60;
            n[stride * 4 + i * 2 + 1] = 200;
        }
        let a = layout(Chroma::Planar, w, h, stride).to_rgba(&p, w).unwrap();
        let b = layout(Chroma::SemiPlanar, w, h, stride).to_rgba(&n, w).unwrap();
        assert_eq!(a, b);
        assert_eq!((a.width, a.height), (4, 4));
    }

    #[test]
    fn row_padding_and_slice_height_padding_are_not_read_as_picture() {
        // Stride 8 for a 4-wide picture: the four bytes after each row are junk.
        let (w, h, stride) = (4, 4, 8);
        let mut p = planar(w, h, stride, 235, 128, 128);
        for r in 0..h {
            p[r * stride + w..(r + 1) * stride].fill(0x00);
        }
        let out = layout(Chroma::Planar, w, h, stride).to_rgba(&p, w).unwrap();
        assert!(out.pixels.chunks(4).all(|px| px == [255, 255, 255, 255]));
    }

    #[test]
    fn crop_selects_the_visible_rectangle() {
        // A 4x2 picture inside an 8x4 buffer: left half black, right half white.
        let (stride, slice) = (8, 4);
        let mut b = vec![0u8; stride * slice + 2 * stride * slice / 4 * 2];
        for r in 0..slice {
            for c in 0..stride {
                b[r * stride + c] = if c < 4 { 16 } else { 235 };
            }
        }
        let u0 = stride * slice;
        for i in 0..(stride / 2) * (slice / 2) * 2 {
            b[u0 + i] = 128;
        }
        let yuv = Yuv420 { chroma: Chroma::SemiPlanar, width: 4, height: 2, stride, slice_height: slice, crop_x: 4, crop_y: 2, matrix: Matrix::Bt601 };
        let out = yuv.to_rgba(&b, 4).unwrap();
        assert_eq!((out.width, out.height), (4, 2));
        assert!(out.pixels.chunks(4).all(|px| px[0] == 255));
    }

    #[test]
    fn a_short_buffer_is_an_error_not_a_panic() {
        let yuv = layout(Chroma::Planar, 4, 4, 4);
        assert!(yuv.to_rgba(&[0u8; 10], 4).is_err());
    }

    #[test]
    fn output_is_scaled_to_the_requested_width_keeping_aspect() {
        let (w, h, stride) = (16, 8, 16);
        let p = planar(w, h, stride, 128, 128, 128);
        let out = layout(Chroma::Planar, w, h, stride).to_rgba(&p, 4).unwrap();
        assert_eq!((out.width, out.height), (4, 2));
        assert_eq!(out.pixels.len(), 4 * 2 * 4);
    }

    #[test]
    fn looping_wraps_into_the_clip_and_an_unknown_length_only_clamps() {
        assert!((looped(5.5, 2.0) - 1.5).abs() < 1e-9);
        assert!((looped(-0.5, 2.0) - 1.5).abs() < 1e-9);
        assert_eq!(looped(3.0, 0.0), 3.0);
        assert_eq!(looped(-3.0, 0.0), 0.0);
    }

    #[test]
    fn matrix_follows_height() {
        assert_eq!(Matrix::for_height(1080), Matrix::Bt709);
        assert_eq!(Matrix::for_height(240), Matrix::Bt601);
    }
}
