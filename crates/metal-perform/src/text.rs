//! S.W — `text.source`: a string rendered to an RGBA frame, and the closed
//! vocabulary of per-glyph `fx.text.*` effects.
//!
//! P-B: the `gpu` painter draws the same [`plan`] with glyphon
//! (`crate::text_gpu`); this CPU rasterizer is the `Stand`/wasm path and the
//! reference. **Why a CPU rasterizer existed first.** The painters hand frames
//! around as raw RGBA (`Paint::solo` reads a texture back before compose sees
//! it), so a text frame only has to *exist* as pixels at the right size. That
//! removes the only reason to build a second GPU pipeline: shaping and
//! rasterization happen here, once, and the same bytes feed the GPU path and
//! the CPU `Stand`. The typography engine is [`cosmic_text`] (pure Rust, no
//! HTML), chosen in `the design notes` § "Text Source y Text Effects".
//!
//! **What this module owns**: [`TextConfig`] (the whole property panel as
//! data), [`TextEffect`] (the closed vocabulary — `typewriter`, `wave`,
//! `jitter`, `color_cycle`, `scale_pulse`, each an easing over
//! `(index, t)`), and [`render`], a pure function of config + row + time.
//! Same config, same row, same `t` ⇒ same bytes, which is what lets the
//! frame pipeline stay deterministic.

use std::cell::RefCell;

use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, SwashCache, Weight,
};
use serde::{Deserialize, Serialize};

use metal_schema::{read_signal, TEXT_EFFECT_NAMES};

/// Horizontal alignment of the text block. A closed set; anything else reads
/// as `Left`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Default for Align {
    fn default() -> Self {
        Align::Left
    }
}

/// Vertical placement of the text block inside the frame minus margin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VAlign {
    #[default]
    Top,
    Center,
    Bottom,
}

/// An outline around every glyph: the ring between the glyph coverage
/// dilated by `width` and the fill itself. `width` is in em (a fraction of
/// the font size); `0` is no stroke and renders byte-identical to none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stroke {
    pub width: f32,
    /// sRGB 0..1.
    pub color: [f32; 4],
}

impl Default for Stroke {
    fn default() -> Self {
        Stroke { width: 0.0, color: [0.0, 0.0, 0.0, 1.0] }
    }
}

/// A box behind the text block (TouchDesigner's "Border Space"): the block's
/// bounds grown by `padding`, filled with `color`, corners rounded by
/// `radius`. `padding` and `radius` are fractions of the frame height;
/// alpha 0 means no box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Border {
    pub padding: f32,
    pub color: [f32; 4],
    pub radius: f32,
}

impl Default for Border {
    fn default() -> Self {
        Border { padding: 0.02, color: [0.0, 0.0, 0.0, 0.0], radius: 0.0 }
    }
}

/// The text node's config — the property panel, serialized. Every field is
/// `#[serde(default)]`, so a node written by an older version, or by hand,
/// loads with the defaults rather than failing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextConfig {
    /// The string. `\n` starts a new line. `text` is accepted as an alias:
    /// the authoring timeline's text track wrote that key before `text.source`
    /// and its segments still read.
    #[serde(alias = "text")]
    pub content: String,
    /// Family name, or one of `sans-serif`/`serif`/`monospace`/`cursive`/`fantasy`.
    pub family: String,
    /// CSS-style weight (100..900); `400` is regular.
    pub weight: u16,
    /// `normal` or `italic`.
    pub style: String,
    /// Cap height as a fraction of the frame's height. `0.18` is a title.
    pub size: f32,
    /// Extra advance per glyph, in em.
    pub tracking: f32,
    /// Line height as a multiple of `size`.
    pub leading: f32,
    pub align: Align,
    pub valign: VAlign,
    /// Inset from the frame edge, as a fraction of the frame.
    pub margin: f32,
    /// Fill, sRGB 0..1.
    pub color: [f32; 4],
    /// Block background, sRGB 0..1; alpha 0 means none.
    pub background: [f32; 4],
    pub stroke: Stroke,
    pub border: Border,
    /// Wrap lines to the frame width minus margin. `false` keeps each `\n`
    /// line whole (and lets it run off the frame).
    pub line_wrap: bool,
    /// `fx.text.*` steps, in order. Their `kind` is one of
    /// [`TEXT_EFFECT_NAMES`]; an unknown one is skipped, not a crash.
    pub effects: Vec<TextEffectStep>,
}

impl Default for TextConfig {
    fn default() -> Self {
        TextConfig {
            content: "MetalVJ".into(),
            family: "sans-serif".into(),
            weight: 400,
            style: "normal".into(),
            size: 0.18,
            tracking: 0.0,
            leading: 1.2,
            align: Align::Left,
            margin: 0.06,
            color: [1.0, 1.0, 1.0, 1.0],
            background: [0.0, 0.0, 0.0, 0.0],
            valign: VAlign::Top,
            stroke: Stroke::default(),
            border: Border::default(),
            line_wrap: true,
            effects: Vec::new(),
        }
    }
}

/// One `fx.text.*` step: a named effect and its amount. `t` (time) is the
/// other input and arrives from the frame, not the config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextEffectStep {
    pub kind: String,
    #[serde(default = "one")]
    pub amount: f32,
}

fn one() -> f32 {
    1.0
}

/// The closed vocabulary, as an enum. [`TextEffect::parse`] is the single
/// place the config's strings become behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEffect {
    Typewriter,
    Wave,
    Jitter,
    ColorCycle,
    ScalePulse,
}

/// What one effect does to one glyph, in frame pixels and sRGB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphFx {
    pub dx: f32,
    pub dy: f32,
    pub scale: f32,
    pub alpha: f32,
    /// Rotation of hue, turns of the colour wheel.
    pub hue: f32,
}

impl Default for GlyphFx {
    fn default() -> Self {
        GlyphFx { dx: 0.0, dy: 0.0, scale: 1.0, alpha: 1.0, hue: 0.0 }
    }
}

impl TextEffect {
    pub fn name(self) -> &'static str {
        match self {
            TextEffect::Typewriter => "typewriter",
            TextEffect::Wave => "wave",
            TextEffect::Jitter => "jitter",
            TextEffect::ColorCycle => "color_cycle",
            TextEffect::ScalePulse => "scale_pulse",
        }
    }

    pub fn parse(text: &str) -> Option<TextEffect> {
        let name = text.strip_prefix("fx.text.").unwrap_or(text);
        Some(match name {
            "typewriter" => TextEffect::Typewriter,
            "wave" => TextEffect::Wave,
            "jitter" => TextEffect::Jitter,
            "color_cycle" => TextEffect::ColorCycle,
            "scale_pulse" => TextEffect::ScalePulse,
            _ => return None,
        })
    }

    /// The whole of the effect: an easing over the glyph's index in the run
    /// and the frame's time. Pure, so the tests can pin it.
    ///
    /// `count` is the number of glyphs in the run, `index` this glyph's
    /// position in it. `amount` is the effect's knob; `seed` is a stable
    /// per-glyph value the caller derives from the character and index, used
    /// by `jitter` so it does not flicker per frame.
    pub fn apply(
        self,
        index: usize,
        count: usize,
        t: f32,
        amount: f32,
        seed: u32,
        height: f32,
    ) -> GlyphFx {
        let n = count.max(1) as f32;
        let amount = amount.max(0.0);
        match self {
            // Reveal a growing prefix; at amount 1 the whole string shows.
            TextEffect::Typewriter => {
                let shown = (amount * n).floor() as usize;
                let alpha = if index < shown { 1.0 } else { 0.0 };
                GlyphFx { alpha, ..GlyphFx::default() }
            }
            // A travelling sine, measured in glyphs so it does not depend on
            // the font size.
            TextEffect::Wave => {
                let phase = t * std::f32::consts::TAU + index as f32 * 0.5;
                GlyphFx { dy: amount * height * 0.06 * phase.sin(), ..GlyphFx::default() }
            }
            // Deterministic shake: the same glyph at the same time lands on
            // the same offset, so a paused frame is still.
            TextEffect::Jitter => {
                let a = hash(seed ^ (t * 60.0) as u32);
                let b = hash(seed.wrapping_add(0x9e37_79b9) ^ (t * 60.0) as u32);
                let x = (a as f32 / u32::MAX as f32) * 2.0 - 1.0;
                let y = (b as f32 / u32::MAX as f32) * 2.0 - 1.0;
                GlyphFx {
                    dx: amount * height * 0.02 * x,
                    dy: amount * height * 0.02 * y,
                    ..GlyphFx::default()
                }
            }
            TextEffect::ColorCycle => GlyphFx {
                hue: t * amount + index as f32 * 0.08,
                ..GlyphFx::default()
            },
            TextEffect::ScalePulse => {
                let phase = t * std::f32::consts::TAU + index as f32 * 0.3;
                GlyphFx { scale: 1.0 + amount * 0.35 * phase.sin(), ..GlyphFx::default() }
            }
        }
    }
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

impl TextConfig {
    /// Parses a graph node's free-form config, falling back to defaults on
    /// anything unreadable. A text node always draws something.
    pub fn from_json(json: &str) -> TextConfig {
        serde_json::from_str(json).unwrap_or_default()
    }

    fn family(&self) -> Family<'_> {
        match self.family.as_str() {
            "serif" => Family::Serif,
            "monospace" => Family::Monospace,
            "cursive" => Family::Cursive,
            "fantasy" => Family::Fantasy,
            "sans-serif" | "" => Family::SansSerif,
            other => Family::Name(other),
        }
    }

    fn attrs(&self) -> Attrs<'_> {
        let style = if self.style.eq_ignore_ascii_case("italic") {
            Style::Italic
        } else {
            Style::Normal
        };
        Attrs::new()
            .family(self.family())
            .weight(Weight(self.weight.clamp(1, 1000)))
            .style(style)
    }
}

thread_local! {
    /// Fonts are expensive to open and the same for every text node in a
    /// frame, so they are opened once per thread and reused.
    static FONTS: RefCell<(FontSystem, SwashCache)> =
        RefCell::new((FontSystem::new(), SwashCache::new()));
}

/// One laid-out glyph with its `fx.text.*` effects folded in: what both the
/// CPU rasterizer below and the GPU painter (`crate::text_gpu`) draw.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlacedGlyph {
    /// The glyph's raster key (font, id, size, subpixel bin).
    pub key: cosmic_text::CacheKey,
    /// Pen position in frame pixels, rounded (the image's `left`/`top`
    /// placement is relative to it).
    pub gx: i32,
    pub gy: i32,
    /// `scale_pulse`'s factor about the glyph's centre.
    pub scale: f32,
    pub rgb: [u8; 3],
    /// The effects' alpha (typewriter) — the stroke's mask uses this alone;
    /// the fill multiplies it by [`TextPlan::fill_alpha`].
    pub fx_alpha: f32,
}

/// A whole frame of text, resolved: layout, effects, box and stroke. Pure
/// data, so the CPU and GPU paths cannot disagree on *where*, only on how a
/// pixel is blended.
pub(crate) struct TextPlan {
    pub glyphs: Vec<PlacedGlyph>,
    /// `color.a × (0.4 + 0.6 · sig_level)`.
    pub fill_alpha: f32,
    /// `(x0, y0, x1, y1, radius, rgba)` in frame pixels.
    pub border: Option<(f32, f32, f32, f32, f32, [u8; 4])>,
    /// Dilation radius in pixels and colour.
    pub stroke: Option<(i32, [u8; 4])>,
}

/// Lays `config` out on a `w × h` frame. `None` when there is nothing to draw
/// (the background alone).
pub(crate) fn plan(
    config: &TextConfig,
    row: &[f32],
    time: f32,
    w: u32,
    h: u32,
    font_system: &mut FontSystem,
) -> Option<TextPlan> {
    let text = config.content.trim_end_matches('\n');
    if text.is_empty() {
        return None;
    }
    // Reactive knobs: hue from `sig_hue`, shake from `sig_level`. The signal
    // block sits before the effects block, so `read_signal` needs a row that
    // carries both; a short row is not an error.
    let (sig_hue, sig_level) = if row.len() >= metal_schema::EFFECTS_WIDTH + metal_schema::SIGNAL_WIDTH {
        let s = read_signal(row);
        (s.hue, s.level)
    } else {
        (0.0, 0.0)
    };

    let font_size = (config.size.clamp(0.02, 0.9) * h as f32).max(1.0);
    let line_height = (font_size * config.leading.max(0.5)).max(1.0);
    let margin = config.margin.clamp(0.0, 0.4) * h as f32;
    let inner_w = (w as f32 - margin * 2.0).max(1.0);
    let inner_h = (h as f32 - margin * 2.0).max(1.0);

    let mut buffer = Buffer::new(font_system, Metrics::new(font_size, line_height));
    let wrap_w = if config.line_wrap { Some(inner_w) } else { None };
    buffer.set_size(wrap_w, Some(inner_h));
    buffer.set_text(text, &config.attrs(), Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);

    // Block bounds, for the vertical placement and the border box.
    let mut text_h = 0.0f32;
    let (mut x0, mut x1) = (f32::MAX, f32::MIN);
    for run in buffer.layout_runs() {
        text_h = text_h.max(run.line_top + run.line_height);
        let lx = line_x(config.align, margin, inner_w, run.line_w);
        x0 = x0.min(lx);
        x1 = x1.max(lx + run.line_w);
    }
    let oy = match config.valign {
        VAlign::Top => margin,
        VAlign::Center => margin + (inner_h - text_h) * 0.5,
        VAlign::Bottom => margin + inner_h - text_h,
    };

    let bc = to_rgba(config.border.color);
    let border = (bc[3] > 0 && x1 > x0).then(|| {
        let pad = config.border.padding.clamp(0.0, 0.5) * h as f32;
        let radius = config.border.radius.clamp(0.0, 0.5) * h as f32;
        (x0 - pad, oy - pad, x1 + pad, oy + text_h + pad, radius, bc)
    });

    let stroke_px = config.stroke.width.clamp(0.0, 0.5) * font_size;
    let sc = to_rgba(config.stroke.color);
    let stroke = (stroke_px >= 0.5 && sc[3] > 0).then(|| (stroke_px.round() as i32, sc));

    let count: usize = buffer.layout_runs().map(|run| run.glyphs.len()).sum();
    let mut glyphs = Vec::with_capacity(count);
    let mut index = 0usize;
    for run in buffer.layout_runs() {
        let lx = line_x(config.align, margin, inner_w, run.line_w);
        for glyph in run.glyphs.iter() {
            let physical = glyph.physical((0.0, 0.0), 1.0);
            let mut fx = GlyphFx::default();
            for step in &config.effects {
                if let Some(effect) = TextEffect::parse(&step.kind) {
                    let one = effect.apply(
                        index,
                        count,
                        time,
                        step.amount,
                        glyph.glyph_id as u32 ^ (index as u32).wrapping_mul(2654435761),
                        h as f32,
                    );
                    fx.dx += one.dx;
                    fx.dy += one.dy;
                    fx.scale *= one.scale;
                    fx.alpha *= one.alpha;
                    fx.hue += one.hue;
                }
            }
            glyphs.push(PlacedGlyph {
                key: physical.cache_key,
                gx: (lx + physical.x as f32 + fx.dx).round() as i32,
                gy: (oy + run.line_y + physical.y as f32 + fx.dy).round() as i32,
                scale: fx.scale,
                rgb: shift_hue(config.color, sig_hue + fx.hue),
                fx_alpha: fx.alpha.clamp(0.0, 1.0),
            });
            index += 1;
        }
    }
    Some(TextPlan {
        glyphs,
        fill_alpha: config.color[3] * (0.4 + 0.6 * sig_level),
        border,
        stroke,
    })
}

/// Renders `config` to an RGBA8 frame of `w × h`.
///
/// `row` is the node's float row; the `sig_*` block drives the reactive
/// knobs (hue from `sig_hue`, shake from `sig_level`, …) exactly as it does
/// for every other model. A row too short for the signal block just leaves
/// the reactive knobs at their config values.
///
/// This is the `Stand`/wasm path and the reference the GPU painter's output
/// (`crate::text_gpu`) is diffed against.
pub fn render(config: &TextConfig, row: &[f32], time: f32, w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let (w, h) = (w.max(1), h.max(1));
    let mut frame = vec![0u8; (w as usize) * (h as usize) * 4];
    let bg = to_rgba(config.background);
    if bg[3] > 0 {
        for px in frame.chunks_exact_mut(4) {
            px.copy_from_slice(&bg);
        }
    }

    FONTS.with(|cell| {
        let (font_system, swash) = &mut *cell.borrow_mut();
        let Some(plan) = plan(config, row, time, w, h, font_system) else {
            return;
        };
        if let Some((x0, y0, x1, y1, radius, rgba)) = plan.border {
            rounded_rect(&mut frame, w, h, x0, y0, x1, y1, radius, rgba);
        }
        if let Some((r, stroke)) = plan.stroke {
            let mut fill = vec![0u8; (w as usize) * (h as usize)];
            draw_glyphs(&plan, font_system, swash, &mut Target::Mask(&mut fill), w, h);
            let ring = dilate(&fill, w, h, r);
            let sa = stroke[3] as f32 / 255.0;
            for (i, (&d, &f)) in ring.iter().zip(fill.iter()).enumerate() {
                let a = d.saturating_sub(f);
                if a == 0 {
                    continue;
                }
                let x = (i % w as usize) as i32;
                let y = (i / w as usize) as i32;
                blend(&mut frame, w, h, x, y, [stroke[0], stroke[1], stroke[2], scale_alpha(a, sa)]);
            }
        }
        draw_glyphs(&plan, font_system, swash, &mut Target::Frame(&mut frame), w, h);
    });

    (frame, w, h)
}

pub(crate) fn line_x(align: Align, margin: f32, inner_w: f32, line_w: f32) -> f32 {
    match align {
        Align::Left => margin,
        Align::Center => margin + (inner_w - line_w) * 0.5,
        Align::Right => margin + (inner_w - line_w),
    }
}

/// Where glyph pixels land: the RGBA frame (blended), or a one-byte coverage
/// mask (max of alpha) the stroke dilates.
enum Target<'a> {
    Frame(&'a mut [u8]),
    Mask(&'a mut [u8]),
}

impl Target<'_> {
    fn put(&mut self, w: u32, h: u32, x: i32, y: i32, rgba: [u8; 4]) {
        match self {
            Target::Frame(frame) => blend(frame, w, h, x, y, rgba),
            Target::Mask(mask) => {
                if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                    return;
                }
                let i = y as usize * w as usize + x as usize;
                mask[i] = mask[i].max(rgba[3]);
            }
        }
    }
}

fn draw_glyphs(
    plan: &TextPlan,
    font_system: &mut FontSystem,
    swash: &mut SwashCache,
    target: &mut Target<'_>,
    w: u32,
    h: u32,
) {
    for g in &plan.glyphs {
        // The stroke's mask is the glyph's shape, not its dimmed fill.
        let alpha = if matches!(target, Target::Mask(_)) {
            g.fx_alpha
        } else {
            (plan.fill_alpha * g.fx_alpha).clamp(0.0, 1.0)
        };
        let color = g.rgb;
        let (gx, gy) = (g.gx, g.gy);
        if let Some(image) = swash.get_image(font_system, g.key) {
            let (top, left) = (image.placement.top, image.placement.left);
            let (gw, gh) = (image.placement.width, image.placement.height);
            let data = &image.data;
            let mask = matches!(image.content, cosmic_text::SwashContent::Mask);
            for sy in 0..gh as i32 {
                for sx in 0..gw as i32 {
                    let si = (sy as usize) * gw as usize + sx as usize;
                    let mut rgba = [color[0], color[1], color[2], 0u8];
                    if mask {
                        if data.get(si).copied().unwrap_or(0) == 0 {
                            continue;
                        }
                        rgba[3] = scale_alpha(data[si], alpha);
                    } else {
                        let i = si * 4;
                        let (Some(&r), Some(&g), Some(&b), Some(&a)) =
                            (data.get(i), data.get(i + 1), data.get(i + 2), data.get(i + 3))
                        else {
                            continue;
                        };
                        rgba = [r, g, b, scale_alpha(a, alpha)];
                    }
                    if rgba[3] == 0 {
                        continue;
                    }
                    let dx = (gx + left + sx) as f32;
                    let dy = (gy - top + sy) as f32;
                    if (g.scale - 1.0).abs() < 0.001 {
                        target.put(w, h, dx.round() as i32, dy.round() as i32, rgba);
                    } else {
                        let cx = (gx + left) as f32 + gw as f32 * 0.5;
                        let cy = (gy - top) as f32 + gh as f32 * 0.5;
                        let s = g.scale.clamp(0.05, 8.0);
                        let px = (cx + (dx - cx) * s).round() as i32;
                        let py = (cy + (dy - cy) * s).round() as i32;
                        // Fill the scaled footprint rather than leave gaps.
                        let reach = (s.ceil() as i32).max(1);
                        for j in -reach..=reach {
                            for i in -reach..=reach {
                                target.put(w, h, px + i, py + j, rgba);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Max filter of radius `r` over a coverage mask: separable (a square
/// structuring element), O(1) per pixel via a monotonic deque, restricted to
/// the coverage bounding box grown by `r`.
fn dilate(mask: &[u8], w: u32, h: u32, r: i32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let r = r.max(0) as usize;
    // Bounding box of coverage, grown by r.
    let (mut bx0, mut by0, mut bx1, mut by1) = (w, h, 0usize, 0usize);
    for y in 0..h {
        let row = &mask[y * w..(y + 1) * w];
        if let Some(first) = row.iter().position(|&a| a > 0) {
            let last = row.iter().rposition(|&a| a > 0).unwrap_or(first);
            bx0 = bx0.min(first);
            bx1 = bx1.max(last + 1);
            by0 = by0.min(y);
            by1 = by1.max(y + 1);
        }
    }
    let mut out = vec![0u8; w * h];
    if bx1 <= bx0 {
        return out;
    }
    let (ex0, ex1) = (bx0.saturating_sub(r), (bx1 + r).min(w));
    let (ey0, ey1) = (by0.saturating_sub(r), (by1 + r).min(h));
    // Horizontal running max (monotonic deque), rows that hold coverage.
    let mut tmp = vec![0u8; w * h];
    let mut dq: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for y in by0..by1 {
        let row = &mask[y * w..(y + 1) * w];
        running_max(|x| row[x], ex0, ex1, bx0, bx1, r, &mut dq, |x, v| tmp[y * w + x] = v);
    }
    // Vertical: van Herk / Gil-Werman over whole row slices (block prefix
    // and suffix maxima), so every step is a contiguous, vectorisable max.
    let k = 2 * r + 1;
    let (cx0, cx1) = (ex0, ex1);
    let cw = cx1 - cx0;
    let rows = ey1 - ey0;
    let src = |y: usize| &tmp[(ey0 + y) * w + cx0..(ey0 + y) * w + cx1];
    let mut g = vec![0u8; rows * cw];
    let mut hh = vec![0u8; rows * cw];
    for y in 0..rows {
        let (prev, cur) = g.split_at_mut(y * cw);
        let cur = &mut cur[..cw];
        cur.copy_from_slice(src(y));
        if y % k != 0 {
            for (c, p) in cur.iter_mut().zip(&prev[(y - 1) * cw..]) {
                *c = (*c).max(*p);
            }
        }
    }
    for y in (0..rows).rev() {
        let (cur, next) = hh.split_at_mut((y + 1) * cw);
        let cur = &mut cur[y * cw..];
        cur.copy_from_slice(src(y));
        if (y + 1) % k != 0 && y + 1 < rows {
            for (c, n) in cur.iter_mut().zip(&next[..cw]) {
                *c = (*c).max(*n);
            }
        }
    }
    for y in 0..rows {
        let lo = y.saturating_sub(r);
        let hi = (y + r).min(rows - 1);
        let dst = &mut out[(ey0 + y) * w + cx0..(ey0 + y) * w + cx1];
        let a = &hh[lo * cw..lo * cw + cw];
        let b = &g[hi * cw..hi * cw + cw];
        for ((d, &x), &z) in dst.iter_mut().zip(a).zip(b) {
            *d = x.max(z);
        }
    }
    out
}

/// Sliding-window max of `get` over `[lo, hi)` with window `±r`, reading
/// only `[src_lo, src_hi)` (outside reads as zero).
#[allow(clippy::too_many_arguments)]
fn running_max(
    get: impl Fn(usize) -> u8,
    lo: usize,
    hi: usize,
    src_lo: usize,
    src_hi: usize,
    r: usize,
    dq: &mut std::collections::VecDeque<usize>,
    mut set: impl FnMut(usize, u8),
) {
    dq.clear();
    let mut next = src_lo;
    for x in lo..hi {
        let reach = (x + r + 1).min(src_hi);
        while next < reach {
            let v = get(next);
            while dq.back().is_some_and(|&b| get(b) <= v) {
                dq.pop_back();
            }
            dq.push_back(next);
            next += 1;
        }
        while dq.front().is_some_and(|&f| f + r < x) {
            dq.pop_front();
        }
        if let Some(&f) = dq.front() {
            set(x, get(f));
        }
    }
}

/// Fills a rounded rectangle, anti-aliased on the corners.
#[allow(clippy::too_many_arguments)]
fn rounded_rect(frame: &mut [u8], w: u32, h: u32, x0: f32, y0: f32, x1: f32, y1: f32, radius: f32, rgba: [u8; 4]) {
    let radius = radius.min((x1 - x0) * 0.5).min((y1 - y0) * 0.5).max(0.0);
    let (ix0, iy0) = (x0.floor().max(0.0) as i32, y0.floor().max(0.0) as i32);
    let (ix1, iy1) = (x1.ceil().min(w as f32) as i32, y1.ceil().min(h as f32) as i32);
    for y in iy0..iy1 {
        for x in ix0..ix1 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // Signed distance to the rounded box (negative inside).
            let (hx, hy) = ((x1 - x0) * 0.5 - radius, (y1 - y0) * 0.5 - radius);
            let qx = (px - (x0 + x1) * 0.5).abs() - hx;
            let qy = (py - (y0 + y1) * 0.5).abs() - hy;
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            let d = outside + qx.max(qy).min(0.0) - radius;
            let cover = (0.5 - d).clamp(0.0, 1.0);
            if cover <= 0.0 {
                continue;
            }
            blend(frame, w, h, x, y, [rgba[0], rgba[1], rgba[2], scale_alpha(rgba[3], cover)]);
        }
    }
}

fn scale_alpha(a: u8, mul: f32) -> u8 {
    ((a as f32) * mul).clamp(0.0, 255.0).round() as u8
}

pub(crate) fn to_rgba(c: [f32; 4]) -> [u8; 4] {
    [
        (c[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

/// Straight-alpha source-over of one pixel.
fn blend(frame: &mut [u8], w: u32, h: u32, x: i32, y: i32, rgba: [u8; 4]) {
    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
        return;
    }
    let i = ((y as usize) * w as usize + x as usize) * 4;
    let a = rgba[3] as u32;
    for c in 0..3 {
        frame[i + c] = ((rgba[c] as u32 * a + frame[i + c] as u32 * (255 - a)) / 255) as u8;
    }
    let da = frame[i + 3] as u32;
    frame[i + 3] = (a + da * (255 - a) / 255).min(255) as u8;
}

fn shift_hue(color: [f32; 4], turns: f32) -> [u8; 3] {
    let [r, g, b, _] = color;
    if turns.abs() < 1e-4 {
        return [
            (r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (b.clamp(0.0, 1.0) * 255.0).round() as u8,
        ];
    }
    // Rotate in HSV, then back to RGB.
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let v = max;
    let d = max - min;
    let s = if max <= 0.0 { 0.0 } else { d / max };
    let mut h = if d <= 1e-6 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h = (h / 6.0 + turns).rem_euclid(1.0);
    let hi = (h * 6.0).floor();
    let f = h * 6.0 - hi;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match hi as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    ]
}

/// The `fx.text.*` vocabulary as data, for the catalog and for the test that
/// keeps this module in step with `metal_schema::TEXT_EFFECT_NAMES`.
pub fn effect_names() -> &'static [&'static str] {
    TEXT_EFFECT_NAMES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_vocabulary_effect_has_an_implementation() {
        // The closed vocabulary is declared once in `metal-schema`; this is the
        // test that a name never becomes a silent no-op.
        for name in TEXT_EFFECT_NAMES {
            assert!(
                TextEffect::parse(name).is_some(),
                "fx.text.{name} has no implementation"
            );
        }
        assert_eq!(effect_names(), TEXT_EFFECT_NAMES);
    }

    #[test]
    fn a_config_round_trips_and_defaults_a_short_json() {
        let config = TextConfig::from_json(r#"{"content":"hola"}"#);
        assert_eq!(config.content, "hola");
        assert_eq!(config.size, TextConfig::default().size);
        assert_eq!(TextConfig::from_json("not json").content, "MetalVJ");
    }

    #[test]
    fn render_paints_the_text_and_is_deterministic() {
        let config = TextConfig {
            content: "FW".into(),
            color: [1.0, 1.0, 1.0, 1.0],
            ..TextConfig::default()
        };
        let (a, w, h) = render(&config, &[], 0.0, 320, 180);
        let (b, _, _) = render(&config, &[], 0.0, 320, 180);
        assert_eq!(w, 320);
        assert_eq!(h, 180);
        assert_eq!(a.len(), 320 * 180 * 4);
        assert_eq!(a, b, "same config and time must be byte-identical");
        let lit = a.chunks_exact(4).filter(|px| px[3] > 0).count();
        assert!(lit > 0, "two letters should leave a mark");
    }

    #[test]
    fn an_empty_string_is_the_background() {
        let config = TextConfig {
            content: String::new(),
            background: [1.0, 0.0, 0.0, 1.0],
            ..TextConfig::default()
        };
        let (frame, _, _) = render(&config, &[], 0.0, 8, 8);
        assert!(frame.chunks_exact(4).all(|px| px == [255, 0, 0, 255]));
    }

    #[test]
    fn the_typewriter_reveals_a_prefix() {
        let effect = TextEffect::Typewriter;
        assert_eq!(effect.apply(0, 10, 0.0, 0.5, 0, 100.0).alpha, 1.0);
        assert_eq!(effect.apply(8, 10, 0.0, 0.5, 0, 100.0).alpha, 0.0);
        assert_eq!(effect.apply(9, 10, 0.0, 1.0, 0, 100.0).alpha, 1.0);
    }

    #[test]
    fn the_wave_moves_glyphs_apart_in_time() {
        let wave = TextEffect::Wave;
        let a = wave.apply(3, 10, 0.0, 1.0, 0, 200.0);
        let b = wave.apply(3, 10, 0.25, 1.0, 0, 200.0);
        assert!(a.dy.abs() <= 200.0 * 0.06, "the wave stays inside its amplitude");
        assert!((a.dy - b.dy).abs() > 1e-6, "a quarter period changes the offset");
    }

    #[test]
    fn jitter_is_stable_at_a_paused_frame() {
        let jitter = TextEffect::Jitter;
        let a = jitter.apply(2, 5, 1.0, 1.0, 42, 100.0);
        let b = jitter.apply(2, 5, 1.0, 1.0, 42, 100.0);
        assert_eq!(a.dx, b.dx);
        assert_eq!(a.dy, b.dy);
    }

    fn lit(frame: &[u8]) -> usize {
        frame.chunks_exact(4).filter(|px| px[3] > 0).count()
    }

    fn rows_lit(frame: &[u8], w: usize) -> (usize, usize) {
        let rows: Vec<usize> = frame
            .chunks_exact(w * 4)
            .enumerate()
            .filter(|(_, row)| row.chunks_exact(4).any(|px| px[3] > 0))
            .map(|(y, _)| y)
            .collect();
        (rows[0], *rows.last().unwrap())
    }

    #[test]
    fn an_old_node_loads_with_the_new_defaults() {
        let config = TextConfig::from_json(r#"{"content":"x","align":"center"}"#);
        assert_eq!(config.valign, VAlign::Top);
        assert_eq!(config.stroke, Stroke::default());
        assert_eq!(config.border, Border::default());
        assert!(config.line_wrap);
        let config = TextConfig::from_json(
            r#"{"valign":"bottom","stroke":{"width":0.1},"border":{"color":[1,0,0,1]},"line_wrap":false}"#,
        );
        assert_eq!(config.valign, VAlign::Bottom);
        assert_eq!(config.stroke.width, 0.1);
        assert_eq!(config.stroke.color, Stroke::default().color);
        assert_eq!(config.border.color, [1.0, 0.0, 0.0, 1.0]);
        assert!(!config.line_wrap);
    }

    #[test]
    fn a_zero_stroke_is_byte_identical_to_none() {
        let base = TextConfig { content: "Ag".into(), ..TextConfig::default() };
        let zero = TextConfig {
            stroke: Stroke { width: 0.0, color: [1.0, 0.0, 0.0, 1.0] },
            ..base.clone()
        };
        assert_eq!(render(&base, &[], 0.3, 200, 120).0, render(&zero, &[], 0.3, 200, 120).0);
    }

    #[test]
    fn a_stroke_rings_the_glyphs_in_its_colour() {
        let base = TextConfig { content: "O".into(), size: 0.5, ..TextConfig::default() };
        let stroked = TextConfig {
            stroke: Stroke { width: 0.08, color: [1.0, 0.0, 0.0, 1.0] },
            ..base.clone()
        };
        let (a, _, _) = render(&base, &[], 0.0, 200, 200);
        let (b, _, _) = render(&stroked, &[], 0.0, 200, 200);
        assert!(lit(&b) > lit(&a), "the ring adds coverage");
        assert!(b.chunks_exact(4).any(|px| px == [255, 0, 0, 255]), "pure stroke pixels");
        // Fully covered fill pixels stay the fill colour (ring = dilate − fill).
        let full = a.chunks_exact(4).map(|px| px[3]).max().unwrap();
        for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
            if pa[3] == full {
                assert_eq!(pb, pa);
            }
        }
    }

    #[test]
    fn the_border_box_sits_behind_the_text_and_rounds_its_corners() {
        let base = TextConfig { content: "HI".into(), size: 0.3, margin: 0.2, ..TextConfig::default() };
        let boxed = TextConfig {
            border: Border { padding: 0.05, color: [0.0, 0.0, 1.0, 1.0], radius: 0.0 },
            ..base.clone()
        };
        let round = TextConfig {
            border: Border { radius: 0.05, ..boxed.border.clone() },
            ..base.clone()
        };
        let (a, _, _) = render(&base, &[], 0.0, 200, 200);
        let (b, _, _) = render(&boxed, &[], 0.0, 200, 200);
        let (c, _, _) = render(&round, &[], 0.0, 200, 200);
        assert!(b.chunks_exact(4).any(|px| px == [0, 0, 255, 255]));
        assert!(b.chunks_exact(4).any(|px| px[0] > 50 && px[2] == 255), "text over the box");
        assert_ne!(a, b);
        assert!(lit(&c) < lit(&b), "rounded corners cover less");
        // Alpha 0 is no box at all.
        let none = TextConfig { border: Border { color: [0.0, 0.0, 1.0, 0.0], ..boxed.border.clone() }, ..base.clone() };
        assert_eq!(render(&none, &[], 0.0, 200, 200).0, a);
    }

    #[test]
    fn valign_moves_the_block_down_the_frame() {
        let at = |valign| {
            let config = TextConfig { content: "Hx".into(), size: 0.2, valign, ..TextConfig::default() };
            rows_lit(&render(&config, &[], 0.0, 200, 200).0, 200)
        };
        let (top, center, bottom) = (at(VAlign::Top), at(VAlign::Center), at(VAlign::Bottom));
        assert!(top.0 < center.0 && center.0 < bottom.0, "{top:?} {center:?} {bottom:?}");
        assert!(bottom.1 < 200 - (0.06 * 200.0) as usize + 2, "bottom respects the margin");
    }

    #[test]
    fn line_wrap_off_keeps_one_line() {
        let long = "word word word word word word word word";
        let wrapped = TextConfig { content: long.into(), size: 0.1, ..TextConfig::default() };
        let single = TextConfig { line_wrap: false, ..wrapped.clone() };
        let (a, b) = (
            rows_lit(&render(&wrapped, &[], 0.0, 200, 200).0, 200),
            rows_lit(&render(&single, &[], 0.0, 200, 200).0, 200),
        );
        assert!(a.1 - a.0 > (b.1 - b.0) * 2, "wrapped {a:?} vs single {b:?}");
    }
}

#[cfg(test)]
mod measure {
    use super::*;

    /// `cargo test -p metal-perform --release text::measure -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn render_ms_per_frame() {
        let still = TextConfig { content: "METAL VJ\nlive".into(), ..TextConfig::default() };
        let wave = TextConfig {
            effects: vec![TextEffectStep { kind: "fx.text.wave".into(), amount: 1.0 }],
            ..still.clone()
        };
        let stroked = TextConfig {
            stroke: Stroke { width: 0.06, color: [0.0, 0.0, 0.0, 1.0] },
            border: Border { padding: 0.02, color: [0.1, 0.1, 0.1, 0.8], radius: 0.02 },
            ..wave.clone()
        };
        for (name, cfg) in [("static", &still), ("wave", &wave), ("wave+stroke+border", &stroked)] {
            for (w, h) in [(1920u32, 1080u32), (3840, 2160)] {
                let _ = render(cfg, &[], 0.0, w, h);
                let n = 30;
                let t0 = std::time::Instant::now();
                for i in 0..n {
                    std::hint::black_box(render(cfg, &[], i as f32 / 60.0, w, h));
                }
                let ms = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
                println!("MEASURE {name} {w}x{h}: {ms:.2} ms/frame");
            }
        }
    }
}
