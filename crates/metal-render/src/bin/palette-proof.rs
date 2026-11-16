//! Proof sheets for the show's palettes, headless.
//!
//!     cargo run --release -p metal-render --bin palette-proof -- \
//!         --palettes data/library/palettes.json --out /tmp/palettes \
//!         --sheet docs/palettes/proof.png
//!
//! Reads `data/library/palettes.json` (`[{name, colors: [12 floats]}]`, four
//! RGB stops in slot order `base, acento, fondo, destello`) and draws every
//! palette on a few contrasting models (`--models`, default
//! `puzzle,rings,aurora`) through [`Renderer::solo`] — the same call the
//! shell's solo preview uses, so palette motion is applied exactly as on air.
//!
//! The palette goes into every colour slot of the model; the background slot
//! `fondo` gets the palette's `fondo` stop as all four of its stops. The
//! effects' `leak_color` (the row's last slot) keeps its default.
//!
//! Each palette is rendered at the neutral motion and at the motion extremes
//! a palette must survive (`col_hue` ±0.08, `col_bri` 0.6 / 1.4,
//! `col_contrast` ±0.5). Models that build up over time are simulated
//! forward for `--seconds` at `--fps` and the last frame is kept.
//!
//! Output: `<out>/<name>.png` per palette (rows = models, columns = motions,
//! a swatch strip on the left), and `--sheet` (default `<out>/proof.png`):
//! one row per palette — swatches, every model at neutral, then the first
//! model under each extreme, at quarter size.
//!
//! # The tone strip (the tone design notes step 13), GPU-free
//!
//! Given `--tone-digest <path.npy>` or `--synth`, the binary skips the GPU
//! entirely and writes `<out>/tone-strip.png`: one vertical column per digest
//! row, its four §5 gradient stops stacked top to bottom. Each row's
//! `tone_x`/`tone_y`/`tone_register`/`tone_density`/`tone_mode` is turned into
//! the gradient by `metal_notation::tone::tone_gradient` — the same function the
//! drive path calls — so what the sheet shows is exactly what a model would be
//! painted with. `--synth` fabricates a deterministic sweep (a full turn of the
//! centre, a triangle of the spread) so the strip is meaningful **while
//! `data/models/basic-pitch/model.onnx` is absent**: it is the stand-in for
//! step 13's by-eye review until a real model exists to produce a digest.

use std::io::BufWriter;

use metal_notation::tone::{tone_gradient, Palette};
use metal_render::headless::Renderer;
use metal_schema::Block;

const MOTIONS: [(&str, [f32; 4]); 7] = [
    ("neutral", [0.0, 1.0, 1.0, 0.0]),
    ("hue-0.08", [-0.08, 1.0, 1.0, 0.0]),
    ("hue+0.08", [0.08, 1.0, 1.0, 0.0]),
    ("bri0.6", [0.0, 1.0, 0.6, 0.0]),
    ("bri1.4", [0.0, 1.0, 1.4, 0.0]),
    ("con-0.5", [0.0, 1.0, 1.0, -0.5]),
    ("con+0.5", [0.0, 1.0, 1.0, 0.5]),
];
const MOTION_FLOATS: [&str; 4] = ["col_hue", "col_sat", "col_bri", "col_contrast"];

/// An RGBA canvas the sheets are pasted into.
struct Canvas {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Self {
        let mut px = vec![0u8; (w * h * 4) as usize];
        for p in px.chunks_mut(4) {
            p.copy_from_slice(&[24, 24, 24, 255]);
        }
        Canvas { w, h, px }
    }

    /// Pastes `src` (sw x sh) at (x, y), box-downscaled by `f`.
    fn paste(&mut self, src: &[u8], sw: u32, sh: u32, f: u32, x: u32, y: u32) {
        let (dw, dh) = (sw / f, sh / f);
        for j in 0..dh {
            for i in 0..dw {
                let mut acc = [0u32; 3];
                for dy in 0..f {
                    for dx in 0..f {
                        let o = (((j * f + dy) * sw + i * f + dx) * 4) as usize;
                        for c in 0..3 {
                            acc[c] += src[o + c] as u32;
                        }
                    }
                }
                let (px, py) = (x + i, y + j);
                if px < self.w && py < self.h {
                    let o = ((py * self.w + px) * 4) as usize;
                    for c in 0..3 {
                        self.px[o + c] = (acc[c] / (f * f)) as u8;
                    }
                }
            }
        }
    }

    fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, rgb: [f32; 3]) {
        for py in y..(y + h).min(self.h) {
            for px in x..(x + w).min(self.w) {
                let o = ((py * self.w + px) * 4) as usize;
                for c in 0..3 {
                    self.px[o + c] = (rgb[c].clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        }
    }

    /// Four stacked swatches, slot order.
    fn swatches(&mut self, x: u32, y: u32, w: u32, h: u32, colors: &[f32]) {
        let sh = h / 4;
        for s in 0..4usize {
            self.fill(x, y + s as u32 * sh, w, sh, [colors[s * 3], colors[s * 3 + 1], colors[s * 3 + 2]]);
        }
    }

    fn save(&self, path: &str) {
        if let Some(dir) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let file = std::fs::File::create(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let mut enc = png::Encoder::new(BufWriter::new(file), self.w, self.h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::High);
        let rgb: Vec<u8> = self.px.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        enc.write_header().expect("png header").write_image_data(&rgb).expect("png data");
    }
}

/// The model's row with the palette in every colour slot (fondo flat) and the
/// motion floats set.
fn row(schema: &metal_schema::Schema, colors: &[f32], motion: [f32; 4]) -> Vec<f32> {
    let mut out = schema.defaults();
    let leak_at = out.len() - 12;
    let mut at = 0;
    for block in schema.blocks {
        match block {
            Block::ColorSlot(name, _) if at != leak_at => {
                if *name == "background" {
                    for s in 0..4 {
                        out[at + s * 3..at + s * 3 + 3].copy_from_slice(&colors[6..9]);
                    }
                } else {
                    out[at..at + 12].copy_from_slice(colors);
                }
            }
            Block::Floats(fs) => {
                for (i, s) in fs.iter().enumerate() {
                    if let Some(k) = MOTION_FLOATS.iter().position(|m| *m == s.name) {
                        out[at + i] = motion[k];
                    }
                }
            }
            _ => {}
        }
        at += block.width();
    }
    out
}

/// The four gradient stops one tone row maps to.
///
/// `tone_x`/`tone_y` store the harmony centre as a vector (the tone design notes D3), so
/// the angle is `atan2` and the focus is the vector's length inverted into a
/// spread — exactly the recovery `metal-perform::drive::tone_slot` does before it
/// calls the same [`tone_gradient`]. Spectrum is used so the strip shows the
/// hue moving with the centre; MetalVJ would hold every column red and the
/// centre sweep would be invisible.
fn tone_row(t: &[f32; 6]) -> [[f32; 3]; 4] {
    let (x, y, register, density, mode) = (t[0], t[1], t[2], t[3], t[4]);
    let centre_deg = (y - 0.5).atan2(x - 0.5).to_degrees();
    let spread = (1.0 - 2.0 * ((x - 0.5).hypot(y - 0.5))).clamp(0.0, 1.0);
    tone_gradient(centre_deg, spread, register, density, mode, Palette::Spectrum)
}

/// How many of the four stops are visually different, by 8-bit triple — the
/// observable an "n stops" claim has when the slot count is always four.
#[cfg(test)]
fn distinct_stops(stops: [[f32; 3]; 4]) -> usize {
    let mut seen: Vec<[i32; 3]> = Vec::new();
    for c in stops {
        let t = [
            (c[0] * 255.0).round() as i32,
            (c[1] * 255.0).round() as i32,
            (c[2] * 255.0).round() as i32,
        ];
        if !seen.contains(&t) {
            seen.push(t);
        }
    }
    seen.len()
}

/// Reads a digest matrix's tone block, inline.
///
/// A copy of `metal-router::npy::load`'s header parse, kept here so this bin needs
/// no `metal-router` dependency: a `.npy` is a magic number, a header and floats,
/// and a whole crate for that would be more code than this is. Only what the
/// pipeline writes is accepted (little-endian float32, 2-D, C order); a 35-wide
/// row carries the block at columns 29..35, and a 29- or 16-wide row (a digest
/// from before the tone block, which is what the library on disk still is) has
/// its tone block read as all zero — the same fallback `npy::load` gives, so a
/// strip from an old song is a flat neutral rather than a wrong hue.
fn read_tone_block(path: &str) -> Vec<[f32; 6]> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    if bytes.len() < 12 || &bytes[..6] != b"\x93NUMPY" {
        panic!("{path} is not a .npy");
    }
    let (header_len, start) = match bytes[6] {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        _ => (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12,
        ),
    };
    if start + header_len > bytes.len() {
        panic!("{path}: header runs past the end");
    }
    let header = String::from_utf8_lossy(&bytes[start..start + header_len]).to_string();
    if !(header.contains("'<f4'") || header.contains("\"<f4\"")) {
        panic!("{path} is not little-endian float32");
    }
    if header.contains("'fortran_order': True") {
        panic!("{path} is column-major; the digest is not");
    }
    let open = header.find('(').unwrap_or_else(|| panic!("{path}: no shape"));
    let close = header[open..].find(')').unwrap_or_else(|| panic!("{path}: no shape")) + open;
    let shape: Vec<usize> = header[open + 1..close]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    if shape.len() != 2 {
        panic!("{path}: expected a 2-D matrix, got {shape:?}");
    }
    let (rows, width) = (shape[0], shape[1]);
    let tone_at = metal_schema::MUSIC + metal_schema::VOICE;
    if width != metal_schema::FEATURES && width != tone_at && width != metal_schema::MUSIC {
        panic!(
            "{path}: {width} columns, but a digest row is {}, {} or {}",
            metal_schema::FEATURES,
            tone_at,
            metal_schema::MUSIC
        );
    }
    let body = &bytes[start + header_len..];
    if body.len() < rows * width * 4 {
        panic!("{path}: {} bytes short", rows * width * 4 - body.len());
    }

    let mut out = Vec::with_capacity(rows);
    for r in 0..rows {
        let mut t = [0.0f32; 6];
        // The tone block only exists at the current width; an older row reads
        // it as zero, which is why a 29- or 16-wide file draws the neutral.
        if width == metal_schema::FEATURES {
            for (c, slot) in t.iter_mut().enumerate() {
                let at = (r * width + tone_at + c) * 4;
                *slot = f32::from_le_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]]);
            }
        }
        out.push(t);
    }
    out
}

/// A deterministic tone sweep, the stand-in for step 13 while no model exists.
///
/// A full turn of the centre (`tone_x`/`tone_y` trace a circle of decreasing
/// radius) and a triangle of the spread, so the strip shows every hue and both
/// ends of the stop count; register, density and mode advance deterministically
/// so lightness, chroma and the warm/cool bias are all exercised. It is a
/// function, not random: two runs draw the same sheet.
fn synth_tone_rows() -> Vec<[f32; 6]> {
    let n = 512usize;
    (0..n)
        .map(|i| {
            let u = i as f32 / (n - 1) as f32;
            let centre = u * std::f32::consts::TAU;
            // 0 → 1 → 0, so the strip passes through focused and diffuse.
            let spread = 1.0 - (2.0 * u - 1.0).abs();
            // The vector the gradient recovers the angle and focus from: length
            // (1 - spread)/2 along the centre's direction, mapped into 0..1.
            let radius = (1.0 - spread) * 0.5;
            [
                0.5 + radius * centre.cos(),
                0.5 + radius * centre.sin(),
                u,
                0.5 + 0.5 * (centre * 2.0).sin(),
                0.5 + 0.5 * (centre * 3.0).cos(),
                1.0,
            ]
        })
        .collect()
}

/// Draws a digest's tone block as `<path>`: one column per row, its four §5
/// stops stacked, so a gradient is legible as an image. Long digests are
/// strided down to a bounded strip rather than written thousands of pixels
/// wide.
fn tone_strip(rows: &[[f32; 6]], path: &str) {
    let step = rows.len().div_ceil(2400).max(1);
    let cols = rows.len().div_ceil(step);
    let cell = 48u32;
    let mut canvas = Canvas::new(cols.max(1) as u32, cell * 4);
    for (i, r) in rows.iter().step_by(step).enumerate() {
        let stops = tone_row(r);
        for (s, stop) in stops.iter().enumerate() {
            canvas.fill(i as u32, s as u32 * cell, 1, cell, *stop);
        }
    }
    canvas.save(path);
}

fn main() {
    let mut palettes = String::from("data/library/palettes.json");
    let mut out = String::from("/tmp/palette-proof");
    let mut sheet: Option<String> = None;
    let mut models = vec!["puzzle".to_string(), "rings".into(), "aurora".into()];
    let (mut w, mut h, mut fps, mut seconds) = (640u32, 360u32, 8.0f32, 4.0f32);
    let mut only: Option<String> = None;
    let mut tone_digest: Option<String> = None;
    let mut synth = false;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--palettes" => palettes = val(),
            "--out" => out = val(),
            "--sheet" => sheet = Some(val()),
            "--models" => models = val().split(',').map(|s| s.trim().to_string()).collect(),
            "--w" => w = val().parse().expect("bad --w"),
            "--h" => h = val().parse().expect("bad --h"),
            "--fps" => fps = val().parse().expect("bad --fps"),
            "--seconds" => seconds = val().parse().expect("bad --seconds"),
            "--only" => only = Some(val()),
            // The GPU-free tone strip (step 13). Either flag means "do not
            // touch the GPU"; `--synth` wins when both are given.
            "--tone-digest" => tone_digest = Some(val()),
            "--synth" => synth = true,
            other => panic!("unknown argument {other}"),
        }
    }

    if synth || tone_digest.is_some() {
        let rows = if synth {
            synth_tone_rows()
        } else {
            read_tone_block(tone_digest.as_deref().expect("a path"))
        };
        let path = format!("{out}/tone-strip.png");
        tone_strip(&rows, &path);
        println!(
            "tone strip {path}: {} rows{}",
            rows.len(),
            if synth { " (synthetic sweep)" } else { "" }
        );
        return;
    }

    let text = std::fs::read_to_string(&palettes).unwrap_or_else(|e| panic!("{palettes}: {e}"));
    let list: Vec<serde_json::Value> = serde_json::from_str(&text).expect("palettes is not a JSON array");
    let list: Vec<(String, Vec<f32>)> = list
        .iter()
        .map(|p| {
            let name = p["name"].as_str().expect("palette name").to_string();
            let colors: Vec<f32> =
                p["colors"].as_array().expect("colors").iter().map(|v| v.as_f64().unwrap() as f32).collect();
            assert_eq!(colors.len(), 12, "{name}: 12 floats");
            (name, colors)
        })
        .filter(|(n, _)| only.as_ref().is_none_or(|o| o == n))
        .collect();
    std::fs::create_dir_all(&out).expect("create --out");

    let mut renderer = Renderer::new();
    let index: Vec<usize> = models
        .iter()
        .map(|m| renderer.ids().iter().position(|id| id == m).unwrap_or_else(|| panic!("no model {m:?}")))
        .collect();
    let schemas = renderer.schemas();

    // Overall sheet geometry: half-size tiles.
    let (tw, th) = (w / 2, h / 2);
    let sw = tw / 3;
    let cols = models.len() as u32 + MOTIONS.len() as u32 - 1;
    let (qw, qh, qs) = (w / 4, h / 4, w / 12);
    let mut overall = Canvas::new(qs + cols * (qw + 2), list.len() as u32 * (qh + 2));

    let frames = (seconds * fps).round().max(1.0) as u32;
    for (p, (name, colors)) in list.iter().enumerate() {
        let mut page = Canvas::new(sw + MOTIONS.len() as u32 * (tw + 4), models.len() as u32 * (th + 4));
        page.swatches(0, 0, sw - 4, page.h, colors);
        let oy = p as u32 * (qh + 2);
        overall.swatches(0, oy, qs - 2, qh, colors);
        for (m, &idx) in index.iter().enumerate() {
            for (k, (_, motion)) in MOTIONS.iter().enumerate() {
                let floats = row(schemas[idx], colors, *motion);
                let mut last = None;
                for f in 0..frames {
                    last = Some(renderer.solo(idx, &floats, f as f32 / fps, w, h, false));
                }
                let (rgba, rw, rh) = last.unwrap();
                page.paste(&rgba, rw, rh, 2, sw + k as u32 * (tw + 4), m as u32 * (th + 4));
                let col = if k == 0 {
                    Some(m as u32)
                } else if m == 0 {
                    Some(models.len() as u32 + k as u32 - 1)
                } else {
                    None
                };
                if let Some(c) = col {
                    overall.paste(&rgba, rw, rh, 4, qs + c * (qw + 2), oy);
                }
            }
        }
        let file = format!("{out}/{name}.png");
        page.save(&file);
        println!("[{}/{}] {file}", p + 1, list.len());
    }
    let sheet = sheet.unwrap_or_else(|| format!("{out}/proof.png"));
    overall.save(&sheet);
    println!("sheet {sheet}: rows = palettes in file order; columns = swatches, {} at neutral, {} at {}",
        models.join(", "), models[0], MOTIONS[1..].iter().map(|m| m.0).collect::<Vec<_>>().join(", "));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tone row with a chosen focus: `x`/`y` on the vector circle. `spread`
    /// 0 puts it on the rim (`|v| = 1`), `spread` 1 at the centre.
    fn row_at(centre_deg: f32, spread: f32) -> [f32; 6] {
        let radius = (1.0 - spread) * 0.5;
        let a = centre_deg.to_radians();
        [0.5 + radius * a.cos(), 0.5 + radius * a.sin(), 0.5, 0.5, 0.5, 1.0]
    }

    /// A focused harmony gives two stops, a diffuse one four — the same
    /// observable `metal-perform::drive`'s `n_stops_grows_with_spread` pins, so
    /// the proof sheet and the engine agree about what the block means.
    #[test]
    fn tone_rows_map_to_the_expected_stop_counts() {
        assert_eq!(distinct_stops(tone_row(&row_at(0.0, 0.0))), 2, "focused is two stops");
        assert_eq!(distinct_stops(tone_row(&row_at(0.0, 1.0))), 4, "diffuse is four stops");
    }

    /// A NaN anywhere in a row must not panic or paint a non-displayable stop.
    #[test]
    fn a_nan_tone_row_does_not_panic() {
        for bad in [f32::NAN, f32::INFINITY] {
            let r = [bad, bad, bad, bad, bad, bad];
            for stop in tone_row(&r) {
                for v in stop {
                    assert!(v.is_finite() && (0.0..=1.0).contains(&v), "bad stop {v}");
                }
            }
        }
    }

    /// The synthetic sweep has to be deterministic and fully displayable, or
    /// the step-13 stand-in is not a sheet anyone can review.
    #[test]
    fn the_synth_sweep_is_deterministic_and_displayable() {
        let a = synth_tone_rows();
        let b = synth_tone_rows();
        assert_eq!(a.len(), 512);
        assert_eq!(a, b, "the sweep is a function, not random");
        for r in &a {
            for stop in tone_row(r) {
                for v in stop {
                    assert!(v.is_finite() && (0.0..=1.0).contains(&v));
                }
            }
        }
    }
}
