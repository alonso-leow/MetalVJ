//! Pixel packing for the sinks that do not take RGBA: YUYV (4:2:2, BT.601
//! limited range — what every v4l2 consumer reads without asking) and RGB24.

/// BT.601 limited-range Y/U/V of one RGB pixel.
#[inline]
fn yuv(r: u8, g: u8, b: u8) -> (i32, i32, i32) {
    let (r, g, b) = (r as i32, g as i32, b as i32);
    let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
    (y, u, v)
}

/// Packs `w × h` RGBA into YUYV: two pixels → `Y0 U Y1 V`, chroma averaged
/// over the pair. An odd width repeats the last pixel for the missing half.
pub fn rgba_to_yuyv(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let pairs = w.div_ceil(2);
    let mut out = Vec::with_capacity(pairs * 4 * h);
    for y in 0..h {
        let row = &rgba[y * w * 4..(y + 1) * w * 4];
        for p in 0..pairs {
            let a = p * 2;
            let b = (a + 1).min(w - 1);
            let (y0, u0, v0) = yuv(row[a * 4], row[a * 4 + 1], row[a * 4 + 2]);
            let (y1, u1, v1) = yuv(row[b * 4], row[b * 4 + 1], row[b * 4 + 2]);
            out.push(y0.clamp(0, 255) as u8);
            out.push(((u0 + u1) / 2).clamp(0, 255) as u8);
            out.push(y1.clamp(0, 255) as u8);
            out.push(((v0 + v1) / 2).clamp(0, 255) as u8);
        }
    }
    out
}

/// Drops alpha.
pub fn rgba_to_rgb24(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_and_white_land_on_the_limited_range_ends() {
        let px = [0, 0, 0, 255, 255, 255, 255, 255];
        let out = rgba_to_yuyv(&px, 2, 1);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], 16, "black is Y=16");
        assert_eq!(out[2], 235, "white is Y=235");
        assert_eq!(out[1], 128, "grey has no chroma");
        assert_eq!(out[3], 128);
    }

    #[test]
    fn red_pushes_v_up_and_u_down() {
        let px = [255, 0, 0, 255, 255, 0, 0, 255];
        let out = rgba_to_yuyv(&px, 2, 1);
        assert_eq!(out[0], 82);
        assert!(out[1] < 128 && out[3] > 200, "{out:?}");
    }

    #[test]
    fn odd_width_rounds_up_to_whole_pairs() {
        let px = vec![10u8; 3 * 2 * 4];
        assert_eq!(rgba_to_yuyv(&px, 3, 2).len(), 2 * 4 * 2);
    }

    #[test]
    fn rgb24_drops_alpha() {
        assert_eq!(rgba_to_rgb24(&[1, 2, 3, 4, 5, 6, 7, 8]), vec![1, 2, 3, 5, 6, 7]);
    }
}
