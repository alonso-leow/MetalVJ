//! H.5c: `locate` by discrete choice of strips, never by estimating coordinates
//! on a photo.
//!
//! `the design notes` §H.5c measures why this module exists: a vision model
//! that estimates coordinates on an image misses the center by ~16–24%; the
//! same model choosing among numbered strips misses by ~3–4%. So the protocol
//! never lets the model emit a number other than "strip 3" or "row 5" —
//! everything this module does is the *judgment-free* part of the protocol:
//! draw the grid, convert chosen strips into a normalized region, compose that
//! region with the one from a previous step, and draw the final box for the
//! verification step. The choice of which strips touch the target is made by
//! the model, outside this module, looking at what [`overlay_grid`] returns —
//! this module does not call any model.
//!
//! **Ambiguity before geometry** (H.5c): if the description of the target can
//! match several instances, none of the functions here fixes it — that is
//! resolved with the `question` tool *before* calling `locate_grid`. There is
//! no way to detect that ambiguity on a rectangle of pixels, so it is not
//! attempted here; it is the responsibility of the agent turn (H.7 has to show
//! it in the shot corpus).

use png::{BitDepth, ColorType, Decoder, Encoder};
use std::io::Cursor;

/// A normalized 0..1 region inside an image — the `box` of the `locate`
/// contract. `x0 <= x1`, `y0 <= y1`, always.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Region {
    /// The whole image — starting region of step 1.
    pub fn full() -> Region {
        Region { x0: 0.0, y0: 0.0, x1: 1.0, y1: 1.0 }
    }

    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    pub fn center(&self) -> (f64, f64) {
        ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }

    pub fn as_box(&self) -> [f64; 4] {
        [self.x0, self.y0, self.x1, self.y1]
    }
}

/// Step 3 of the protocol: `global = parent_origin + local × parent_size`,
/// exactly the formula of §H.5c. `local` is in the 0..1 space of the fine grid
/// overlaid on the crop of `parent`; the result is back in the 0..1 space of
/// the original image.
pub fn compose(parent: &Region, local: &Region) -> Region {
    Region {
        x0: parent.x0 + local.x0 * parent.width(),
        y0: parent.y0 + local.y0 * parent.height(),
        x1: parent.x0 + local.x1 * parent.width(),
        y1: parent.y0 + local.y1 * parent.height(),
    }
}

/// Converts the strips the model says the target touches into a normalized
/// region, within the space of the grid that was shown to it (not necessarily
/// the original image — [`compose`] resolves that afterwards). `n` is the grid
/// size (9 in the first pass, 6 in the second); `cols`/`rows` are **1-indexed**
/// strip indices, like the numbers [`overlay_grid`] draws, so what the model
/// reads is exactly what it must return — there is no convention translation
/// in between.
///
/// Returns an error if the list is empty (nothing chosen) or if any index falls
/// outside `1..=n` — both are a model answering badly, not a case to guess
/// silently.
pub fn strips_to_region(n: u32, cols: &[u32], rows: &[u32]) -> Result<Region, String> {
    if n == 0 {
        return Err("n must be >= 1".to_string());
    }
    if cols.is_empty() || rows.is_empty() {
        return Err("cols and rows must each list at least one strip".to_string());
    }
    let check = |label: &str, xs: &[u32]| -> Result<(u32, u32), String> {
        let min = *xs.iter().min().unwrap();
        let max = *xs.iter().max().unwrap();
        if min < 1 || max > n {
            return Err(format!("{label} strip out of range 1..={n}: got {min}..={max}"));
        }
        Ok((min, max))
    };
    let (col_min, col_max) = check("cols", cols)?;
    let (row_min, row_max) = check("rows", rows)?;
    let n = n as f64;
    Ok(Region {
        x0: (col_min - 1) as f64 / n,
        x1: col_max as f64 / n,
        y0: (row_min - 1) as f64 / n,
        y1: row_max as f64 / n,
    })
}

// -- pixel work: decode, overlay, crop, re-encode ---------------------------

struct Rgba {
    w: u32,
    h: u32,
    px: Vec<u8>, // len = w*h*4
}

fn decode_png(bytes: &[u8]) -> Result<Rgba, String> {
    let decoder = Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info().map_err(|e| format!("png decode: {e}"))?;
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or("png: could not size output buffer")?];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("png decode: {e}"))?;
    let (w, h) = (info.width, info.height);
    // We normalize everything to RGBA8 — the calibration grid and this repo's
    // cameras already produce that, but a grayscale or alpha-less PNG should
    // not panic here.
    let px = match (info.color_type, info.bit_depth) {
        (ColorType::Rgba, BitDepth::Eight) => buf[..(w * h * 4) as usize].to_vec(),
        (ColorType::Rgb, BitDepth::Eight) => {
            let rgb = &buf[..(w * h * 3) as usize];
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for chunk in rgb.chunks(3) {
                out.extend_from_slice(chunk);
                out.push(255);
            }
            out
        }
        (ColorType::Grayscale, BitDepth::Eight) => {
            let g = &buf[..(w * h) as usize];
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for &v in g {
                out.extend_from_slice(&[v, v, v, 255]);
            }
            out
        }
        (ct, bd) => return Err(format!("unsupported png shape: {ct:?}/{bd:?}")),
    };
    Ok(Rgba { w, h, px })
}

fn encode_png(img: &Rgba) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = Encoder::new(&mut out, img.w, img.h);
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| format!("png encode: {e}"))?;
        writer.write_image_data(&img.px).map_err(|e| format!("png encode: {e}"))?;
    }
    Ok(out)
}

impl Rgba {
    fn set(&mut self, x: i64, y: i64, rgb: [u8; 3]) {
        if x < 0 || y < 0 || x as u32 >= self.w || y as u32 >= self.h {
            return;
        }
        let i = ((y as u32 * self.w + x as u32) * 4) as usize;
        self.px[i] = rgb[0];
        self.px[i + 1] = rgb[1];
        self.px[i + 2] = rgb[2];
        self.px[i + 3] = 255;
    }

    /// 2px thick line, so it survives the JPEG compression some providers
    /// apply to the attached image before the model sees it.
    fn h_line(&mut self, y: i64, rgb: [u8; 3]) {
        for x in 0..self.w as i64 {
            self.set(x, y, rgb);
            self.set(x, y + 1, rgb);
        }
    }

    fn v_line(&mut self, x: i64, rgb: [u8; 3]) {
        for y in 0..self.h as i64 {
            self.set(x, y, rgb);
            self.set(x + 1, y, rgb);
        }
    }

    fn rect_outline(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, rgb: [u8; 3], thickness: i64) {
        for t in 0..thickness {
            for x in x0..=x1 {
                self.set(x, y0 + t, rgb);
                self.set(x, y1 - t, rgb);
            }
            for y in y0..=y1 {
                self.set(x0 + t, y, rgb);
                self.set(x1 - t, y, rgb);
            }
        }
    }

    fn digit(&mut self, x0: i64, y0: i64, d: u8, scale: i64, rgb: [u8; 3]) {
        for (row, bits) in DIGIT_FONT[d as usize % 10].iter().enumerate() {
            for col in 0..3 {
                if bits & (1 << (2 - col)) != 0 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            self.set(x0 + col as i64 * scale + sx, y0 + row as i64 * scale + sy, rgb);
                        }
                    }
                }
            }
        }
    }

    /// Draws a number (1 or 2 digits, which is all a grid of up to 99 strips
    /// needs) starting at `(x0, y0)`.
    fn number(&mut self, x0: i64, y0: i64, n: u32, scale: i64, rgb: [u8; 3]) {
        let s = n.to_string();
        for (i, c) in s.chars().enumerate() {
            let d = c.to_digit(10).unwrap_or(0) as u8;
            self.digit(x0 + i as i64 * (4 * scale), y0, d, scale, rgb);
        }
    }
}

/// 3x5 font for digits 0-9, one `u8` per row (3 bits used).
const DIGIT_FONT: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111], // 0
    [0b010, 0b110, 0b010, 0b010, 0b111], // 1
    [0b111, 0b001, 0b111, 0b100, 0b111], // 2
    [0b111, 0b001, 0b111, 0b001, 0b111], // 3
    [0b101, 0b101, 0b111, 0b001, 0b001], // 4
    [0b111, 0b100, 0b111, 0b001, 0b111], // 5
    [0b111, 0b100, 0b111, 0b101, 0b111], // 6
    [0b111, 0b001, 0b001, 0b001, 0b001], // 7
    [0b111, 0b101, 0b111, 0b101, 0b111], // 8
    [0b111, 0b101, 0b111, 0b001, 0b111], // 9
];

const GRID_COLOR: [u8; 3] = [255, 32, 200]; // magenta: usually absent from a real scene
const BOX_COLOR: [u8; 3] = [40, 255, 40]; // green: the final verification box

/// Width and height of a PNG, from its header alone.
pub fn png_size(png_bytes: &[u8]) -> Result<(u32, u32), String> {
    let reader = Decoder::new(Cursor::new(png_bytes)).read_info().map_err(|e| format!("png decode: {e}"))?;
    let info = reader.info();
    Ok((info.width, info.height))
}

/// Steps 1 and 2 of the protocol: draws a numbered `n×n` grid over the given
/// PNG and returns the PNG with the grid on it. Columns are numbered along
/// the top and rows down the left, from 1, the same numbers
/// [`strips_to_region`] takes back. `n` sets the fineness — 9 on the first
/// pass, 6 on the second over the enlarged crop — but the function does not
/// know which pass it is on; the caller decides.
///
/// The one grid drawing in the tree: `venue_gen` sends every image under
/// this same grid (`locate`'s first pass) and asks for a venue's image
/// points as its cells.
pub fn overlay_grid(png_bytes: &[u8], n: u32) -> Result<Vec<u8>, String> {
    if n < 1 {
        return Err("n must be >= 1".to_string());
    }
    let mut img = decode_png(png_bytes)?;
    let cell_w = img.w as f64 / n as f64;
    let cell_h = img.h as f64 / n as f64;
    let scale = ((img.w.min(img.h) as i64) / (n as i64) / 12).clamp(1, 6);

    for i in 1..n {
        img.v_line((i as f64 * cell_w).round() as i64, GRID_COLOR);
        img.h_line((i as f64 * cell_h).round() as i64, GRID_COLOR);
    }
    // Column numbers above each strip, rows down the left — 1-indexed to match
    // what `strips_to_region` expects back.
    for col in 0..n {
        let x = (col as f64 * cell_w).round() as i64 + 2;
        img.number(x, 2, col + 1, scale, GRID_COLOR);
    }
    for row in 0..n {
        let y = (row as f64 * cell_h).round() as i64 + 2;
        img.number(2, y, row + 1, scale, GRID_COLOR);
    }
    encode_png(&img)
}

/// Step 2, the "crop and enlarge" half: crops `region` (normalized 0..1 in the
/// input PNG's space) and rescales it by nearest neighbor to `out_w × out_h`.
/// Nearest neighbor and not bilinear on purpose — there is no reason to smooth
/// an image that is going to be covered again with grid lines and numbers.
pub fn crop_and_scale(png_bytes: &[u8], region: &Region, out_w: u32, out_h: u32) -> Result<Vec<u8>, String> {
    let img = decode_png(png_bytes)?;
    let x0 = (region.x0 * img.w as f64).round().clamp(0.0, img.w as f64) as u32;
    let y0 = (region.y0 * img.h as f64).round().clamp(0.0, img.h as f64) as u32;
    let x1 = (region.x1 * img.w as f64).round().clamp(0.0, img.w as f64) as u32;
    let y1 = (region.y1 * img.h as f64).round().clamp(0.0, img.h as f64) as u32;
    let (crop_w, crop_h) = (x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1));

    let mut out = Rgba { w: out_w.max(1), h: out_h.max(1), px: vec![0u8; (out_w.max(1) * out_h.max(1) * 4) as usize] };
    for oy in 0..out.h {
        let sy = (y0 + (oy as u64 * crop_h as u64 / out.h as u64) as u32).min(img.h.saturating_sub(1));
        for ox in 0..out.w {
            let sx = (x0 + (ox as u64 * crop_w as u64 / out.w as u64) as u32).min(img.w.saturating_sub(1));
            let si = ((sy * img.w + sx) * 4) as usize;
            let di = ((oy * out.w + ox) * 4) as usize;
            out.px[di..di + 4].copy_from_slice(&img.px[si..si + 4]);
        }
    }
    encode_png(&out)
}

/// Step 4, "draw the box and look at it": draws the outline of `region` over
/// the full original PNG. It makes no judgment about whether the box is well
/// placed — that is what the model does when looking at the result, which is
/// exactly why this step exists and is not optional (§H.5c).
pub fn draw_box(png_bytes: &[u8], region: &Region) -> Result<Vec<u8>, String> {
    let mut img = decode_png(png_bytes)?;
    let x0 = (region.x0 * img.w as f64).round() as i64;
    let y0 = (region.y0 * img.h as f64).round() as i64;
    let x1 = ((region.x1 * img.w as f64).round() as i64 - 1).max(x0);
    let y1 = ((region.y1 * img.h as f64).round() as i64 - 1).max(y0);
    img.rect_outline(x0, y0, x1, y1, BOX_COLOR, 2);
    encode_png(&img)
}

/// A closed outline to draw: points in `0..1` of the image, a color, and the
/// number written at its first point (its row in whatever legend the caller
/// hands the model beside the picture).
pub struct Outline {
    pub points: Vec<[f32; 2]>,
    pub rgb: [u8; 3],
    pub label: u32,
}

/// Draws each outline, closed, 3 px thick, with its label beside its first
/// point — `look_venue`'s picture of what the venue expects, laid over what
/// a camera sees. Points outside the image are allowed; the part of an
/// outline inside is drawn.
pub fn draw_outlines(png_bytes: &[u8], outlines: &[Outline]) -> Result<Vec<u8>, String> {
    let mut img = decode_png(png_bytes)?;
    let (w, h) = (img.w as f64, img.h as f64);
    let scale = ((img.w.min(img.h) as i64) / 120).clamp(1, 5);
    for o in outlines {
        let px: Vec<(i64, i64)> = o
            .points
            .iter()
            .map(|p| ((p[0] as f64 * w).round() as i64, (p[1] as f64 * h).round() as i64))
            .collect();
        for k in 0..px.len() {
            let (a, b) = (px[k], px[(k + 1) % px.len()]);
            img.line(a, b, o.rgb, 3);
        }
        if let Some(&(x, y)) = px.first() {
            img.number(x + 4, y + 4, o.label, scale, o.rgb);
        }
    }
    encode_png(&img)
}

impl Rgba {
    /// Bresenham, thickened into a square brush. Clipped by `set`, and the
    /// walk itself is bounded so a point far off the image cannot spin.
    fn line(&mut self, a: (i64, i64), b: (i64, i64), rgb: [u8; 3], thickness: i64) {
        let limit = 4 * (self.w.max(self.h) as i64);
        let clamp = |v: i64| v.clamp(-limit, limit);
        let (mut x0, mut y0, x1, y1) = (clamp(a.0), clamp(a.1), clamp(b.0), clamp(b.1));
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let mut err = dx + dy;
        let r = thickness / 2;
        loop {
            for oy in -r..=r {
                for ox in -r..=r {
                    self.set(x0 + ox, y0 + oy, rgb);
                }
            }
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- pure geometry: no PNG, no model --------------------------------

    #[test]
    fn strips_to_region_maps_1_indexed_strips_to_0_1() {
        // 9x9 grid, target touches columns 3..5 and rows 2..3
        let r = strips_to_region(9, &[3, 4, 5], &[2, 3]).unwrap();
        assert!((r.x0 - 2.0 / 9.0).abs() < 1e-9);
        assert!((r.x1 - 5.0 / 9.0).abs() < 1e-9);
        assert!((r.y0 - 1.0 / 9.0).abs() < 1e-9);
        assert!((r.y1 - 3.0 / 9.0).abs() < 1e-9);
    }

    #[test]
    fn strips_to_region_takes_min_and_max_when_unsorted() {
        let r = strips_to_region(9, &[5, 3, 4], &[3, 2]).unwrap();
        assert_eq!(r.as_box(), strips_to_region(9, &[3, 4, 5], &[2, 3]).unwrap().as_box());
    }

    #[test]
    fn strips_to_region_a_single_cell_is_a_1_over_n_square() {
        let r = strips_to_region(6, &[1], &[1]).unwrap();
        assert!((r.width() - 1.0 / 6.0).abs() < 1e-9);
        assert!((r.height() - 1.0 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn strips_to_region_rejects_out_of_range_indices() {
        assert!(strips_to_region(9, &[0], &[1]).is_err(), "0 is not a valid 1-indexed strip");
        assert!(strips_to_region(9, &[10], &[1]).is_err(), "10 is out of a 9-wide grid");
    }

    #[test]
    fn strips_to_region_rejects_an_empty_list() {
        assert!(strips_to_region(9, &[], &[1]).is_err());
        assert!(strips_to_region(9, &[1], &[]).is_err());
    }

    /// The end-to-end case of §H.5c: pass 1 picks a wide region over the 9x9
    /// grid, pass 2 refines with a 6x6 grid *inside that crop* — `compose` has
    /// to return the final region in the original image's space, not in the
    /// crop's space.
    #[test]
    fn compose_maps_a_second_pass_selection_back_to_original_space() {
        let pass1 = strips_to_region(9, &[3, 4, 5], &[2, 3]).unwrap(); // wide region
        // In the enlarged crop, pass 2 with a 6x6 grid picks the top-left
        // quadrant (cell 1,1 of 6x6) as the exact target.
        let pass2_local = strips_to_region(6, &[1], &[1]).unwrap();
        let global = compose(&pass1, &pass2_local);

        // The result has to be fully contained in pass1...
        assert!(global.x0 >= pass1.x0 - 1e-9 && global.x1 <= pass1.x1 + 1e-9);
        assert!(global.y0 >= pass1.y0 - 1e-9 && global.y1 <= pass1.y1 + 1e-9);
        // ...and be exactly 1/6 of its width/height, in the top-left corner.
        assert!((global.x0 - pass1.x0).abs() < 1e-9);
        assert!((global.width() - pass1.width() / 6.0).abs() < 1e-9);
    }

    #[test]
    fn compose_with_a_full_parent_is_the_identity() {
        let local = strips_to_region(9, &[4], &[7]).unwrap();
        let global = compose(&Region::full(), &local);
        assert_eq!(global.as_box(), local.as_box());
    }

    #[test]
    fn region_center_is_the_midpoint() {
        let r = Region { x0: 0.2, y0: 0.4, x1: 0.6, y1: 0.8 };
        let (cx, cy) = r.center();
        assert!((cx - 0.4).abs() < 1e-9);
        assert!((cy - 0.6).abs() < 1e-9);
    }

    // -- pixels: real PNG, no camera or model -----------------------------

    fn solid_png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut img = Rgba { w, h, px: vec![0u8; (w * h * 4) as usize] };
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                img.set(x, y, rgb);
            }
        }
        encode_png(&img).unwrap()
    }

    #[test]
    fn overlay_grid_round_trips_as_a_valid_png_of_the_same_size() {
        let png = solid_png(90, 90, [10, 10, 10]);
        let out = overlay_grid(&png, 9).unwrap();
        let decoded = decode_png(&out).unwrap();
        assert_eq!((decoded.w, decoded.h), (90, 90));
    }

    #[test]
    fn overlay_grid_actually_draws_something_the_solid_background_did_not_have() {
        let png = solid_png(90, 90, [10, 10, 10]);
        let before = decode_png(&png).unwrap();
        let out = overlay_grid(&png, 9).unwrap();
        let after = decode_png(&out).unwrap();
        assert_ne!(before.px, after.px, "a grid over a solid frame must change pixels");
    }

    #[test]
    fn overlay_grid_rejects_zero_cells() {
        let png = solid_png(10, 10, [0, 0, 0]);
        assert!(overlay_grid(&png, 0).is_err());
    }

    #[test]
    fn crop_and_scale_extracts_the_requested_region_at_the_requested_size() {
        // Left quadrant white, right black; cropping only the left and
        // requesting an all-white result confirms that the crop took the right
        // region and not, for example, the whole image.
        let mut img = Rgba { w: 100, h: 100, px: vec![0u8; 100 * 100 * 4] };
        for y in 0..100i64 {
            for x in 0..50i64 {
                img.set(x, y, [255, 255, 255]);
            }
        }
        let png = encode_png(&img).unwrap();
        let region = Region { x0: 0.0, y0: 0.0, x1: 0.5, y1: 1.0 };
        let out = crop_and_scale(&png, &region, 40, 40).unwrap();
        let decoded = decode_png(&out).unwrap();
        assert_eq!((decoded.w, decoded.h), (40, 40));
        assert!(decoded.px.chunks(4).all(|p| p[0] == 255), "the whole crop should be the white half");
    }

    #[test]
    fn draw_box_changes_pixels_at_the_boxs_edge() {
        let png = solid_png(100, 100, [0, 0, 0]);
        let region = Region { x0: 0.2, y0: 0.2, x1: 0.8, y1: 0.8 };
        let out = draw_box(&png, &region).unwrap();
        let decoded = decode_png(&out).unwrap();
        // The box's left edge falls at x=20.
        let i = ((50 * decoded.w + 20) * 4) as usize;
        assert_eq!(&decoded.px[i..i + 3], &BOX_COLOR);
    }
}
